use crate::project::project::{KeyBindings, ProjectFile};
use crate::utils::format::{
    current_and_prev_file_hints, format_duration, format_markers_with_ordinals_batch, format_size,
    reverse_and_reindex_file_list,
};
use crate::utils::indexes::orig_to_ui_index;
use crate::utils::keyboard::Action;
use anyhow::Result;
use crossbeam_channel::{Receiver, Sender};
use slint::Model;
use std::cell::{Cell, RefCell};
use std::collections::HashSet;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

slint::include_modules!();

pub struct UIState {
    // — волновой график, уровень, список файлов и статусная строка
    pub waveform_prev: Vec<f32>,
    pub waveform_current: Vec<f32>,
    /// Версия результатов фоновой подготовки, независимая от списка и выбранной строки.
    pub waveform_version: u64,
    pub waveform_current_loading: bool,
    pub waveform_prev_loading: bool,
    pub level: f32,
    pub file_list: Vec<FileInfo>,
    pub current_file_index: i32,
    pub current_file_name: String,
    pub total_files: String,
    pub total_duration: String,
    pub total_size: String,
    pub is_recording: bool,
    pub playback_position: f32, // Позиция воспроизведения (0.0 - 1.0) для текущего файла
    // — метаданные проекта и настройки (диалог project)
    pub dialog_project_title: String,
    pub dialog_project_author: String,
    pub dialog_project_year: String,
    pub dialog_project_hint: String,
    pub dialog_project_reader: String,
    pub dialog_project_format_audio: String,
    pub dialog_project_normalize: bool,
    pub dialog_project_cover: String,
    pub dialog_project_section_split: bool,
    pub dialog_project_denoise: bool,
    // — подсказки текущего/предыдущего файла и секции
    pub hintbox_current_start: String,
    pub hintbox_current_end: String,
    pub hintbox_prev: String,
    pub hintbox_prev_start: String,
    pub hintbox_prev_end: String,
    pub section_titles: String,
    pub dialog_toc_list_text: String,
    pub dialog_save_result: Option<super::dialogs::DialogReply>,
    pub dialog_marker_snapshot: std::collections::HashMap<String, crate::project::MarkerSettings>,
    // — диалог маркеров
    pub dialog_markers_open: bool,
    // — прогресс компиляции
    pub compile_progress: f32,
    pub compile_stage: String,
    /// Текст всех стадий компиляции (многострочный)
    pub dialog_compile_stages_text: String,
    /// Оставшееся время в секундах; < 0 — неизвестно
    pub dialog_compile_eta_secs: f32,
    /// Overlay остаётся открытым и после завершения, пока пользователь не нажмёт Close.
    pub is_compiling: bool,
    pub compile_cancel_requested: bool,
    pub compile_publishing: bool,
    /// Наличие времени означает завершение операции; автоматического закрытия нет.
    pub compile_finished_at: Option<std::time::Instant>,
    /// Версия полного снимка структуры/данных; точечные статусы передаются через dirty rows.
    pub file_list_version: u64,
    /// Индексы изменённых строк без структурного обновления; GUI забирает очередь при тике.
    pub file_list_dirty_rows: HashSet<usize>,
    // — диалоги: горячие клавиши, удаление отрывка
    pub dialog_shortcuts_open: bool,
    pub dialog_shortcuts_list_text: String,
    pub recording_mode: String,
    pub dialog_delete_open: bool,
    pub dialog_delete_text: String,
    pub dialog_delete_file_index: i32,
    /// Последняя ошибка операции; остаётся видимой до явного закрытия пользователем.
    pub error_message: String,
}

impl Default for UIState {
    fn default() -> Self {
        Self {
            waveform_prev: Vec::new(),
            waveform_current: Vec::new(),
            waveform_version: 0,
            waveform_current_loading: false,
            waveform_prev_loading: false,
            level: 0.0,
            file_list: Vec::new(),
            current_file_index: -1,
            current_file_name: String::new(),
            total_files: String::new(),
            total_duration: String::new(),
            total_size: String::new(),
            is_recording: false,
            playback_position: 0.0,
            dialog_project_title: String::new(),
            dialog_project_author: String::new(),
            dialog_project_year: String::new(),
            dialog_project_hint: String::new(),
            dialog_project_reader: String::new(),
            dialog_project_format_audio: "wav".to_string(),
            dialog_project_normalize: true,
            dialog_project_cover: "cover.png".to_string(),
            dialog_project_section_split: false,
            dialog_project_denoise: false,
            hintbox_current_start: String::new(),
            hintbox_current_end: String::new(),
            hintbox_prev: String::new(),
            hintbox_prev_start: String::new(),
            hintbox_prev_end: String::new(),
            section_titles: String::new(),
            dialog_toc_list_text: String::new(),
            dialog_save_result: None,
            dialog_marker_snapshot: Default::default(),
            dialog_markers_open: false,
            compile_progress: 0.0,
            compile_stage: String::new(),
            dialog_compile_stages_text: String::new(),
            dialog_compile_eta_secs: -1.0,
            is_compiling: false,
            compile_cancel_requested: false,
            compile_publishing: false,
            compile_finished_at: None,
            file_list_version: 0,
            file_list_dirty_rows: HashSet::new(),
            dialog_shortcuts_open: false,
            dialog_shortcuts_list_text: String::new(),
            recording_mode: "A".to_string(),
            dialog_delete_open: false,
            dialog_delete_text: String::new(),
            dialog_delete_file_index: -1,
            error_message: String::new(),
        }
    }
}

impl UIState {
    /// Coalesces row updates until the next GUI tick without invalidating the full snapshot.
    pub fn mark_file_list_row_changed(&mut self, index: usize) {
        if index < self.file_list.len() {
            self.file_list_dirty_rows.insert(index);
        }
    }
}

pub struct UI {
    _has_ui: bool,
    state: Arc<Mutex<UIState>>,
    window: Option<MainWindow>,
    window_weak: Option<slint::Weak<MainWindow>>,
    _action_tx: Option<Sender<Action>>,
    _should_close: Arc<std::sync::atomic::AtomicBool>,
    _file_list_model: Option<std::rc::Rc<slint::VecModel<FileInfo>>>,
    _waveform_prev_model: Option<std::rc::Rc<slint::VecModel<f32>>>,
    _waveform_current_model: Option<std::rc::Rc<slint::VecModel<f32>>>,
    _timer: Option<slint::Timer>,
}

fn apply_file_list_to_model(model: &slint::VecModel<FileInfo>, list: &[FileInfo]) {
    let current_count = model.row_count();
    let new_len = list.len();
    // В UI список перевёрнут: новая запись меняет и существующие строки,
    // поэтому даже при росте модели нужно сначала синхронизировать общий префикс.
    for (index, new_row) in list.iter().take(current_count).enumerate() {
        if model.row_data(index).as_ref() != Some(new_row) {
            model.set_row_data(index, new_row.clone());
        }
    }
    for item in list.iter().skip(current_count) {
        model.push(item.clone());
    }
    for _ in new_len..current_count {
        model.remove(model.row_count() - 1);
    }
}

fn allows_project_actions(window_weak: &slint::Weak<MainWindow>) -> bool {
    window_weak
        .upgrade()
        .map(|window| !window.get_modal_open())
        .unwrap_or(false)
}

impl UI {
    /// pending_index_rx: ui_index для немедленного обновления в окне; прокрутка только если строка не видна.
    pub fn new(
        action_tx: Sender<Action>,
        key_bindings: KeyBindings,
        pending_index_rx: Option<Receiver<i32>>,
    ) -> Result<Self> {
        let window = MainWindow::new()?;
        window.set_window_title(
            format!(
                "iamreader {} {}",
                env!("CARGO_PKG_VERSION"),
                env!("BUILD_DATE")
            )
            .into(),
        );
        let window_weak = window.as_weak();

        let state = Arc::new(Mutex::new(UIState::default()));
        let dialogs =
            super::dialogs::DialogController::install(&window, state.clone(), action_tx.clone());

        let state_clone = state.clone();
        let action_tx_clone = action_tx.clone();
        let bindings_clone = key_bindings.clone();
        let should_close = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let should_close_clone = should_close.clone();

        let action_tx_for_keys = action_tx_clone.clone();
        let bindings_for_keys = bindings_clone.clone();
        let window_weak_for_keys = window_weak.clone();

        window.on_key_pressed(move |key: slint::SharedString| {
            let key_str = key.as_str();
            let key_bytes = key_str.as_bytes();
            // Логируем все нажатия клавиш для отладки, особенно функциональные
            if key_str.to_lowercase().starts_with("f") && key_str.len() <= 3 {
                log::debug!(
                    "[UI] Functional key pressed: {:?} (len={}, bytes={:?})",
                    key_str,
                    key_str.len(),
                    key_bytes
                );
            } else if key_bytes.len() == 3 {
                log::debug!(
                    "[UI] Key pressed: {:?} (len={}, bytes={:?}) - might be arrow/function key",
                    key_str,
                    key_str.len(),
                    key_bytes
                );
            } else {
                log::debug!(
                    "[UI] Key pressed: {:?} (len={}, bytes={:?})",
                    key_str,
                    key_str.len(),
                    key_bytes
                );
            }
            // Delete проверяем по точному совпадению до нормализации, иначе "Delete" -> 'd' совпадает с Stop
            let is_delete_key = key_str == bindings_for_keys.delete.as_str()
                || (key_str == "\u{7f}" && bindings_for_keys.delete == "Delete");
            let key_for_normalize = if key_str == "\u{7f}" {
                "Delete"
            } else {
                key_str
            };
            let normalized_key = normalize_key_for_layout(key_for_normalize);

            // Тот же guard, что и в Slint, включая справку и компиляцию.
            if !allows_project_actions(&window_weak_for_keys) {
                return;
            }

            let action = if is_delete_key {
                Action::OpenDeleteChunkDialog
            } else if normalized_key == normalize_key_for_layout(&bindings_for_keys.record) {
                Action::Record {
                    duration_secs: None,
                }
            } else if normalized_key == normalize_key_for_layout(&bindings_for_keys.ok) {
                Action::Ok
            } else if normalized_key == normalize_key_for_layout(&bindings_for_keys.stop) {
                Action::Stop
            } else if normalized_key == normalize_key_for_layout(&bindings_for_keys.prev) {
                Action::Prev
            } else if normalized_key == normalize_key_for_layout(&bindings_for_keys.next) {
                Action::Next
            } else if normalized_key == normalize_key_for_layout(&bindings_for_keys.chapter_prev) {
                Action::PrevSect
            } else if normalized_key == normalize_key_for_layout(&bindings_for_keys.chapter_next) {
                Action::NextSect
            } else if normalized_key == normalize_key_for_layout(&bindings_for_keys.play) {
                Action::Play
            } else if normalized_key == normalize_key_for_layout(&bindings_for_keys.mode_update) {
                Action::ModeUpdate
            } else if normalized_key == normalize_key_for_layout(&bindings_for_keys.mode_insert) {
                Action::ModeInsert
            } else {
                Action::None
            };
            if !matches!(action, Action::None) {
                let _ = action_tx_for_keys.try_send(action);
            }
        });

        let should_close_for_callback = should_close_clone.clone();
        window.on_window_closed(move || {
            should_close_for_callback.store(true, std::sync::atomic::Ordering::SeqCst);
        });

        // Обработчик одинарного клика по строке файла (только выделение)
        let action_tx_for_file_click = action_tx.clone();
        let window_weak_for_file_click = window_weak.clone();
        window.on_file_clicked(move |ui_index: i32| {
            if !allows_project_actions(&window_weak_for_file_click) {
                return;
            }
            let _ = action_tx_for_file_click.try_send(Action::Goto {
                index: Some(ui_index),
                play: false,
            });
        });

        let action_tx_for_search_up = action_tx.clone();
        let window_weak_for_search_up = window_weak.clone();
        window.on_search_hint_up(move |query: slint::SharedString| {
            if !allows_project_actions(&window_weak_for_search_up) {
                return;
            }
            let _ = action_tx_for_search_up.try_send(Action::SearchHintUp(query.to_string()));
        });
        let action_tx_for_search_down = action_tx.clone();
        let window_weak_for_search_down = window_weak.clone();
        window.on_search_hint_down(move |query: slint::SharedString| {
            if !allows_project_actions(&window_weak_for_search_down) {
                return;
            }
            let _ = action_tx_for_search_down.try_send(Action::SearchHintDown(query.to_string()));
        });

        // Обработчик двойного клика по строке файла (открывает диалог настроек отрывка)
        let state_for_file_double_click = state.clone();
        let window_weak_for_file_double_click = window_weak.clone();
        let dialogs_for_chunk = dialogs.clone();
        window.on_file_double_clicked(move |ui_index: i32| {
            log::debug!("[UI] File double-clicked, ui_index={}", ui_index);
            if let Some(window) = window_weak_for_file_double_click.upgrade() {
                // Проверяем, не открыт ли уже диалог
                if window.get_modal_open() {
                    log::debug!("[UI] File double-clicked but dialog is already open, ignoring");
                    return;
                }
                if let Ok(state_guard) = state_for_file_double_click.lock() {
                    let file_list_len = state_guard.file_list.len();
                    log::debug!("[UI] File double-clicked, ui_index={}, file_list_len={}",
                        ui_index, file_list_len);
                    if ui_index >= 0 && (ui_index as usize) < file_list_len {
                        let file = &state_guard.file_list[ui_index as usize];
                        dialogs_for_chunk.borrow_mut().open_chunk(&window, file.path.to_string());
                        window.set_dialog_chunk_file_index(ui_index);
                        window.set_dialog_chunk_title(file.title.clone());
                        window.set_dialog_chunk_author(file.author.clone());
                        window.set_dialog_chunk_year(file.year.clone());
                        window.set_dialog_chunk_hint(file.hint.clone());
                        window.set_dialog_chunk_open(true);
                        log::debug!("[UI] Chunk settings dialog opened via double-click for ui_index={}, title={}, hint={}",
                            ui_index, file.title, file.hint);
                    } else {
                        log::warn!("[UI] File double-clicked but ui_index is invalid: ui_index={}, file_list_len={}",
                            ui_index, file_list_len);
                    }
                } else {
                    log::warn!("[UI] File double-clicked but failed to lock state");
                }
            } else {
                log::warn!("[UI] File double-clicked but window is not available");
            }
        });

        // Обработчик добавления маркера (Ctrl+1-9)
        let action_tx_for_add_marker = action_tx.clone();
        let window_weak_for_add_marker = window_weak.clone();
        window.on_add_marker(move |marker: slint::SharedString| {
            if !allows_project_actions(&window_weak_for_add_marker) {
                return;
            }
            let marker_str = marker.as_str().to_string();
            let _ = action_tx_for_add_marker.try_send(Action::AddMarker { marker: marker_str });
        });

        // Обработчик добавления маркера с нормализацией (Ctrl+1-9)
        let action_tx_for_add_marker_norm = action_tx.clone();
        let window_weak_for_add_marker_norm = window_weak.clone();
        window.on_add_marker_with_normalization(move |marker: slint::SharedString| {
            if !allows_project_actions(&window_weak_for_add_marker_norm) {
                return;
            }
            let marker_str = marker.as_str().to_string();
            // Нормализуем маркер для работы независимо от раскладки
            let normalized_marker = normalize_key_for_layout(&marker_str);
            let _ = action_tx_for_add_marker_norm.try_send(Action::AddMarker {
                marker: normalized_marker,
            });
        });

        // Обработчик открытия диалога настроек маркеров (через кнопку Markers)
        let state_for_marker_open = state.clone();
        let action_tx_for_marker_open = action_tx.clone();
        let window_weak_for_marker_open = window_weak.clone();
        window.on_dialog_markers_request_open(move || {
            if !allows_project_actions(&window_weak_for_marker_open) {
                return;
            }
            if let Err(error) = action_tx_for_marker_open.try_send(Action::OpenMarkerSettings) {
                if let Ok(mut state) = state_for_marker_open.lock() {
                    state.error_message = format!("Cannot open marker settings: {error}");
                }
            }
        });

        // Обработчик открытия диалога метаданных проекта (через F5 или меню)
        let state_for_meta_open = state.clone();
        let window_weak_for_meta_open = window_weak.clone();
        window.on_dialog_project_request_open(move || {
            if let Some(window) = window_weak_for_meta_open.upgrade() {
                if !window.get_modal_open() {
                    if let Ok(state_guard) = state_for_meta_open.lock() {
                        window.set_dialog_project_title(
                            state_guard.dialog_project_title.clone().into(),
                        );
                        window.set_dialog_project_author(
                            state_guard.dialog_project_author.clone().into(),
                        );
                        window.set_dialog_project_year(
                            state_guard.dialog_project_year.clone().into(),
                        );
                        window.set_dialog_project_hint(
                            state_guard.dialog_project_hint.clone().into(),
                        );
                        window.set_dialog_project_reader(
                            state_guard.dialog_project_reader.clone().into(),
                        );
                        window.set_dialog_project_format_audio(
                            state_guard.dialog_project_format_audio.clone().into(),
                        );
                        window.set_dialog_project_normalize(state_guard.dialog_project_normalize);
                        window.set_dialog_project_cover(
                            state_guard.dialog_project_cover.clone().into(),
                        );
                        window.set_dialog_project_section_split(
                            state_guard.dialog_project_section_split,
                        );
                        window.set_dialog_project_denoise(state_guard.dialog_project_denoise);
                        window.set_dialog_error_message("".into());
                        window.set_dialog_project_open(true);
                        log::debug!(
                            "[UI] Meta dialog opened via callback, format: {}",
                            state_guard.dialog_project_format_audio
                        );
                    }
                }
            }
        });

        // Обработчик открытия диалога TOC (через F10 или меню)
        let state_for_toc_open = state.clone();
        let window_weak_for_toc_open = window_weak.clone();
        window.on_dialog_toc_request_open(move || {
            if let Some(window) = window_weak_for_toc_open.upgrade() {
                if window.get_modal_open() {
                    return;
                }
                if let Ok(state_guard) = state_for_toc_open.lock() {
                    window
                        .set_dialog_toc_list_text(state_guard.dialog_toc_list_text.clone().into());
                    window.set_dialog_toc_open(true);
                }
            }
        });
        let window_weak_for_toc_cancel = window_weak.clone();
        window.on_dialog_toc_cancel(move || {
            if let Some(window) = window_weak_for_toc_cancel.upgrade() {
                window.set_dialog_toc_open(false);
            }
        });

        // Обработчик компиляции (через F8 или меню)
        let action_tx_for_compile = action_tx.clone();
        let window_weak_for_compile = window_weak.clone();
        window.on_compile(move || {
            if !allows_project_actions(&window_weak_for_compile) {
                return;
            }
            log::info!("[UI] Compile action triggered via callback");
            let _ = action_tx_for_compile.try_send(Action::Compile);
        });
        let action_tx_for_compile_cancel = action_tx.clone();
        let state_for_compile_cancel = state.clone();
        let window_weak_for_compile_cancel = window_weak.clone();
        window.on_dialog_compile_cancel(move || {
            if let Ok(mut state_guard) = state_for_compile_cancel.lock() {
                if !state_guard.is_compiling
                    || state_guard.compile_finished_at.is_some()
                    || state_guard.compile_cancel_requested
                    || state_guard.compile_publishing
                {
                    return;
                }
                if let Err(error) = action_tx_for_compile_cancel.try_send(Action::CompileCancel) {
                    state_guard.error_message =
                        format!("Cannot request export cancellation: {error}");
                    return;
                }
                state_guard.compile_cancel_requested = true;
                if let Some(window) = window_weak_for_compile_cancel.upgrade() {
                    window.set_dialog_compile_cancel_requested(true);
                    window.set_dialog_compile_cancel_enabled(false);
                }
            }
        });
        let state_for_compile_close = state.clone();
        let window_weak_for_compile_close = window_weak.clone();
        window.on_dialog_compile_close(move || {
            if let Ok(mut state_guard) = state_for_compile_close.lock() {
                if !state_guard.is_compiling || state_guard.compile_finished_at.is_none() {
                    return;
                }
                state_guard.is_compiling = false;
                state_guard.compile_finished_at = None;
                state_guard.compile_cancel_requested = false;
                state_guard.compile_publishing = false;
                state_guard.compile_progress = 0.0;
                state_guard.compile_stage.clear();
                state_guard.dialog_compile_stages_text.clear();
                state_guard.dialog_compile_eta_secs = -1.0;
                if let Some(window) = window_weak_for_compile_close.upgrade() {
                    window.set_is_compiling(false);
                    window.set_dialog_compile_finished(false);
                    window.set_dialog_compile_cancel_enabled(false);
                }
            }
        });

        // Обработчик F1 — открыть справку по клавишам
        let action_tx_for_shortcuts = action_tx.clone();
        let window_weak_for_shortcuts = window_weak.clone();
        window.on_dialog_shortcuts_request_open(move || {
            if !allows_project_actions(&window_weak_for_shortcuts) {
                return;
            }
            let _ = action_tx_for_shortcuts.try_send(Action::OpenShortcutsDialog);
        });

        // Обработчик закрытия диалога справки по клавишам
        let state_for_shortcuts_cancel = state.clone();
        window.on_dialog_shortcuts_cancel(move || {
            if let Ok(mut state_guard) = state_for_shortcuts_cancel.lock() {
                state_guard.dialog_shortcuts_open = false;
            }
        });

        let action_tx_for_delete_confirm = action_tx.clone();
        window.on_dialog_delete_confirm(move |ui_index: i32| {
            let _ = action_tx_for_delete_confirm.try_send(Action::ConfirmDeleteChunk { ui_index });
        });

        let state_for_delete_cancel = state.clone();
        window.on_dialog_delete_cancel(move || {
            if let Ok(mut state_guard) = state_for_delete_cancel.lock() {
                state_guard.dialog_delete_open = false;
            }
        });

        let state_for_dismiss_error = state.clone();
        window.on_dismiss_error(move || {
            if let Ok(mut state_guard) = state_for_dismiss_error.lock() {
                state_guard.error_message.clear();
            }
        });

        // Создаем модель для списка файлов и сохраняем Rc для обновления
        let file_list_vec_model = std::rc::Rc::new(slint::VecModel::from(Vec::<FileInfo>::new()));
        let file_list_model_rc = slint::ModelRc::from(file_list_vec_model.clone());
        window.set_file_list(file_list_model_rc);

        let waveform_prev_model = std::rc::Rc::new(slint::VecModel::from(Vec::<f32>::new()));
        let waveform_current_model = std::rc::Rc::new(slint::VecModel::from(Vec::<f32>::new()));
        window.set_waveform_prev(slint::ModelRc::from(waveform_prev_model.clone()));
        window.set_waveform_current(slint::ModelRc::from(waveform_current_model.clone()));

        let timer = slint::Timer::default();
        log::info!("[UI] Timer created, starting...");
        timer.start(slint::TimerMode::Repeated, Duration::from_millis(40), {
            let state = state_clone.clone();
            let window_weak = window_weak.clone();
            let mut pending_index_rx = pending_index_rx;
            let file_list_model = file_list_vec_model.clone();
            let waveform_prev_model = waveform_prev_model.clone();
            let waveform_current_model = waveform_current_model.clone();
            let last_len = Cell::new(0usize);
            let last_version = Cell::new(0u64);
            let last_waveform_prev_len = Cell::new(0usize);
            let last_waveform_current_len = Cell::new(0usize);
            let last_waveform_version = Cell::new(0u64);
            // Индекс записи, при котором последний раз применили waveform из state (отдельно от канала).
            let last_index_for_waveform = Cell::new(-2i32);
            let waveform_tick = Cell::new(0u32);
            let last_level = Cell::new(-1.0f32);
            let last_playback_position = Cell::new(-1.0f32);
            let last_current_file_index = Cell::new(-2i32);
            let last_total_files = RefCell::new(String::new());
            let last_total_duration = RefCell::new(String::new());
            let last_total_size = RefCell::new(String::new());
            let last_hintbox_current_start = RefCell::new(String::new());
            let last_hintbox_current_end = RefCell::new(String::new());
            let last_hintbox_prev_start = RefCell::new(String::new());
            let last_hintbox_prev_end = RefCell::new(String::new());
            let last_section_titles = RefCell::new(String::new());
            let dialogs = dialogs.clone();
            move || {
                if let Some(window) = window_weak.upgrade() {
                    if let Some(ref mut rx) = pending_index_rx {
                        if let Ok(idx) = rx.try_recv() {
                            window.set_current_file_index(idx);
                            last_current_file_index.set(idx);
                            let total = state
                                .lock()
                                .map(|state_guard| state_guard.file_list.len())
                                .unwrap_or(0);
                            if idx >= 0 && total > 0 {
                                let current_y = window.get_file_list_scroll_viewport_y() as f32;
                                let visible_height = window.get_file_list_visible_height();
                                let row_top = (idx as f32) * UI::ROW_HEIGHT;
                                let row_bottom = (idx as f32 + 1.0) * UI::ROW_HEIGHT;
                                let visible_top = -current_y;
                                let visible_bottom = -current_y + visible_height;
                                if visible_height > 0.0
                                    && !(row_top >= visible_top && row_bottom <= visible_bottom)
                                {
                                    window.set_file_list_scroll_viewport_y(UI::scroll_y_for_index(
                                        idx,
                                        total,
                                        visible_height,
                                    ));
                                }
                            }
                        }
                    }
                    let (file_list_copy, need_list_update, current_len, current_version);
                    let recording_row;
                    let row_patches;
                    let (waveform_version, waveform_current_loading, waveform_prev_loading);
                    let (
                        waveform_prev_copy,
                        waveform_current_copy,
                        need_waveform_prev,
                        need_waveform_current,
                    );
                    let (
                        level,
                        playback_position,
                        current_idx,
                        total_files,
                        total_duration,
                        total_size,
                        hintbox_current_start,
                        hintbox_current_end,
                        hintbox_prev_start,
                        hintbox_prev_end,
                        section_titles,
                        is_rec,
                        is_compiling,
                        compile_progress,
                        compile_stage,
                        compile_stages_text,
                        compile_eta_secs,
                        compile_cancel_enabled,
                        compile_finished,
                        compile_cancel_requested,
                        compile_publishing,
                    );
                    let (shortcuts_dialog_open, shortcuts_list_text, recording_mode);
                    let (dialog_delete_open, dialog_delete_text, dialog_delete_file_index);
                    let error_message;
                    {
                        let mut state_guard = match state.lock() {
                            Ok(guard) => guard,
                            Err(_) => return,
                        };
                        need_list_update = state_guard.file_list.len() != last_len.get()
                            || state_guard.file_list_version != last_version.get();
                        file_list_copy = if need_list_update {
                            Some(state_guard.file_list.clone())
                        } else {
                            None
                        };
                        row_patches = if need_list_update {
                            // Полный snapshot уже включает все накопленные точечные изменения.
                            state_guard.file_list_dirty_rows.clear();
                            Vec::new()
                        } else {
                            std::mem::take(&mut state_guard.file_list_dirty_rows)
                                .into_iter()
                                .filter_map(|index| {
                                    state_guard
                                        .file_list
                                        .get(index)
                                        .cloned()
                                        .map(|row| (index, row))
                                })
                                .collect::<Vec<_>>()
                        };
                        // Таймер записи меняет одну строку без версии всего списка.
                        recording_row = if !need_list_update && state_guard.is_recording {
                            usize::try_from(state_guard.current_file_index)
                                .ok()
                                .and_then(|index| {
                                    state_guard
                                        .file_list
                                        .get(index)
                                        .cloned()
                                        .map(|row| (index, row))
                                })
                        } else {
                            None
                        };
                        if need_list_update {
                            current_len = state_guard.file_list.len();
                            current_version = state_guard.file_list_version;
                        } else {
                            current_len = last_len.get();
                            current_version = last_version.get();
                        }
                        let index_for_waveform_changed =
                            state_guard.current_file_index != last_index_for_waveform.get();
                        waveform_version = state_guard.waveform_version;
                        waveform_current_loading = state_guard.waveform_current_loading;
                        waveform_prev_loading = state_guard.waveform_prev_loading;
                        let waveform_version_changed =
                            waveform_version != last_waveform_version.get();
                        need_waveform_prev = state_guard.waveform_prev.len()
                            != last_waveform_prev_len.get()
                            || waveform_version_changed
                            || need_list_update
                            || index_for_waveform_changed;
                        let tick = waveform_tick.get().wrapping_add(1);
                        waveform_tick.set(tick);
                        need_waveform_current = state_guard.waveform_current.len()
                            != last_waveform_current_len.get()
                            || waveform_version_changed
                            || need_list_update
                            || (state_guard.is_recording && tick % 3 == 0)
                            || index_for_waveform_changed;
                        waveform_prev_copy = if need_waveform_prev {
                            Some(state_guard.waveform_prev.clone())
                        } else {
                            None
                        };
                        waveform_current_copy = if need_waveform_current {
                            Some(state_guard.waveform_current.clone())
                        } else {
                            None
                        };
                        level = state_guard.level;
                        playback_position = state_guard.playback_position;
                        current_idx = state_guard.current_file_index;
                        total_files = state_guard.total_files.clone();
                        total_duration = state_guard.total_duration.clone();
                        total_size = state_guard.total_size.clone();
                        hintbox_current_start = state_guard.hintbox_current_start.clone();
                        hintbox_current_end = state_guard.hintbox_current_end.clone();
                        hintbox_prev_start = state_guard.hintbox_prev_start.clone();
                        hintbox_prev_end = state_guard.hintbox_prev_end.clone();
                        section_titles = state_guard.section_titles.clone();
                        is_rec = state_guard.is_recording;
                        is_compiling = state_guard.is_compiling;
                        compile_progress = state_guard.compile_progress;
                        compile_stage = state_guard.compile_stage.clone();
                        compile_stages_text = state_guard.dialog_compile_stages_text.clone();
                        compile_eta_secs = state_guard.dialog_compile_eta_secs;
                        compile_finished = state_guard.compile_finished_at.is_some();
                        compile_cancel_requested = state_guard.compile_cancel_requested;
                        compile_publishing = state_guard.compile_publishing;
                        compile_cancel_enabled = state_guard.is_compiling
                            && !compile_finished
                            && !compile_cancel_requested
                            && !compile_publishing;
                        shortcuts_dialog_open = state_guard.dialog_shortcuts_open;
                        shortcuts_list_text = state_guard.dialog_shortcuts_list_text.clone();
                        recording_mode = state_guard.recording_mode.clone();
                        dialog_delete_open = state_guard.dialog_delete_open;
                        dialog_delete_text = state_guard.dialog_delete_text.clone();
                        dialog_delete_file_index = state_guard.dialog_delete_file_index;
                        error_message = state_guard.error_message.clone();
                    }
                    if let Some(ref list) = file_list_copy {
                        apply_file_list_to_model(&file_list_model, list);
                        last_len.set(current_len);
                        last_version.set(current_version);
                    }
                    if let Some((index, row)) = recording_row {
                        if file_list_model.row_data(index).as_ref() != Some(&row) {
                            file_list_model.set_row_data(index, row);
                        }
                    }
                    for (index, row) in row_patches {
                        if file_list_model.row_data(index).as_ref() != Some(&row) {
                            file_list_model.set_row_data(index, row);
                        }
                    }
                    if let Some(ref waveform_data) = waveform_prev_copy {
                        while waveform_prev_model.row_count() > 0 {
                            waveform_prev_model.remove(waveform_prev_model.row_count() - 1);
                        }
                        for sample in waveform_data {
                            waveform_prev_model.push(*sample);
                        }
                        last_waveform_prev_len.set(waveform_data.len());
                        last_index_for_waveform.set(current_idx);
                    }
                    if let Some(ref waveform_data) = waveform_current_copy {
                        while waveform_current_model.row_count() > 0 {
                            waveform_current_model.remove(waveform_current_model.row_count() - 1);
                        }
                        for sample in waveform_data {
                            waveform_current_model.push(*sample);
                        }
                        last_waveform_current_len.set(waveform_data.len());
                        last_index_for_waveform.set(current_idx);
                    }
                    last_waveform_version.set(waveform_version);
                    window.set_waveform_current_loading(waveform_current_loading);
                    window.set_waveform_prev_loading(waveform_prev_loading);
                    if level != last_level.get() {
                        window.set_level(level);
                        last_level.set(level);
                    }
                    if playback_position != last_playback_position.get() {
                        window.set_playback_position(playback_position);
                        last_playback_position.set(playback_position);
                    }
                    const RECORDING_WAVEFORM_WINDOW: i32 = 1500;
                    window.set_waveform_current_window(if is_rec {
                        RECORDING_WAVEFORM_WINDOW
                    } else {
                        0
                    });
                    if current_idx != last_current_file_index.get() {
                        window.set_current_file_index(current_idx);
                        last_current_file_index.set(current_idx);
                    }
                    if *last_total_files.borrow() != total_files {
                        *last_total_files.borrow_mut() = total_files.clone();
                        window.set_total_files(total_files.into());
                    }
                    if *last_total_duration.borrow() != total_duration {
                        *last_total_duration.borrow_mut() = total_duration.clone();
                        window.set_total_duration(total_duration.into());
                    }
                    if *last_total_size.borrow() != total_size {
                        *last_total_size.borrow_mut() = total_size.clone();
                        window.set_total_size(total_size.into());
                    }
                    if *last_hintbox_current_start.borrow() != hintbox_current_start
                        || *last_hintbox_current_end.borrow() != hintbox_current_end
                    {
                        *last_hintbox_current_start.borrow_mut() = hintbox_current_start.clone();
                        *last_hintbox_current_end.borrow_mut() = hintbox_current_end.clone();
                        window.set_hintbox_current_start(hintbox_current_start.into());
                        window.set_hintbox_current_end(hintbox_current_end.into());
                    }
                    if *last_hintbox_prev_start.borrow() != hintbox_prev_start
                        || *last_hintbox_prev_end.borrow() != hintbox_prev_end
                    {
                        *last_hintbox_prev_start.borrow_mut() = hintbox_prev_start.clone();
                        *last_hintbox_prev_end.borrow_mut() = hintbox_prev_end.clone();
                        window.set_hintbox_prev_start(hintbox_prev_start.into());
                        window.set_hintbox_prev_end(hintbox_prev_end.into());
                    }
                    if *last_section_titles.borrow() != section_titles {
                        *last_section_titles.borrow_mut() = section_titles.clone();
                        window.set_section_titles(section_titles.into());
                    }

                    dialogs.borrow_mut().sync(&window);

                    window.set_dialog_shortcuts_open(shortcuts_dialog_open);
                    window.set_dialog_shortcuts_list_text(shortcuts_list_text.into());
                    window.set_dialog_delete_open(dialog_delete_open);
                    window.set_dialog_delete_text(dialog_delete_text.into());
                    window.set_dialog_delete_file_index(dialog_delete_file_index);
                    window.set_error_message(error_message.into());

                    window.set_is_recording(is_rec);
                    window.set_recording_mode(recording_mode.clone().into());
                    window.set_is_compiling(is_compiling);
                    window.set_compile_progress(compile_progress);
                    window.set_compile_stage(compile_stage.into());
                    window.set_dialog_compile_stages_text(compile_stages_text.into());
                    window.set_dialog_compile_eta_secs(compile_eta_secs);
                    window.set_dialog_compile_cancel_enabled(compile_cancel_enabled);
                    window.set_dialog_compile_finished(compile_finished);
                    window.set_dialog_compile_cancel_requested(compile_cancel_requested);
                    window.set_dialog_compile_publishing(compile_publishing);
                }
            }
        });

        let window_weak_clone = window_weak.clone();

        Ok(Self {
            _has_ui: true,
            state,
            window: Some(window),
            window_weak: Some(window_weak_clone),
            _action_tx: Some(action_tx),
            _should_close: should_close,
            _file_list_model: Some(file_list_vec_model),
            _waveform_prev_model: Some(waveform_prev_model),
            _waveform_current_model: Some(waveform_current_model),
            _timer: Some(timer),
        })
    }

    /// Обновляет только текущий индекс (подсветка строки) без пересборки списка.
    /// Прокручивает список к строке только если она не видна в текущей области.
    pub fn set_current_file_index_only(&self, ui_index: i32) -> Result<()> {
        if let Ok(mut state_guard) = self.state.lock() {
            state_guard.current_file_index = ui_index;
        }
        if let Some(window) = self.window_weak.as_ref().and_then(|weak| weak.upgrade()) {
            window.set_current_file_index(ui_index);
            self.ensure_row_visible(&window, ui_index)?;
        }
        Ok(())
    }

    const ROW_HEIGHT: f32 = 30.0;

    /// Прокручивает список к строке только если она не видна. Вызывается при любой смене текущего индекса.
    fn ensure_row_visible(&self, window: &MainWindow, ui_index: i32) -> Result<()> {
        let total_files = self
            .state
            .lock()
            .map(|state_guard| state_guard.file_list.len())
            .unwrap_or(0);
        if ui_index < 0 || total_files == 0 {
            return Ok(());
        }
        let current_y = window.get_file_list_scroll_viewport_y() as f32;
        let visible_height = window.get_file_list_visible_height();
        if visible_height <= 0.0 {
            return Ok(());
        }
        let row_top = (ui_index as f32) * Self::ROW_HEIGHT;
        let row_bottom = (ui_index as f32 + 1.0) * Self::ROW_HEIGHT;
        let visible_top = -current_y;
        let visible_bottom = -current_y + visible_height;
        let row_visible = row_top >= visible_top && row_bottom <= visible_bottom;
        if row_visible {
            return Ok(());
        }
        let scroll_y = Self::scroll_y_for_index(ui_index, total_files, visible_height);
        window.set_file_list_scroll_viewport_y(scroll_y);
        Ok(())
    }

    fn scroll_y_for_index(index: i32, total_files: usize, visible_height: f32) -> f32 {
        if index < 0 || total_files == 0 {
            return 0.0;
        }
        let total_h = (total_files as f32) * Self::ROW_HEIGHT;
        let max_scroll = (total_h - visible_height).max(0.0);
        (-(index as f32) * Self::ROW_HEIGHT).clamp(-max_scroll, 0.0)
    }

    /// Синхронизирует данные статусной строки из state в окно (без ожидания таймера).
    pub fn sync_status_line_to_window(&self) -> Result<()> {
        let (total_files, total_duration, total_size) = {
            let state_guard = self
                .state
                .lock()
                .map_err(|_| anyhow::anyhow!("state lock poisoned"))?;
            (
                state_guard.total_files.clone(),
                state_guard.total_duration.clone(),
                state_guard.total_size.clone(),
            )
        };
        if let Some(window) = self.window_weak.as_ref().and_then(|weak| weak.upgrade()) {
            window.set_total_files(total_files.into());
            window.set_total_duration(total_duration.into());
            window.set_total_size(total_size.into());
        }
        Ok(())
    }

    /// Синхронизирует state.file_list с моделью списка в окне (без ожидания таймера).
    pub fn sync_file_list_to_model(&self) -> Result<()> {
        let list = {
            let state_guard = self
                .state
                .lock()
                .map_err(|_| anyhow::anyhow!("state lock poisoned"))?;
            state_guard.file_list.clone()
        };
        if let Some(ref model) = self._file_list_model {
            apply_file_list_to_model(model, &list);
        }
        Ok(())
    }

    pub fn get_state(&self) -> Arc<Mutex<UIState>> {
        self.state.clone()
    }

    /// Загружает метаданные проекта в UIState и обновляет панель метаданных в окне.
    pub fn load_meta_from_project(
        &self,
        meta: &crate::project::project::Meta,
        settings: &crate::project::project::Settings,
    ) -> Result<()> {
        if let Ok(mut state_guard) = self.state.lock() {
            state_guard.dialog_project_title = meta.title.clone();
            state_guard.dialog_project_author = meta.author.clone();
            state_guard.dialog_project_year = meta.year.clone();
            state_guard.dialog_project_hint = meta.hint.clone();
            state_guard.dialog_project_reader = meta.reader.clone();
            state_guard.dialog_project_format_audio = settings.format_audio.clone();
            state_guard.dialog_project_normalize = settings.normalize;
            state_guard.dialog_project_cover = settings.cover.clone();
            state_guard.dialog_project_section_split = settings.section_split;
            state_guard.dialog_project_denoise = settings.denoise;
        }
        let _ = update_meta_window_from_state(&self.window_weak, &self.state);
        Ok(())
    }

    pub fn run(&mut self) -> std::result::Result<(), slint::PlatformError> {
        if let Some(window) = self.window.take() {
            window.run()
        } else {
            Ok(())
        }
    }

    pub fn update_waveform_prev(&mut self, data: &[f32]) -> Result<()> {
        if let Ok(mut state) = self.state.lock() {
            state.waveform_prev = data.to_vec();
        }
        Ok(())
    }

    pub fn update_waveform_current(&mut self, data: &[f32]) -> Result<()> {
        if let Ok(mut state) = self.state.lock() {
            state.waveform_current = data.to_vec();
        }
        Ok(())
    }

    pub fn update_level_indicator(&mut self, level: f32) -> Result<()> {
        if let Ok(mut state) = self.state.lock() {
            state.level = level;
        }
        Ok(())
    }

    pub fn update_status_line(
        &mut self,
        current_index: Option<usize>,
        total_files: usize,
        _current_file: Option<&ProjectFile>,
        _current_duration: Duration,
        total_duration: Duration,
        record_length_ms: u64,
        recording_elapsed: Duration,
        total_size: u64,
        free_space: u64,
        is_recording: bool,
        recording_mode: &str,
    ) -> Result<()> {
        if let Ok(mut state) = self.state.lock() {
            let total_dur_str = format_duration(total_duration, false);
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
            // current_index - это индекс в project.files (до reverse)
            // Нужно преобразовать его в номер файла (1-based, от последнего к первому)
            // После reverse: номер = total - orig_idx
            let current_file_num = if let Some(orig_idx) = current_index {
                if orig_idx < total_files {
                    total_files - orig_idx
                } else {
                    0
                }
            } else if !state.file_list.is_empty() {
                // Если current_index не установлен, используем последний файл (самый свежий)
                // После reverse он становится первым, но номер = total
                total_files
            } else {
                0
            };

            // Вычисляем общий размер диска (записанных + свободное место)
            let total_disk_size = total_size + free_space;
            let total_disk_str = format_size(total_disk_size);

            let file_name = _current_file
                .map(|file| {
                    Path::new(&file.path)
                        .file_name()
                        .and_then(|name| name.to_str())
                        .unwrap_or("")
                })
                .unwrap_or("");

            state.current_file_name = file_name.to_string();
            state.total_files = format!("{}/{}", current_file_num, total_files);
            state.total_duration = duration_display;
            state.total_size = format!("{} / {}", total_size_str, total_disk_str);
            state.is_recording = is_recording;
            state.recording_mode = recording_mode.to_string();
        }
        Ok(())
    }

    pub fn update_file_list(
        &mut self,
        files: &[ProjectFile],
        current_index: Option<usize>,
        recording_duration: Duration,
        is_playing: bool,
        temp_file_path: Option<&str>,
        recording_mode: &str,
    ) -> Result<()> {
        if let Ok(mut state) = self.state.lock() {
            let is_recording = !recording_duration.is_zero();
            let mut cumulative_duration = Duration::ZERO;
            let markers_batch = format_markers_with_ordinals_batch(files);

            let file_list: Vec<FileInfo> = files
                .iter()
                .enumerate()
                .map(|(orig_idx, f)| {
                    let path = Path::new(&f.path);
                    let name = path
                        .file_name()
                        .and_then(|name| name.to_str())
                        .unwrap_or("")
                        .to_string();
                    let file_is_playing = is_playing && current_index == Some(orig_idx);
                    let duration_ms = f.duration_ms;
                    let duration = format_duration(Duration::from_millis(duration_ms), false);
                    let start_time_str = format_duration(cumulative_duration, false);
                    cumulative_duration += Duration::from_millis(duration_ms);

                    let size_str = format_size(f.size);

                    let markers_str = markers_batch.get(orig_idx).cloned().unwrap_or_default();
                    let row_is_recording =
                        recording_mode == "U" && is_recording && current_index == Some(orig_idx);
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

            let mut file_list = file_list;
            let total_files = file_list.len();
            let mut new_index = if let Some(orig_idx) = current_index {
                orig_to_ui_index(orig_idx, total_files)
            } else if total_files > 0 {
                0
            } else {
                -1
            };

            if let Some(temp_path) = temp_file_path {
                if is_recording {
                    let path = Path::new(temp_path);
                    let name = path
                        .file_name()
                        .and_then(|name| name.to_str())
                        .unwrap_or("")
                        .to_string();
                    if recording_mode == "U" {
                        // Режим U: временная строка не добавляется, индикатор уже на строке
                    } else if recording_mode == "I" && current_index.is_some() {
                        let insert_at = current_index.unwrap() + 1;
                        let cum_insert: Duration = files
                            .iter()
                            .take(insert_at)
                            .map(|file| Duration::from_millis(file.duration_ms))
                            .sum();
                        let start_time_str = format_duration(cum_insert, false);
                        let temp_info = FileInfo {
                            index: (insert_at + 1) as i32,
                            name: name.into(),
                            path: temp_path.to_string().into(),
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
                        };
                        file_list.insert(insert_at, temp_info);
                        let len = file_list.len();
                        new_index = (len - 1 - insert_at) as i32;
                    } else {
                        let start_time_str = format_duration(cumulative_duration, false);
                        file_list.push(FileInfo {
                            index: (total_files + 1) as i32,
                            name: name.into(),
                            path: temp_path.to_string().into(),
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
                        new_index = 0;
                    }
                }
            }

            state.file_list = reverse_and_reindex_file_list(file_list);
            state.file_list_dirty_rows.clear();
            state.file_list_version = state.file_list_version.wrapping_add(1);
            state.current_file_index = new_index;

            Self::apply_hints_to_state(files, current_index, &mut state);
        }
        Ok(())
    }

    /// Обновляет в state подсказки текущей и предыдущей записи (одно место расчёта для обеих панелей).
    pub fn update_file_hints(
        &mut self,
        files: &[ProjectFile],
        current_index: Option<usize>,
    ) -> Result<()> {
        if let Ok(mut state) = self.state.lock() {
            Self::apply_hints_to_state(files, current_index, &mut state);
        }
        Ok(())
    }

    fn apply_hints_to_state(
        files: &[ProjectFile],
        current_index: Option<usize>,
        state: &mut UIState,
    ) {
        const HINT_MAX_LEN: usize = 100;
        let (cur_start, cur_end, prev_start, prev_end) =
            current_and_prev_file_hints(files, current_index, HINT_MAX_LEN);
        state.hintbox_current_start = cur_start.clone();
        state.hintbox_current_end = cur_end.clone();
        state.hintbox_prev_start = prev_start.clone();
        state.hintbox_prev_end = prev_end.clone();
        state.hintbox_prev = if state.hintbox_prev_end.is_empty() {
            state.hintbox_prev_start.clone()
        } else {
            format!(
                "{}[...]{}",
                state.hintbox_prev_start, state.hintbox_prev_end
            )
        };
    }

    pub fn render(&mut self) -> Result<()> {
        Ok(())
    }
}

fn normalize_key_for_layout(key: &str) -> String {
    if key.is_empty() {
        return String::new();
    }
    let first_char = key.chars().next().unwrap_or(' ');
    let normalized = match first_char {
        'й' | 'Й' => 'q',
        'ц' | 'Ц' => 'w',
        'у' | 'У' => 'e',
        'к' | 'К' => 'r',
        'е' | 'Е' => 't',
        'н' | 'Н' => 'y',
        'г' | 'Г' => 'u',
        'ш' | 'Ш' => 'i',
        'щ' | 'Щ' => 'o',
        'з' | 'З' => 'p',
        'ф' | 'Ф' => 'a',
        'ы' | 'Ы' => 's',
        'в' | 'В' => 'd',
        'а' | 'А' => 'f',
        'п' | 'П' => 'g',
        'р' | 'Р' => 'h',
        'о' | 'О' => 'j',
        'л' | 'Л' => 'k',
        'д' | 'Д' => 'l',
        'я' | 'Я' => 'z',
        'ч' | 'Ч' => 'x',
        'с' | 'С' => 'c',
        'м' | 'М' => 'v',
        'и' | 'И' => 'b',
        'т' | 'Т' => 'n',
        'ь' | 'Ь' | 'ъ' | 'Ъ' => 'm',
        'б' | 'Б' => '<',
        'ю' | 'Ю' => '>',
        ',' => '<',
        '.' => '>',
        char_val if char_val.is_ascii_alphanumeric() => char_val.to_ascii_lowercase(),
        _ => first_char.to_ascii_lowercase(),
    };
    normalized.to_string()
}

fn update_meta_window_from_state(
    window_weak: &Option<slint::Weak<MainWindow>>,
    state: &Arc<Mutex<UIState>>,
) -> Result<()> {
    if let Some(window) = window_weak.as_ref().and_then(|weak| weak.upgrade()) {
        if let Ok(state_guard) = state.lock() {
            window.set_meta_title(state_guard.dialog_project_title.clone().into());
            window.set_meta_author(state_guard.dialog_project_author.clone().into());
            window.set_meta_year(state_guard.dialog_project_year.clone().into());
            window.set_meta_hint(state_guard.dialog_project_hint.clone().into());
            window.set_meta_reader(state_guard.dialog_project_reader.clone().into());
            window.set_hintbox_current_start(state_guard.hintbox_current_start.clone().into());
            window.set_hintbox_current_end(state_guard.hintbox_current_end.clone().into());
            window.set_hintbox_prev_start(state_guard.hintbox_prev_start.clone().into());
            window.set_hintbox_prev_end(state_guard.hintbox_prev_end.clone().into());
            window.set_section_titles(state_guard.section_titles.clone().into());
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::dialogs::DialogReply;
    use crate::utils::keyboard::DialogEdit;
    use slint::platform::software_renderer::{MinimalSoftwareWindow, RepaintBufferType};
    use slint::platform::{Key, Platform, PointerEventButton, WindowAdapter, WindowEvent};
    use slint::ComponentHandle;
    use std::rc::Rc;

    struct TestPlatform(Rc<MinimalSoftwareWindow>);

    impl Platform for TestPlatform {
        fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, slint::PlatformError> {
            Ok(self.0.clone())
        }
    }

    fn draw(adapter: &MinimalSoftwareWindow) -> slint::SharedPixelBuffer<slint::Rgb8Pixel> {
        slint::platform::update_timers_and_animations();
        adapter.request_redraw();
        let size = adapter.size();
        let mut buffer = slint::SharedPixelBuffer::<slint::Rgb8Pixel>::new(size.width, size.height);
        adapter.draw_if_needed(|renderer| {
            renderer.render(buffer.make_mut_slice(), size.width as usize);
        });
        buffer
    }

    fn press(window: &MainWindow, key: impl Into<slint::SharedString>) {
        let text = key.into();
        window
            .window()
            .dispatch_event(WindowEvent::KeyPressed { text: text.clone() });
        window
            .window()
            .dispatch_event(WindowEvent::KeyReleased { text });
    }

    fn next_dialog_save(actions: &Receiver<Action>) -> (u64, DialogEdit) {
        let Action::SaveDialog { request_id, edit } = actions.try_recv().unwrap() else {
            panic!("Expected a dialog save");
        };
        (request_id, *edit)
    }

    #[test]
    #[ignore = "requires a disposable IAMREADER_SMOKE_PROJECT and IAMREADER_SMOKE_IMAGE_DIR"]
    fn ui_supplied_project_smoke() {
        use crate::audio::waveform::read_waveform_samples;
        use crate::project::project::Project;
        use crate::utils::paths::resolve_project_file;
        use crate::utils::stats::{calculate_sizes, calculate_total_duration, get_free_space};
        use std::io::Write;

        std::thread::spawn(|| {
            let project_path = std::path::PathBuf::from(
                std::env::var_os("IAMREADER_SMOKE_PROJECT")
                    .expect("IAMREADER_SMOKE_PROJECT is required"),
            )
            .canonicalize()
            .unwrap();
            let project_dir = project_path.parent().unwrap();
            let temp_dir = std::env::temp_dir().canonicalize().unwrap();
            assert!(
                project_dir.starts_with(&temp_dir),
                "Smoke requires a disposable project copy under the temporary directory"
            );
            let image_dir = std::path::PathBuf::from(
                std::env::var_os("IAMREADER_SMOKE_IMAGE_DIR")
                    .expect("IAMREADER_SMOKE_IMAGE_DIR is required"),
            );
            assert!(
                image_dir.starts_with(&temp_dir),
                "Smoke screenshots must be written under the temporary directory"
            );
            std::fs::create_dir_all(&image_dir).unwrap();

            let project_bytes = std::fs::read(&project_path).unwrap();
            let project = Project::load(&project_path).unwrap();
            let available: Vec<_> = project
                .files
                .iter()
                .filter_map(|entry| {
                    let path = resolve_project_file(project_dir, &entry.path)
                        .canonicalize()
                        .ok()?;
                    if !path.starts_with(project_dir)
                        || !path.is_file()
                        || !path
                            .extension()
                            .is_some_and(|extension| extension.eq_ignore_ascii_case("wav"))
                    {
                        return None;
                    }
                    let reader = hound::WavReader::open(&path).ok()?;
                    if reader.duration() == 0 {
                        return None;
                    }
                    Some((entry.clone(), path))
                })
                .take(20)
                .collect();
            assert!(
                !available.is_empty(),
                "No supplied WAV entries resolve inside the disposable project copy"
            );
            let files: Vec<_> = available.iter().map(|(entry, _)| entry.clone()).collect();
            let source_stats: Vec<_> = available
                .iter()
                .map(|(_, path)| {
                    let metadata = std::fs::metadata(path).unwrap();
                    (metadata.len(), metadata.modified().unwrap())
                })
                .collect();

            let adapter = MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer);
            slint::platform::set_platform(Box::new(TestPlatform(adapter.clone()))).unwrap();
            let (tx, rx) = crossbeam_channel::unbounded();
            let mut ui = UI::new(tx, project.settings.keys.clone(), None).unwrap();
            ui._timer.as_ref().unwrap().stop();
            ui.load_meta_from_project(&project.meta, &project.settings)
                .unwrap();
            let mut original_index = files.len() - 1;
            ui.update_file_list(
                &files,
                Some(original_index),
                Duration::ZERO,
                false,
                None,
                "A",
            )
            .unwrap();
            ui.sync_file_list_to_model().unwrap();
            let window = ui.window.as_ref().unwrap().clone_strong();
            window.show().unwrap();
            adapter.set_size(slint::LogicalSize::new(1200.0, 800.0));

            let update_selection = |ui: &mut UI, original_index: usize| {
                ui.set_current_file_index_only(orig_to_ui_index(original_index, files.len()))
                    .unwrap();
                ui.update_file_hints(&files, Some(original_index)).unwrap();
                ui.update_status_line(
                    Some(original_index),
                    files.len(),
                    Some(&files[original_index]),
                    Duration::from_millis(files[original_index].duration_ms),
                    calculate_total_duration(&files),
                    project.stats.record_length,
                    Duration::ZERO,
                    calculate_sizes(&files),
                    get_free_space(project_dir),
                    false,
                    "A",
                )
                .unwrap();
                ui.sync_status_line_to_window().unwrap();
                let current =
                    read_waveform_samples(&available[original_index].1, 1500, false).unwrap();
                let previous = original_index
                    .checked_sub(1)
                    .map(|index| read_waveform_samples(&available[index].1, 1500, false).unwrap())
                    .unwrap_or_default();
                ui.update_waveform_current(&current).unwrap();
                ui.update_waveform_prev(&previous).unwrap();
                ui.get_state().lock().unwrap().waveform_version += 1;
            };
            let snapshot = |name: &str| {
                let pixels = draw(&adapter);
                let mut writer =
                    std::io::BufWriter::new(std::fs::File::create(image_dir.join(name)).unwrap());
                write!(writer, "P6\n{} {}\n255\n", pixels.width(), pixels.height()).unwrap();
                for pixel in pixels.as_slice() {
                    writer.write_all(&[pixel.r, pixel.g, pixel.b]).unwrap();
                }
                writer.flush().unwrap();
            };
            update_selection(&mut ui, original_index);
            ui._timer.as_ref().unwrap().restart();
            std::thread::sleep(Duration::from_millis(45));
            snapshot("main-100.ppm");

            window
                .window()
                .dispatch_event(WindowEvent::ScaleFactorChanged { scale_factor: 1.25 });
            adapter.set_size(slint::LogicalSize::new(640.0, 720.0));
            draw(&adapter);
            let click = |position| {
                window.window().dispatch_event(WindowEvent::PointerPressed {
                    position,
                    button: PointerEventButton::Left,
                });
                window
                    .window()
                    .dispatch_event(WindowEvent::PointerReleased {
                        position,
                        button: PointerEventButton::Left,
                    });
            };
            click(slint::LogicalPosition::new(
                30.0,
                660.0 - window.get_file_list_visible_height() / 2.0,
            ));
            let Action::Goto {
                index: Some(ui_index),
                play: false,
            } = rx.try_recv().unwrap()
            else {
                panic!("Click must select a supplied fragment");
            };
            assert!(ui_index >= 0 && (ui_index as usize) < files.len());
            original_index = files.len() - ui_index as usize - 1;
            update_selection(&mut ui, original_index);
            std::thread::sleep(Duration::from_millis(45));
            snapshot("main-half-125.ppm");

            press(&window, Key::F5);
            assert!(window.get_dialog_project_open());
            snapshot("project-half-125.ppm");
            click(slint::LogicalPosition::new(230.0, 187.0)); // Export tab
            snapshot("project-export-half-125.ppm");
            click(slint::LogicalPosition::new(300.0, 227.0));
            draw(&adapter);
            click(slint::LogicalPosition::new(300.0, 272.0));
            assert_eq!(window.get_dialog_project_format_audio(), "mp3");
            draw(&adapter);
            click(slint::LogicalPosition::new(300.0, 227.0));
            draw(&adapter);
            click(slint::LogicalPosition::new(300.0, 227.0));
            assert_eq!(window.get_dialog_project_format_audio(), "wav");
            snapshot("project-export-half-125.ppm");
            click(slint::LogicalPosition::new(524.0, 582.0));
            assert!(window.get_dialog_project_open());
            assert!(window.get_dialog_busy());
            let (request_id, DialogEdit::Meta(meta)) = next_dialog_save(&rx)
            else {
                panic!("Project Save must submit metadata, not mutate the source project");
            };
            ui.get_state().lock().unwrap().dialog_save_result = Some(DialogReply {
                request_id,
                result: Ok(None),
                field: None,
            });
            std::thread::sleep(Duration::from_millis(45));
            draw(&adapter);
            assert!(!window.get_dialog_project_open());
            assert_eq!(meta.title, project.meta.title);
            assert_eq!(meta.author, project.meta.author);
            assert_eq!(meta.year, project.meta.year);
            assert_eq!(meta.format_audio, "wav");

            window.invoke_file_double_clicked(ui_index);
            assert!(window.get_dialog_chunk_open());
            snapshot("chunk-half-125.ppm");
            click(slint::LogicalPosition::new(524.0, 532.0));
            assert!(window.get_dialog_chunk_open());
            let (request_id, DialogEdit::Chunk { data: chunk, expected_path }) = next_dialog_save(&rx)
            else {
                panic!("Chunk Save must submit metadata, not mutate the source project");
            };
            assert_eq!(expected_path, files[original_index].path);
            ui.get_state().lock().unwrap().dialog_save_result = Some(DialogReply {
                request_id,
                result: Ok(None),
                field: None,
            });
            std::thread::sleep(Duration::from_millis(45));
            draw(&adapter);
            assert!(!window.get_dialog_chunk_open());
            assert_eq!(chunk.title, files[original_index].title);
            assert_eq!(chunk.author, files[original_index].author);
            assert_eq!(chunk.year, files[original_index].year);
            assert_eq!(chunk.hint, files[original_index].hint);
            assert!(rx.is_empty());

            // Real supplied marker definitions; local drafts and captures never touch JSON/WAV.
            for (width, height, scale_factor, label) in [
                (400.0_f32, 300.0_f32, 1.0, "minimum-100"),
                (400.0, 300.0, 1.25, "minimum-125"),
                (640.0, 480.0, 1.0, "half-100"),
                (640.0, 480.0, 1.25, "half-125"),
                (1200.0, 800.0, 1.0, "wide-100"),
            ] {
                window.window().dispatch_event(WindowEvent::ScaleFactorChanged { scale_factor });
                adapter.set_size(slint::LogicalSize::new(width, height));
                let dw = 520.0_f32.min(width - 20.0);
                let dh = 500.0_f32.min(height - 20.0);
                press(&window, Key::F5);
                draw(&adapter);
                click(slint::LogicalPosition::new((width - dw) / 2.0 + 62.0, (height - dh) / 2.0 + 77.0));
                snapshot(&format!("project-{label}.ppm"));
                press(&window, Key::Escape);
                window.invoke_file_double_clicked(ui_index);
                snapshot(&format!("chunk-{label}.ppm"));
                press(&window, Key::Escape);
                {
                    let state = ui.get_state();
                    let mut state = state.lock().unwrap();
                    state.dialog_marker_snapshot = project.markers.clone();
                    state.dialog_markers_open = true;
                }
                std::thread::sleep(Duration::from_millis(45));
                snapshot(&format!("markers-{label}.ppm"));
                assert!(window.get_dialog_markers_open());
                assert_eq!(window.get_dialog_markers_list().row_count(), project.markers.len());
                if label == "half-125" {
                    // Exercise the actual selector callback while retaining another marker's draft.
                    let marker_width = 900.0_f32.min(width - 20.0);
                    let marker_height = 680.0_f32.min(height - 20.0);
                    let combo_y = (height - marker_height) / 2.0 + 77.0;
                    window.set_dialog_markers_title("Unsaved draft".into());
                    click(slint::LogicalPosition::new(300.0, combo_y));
                    draw(&adapter);
                    click(slint::LogicalPosition::new(300.0, combo_y + 45.0));
                    draw(&adapter);
                    assert_eq!(window.get_dialog_markers_selected_index(), 1);
                    assert_ne!(window.get_dialog_markers_title(), "Unsaved draft");
                    click(slint::LogicalPosition::new(300.0, combo_y));
                    draw(&adapter);
                    click(slint::LogicalPosition::new(300.0, combo_y));
                    draw(&adapter);
                    assert_eq!(window.get_dialog_markers_selected_index(), 0);
                    assert_eq!(window.get_dialog_markers_title(), "Unsaved draft");
                    assert!(rx.is_empty());
                    assert!(marker_width > 0.0);
                }
                window.window().dispatch_event(WindowEvent::PointerScrolled {
                    position: slint::LogicalPosition::new(width / 2.0, height / 2.0), delta_x: 0.0, delta_y: -460.0,
                });
                snapshot(&format!("marker-sounds-{label}.ppm"));
                window.set_dialog_error_message("Cannot save project: disk is full. The draft is still here. Free some space and try Save again. A very long path: /home/reader/books/a-long-book-title/iamreader.json".into());
                snapshot(&format!("marker-error-{label}.ppm"));
                let dw = 900.0_f32.min(width - 20.0);
                let dh = 680.0_f32.min(height - 20.0);
                click(slint::LogicalPosition::new((width + dw) / 2.0 - 148.0, (height + dh) / 2.0 - 28.0));
                assert!(!window.get_dialog_markers_open(), "Cancel must stay reachable at {label}");
                assert!(rx.is_empty());
            }
            ui._timer.as_ref().unwrap().stop();
            adapter.set_size(slint::LogicalSize::new(400.0, 300.0));
            window.set_dialog_toc_list_text("1. Chapter one\n2. A longer chapter title that should wrap without horizontal scrolling\n3. Chapter three".into());
            window.set_dialog_toc_open(true);
            snapshot("toc-minimum.ppm");
            press(&window, Key::Escape);
            window.set_dialog_shortcuts_list_text("r — Record\ne — Confirm\nd — Stop\na / f — Previous / Next\ns — Play\nu — Update mode\ni — Insert mode\nF1 — Shortcuts\nF4 — Markers\nF5 — Project\nF8 — Compile\nF10 — Table of contents".into());
            window.set_dialog_shortcuts_open(true);
            snapshot("shortcuts-minimum.ppm");
            press(&window, Key::Escape);
            window.set_dialog_delete_text("Delete recording 00013.wav? This action cannot be undone.".into());
            window.set_dialog_delete_open(true);
            snapshot("delete-minimum.ppm");
            press(&window, Key::Escape);

            assert_eq!(std::fs::read(&project_path).unwrap(), project_bytes);
            for ((_, path), expected) in available.iter().zip(source_stats) {
                let metadata = std::fs::metadata(path).unwrap();
                assert_eq!((metadata.len(), metadata.modified().unwrap()), expected);
            }
            eprintln!(
                "Read-only software-renderer smoke: {} supplied WAV entries, screenshots in {}",
                files.len(),
                image_dir.display()
            );
        })
        .join()
        .unwrap();
    }

    #[test]
    fn ui_export_outcomes_remain_until_close() {
        use crate::project::compile_progress::{CompileProgress, Operation, ProgressUnit};
        use crate::project::export_workspace::ExportCancelled;
        use std::io::Write;

        std::thread::spawn(|| {
            // Optional software-renderer captures; normal test runs never write images.
            let image_dir = std::env::var_os("IAMREADER_EXPORT_IMAGE_DIR").map(|path| {
                let path = std::path::PathBuf::from(path);
                let temp_dir = std::env::temp_dir().canonicalize().unwrap();
                assert!(
                    path.starts_with(&temp_dir)
                        && !path.components().any(|part| matches!(part, std::path::Component::ParentDir)),
                    "Export screenshots must be written under the temporary directory"
                );
                let existing_ancestor = path.ancestors().find(|ancestor| ancestor.exists()).unwrap();
                assert!(existing_ancestor.canonicalize().unwrap().starts_with(&temp_dir));
                std::fs::create_dir_all(&path).unwrap();
                let path = path.canonicalize().unwrap();
                assert!(path.starts_with(&temp_dir));
                path
            });
            let scale_factor = if image_dir.is_some() { 1.25 } else { 1.0 };
            let adapter = MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer);
            slint::platform::set_platform(Box::new(TestPlatform(adapter.clone()))).unwrap();
            let (tx, rx) = crossbeam_channel::unbounded();
            let ui = UI::new(tx, KeyBindings::default(), None).unwrap();
            let state = ui.get_state();
            let window = ui.window.as_ref().unwrap();
            window.show().unwrap();
            window.window().dispatch_event(WindowEvent::ScaleFactorChanged { scale_factor });
            adapter.set_size(slint::LogicalSize::new(640.0, 720.0));
            let snapshot = |name: &str, pixels: &slint::SharedPixelBuffer<slint::Rgb8Pixel>| {
                if let Some(image_dir) = &image_dir {
                    let mut writer = std::io::BufWriter::new(
                        std::fs::File::create(image_dir.join(name)).unwrap(),
                    );
                    write!(writer, "P6\n{} {}\n255\n", pixels.width(), pixels.height()).unwrap();
                    for pixel in pixels.as_slice() {
                        writer.write_all(&[pixel.r, pixel.g, pixel.b]).unwrap();
                    }
                    writer.flush().unwrap();
                }
            };
            let tick = || {
                std::thread::sleep(Duration::from_millis(45));
                draw(&adapter)
            };
            let click_action = || {
                let position = slint::LogicalPosition::new(320.0, 485.0);
                window.window().dispatch_event(WindowEvent::PointerPressed {
                    position,
                    button: PointerEventButton::Left,
                });
                window
                    .window()
                    .dispatch_event(WindowEvent::PointerReleased {
                        position,
                        button: PointerEventButton::Left,
                    });
            };

            window.invoke_dialog_compile_cancel();
            window.invoke_dialog_compile_close();
            assert!(rx.is_empty());
            state.lock().unwrap().is_compiling = true;
            let mut operation = CompileProgress::new(
                Some(&state),
                None,
                Operation {
                    label: "Section 2/3 · Analyze level".into(),
                    start: 0.2,
                    end: 0.6,
                    unit: ProgressUnit::Audio { sample_rate: 48_000, channels: 2 },
                },
            );
            operation.report(0, 0).unwrap();
            // Exercise real elapsed-time ETA and interleaved-sample formatting without audio I/O.
            std::thread::sleep(Duration::from_millis(550));
            let total_samples = 48_000 * 2 * 3600;
            let done_samples = total_samples / 4;
            operation.report(done_samples, total_samples).unwrap();
            for (capture_scale, image_name) in [
                (1.0, "export-audio-progress-half-100.ppm"),
                (1.25, "export-audio-progress-half-125.ppm"),
            ] {
                window.window().dispatch_event(WindowEvent::ScaleFactorChanged { scale_factor: capture_scale });
                adapter.set_size(slint::LogicalSize::new(640.0, 480.0));
                let pixels = tick();
                assert_eq!(window.get_compile_stage(), "Section 2/3 · Analyze level: 25% · 00:15:00 / 01:00:00");
                assert!(window.get_dialog_compile_stages_text().contains("00:15:00 / 01:00:00"));
                assert!((window.get_compile_progress() - 0.3).abs() < f32::EPSILON);
                assert!(window.get_dialog_compile_eta_secs().is_finite());
                assert!(window.get_dialog_compile_eta_secs() > 0.0);
                assert!(window.get_dialog_compile_cancel_enabled());
                assert!(!window.get_dialog_compile_finished());
                snapshot(image_name, &pixels);
            }
            window.window().dispatch_event(WindowEvent::ScaleFactorChanged { scale_factor });
            adapter.set_size(slint::LogicalSize::new(640.0, 720.0));
            snapshot("export-running-125.ppm", &tick());
            assert!(window.get_is_compiling());
            assert!(window.get_dialog_compile_cancel_enabled());
            assert!(!window.get_dialog_compile_finished());
            window.invoke_dialog_compile_close();
            assert!(state.lock().unwrap().is_compiling);
            click_action();
            assert_eq!(rx.try_recv().unwrap(), Action::CompileCancel);
            assert!(state.lock().unwrap().compile_cancel_requested);
            assert!(window.get_dialog_compile_cancel_requested());
            assert!(!window.get_dialog_compile_cancel_enabled());
            click_action();
            window.invoke_dialog_compile_cancel();
            press(window, Key::Escape);
            assert!(rx.is_empty());
            assert!(window.get_is_compiling());
            std::thread::sleep(Duration::from_millis(110));
            assert!(operation.report(done_samples, total_samples).unwrap_err().is::<ExportCancelled>());
            tick();
            assert_eq!(window.get_compile_stage(), "Cancelling…");
            assert!(window.get_dialog_compile_eta_secs() < 0.0);

            let mut finished_status_images = Vec::new();
            for (index, (outcome, image_name)) in [
                ("Cancelled: no output published", "export-cancelled-125.ppm"),
                ("Done: /tmp/book/export/book.wav", "export-done-125.ppm"),
                (
                    "Error: Cannot publish /tmp/project-with-a-long-name/export/audiobook-with-a-long-title.wav: Disk is full",
                    "export-error-125.ppm",
                ),
            ]
            .into_iter()
            .enumerate()
            {
                if index == 1 {
                    {
                        let mut state = state.lock().unwrap();
                        state.is_compiling = true;
                        state.compile_publishing = true;
                        state.compile_stage = "Publishing".to_owned();
                    }
                    tick();
                    assert!(window.get_dialog_compile_publishing());
                    assert!(!window.get_dialog_compile_cancel_enabled());
                    assert!(!window.get_dialog_compile_finished());
                    click_action();
                    window.invoke_dialog_compile_cancel();
                    window.invoke_dialog_compile_close();
                    press(window, Key::Escape);
                    assert!(rx.is_empty());
                    assert!(!state.lock().unwrap().compile_cancel_requested);
                    assert!(window.get_is_compiling());
                }
                {
                    let mut state = state.lock().unwrap();
                    state.is_compiling = true;
                    state.compile_stage = outcome.to_owned();
                    state.dialog_compile_stages_text =
                        "Reading: done\nNormalization: done\nEncoding: done".to_owned();
                    // An old timestamp must not auto-dismiss any terminal result.
                    state.compile_finished_at =
                        Some(std::time::Instant::now() - Duration::from_secs(60));
                }
                let pixels = tick();
                snapshot(image_name, &pixels);
                assert!(window.get_is_compiling());
                assert!(window.get_dialog_compile_finished());
                assert!(!window.get_dialog_compile_cancel_enabled());
                assert_eq!(window.get_compile_stage(), outcome);
                // Verify the displayed status changes despite an identical non-empty stages log.
                let physical = |logical: f32| (logical * scale_factor) as usize;
                let status_pixels: Vec<_> = (physical(245.0)..physical(290.0))
                    .flat_map(|y| {
                        pixels.as_slice()
                            [y * pixels.width() as usize + physical(100.0)
                                ..y * pixels.width() as usize + physical(540.0)]
                            .iter()
                            .copied()
                    })
                    .collect();
                if let Some(previous) = finished_status_images.last() {
                    assert_ne!(previous, &status_pixels);
                }
                finished_status_images.push(status_pixels);
                tick();
                assert!(window.get_is_compiling());
                assert_eq!(window.get_compile_stage(), outcome);
                window.invoke_dialog_compile_cancel();
                assert!(rx.is_empty());
                if index == 0 {
                    press(window, Key::Escape);
                } else {
                    click_action();
                }
                assert!(!window.get_is_compiling());
                assert!(!window.get_modal_open());
                assert!(rx.is_empty(), "Close must not dispatch Cancel");
                let state = state.lock().unwrap();
                assert!(!state.is_compiling);
                assert!(state.compile_finished_at.is_none());
                assert!(!state.compile_cancel_requested);
                assert!(!state.compile_publishing);
                assert!(state.compile_stage.is_empty());
                assert!(state.dialog_compile_stages_text.is_empty());
            }
        })
        .join()
        .unwrap();
    }

    #[test]
    fn ui_model_shortcuts_modality_and_viewport() {
        // One dedicated thread owns the Slint platform; no display server or audio device is used.
        std::thread::spawn(|| {
            let adapter = MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer);
            slint::platform::set_platform(Box::new(TestPlatform(adapter.clone()))).unwrap();
            let (tx, rx) = crossbeam_channel::unbounded();
            let ui = UI::new(tx, KeyBindings::default(), None).unwrap();
            ui._timer.as_ref().unwrap().stop();
            let window = ui.window.as_ref().unwrap();
            window.show().unwrap();
            adapter.set_size(slint::LogicalSize::new(1200.0, 800.0));

            let state = ui.get_state();
            state.lock().unwrap().file_list = (0..100)
                .map(|index| FileInfo {
                    index,
                    name: format!("chunk-{index}").into(),
                    ..Default::default()
                })
                .collect();
            ui.sync_file_list_to_model().unwrap();
            let first = FileInfo {
                index: 100,
                name: "newest".into(),
                author: "Original author".into(),
                year: "1984".into(),
                ..Default::default()
            };
            state.lock().unwrap().file_list.insert(0, first.clone());
            ui.sync_file_list_to_model().unwrap();
            assert_eq!(window.get_file_list().row_data(0), Some(first));
            assert_eq!(window.get_file_list().row_data(1).unwrap().name, "chunk-0");
            {
                let mut state = state.lock().unwrap();
                state.file_list[0].index = 101;
                state.file_list[0].name = "renamed".into();
                state.file_list[0].size = "2 MB".into();
            }
            ui.sync_file_list_to_model().unwrap();
            assert_eq!(
                window.get_file_list().row_data(0),
                Some(state.lock().unwrap().file_list[0].clone())
            );
            state.lock().unwrap().file_list.pop();
            ui.sync_file_list_to_model().unwrap();
            assert_eq!(window.get_file_list().row_count(), 100);
            draw(&adapter);

            window.invoke_file_double_clicked(0);
            draw(&adapter);
            assert!(window.get_dialog_chunk_open());
            assert_eq!(window.get_dialog_chunk_author(), "Original author");
            assert_eq!(window.get_dialog_chunk_year(), "1984");
            window.set_dialog_chunk_title("Edited title".into());
            window.set_dialog_chunk_hint("Edited hint".into());
            let position = slint::LogicalPosition::new(804.0, 572.0);
            window.window().dispatch_event(WindowEvent::PointerPressed { position, button: PointerEventButton::Left });
            window.window().dispatch_event(WindowEvent::PointerReleased { position, button: PointerEventButton::Left });
            assert!(window.get_dialog_chunk_open());
            let (request_id, DialogEdit::Chunk { data: settings, .. }) = next_dialog_save(&rx) else {
                panic!("Chunk Save must submit the existing metadata");
            };
            state.lock().unwrap().dialog_save_result = Some(DialogReply { request_id, result: Ok(None), field: None });
            ui._timer.as_ref().unwrap().restart();
            std::thread::sleep(Duration::from_millis(45));
            draw(&adapter);
            ui._timer.as_ref().unwrap().stop();
            assert!(!window.get_dialog_chunk_open());
            assert_eq!(settings.ui_index, 0);
            assert_eq!(settings.title, "Edited title");
            assert_eq!(settings.hint, "Edited hint");
            assert_eq!(settings.author, "Original author");
            assert_eq!(settings.year, "1984");
            assert!(rx.is_empty());
            draw(&adapter);

            let position = slint::LogicalPosition::new(
                30.0,
                740.0 - window.get_file_list_visible_height() / 2.0,
            );
            window.window().dispatch_event(WindowEvent::PointerPressed {
                position,
                button: PointerEventButton::Left,
            });
            window
                .window()
                .dispatch_event(WindowEvent::PointerReleased {
                    position,
                    button: PointerEventButton::Left,
                });
            assert!(matches!(rx.try_recv().unwrap(), Action::Goto { .. }));
            let shortcuts: [(slint::SharedString, Action); 7] = [
                (Key::UpArrow.into(), Action::Next),
                (Key::DownArrow.into(), Action::Prev),
                (
                    "3".into(),
                    Action::AddMarker {
                        marker: "3".to_owned(),
                    },
                ),
                (Key::F1.into(), Action::OpenShortcutsDialog),
                (Key::F4.into(), Action::OpenMarkerSettings),
                (Key::F8.into(), Action::Compile),
                (Key::Delete.into(), Action::OpenDeleteChunkDialog),
            ];
            for (key, action) in shortcuts {
                press(window, key);
                assert_eq!(rx.try_recv().unwrap(), action);
                assert!(rx.is_empty());
            }

            let dialogs: [fn(&MainWindow, bool); 7] = [
                MainWindow::set_dialog_project_open,
                MainWindow::set_dialog_chunk_open,
                MainWindow::set_dialog_markers_open,
                MainWindow::set_dialog_toc_open,
                MainWindow::set_dialog_shortcuts_open,
                MainWindow::set_dialog_delete_open,
                MainWindow::set_is_compiling,
            ];
            for set_open in dialogs {
                set_open(window, true);
                draw(&adapter);
                assert!(window.get_modal_open());
                let keys: [slint::SharedString; 8] = [
                    Key::F1.into(),
                    Key::F4.into(),
                    Key::F5.into(),
                    Key::F8.into(),
                    Key::F10.into(),
                    Key::UpArrow.into(),
                    "3".into(),
                    "r".into(),
                ];
                for key in keys {
                    press(window, key);
                }
                window.invoke_key_pressed("r".into());
                window.invoke_add_marker_with_normalization("3".into());
                window.invoke_compile();
                window.invoke_dialog_markers_request_open();
                window.invoke_file_clicked(0);
                assert!(rx.is_empty());
                if window.get_is_compiling() {
                    state.lock().unwrap().is_compiling = true;
                    window.invoke_dialog_compile_cancel();
                    assert_eq!(rx.try_recv().unwrap(), Action::CompileCancel);
                    let mut state = state.lock().unwrap();
                    state.is_compiling = false;
                    state.compile_cancel_requested = false;
                }
                set_open(window, false);
                assert!(
                    !window.get_modal_open(),
                    "a shortcut opened a second dialog"
                );
                draw(&adapter);
            }

            press(window, Key::F5);
            draw(&adapter);
            assert!(window.get_dialog_project_open());
            // Tab traverses Close, Book, Export and then the title, never the background.
            for _ in 0..4 { press(window, Key::Tab); }
            press(window, "r");
            press(window, "7");
            assert_eq!(window.get_dialog_project_title(), "r7");
            assert!(rx.is_empty());
            press(window, Key::Escape);
            assert!(!window.get_dialog_project_open());
            press(window, "r");
            assert_eq!(
                rx.try_recv().unwrap(),
                Action::Record {
                    duration_secs: None
                }
            );

            for height in [600.0, 1000.0] {
                adapter.set_size(slint::LogicalSize::new(1200.0, height));
                draw(&adapter);
                window.set_file_list_scroll_viewport_y(0.0);
                ui.set_current_file_index_only(99).unwrap();
                let viewport_height = window.get_file_list_visible_height();
                let scroll_y = window.get_file_list_scroll_viewport_y();
                assert!(viewport_height > 0.0);
                assert!((3000.0 + scroll_y - viewport_height).abs() < 1.0);
            }

            // A half-screen/non-maximized window must keep the dialog and Save button reachable.
            for scale_factor in [1.0, 1.25] {
                window.window().dispatch_event(WindowEvent::ScaleFactorChanged { scale_factor });
                for (width, height) in [(400.0_f32, 300.0_f32), (640.0, 480.0), (800.0, 800.0)] {
                    adapter.set_size(slint::LogicalSize::new(width, height));
                    for markers in [false, true] {
                        let (dialog_width, dialog_height) = if markers {
                            window.set_dialog_markers_list(slint::ModelRc::new(slint::VecModel::from(vec!["chapter".into()])));
                            window.set_dialog_markers_selected_index(0);
                            window.set_dialog_markers_open(true);
                            (900.0_f32.min(width - 20.0), 680.0_f32.min(height - 20.0))
                        } else {
                            window.set_dialog_project_open(true);
                            (520.0_f32.min(width - 20.0), 500.0_f32.min(height - 20.0))
                        };
                        let pixels = draw(&adapter);
                        let left = (width - dialog_width) / 2.0;
                        let top = (height - dialog_height) / 2.0;
                        for (x, y) in [(left + 5.0, top + 5.0), (left + dialog_width - 5.0, top + dialog_height - 5.0)] {
                            let index = (y * scale_factor) as usize * pixels.width() as usize + (x * scale_factor) as usize;
                            assert_eq!(pixels.as_slice()[index], slint::Rgb8Pixel::new(255, 255, 255));
                        }
                        let position = slint::LogicalPosition::new(left + dialog_width - 56.0, top + dialog_height - 28.0);
                        window.window().dispatch_event(WindowEvent::PointerPressed { position, button: PointerEventButton::Left });
                        window.window().dispatch_event(WindowEvent::PointerReleased { position, button: PointerEventButton::Left });
                        assert!(window.get_dialog_busy(), "Save outside visible dialog: {width}x{height}, scale={scale_factor}, markers={markers}");
                        let Action::SaveDialog { request_id, .. } = rx.try_recv().unwrap() else { panic!("Expected dialog save"); };
                        state.lock().unwrap().dialog_save_result = Some(DialogReply { request_id, result: Ok(None), field: None });
                        ui._timer.as_ref().unwrap().restart();
                        std::thread::sleep(Duration::from_millis(45));
                        draw(&adapter);
                        ui._timer.as_ref().unwrap().stop();
                        if markers { window.invoke_dialog_markers_cancel(); }
                        assert!(!window.get_modal_open());
                        assert!(rx.is_empty());
                    }
                }
            }

            {
                let mut state = state.lock().unwrap();
                state.error_message = "Cannot save project\nDisk is full".to_owned();
                state.waveform_prev = vec![0.1, 0.2];
                state.waveform_current = vec![0.3, 0.4];
            }
            ui._timer.as_ref().unwrap().restart();
            std::thread::sleep(Duration::from_millis(45));
            draw(&adapter);
            assert_eq!(
                window.get_error_message(),
                "Cannot save project\nDisk is full"
            );
            assert!(!window.get_modal_open());
            window.invoke_dismiss_error();
            assert!(state.lock().unwrap().error_message.is_empty());
            {
                let mut state = state.lock().unwrap();
                state.is_recording = true;
                state.file_list[99].duration = "00:04".into();
            }
            let version = state.lock().unwrap().file_list_version;
            std::thread::sleep(Duration::from_millis(45));
            draw(&adapter);
            assert!(window.get_error_message().is_empty());
            assert_eq!(
                window.get_file_list().row_data(99).unwrap().duration,
                "00:04"
            );
            assert_eq!(state.lock().unwrap().file_list_version, version);

            {
                let mut state = state.lock().unwrap();
                state.is_recording = false;
                state.waveform_prev = vec![0.5, 0.6];
                state.waveform_current = vec![0.7, 0.8];
                state.file_list_version += 1;
            }
            std::thread::sleep(Duration::from_millis(45));
            draw(&adapter);
            assert_eq!(window.get_waveform_prev().row_data(0), Some(0.5));
            assert_eq!(window.get_waveform_current().row_data(0), Some(0.7));

            // Async results can replace same-length graphs without changing list or selection.
            let list_version = state.lock().unwrap().file_list_version;
            {
                let mut state = state.lock().unwrap();
                state.waveform_prev = vec![0.9, 1.0];
                state.waveform_current = vec![0.2, 0.1];
                state.waveform_version += 1;
            }
            std::thread::sleep(Duration::from_millis(45));
            draw(&adapter);
            assert_eq!(window.get_waveform_prev().row_data(0), Some(0.9));
            assert_eq!(window.get_waveform_current().row_data(0), Some(0.2));
            assert_eq!(state.lock().unwrap().file_list_version, list_version);
            {
                let mut state = state.lock().unwrap();
                state.waveform_prev.clear();
                state.waveform_current.clear();
                state.waveform_prev_loading = true;
                state.waveform_current_loading = true;
                state.waveform_version += 1;
            }
            std::thread::sleep(Duration::from_millis(45));
            draw(&adapter);
            assert!(window.get_waveform_prev_loading());
            assert!(window.get_waveform_current_loading());
            assert_eq!(window.get_waveform_prev().row_count(), 0);
            assert_eq!(window.get_waveform_current().row_count(), 0);
            {
                let mut state = state.lock().unwrap();
                state.waveform_prev = vec![0.8, 0.3];
                state.waveform_current = vec![0.4, 0.6];
                state.waveform_prev_loading = false;
                state.waveform_current_loading = false;
                state.waveform_version += 1;
            }
            std::thread::sleep(Duration::from_millis(45));
            draw(&adapter);
            assert!(!window.get_waveform_prev_loading());
            assert!(!window.get_waveform_current_loading());
            assert_eq!(window.get_waveform_prev().row_data(0), Some(0.8));
            assert_eq!(window.get_waveform_current().row_data(0), Some(0.4));
            assert_eq!(state.lock().unwrap().file_list_version, list_version);

            // Selection/playback patches coalesce between ticks without a full-list invalidation.
            {
                let mut state = state.lock().unwrap();
                state.file_list[0].is_playing = true;
                state.mark_file_list_row_changed(0);
            }
            std::thread::sleep(Duration::from_millis(45));
            draw(&adapter);
            assert!(window.get_file_list().row_data(0).unwrap().is_playing);
            let version = state.lock().unwrap().file_list_version;
            {
                let mut state = state.lock().unwrap();
                state.file_list[0].is_playing = false;
                state.mark_file_list_row_changed(0);
                state.file_list[1].is_playing = true;
                state.mark_file_list_row_changed(1);
                state.file_list[1].is_playing = false;
                state.mark_file_list_row_changed(1);
                state.file_list[2].is_playing = true;
                state.mark_file_list_row_changed(2);
                state.current_file_index = 2;
                state.mark_file_list_row_changed(usize::MAX);
                assert_eq!(state.file_list_dirty_rows.len(), 3);
            }
            std::thread::sleep(Duration::from_millis(45));
            draw(&adapter);
            assert!(!window.get_file_list().row_data(0).unwrap().is_playing);
            assert!(!window.get_file_list().row_data(1).unwrap().is_playing);
            assert!(window.get_file_list().row_data(2).unwrap().is_playing);
            assert_eq!(window.get_current_file_index(), 2);
            assert_eq!(state.lock().unwrap().file_list_version, version);
            assert!(state.lock().unwrap().file_list_dirty_rows.is_empty());

            // A structural snapshot supersedes old patches, including now-out-of-range rows.
            {
                let mut state = state.lock().unwrap();
                state.mark_file_list_row_changed(2);
                state.file_list.truncate(1);
                state.file_list[0].name = "replacement".into();
                state.file_list_version += 1;
            }
            std::thread::sleep(Duration::from_millis(45));
            draw(&adapter);
            assert_eq!(window.get_file_list().row_count(), 1);
            assert_eq!(window.get_file_list().row_data(0).unwrap().name, "replacement");
            assert!(state.lock().unwrap().file_list_dirty_rows.is_empty());
        })
        .join()
        .unwrap();
    }
}
