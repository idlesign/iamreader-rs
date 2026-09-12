use crate::app::app::{App, AppState, RecordingMode, FILE_LIST_UPDATE_THROTTLE_MS};
use crate::audio::waveform_loader::WaveformSlot;
use crate::ui::FileInfo;
use crate::utils::indexes::orig_to_ui_index;
use crate::utils::stats::{calculate_sizes, calculate_total_duration, get_free_space};
use crate::utils::{
    current_and_prev_file_hints, format_duration, format_markers_with_ordinals_batch, format_size,
    reverse_and_reindex_file_list,
};
use anyhow::Result;
use log::debug;
use std::path::Path;
use std::time::{Duration, Instant};

/// Cheap identity of the data and selection represented by the shared UI snapshot.
/// Project mutations with unchanged length must also set `file_list_update_pending`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct UiUpdateKey {
    pub current_index: Option<usize>,
    pub total_files: usize,
    pub recording_mode: RecordingMode,
    pub recording_started: Option<Instant>,
    pub playing_index: Option<usize>,
    pub record_length_ms: u64,
}

/// Keeps the 50 ms tick independent of the number of project fragments.
#[derive(Default)]
pub struct UiUpdateCache {
    key: Option<UiUpdateKey>,
    section_indices: Vec<usize>,
    total_duration: Duration,
    total_size: u64,
    last_free_space_update: Option<Instant>,
    last_recording_second: Option<u64>,
}

impl UiUpdateCache {
    pub fn needs_rebuild(&self, key: UiUpdateKey, data_changed: bool) -> bool {
        let Some(previous) = self.key else {
            return true;
        };
        data_changed
            || previous.total_files != key.total_files
            || previous.recording_mode != key.recording_mode
            || previous.recording_started != key.recording_started
            || previous.record_length_ms != key.record_length_ms
            // A/I temporary-row placement and the U recording flag depend on selection.
            || (key.recording_started.is_some() && previous.current_index != key.current_index)
    }

    pub fn needs_navigation_update(&self, key: UiUpdateKey) -> bool {
        self.key.is_some_and(|previous| {
            previous.current_index != key.current_index
                || previous.playing_index != key.playing_index
        })
    }

    /// Sorted original indices from the last structural snapshot; unavailable before it exists.
    /// Callers must reject the snapshot while `file_list_update_pending` is set.
    pub fn section_indices(&self) -> Option<&[usize]> {
        self.key.map(|_| self.section_indices.as_slice())
    }

    pub fn record_rebuild(
        &mut self,
        key: UiUpdateKey,
        duration: Duration,
        size: u64,
        section_indices: Vec<usize>,
    ) {
        self.key = Some(key);
        self.section_indices = section_indices;
        self.total_duration = duration;
        self.total_size = size;
        self.last_free_space_update = Some(Instant::now());
        self.last_recording_second = None;
    }

    /// Records a lightweight update after checking `needs_rebuild` first.
    pub fn record_navigation(&mut self, key: UiUpdateKey) {
        self.key = Some(key);
    }
}

/// Finds the strictly adjacent section in O(log S), where S is the number of sections.
/// `next` means a newer fragment (a larger original index).
pub fn adjacent_section_index(
    section_indices: &[usize],
    current_index: usize,
    next: bool,
) -> Option<usize> {
    if next {
        let position = section_indices.partition_point(|&index| index <= current_index);
        section_indices.get(position).copied()
    } else {
        let position = section_indices.partition_point(|&index| index < current_index);
        position
            .checked_sub(1)
            .and_then(|position| section_indices.get(position).copied())
    }
}

/// Patches only the previous/current playback row in a non-recording, reversed list.
/// Selection itself is a scalar property, so it does not invalidate any row or list version.
pub fn update_playing_rows(
    rows: &mut [FileInfo],
    previous_index: Option<usize>,
    playing_index: Option<usize>,
) -> [Option<usize>; 2] {
    let mut changed = [None; 2];
    if previous_index == playing_index {
        return changed;
    }
    for (slot, (original_index, is_playing)) in [(previous_index, false), (playing_index, true)]
        .into_iter()
        .enumerate()
    {
        let Some(original_index) = original_index else {
            continue;
        };
        let Ok(ui_index) = usize::try_from(orig_to_ui_index(original_index, rows.len())) else {
            continue;
        };
        if let Some(row) = rows.get_mut(ui_index) {
            if row.is_playing != is_playing {
                row.is_playing = is_playing;
                changed[slot] = Some(ui_index);
            }
        }
    }
    changed
}

/// Updates only the active recording row; the structural list version is unchanged.
/// The GUI timer observes this row separately from full-list snapshots.
pub fn update_recording_row_duration(
    rows: &mut [FileInfo],
    current_index: i32,
    elapsed: Duration,
) -> bool {
    let Some(row) = usize::try_from(current_index)
        .ok()
        .and_then(|index| rows.get_mut(index))
    else {
        return false;
    };
    if !row.is_recording {
        return false;
    }
    let duration = format_duration(elapsed, false);
    if row.duration.as_str() == duration.as_str() {
        return false;
    }
    row.duration = duration.into();
    true
}

/// Maps selection to the reversed UI list, including a temporary A/I recording row.
pub fn file_list_current_index(
    total_files: usize,
    current_index: Option<usize>,
    recording_mode: RecordingMode,
    is_recording: bool,
) -> i32 {
    let has_temp = is_recording && recording_mode != RecordingMode::Update;
    if has_temp {
        if recording_mode == RecordingMode::Insert {
            if let Some(index) = current_index.filter(|&index| index < total_files) {
                return (total_files - index - 1) as i32;
            }
        }
        0
    } else if let Some(index) = current_index.filter(|&index| index < total_files) {
        orig_to_ui_index(index, total_files)
    } else if total_files > 0 {
        0
    } else {
        -1
    }
}

impl App {
    /// Обновляет UI. При skip_file_list_rebuild=true не пересобирает список (быстрый путь для Goto/Prev/Next).
    /// При смене индекса прокрутка выполняется только если строка не видна.
    pub fn update_ui(&mut self, skip_file_list_rebuild: bool) -> Result<()> {
        let current_file = self
            .current_index
            .and_then(|index| self.project.files.get(index));

        let current_duration = if let AppState::Recording { start_time, .. } = &self.state {
            start_time.elapsed()
        } else if let Some(file) = current_file {
            Duration::from_millis(file.duration_ms)
        } else {
            Duration::ZERO
        };

        let total_duration = calculate_total_duration(&self.project.files);
        let total_size = calculate_sizes(&self.project.files);

        let current_index = self.current_index;
        let total_files = self.project.files.len();
        let waveform_current = self.waveform_current.clone();
        let waveform_prev = self.waveform_prev.clone();
        let is_recording = matches!(&self.state, AppState::Recording { .. });

        let ui = self
            .ui
            .as_mut()
            .ok_or_else(|| anyhow::anyhow!("UI not available"))?;

        if self.status_line_update_pending {
            let free_space = get_free_space(&self.chunks_dir);
            let record_length_ms = self.project.stats.record_length;
            let recording_elapsed = if let AppState::Recording { start_time, .. } = &self.state {
                start_time.elapsed()
            } else {
                Duration::ZERO
            };
            ui.update_status_line(
                current_index,
                total_files,
                current_file,
                current_duration,
                total_duration,
                record_length_ms,
                recording_elapsed,
                total_size,
                free_space,
                is_recording,
                self.recording_mode.as_str(),
            )?;
            let _ = ui.sync_status_line_to_window();
            self.status_line_update_pending = false;
            // Во время записи обновляем строку снова при следующем тике (чтобы росло " / record_total")
            if is_recording {
                self.status_line_update_pending = true;
            }
        }

        if skip_file_list_rebuild {
            let ui_index = if is_recording {
                0
            } else if let Some(orig) = current_index {
                orig_to_ui_index(orig, total_files)
            } else {
                -1
            };
            ui.set_current_file_index_only(ui_index)?;
            ui.update_file_hints(&self.project.files, current_index)?;
        } else {
            let recording_duration = if let AppState::Recording { start_time, .. } = &self.state {
                start_time.elapsed()
            } else {
                Duration::ZERO
            };
            let is_playing = matches!(&self.state, AppState::Playing { .. });
            let temp_file_path = if let AppState::Recording { writer_path, .. } = &self.state {
                Some(writer_path.to_string_lossy().to_string())
            } else {
                None
            };

            let do_file_list_update = self.file_list_update_pending
                || self
                    .last_file_list_update
                    .map(|instant| {
                        instant.elapsed() >= Duration::from_millis(FILE_LIST_UPDATE_THROTTLE_MS)
                    })
                    .unwrap_or(true);
            if do_file_list_update {
                ui.update_file_list(
                    &self.project.files,
                    current_index,
                    recording_duration,
                    is_playing,
                    temp_file_path.as_deref(),
                    self.recording_mode.as_str(),
                )?;
                ui.sync_file_list_to_model()?;
                self.file_list_update_pending = false;
                self.last_file_list_update = Some(Instant::now());
            }
        }

        ui.update_waveform_current(&waveform_current)?;
        ui.update_waveform_prev(&waveform_prev)?;

        ui.render()?;
        Ok(())
    }

    /// Обновляет UI после изменения данных (действие пользователя и т.п.)
    pub fn update_ui_after_change(&mut self) {
        if self.debug {
            debug!("[update_ui_after_change] called, current_index={:?}, ui.is_some()={}, ui_state.is_some()={}",
                self.current_index, self.ui.is_some(), self.ui_state.is_some());
        }
        self.file_list_update_pending = true;
        self.status_line_update_pending = true;
        self.wav_spec_cache = None;
        self.notify_ui_refresh(false, false);
    }

    /// Обновляет UI или UI state: при status_line_pending выставляет status_line_update_pending и вызывает update_ui или update_ui_state.
    pub fn notify_ui_refresh(&mut self, status_line_pending: bool, skip_file_list_rebuild: bool) {
        if status_line_pending {
            self.status_line_update_pending = true;
        }
        if !skip_file_list_rebuild {
            self.file_list_update_pending = true;
        }
        if self.ui.is_some() {
            let _ = self.update_ui(skip_file_list_rebuild);
        } else if self.ui_state.is_some() {
            let _ = self.update_ui_state();
        }
    }

    /// Rebuilds project-derived UI data only on changes; live values remain O(1).
    pub fn update_ui_state(&mut self) -> Result<()> {
        let key = UiUpdateKey {
            current_index: self
                .current_index
                .or_else(|| self.project.files.len().checked_sub(1)),
            total_files: self.project.files.len(),
            recording_mode: self.recording_mode,
            recording_started: match &self.state {
                AppState::Recording { start_time, .. } => Some(*start_time),
                _ => None,
            },
            playing_index: match &self.state {
                AppState::Playing { current_index, .. } => Some(*current_index),
                _ => None,
            },
            record_length_ms: self.project.stats.record_length,
        };
        if self
            .ui_update_cache
            .needs_rebuild(key, self.file_list_update_pending)
        {
            self.rebuild_ui_state(key)?;
        } else if self.ui_update_cache.needs_navigation_update(key) {
            self.update_ui_navigation(key)?;
        }

        let recording_elapsed = key.recording_started.map(|started| started.elapsed());
        let recording_second = recording_elapsed.map(|elapsed| elapsed.as_secs());
        let recording_duration_changed =
            recording_second != self.ui_update_cache.last_recording_second;
        let update_free_space = self
            .ui_update_cache
            .last_free_space_update
            .map(|updated| updated.elapsed() >= Duration::from_secs(1))
            .unwrap_or(true);
        let total_size = if update_free_space {
            let size = self.ui_update_cache.total_size;
            let free_space = get_free_space(&self.chunks_dir);
            self.ui_update_cache.last_free_space_update = Some(Instant::now());
            Some(format!(
                "{} / {}",
                format_size(size),
                format_size(size.saturating_add(free_space))
            ))
        } else {
            None
        };

        if recording_elapsed.is_none() && key.playing_index.is_none() && total_size.is_none() {
            self.status_line_update_pending = false;
            return Ok(());
        }

        let playback_position = match &self.state {
            AppState::Playing {
                start_time,
                current_index,
                ..
            } => self
                .project
                .files
                .get(*current_index)
                .filter(|file| file.duration_ms > 0)
                .map(|file| {
                    let elapsed = start_time
                        .elapsed()
                        .saturating_sub(Duration::from_millis(200));
                    (elapsed.as_secs_f32() / Duration::from_millis(file.duration_ms).as_secs_f32())
                        .clamp(0.0, 1.0)
                })
                .unwrap_or(0.0),
            _ => 0.0,
        };
        let ui_state = self
            .ui_state
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("UI state not available"))?;
        let mut state = ui_state
            .lock()
            .map_err(|_| anyhow::anyhow!("UI state lock poisoned"))?;
        state.playback_position = playback_position;
        if let Some(elapsed) = recording_elapsed {
            // Recording waveform is bounded to 1500 samples, not project size.
            state.waveform_current.clone_from(&self.waveform_current);
            if recording_duration_changed {
                let current_index = state.current_file_index;
                update_recording_row_duration(&mut state.file_list, current_index, elapsed);
                state.total_duration = format!(
                    "{} / {}",
                    format_duration(self.ui_update_cache.total_duration, false),
                    format_duration(
                        Duration::from_millis(key.record_length_ms).saturating_add(elapsed),
                        false
                    ),
                );
            }
        }
        if let Some(total_size) = total_size {
            state.total_size = total_size;
        }
        self.ui_update_cache.last_recording_second = recording_second;
        self.status_line_update_pending = false;
        Ok(())
    }

    /// Applies selection/playback scalars without rebuilding rows, TOC or aggregates.
    fn update_ui_navigation(&mut self, key: UiUpdateKey) -> Result<()> {
        let previous = self
            .ui_update_cache
            .key
            .ok_or_else(|| anyhow::anyhow!("UI snapshot not initialized"))?;
        let selection_changed = previous.current_index != key.current_index;
        let selection = selection_changed.then(|| {
            let current_file = key
                .current_index
                .and_then(|index| self.project.files.get(index));
            let file_name = current_file
                .and_then(|file| Path::new(&file.path).file_name())
                .and_then(|name| name.to_str())
                .unwrap_or("")
                .to_string();
            let hints = current_and_prev_file_hints(&self.project.files, key.current_index, 100);
            // Both waveform arrays have a fixed display-size bound, independent of the project.
            (
                file_name,
                hints,
                self.waveform_current.clone(),
                self.waveform_prev.clone(),
            )
        });
        let ui_state = self
            .ui_state
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("UI state not available"))?;
        let mut state = ui_state
            .lock()
            .map_err(|_| anyhow::anyhow!("UI state lock poisoned"))?;
        for index in update_playing_rows(
            &mut state.file_list,
            previous.playing_index,
            key.playing_index,
        )
        .into_iter()
        .flatten()
        {
            state.mark_file_list_row_changed(index);
        }
        if let Some((
            file_name,
            (cur_start, cur_end, prev_start, prev_end),
            waveform_current,
            waveform_prev,
        )) = selection
        {
            state.current_file_index = file_list_current_index(
                key.total_files,
                key.current_index,
                key.recording_mode,
                false,
            );
            let current_file_num = usize::try_from(state.current_file_index)
                .ok()
                .and_then(|index| key.total_files.checked_sub(index))
                .unwrap_or(0);
            state.current_file_name = file_name;
            state.total_files = format!("{}/{}", current_file_num, key.total_files);
            state.hintbox_prev = if prev_end.is_empty() {
                prev_start.clone()
            } else {
                format!("{}[...]{}", prev_start, prev_end)
            };
            state.hintbox_current_start = cur_start;
            state.hintbox_current_end = cur_end;
            state.hintbox_prev_start = prev_start;
            state.hintbox_prev_end = prev_end;
            state.waveform_current = waveform_current;
            state.waveform_prev = waveform_prev;
        }
        // A stopped player takes the idle fast path below; clear its last position now.
        if previous.playing_index != key.playing_index {
            state.playback_position = 0.0;
        }
        self.ui_update_cache.record_navigation(key);
        Ok(())
    }

    /// Rebuilds the static snapshot after project/structure/mode/recording transitions.
    fn rebuild_ui_state(&mut self, key: UiUpdateKey) -> Result<()> {
        // Используем self.current_index, но если он None, используем последний файл (самый свежий)
        let current_index = self.current_index.or_else(|| {
            if !self.project.files.is_empty() {
                Some(self.project.files.len() - 1)
            } else {
                None
            }
        });

        // Волновой график предыдущей/текущей записи обновляется только при смене текущей записи (см. goto_to_index, start_playback, stop_playback, finish_recording, main).

        let current_file = current_index.and_then(|index| self.project.files.get(index));

        let total_duration = calculate_total_duration(&self.project.files);
        let total_size = calculate_sizes(&self.project.files);
        let free_space = get_free_space(&self.chunks_dir);
        let waveform_current = self.waveform_current.clone();
        let waveform_prev = self.waveform_prev.clone();
        let files = &self.project.files;
        let is_recording = matches!(&self.state, AppState::Recording { .. });
        let is_playing = matches!(&self.state, AppState::Playing { .. });

        // Для определения воспроизведения используем current_index из self, а не из параметра
        // При воспроизведении current_index указывает на файл, который воспроизводится
        let playing_index = if is_playing {
            if let AppState::Playing {
                current_index: play_idx,
                ..
            } = &self.state
            {
                Some(*play_idx)
            } else {
                self.current_index
            }
        } else {
            None
        };

        // Вычисляем позицию воспроизведения (0.0 - 1.0) для текущего файла
        // Вводим поправку 200ms, чтобы индикатор не обгонял воспроизведение
        let playback_position = if is_playing {
            if let AppState::Playing {
                start_time,
                current_index: play_idx,
                ..
            } = &self.state
            {
                if let Some(file) = self.project.files.get(*play_idx) {
                    let elapsed = start_time.elapsed();
                    // Вычитаем 200ms для компенсации задержки
                    let adjusted_elapsed = if elapsed > Duration::from_millis(200) {
                        elapsed - Duration::from_millis(200)
                    } else {
                        Duration::ZERO
                    };
                    let file_duration = Duration::from_millis(file.duration_ms);
                    if file_duration > Duration::ZERO {
                        (adjusted_elapsed.as_secs_f32() / file_duration.as_secs_f32())
                            .min(1.0)
                            .max(0.0)
                    } else {
                        0.0
                    }
                } else {
                    0.0
                }
            } else {
                0.0
            }
        } else {
            0.0
        };

        // Обновляем список файлов
        // Показываем все файлы из проекта (все считаются завершенными)
        let recording_duration = if let AppState::Recording { start_time, .. } = &self.state {
            start_time.elapsed()
        } else {
            Duration::ZERO
        };

        let mut cumulative_duration = Duration::ZERO;
        let markers_batch = format_markers_with_ordinals_batch(files);

        let temp_file_path = if let AppState::Recording { writer_path, .. } = &self.state {
            Some(writer_path.to_string_lossy().to_string())
        } else {
            None
        };

        let row_is_recording_update = self.recording_mode == RecordingMode::Update && is_recording;

        let mut new_file_list: Vec<FileInfo> = files
            .iter()
            .enumerate()
            .map(|(orig_idx, f)| {
                let path = Path::new(&f.path);
                let name = path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .unwrap_or("")
                    .to_string();
                let file_is_playing = is_playing && playing_index == Some(orig_idx);

                let duration_ms = f.duration_ms;
                let duration = format_duration(Duration::from_millis(duration_ms), false);
                let start_time_str = format_duration(cumulative_duration, false);
                cumulative_duration += Duration::from_millis(duration_ms);

                let size_str = format_size(f.size);

                let markers_str = markers_batch.get(orig_idx).cloned().unwrap_or_default();
                let row_is_recording = row_is_recording_update && current_index == Some(orig_idx);
                FileInfo {
                    index: (orig_idx + 1) as i32,
                    name: name.into(),
                    path: f.path.clone().into(),
                    markers: markers_str.into(),
                    duration: duration.into(),
                    start_time: start_time_str.into(),
                    size: size_str.into(),
                    is_recording: row_is_recording,
                    is_playing: file_is_playing,
                    title: f.title.clone().into(),
                    author: f.author.clone().into(),
                    year: f.year.clone().into(),
                    hint: f.hint.clone().into(),
                }
            })
            .collect();

        let total_files = new_file_list.len();

        if let Some(ref temp_path) = temp_file_path {
            if is_recording {
                let path = Path::new(temp_path);
                let name = path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .unwrap_or("")
                    .to_string();
                match self.recording_mode {
                    RecordingMode::Update => {}
                    RecordingMode::Insert
                        if current_index.is_some_and(|index| index < total_files) =>
                    {
                        let insert_at = current_index.unwrap() + 1;
                        let cum_insert: Duration = files
                            .iter()
                            .take(insert_at)
                            .map(|file| Duration::from_millis(file.duration_ms))
                            .sum();
                        let start_time_str = format_duration(cum_insert, false);
                        new_file_list.insert(
                            insert_at,
                            FileInfo {
                                index: (insert_at + 1) as i32,
                                name: name.into(),
                                path: temp_path.clone().into(),
                                markers: String::new().into(),
                                duration: format_duration(recording_duration, false).into(),
                                start_time: start_time_str.into(),
                                size: "...".to_string().into(),
                                is_recording: true,
                                is_playing: false,
                                title: String::new().into(),
                                author: String::new().into(),
                                year: String::new().into(),
                                hint: String::new().into(),
                            },
                        );
                    }
                    _ => {
                        let start_time_str = format_duration(cumulative_duration, false);
                        new_file_list.push(FileInfo {
                            index: (total_files + 1) as i32,
                            name: name.into(),
                            path: temp_path.clone().into(),
                            markers: String::new().into(),
                            duration: format_duration(recording_duration, false).into(),
                            start_time: start_time_str.into(),
                            size: "...".to_string().into(),
                            is_recording: true,
                            is_playing: false,
                            title: String::new().into(),
                            author: String::new().into(),
                            year: String::new().into(),
                            hint: String::new().into(),
                        });
                    }
                }
            }
        }

        let reversed_file_list = reverse_and_reindex_file_list(new_file_list);
        let new_file_list_len = reversed_file_list.len();

        let new_index = file_list_current_index(
            total_files,
            current_index,
            self.recording_mode,
            is_recording,
        );

        // Обновляем статусную строку
        let total_dur_str = format_duration(total_duration, false);
        let record_length_ms = self.project.stats.record_length;
        let recording_elapsed = if let AppState::Recording { start_time, .. } = &self.state {
            start_time.elapsed()
        } else {
            Duration::ZERO
        };
        let duration_display = if record_length_ms > 0 || is_recording {
            let record_total =
                Duration::from_millis(record_length_ms).saturating_add(recording_elapsed);
            format!(
                "{} / {}",
                total_dur_str,
                format_duration(record_total, false)
            )
        } else {
            total_dur_str
        };
        let total_size_str = format_size(total_size);

        // Вычисляем текущий индекс файла для отображения в суммирующей строке
        // new_index - это индекс после reverse (0-based в UI)
        // Нужно преобразовать его в номер файла (1-based, от последнего к первому)
        let current_file_num = if new_index >= 0 && new_file_list_len > 0 {
            let ui_index = new_index as usize;
            if ui_index < new_file_list_len {
                // После reverse: номер = total - ui_index
                new_file_list_len - ui_index
            } else {
                0
            }
        } else {
            0
        };

        // Вычисляем общий размер диска (записанных + свободное место)
        let total_disk_size = total_size + free_space;
        let total_disk_str = format_size(total_disk_size);

        let file_name = current_file
            .map(|file| {
                Path::new(&file.path)
                    .file_name()
                    .and_then(|name| name.to_str())
                    .unwrap_or("")
            })
            .unwrap_or("");

        const HINT_MAX_LEN: usize = 100;
        let (cur_start, cur_end, prev_start, prev_end) =
            current_and_prev_file_hints(&self.project.files, current_index, HINT_MAX_LEN);
        let hintbox_prev = if prev_end.is_empty() {
            prev_start.clone()
        } else {
            format!("{}[...]{}", prev_start, prev_end)
        };

        // Вычисляем section titles: самый свежий title для каждого маркера с section=true.
        // Порядковый номер берём тот же, что в столбце маркеров (format_markers_with_ordinals_batch).
        use std::collections::{HashMap, HashSet};
        let mut marker_to_title: HashMap<String, String> = HashMap::new();

        for file in self.project.files.iter().rev() {
            if file.title.is_empty() {
                continue;
            }
            for marker_name in &file.markers {
                if let Some(marker_settings) = self.project.markers.get(marker_name) {
                    if marker_settings.section {
                        marker_to_title
                            .entry(marker_name.clone())
                            .or_insert_with(|| file.title.clone());
                    }
                }
            }
        }

        let mut marker_counts: HashMap<String, usize> = HashMap::new();
        let mut marker_added: HashSet<String> = HashSet::new();
        let mut section_titles = Vec::new();

        for file in &self.project.files {
            if file.title.is_empty() {
                continue;
            }
            for marker_name in &file.markers {
                if let Some(marker_settings) = self.project.markers.get(marker_name) {
                    if !marker_settings.section {
                        continue;
                    }
                    if marker_added.contains(marker_name) {
                        continue;
                    }
                    if let Some(fresh_title) = marker_to_title.get(marker_name) {
                        if fresh_title == &file.title {
                            let ordinal = marker_counts.get(marker_name).copied().unwrap_or(0) + 1;
                            section_titles.push(format!("{}: {}", ordinal, file.title));
                            marker_added.insert(marker_name.clone());
                        }
                    }
                }
            }
            for marker_name in &file.markers {
                *marker_counts.entry(marker_name.clone()).or_insert(0) += 1;
            }
        }

        // Текст для диалога TOC: время начала (hh:mm:ss) и заголовок каждой секции,
        // затем блоки по маркерам (имя маркера и время начала каждой помеченной им записи)
        let mut cumulative_ms: u64 = 0;
        let mut toc_lines: Vec<String> = Vec::new();
        let mut marker_entries: std::collections::HashMap<String, Vec<(u64, String)>> =
            std::collections::HashMap::new();
        let mut section_indices = Vec::new();
        for (index, file) in self.project.files.iter().enumerate() {
            let display_name = if file.title.is_empty() {
                Path::new(&file.path)
                    .file_name()
                    .and_then(|name| name.to_str())
                    .unwrap_or("")
                    .to_string()
            } else {
                file.title.clone()
            };
            let has_section = file.markers.iter().any(|marker_name| {
                self.project
                    .markers
                    .get(marker_name)
                    .map(|settings| settings.section)
                    .unwrap_or(false)
            });
            if has_section {
                section_indices.push(index);
                let start_d = Duration::from_millis(cumulative_ms);
                toc_lines.push(format!(
                    "{}  {}",
                    format_duration(start_d, true),
                    &display_name
                ));
            }
            for marker_name in &file.markers {
                marker_entries
                    .entry(marker_name.clone())
                    .or_default()
                    .push((cumulative_ms, display_name.clone()));
            }
            cumulative_ms = cumulative_ms.saturating_add(file.duration_ms);
        }
        let mut out = toc_lines.join("\n");
        let mut marker_names: Vec<&String> = marker_entries.keys().collect();
        marker_names.sort();
        for marker_name in marker_names {
            let entries = marker_entries.get(marker_name).unwrap();
            out.push_str("\n\n\n");
            out.push_str(marker_name);
            out.push('\n');
            for (ms, name) in entries {
                let start_d = Duration::from_millis(*ms);
                out.push_str(&format!("{}  {}\n", format_duration(start_d, true), name));
            }
        }
        // Метаданные обновляются только при сохранении через диалог, не при каждом обновлении UI

        // All O(N) construction above happens without blocking the GUI's mutex.
        let ui_state = self
            .ui_state
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("UI state not available"))?;
        let mut state = ui_state
            .lock()
            .map_err(|_| anyhow::anyhow!("UI state lock poisoned"))?;
        state.current_file_name = file_name.to_string();
        state.total_files = format!("{}/{}", current_file_num, total_files);
        state.total_duration = duration_display;
        state.total_size = format!("{} / {}", total_size_str, total_disk_str);
        state.is_recording = is_recording;
        state.playback_position = playback_position;
        state.recording_mode = self.recording_mode.as_str().to_string();
        state.hintbox_current_start = cur_start;
        state.hintbox_current_end = cur_end;
        state.hintbox_prev_start = prev_start;
        state.hintbox_prev_end = prev_end;
        state.hintbox_prev = hintbox_prev;
        state.section_titles = section_titles.join(" | ");
        state.dialog_toc_list_text = out;

        // Only structural/data updates invalidate the GUI's full-list snapshot.
        let previous_file_list = std::mem::replace(&mut state.file_list, reversed_file_list);
        state.file_list_version = state.file_list_version.wrapping_add(1);
        state.file_list_dirty_rows.clear();

        // Всегда обновляем, чтобы гарантировать синхронизацию
        state.current_file_index = new_index;

        // Обновляем волновой график
        state.waveform_current = waveform_current;
        state.waveform_prev = waveform_prev;
        self.ui_update_cache
            .record_rebuild(key, total_duration, total_size, section_indices);
        self.file_list_update_pending = false;
        self.status_line_update_pending = false;
        drop(state);
        drop(previous_file_list);

        // Состояние диалога настроек отрывка не обновляется здесь, только при открытии/закрытии

        Ok(())
    }

    /// Enqueue only the most recent desired graph. File reads never run in the command loop.
    fn fill_waveform_for_index(&mut self, index: Option<usize>, is_prev: bool) {
        let slot = if is_prev {
            WaveformSlot::Previous
        } else {
            WaveformSlot::Current
        };
        let path = index
            .and_then(|index| self.project.files.get(index))
            .map(|file| self.resolve_file_path(&file.path));
        let loading = if let Some(path) = path {
            if !self.waveform_loader.request(slot, path) {
                return;
            }
            true
        } else {
            self.waveform_loader.clear(slot);
            false
        };
        if is_prev {
            self.waveform_prev.clear();
            self.waveform_prev_loading = loading;
        } else {
            self.waveform_current.clear();
            self.waveform_current_loading = loading;
        }
        self.publish_waveforms();
    }

    /// O(number of display points), independent of the project's fragment count.
    pub fn publish_waveforms(&self) {
        if let Some(state) = self.ui_state.as_ref() {
            if let Ok(mut state) = state.lock() {
                state.waveform_prev.clone_from(&self.waveform_prev);
                state.waveform_current.clone_from(&self.waveform_current);
                state.waveform_prev_loading = self.waveform_prev_loading;
                state.waveform_current_loading = self.waveform_current_loading;
                state.waveform_version = state.waveform_version.wrapping_add(1);
            }
        }
    }

    pub fn clear_waveform_requests(&mut self) {
        self.waveform_loader.clear(WaveformSlot::Previous);
        self.waveform_loader.clear(WaveformSlot::Current);
        self.waveform_prev_loading = false;
        self.waveform_current_loading = false;
        self.publish_waveforms();
    }

    pub fn poll_waveforms(&mut self) {
        let mut changed = false;
        while let Some(result) = self.waveform_loader.poll() {
            let slot = result.slot();
            let data = match result.samples {
                Ok(samples) => Self::waveform_linear_to_display(&samples),
                Err(error) => {
                    self.report_error(&anyhow::anyhow!(error));
                    Vec::new()
                }
            };
            match slot {
                WaveformSlot::Previous => {
                    self.waveform_prev = data;
                    self.waveform_prev_loading = false;
                }
                WaveformSlot::Current => {
                    self.waveform_current = data;
                    self.waveform_current_loading = false;
                }
            }
            changed = true;
        }
        if changed {
            self.publish_waveforms();
        }
    }

    /// Обновляет волновой график предыдущего файла
    pub fn update_prev_waveform(&mut self) {
        let current_idx = if matches!(&self.state, AppState::Recording { .. }) {
            // The temporary take has no ProjectFile yet. Its predecessor depends on
            // whether it appends, replaces, or follows the selected accepted fragment.
            match self.recording_mode {
                RecordingMode::Append => self.project.files.len(),
                RecordingMode::Update => self.update_recording_index.unwrap_or(0),
                RecordingMode::Insert => self.insert_recording_index.unwrap_or(0),
            }
        } else {
            self.current_index.unwrap_or_else(|| {
                if !self.project.files.is_empty() {
                    self.project.files.len() - 1
                } else {
                    0
                }
            })
        };
        let prev_idx = if current_idx > 0 {
            (0..current_idx).rev().find(|&idx| {
                self.project
                    .files
                    .get(idx)
                    .map(|file| self.resolve_file_path(&file.path).exists())
                    .unwrap_or(false)
            })
        } else {
            None
        };
        self.fill_waveform_for_index(prev_idx, true);
    }

    /// Обновляет волновой график текущего файла
    pub fn update_current_waveform(&mut self) {
        if matches!(&self.state, AppState::Recording { .. }) {
            return;
        }
        let current_idx = self.current_index.unwrap_or_else(|| {
            if !self.project.files.is_empty() {
                self.project.files.len() - 1
            } else {
                0
            }
        });
        self.fill_waveform_for_index(Some(current_idx), false);
    }

    /// Преобразует пиковую амплитуду (0..1) в значение для отображения по шкале dB FS.
    /// Мягкий порог (soft knee): тихий диапазон (-60..KNEE_DB) сжимается в тонкую полосу (0..SILENCE_MAX),
    /// чтобы линия тишины не исчезала и не была толстой; выше колена — обычная шкала.
    fn amplitude_to_level_display(amplitude: f32) -> f32 {
        const MIN_AMP: f32 = 1e-3; // ≈ -60 dB
        const KNEE_DB: f32 = -42.0; // ниже — «тишина», сжимаем в тонкую полосу
        const SILENCE_MAX: f32 = 0.03; // макс. высота отображения тишины (3%)
        let amp = amplitude.clamp(MIN_AMP, 1.0);
        let db = 20.0 * amp.log10();
        if db < KNEE_DB {
            // -60..KNEE_DB → 0..SILENCE_MAX
            let ratio = (db + 60.0) / (KNEE_DB + 60.0);
            ratio.clamp(0.0, 1.0) * SILENCE_MAX
        } else {
            // KNEE_DB..0 → SILENCE_MAX..1
            let ratio = (db - KNEE_DB) / (-KNEE_DB);
            SILENCE_MAX + ratio.clamp(0.0, 1.0) * (1.0 - SILENCE_MAX)
        }
    }

    /// Переводит сэмплы волнового графика (линейная амплитуда 0..1) в шкалу отображения (0..1 по dB FS).
    fn waveform_linear_to_display(samples: &[f32]) -> Vec<f32> {
        samples
            .iter()
            .map(|&sample| Self::amplitude_to_level_display(sample))
            .collect()
    }

    /// Обновляет индикатор уровня записи
    pub fn update_level_indicator(&mut self) {
        while let Ok(level) = self.level_rx.try_recv() {
            let amplitude = level.clamp(0.0, 1.0);
            let normalized_level = Self::amplitude_to_level_display(amplitude);
            self.last_level = normalized_level;
            if let Some(ref mut ui) = self.ui {
                let _ = ui.update_level_indicator(normalized_level);
            } else if let Some(ref ui_state) = self.ui_state {
                if let Ok(mut state) = ui_state.lock() {
                    state.level = normalized_level;
                }
            }
            if let AppState::Recording { .. } = &self.state {
                // Окно 30 с (~50 об/с буфера): заполнение слева, при переполнении — сдвиг
                const RECORDING_WAVEFORM_WINDOW_SAMPLES: usize = 1500;
                self.waveform_current.push(normalized_level);
                if self.waveform_current.len() > RECORDING_WAVEFORM_WINDOW_SAMPLES {
                    self.waveform_current.remove(0);
                }
            }
        }
        // Всегда обновляем уровень из сохраненного значения, даже если нет новых данных
        if let Some(ref mut ui) = self.ui {
            let _ = ui.update_level_indicator(self.last_level);
        } else if let Some(ref ui_state) = self.ui_state {
            if let Ok(mut state) = ui_state.lock() {
                state.level = self.last_level;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        adjacent_section_index, file_list_current_index, update_playing_rows,
        update_recording_row_duration, UiUpdateCache, UiUpdateKey,
    };
    use crate::app::app::RecordingMode;
    use crate::ui::FileInfo;
    use std::time::{Duration, Instant};

    fn idle_key() -> UiUpdateKey {
        UiUpdateKey {
            current_index: Some(4),
            total_files: 5,
            recording_mode: RecordingMode::Append,
            recording_started: None,
            playing_index: None,
            record_length_ms: 100_000,
        }
    }

    #[test]
    fn unchanged_ticks_do_not_rebuild_project_snapshot() {
        let key = idle_key();
        let mut cache = UiUpdateCache::default();
        assert!(cache.needs_rebuild(key, false));
        cache.record_rebuild(key, Duration::from_secs(100), 4000, vec![]);
        for _ in 0..16 {
            assert!(!cache.needs_rebuild(key, false));
            assert!(!cache.needs_navigation_update(key));
        }
        // Title/hint/marker edits need invalidation even when length stays the same.
        assert!(cache.needs_rebuild(key, true));
    }

    #[test]
    fn structural_and_data_transitions_rebuild() {
        let key = idle_key();
        let mut cache = UiUpdateCache::default();
        cache.record_rebuild(key, Duration::ZERO, 0, vec![]);
        for changed in [
            UiUpdateKey {
                total_files: 6,
                ..key
            },
            UiUpdateKey {
                recording_mode: RecordingMode::Insert,
                ..key
            },
            UiUpdateKey {
                recording_started: Some(Instant::now()),
                ..key
            },
            UiUpdateKey {
                record_length_ms: 100_001,
                ..key
            },
        ] {
            assert!(cache.needs_rebuild(changed, false));
        }
    }

    #[test]
    fn selection_and_playback_transitions_only_need_lightweight_updates() {
        let key = idle_key();
        let mut cache = UiUpdateCache::default();
        cache.record_rebuild(key, Duration::ZERO, 0, vec![0, 3]);
        for changed in [
            UiUpdateKey {
                current_index: Some(1),
                ..key
            },
            UiUpdateKey {
                current_index: Some(1),
                playing_index: Some(1),
                ..key
            },
            UiUpdateKey {
                current_index: Some(3),
                playing_index: Some(3),
                ..key
            },
            UiUpdateKey {
                current_index: Some(3),
                ..key
            },
        ] {
            assert!(!cache.needs_rebuild(changed, false));
            assert!(cache.needs_navigation_update(changed));
            cache.record_navigation(changed);
            assert!(!cache.needs_navigation_update(changed));
            assert_eq!(cache.section_indices(), Some([0, 3].as_slice()));
        }
    }

    #[test]
    fn recording_restart_rebuilds_but_elapsed_ticks_do_not() {
        let started = Instant::now();
        let key = UiUpdateKey {
            recording_started: Some(started),
            ..idle_key()
        };
        let mut cache = UiUpdateCache::default();
        cache.record_rebuild(key, Duration::ZERO, 0, vec![]);
        for _ in 0..16 {
            assert!(!cache.needs_rebuild(key, false));
        }
        assert!(cache.needs_rebuild(
            UiUpdateKey {
                recording_started: Some(started + Duration::from_secs(1)),
                ..key
            },
            false
        ));
        // Finishing recording removes temporary rows or restores the U row.
        assert!(cache.needs_rebuild(idle_key(), false));
        assert!(cache.needs_rebuild(
            UiUpdateKey {
                current_index: Some(1),
                ..key
            },
            false,
        ));
    }

    #[test]
    fn recording_modes_select_the_expected_reversed_row() {
        assert_eq!(
            file_list_current_index(3, Some(0), RecordingMode::Append, true),
            0
        );
        assert_eq!(
            file_list_current_index(3, Some(0), RecordingMode::Insert, true),
            2
        );
        assert_eq!(
            file_list_current_index(3, Some(1), RecordingMode::Update, true),
            1
        );
        assert_eq!(
            file_list_current_index(3, Some(0), RecordingMode::Insert, false),
            2
        );
        assert_eq!(
            file_list_current_index(0, Some(0), RecordingMode::Insert, true),
            0
        );
        assert_eq!(
            file_list_current_index(0, None, RecordingMode::Append, false),
            -1
        );
    }

    #[test]
    fn recording_duration_tick_changes_only_the_active_row() {
        for index in [0, 2, 4] {
            let mut rows = vec![FileInfo::default(); 5];
            rows[index].is_recording = true;
            rows[index].duration = "00:00".into();
            assert!(update_recording_row_duration(
                &mut rows,
                index as i32,
                Duration::from_secs(2)
            ));
            assert_eq!(rows[index].duration.as_str(), "00:02");
            assert!(!update_recording_row_duration(
                &mut rows,
                index as i32,
                Duration::from_millis(2050)
            ));
            assert!(rows
                .iter()
                .enumerate()
                .all(|(i, row)| i == index || row.duration.is_empty()));
            assert!(!update_recording_row_duration(
                &mut rows,
                -1,
                Duration::ZERO
            ));
            assert!(!update_recording_row_duration(&mut rows, 5, Duration::ZERO));
        }
    }

    #[test]
    fn playback_patches_only_previous_and_new_reversed_rows() {
        let mut rows = vec![FileInfo::default(); 5];
        assert_eq!(
            update_playing_rows(&mut rows, None, Some(4)),
            [None, Some(0)]
        );
        assert!(rows[0].is_playing);
        assert_eq!(
            update_playing_rows(&mut rows, Some(4), Some(1)),
            [Some(0), Some(3)]
        );
        assert!(!rows[0].is_playing);
        assert!(rows[3].is_playing);
        assert!(rows
            .iter()
            .enumerate()
            .all(|(index, row)| index == 3 || !row.is_playing));
        assert_eq!(
            update_playing_rows(&mut rows, Some(1), Some(1)),
            [None, None]
        );
        assert_eq!(
            update_playing_rows(&mut rows, Some(1), None),
            [Some(3), None]
        );
        assert!(rows.iter().all(|row| !row.is_playing));
        assert_eq!(update_playing_rows(&mut rows, None, Some(5)), [None, None]);
        assert_eq!(update_playing_rows(&mut [], Some(0), None), [None, None]);
    }

    #[test]
    fn section_navigation_uses_strict_neighbors_and_refreshes_with_structure() {
        let key = idle_key();
        let mut cache = UiUpdateCache::default();
        assert_eq!(cache.section_indices(), None);
        cache.record_rebuild(key, Duration::ZERO, 0, vec![0, 2, 4]);
        let indices = cache.section_indices().unwrap();
        assert_eq!(adjacent_section_index(indices, 0, false), None);
        assert_eq!(adjacent_section_index(indices, 4, true), None);
        assert_eq!(adjacent_section_index(indices, 2, false), Some(0));
        assert_eq!(adjacent_section_index(indices, 2, true), Some(4));
        assert_eq!(adjacent_section_index(indices, 3, false), Some(2));
        assert_eq!(adjacent_section_index(indices, 3, true), Some(4));
        cache.record_rebuild(key, Duration::ZERO, 0, vec![1]);
        assert_eq!(
            adjacent_section_index(cache.section_indices().unwrap(), 0, true),
            Some(1)
        );
        assert_eq!(adjacent_section_index(&[], 0, true), None);
        assert_eq!(adjacent_section_index(&[], 0, false), None);
    }
}
