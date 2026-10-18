use eframe::egui::{self, Color32, FontId, Rect, Sense, Stroke, StrokeKind, Ui, Vec2};

pub(super) fn show_text_bar(ui: &mut Ui, text: &str, bold: bool, height: f32, fill: Color32, color: Color32) {
    let (rect, response) = ui.allocate_exact_size(Vec2::new(ui.available_width(), height), Sense::hover());
    ui.painter().rect(rect, 0.0, fill, Stroke::new(1.0, Color32::from_gray(204)), StrokeKind::Inside);
    let content = egui::RichText::new(text).color(color);
    let content = if bold { content.family(egui::FontFamily::Name("Ubuntu Bold".into())) } else { content };
    ui.put(rect.shrink2(egui::vec2(10.0, 0.0)), egui::Label::new(content).truncate().selectable(false));
    response.on_hover_text(text);
}

pub(super) fn show_waveform(ui: &mut Ui, label: &str, samples: &[f32], loading: bool, position: f32, recording: bool, size: Vec2) {
    let (rect, response) = ui.allocate_exact_size(size, Sense::hover());
    response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Image, true, label));
    let painter = ui.painter_at(rect);
    painter.rect(rect, 0.0, Color32::from_gray(245), Stroke::new(1.0, Color32::from_gray(204)), StrokeKind::Inside);
    let rect = rect.shrink(1.0);
    let slots = if recording { 1500 } else { samples.len().max(1) };
    // One peak per physical pixel at most, even on a narrow or HiDPI display.
    let columns = (rect.width() * ui.ctx().pixels_per_point()).max(1.0) as usize;
    let step = slots.div_ceil(columns).max(1);
    for (index, peak) in samples.chunks(step).enumerate() {
        let amplitude = peak.iter().copied().filter(|sample| sample.is_finite()).fold(0.0_f32, f32::max).clamp(0.0, 1.0);
        let horizontal = rect.left() + (index * step) as f32 / slots as f32 * rect.width();
        let half_height = amplitude * rect.height() * 0.5;
        painter.line_segment([
            egui::pos2(horizontal, rect.center().y - half_height),
            egui::pos2(horizontal, rect.center().y + half_height),
        ], Stroke::new(1.0, Color32::from_rgb(173, 216, 230)));
    }
    if position.is_finite() && position > 0.0 {
        let horizontal = rect.left() + position.clamp(0.0, 1.0) * rect.width();
        painter.line_segment([egui::pos2(horizontal, rect.top()), egui::pos2(horizontal, rect.bottom())], Stroke::new(2.0, Color32::RED));
    }
    if loading && samples.is_empty() {
        painter.text(rect.center(), egui::Align2::CENTER_CENTER, "Loading waveform…",
            egui::TextStyle::Body.resolve(ui.style()), ui.visuals().text_color());
    }
}

pub(super) fn show_level(ui: &mut Ui, level: f32, size: Vec2) {
    let (rect, response) = ui.allocate_exact_size(size, Sense::hover());
    response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Image, ui.is_enabled(), "Input level (dB FS)"));
    let painter = ui.painter_at(rect);
    painter.rect(rect, 0.0, Color32::from_gray(245), Stroke::new(1.0, Color32::from_gray(204)), StrokeKind::Inside);
    let scale = rect.shrink2(egui::vec2(3.0, 2.0));
    let font = FontId::monospace(egui::TextStyle::Small.resolve(ui.style()).size);
    let label_height = ui.fonts_mut(|fonts| fonts.row_height(&font));
    let bottom_label_top = (scale.bottom() - label_height).max(scale.top());
    // Adjacent dB labels collapse on short windows instead of overlapping.
    let mut previous_bottom = f32::NEG_INFINITY;
    for decibels in [0, -6, -12, -20, -40, -60] {
        let top = (scale.top() + -decibels as f32 / 60.0 * scale.height() - label_height * 0.5)
            .clamp(scale.top(), bottom_label_top);
        if top < previous_bottom { continue; }
        if decibels != 0 && decibels != -60 && top + label_height > bottom_label_top { continue; }
        painter.text(egui::pos2(scale.left() + 25.0, top), egui::Align2::RIGHT_TOP,
            decibels.to_string(), font.clone(), Color32::from_gray(80));
        previous_bottom = top + label_height;
    }
    let meter = Rect::from_min_max(egui::pos2(scale.left() + 30.0, scale.top()), scale.right_bottom());
    let level = if level.is_finite() { level.clamp(0.0, 1.0) } else { 0.0 };
    let filled = Rect::from_min_max(egui::pos2(meter.left(), meter.bottom() - meter.height() * level), meter.right_bottom());
    let strip_height = 1.0 / ui.ctx().pixels_per_point();
    let strips = (filled.height() / strip_height).ceil() as usize;
    for index in 0..strips {
        let top = filled.top() + index as f32 * strip_height;
        let fraction = (top - meter.top()) / meter.height();
        let color = if fraction < 0.45 {
            Color32::from_rgb(204 + (51.0 * fraction / 0.45) as u8, (204.0 * fraction / 0.45) as u8, 0)
        } else {
            let fraction = (fraction - 0.45) / 0.55;
            Color32::from_rgb((255.0 * (1.0 - fraction)) as u8, (204.0 - 34.0 * fraction) as u8, 0)
        };
        painter.rect_filled(Rect::from_min_max(egui::pos2(meter.left(), top),
            egui::pos2(meter.right(), (top + strip_height).min(filled.bottom()))), 0.0, color);
    }
}
