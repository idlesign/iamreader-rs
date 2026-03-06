use crate::app::app::{App, AppState, RecordingMode};
use crate::utils::keyboard::Action;
use crate::project;
use crate::project::project::{Project, MarkerSettingsData};
use crate::project::markers::compute_effective_durations_ms;
use crate::utils::stats::get_wav_spec;
use crate::utils::indexes::{ui_to_orig_index, find_section_ui_index};
use anyhow::Result;
use log::{info, warn, debug};
use std::path::Path;
use std::sync::Arc;
use std::time::Instant;

impl App {
    /// Обрабатывает действие пользователя
    pub fn handle_action(&mut self, action: Action) -> Result<()> {
        match action {
            Action::Record { duration_secs } => {
                // Если уже идёт запись — останавливаем и удаляем её, начинаем новую (не трогая сохранённые файлы).
                // Иначе — начинаем запись с заменой текущего файла.
                let was_recording = matches!(&self.state, AppState::Recording { .. });
                self.start_recording_with_duration(duration_secs, !was_recording)?;
            },
            Action::Ok => {
                if matches!(&self.state, AppState::Recording { .. }) {
                    let was_edit_mode = self.recording_mode == RecordingMode::Update || self.recording_mode == RecordingMode::Insert;
                    self.finish_recording()?;
                    if !was_edit_mode {
                        self.start_recording_with_duration(None, false)?;
                    }
                } else {
                    self.start_recording_with_duration(None, false)?;
                }
            },
            Action::Stop => {
                // Останавливаем запись, если она идёт
                if matches!(&self.state, AppState::Recording { .. }) {
                    self.stop_recording()?;
                }
                // Останавливаем воспроизведение, если оно идёт
                if matches!(&self.state, AppState::Playing { .. }) {
                    self.stop_playback()?;
                }
            },
            Action::Prev => {
                // a - предыдущая (более старая запись) = увеличение UI индекса
                if let Some((current_orig, current_ui)) = self.current_orig_and_ui() {
                    if (current_ui as usize) + 1 < self.project.files.len() {
                        if self.debug {
                            debug!("Action::Prev: current_orig={}, current_ui={}, moving to ui_index={}", current_orig, current_ui, current_ui + 1);
                        }
                        self.goto_to_index(Some(current_ui + 1), true)?;
                    }
                }
            },
            Action::Next => {
                // f - следующая (более свежая запись) = уменьшение UI индекса
                if let Some((current_orig, current_ui)) = self.current_orig_and_ui() {
                    if current_ui > 0 {
                        if self.debug {
                            debug!("Action::Next: current_orig={}, current_ui={}, moving to ui_index={}", current_orig, current_ui, current_ui - 1);
                        }
                        self.goto_to_index(Some(current_ui - 1), true)?;
                    }
                }
            },
            Action::PrevSect => self.handle_prev_sect()?,
            Action::NextSect => self.handle_next_sect()?,
            Action::ModeUpdate => {
                if matches!(&self.state, AppState::Recording { .. }) {
                    return Ok(());
                }
                if !self.project.files.is_empty() {
                    self.recording_mode = if self.recording_mode == RecordingMode::Update {
                        RecordingMode::Append
                    } else {
                        self.update_recording_path = None;
                        self.update_recording_index = None;
                        RecordingMode::Update
                    };
                    if self.debug {
                        debug!("Recording mode: {:?}", self.recording_mode);
                    }
                    self.notify_ui_refresh(true, false);
                }
            }
            Action::ModeInsert => {
                if matches!(&self.state, AppState::Recording { .. }) {
                    return Ok(());
                }
                if !self.project.files.is_empty() {
                    self.recording_mode = if self.recording_mode == RecordingMode::Insert {
                        RecordingMode::Append
                    } else {
                        self.insert_recording_path = None;
                        self.insert_recording_index = None;
                        RecordingMode::Insert
                    };
                    if self.debug {
                        debug!("Recording mode: {:?}", self.recording_mode);
                    }
                    self.notify_ui_refresh(true, false);
                }
            }
            Action::Play => self.start_playback()?,
            Action::Goto { index, play } => self.goto_to_index(index, play)?,
            Action::SearchHintUp(ref query) => self.search_hint(query, false)?,
            Action::SearchHintDown(ref query) => self.search_hint(query, true)?,
            Action::Shutdown => {
                self.should_exit = true;
                info!("Shutdown command received");
            }
            Action::SaveMeta(data) => {
                let project::MetaData { title, author, year, hint, reader, format_audio, normalize, cover, section_split, denoise } = data;
                self.project.settings.format_audio = format_audio;
                self.project.settings.normalize = normalize;
                self.project.settings.cover = cover;
                self.project.settings.section_split = section_split;
                self.project.settings.denoise = denoise;
                self.project.meta.title = title;
                self.project.meta.author = author;
                self.project.meta.year = year;
                self.project.meta.hint = hint;
                self.project.meta.reader = reader;
                self.project.save(&self.project_path)?;
                info!("Metadata saved");
            }
            Action::SaveChunkSettings(data) => {
                let project::ChunkSettingsData { ui_index, title, author, year, hint } = data;
                let total_files = self.project.files.len();
                let orig_idx = match ui_to_orig_index(ui_index, total_files) {
                    Some(idx) => idx,
                    None => return Ok(()),
                };
                if orig_idx < self.project.files.len() {
                    self.project.files[orig_idx].title = title;
                    self.project.files[orig_idx].author = author;
                    self.project.files[orig_idx].year = year;
                    self.project.files[orig_idx].hint = hint;
                    self.project.save(&self.project_path)?;
                    info!("Chunk settings saved for file at index {}", orig_idx);
                    // Устанавливаем current_index в App, чтобы при обновлении UI current_file_index пересчитался правильно
                    self.current_index = Some(orig_idx);
                    if self.debug {
                        debug!("SaveChunkSettings: saved for ui_index={}, orig_idx={}, current_index={:?}", 
                            ui_index, orig_idx, self.current_index);
                    }
                    self.update_prev_waveform();
                    self.update_current_waveform();
                    // Обновляем UI, чтобы отобразить изменения (title, hint, markers)
                    self.update_ui_after_change();
                    // Диалог закрывается автоматически через callback в UI
                }
            }
            Action::AddMarker { marker } => {
                // marker здесь - это shortcut (цифра 0-9)
                // Ищем маркер с таким shortcut
                let marker_name = self.project.markers.iter()
                    .find(|(_, settings)| {
                        settings.shortcut.as_ref().map(|s| s.as_str()) == Some(marker.as_str())
                    })
                    .map(|(name, _)| name.clone());
                
                if let Some(marker_name) = marker_name {
                    // Добавляем маркер к текущему файлу
                    // Используем current_index из App, или вычисляем из UIState
                    let current_idx = if let Some(idx) = self.current_index {
                        Some(idx)
                    } else if let Some(ref ui_state) = self.ui_state {
                        if let Ok(state) = ui_state.lock() {
                            let total_files = self.project.files.len();
                            ui_to_orig_index(state.current_file_index, total_files)
                                .filter(|&orig| orig < self.project.files.len())
                        } else {
                            None
                        }
                    } else {
                        None
                    };
                    
                    if let Some(current_idx) = current_idx {
                        if current_idx < self.project.files.len() {
                            let file = &mut self.project.files[current_idx];
                            // Переключаем маркер: если есть - удаляем, если нет - добавляем
                            if file.markers.contains(&marker_name) {
                                file.markers.retain(|m| m != &marker_name);
                                crate::project::markers::normalize_markers(file);
                                self.project.save(&self.project_path)?;
                                info!("Marker '{}' removed from file at index {}", marker_name, current_idx);
                                // Обновляем UI, чтобы отобразить удаление маркера
                                self.update_ui_after_change();
                            } else {
                                file.markers.push(marker_name.clone());
                                crate::project::markers::normalize_markers(file);
                                // Если маркер имеет section=true, устанавливаем title
                                if let Some(marker_settings) = self.project.markers.get(&marker_name) {
                                    if marker_settings.section && file.title.is_empty() {
                                        let file_number = current_idx + 1;
                                        file.title = format!("Title for {}", file_number);
                                    }
                                }
                                self.project.save(&self.project_path)?;
                                info!("Marker '{}' added to file at index {}", marker_name, current_idx);
                                // Обновляем UI, чтобы отобразить новый маркер
                                self.update_ui_after_change();
                            }
                        }
                    }
                } else {
                    warn!("No marker found with shortcut '{}'", marker);
                }
            }
            Action::AddMarkers { file_index, markers } => {
                if let Some(orig_idx) = Project::file_index_1based_to_orig(file_index, self.project.files.len()) {
                    let file = &mut self.project.files[orig_idx];
                    let mut has_section_marker = false;
                    for marker in &markers {
                        if !file.markers.contains(marker) {
                            file.markers.push(marker.clone());
                            // Проверяем, есть ли среди добавляемых маркеров хотя бы один с section=true
                            if let Some(marker_settings) = self.project.markers.get(marker) {
                                if marker_settings.section {
                                    has_section_marker = true;
                                }
                            }
                        }
                    }
                    // Если добавлен маркер с section=true и title пустой, устанавливаем title
                    if has_section_marker && file.title.is_empty() {
                        let file_number = orig_idx + 1;
                        file.title = format!("Title for {}", file_number);
                    }
                    crate::project::markers::normalize_markers(file);
                    self.project.save(&self.project_path)?;
                    info!("Markers added to file at index {}", orig_idx);
                }
            }
            Action::RemoveMarkers { file_index, markers } => {
                if let Some(orig_idx) = Project::file_index_1based_to_orig(file_index, self.project.files.len()) {
                    let file = &mut self.project.files[orig_idx];
                    file.markers.retain(|m| !markers.contains(m));
                    crate::project::markers::normalize_markers(file);
                    self.project.save(&self.project_path)?;
                    info!("Markers removed from file at index {}", orig_idx);
                }
            }
            Action::SetMarkers { file_index, markers } => {
                if let Some(orig_idx) = Project::file_index_1based_to_orig(file_index, self.project.files.len()) {
                    // Убираем дубликаты и сортируем
                    let mut markers_vec: Vec<String> = markers
                        .into_iter()
                        .collect::<std::collections::HashSet<_>>()
                        .into_iter()
                        .collect();
                    markers_vec.sort();
                    self.project.files[orig_idx].markers = markers_vec;
                    self.project.save(&self.project_path)?;
                    info!("Markers set for file at index {}", orig_idx);
                }
            }
            Action::OpenMarkerSettings => {
                // Показываем маркеры из project.markers (определенные для проекта)
                let mut markers_list: Vec<String> = self.project.markers.keys().cloned().collect();
                markers_list.sort();
                
                // Открываем диалог с собранным списком маркеров
                if let Some(ref ui_state) = self.ui_state {
                    if let Ok(mut state) = ui_state.lock() {
                        state.dialog_markers_list = markers_list.clone();
                        state.dialog_markers_open = true;
                        // Если есть маркеры, выбираем первый
                        if !markers_list.is_empty() {
                            state.dialog_markers_selected_index = 0;
                            // Загружаем настройки первого маркера
                            let first_marker = &markers_list[0];
                            if let Some(settings) = self.project.markers.get(first_marker) {
                                crate::project::markers::load_marker_settings_to_state(&mut state, settings);
                            } else {
                                crate::project::markers::set_default_marker_settings_to_state(&mut state);
                            }
                        } else {
                            state.dialog_markers_selected_index = -1;
                            crate::project::markers::set_default_marker_settings_to_state(&mut state);
                        }
                    }
                }
                // Обновляем диалог сразу для установки содержимого
                self.update_dialog_markers();
            }
            Action::LoadMarkerSettings { marker } => {
                // Загружаем настройки выбранного маркера
                if let Some(ref ui_state) = self.ui_state {
                    if let Ok(mut state) = ui_state.lock() {
                        if let Some(settings) = self.project.markers.get(&marker) {
                            crate::project::markers::load_marker_settings_to_state(&mut state, settings);
                        } else {
                            crate::project::markers::set_default_marker_settings_to_state(&mut state);
                        }
                    }
                }
                // Таймер обновит состояние диалога автоматически
            }
            Action::SaveMarkerSettings(data) => self.handle_save_marker_settings(data)?,
            Action::UpdateFilesMeta => self.handle_update_files_meta()?,
            Action::AddMarkerDefinition { alias } => self.handle_add_marker_definition(alias)?,
            Action::Compile => self.handle_compile()?,
            Action::CompileCancel => {
                if let Some(ref c) = self.compile_cancel {
                    c.store(true, std::sync::atomic::Ordering::Relaxed);
                }
                if let Some(ref ui_state) = self.ui_state {
                    if let Ok(mut state) = ui_state.lock() {
                        state.compile_progress = 0.0;
                        state.compile_stage = "Cancelling…".to_string();
                        state.dialog_compile_eta_secs = -1.0;
                    }
                }
            }
            Action::Transcribe { file_index } => {
                let total_files = self.project.files.len();
                if total_files == 0 {
                    warn!("No files to transcribe");
                    return Ok(());
                }
                // file_index в команде - 1-based номер в UI-порядке; 1 = самый новый
                let ui_idx = file_index - 1;
                if let Some(orig_idx) = ui_to_orig_index(ui_idx, total_files) {
                    self.start_transcription_for_file(orig_idx)?;
                } else {
                    warn!("Invalid file index: {} (total files: {})", file_index, total_files);
                }
            }
            Action::OpenShortcutsDialog => self.handle_open_shortcuts_dialog()?,
            Action::OpenDeleteChunkDialog => self.handle_open_delete_chunk_dialog()?,
            Action::ConfirmDeleteChunk { ui_index } => self.handle_confirm_delete_chunk(ui_index)?,
            Action::None => {}
        }
        Ok(())
    }

    fn handle_save_marker_settings(&mut self, data: MarkerSettingsData) -> Result<()> {
        use project::{MarkerSettings, MarkerAssets, MarkerAsset};
        use crate::utils::parse::{parse_reduction, parse_repeat, parse_optional_string};
        let MarkerSettingsData { marker, title, hint, shortcut, begin_audio, begin_kind, begin_reduction, begin_repeat, end_audio, end_kind, end_reduction, end_repeat, section } = data;
        let begin_reduction_opt = parse_reduction(&begin_reduction);
        let begin_repeat_opt = parse_repeat(&begin_repeat);
        let end_reduction_opt = parse_reduction(&end_reduction);
        let end_repeat_opt = parse_repeat(&end_repeat);
        let shortcut_opt = parse_optional_string(&shortcut);
        let settings = MarkerSettings {
            title: title.clone(),
            author: String::new(),
            year: String::new(),
            hint: hint.clone(),
            shortcut: shortcut_opt,
            assets: MarkerAssets {
                begin: MarkerAsset {
                    audio: begin_audio.clone(),
                    kind: begin_kind.clone(),
                    reduction: begin_reduction_opt,
                    repeat: begin_repeat_opt,
                },
                end: MarkerAsset {
                    audio: end_audio.clone(),
                    kind: end_kind.clone(),
                    reduction: end_reduction_opt,
                    repeat: end_repeat_opt,
                },
            },
            section,
        };
        self.project.markers.insert(marker.clone(), settings);
        self.project.save(&self.project_path)?;
        info!("Marker settings saved for marker: {}", marker);
        Ok(())
    }

    fn handle_update_files_meta(&mut self) -> Result<()> {
        let project_dir = self.project_path.parent().unwrap_or_else(|| Path::new("."));
        if let Err(e) = self.project.update_files_meta_from_disk(project_dir) {
            warn!("Update files meta from disk failed: {}", e);
        } else if let Err(e) = self.project.save(&self.project_path) {
            warn!("Save project after update meta failed: {}", e);
        } else {
            info!("Files meta updated from disk");
        }
        let sound_dir = self.project_path.parent().unwrap_or_else(|| Path::new("."));
        let (sample_rate, channels) = if let Some(cached) = self.wav_spec_cache {
            cached
        } else {
            let spec = self.project.files.iter()
                .find_map(|f| {
                    let p = Path::new(&f.path);
                    if p.exists() {
                        get_wav_spec(p).ok().map(|(sr, ch)| (sr, ch))
                    } else {
                        None
                    }
                })
                .unwrap_or((44100, 2));
            self.wav_spec_cache = Some(spec);
            spec
        };
        let effective = compute_effective_durations_ms(
            &self.project.files,
            &self.project.markers,
            sound_dir,
            sample_rate,
            channels,
        ).ok();
        if let Some(ref eff) = effective {
            for (file, &dur_ms) in self.project.files.iter_mut().zip(eff.iter()) {
                file.duration_ms = dur_ms;
            }
            if let Err(e) = self.project.save(&self.project_path) {
                warn!("Save project after effective durations failed: {}", e);
            }
        }
        self.update_ui_after_change();
        Ok(())
    }

    fn handle_add_marker_definition(&mut self, alias: String) -> Result<()> {
        let alias = alias.trim().to_string();
        if alias.is_empty() {
            return Ok(());
        }
        if self.project.markers.contains_key(&alias) {
            warn!("Marker '{}' already exists", alias);
            return Ok(());
        }
        use project::{MarkerSettings, MarkerAssets, MarkerAsset};
        let settings = MarkerSettings {
            title: String::new(),
            author: String::new(),
            year: String::new(),
            hint: String::new(),
            shortcut: None,
            assets: MarkerAssets {
                begin: MarkerAsset {
                    audio: String::new(),
                    kind: "add".to_string(),
                    reduction: Some(0),
                    repeat: Some(1),
                },
                end: MarkerAsset {
                    audio: String::new(),
                    kind: "add".to_string(),
                    reduction: Some(0),
                    repeat: Some(1),
                },
            },
            section: false,
        };
        self.project.markers.insert(alias.clone(), settings);
        self.project.save(&self.project_path)?;
        info!("New marker added: {}", alias);
        if let Some(ref ui_state) = self.ui_state {
            if let Ok(mut state) = ui_state.lock() {
                let mut markers_list: Vec<String> = self.project.markers.keys().cloned().collect();
                markers_list.sort();
                state.dialog_markers_list = markers_list.clone();
                if let Some(index) = markers_list.iter().position(|m| m == &alias) {
                    state.dialog_markers_selected_index = index as i32;
                    if let Some(settings) = self.project.markers.get(&alias) {
                        crate::project::markers::load_marker_settings_to_state(&mut state, settings);
                    }
                }
            }
        }
        self.update_dialog_markers();
        Ok(())
    }

    fn handle_compile(&mut self) -> Result<()> {
        let project_files = self.project.files.clone();
        let project_path = self.project_path.clone();
        let markers = self.project.markers.clone();
        let meta = self.project.meta.clone();
        let settings = self.project.settings.clone();
        let ui_state = self.ui_state.clone();
        let debug = self.debug;
        let cancel = Arc::new(std::sync::atomic::AtomicBool::new(false));
        self.compile_cancel = Some(cancel.clone());
        std::thread::spawn(move || {
            let ui_state_clone = ui_state.clone();
            if let Err(err) = crate::project::compiler::compile_wav_files_static(
                project_files, project_path, markers, meta, settings, ui_state, Some(cancel), debug,
            ) {
                log::error!("Compilation error: {}", err);
                if let Some(ref ui_state) = ui_state_clone {
                    if let Ok(mut state) = ui_state.lock() {
                        state.compile_stage = format!("Error: {}", err);
                        state.compile_finished_at = Some(Instant::now());
                    }
                }
            }
        });
        Ok(())
    }

    fn handle_open_shortcuts_dialog(&mut self) -> Result<()> {
        let keys = &self.project.settings.keys;
        let mut lines = vec![
            format!("{} — Start/restart recording", keys.record),
            format!("{} — Phrase OK, new recording", keys.ok),
            format!("{} — Stop recording", keys.stop),
            format!("{} — Previous recording", keys.prev),
            format!("{} — Next recording", keys.next),
            format!("{} — Playback", keys.play),
            format!("{} — Next chapter", keys.chapter_next),
            format!("{} — Previous chapter", keys.chapter_prev),
        ];
        let mut markers_with_shortcut: Vec<_> = self.project.markers
            .iter()
            .filter_map(|(alias, marker_settings)| marker_settings.shortcut.as_ref().map(|shortcut_str| (shortcut_str.clone(), alias.clone())))
            .collect();
        markers_with_shortcut.sort_by(|first, second| first.0.cmp(&second.0));
        for (shortcut, alias) in markers_with_shortcut {
            lines.push(format!("{} — marker \"{}\"", shortcut, alias));
        }
        let text = lines.join("\n");
        if let Some(ref state) = self.ui_state {
            if let Ok(mut guard) = state.lock() {
                guard.dialog_shortcuts_list_text = text;
                guard.dialog_shortcuts_open = true;
            }
        }
        Ok(())
    }

    fn handle_open_delete_chunk_dialog(&mut self) -> Result<()> {
        info!("OpenDeleteChunkDialog: action received");
        if matches!(&self.state, AppState::Recording { .. }) {
            info!("OpenDeleteChunkDialog: skipped (recording)");
            return Ok(());
        }
        let total_files = self.project.files.len();
        if total_files == 0 {
            info!("OpenDeleteChunkDialog: skipped (no files)");
            return Ok(());
        }
        let Some((current_orig, current_ui)) = self.current_orig_and_ui() else {
            info!("OpenDeleteChunkDialog: skipped (current_orig_and_ui=None, current_index={:?})", self.current_index);
            return Ok(());
        };
        let Some(file) = self.project.files.get(current_orig) else {
            info!("OpenDeleteChunkDialog: skipped (no file at orig={})", current_orig);
            return Ok(());
        };
        let num = total_files - (current_ui as usize);
        let title = if file.title.is_empty() { "—" } else { file.title.as_str() };
        let text = format!("Запись №{}: {}, файл: {}", num, title, file.path);
        if let Some(ref state) = self.ui_state {
            if let Ok(mut guard) = state.lock() {
                guard.dialog_delete_text = text.clone();
                guard.dialog_delete_file_index = current_ui;
                guard.dialog_delete_open = true;
                info!("OpenDeleteChunkDialog: opened for ui_index={}, text={}", current_ui, text);
            }
        }
        Ok(())
    }

    fn handle_confirm_delete_chunk(&mut self, ui_index: i32) -> Result<()> {
        let total_files = self.project.files.len();
        if let Some(orig_index) = ui_to_orig_index(ui_index, total_files) {
            if let Some(path) = self.project.remove_file_at(orig_index) {
                let full_path = self.chunks_dir.join(&path);
                if full_path.exists() {
                    let _ = std::fs::remove_file(&full_path);
                }
                let _ = crate::audio::waveform::remove_waveform_cache(&full_path);
                self.waveform_cache.remove(&path);
                if matches!(&self.state, AppState::Playing { .. }) {
                    self.stop_playback()?;
                }
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
                self.project.save(&self.project_path)?;
                self.file_list_update_pending = true;
                if let Some(ref state) = self.ui_state {
                    if let Ok(mut guard) = state.lock() {
                        guard.dialog_delete_open = false;
                    }
                }
                info!("Deleted chunk at orig_index={}, path={}", orig_index, path);
            }
        }
        Ok(())
    }

    fn handle_prev_sect(&mut self) -> Result<()> {
        let total_files = self.project.files.len();
        if let Some((_current_orig, current_ui)) = self.current_orig_and_ui() {
            let has_section = |orig: usize| -> bool {
                self.project.files.get(orig).map(|f| {
                    f.markers.iter().any(|m| self.project.markers.get(m).map(|s| s.section).unwrap_or(false))
                }).unwrap_or(false)
            };
            if let Some(ui) = find_section_ui_index(total_files, current_ui, false, has_section) {
                if self.debug {
                    debug!("Action::PrevSect: current_ui={}, moving to section ui_index={}", current_ui, ui);
                }
                self.goto_to_index(Some(ui), true)?;
            }
        }
        Ok(())
    }

    fn handle_next_sect(&mut self) -> Result<()> {
        let total_files = self.project.files.len();
        if let Some((_current_orig, current_ui)) = self.current_orig_and_ui() {
            let has_section = |orig: usize| -> bool {
                self.project.files.get(orig).map(|f| {
                    f.markers.iter().any(|m| self.project.markers.get(m).map(|s| s.section).unwrap_or(false))
                }).unwrap_or(false)
            };
            if let Some(ui) = find_section_ui_index(total_files, current_ui, true, has_section) {
                if self.debug {
                    debug!("Action::NextSect: current_ui={}, moving to section ui_index={}", current_ui, ui);
                }
                self.goto_to_index(Some(ui), true)?;
            }
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
        let current_ui = if let Some(ref ui_state) = self.ui_state {
            if let Ok(guard) = ui_state.lock() {
                guard.current_file_index
            } else {
                return Ok(());
            }
        } else {
            return Ok(());
        };
        let is_recording = matches!(&self.state, AppState::Recording { .. });
        let total_ui = total_files + if is_recording { 1 } else { 0 };

        let ui_range: Box<dyn Iterator<Item = i32>> = if down {
            Box::new((current_ui + 1)..(total_ui as i32))
        } else {
            Box::new((0..current_ui).rev())
        };

        for ui in ui_range {
            if is_recording && ui == 0 {
                continue;
            }
            let orig = if is_recording {
                (total_files as i32 - ui) as usize
            } else {
                match ui_to_orig_index(ui, total_files) {
                    Some(o) => o,
                    None => continue,
                }
            };
            if let Some(file) = self.project.files.get(orig) {
                if file.hint.to_lowercase().contains(&query_lower) {
                    if self.debug {
                        debug!("SearchHint: found at ui={}, orig={}", ui, orig);
                    }
                    return self.goto_to_index(Some(ui), false);
                }
            }
        }
        Ok(())
    }
}
