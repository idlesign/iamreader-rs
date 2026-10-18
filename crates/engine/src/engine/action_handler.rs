use super::runtime::{Engine, EngineState, RecordingMode};
use super::command::{ChunkId, EngineCommand, ProjectEdit};
use super::event::CommandOutcome;
use crate::project::export_status::{ExportObserver, ExportOutcome, ExportUpdate};
use crate::project::dialog_edit::{
    persist_staged, save_chunk_dialog, save_marker_dialog, save_meta_dialog, DialogSaveOutcome,
};
use crate::project::markers::compute_effective_durations_ms;
use crate::utils::indexes::{find_section_ui_index};
use crate::utils::stats::get_wav_spec;
use anyhow::Result;
use log::{debug, info};
use std::path::Path;
use std::sync::Arc;
use std::time::Instant;

impl Engine {
    /// Обрабатывает действие пользователя
    pub(crate) fn handle_command(&mut self, action: EngineCommand) -> Result<CommandOutcome> {
        match action {
            EngineCommand::Record { duration_secs } => {
                // Если уже идёт запись — останавливаем и удаляем её, начинаем новую (не трогая сохранённые файлы).
                // Иначе — начинаем запись с заменой текущего файла.
                let was_recording = matches!(&self.state, EngineState::Recording { .. });
                self.start_recording_with_duration(duration_secs, !was_recording)?;
            }
            EngineCommand::Ok => {
                if matches!(&self.state, EngineState::Recording { .. }) {
                    let was_edit_mode = self.recording_mode == RecordingMode::Update
                        || self.recording_mode == RecordingMode::Insert;
                    self.finish_recording()?;
                    if !was_edit_mode {
                        self.start_recording_with_duration(None, false)?;
                    }
                } else {
                    self.start_recording_with_duration(None, false)?;
                }
            }
            EngineCommand::Stop => {
                // Останавливаем запись, если она идёт
                if matches!(&self.state, EngineState::Recording { .. }) {
                    self.stop_recording()?;
                }
                // Останавливаем воспроизведение, если оно идёт
                if matches!(&self.state, EngineState::Playing { .. }) {
                    self.stop_playback()?;
                }
            }
            EngineCommand::Prev => {
                // a - предыдущая (более старая запись) = увеличение UI индекса
                if let Some((current_orig, current_ui)) = self.current_orig_and_ui() {
                    if (current_ui as usize) + 1 < self.project.files.len() {
                        if self.debug {
                            debug!("EngineCommand::Prev: current_orig={}, current_ui={}, moving to ui_index={}", current_orig, current_ui, current_ui + 1);
                        }
                        self.goto_to_index(Some(current_ui + 1), true)?;
                    }
                }
            }
            EngineCommand::Next => {
                // f - следующая (более свежая запись) = уменьшение UI индекса
                if let Some((current_orig, current_ui)) = self.current_orig_and_ui() {
                    if current_ui > 0 {
                        if self.debug {
                            debug!("EngineCommand::Next: current_orig={}, current_ui={}, moving to ui_index={}", current_orig, current_ui, current_ui - 1);
                        }
                        self.goto_to_index(Some(current_ui - 1), true)?;
                    }
                }
            }
            EngineCommand::PrevSect => self.handle_prev_sect()?,
            EngineCommand::NextSect => self.handle_next_sect()?,
            EngineCommand::ModeUpdate => {
                if matches!(&self.state, EngineState::Recording { .. }) {
                    return Ok(CommandOutcome::Completed);
                }
                if !self.project.files.is_empty() {
                    self.recording_mode = if self.recording_mode == RecordingMode::Update {
                        RecordingMode::Append
                    } else {
                        self.update_recording_index = None;
                        RecordingMode::Update
                    };
                    if self.debug {
                        debug!("Recording mode: {:?}", self.recording_mode);
                    }
                    self.refresh_projection(true, false);
                }
            }
            EngineCommand::ModeInsert => {
                if matches!(&self.state, EngineState::Recording { .. }) {
                    return Ok(CommandOutcome::Completed);
                }
                if !self.project.files.is_empty() {
                    self.recording_mode = if self.recording_mode == RecordingMode::Insert {
                        RecordingMode::Append
                    } else {
                        self.insert_recording_index = None;
                        RecordingMode::Insert
                    };
                    if self.debug {
                        debug!("Recording mode: {:?}", self.recording_mode);
                    }
                    self.refresh_projection(true, false);
                }
            }
            EngineCommand::Play => self.start_playback()?,
            EngineCommand::Select { chunk, play } => {
                let index = chunk.as_ref().map(|chunk| self.find_chunk_index(chunk)).transpose()?;
                let display_index = index.map(|index| crate::utils::indexes::orig_to_ui_index(index, self.project.files.len()));
                self.goto_to_index(display_index, play)?;
            }
            EngineCommand::SearchHintUp(ref query) => self.search_hint(query, false)?,
            EngineCommand::SearchHintDown(ref query) => self.search_hint(query, true)?,
            EngineCommand::Shutdown => {
                self.shutdown()?;
                info!("Shutdown command received");
            }
            EngineCommand::StartMonitoring => self.audio.start_monitoring(self.level_tx.clone())?,
            EngineCommand::StopMonitoring => self.audio.stop_monitoring()?,
            EngineCommand::SaveProjectEdit(edit) => return self.handle_save_dialog(*edit),
            EngineCommand::AddMarker { marker } => {
                let marker_name = self.project.markers.iter().find(|(_, settings)| settings.shortcut.as_deref() == Some(marker.as_str()))
                    .map(|(name, _)| name.clone()).ok_or_else(|| anyhow::anyhow!("No marker found with shortcut: {marker}"))?;
                let index = self.current_orig_and_ui().map(|(index, _)| index).ok_or_else(|| anyhow::anyhow!("No recording selected"))?;
                let chunk = ChunkId(self.project.files[index].path.clone());
                let markers = self.project.files[index].markers.clone();
                let next = if markers.contains(&marker_name) {
                    markers.into_iter().filter(|name| name != &marker_name).collect()
                } else {
                    let mut next = markers; next.push(marker_name); next
                };
                self.save_markers(&chunk, next)?;
            }
            EngineCommand::AddMarkers { chunk, markers } => {
                let index = self.find_chunk_index(&chunk)?;
                let mut next = self.project.files[index].markers.clone();
                next.extend(markers);
                self.save_markers(&chunk, next)?;
            }
            EngineCommand::RemoveMarkers { chunk, markers } => {
                let index = self.find_chunk_index(&chunk)?;
                let next = self.project.files[index].markers.iter().filter(|marker| !markers.contains(marker)).cloned().collect();
                self.save_markers(&chunk, next)?;
            }
            EngineCommand::SetMarkers { chunk, markers } => self.save_markers(&chunk, markers)?,
            EngineCommand::Compile => self.handle_compile()?,
            EngineCommand::CompileCancel => {
                let mut status = self.export.lock().map_err(|_| anyhow::anyhow!("Export status lock poisoned"))?;
                if status.started && status.finished_at.is_none() && !status.publishing {
                    status.apply(ExportUpdate::Cancelling)?;
                    if let Some(cancel) = &self.compile_cancel {
                        cancel.store(true, std::sync::atomic::Ordering::Relaxed);
                    }
                }
            }
            EngineCommand::Transcribe { chunk } => {
                let index = self.find_chunk_index(&chunk)?;
                self.start_transcription_for_file(index)?;
            }
            EngineCommand::DeleteChunk { chunk } => self.handle_confirm_delete_chunk(chunk)?,
        }
        Ok(CommandOutcome::Completed)
    }

    fn save_markers(&mut self, chunk: &ChunkId, markers: Vec<String>) -> Result<()> {
        let index = self.find_chunk_index(chunk)?;
        let mut staged = self.project.clone();
        let file = &mut staged.files[index];
        file.markers = markers;
        crate::project::markers::normalize_markers(file);
        if file.title.is_empty() && file.markers.iter().any(|marker| staged.markers.get(marker).is_some_and(|settings| settings.section)) {
            file.title = format!("Title for {}", index + 1);
        }
        let outcome = persist_staged(&mut self.project, &self.project_path, staged)?;
        if let Some(warning) = outcome.warning { log::warn!("{warning}"); }
        self.update_projection_after_change();

        Ok(())
    }

    fn handle_save_dialog(&mut self, edit: ProjectEdit) -> Result<CommandOutcome> {
        let outcome = match edit {
            ProjectEdit::Meta(data) => save_meta_dialog(&mut self.project, &self.project_path, &data),
            ProjectEdit::Chunk { data, expected_path } => save_chunk_dialog(&mut self.project, &self.project_path, &data, &expected_path),
            ProjectEdit::Marker(data) => save_marker_dialog(&mut self.project, &self.project_path, &[data]),
            ProjectEdit::RefreshMetadata => self.handle_update_files_meta(),
        }?;
        self.update_projection_after_change();
        let mut snapshot = self.snapshot.lock().map_err(|_| anyhow::anyhow!("Engine snapshot lock poisoned"))?;
        snapshot.meta = self.project.meta.clone();
        snapshot.settings = self.project.settings.clone();
        snapshot.markers = self.project.markers.clone();
        snapshot.project_version = snapshot.project_version.wrapping_add(1);

        Ok(CommandOutcome::Saved { warning: outcome.warning })
    }

    fn handle_update_files_meta(&mut self) -> Result<DialogSaveOutcome> {
        let project_dir = self.project_path.parent().unwrap_or_else(|| Path::new("."));
        let mut staged = self.project.clone();
        staged.update_files_meta_from_disk(project_dir)?;
        let (sample_rate, channels) = staged
            .files
            .iter()
            .find_map(|file| {
                let path = crate::utils::paths::resolve_project_file(project_dir, &file.path);
                get_wav_spec(&path).ok()
            })
            .unwrap_or((44100, 2));
        let effective = compute_effective_durations_ms(
            &staged.files,
            &staged.markers,
            project_dir,
            sample_rate,
            channels,
        )?;
        for (file, duration_ms) in staged.files.iter_mut().zip(effective) {
            file.duration_ms = duration_ms;
        }
        let outcome = persist_staged(&mut self.project, &self.project_path, staged)?;
        info!("Files meta updated from disk");
        if !matches!(&self.state, EngineState::Recording { .. }) {
            // An explicit disk refresh must also retry/reload the same selected WAV.
            self.clear_waveform_requests();
            self.update_current_waveform();
            self.update_prev_waveform();
        }
        Ok(outcome)
    }

    fn handle_compile(&mut self) -> Result<()> {
        if self.compile_worker.as_ref().is_some_and(|worker| !worker.is_finished()) {
            anyhow::bail!("An export is already running");
        }
        if let Some(worker) = self.compile_worker.take() {
            worker.join().map_err(|_| anyhow::anyhow!("Previous export worker panicked"))?;
        }
        let project = self.project.clone();
        let project_path = self.project_path.clone();
        let export = self.export.clone();
        let observer: Arc<dyn ExportObserver> = if let Some(listener) = &self.export_listener {
            Arc::new(super::export_observer::ObservedExport { status: export.clone(), listener: listener.clone() })
        } else { export.clone() };
        let debug = self.debug;
        let cancel = Arc::new(std::sync::atomic::AtomicBool::new(false));
        crate::project::compiler::begin_compile(Some(observer.as_ref()), Some(&cancel))?;
        self.compile_cancel = Some(cancel.clone());
        let spawned = std::thread::Builder::new().name("export-worker".into()).spawn(move || {
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                crate::project::compiler::compile_wav_files_static(
                    project.files, project_path, project.markers, project.meta, project.settings,
                    Some(observer), Some(cancel), debug,
                )
            }));
            match result {
                Ok(Ok(())) => {}
                Ok(Err(error)) => log::error!("Export ended: {error:#}"),
                Err(_) => {
                    let error = anyhow::anyhow!("Export worker stopped unexpectedly. Check the export directory before retrying.");
                    log::error!("{error}");
                    if let Ok(mut status) = export.lock() {
                        status.publishing = false;
                        status.finished_at = Some(Instant::now());
                        status.eta_secs = -1.0;
                        status.stage = "Error: export worker stopped unexpectedly".into();
                        status.error_message = error.to_string();
                    }
                }
            }
        });
        match spawned {
            Ok(worker) => self.compile_worker = Some(worker),
            Err(error) => {
                self.export.update(ExportUpdate::Finished {
                    at: self.now(), outcome: ExportOutcome::Failed { message: format!("Cannot start export worker: {error}") },
                })?;
                return Err(anyhow::Error::new(error).context("Cannot start export worker"));
            }
        }

        Ok(())
    }

    fn handle_confirm_delete_chunk(&mut self, chunk: ChunkId) -> Result<()> {
        if matches!(&self.state, EngineState::Recording { .. }) {
            return Ok(());
        }
        {
            let orig_index = self.find_chunk_index(&chunk)?;
            if matches!(&self.state, EngineState::Playing { .. }) { self.stop_playback()?; }
            let mut next_project = self.project.clone();
            if let Some(path) = next_project.remove_file_at(orig_index) {
                next_project.save(&self.project_path)?;
                self.project = next_project;
                let full_path = self.resolve_file_path(&path);
                if full_path.exists() {
                    if let Err(error) = std::fs::remove_file(&full_path) { log::warn!("Deleted project entry but could not remove WAV: {error}"); }
                }
                let _ = crate::audio::waveform::remove_waveform_cache(&full_path);
                if self.current_index == Some(orig_index) {
                    self.current_index = if orig_index > 0 {
                        Some(orig_index - 1)
                    } else if !self.project.files.is_empty() {
                        Some(0)
                    } else {
                        None
                    };
                } else if self.current_index.map(|c| c > orig_index).unwrap_or(false) {
                    self.current_index = self.current_index.map(|c| c - 1);
                }
                self.update_current_waveform();
                self.update_prev_waveform();
                self.update_projection_after_change();
                info!("Deleted chunk at orig_index={}, path={}", orig_index, path);
            }
        }
        Ok(())
    }

    fn handle_prev_sect(&mut self) -> Result<()> {
        self.handle_adjacent_section(false)
    }

    fn handle_next_sect(&mut self) -> Result<()> {
        self.handle_adjacent_section(true)
    }

    fn handle_adjacent_section(&mut self, next: bool) -> Result<()> {
        let Some((current_orig, current_ui)) = self.current_orig_and_ui() else {
            return Ok(());
        };
        let total_files = self.project.files.len();
        let cached = if self.file_list_update_pending {
            None
        } else {
            self.projection_cache.section_indices()
        };
        let target = if let Some(indices) = cached {
            super::projection::adjacent_section_index(indices, current_orig, next)
                .map(|index| crate::utils::indexes::orig_to_ui_index(index, total_files))
        } else {
            // Headless instances have no UI snapshot; retain the direct fallback.
            find_section_ui_index(total_files, current_ui, next, |index| {
                self.project.files[index].markers.iter().any(|marker| {
                    self.project
                        .markers
                        .get(marker)
                        .is_some_and(|settings| settings.section)
                })
            })
        };
        if let Some(index) = target {
            self.goto_to_index(Some(index), true)?;
        }
        Ok(())
    }

    /// Регистронезависимый поиск по hint. down: true = более старые (ниже), false = более новые (выше).
    fn search_hint(&mut self, query: &str, down: bool) -> Result<()> {
        let query_lower = query.to_lowercase();
        if query_lower.is_empty() {
            return Ok(());
        }
        let total_files = self.project.files.len();
        if total_files == 0 {
            return Ok(());
        }
        let anchor = self.current_index.unwrap_or(total_files);
        let indices: Box<dyn Iterator<Item = usize>> = if down {
            Box::new((0..anchor.min(total_files)).rev())
        } else {
            Box::new(anchor.saturating_add(1)..total_files)
        };
        for index in indices {
            if self.project.files[index].hint.to_lowercase().contains(&query_lower) {
                let display_index = crate::utils::indexes::orig_to_ui_index(index, total_files);
                return self.goto_to_index(Some(display_index), false);
            }
        }

        Ok(())
    }
}
