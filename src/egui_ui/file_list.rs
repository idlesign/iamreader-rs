use eframe::egui::{self, Align, Color32, FontId, Rect, Response, Sense, Stroke, StrokeKind, TextStyle, Ui};
use iamreader_engine::engine::EngineSnapshot;
use iamreader_engine::engine::snapshot::FragmentRow;

struct FileColumns {
    cells: [Rect; 8],
}

impl FileColumns {
    fn new(rect: Rect) -> Self {
        let compact = rect.width() < 600.0;
        let inset = if compact { 6.0 } else { 10.0 };
        let number = if compact { 30.0 } else { 50.0 };
        let status = if compact { 18.0 } else { 24.0 };
        let available = (rect.width() - inset * 2.0 - number - status).max(0.0);
        let markers = if compact { 0.0 } else { (available * 10.0 / 88.0).max(60.0) };
        let duration = (available * 10.0 / 88.0).max(if compact { 52.0 } else { 64.0 });
        let start_time = duration;
        let name = (available * 14.0 / 88.0).max(if compact { 92.0 } else { 104.0 });
        let size = (available * 6.0 / 88.0).max(if compact { 64.0 } else { 80.0 });
        let title = (available - markers - duration - start_time - name - size).max(0.0);
        let mut left = rect.left() + inset;
        let cells = [number, title, markers, duration, start_time, name, size, status].map(|width| {
            let cell = Rect::from_min_size(egui::pos2(left, rect.top()), egui::vec2(width, rect.height()));
            left += width;
            cell
        });

        Self { cells }
    }
}

fn paint_cell(ui: &Ui, rect: Rect, text: &str, font: FontId, align: Align, color: Color32) {
    if rect.width() <= 8.0 || text.is_empty() { return; }
    let rect = rect.shrink2(egui::vec2(4.0, 0.0));
    let mut job = egui::text::LayoutJob::simple_singleline(text.to_owned(), font, color);
    job.wrap.max_width = rect.width();
    job.wrap.max_rows = 1;
    let galley = ui.fonts_mut(|fonts| fonts.layout_job(job));
    let left = if align == Align::RIGHT { rect.right() - galley.size().x } else { rect.left() };
    ui.painter().with_clip_rect(rect.intersect(ui.clip_rect())).galley(
        egui::pos2(left, rect.center().y - galley.size().y * 0.5), galley, color);
}

pub(super) fn show_fragment(ui: &mut Ui, row: &FragmentRow, selected: bool, height: f32) -> Response {
    let (rect, response) = ui.allocate_exact_size(egui::vec2(ui.available_width(), height), Sense::click());
    let fill = if selected { ui.visuals().selection.bg_fill }
        else if response.hovered() { Color32::from_rgb(233, 240, 246) }
        else { Color32::from_gray(245) };
    let border = if response.has_focus() { ui.visuals().selection.stroke } else { Stroke::new(1.0, Color32::from_gray(204)) };
    ui.painter().rect(rect, 0.0, fill, border, StrokeKind::Inside);
    let columns = FileColumns::new(rect);
    let body = TextStyle::Body.resolve(ui.style());
    let monospace = TextStyle::Monospace.resolve(ui.style());
    for (index, text, font, align, color) in [
        (0, format!("{}.", row.index), monospace.clone(), Align::LEFT, Color32::from_gray(51)),
        (1, row.title.clone(), body.clone(), Align::LEFT, Color32::from_gray(51)),
        (2, row.markers.clone(), TextStyle::Small.resolve(ui.style()), Align::LEFT, Color32::from_gray(102)),
        (3, row.duration.clone(), monospace.clone(), Align::RIGHT, Color32::from_gray(51)),
        (4, row.start_time.clone(), monospace.clone(), Align::RIGHT, Color32::from_gray(51)),
        (5, row.name.clone(), body, Align::LEFT, Color32::from_gray(51)),
        (6, row.size.clone(), monospace, Align::RIGHT, Color32::from_gray(51)),
    ] {
        paint_cell(ui, columns.cells[index], &text, font, align, color);
    }
    let indicator = columns.cells[7];
    if row.is_recording {
        ui.painter().circle_filled(indicator.center() - egui::vec2(4.0, 0.0), 3.5, Color32::RED);
    }
    if row.is_playing {
        ui.painter().rect_filled(Rect::from_center_size(indicator.center() + egui::vec2(4.0, 0.0), egui::vec2(7.0, 7.0)),
            1.0, Color32::from_rgb(0, 170, 0));
    }
    let title = if row.title.is_empty() { &row.name } else { &row.title };
    let state = if row.is_recording { "REC" } else if row.is_playing { "PLAY" } else { "" };
    let label = format!("{} · {} · {} · {} {} · {} · {} · {}", row.index, title, row.duration, row.markers,
        state, row.start_time, row.name, row.size);
    response.widget_info(|| egui::WidgetInfo::selected(egui::Role::Button, ui.is_enabled(), selected, &label));

    response.on_hover_text(&row.hint)
}

pub(super) fn show_summary(ui: &mut Ui, snapshot: &EngineSnapshot, width: f32, height: f32) {
    let (rect, response) = ui.allocate_exact_size(egui::vec2(ui.available_width(), height), Sense::hover());
    ui.painter().rect(rect, 0.0, Color32::from_gray(232), Stroke::new(1.0, Color32::from_gray(170)), StrokeKind::Inside);
    let columns = FileColumns::new(Rect::from_min_size(rect.min, egui::vec2(width, height)));
    let time = columns.cells[3].union(columns.cells[4]);
    let size = columns.cells[5].union(columns.cells[6]).union(columns.cells[7]);
    for (cell, text, align) in [
        (columns.cells[0].union(columns.cells[1]), &snapshot.total_files, Align::LEFT),
        (time, &snapshot.total_duration, Align::RIGHT),
        (size, &snapshot.total_size, Align::RIGHT),
    ] {
        paint_cell(ui, cell, text, TextStyle::Monospace.resolve(ui.style()), align, Color32::from_gray(51));
    }
    response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Label, ui.is_enabled(),
        format!("{} · {} · {}", snapshot.total_files, snapshot.total_duration, snapshot.total_size)));
}
