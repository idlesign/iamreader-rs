use crate::app::app::{App, AppState, RecordingMode, FILE_LIST_UPDATE_THROTTLE_MS};
use crate::ui::FileInfo;
use crate::utils::{format_duration, format_size, reverse_and_reindex_file_list, current_and_prev_file_hints, format_markers_with_ordinals_batch};
use crate::utils::indexes::orig_to_ui_index;
use crate::utils::stats::{calculate_total_duration, calculate_sizes, get_free_space};
use crate::audio::waveform::read_waveform_samples;
use std::path::Path;
use std::time::{Duration, Instant};
use anyhow::Result;
use log::debug;

impl App {
    /// Обновляет UI. При skip_file_list_rebuild=true не пересобирает список (быстрый путь для Goto/Prev/Next).
    /// При смене индекса прокрутка выполняется только если строка не видна.
    pub fn update_ui(&mut self, skip_file_list_rebuild: bool) -> Result<()> {
        let current_file = self.current_index
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
        
        let ui = self.ui.as_mut().ok_or_else(|| anyhow::anyhow!("UI not available"))?;

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
                || self.last_file_list_update
                    .map(|instant| instant.elapsed() >= Duration::from_millis(FILE_LIST_UPDATE_THROTTLE_MS))
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
        if self.ui.is_some() {
            let _ = self.update_ui(skip_file_list_rebuild);
        } else if self.ui_state.is_some() {
            let _ = self.update_ui_state();
        }
    }
    
    /// Обновляет диалог настроек маркеров в UI
    pub fn update_dialog_markers(&self) {
        if let Some(ref ui) = self.ui {
            let _ = ui.update_dialog_markers();
        }
    }
    
    /// Обновляет UI state (для headless режима)
    pub fn update_ui_state(&mut self) -> Result<()> {
        // Используем self.current_index, но если он None, используем последний файл (самый свежий)
        let current_index = self.current_index.or_else(|| {
            if !self.project.files.is_empty() {
                Some(self.project.files.len() - 1)
            } else {
                None
            }
        });
        
        // Волновой график предыдущей/текущей записи обновляется только при смене текущей записи (см. goto_to_index, start_playback, stop_playback, finish_recording, SaveChunkSettings, main).
        
        let current_file = current_index
            .and_then(|index| self.project.files.get(index));
        
        let total_duration = calculate_total_duration(&self.project.files);
        let total_size = calculate_sizes(&self.project.files);
        let free_space = get_free_space(&self.chunks_dir);
        let _total_files = self.project.files.len();
        let waveform_current = self.waveform_current.clone();
        let waveform_prev = self.waveform_prev.clone();
        let files = self.project.files.clone();
        let is_recording = matches!(&self.state, AppState::Recording { .. });
        let is_playing = matches!(&self.state, AppState::Playing { .. });
        
        // Для определения воспроизведения используем current_index из self, а не из параметра
        // При воспроизведении current_index указывает на файл, который воспроизводится
        let playing_index = if is_playing {
            if let AppState::Playing { current_index: play_idx, .. } = &self.state {
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
            if let AppState::Playing { start_time, current_index: play_idx, .. } = &self.state {
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
                        (adjusted_elapsed.as_secs_f32() / file_duration.as_secs_f32()).min(1.0).max(0.0)
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
        
        let ui_state = self.ui_state.as_ref().ok_or_else(|| anyhow::anyhow!("UI state not available"))?;
        let mut state = ui_state.lock().unwrap();
        
        // Обновляем список файлов
        // Показываем все файлы из проекта (все считаются завершенными)
        let recording_duration = if let AppState::Recording { start_time, .. } = &self.state {
            start_time.elapsed()
        } else {
            Duration::ZERO
        };
        
        let mut cumulative_duration = Duration::ZERO;
        let markers_batch = format_markers_with_ordinals_batch(&files);

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
                let name = path.file_name().and_then(|name| name.to_str()).unwrap_or("").to_string();
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
                    hint: f.hint.clone().into(),
                }
            })
            .collect();

        let total_files = new_file_list.len();

        let has_temp = temp_file_path.is_some() && is_recording && self.recording_mode != RecordingMode::Update;

        if let Some(ref temp_path) = temp_file_path {
            if is_recording {
                let path = Path::new(temp_path);
                let name = path.file_name().and_then(|name| name.to_str()).unwrap_or("").to_string();
                match self.recording_mode {
                    RecordingMode::Update => {}
                    RecordingMode::Insert if current_index.is_some() => {
                        let insert_at = current_index.unwrap() + 1;
                        let cum_insert: Duration = files.iter().take(insert_at).map(|file| Duration::from_millis(file.duration_ms)).sum();
                        let start_time_str = format_duration(cum_insert, false);
                        new_file_list.insert(insert_at, FileInfo {
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
                            hint: String::new().into(),
                        });
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
                            hint: String::new().into(),
                        });
                    }
                }
            }
        }

        let reversed_file_list = reverse_and_reindex_file_list(new_file_list);
        let new_file_list_len = reversed_file_list.len();

        let new_index = if has_temp && self.recording_mode == RecordingMode::Insert && current_index.is_some() {
            let insert_at = current_index.unwrap() + 1;
            (new_file_list_len - 1 - insert_at) as i32
        } else if has_temp {
            0
        } else if let Some(orig_idx) = current_index {
            if new_file_list_len > 0 {
                orig_to_ui_index(orig_idx, new_file_list_len)
            } else {
                -1
            }
        } else {
            // Если current_index не установлен, используем самый свежий файл (последний в project.files)
            // После reverse он становится первым (индекс 0)
            if new_file_list_len > 0 {
                0 as i32
            } else {
                -1
            }
        };
        
        // Обновляем статусную строку
        let total_dur_str = format_duration(total_duration, false);
        let record_length_ms = self.project.stats.record_length;
        let recording_elapsed = if let AppState::Recording { start_time, .. } = &self.state {
            start_time.elapsed()
        } else {
            Duration::ZERO
        };
        let duration_display = if record_length_ms > 0 || is_recording {
            let record_total = Duration::from_millis(record_length_ms).saturating_add(recording_elapsed);
            format!("{} / {}", total_dur_str, format_duration(record_total, false))
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
            .map(|file| Path::new(&file.path).file_name().and_then(|name| name.to_str()).unwrap_or(""))
            .unwrap_or("");

        state.current_file_name = file_name.to_string();
        state.total_files = format!("{}/{}", current_file_num, total_files);
        state.total_duration = duration_display;
        state.total_size = format!("{} / {}", total_size_str, total_disk_str);
        state.is_recording = is_recording;
        state.playback_position = playback_position;
        state.recording_mode = self.recording_mode.as_str().to_string();

        const HINT_MAX_LEN: usize = 100;
        let (cur_start, cur_end, prev_start, prev_end) =
            current_and_prev_file_hints(&self.project.files, current_index, HINT_MAX_LEN);
        state.hintbox_current_start = cur_start.clone();
        state.hintbox_current_end = cur_end.clone();
        state.hintbox_prev_start = prev_start.clone();
        state.hintbox_prev_end = prev_end.clone();
        state.hintbox_prev = if state.hintbox_prev_end.is_empty() {
            state.hintbox_prev_start.clone()
        } else {
            format!("{}[...]{}", state.hintbox_prev_start, state.hintbox_prev_end)
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
                        marker_to_title.entry(marker_name.clone()).or_insert_with(|| file.title.clone());
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

        state.section_titles = section_titles.join(" | ");

        // Текст для диалога TOC: время начала (hh:mm:ss) и заголовок каждой секции,
        // затем блоки по маркерам (имя маркера и время начала каждой помеченной им записи)
        let mut cumulative_ms: u64 = 0;
        let mut toc_lines: Vec<String> = Vec::new();
        let mut marker_entries: std::collections::HashMap<String, Vec<(u64, String)>> = std::collections::HashMap::new();
        for file in &self.project.files {
            let display_name = if file.title.is_empty() {
                Path::new(&file.path).file_name().and_then(|name| name.to_str()).unwrap_or("").to_string()
            } else {
                file.title.clone()
            };
            let has_section = file.markers.iter().any(|marker_name| {
                self.project.markers.get(marker_name).map(|settings| settings.section).unwrap_or(false)
            });
            if has_section {
                let start_d = Duration::from_millis(cumulative_ms);
                toc_lines.push(format!("{}  {}", format_duration(start_d, true), &display_name));
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
        state.dialog_toc_list_text = out;

        // Метаданные обновляются только при сохранении через диалог, не при каждом обновлении UI
        
        // Всегда обновляем список файлов
        state.file_list = reversed_file_list;
        state.file_list_version = state.file_list_version.wrapping_add(1);

        // Всегда обновляем, чтобы гарантировать синхронизацию
        state.current_file_index = new_index;

        // Обновляем волновой график
        state.waveform_current = waveform_current;
        state.waveform_prev = waveform_prev;
        
        // Состояние диалога настроек отрывка не обновляется здесь, только при открытии/закрытии
        
        Ok(())
    }

    /// Заполняет волновой график по индексу файла в указанный слот (prev или current).
    fn fill_waveform_for_index(&mut self, index: Option<usize>, is_prev: bool) {
        let cached_matches = if is_prev {
            self.cached_prev_index == index
        } else {
            self.cached_current_index == index
        };
        if cached_matches {
            if let Some(idx) = index {
                if let Some(file) = self.project.files.get(idx) {
                    if let Some(cached) = self.waveform_cache.get(&file.path) {
                        if is_prev {
                            self.waveform_prev.clone_from(cached);
                        } else {
                            self.waveform_current.clone_from(cached);
                        }
                        return;
                    }
                }
            } else {
                if is_prev {
                    self.waveform_prev.clear();
                } else {
                    self.waveform_current.clear();
                }
                return;
            }
        }

        if is_prev {
            self.waveform_prev.clear();
        } else {
            self.waveform_current.clear();
        }
        if let Some(idx) = index {
            if let Some(file) = self.project.files.get(idx) {
                let path = Path::new(&file.path);
                if path.exists() {
                    if let Ok(data) = read_waveform_samples(path, 500, self.debug) {
                        let display_data = Self::waveform_linear_to_display(&data);
                        self.waveform_cache.insert(file.path.clone(), display_data.clone());
                        if is_prev {
                            self.waveform_prev = display_data;
                            self.cached_prev_index = Some(idx);
                        } else {
                            self.waveform_current = display_data;
                            self.cached_current_index = Some(idx);
                        }
                        if self.debug {
                            let len = if is_prev { self.waveform_prev.len() } else { self.waveform_current.len() };
                            debug!("Updated waveform: {} samples from file: {:?}", len, path);
                        }
                    } else if self.debug {
                        debug!("Failed to read waveform samples from: {:?}", path);
                    }
                } else if self.debug {
                    debug!("File does not exist: {:?}", path);
                }
            } else if is_prev {
                self.cached_prev_index = None;
            } else {
                self.cached_current_index = None;
            }
        } else {
            if is_prev {
                self.cached_prev_index = None;
            } else {
                self.cached_current_index = None;
            }
            if self.debug {
                debug!("No file for waveform at index");
            }
        }
    }

    /// Обновляет волновой график предыдущего файла
    pub fn update_prev_waveform(&mut self) {
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
        let prev_idx = if current_idx > 0 {
            (0..current_idx).rev().find(|&idx| {
                self.project.files.get(idx).map(|file| Path::new(&file.path).exists()).unwrap_or(false)
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
        const MIN_AMP: f32 = 1e-3;       // ≈ -60 dB
        const KNEE_DB: f32 = -42.0;     // ниже — «тишина», сжимаем в тонкую полосу
        const SILENCE_MAX: f32 = 0.03;  // макс. высота отображения тишины (3%)
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
        samples.iter().map(|&sample| Self::amplitude_to_level_display(sample)).collect()
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
