use crate::app::app::{App, AppState, RecordingMode};
use crate::project::dialog_edit::{
    persist_staged, save_chunk_dialog, save_marker_dialog, save_meta_dialog, DialogSaveOutcome,
    FieldError,
};
use crate::project::markers::compute_effective_durations_ms;
use crate::project::project::Project;
use crate::utils::indexes::{find_section_ui_index, ui_to_orig_index};
use crate::utils::keyboard::{Action, DialogEdit};
use crate::utils::stats::get_wav_spec;
use anyhow::Result;
use log::{debug, info, warn};
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
            }
            Action::Ok => {
                if matches!(&self.state, AppState::Recording { .. }) {
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
            Action::Stop => {
                // Останавливаем запись, если она идёт
                if matches!(&self.state, AppState::Recording { .. }) {
                    self.stop_recording()?;
                }
                // Останавливаем воспроизведение, если оно идёт
                if matches!(&self.state, AppState::Playing { .. }) {
                    self.stop_playback()?;
                }
            }
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
            }
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
            }
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
            Action::SaveDialog { request_id, edit } => {
                self.handle_save_dialog(request_id, *edit);
            }
            Action::AddMarker { marker } => {
                // marker здесь - это shortcut (цифра 0-9)
                // Ищем маркер с таким shortcut
                let marker_name = self
                    .project
                    .markers
                    .iter()
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
                                info!(
                                    "Marker '{}' removed from file at index {}",
                                    marker_name, current_idx
                                );
                                // Обновляем UI, чтобы отобразить удаление маркера
                                self.update_ui_after_change();
                            } else {
                                file.markers.push(marker_name.clone());
                                crate::project::markers::normalize_markers(file);
                                // Если маркер имеет section=true, устанавливаем title
                                if let Some(marker_settings) =
                                    self.project.markers.get(&marker_name)
                                {
                                    if marker_settings.section && file.title.is_empty() {
                                        let file_number = current_idx + 1;
                                        file.title = format!("Title for {}", file_number);
                                    }
                                }
                                self.project.save(&self.project_path)?;
                                info!(
                                    "Marker '{}' added to file at index {}",
                                    marker_name, current_idx
                                );
                                // Обновляем UI, чтобы отобразить новый маркер
                                self.update_ui_after_change();
                            }
                        }
                    }
                } else {
                    warn!("No marker found with shortcut '{}'", marker);
                }
            }
            Action::AddMarkers {
                file_index,
                markers,
            } => {
                if let Some(orig_idx) =
                    Project::file_index_1based_to_orig(file_index, self.project.files.len())
                {
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
                    self.update_ui_after_change();
                }
            }
            Action::RemoveMarkers {
                file_index,
                markers,
            } => {
                if let Some(orig_idx) =
                    Project::file_index_1based_to_orig(file_index, self.project.files.len())
                {
                    let file = &mut self.project.files[orig_idx];
                    file.markers.retain(|m| !markers.contains(m));
                    crate::project::markers::normalize_markers(file);
                    self.project.save(&self.project_path)?;
                    info!("Markers removed from file at index {}", orig_idx);
                    self.update_ui_after_change();
                }
            }
            Action::SetMarkers {
                file_index,
                markers,
            } => {
                if let Some(orig_idx) =
                    Project::file_index_1based_to_orig(file_index, self.project.files.len())
                {
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
                    self.update_ui_after_change();
                }
            }
            Action::OpenMarkerSettings => {
                let ui_state = self
                    .ui_state
                    .clone()
                    .or_else(|| self.ui.as_ref().map(|ui| ui.get_state()));
                if let Some(ui_state) = ui_state {
                    if let Ok(mut state) = ui_state.lock() {
                        state.dialog_marker_snapshot = self.project.markers.clone();
                        state.dialog_markers_open = true;
                    }
                }
                // The UI controller owns selection and drafts derived from this snapshot.
            }
            Action::Compile => self.handle_compile()?,
            Action::CompileCancel => {
                if self
                    .compile_worker
                    .as_ref()
                    .is_none_or(|worker| worker.is_finished())
                {
                    return Ok(());
                }
                if let Some(state) = &self.ui_state {
                    if let Ok(mut state) = state.lock() {
                        if state.compile_finished_at.is_some() || state.compile_publishing {
                            return Ok(());
                        }
                        state.compile_cancel_requested = true;
                        state.compile_stage = "Cancelling…".into();
                        state.dialog_compile_eta_secs = -1.0;
                        if let Some(cancel) = &self.compile_cancel {
                            cancel.store(true, std::sync::atomic::Ordering::Relaxed);
                        }
                    }
                } else if let Some(cancel) = &self.compile_cancel {
                    cancel.store(true, std::sync::atomic::Ordering::Relaxed);
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
                    warn!(
                        "Invalid file index: {} (total files: {})",
                        file_index, total_files
                    );
                }
            }
            Action::OpenShortcutsDialog => self.handle_open_shortcuts_dialog()?,
            Action::OpenDeleteChunkDialog => self.handle_open_delete_chunk_dialog()?,
            Action::ConfirmDeleteChunk { ui_index } => {
                self.handle_confirm_delete_chunk(ui_index)?
            }
            Action::None => {}
        }
        Ok(())
    }

    fn handle_save_dialog(&mut self, request_id: u64, edit: DialogEdit) {
        let affects_file_data = !matches!(&edit, DialogEdit::Meta(_));
        let saved = match edit {
            DialogEdit::Meta(data) => {
                save_meta_dialog(&mut self.project, &self.project_path, &data)
            }
            DialogEdit::Chunk {
                data,
                expected_path,
            } => save_chunk_dialog(&mut self.project, &self.project_path, &data, &expected_path),
            DialogEdit::Marker(data) => {
                save_marker_dialog(&mut self.project, &self.project_path, &[data])
            }
            DialogEdit::RefreshMetadata => self.handle_update_files_meta(),
        };
        let (result, field) = match saved {
            Ok(outcome) => {
                if affects_file_data {
                    self.update_ui_after_change();
                }
                (Ok(outcome.warning), None)
            }
            Err(error) => {
                let field = error
                    .downcast_ref::<FieldError>()
                    .map(|error| error.field.to_owned());
                (Err(format!("{error:#}")), field)
            }
        };
        let ui_state = self
            .ui_state
            .clone()
            .or_else(|| self.ui.as_ref().map(|ui| ui.get_state()));
        if let Some(ui_state) = ui_state {
            if let Ok(mut state) = ui_state.lock() {
                if result.is_ok() {
                    // The command-loop App owns no Slint UI. Publish committed
                    // metadata and its acknowledgement atomically through shared state.
                    state.dialog_project_title = self.project.meta.title.clone();
                    state.dialog_project_author = self.project.meta.author.clone();
                    state.dialog_project_year = self.project.meta.year.clone();
                    state.dialog_project_hint = self.project.meta.hint.clone();
                    state.dialog_project_reader = self.project.meta.reader.clone();
                    state.dialog_project_format_audio = self.project.settings.format_audio.clone();
                    state.dialog_project_normalize = self.project.settings.normalize;
                    state.dialog_project_cover = self.project.settings.cover.clone();
                    state.dialog_project_section_split = self.project.settings.section_split;
                    state.dialog_project_denoise = self.project.settings.denoise;
                }
                state.dialog_save_result = Some(crate::ui::dialogs::DialogReply {
                    request_id,
                    result,
                    field,
                });
            }
        }
        // Persistence/validation failures belong to the pending dialog request,
        // not to handle_action's global error banner.
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
        if !matches!(&self.state, AppState::Recording { .. }) {
            // An explicit disk refresh must also retry/reload the same selected WAV.
            self.clear_waveform_requests();
            self.update_current_waveform();
            self.update_prev_waveform();
        }
        Ok(outcome)
    }

    fn handle_compile(&mut self) -> Result<()> {
        if self
            .compile_worker
            .as_ref()
            .is_some_and(|worker| !worker.is_finished())
        {
            return Ok(());
        }
        if let Some(worker) = self.compile_worker.take() {
            if worker.join().is_err() {
                self.report_error(&anyhow::anyhow!("Previous compilation worker panicked"));
            }
        }
        let project_files = self.project.files.clone();
        let project_path = self.project_path.clone();
        let markers = self.project.markers.clone();
        let meta = self.project.meta.clone();
        let settings = self.project.settings.clone();
        let ui_state = self.ui_state.clone();
        let debug = self.debug;
        let cancel = Arc::new(std::sync::atomic::AtomicBool::new(false));
        self.compile_cancel = Some(cancel.clone());
        crate::project::compiler::begin_compile_ui(ui_state.as_ref(), Some(&cancel));
        let spawned = std::thread::Builder::new().name("export-worker".into()).spawn(move || {
            let state_for_panic = ui_state.clone();
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                crate::project::compiler::compile_wav_files_static(
                    project_files,
                    project_path,
                    markers,
                    meta,
                    settings,
                    ui_state,
                    Some(cancel),
                    debug,
                )
            }));
            match result {
                Ok(Ok(())) => {}
                Ok(Err(error)) => log::error!("Compilation ended: {error:#}"),
                Err(payload) => {
                    let reason = payload
                        .downcast_ref::<String>()
                        .map(String::as_str)
                        .or_else(|| payload.downcast_ref::<&str>().copied())
                        .unwrap_or("unknown panic");
                    let message = format!("Export worker stopped unexpectedly: {reason}. Check the export directory before retrying.");
                    log::error!("{message}");
                    if let Some(state) = state_for_panic {
                        if let Ok(mut state) = state.lock() {
                            state.compile_stage =
                                "Error: export worker stopped unexpectedly".into();
                            state.error_message = message;
                            state.compile_publishing = false;
                            state.compile_finished_at = Some(Instant::now());
                            state.dialog_compile_eta_secs = -1.0;
                        }
                    }
                }
            }
        });
        match spawned {
            Ok(worker) => self.compile_worker = Some(worker),
            Err(error) => {
                if let Some(state) = &self.ui_state {
                    if let Ok(mut state) = state.lock() {
                        state.compile_stage = format!("Error: cannot start export worker: {error}");
                        state.compile_finished_at = Some(Instant::now());
                        state.dialog_compile_eta_secs = -1.0;
                    }
                }
                return Err(anyhow::Error::new(error).context("Cannot start export worker"));
            }
        }
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
        let mut markers_with_shortcut: Vec<_> = self
            .project
            .markers
            .iter()
            .filter_map(|(alias, marker_settings)| {
                marker_settings
                    .shortcut
                    .as_ref()
                    .map(|shortcut_str| (shortcut_str.clone(), alias.clone()))
            })
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
            info!(
                "OpenDeleteChunkDialog: skipped (current_orig_and_ui=None, current_index={:?})",
                self.current_index
            );
            return Ok(());
        };
        let Some(file) = self.project.files.get(current_orig) else {
            info!(
                "OpenDeleteChunkDialog: skipped (no file at orig={})",
                current_orig
            );
            return Ok(());
        };
        let num = total_files - (current_ui as usize);
        let title = if file.title.is_empty() {
            "—"
        } else {
            file.title.as_str()
        };
        let text = format!("Запись №{}: {}, файл: {}", num, title, file.path);
        if let Some(ref state) = self.ui_state {
            if let Ok(mut guard) = state.lock() {
                guard.dialog_delete_text = text.clone();
                guard.dialog_delete_file_index = current_ui;
                guard.dialog_delete_open = true;
                info!(
                    "OpenDeleteChunkDialog: opened for ui_index={}, text={}",
                    current_ui, text
                );
            }
        }
        Ok(())
    }

    fn handle_confirm_delete_chunk(&mut self, ui_index: i32) -> Result<()> {
        if matches!(&self.state, AppState::Recording { .. }) {
            return Ok(());
        }
        let total_files = self.project.files.len();
        if let Some(orig_index) = ui_to_orig_index(ui_index, total_files) {
            let mut next_project = self.project.clone();
            if let Some(path) = next_project.remove_file_at(orig_index) {
                next_project.save(&self.project_path)?;
                self.project = next_project;
                let full_path = self.resolve_file_path(&path);
                if full_path.exists() {
                    let _ = std::fs::remove_file(&full_path);
                }
                let _ = crate::audio::waveform::remove_waveform_cache(&full_path);
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
                self.update_current_waveform();
                self.update_prev_waveform();
                self.update_ui_after_change();
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
            self.ui_update_cache.section_indices()
        };
        let target = if let Some(indices) = cached {
            crate::ui::updater::adjacent_section_index(indices, current_orig, next)
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
