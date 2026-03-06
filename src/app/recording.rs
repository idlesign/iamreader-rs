use crate::app::app::{App, AppState, RecordingMode};
use crate::project::{Project, ProjectFile};
use crate::utils::transcription::TranscriptionTask;
use crate::utils::stats::get_wav_duration;
use crate::audio::waveform::remove_waveform_cache;
use std::path::Path;
use std::time::{Duration, Instant};
use anyhow::Result;
use log::{info, warn, debug};
use std::fs;

impl App {
    /// Начинает запись с опциональной длительностью.
    /// replace_current: в режиме A при true (r) заменяем текущий отрезок, при false (e) добавляем после.
    pub fn start_recording_with_duration(&mut self, duration_secs: Option<u64>, replace_current: bool) -> Result<()> {
        if self.debug {
            debug!("Starting recording (duration: {:?}, replace_current: {}, mode: {:?})",
                duration_secs, replace_current, self.recording_mode);
        }
        if let Some(dur) = duration_secs {
            info!("Starting recording for {} seconds", dur);
        } else {
            info!("Starting recording");
        }

        if let AppState::Playing { .. } = &self.state {
            self.stop_playback()?;
            std::thread::sleep(Duration::from_millis(50));
        }

        if let AppState::Recording { .. } = &self.state {
            self.stop_recording()?;
        }

        let current_idx = if let Some(idx) = self.current_index {
            idx
        } else {
            if !self.project.files.is_empty() {
                let idx = self.project.files.len() - 1;
                self.current_index = Some(idx);
                idx
            } else {
                self.current_index = Some(0);
                0
            }
        };

        let path = match self.recording_mode {
            RecordingMode::Append => {
                let remove_from_idx = if replace_current { current_idx } else { current_idx + 1 };
                if remove_from_idx < self.project.files.len() {
                    let removed_paths = self.project.remove_files_from_index(remove_from_idx);
                    for file_path in &removed_paths {
                        let path = Path::new(file_path);
                        if path.exists() {
                            fs::remove_file(path).ok();
                        }
                        remove_waveform_cache(path).ok();
                        self.waveform_cache.remove(file_path);
                    }
                    self.project.save(&self.project_path)?;
                }
                self.current_index = None;
                self.project.get_next_file_path(&self.chunks_dir)
            }
            RecordingMode::Update => {
                let path = if let Some(p) = &self.update_recording_path {
                    p.clone()
                } else {
                    let hhmm = chrono::Local::now().format("%H%M").to_string();
                    let p = Project::get_file_path_for_update(&self.chunks_dir, current_idx, &hhmm);
                    self.update_recording_path = Some(p.clone());
                    self.update_recording_index = Some(current_idx);
                    p
                };
                self.current_index = self.update_recording_index;
                path
            }
            RecordingMode::Insert => {
                let path = if let Some(p) = &self.insert_recording_path {
                    p.clone()
                } else {
                    let hhmm = chrono::Local::now().format("%H%M").to_string();
                    let p = Project::get_file_path_for_insert(&self.chunks_dir, current_idx, &hhmm);
                    self.insert_recording_path = Some(p.clone());
                    self.insert_recording_index = Some(current_idx + 1);
                    p
                };
                self.current_index = Some(self.insert_recording_index.unwrap() - 1);
                path
            }
        };

        let (stream, writer) = self.recorder.start_recording(path.clone(), self.level_tx.clone())?;
        self.waveform_current.clear();

        self.state = AppState::Recording {
            stream,
            writer,
            writer_path: path.clone(),
            start_time: Instant::now(),
            duration_secs,
        };

        self.notify_ui_refresh(true, false);

        Ok(())
    }

    /// Завершает запись: добавляет/обновляет запись в проекте; размер и длительность достраиваются в фоне.
    pub fn finish_recording(&mut self) -> Result<()> {
        if let AppState::Recording { stream, writer, writer_path, .. } = std::mem::replace(&mut self.state, AppState::Idle) {
            drop(stream);

            let file_path = writer_path.to_string_lossy().to_string();
            let writer_opt = writer.lock().map_err(|_| anyhow::anyhow!("writer lock poisoned"))?.take();

            let file = ProjectFile {
                path: file_path.clone(),
                title: String::new(),
                author: String::new(),
                year: String::new(),
                hint: String::new(),
                markers: Vec::new(),
                size: 0,
                duration_ms: 0,
            };

            match self.recording_mode {
                RecordingMode::Append => {
                    self.project.files.push(file);
                    self.current_index = Some(self.project.files.len() - 1);
                }
                RecordingMode::Update => {
                    if let Some(idx) = self.update_recording_index {
                        if idx < self.project.files.len() {
                            let old = &self.project.files[idx];
                            let old_path = old.path.clone();
                            let title = old.title.clone();
                            let author = old.author.clone();
                            let year = old.year.clone();
                            let hint = old.hint.clone();
                            let markers = old.markers.clone();
                            let project_dir = self.project_path.parent().unwrap_or_else(|| Path::new("."));
                            let old_full = project_dir.join(&old_path);
                            if old_full.exists() {
                                fs::remove_file(&old_full).ok();
                                remove_waveform_cache(&old_full).ok();
                                self.waveform_cache.remove(&old_path);
                            }
                            self.project.files[idx] = ProjectFile {
                                path: file.path.clone(),
                                title,
                                author,
                                year,
                                hint,
                                markers,
                                size: 0,
                                duration_ms: 0,
                            };
                        }
                    }
                    self.current_index = self.update_recording_index;
                    self.update_recording_path = None;
                    self.update_recording_index = None;
                    self.recording_mode = RecordingMode::Append;
                }
                RecordingMode::Insert => {
                    if let Some(insert_at) = self.insert_recording_index {
                        self.project.files.insert(insert_at, file);
                        self.current_index = Some(insert_at);
                    } else {
                        self.project.files.push(file);
                        self.current_index = Some(self.project.files.len() - 1);
                    }
                    self.insert_recording_path = None;
                    self.insert_recording_index = None;
                    self.recording_mode = RecordingMode::Append;
                }
            }

            self.project.save(&self.project_path)?;

            self.waveform_current.clear();
            self.cached_current_index = None;
            self.cached_prev_index = None;
            self.update_prev_waveform();
            self.update_current_waveform();

            log::info!("[finish_recording] Recording stopped, file added/updated: {}", file_path);

            let tx = self.finish_recording_result_tx.clone();
            std::thread::spawn(move || {
                if let Some(w) = writer_opt {
                    let _ = w.finalize();
                }
                for _ in 0..30 {
                    if Path::new(&file_path).exists() {
                        break;
                    }
                    std::thread::sleep(Duration::from_millis(50));
                }
                std::thread::sleep(Duration::from_millis(100));
                let file_path_obj = Path::new(&file_path);
                let file_size = file_path_obj
                    .exists()
                    .then(|| std::fs::metadata(&file_path).map(|m| m.len()).unwrap_or(0))
                    .unwrap_or(0);
                let file_duration_ms = file_path_obj
                    .exists()
                    .then(|| get_wav_duration(file_path_obj).map(|d| d.as_millis() as u64).unwrap_or(0))
                    .unwrap_or(0);
                let _ = tx.send((file_path, file_size, file_duration_ms));
            });
            self.update_ui_after_change();
        }
        Ok(())
    }

    /// Останавливает запись и удаляет временный файл. Длительность отменённого отрезка прибавляется к record_length.
    pub fn stop_recording(&mut self) -> Result<()> {
        if let AppState::Recording { stream, writer, writer_path, start_time, .. } = std::mem::replace(&mut self.state, AppState::Idle) {
            let elapsed_ms = start_time.elapsed().as_millis() as u64;
            self.project.stats.record_length = self.project.stats.record_length.saturating_add(elapsed_ms);
            if let Err(e) = self.project.save(&self.project_path) {
                warn!("Failed to save project after stop_recording: {}", e);
            }
            drop(stream); // Drop stream to release references to writer
            
            // Даем время callback'ам завершиться
            std::thread::sleep(Duration::from_millis(100));
            
            // Финализируем writer перед удалением
            if let Some(w) = writer.lock().unwrap().take() {
                w.finalize().ok();
            }
            
            fs::remove_file(&writer_path).ok();
            // Удаляем кеш для временного файла
            remove_waveform_cache(&writer_path).ok();
            
            // Очищаем waveform для текущей записи
            self.waveform_current.clear();
            
            self.notify_ui_refresh(true, false);

            if self.debug {
                debug!("Stopped recording, removed temporary file: {:?}, record_length +{} ms", writer_path, elapsed_ms);
            }
        }
        Ok(())
    }

    /// Запускает распознавание речи для файла
    pub fn start_transcription_for_file(&self, file_index: usize) -> Result<()> {
        // Общий метод для запуска распознавания файла
        // Используется из finish_recording, goto_to_index и FIFO команды trans
        if let Some(ref tx) = self.transcription_tx {
            if let Some(file) = self.project.files.get(file_index) {
                let file_path = std::path::PathBuf::from(&file.path);
                if file_path.exists() {
                    let task = TranscriptionTask {
                        file_path,
                        file_index,
                        project_path: self.project_path.clone(),
                    };
                    if let Err(e) = tx.send(task) {
                        warn!("Failed to send transcription task: {}", e);
                    } else {
                        info!("Transcription task queued for file index {}", file_index);
                    }
                } else {
                    warn!("File does not exist for transcription: {:?}", file.path);
                }
            } else {
                warn!("File index {} out of bounds (total: {})", file_index, self.project.files.len());
            }
        } else {
            if self.debug {
                debug!("Transcription not available (model not found)");
            }
        }
        Ok(())
    }
}
