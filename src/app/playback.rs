use crate::app::app::{App, AppState, PLAYBACK_STOP_DELAY_MS};
use crate::utils::indexes::{orig_to_ui_index, ui_to_orig_index};
use std::path::Path;
use std::time::{Duration, Instant};
use anyhow::Result;
use log::{info, debug};

impl App {
    /// Переходит к указанному индексу файла. play_after: запустить воспроизведение после перехода.
    pub fn goto_to_index(&mut self, ui_index: Option<i32>, play_after: bool) -> Result<()> {
        if self.debug {
            debug!("goto_to_index: ui_index={:?}, play_after={}", ui_index, play_after);
        }

        // Останавливаем запись и удаляем временную запись при переходе
        if let AppState::Recording { .. } = &self.state {
            self.stop_recording()?;
        }

        // Всегда останавливаем воспроизведение перед перемещением
        // Это гарантирует, что не будет одновременного воспроизведения нескольких записей
        if let AppState::Playing { .. } = &self.state {
            self.stop_playback()?;
            std::thread::sleep(Duration::from_millis(PLAYBACK_STOP_DELAY_MS));
        }

        let total_files = self.project.files.len();
        if total_files == 0 {
            if self.debug {
                debug!("goto_to_index: no files available");
            }
            return Ok(());
        }

        let orig_idx = match ui_index {
            None => total_files - 1,
            Some(-1) => total_files - 1,
            Some(ui_idx) => match ui_to_orig_index(ui_idx, total_files) {
                Some(orig) => orig,
                None => {
                    if self.debug {
                        debug!("goto_to_index: invalid ui_index {}, total_files={}", ui_idx, total_files);
                    }
                    return Ok(());
                }
            }
        };

        if orig_idx >= total_files {
            if self.debug {
                debug!("goto_to_index: orig_idx {} >= total_files {}", orig_idx, total_files);
            }
            return Ok(());
        }

        self.current_index = Some(orig_idx);
        if self.debug {
            debug!("goto_to_index: set current_index to {} (ui_index={:?}, total_files={})", orig_idx, ui_index, total_files);
        }
        info!("goto_to_index: moved to file index {} (ui_index={:?})", orig_idx, ui_index);

        // Запускаем распознавание, если hint пустой и распознавание доступно
        if let Some(file) = self.project.files.get(orig_idx) {
            if file.hint.is_empty() {
                self.start_transcription_for_file(orig_idx)?;
            }
        }

        // Обновляем waveform
        self.update_prev_waveform();
        self.update_current_waveform();

        // Обновляем UIState
        if self.ui_state.is_some() {
            if let Err(e) = self.update_ui_state() {
                if self.debug {
                    debug!("Failed to update UI state in goto_to_index: {:?}", e);
                }
            }
        }

        let ui_index_to_show = orig_to_ui_index(orig_idx, total_files);
        if let Some(ref tx) = self.current_index_tx {
            let _ = tx.send(ui_index_to_show);
        }

        self.notify_ui_refresh(true, true);

        if play_after {
            if self.debug {
                debug!("goto_to_index: starting playback from index {} (single file)", orig_idx);
            }
            info!("goto_to_index: starting playback from index {} (single file)", orig_idx);
            self.start_playback_from_index(Some(orig_idx))?;
            self.notify_ui_refresh(true, false);
        }

        Ok(())
    }

    /// Начинает воспроизведение только текущей записи
    pub fn start_playback(&mut self) -> Result<()> {
        let single_idx = self.current_index.or_else(|| {
            if !self.project.files.is_empty() {
                Some(self.project.files.len() - 1)
            } else {
                None
            }
        });
        self.start_playback_from_index(single_idx)
    }
    
    /// Начинает воспроизведение с указанного индекса
    pub fn start_playback_from_index(&mut self, play_single_file: Option<usize>) -> Result<()> {
        if self.debug {
            debug!("Starting playback, current_index={:?}, play_single_file={:?}", self.current_index, play_single_file);
        }
        info!("Starting playback");

        if let AppState::Recording { .. } = &self.state {
            self.stop_recording()?;
        }

        // Всегда останавливаем предыдущее воспроизведение перед началом нового
        // Это гарантирует, что не будет одновременного воспроизведения нескольких записей
        if let AppState::Playing { .. } = &self.state {
            self.stop_playback()?;
            std::thread::sleep(Duration::from_millis(PLAYBACK_STOP_DELAY_MS));
        }

        // Сохраняем current_index ПОСЛЕ остановки воспроизведения, чтобы использовать актуальное значение
        // current_index должен быть установлен в goto_to_index перед вызовом start_playback
        let saved_current_index = self.current_index;
        
        if self.debug {
            debug!("start_playback_from_index: saved_current_index={:?}, total_files={}, play_single_file={:?}", saved_current_index, self.project.files.len(), play_single_file);
        }
        
        // Убеждаемся, что current_index установлен правильно
        let start_idx = if let Some(single_idx) = play_single_file {
            // Если указан конкретный индекс для воспроизведения одного файла, используем его
            if single_idx < self.project.files.len() {
                if self.debug {
                    debug!("start_playback_from_index: playing single file at index {}", single_idx);
                }
                info!("start_playback_from_index: playing single file at index {}", single_idx);
                single_idx
            } else {
                if self.debug {
                    debug!("start_playback_from_index: invalid single file index {}, using saved_current_index", single_idx);
                }
                saved_current_index.unwrap_or_else(|| {
                    if !self.project.files.is_empty() {
                        self.project.files.len() - 1
                    } else {
                        0
                    }
                })
            }
        } else if let Some(idx) = saved_current_index {
            // Проверяем, что индекс валиден
            if idx < self.project.files.len() {
                if self.debug {
                    debug!("start_playback_from_index: using saved current_index={}", idx);
                }
                info!("start_playback_from_index: playing from file index {}", idx);
                idx
            } else {
                // Если индекс невалиден, используем последний файл
                if !self.project.files.is_empty() {
                    let last_idx = self.project.files.len() - 1;
                    self.current_index = Some(last_idx);
                    if self.debug {
                        debug!("start_playback_from_index: saved current_index invalid ({}), using last_idx={}", idx, last_idx);
                    }
                    last_idx
                } else {
                    0
                }
            }
        } else {
            // Если current_index не установлен, используем последний файл
            if !self.project.files.is_empty() {
                let last_idx = self.project.files.len() - 1;
                self.current_index = Some(last_idx);
                if self.debug {
                    debug!("start_playback_from_index: current_index not set, using last_idx={}", last_idx);
                }
                last_idx
            } else {
                0
            }
        };

        let mut sinks = Vec::new();
        
        // Определяем, до какого индекса воспроизводить
        let end_idx = if play_single_file.is_some() {
            // Если воспроизводим один файл, воспроизводим только его
            start_idx + 1
        } else {
            // Иначе воспроизводим все файлы с start_idx до конца
            self.project.files.len()
        };
        
        // Начинаем воспроизведение с start_idx до end_idx
        for i in start_idx..end_idx {
            let file = &self.project.files[i];
            let path = Path::new(&file.path);
            if path.exists() {
                // Проверяем, что файл не пустой
                if let Ok(metadata) = std::fs::metadata(path) {
                    if metadata.len() > 0 {
                        if let Ok(sink) = self.player.play_file(path) {
                            if self.debug {
                                debug!("start_playback: added file at index {}: {:?}", i, path);
                            }
                            info!("start_playback: playing file at index {}: {:?}", i, path);
                            sinks.push(sink);
                        } else if self.debug {
                            debug!("Failed to play file: {:?}", path);
                        }
                    } else if self.debug {
                        debug!("File is empty: {:?}", path);
                    }
                } else if self.debug {
                    debug!("Failed to get metadata for file: {:?}", path);
                }
            } else if self.debug {
                debug!("File does not exist: {:?}", path);
            }
        }

        if !sinks.is_empty() {
            // Убеждаемся, что current_index соответствует start_idx
            self.current_index = Some(start_idx);
            if self.debug {
                debug!("start_playback: set current_index to start_idx={}", start_idx);
            }
            self.state = AppState::Playing {
                sinks,
                current_index: start_idx,
                start_time: Instant::now(),
            };
            // Обновляем waveform для текущего и предыдущего файла при начале воспроизведения
            // (кэш будет обновлен только если индекс изменился)
            self.update_prev_waveform();
            self.update_current_waveform();
            self.notify_ui_refresh(true, false);
        } else if self.debug {
            debug!("No files to play, start_idx={}, total_files={}", start_idx, self.project.files.len());
            if self.ui_state.is_some() {
                let _ = self.update_ui_state();
            }
        }

        Ok(())
    }

    /// Останавливает воспроизведение
    pub fn stop_playback(&mut self) -> Result<()> {
        if let AppState::Playing { sinks, current_index: play_idx, .. } = std::mem::replace(&mut self.state, AppState::Idle) {
            // Останавливаем все sinks перед их удалением
            // Это гарантирует, что не будет одновременного воспроизведения нескольких записей
            for sink in sinks {
                sink.stop();
                sink.detach();
            }
            // Обновляем current_index из состояния Playing, чтобы он соответствовал файлу, который воспроизводился
            self.current_index = Some(play_idx);
            if self.debug {
                debug!("Stopped playback, current_index set to: {:?} (was: {:?})", play_idx, self.current_index);
            }
            self.update_prev_waveform();
            self.update_current_waveform();
            self.notify_ui_refresh(true, false);
        }
        Ok(())
    }
}
