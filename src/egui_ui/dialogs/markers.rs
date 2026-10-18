use super::fields::show_input;
use eframe::egui::{self, Ui};
use iamreader_engine::engine::CommandFailure;
use iamreader_engine::project::{MarkerSettings, MarkerSettingsData};
use std::collections::{BTreeMap, HashMap};

pub(super) struct MarkerDrafts {
    pub drafts: BTreeMap<String, MarkerSettingsData>,
    pub saved: HashMap<String, MarkerSettings>,
    pub selected: String,
    new_alias: String,
}

impl MarkerDrafts {
    pub fn new(markers: &HashMap<String, MarkerSettings>) -> Self {
        let drafts: BTreeMap<_, _> = markers.iter().map(|(alias, marker)| (alias.clone(), create_form(alias, marker))).collect();
        let selected = drafts.keys().next().cloned().unwrap_or_default();

        Self { drafts, saved: markers.clone(), selected, new_alias: String::new() }
    }

    pub fn show_selector(&mut self, ui: &mut Ui, error: &mut Option<CommandFailure>) {
        let label = ui.label("Marker");
        let response = egui::ComboBox::from_id_salt("marker-selector").selected_text(&self.selected).show_ui(ui, |ui| {
            for alias in self.drafts.keys() {
                if ui.selectable_value(&mut self.selected, alias.clone(), alias).clicked() { *error = None; }
            }
        });
        response.response.labelled_by(label.id);
    }

    pub fn show(&mut self, ui: &mut Ui, error: &mut Option<CommandFailure>, status: &mut String) {
        show_input(ui, "New marker alias", "marker", &mut self.new_alias, false, error.as_ref());
        if ui.button("Add").clicked() {
            let alias = self.new_alias.trim();
            if alias.is_empty() || self.drafts.contains_key(alias) {
                *error = Some(CommandFailure {
                    message: if alias.is_empty() { "Enter a marker alias." } else { "This alias already exists." }.into(),
                    field: Some("marker".into()),
                });
            } else {
                self.drafts.insert(alias.into(), create_form(alias, &MarkerSettings::default()));
                self.selected = alias.into();
                self.new_alias.clear();
                *error = None;
                *status = "New marker — not saved yet.".into();
            }
        }
        ui.separator();
        let Some(form) = self.drafts.get_mut(&self.selected) else { return; };
        show_input(ui, "Title", "title", &mut form.title, false, error.as_ref());
        show_input(ui, "Hint", "hint", &mut form.hint, true, error.as_ref());
        show_input(ui, "Shortcut (0–9)", "shortcut", &mut form.shortcut, false, error.as_ref());
        ui.checkbox(&mut form.section, "Starts a section");
        ui.separator();
        if ui.available_width() >= 600.0 {
            ui.columns(2, |columns| {
                show_asset_editor(&mut columns[0], form, true, error.as_ref());
                show_asset_editor(&mut columns[1], form, false, error.as_ref());
            });
        } else {
            show_asset_editor(ui, form, true, error.as_ref());
            ui.separator();
            show_asset_editor(ui, form, false, error.as_ref());
        }
    }
}

fn show_asset_editor(ui: &mut Ui, form: &mut MarkerSettingsData, begin: bool, error: Option<&CommandFailure>) {
    let (label, fields, values) = if begin {
        ("Begin sound", ["begin_audio", "begin_kind", "begin_reduction", "begin_repeat"],
            [&mut form.begin_audio, &mut form.begin_kind, &mut form.begin_reduction, &mut form.begin_repeat])
    } else {
        ("End sound", ["end_audio", "end_kind", "end_reduction", "end_repeat"],
            [&mut form.end_audio, &mut form.end_kind, &mut form.end_reduction, &mut form.end_repeat])
    };
    show_asset(ui, label, fields, values, error);
}

fn show_asset(ui: &mut Ui, label: &str, fields: [&str; 4], values: [&mut String; 4], error: Option<&CommandFailure>) {
    ui.strong(label);
    for ((suffix, field), value) in ["audio", "kind", "reduction", "repeat"].into_iter().zip(fields).zip(values) {
        show_input(ui, &format!("{label} {suffix}"), field, value, false, error);
    }
    ui.small("Kind: add / underlay. Reduction: 0–100%. Repeat: -1 loops an underlay.");
}

fn create_form(alias: &str, marker: &MarkerSettings) -> MarkerSettingsData {
    MarkerSettingsData {
        marker: alias.into(), title: marker.title.clone(), hint: marker.hint.clone(),
        shortcut: marker.shortcut.clone().unwrap_or_default(), section: marker.section,
        begin_audio: marker.assets.begin.audio.clone(), begin_kind: marker.assets.begin.kind.clone(),
        begin_reduction: marker.assets.begin.reduction.map(|value| value.to_string()).unwrap_or_default(),
        begin_repeat: marker.assets.begin.repeat.map(|value| value.to_string()).unwrap_or_default(),
        end_audio: marker.assets.end.audio.clone(), end_kind: marker.assets.end.kind.clone(),
        end_reduction: marker.assets.end.reduction.map(|value| value.to_string()).unwrap_or_default(),
        end_repeat: marker.assets.end.repeat.map(|value| value.to_string()).unwrap_or_default(),
    }
}
