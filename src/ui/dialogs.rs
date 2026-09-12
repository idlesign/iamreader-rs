//! GUI-owned drafts and acknowledged saves. No filesystem or audio work runs here.
use super::ui::{MainWindow, UIState};
use crate::project::dialog_edit::{validate_marker_form, validate_marker_forms, FieldError};
use crate::project::{ChunkSettingsData, MarkerSettings, MarkerSettingsData, MetaData};
use crate::utils::keyboard::{Action, DialogEdit};
use crossbeam_channel::Sender;
use slint::{ComponentHandle, Model};
use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap};
use std::rc::Rc;
use std::sync::{Arc, Mutex};

#[derive(Debug)]
pub struct DialogReply {
    pub request_id: u64,
    pub result: Result<Option<String>, String>,
    pub field: Option<String>,
}

pub struct DialogController {
    state: Arc<Mutex<UIState>>,
    actions: Sender<Action>,
    next_request: u64,
    pending: Option<(u64, DialogEdit)>,
    chunk_path: String,
    markers_open: bool,
    drafts: BTreeMap<String, MarkerSettingsData>,
    saved_markers: HashMap<String, MarkerSettings>,
}

impl DialogController {
    pub fn install(
        window: &MainWindow,
        state: Arc<Mutex<UIState>>,
        actions: Sender<Action>,
    ) -> Rc<RefCell<Self>> {
        let controller = Rc::new(RefCell::new(Self {
            state,
            actions,
            next_request: 0,
            pending: None,
            chunk_path: String::new(),
            markers_open: false,
            drafts: BTreeMap::new(),
            saved_markers: HashMap::new(),
        }));
        let weak = window.as_weak();
        window.window().on_close_requested(move || {
            if weak
                .upgrade()
                .is_some_and(|window| window.get_dialog_busy())
            {
                slint::CloseRequestResponse::KeepWindowShown
            } else {
                slint::CloseRequestResponse::HideWindow
            }
        });
        let weak = window.as_weak();
        let ctl = controller.clone();
        window.on_dialog_project_save(
            move |title,
                  author,
                  year,
                  hint,
                  reader,
                  format_audio,
                  normalize,
                  cover,
                  section_split,
                  denoise| {
                if let Some(window) = weak.upgrade() {
                    if !window.get_dialog_project_open() {
                        return;
                    }
                    ctl.borrow_mut().submit(
                        &window,
                        DialogEdit::Meta(MetaData {
                            title: title.into(),
                            author: author.into(),
                            year: year.into(),
                            hint: hint.into(),
                            reader: reader.into(),
                            format_audio: format_audio.into(),
                            normalize,
                            cover: cover.into(),
                            section_split,
                            denoise,
                        }),
                    );
                }
            },
        );
        let weak = window.as_weak();
        window.on_dialog_project_cancel(move || {
            if let Some(window) = weak.upgrade() {
                if !window.get_dialog_busy() {
                    window.set_dialog_project_open(false);
                }
            }
        });
        let weak = window.as_weak();
        let ctl = controller.clone();
        window.on_dialog_chunk_save(move |ui_index, title, author, year, hint| {
            if let Some(window) = weak.upgrade() {
                if !window.get_dialog_chunk_open() {
                    return;
                }
                let mut ctl = ctl.borrow_mut();
                let expected_path = ctl.chunk_path.clone();
                ctl.submit(
                    &window,
                    DialogEdit::Chunk {
                        data: ChunkSettingsData {
                            ui_index,
                            title: title.into(),
                            author: author.into(),
                            year: year.into(),
                            hint: hint.into(),
                        },
                        expected_path,
                    },
                );
            }
        });
        let weak = window.as_weak();
        window.on_dialog_chunk_cancel(move || {
            if let Some(window) = weak.upgrade() {
                if !window.get_dialog_busy() {
                    window.set_dialog_chunk_open(false);
                }
            }
        });
        let weak = window.as_weak();
        let ctl = controller.clone();
        window.on_dialog_markers_marker_selected(move |index| {
            if let Some(window) = weak.upgrade() {
                if !window.get_dialog_busy() {
                    ctl.borrow_mut().select_marker(&window, index);
                }
            }
        });
        let weak = window.as_weak();
        let ctl = controller.clone();
        window.on_dialog_markers_add_marker(move |alias| {
            if let Some(window) = weak.upgrade() {
                if !window.get_dialog_markers_open() || window.get_dialog_busy() {
                    return;
                }
                let mut ctl = ctl.borrow_mut();
                clear_marker_errors(&window);
                let alias = alias.trim();
                if alias.is_empty() || ctl.drafts.contains_key(alias) {
                    let error = if alias.is_empty() {
                        "Enter a marker alias."
                    } else {
                        "This alias already exists."
                    };
                    window.set_dialog_markers_alias_error(error.into());
                    return;
                }
                ctl.capture_marker(&window);
                let form = marker_form(alias, &MarkerSettings::default());
                ctl.drafts.insert(alias.to_owned(), form.clone());
                ctl.set_marker_list(&window);
                let index = ctl.drafts.keys().position(|key| key == alias).unwrap();
                window.set_dialog_markers_selected_index(index as i32);
                display_marker(&window, &form);
                window.set_dialog_markers_new_alias("".into());
                window.set_dialog_markers_status_message("New marker — not saved yet.".into());
            }
        });
        let weak = window.as_weak();
        let ctl = controller.clone();
        window.on_dialog_markers_save(
            move |marker,
                  title,
                  hint,
                  shortcut,
                  begin_audio,
                  begin_kind,
                  begin_reduction,
                  begin_repeat,
                  end_audio,
                  end_kind,
                  end_reduction,
                  end_repeat,
                  section| {
                if let Some(window) = weak.upgrade() {
                    if !window.get_dialog_markers_open() || window.get_dialog_busy() {
                        return;
                    }
                    let form = MarkerSettingsData {
                        marker: marker.into(),
                        title: title.into(),
                        hint: hint.into(),
                        shortcut: shortcut.into(),
                        begin_audio: begin_audio.into(),
                        begin_kind: begin_kind.into(),
                        begin_reduction: begin_reduction.into(),
                        begin_repeat: begin_repeat.into(),
                        end_audio: end_audio.into(),
                        end_kind: end_kind.into(),
                        end_reduction: end_reduction.into(),
                        end_repeat: end_repeat.into(),
                        section,
                    };
                    let mut ctl = ctl.borrow_mut();
                    clear_marker_errors(&window);
                    window.set_dialog_markers_status_message("".into());
                    // Only the selected form is validated/saved. Other drafts stay local.
                    if let Err(error) =
                        validate_marker_forms(&ctl.saved_markers, std::slice::from_ref(&form))
                    {
                        let field = error.downcast_ref::<FieldError>().map(|error| error.field);
                        show_error(&window, &error.to_string(), field);
                        return;
                    }
                    ctl.drafts.insert(form.marker.clone(), form.clone());
                    ctl.submit(&window, DialogEdit::Marker(form));
                }
            },
        );
        let weak = window.as_weak();
        let ctl = controller.clone();
        window.on_dialog_markers_update_meta(move || {
            if let Some(window) = weak.upgrade() {
                if window.get_dialog_markers_open() && !window.get_dialog_busy() {
                    ctl.borrow_mut()
                        .submit(&window, DialogEdit::RefreshMetadata);
                }
            }
        });
        let weak = window.as_weak();
        let ctl = controller.clone();
        window.on_dialog_markers_cancel(move || {
            if let Some(window) = weak.upgrade() {
                if window.get_dialog_busy() {
                    return;
                }
                let mut ctl = ctl.borrow_mut();
                ctl.markers_open = false;
                ctl.drafts.clear();
                ctl.state.lock().unwrap().dialog_markers_open = false;
                window.set_dialog_markers_open(false);
            }
        });
        controller
    }

    pub fn open_chunk(&mut self, window: &MainWindow, path: String) {
        self.chunk_path = path;
        window.set_dialog_error_message("".into());
    }

    pub fn sync(&mut self, window: &MainWindow) {
        let (open, snapshot, reply) = {
            let mut state = self.state.lock().unwrap();
            let open = state.dialog_markers_open;
            let snapshot =
                (open && !self.markers_open).then(|| state.dialog_marker_snapshot.clone());
            (open, snapshot, state.dialog_save_result.take())
        };
        if let Some(snapshot) = snapshot {
            self.saved_markers = snapshot;
            self.drafts = self
                .saved_markers
                .iter()
                .map(|(alias, settings)| (alias.clone(), marker_form(alias, settings)))
                .collect();
            self.markers_open = true;
            self.set_marker_list(window);
            let selected = if self.drafts.is_empty() { -1 } else { 0 };
            window.set_dialog_markers_selected_index(selected);
            if let Some(form) = self.drafts.values().next() {
                display_marker(window, form);
            }
            clear_marker_errors(window);
            window.set_dialog_markers_new_alias("".into());
            window.set_dialog_markers_status_message(
                "Save applies to the selected marker only.".into(),
            );
            window.set_dialog_markers_open(true);
        } else if !open && self.markers_open {
            self.markers_open = false;
            self.drafts.clear();
            window.set_dialog_markers_open(false);
        }
        let Some(reply) = reply else {
            return;
        };
        if self.pending.as_ref().map(|(id, _)| *id) != Some(reply.request_id) {
            return;
        }
        let (_, edit) = self.pending.take().unwrap();
        window.set_dialog_busy(false);
        match reply.result {
            Err(error) => show_error(window, &error, reply.field.as_deref()),
            Ok(warning) => {
                match edit {
                    DialogEdit::Meta(_) => {
                        let state = self.state.lock().unwrap();
                        window.set_meta_title(state.dialog_project_title.clone().into());
                        window.set_meta_author(state.dialog_project_author.clone().into());
                        window.set_meta_year(state.dialog_project_year.clone().into());
                        window.set_meta_hint(state.dialog_project_hint.clone().into());
                        window.set_meta_reader(state.dialog_project_reader.clone().into());
                        if warning.is_none() {
                            window.set_dialog_project_open(false);
                        }
                    }
                    DialogEdit::Chunk { .. } => {
                        if warning.is_none() {
                            window.set_dialog_chunk_open(false);
                        }
                    }
                    DialogEdit::Marker(form) => {
                        if let Ok(settings) =
                            validate_marker_form(&form, self.saved_markers.get(&form.marker))
                        {
                            self.saved_markers.insert(form.marker.clone(), settings);
                        }
                        window.set_dialog_markers_status_message(
                            format!("Saved: {}", form.marker).into(),
                        );
                    }
                    DialogEdit::RefreshMetadata => {
                        window.set_dialog_markers_status_message("File metadata refreshed.".into())
                    }
                }
                if let Some(warning) = warning {
                    show_error(window, &format!("Saved, but {warning}"), None);
                }
            }
        }
    }

    fn submit(&mut self, window: &MainWindow, edit: DialogEdit) {
        if self.pending.is_some() {
            return;
        }
        clear_marker_errors(window);
        window.set_dialog_markers_status_message("".into());
        self.next_request += 1;
        let request_id = self.next_request;
        if let Err(error) = self.actions.try_send(Action::SaveDialog {
            request_id,
            edit: Box::new(edit.clone()),
        }) {
            show_error(
                window,
                &format!("Cannot submit changes: {error}. Your edits are still here."),
                None,
            );
            return;
        }
        self.pending = Some((request_id, edit));
        window.set_dialog_busy(true);
    }

    fn capture_marker(&mut self, window: &MainWindow) {
        let Some(alias) = window
            .get_dialog_markers_list()
            .row_data(window.get_dialog_markers_selected_index() as usize)
        else {
            return;
        };
        self.drafts.insert(
            alias.to_string(),
            MarkerSettingsData {
                marker: alias.into(),
                title: window.get_dialog_markers_title().into(),
                hint: window.get_dialog_markers_hint().into(),
                shortcut: window.get_dialog_markers_shortcut().into(),
                section: window.get_dialog_markers_section(),
                begin_audio: window.get_dialog_markers_begin_audio().into(),
                begin_kind: window.get_dialog_markers_begin_kind().into(),
                begin_reduction: window.get_dialog_markers_begin_reduction().into(),
                begin_repeat: window.get_dialog_markers_begin_repeat().into(),
                end_audio: window.get_dialog_markers_end_audio().into(),
                end_kind: window.get_dialog_markers_end_kind().into(),
                end_reduction: window.get_dialog_markers_end_reduction().into(),
                end_repeat: window.get_dialog_markers_end_repeat().into(),
            },
        );
    }

    fn select_marker(&mut self, window: &MainWindow, index: i32) {
        let Some(form) = usize::try_from(index)
            .ok()
            .and_then(|index| self.drafts.values().nth(index))
            .cloned()
        else {
            return;
        };
        self.capture_marker(window);
        // Capture may update this same form; selecting the current item must not reset it.
        let form = self.drafts.get(&form.marker).unwrap();
        window.set_dialog_markers_selected_index(index);
        display_marker(window, form);
        clear_marker_errors(window);
        window
            .set_dialog_markers_status_message("Save applies to the selected marker only.".into());
    }

    fn set_marker_list(&self, window: &MainWindow) {
        window.set_dialog_markers_list(slint::ModelRc::new(slint::VecModel::from(
            self.drafts
                .keys()
                .map(|key| key.clone().into())
                .collect::<Vec<slint::SharedString>>(),
        )));
    }
}

fn marker_form(alias: &str, marker: &MarkerSettings) -> MarkerSettingsData {
    MarkerSettingsData {
        marker: alias.to_owned(),
        title: marker.title.clone(),
        hint: marker.hint.clone(),
        shortcut: marker.shortcut.clone().unwrap_or_default(),
        section: marker.section,
        begin_audio: marker.assets.begin.audio.clone(),
        begin_kind: marker.assets.begin.kind.clone(),
        begin_reduction: marker
            .assets
            .begin
            .reduction
            .map(|n| n.to_string())
            .unwrap_or_default(),
        begin_repeat: marker
            .assets
            .begin
            .repeat
            .map(|n| n.to_string())
            .unwrap_or_default(),
        end_audio: marker.assets.end.audio.clone(),
        end_kind: marker.assets.end.kind.clone(),
        end_reduction: marker
            .assets
            .end
            .reduction
            .map(|n| n.to_string())
            .unwrap_or_default(),
        end_repeat: marker
            .assets
            .end
            .repeat
            .map(|n| n.to_string())
            .unwrap_or_default(),
    }
}

fn display_marker(window: &MainWindow, form: &MarkerSettingsData) {
    window.set_dialog_markers_title(form.title.clone().into());
    window.set_dialog_markers_hint(form.hint.clone().into());
    window.set_dialog_markers_shortcut(form.shortcut.clone().into());
    window.set_dialog_markers_section(form.section);
    window.set_dialog_markers_begin_audio(form.begin_audio.clone().into());
    window.set_dialog_markers_begin_kind(form.begin_kind.clone().into());
    window.set_dialog_markers_begin_reduction(form.begin_reduction.clone().into());
    window.set_dialog_markers_begin_repeat(form.begin_repeat.clone().into());
    window.set_dialog_markers_end_audio(form.end_audio.clone().into());
    window.set_dialog_markers_end_kind(form.end_kind.clone().into());
    window.set_dialog_markers_end_reduction(form.end_reduction.clone().into());
    window.set_dialog_markers_end_repeat(form.end_repeat.clone().into());
}

fn clear_marker_errors(window: &MainWindow) {
    window.set_dialog_error_message("".into());
    window.set_dialog_markers_alias_error("".into());
    window.set_dialog_markers_shortcut_error("".into());
    window.set_dialog_markers_begin_reduction_error("".into());
    window.set_dialog_markers_begin_repeat_error("".into());
    window.set_dialog_markers_end_reduction_error("".into());
    window.set_dialog_markers_end_repeat_error("".into());
}

fn show_error(window: &MainWindow, message: &str, field: Option<&str>) {
    window.set_dialog_error_message(message.into());
    match field {
        Some("marker") => window.set_dialog_markers_alias_error(message.into()),
        Some("shortcut") => window.set_dialog_markers_shortcut_error(message.into()),
        Some("begin_reduction") => window.set_dialog_markers_begin_reduction_error(message.into()),
        Some("begin_repeat") => window.set_dialog_markers_begin_repeat_error(message.into()),
        Some("end_reduction") => window.set_dialog_markers_end_reduction_error(message.into()),
        Some("end_repeat") => window.set_dialog_markers_end_repeat_error(message.into()),
        _ => (),
    }
}

#[cfg(test)]
#[path = "dialogs_tests.rs"]
mod tests;
