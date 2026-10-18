use super::fields::show_input;
use eframe::egui::Ui;
use iamreader_engine::engine::CommandFailure;
use iamreader_engine::project::{ChunkSettingsData, MetaData};

pub(super) struct ProjectDraft {
    pub data: MetaData,
    export_tab: bool,
}

impl ProjectDraft {
    pub fn new(data: MetaData) -> Self { Self { data, export_tab: false } }

    pub fn show_tabs(&mut self, ui: &mut Ui) {
        ui.horizontal(|ui| {
            ui.selectable_value(&mut self.export_tab, false, "Book");
            ui.selectable_value(&mut self.export_tab, true, "Export");
        });
    }

    pub fn show(&mut self, ui: &mut Ui, error: Option<&CommandFailure>) {
        let data = &mut self.data;
        if self.export_tab {
            ui.label("Export format");
            ui.horizontal(|ui| {
                ui.radio_value(&mut data.format_audio, "wav".into(), "WAV (RF64)");
                ui.radio_value(&mut data.format_audio, "mp3".into(), "MP3");
            });
            if let Some(error) = error.filter(|error| error.field.as_deref() == Some("format_audio")) {
                ui.colored_label(ui.visuals().error_fg_color, &error.message);
            }
            show_input(ui, "Cover image", "cover", &mut data.cover, false, error);
            ui.checkbox(&mut data.normalize, "Normalize");
            ui.checkbox(&mut data.denoise, "Denoise");
            ui.checkbox(&mut data.section_split, "Split by sections");
        } else {
            show_input(ui, "Book title", "title", &mut data.title, false, error);
            show_input(ui, "Author", "author", &mut data.author, false, error);
            show_input(ui, "Year", "year", &mut data.year, false, error);
            show_input(ui, "Reader", "reader", &mut data.reader, false, error);
            show_input(ui, "Hint", "hint", &mut data.hint, true, error);
        }
    }
}

pub(super) fn show_chunk(ui: &mut Ui, data: &mut ChunkSettingsData, error: Option<&CommandFailure>) {
    show_input(ui, "Title", "title", &mut data.title, false, error);
    show_input(ui, "Author", "author", &mut data.author, false, error);
    show_input(ui, "Year", "year", &mut data.year, false, error);
    show_input(ui, "Hint", "hint", &mut data.hint, true, error);
}
