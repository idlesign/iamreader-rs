use super::file_list::{show_fragment, show_summary};
use super::keyboard;
use super::dialogs::{DialogKind, EditDialog, ExportDialog};
use super::notifications::RepaintBridge;
use super::transport::EngineClient;
use super::widgets::{show_level, show_text_bar, show_waveform};
use anyhow::Result;
use eframe::egui::{self, Ui};
use iamreader_engine::engine::{ChunkId, EngineCommand, EngineEvent, EngineHandle};

const ROW_HEIGHT: f32 = 36.0;

/// Frontend-local input, selection scrolling and errors; no project mutation or audio ownership.
pub struct EguiApp {
    client: EngineClient,
    search: String,
    error: String,
    last_engine_error: String,
    selection: Option<(i32, String)>,
    scroll_target: Option<usize>,
    repaint: Option<RepaintBridge>,
    dialog: Option<EditDialog>,
    export_dialog: ExportDialog,
    shutdown_request: Option<u64>,
}

impl EguiApp {
    /// Attach without opening audio devices, starting threads or installing native windows.
    pub fn new(handle: EngineHandle) -> Result<Self> {
        Ok(Self {
            client: EngineClient::new(handle)?, search: String::new(), error: String::new(),
            last_engine_error: String::new(), selection: None, scroll_target: None, repaint: None,
            dialog: None, export_dialog: ExportDialog::default(), shutdown_request: None,
        })
    }

    pub(super) fn attach_repaint(&mut self, context: &egui::Context) -> Result<()> {
        self.repaint = Some(RepaintBridge::start(self.client.handle.subscribe_updates()?, context.clone())?);

        Ok(())
    }

    pub(super) fn start_monitoring(&mut self) {
        self.dispatch(EngineCommand::StartMonitoring);
    }

    fn dispatch(&mut self, command: EngineCommand) {
        if let Err(error) = self.client.submit(command) { self.error = format!("{error:#}"); }
    }

    fn poll(&mut self) -> Result<()> {
        for reply in self.client.poll()? {
            if self.dialog.as_mut().is_some_and(|dialog| dialog.accept_reply(&reply)) { continue; }
            if self.export_dialog.accept_reply(&reply) { continue; }
            let EngineEvent::CommandCompleted { result, .. } = reply;
            if let Err(error) = result { self.error = error.message; }
        }
        self.export_dialog.sync(&self.client.export);
        if self.dialog.as_ref().is_some_and(|dialog| !dialog.is_open()) { self.dialog = None; }
        let snapshot = &self.client.snapshot;
        if snapshot.error_message != self.last_engine_error {
            self.error.clone_from(&snapshot.error_message);
            self.last_engine_error.clone_from(&snapshot.error_message);
        }
        let selected_path = usize::try_from(snapshot.current_file_index).ok()
            .and_then(|index| snapshot.file_list.get(index)).map(|row| row.path.clone()).unwrap_or_default();
        let selection = (snapshot.current_file_index, selected_path);
        if self.selection.as_ref() != Some(&selection) {
            self.scroll_target = usize::try_from(selection.0).ok();
            self.selection = Some(selection);
        }

        Ok(())
    }

    /// Draw one frame; suitable for both eframe and an isolated egui_kittest harness.
    pub fn show(&mut self, root: &mut Ui) {
        self.process_engine(root.ctx());
        let was_modal = self.is_modal();
        egui::CentralPanel::default().show(root, |ui| {
            ui.add_enabled_ui(!was_modal, |ui| self.show_content(ui));
        });
        if !was_modal && !self.is_modal() && !root.ctx().text_edit_focused() && !egui::Popup::is_any_open(root.ctx()) {
            let actions = root.ctx().input(|input| keyboard::read_actions(&input.events, input.modifiers, &self.client.snapshot.settings.keys));
            for action in actions {
                if self.is_modal() { break; }
                match action {
                    keyboard::FrontendAction::Command(command) => self.dispatch(command),
                    keyboard::FrontendAction::Open(kind) => self.open_dialog(kind, root.ctx()),
                    keyboard::FrontendAction::Export => self.start_export(),
                }
            }
        }
        // An external FIFO export may cover an edit form, but never destroys its draft.
        if !self.export_dialog.is_open() {
            if let Some(dialog) = &mut self.dialog {
                if let Some(command) = dialog.show(root.ctx()) {
                    match self.client.submit(command.clone()) {
                        Ok(request_id) => dialog.start_request(request_id, command),
                        Err(error) => dialog.fail_submission(format!("{error:#}")),
                    }
                    root.ctx().request_repaint();
                }
                if !dialog.is_open() {
                    self.dialog = None;
                    self.release_focus(root.ctx());
                }
            }
        }
        if self.export_dialog.show(root.ctx(), &self.client.export) {
            match self.client.handle.request_export_cancellation() {
                Ok(()) => self.dispatch(EngineCommand::CompileCancel),
                Err(error) => self.error = format!("{error:#}"),
            }
        }
    }

    /// Process acknowledgements and close requests even while the native window is hidden.
    pub fn process_engine(&mut self, context: &egui::Context) {
        if let Err(error) = self.poll() { self.error = format!("{error:#}"); }
        if self.client.snapshot.stopped {
            context.send_viewport_cmd(egui::ViewportCommand::Close);
        } else if context.input(|input| input.viewport().close_requested()) {
            context.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            if !self.dialog.as_ref().is_some_and(EditDialog::is_busy) && self.shutdown_request.is_none() {
                match self.client.submit(EngineCommand::Shutdown) {
                    Ok(request_id) => self.shutdown_request = Some(request_id),
                    Err(error) => self.error = format!("Cannot close: {error:#}"),
                }
            }
        }
    }

    fn is_modal(&self) -> bool {
        self.dialog.is_some() || self.export_dialog.is_open() || self.shutdown_request.is_some()
    }

    fn release_focus(&self, context: &egui::Context) {
        context.memory_mut(|memory| {
            if let Some(focused) = memory.focused() { memory.surrender_focus(focused); }
        });
        context.request_repaint();
    }

    fn open_dialog(&mut self, kind: DialogKind, context: &egui::Context) {
        if self.is_modal() { return; }
        self.dialog = EditDialog::open(kind, &self.client.snapshot);
        self.release_focus(context);
    }

    fn start_export(&mut self) {
        if self.is_modal() { return; }
        match self.client.submit(EngineCommand::Compile) {
            Ok(request_id) => self.export_dialog.start_request(request_id),
            Err(error) => self.error = format!("Cannot start export: {error:#}"),
        }
    }

    fn show_menu(&mut self, ui: &mut Ui, compact: bool) {
        let height = if compact { 32.0 } else { 42.0 };
        let (rect, _) = ui.allocate_exact_size(egui::vec2(ui.available_width(), height), egui::Sense::hover());
        let narrow = rect.width() < 620.0;
        let labels = if narrow { ["Project", "Markers", "TOC", "Compile", "F1"] }
            else { ["Project F5", "Markers F4", "TOC F10", "Compile F8", "Shortcuts F1"] };
        let padding = if rect.width() < 420.0 { 2.0 } else if narrow { 6.0 } else { 9.0 };
        let inset = if rect.width() < 420.0 { 4.0 } else { 10.0 };
        let spacing = if rect.width() < 420.0 { 4.0 } else { 5.0 };
        let font = egui::TextStyle::Button.resolve(ui.style());
        let menu_width = labels.iter().map(|label| {
            (ui.painter().layout_no_wrap((*label).into(), font.clone(), ui.visuals().text_color()).size().x
                + padding * 2.0).max(ui.spacing().interact_size.x)
        }).sum::<f32>() + (labels.len() - 1) as f32 * spacing + inset * 2.0;
        let menu_rect = egui::Rect::from_min_size(rect.min, egui::vec2(menu_width, height));
        ui.painter().rect_filled(menu_rect, 0.0, egui::Color32::from_gray(232));
        ui.scope_builder(egui::UiBuilder::new().max_rect(menu_rect.shrink2(egui::vec2(inset, 0.0)))
            .layout(egui::Layout::left_to_right(egui::Align::Center)), |ui| {
            ui.spacing_mut().item_spacing.x = spacing;
            ui.spacing_mut().button_padding.x = padding;
            for (index, kind, label) in [(0, DialogKind::Project, "Project F5"),
                (1, DialogKind::Markers, "Markers F4"), (2, DialogKind::Toc, "TOC F10")] {
                if ui.button(labels[index]).accessible_name(label).on_hover_text(label).clicked() {
                    self.open_dialog(kind, ui.ctx());
                }
            }
            if ui.button(labels[3]).accessible_name("Compile F8").on_hover_text("Compile F8").clicked() {
                self.start_export();
            }
            if ui.button(labels[4]).accessible_name("Shortcuts F1").on_hover_text("Shortcuts F1").clicked() {
                self.open_dialog(DialogKind::Shortcuts, ui.ctx());
            }
        });
        let modes = egui::Rect::from_min_size(egui::pos2(rect.right() - 84.0, rect.top()), egui::vec2(84.0, height));
        ui.painter().rect_filled(modes, 0.0, egui::Color32::from_gray(232));
        ui.scope_builder(egui::UiBuilder::new().max_rect(modes)
            .layout(egui::Layout::left_to_right(egui::Align::Center)), |ui| {
            ui.spacing_mut().item_spacing.x = 0.0;
            ui.spacing_mut().button_padding.x = 0.0;
            for (mode, description) in [("A", "Append recording"), ("U", "Replace selected recording"), ("I", "Insert after selected recording")] {
                let selected = self.client.snapshot.recording_mode == mode;
                let text = egui::RichText::new(format!("[{mode}]")).size(18.0)
                    .color(if selected { egui::Color32::from_rgb(204, 0, 0) } else { egui::Color32::BLACK });
                let response = ui.add_enabled(!self.client.snapshot.is_recording,
                    egui::Button::new(text).frame(false).min_size(egui::vec2(28.0, 32.0)))
                    .accessible_name(mode).on_hover_text(description);
                response.widget_info(|| egui::WidgetInfo::selected(egui::Role::Button,
                    ui.is_enabled() && !self.client.snapshot.is_recording, selected, mode));
                if response.clicked() {
                    let command = match mode {
                        "U" => Some(EngineCommand::ModeUpdate), "I" => Some(EngineCommand::ModeInsert),
                        _ => match self.client.snapshot.recording_mode.as_str() {
                            "U" => Some(EngineCommand::ModeUpdate), "I" => Some(EngineCommand::ModeInsert), _ => None,
                        },
                    };
                    if let Some(command) = command { self.dispatch(command); }
                }
            }
        });
    }

    fn show_content(&mut self, ui: &mut Ui) {
        let compact = ui.available_height() < 500.0;
        let bar_height = if compact { 24.0 } else { 34.0 };
        let gap = if compact { 3.0 } else { 8.0 };
        ui.spacing_mut().item_spacing.y = gap;
        egui::Frame::NONE.inner_margin(egui::Margin::symmetric(10, 0)).show(ui, |ui| {
            self.show_menu(ui, compact);
            if !self.error.is_empty() {
                ui.horizontal_wrapped(|ui| {
                    ui.colored_label(ui.visuals().error_fg_color, &self.error);
                    if ui.button("Dismiss error").clicked() { self.error.clear(); }
                });
            }
            let meta = &self.client.snapshot.meta;
            let author = if meta.year.is_empty() { meta.author.clone() } else { format!("{} {}", meta.year, meta.author) };
            let title = format!("{author} — {}", meta.title);
            show_text_bar(ui, &title, false, bar_height, egui::Color32::from_gray(245), egui::Color32::from_gray(51));
            show_text_bar(ui, &self.client.snapshot.section_titles, false, bar_height, egui::Color32::from_gray(240), egui::Color32::from_gray(102));
            ui.add_space(gap);
            self.show_graphs(ui, compact);
            ui.add_space(gap);
            let snapshot = &self.client.snapshot;
            let hint = |start: &str, end: &str| {
                if end.is_empty() { start.to_owned() } else { format!("{start} [...] {end}") }
            };
            show_text_bar(ui, &hint(&snapshot.hintbox_current_start, &snapshot.hintbox_current_end), true,
                bar_height, egui::Color32::from_gray(240), egui::Color32::from_gray(102));
            show_text_bar(ui, &hint(&snapshot.hintbox_prev_start, &snapshot.hintbox_prev_end), false,
                bar_height, egui::Color32::from_gray(240), egui::Color32::from_gray(102));
            ui.add_space(gap);
            self.show_search(ui);
            self.show_fragments(ui, bar_height);
        });
    }

    fn show_search(&mut self, ui: &mut Ui) {
        ui.horizontal(|ui| {
            let width = (ui.available_width() - 64.0 - ui.spacing().item_spacing.x * 2.0).max(32.0);
            let response = ui.add_sized([width, 32.0],
                egui::TextEdit::singleline(&mut self.search).id(egui::Id::unique("transcription-search")))
                .accessible_name("Search");
            if response.has_focus() && ui.input(|input| input.key_pressed(egui::Key::Escape)) {
                response.surrender_focus();
            }
            let previous = ui.add_sized([32.0, 32.0], egui::Button::new(egui::RichText::new("↑").size(20.0))).accessible_name("Previous match")
                .on_hover_text("Previous match");
            if previous.clicked() {
                self.dispatch(EngineCommand::SearchHintUp(self.search.clone()));
            }
            let next = ui.add_sized([32.0, 32.0], egui::Button::new(egui::RichText::new("↓").size(20.0))).accessible_name("Next match")
                .on_hover_text("Next match");
            if next.clicked() {
                self.dispatch(EngineCommand::SearchHintDown(self.search.clone()));
            }
        });
    }

    fn show_graphs(&self, ui: &mut Ui, compact: bool) {
        let snapshot = &self.client.snapshot;
        let height = if compact { 24.0 } else { (ui.ctx().content_rect().height() * 0.1).clamp(48.0, 90.0) };
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 8.0;
            let width = ui.available_width();
            let previous_width = (width * 0.25).max(40.0);
            show_waveform(ui, "Previous waveform", &snapshot.waveform_prev, snapshot.waveform_prev_loading,
                0.0, false, egui::vec2(previous_width, height));
            let current_width = (width - previous_width - 50.0 - 16.0).max(40.0);
            show_waveform(ui, "Current waveform", &snapshot.waveform_current, snapshot.waveform_current_loading,
                snapshot.playback_position, snapshot.is_recording, egui::vec2(current_width, height));
            show_level(ui, snapshot.level, egui::vec2(50.0, height));
        });
    }

    fn show_fragments(&mut self, ui: &mut Ui, summary_height: f32) {
        let total = self.client.snapshot.file_list.len();
        let mut selected = None;
        let mut edit_index = None;
        let inset = if summary_height < 32.0 { 4.0 } else { 8.0 };
        ui.scope(|ui| {
            ui.painter().rect(ui.available_rect_before_wrap(), 0.0, egui::Color32::from_gray(245),
                egui::Stroke::new(1.0, egui::Color32::from_gray(204)), egui::StrokeKind::Inside);
            let visible_height = (ui.available_height() - summary_height - inset).max(0.0);
            let mut scroll = egui::ScrollArea::vertical().id_salt("fragments").auto_shrink([false, false])
                .min_scrolled_height(0.0).max_height(visible_height);
            if let Some(index) = self.scroll_target.take() {
                let offset = index as f32 * ROW_HEIGHT;
                let state = egui::scroll_area::State::load(ui.ctx(), ui.make_persistent_id("fragments"));
                let current_offset = state.map(|state| state.clamped_offset().y).unwrap_or(0.0);
                if offset < current_offset || offset + ROW_HEIGHT > current_offset + visible_height {
                    let content_height = total as f32 * ROW_HEIGHT;
                    scroll = scroll.vertical_scroll_offset(offset.min((content_height - visible_height).max(0.0)));
                }
            }
            ui.spacing_mut().item_spacing.y = 0.0;
            let output = scroll.show_rows(ui, ROW_HEIGHT, total, |ui, visible| {
                for index in visible {
                    let row = &self.client.snapshot.file_list[index];
                    let response = show_fragment(ui, row, self.client.snapshot.current_file_index == index as i32, ROW_HEIGHT);
                    if response.double_clicked() && !row.is_recording { edit_index = Some(index); }
                    if response.clicked() && !row.is_recording { selected = Some(ChunkId(row.path.clone())); }
                }
            });
            ui.add_space(inset);
            show_summary(ui, &self.client.snapshot, output.inner_rect.width(), summary_height);
        });
        if let Some(chunk) = selected { self.dispatch(EngineCommand::Select { chunk: Some(chunk), play: false }); }
        if let Some(index) = edit_index {
            // Capture the clicked row, not the engine's previous asynchronous selection.
            let previous = self.client.snapshot.current_file_index;
            self.client.snapshot.current_file_index = index as i32;
            self.open_dialog(DialogKind::Chunk, ui.ctx());
            self.client.snapshot.current_file_index = previous;
        }
    }
}

impl eframe::App for EguiApp {
    fn logic(&mut self, context: &egui::Context, _frame: &mut eframe::Frame) {
        self.process_engine(context);
    }
    fn ui(&mut self, ui: &mut Ui, _frame: &mut eframe::Frame) {
        self.show(ui);
    }
}
