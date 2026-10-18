use super::runtime::{Engine, EngineState};
use crate::utils::indexes::ui_to_orig_index;
use anyhow::Result;
use log::{debug, info};

impl Engine {
    /// Переходит к указанному индексу файла. play_after: запустить воспроизведение после перехода.
    pub(crate) fn goto_to_index(&mut self, ui_index: Option<i32>, play_after: bool) -> Result<()> {
        if self.debug {
            debug!(
                "goto_to_index: ui_index={:?}, play_after={}",
                ui_index, play_after
            );
        }

        // Останавливаем запись и удаляем временную запись при переходе
        if let EngineState::Recording { .. } = &self.state {
            self.stop_recording()?;
        }

        // Всегда останавливаем воспроизведение перед перемещением
        // Это гарантирует, что не будет одновременного воспроизведения нескольких записей
        if let EngineState::Playing { .. } = &self.state {
            self.stop_playback()?;
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
                        debug!(
                            "goto_to_index: invalid ui_index {}, total_files={}",
                            ui_idx, total_files
                        );
                    }
                    return Ok(());
                }
            },
        };

        if orig_idx >= total_files {
            if self.debug {
                debug!(
                    "goto_to_index: orig_idx {} >= total_files {}",
                    orig_idx, total_files
                );
            }
            return Ok(());
        }

        self.current_index = Some(orig_idx);
        if self.debug {
            debug!(
                "goto_to_index: set current_index to {} (ui_index={:?}, total_files={})",
                orig_idx, ui_index, total_files
            );
        }
        info!(
            "goto_to_index: moved to file index {} (ui_index={:?})",
            orig_idx, ui_index
        );

        // Запускаем распознавание, если hint пустой и распознавание доступно
        if let Some(file) = self.project.files.get(orig_idx) {
            if file.hint.is_empty() {
                self.start_transcription_for_file(orig_idx)?;
            }
        }

        // Обновляем waveform
        self.update_current_waveform();
        self.update_prev_waveform();

        self.refresh_projection(true, true);

        if play_after {
            if self.debug {
                debug!(
                    "goto_to_index: starting playback from index {} (single file)",
                    orig_idx
                );
            }
            info!(
                "goto_to_index: starting playback from index {} (single file)",
                orig_idx
            );
            self.start_playback_from_index(Some(orig_idx))?;
            self.refresh_projection(true, true);
        }

        Ok(())
    }

    /// Начинает воспроизведение только текущей записи
    pub(crate) fn start_playback(&mut self) -> Result<()> {
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
    pub(crate) fn start_playback_from_index(&mut self, play_single_file: Option<usize>) -> Result<()> {
        if self.debug {
            debug!(
                "Starting playback, current_index={:?}, play_single_file={:?}",
                self.current_index, play_single_file
            );
        }
        info!("Starting playback");

        if let EngineState::Recording { .. } = &self.state {
            self.stop_recording()?;
        }

        // Всегда останавливаем предыдущее воспроизведение перед началом нового
        // Это гарантирует, что не будет одновременного воспроизведения нескольких записей
        if let EngineState::Playing { .. } = &self.state {
            self.stop_playback()?;
        }

        if self.project.files.is_empty() {
            self.current_index = None;
            self.refresh_projection(true, true);
            return Ok(());
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
                    debug!(
                        "start_playback_from_index: playing single file at index {}",
                        single_idx
                    );
                }
                info!(
                    "start_playback_from_index: playing single file at index {}",
                    single_idx
                );
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
                    debug!(
                        "start_playback_from_index: using saved current_index={}",
                        idx
                    );
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
                    debug!(
                        "start_playback_from_index: current_index not set, using last_idx={}",
                        last_idx
                    );
                }
                last_idx
            } else {
                0
            }
        };

        let end_idx = if play_single_file.is_some() { start_idx + 1 } else { self.project.files.len() };
        let paths: Vec<_> = self.project.files[start_idx..end_idx].iter()
            .map(|file| self.resolve_file_path(&file.path)).collect();
        let session = self.audio.start_playback(&paths)?;
        self.current_index = Some(start_idx);
        self.state = EngineState::Playing { session, current_index: start_idx, start_time: self.now() };
        self.update_current_waveform();
        self.update_prev_waveform();
        self.refresh_projection(true, true);

        Ok(())
    }

    /// Останавливает воспроизведение
    pub(crate) fn stop_playback(&mut self) -> Result<()> {
        if !matches!(self.state, EngineState::Playing { .. }) { return Ok(()); }
        if let EngineState::Playing {
            session,
            current_index: play_idx,
            ..
        } = std::mem::replace(&mut self.state, EngineState::Idle)
        {
            let stopped = session.stop();
            self.current_index = Some(play_idx);
            if self.debug {
                debug!(
                    "Stopped playback, current_index set to: {:?} (was: {:?})",
                    play_idx, self.current_index
                );
            }
            self.update_current_waveform();
            self.update_prev_waveform();
            self.refresh_projection(true, true);
            stopped?;
        }
        Ok(())
    }
}
