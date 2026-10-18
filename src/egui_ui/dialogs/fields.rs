use eframe::egui::{self, Ui};
use iamreader_engine::engine::CommandFailure;

pub(super) fn settle_modal_layout(context: &egui::Context, size: egui::Vec2, previous: &mut Option<egui::Vec2>) {
    // Areas use the last frame's size to lay out their children. Finish that layout
    // before accepting another click; dynamic feedback must not leave stale hit targets.
    if previous.is_none_or(|previous| (previous - size).length_sq() > 0.01) {
        *previous = Some(size);
        context.request_repaint();
    }
}

pub(super) fn show_input(ui: &mut Ui, label: &str, field: &str, value: &mut String, multiline: bool, error: Option<&CommandFailure>) {
    let inline_error = error.filter(|error| error.field.as_deref() == Some(field));
    let label = ui.label(label);
    let mut edit = if multiline {
        egui::TextEdit::multiline(value).desired_rows(3)
    } else {
        egui::TextEdit::singleline(value)
    }.id(egui::Id::unique(("dialog-field", field))).desired_width(ui.available_width());
    if inline_error.is_some() {
        edit = edit.frame(egui::Frame::new().fill(ui.visuals().extreme_bg_color)
            .stroke(egui::Stroke::new(1.0, ui.visuals().error_fg_color)).inner_margin(4.0));
    }
    ui.add(edit).labelled_by(label.id);
    if let Some(error) = inline_error {
        ui.colored_label(ui.visuals().error_fg_color, &error.message);
    }
}

pub(super) fn show_error(ui: &mut Ui, error: Option<&CommandFailure>) {
    if let Some(error) = error {
        let message = error.field.as_ref().map(|field| format!("Please check {}.", field.replace('_', " ")))
            .unwrap_or_else(|| error.message.clone());
        ui.colored_label(ui.visuals().error_fg_color, message);
    }
}
