use super::runtime::{Engine, EngineState, RecordingMode};
use crate::audio::waveform::remove_waveform_cache;
use crate::project::ProjectFile;
use crate::utils::stats::get_wav_duration;
use crate::utils::transcription::TranscriptionTask;
use anyhow::{Context, Result};
use log::{debug, info, warn};
use std::fs;
use std::path::Path;

impl Engine {
    /// Начинает запись с опциональной длительностью.
    /// replace_current: в режиме A при true (r) заменяем текущий отрезок, при false (e) добавляем после.
    pub(crate) fn start_recording_with_duration(
        &mut self,
        duration_secs: Option<u64>,
        replace_current: bool,
    ) -> Result<()> {
        if self.debug {
            debug!(
                "Starting recording (duration: {:?}, replace_current: {}, mode: {:?})",
                duration_secs, replace_current, self.recording_mode
            );
        }
        if let Some(dur) = duration_secs {
            info!("Starting recording for {} seconds", dur);
        } else {
            info!("Starting recording");
        }

        if let EngineState::Playing { .. } = &self.state {
            self.stop_playback()?;
        }

        if let EngineState::Recording { .. } = &self.state {
            self.stop_recording()?;
        }

        let current_idx = self
            .current_index
            .unwrap_or_else(|| self.project.files.len().saturating_sub(1));
        let mut project = self.project.clone();
        let mut current_index = Some(current_idx);
        let mut update_recording_index = None;
        let mut insert_recording_index = None;

        // Reserve the next creation number before editing the accepted tail.
        fs::create_dir_all(&self.chunks_dir)?;
        let path = project.get_next_file_path(&self.chunks_dir)?;
        let mut removed_paths = Vec::new();
        match self.recording_mode {
            RecordingMode::Append => {
                // Deliberately preserve the narrator's irreversible tail-overwrite workflow.
                let remove_from_idx = if replace_current {
                    current_idx
                } else {
                    current_idx + 1
                };
                if remove_from_idx < project.files.len() {
                    removed_paths = project.remove_files_from_index(remove_from_idx);
                }
                current_index = None;
            }
            RecordingMode::Update => {
                anyhow::ensure!(
                    current_idx < project.files.len(),
                    "No fragment selected for replacement"
                );
                update_recording_index = Some(current_idx);
            }
            RecordingMode::Insert => {
                let insert_at = if project.files.is_empty() {
                    0
                } else {
                    current_idx + 1
                };
                anyhow::ensure!(
                    insert_at <= project.files.len(),
                    "Insertion position is no longer available"
                );
                insert_recording_index = Some(insert_at);
            }
        }
        // Persist the high-water mark before recording, including rejected/failed takes.
        if let Err(error) = project.save(&self.project_path) {
            // The rename may have succeeded before directory sync failed. Never reuse its
            // reserved number, but do not publish a failed tail deletion in memory.
            self.project.next_chunk_number = self
                .project
                .next_chunk_number
                .max(project.next_chunk_number);
            return Err(error.context(format!(
                "Could not save the project before recording to {:?}; no existing WAV was removed",
                path
            )));
        }
        // Invalidate in-flight graphs before publishing a different selection or removing WAVs,
        // including the error path where the audio device refuses to start.
        self.clear_waveform_requests();
        self.waveform_current.clear();
        self.waveform_prev.clear();
        self.publish_waveforms();
        self.project = project;
        self.current_index = current_index;
        for file_path in &removed_paths {
            let old_path = self.resolve_file_path(file_path);
            if let Err(error) = fs::remove_file(&old_path) {
                if error.kind() != std::io::ErrorKind::NotFound {
                    self.report_error(&anyhow::Error::new(error).context(format!(
                        "Failed to remove overwritten recording: {}",
                        file_path
                    )));
                }
            }
            remove_waveform_cache(&old_path).ok();
        }

        let recording = self
            .audio
            .start_recording(&path, self.level_tx.clone())
            .with_context(|| {
                format!(
                    "Could not start recording; an incomplete WAV may remain at {:?}",
                    path
                )
            });
        let session = match recording {
            Ok(recording) => recording,
            Err(error) => {
                self.update_current_waveform();
                self.update_prev_waveform();
                self.update_projection_after_change();
                return Err(error);
            }
        };
        self.update_recording_index = update_recording_index;
        self.insert_recording_index = insert_recording_index;

        self.state = EngineState::Recording {
            session,
            writer_path: path.clone(),
            start_time: self.now(),
            duration_secs,
        };
        self.update_prev_waveform();

        self.refresh_projection(true, false);

        Ok(())
    }

    /// Finalize the WAV before registering it; there is no background write to project metadata.
    pub(crate) fn finish_recording(&mut self) -> Result<()> {
        if !matches!(self.state, EngineState::Recording { .. }) { return Ok(()); }
        if let EngineState::Recording {
            session,
            writer_path,
            ..
        } = std::mem::replace(&mut self.state, EngineState::Idle)
        {
            let file_path = crate::utils::paths::stored_recording_path(
                self.project_path.parent().unwrap_or(Path::new(".")),
                &writer_path,
            )?;
            let completion_result = (|| -> Result<()> {
                session.finish()?;
                let size = fs::metadata(&writer_path)?.len();
                let duration_ms = get_wav_duration(&writer_path)?.as_millis() as u64;
                let mut replaced_path = None;
                let mut project = self.project.clone();
                let current_index;

                let file = ProjectFile {
                    path: file_path.clone(),
                    title: String::new(),
                    author: String::new(),
                    year: String::new(),
                    hint: String::new(),
                    markers: Vec::new(),
                    size,
                    duration_ms,
                };

                match self.recording_mode {
                    RecordingMode::Append => {
                        project.files.push(file);
                        current_index = Some(project.files.len() - 1);
                    }
                    RecordingMode::Update => {
                        let idx = self
                            .update_recording_index
                            .context("No replacement target")?;
                        anyhow::ensure!(
                            idx < project.files.len(),
                            "Replacement target is no longer available"
                        );
                        let old = &project.files[idx];
                        let old_path = old.path.clone();
                        let title = old.title.clone();
                        let author = old.author.clone();
                        let year = old.year.clone();
                        let hint = old.hint.clone();
                        let markers = old.markers.clone();
                        replaced_path = Some(old_path);
                        project.files[idx] = ProjectFile {
                            path: file.path.clone(),
                            title,
                            author,
                            year,
                            hint,
                            markers,
                            size,
                            duration_ms,
                        };
                        current_index = self.update_recording_index;
                    }
                    RecordingMode::Insert => {
                        let insert_at = self
                            .insert_recording_index
                            .context("No insertion position")?;
                        anyhow::ensure!(
                            insert_at <= project.files.len(),
                            "Insertion position is no longer available"
                        );
                        project.files.insert(insert_at, file);
                        current_index = Some(insert_at);
                    }
                }

                project.stats.record_length =
                    project.stats.record_length.saturating_add(duration_ms);
                project.save(&self.project_path)?;
                self.project = project;
                self.current_index = current_index;
                self.recording_mode = RecordingMode::Append;

                if let Some(old_path) = replaced_path {
                    let path = self.resolve_file_path(&old_path);
                    if path != writer_path {
                        if let Err(error) = fs::remove_file(&path) {
                            warn!("Failed to remove replaced recording {:?}: {}", path, error);
                        }
                        remove_waveform_cache(&path).ok();
                    }
                }

                self.waveform_current.clear();
                self.update_current_waveform();
                self.update_prev_waveform();

                log::info!(
                    "[finish_recording] Recording stopped, file added/updated: {}",
                    file_path
                );

                if let Some(index) = self.current_index {
                    if let Err(error) = self.start_transcription_for_file(index) { self.report_error(&error); }
                }
                self.update_projection_after_change();
                Ok(())
            })();
            // A failed take is no longer recording. Retain its mode and selection for a
            // new attempt, but never carry a stale U/I target into that attempt.
            self.update_recording_index = None;
            self.insert_recording_index = None;
            if completion_result.is_err() {
                self.clear_waveform_requests();
                self.update_current_waveform();
                self.update_prev_waveform();
            }
            completion_result.with_context(|| format!(
                "Could not confirm recording completion. WAV kept at {:?}; no previously accepted WAV was removed while finishing",
                writer_path
            ))?;
        }
        Ok(())
    }

    /// Останавливает запись и удаляет временный файл. Длительность отменённого отрезка прибавляется к record_length.
    pub(crate) fn stop_recording(&mut self) -> Result<()> {
        if !matches!(self.state, EngineState::Recording { .. }) { return Ok(()); }
        if let EngineState::Recording {
            session,
            writer_path,
            start_time,
            ..
        } = std::mem::replace(&mut self.state, EngineState::Idle)
        {
            let elapsed_ms = self.elapsed_since(start_time).as_millis() as u64;
            self.project.stats.record_length =
                self.project.stats.record_length.saturating_add(elapsed_ms);
            let saved = self.project.save(&self.project_path)
                .context("Failed to save project after discarding recording");
            let finalized = session.finish();
            let removed = fs::remove_file(&writer_path);
            // Удаляем кеш для временного файла
            remove_waveform_cache(&writer_path).ok();

            self.update_recording_index = None;
            self.insert_recording_index = None;
            // Очищаем waveform для текущей записи
            self.waveform_current.clear();
            self.update_current_waveform();
            self.update_prev_waveform();

            self.refresh_projection(true, false);
            saved?;
            finalized?;
            removed.with_context(|| format!("Cannot remove discarded recording: {:?}", writer_path))?;

            if self.debug {
                debug!(
                    "Stopped recording, removed temporary file: {:?}, record_length +{} ms",
                    writer_path, elapsed_ms
                );
            }
        }
        Ok(())
    }

    /// Запускает распознавание речи для файла
    pub(crate) fn start_transcription_for_file(&mut self, file_index: usize) -> Result<()> {
        self.initialize_transcription()?;
        if let (Some(sender), Some(file)) = (&self.transcription_tx, self.project.files.get(file_index)) {
            let task = TranscriptionTask {
                file_path: self.resolve_file_path(&file.path),
                project_file_path: std::path::PathBuf::from(&file.path),
                previous_hint: file.hint.clone(),
            };
            sender.try_send(task).context("Cannot queue transcription")?;
        }

        Ok(())
    }

    fn initialize_transcription(&mut self) -> Result<()> {
        if self.transcription_attempted {
            return Ok(());
        }
        self.transcription_attempted = true;
        let model_path = match &self.transcription_model {
            Some(model_path) => model_path.clone(),
            None => Some(crate::utils::paths::models_dir()?.join("whisper.bin")),
        };
        if let Some(model_path) = model_path.filter(|path| path.is_file()) {
            let (task_tx, task_rx) = crossbeam_channel::bounded(8);
            let (update_tx, update_rx) = crossbeam_channel::bounded(8);
            self.transcription_worker = Some(crate::utils::transcription::start_transcription_worker(
                task_rx, update_tx, model_path, self.debug,
            ));
            self.transcription_tx = Some(task_tx);
            self.transcription_update_rx = update_rx;
        }

        Ok(())
    }
}
