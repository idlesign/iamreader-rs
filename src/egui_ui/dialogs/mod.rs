//! Modal drafts, inline validation and reliable save acknowledgements.

mod export;
mod fields;
mod markers;
mod metadata;

use eframe::egui::{self, Context};
use iamreader_engine::engine::{ChunkId, CommandFailure, CommandOutcome, EngineCommand, EngineEvent, EngineSnapshot, ProjectEdit};
use iamreader_engine::project::dialog_edit::{validate_marker_forms, FieldError};
use iamreader_engine::project::{ChunkSettingsData, MetaData};
use markers::MarkerDrafts;
use metadata::ProjectDraft;

pub use export::ExportDialog;

/// Frontend-only windows; no application commands are needed to open them.
#[derive(Clone, Copy, Debug)]
pub enum DialogKind { Project, Chunk, Markers, Toc, Shortcuts, Delete }

enum Draft {
    Project(ProjectDraft),
    Chunk { data: ChunkSettingsData, path: String },
    Markers(MarkerDrafts),
    Delete { chunk: ChunkId, message: String },
    Text { title: &'static str, text: String },
}

/// An isolated modal component. A host submits returned commands and routes their replies.
pub struct EditDialog {
    draft: Draft,
    pending: Option<(u64, EngineCommand)>,
    error: Option<CommandFailure>,
    status: String,
    open: bool,
    last_size: Option<egui::Vec2>,
}

impl EditDialog {
    /// Capture a fresh draft, including the stable identity of a recording to edit or delete.
    pub fn open(kind: DialogKind, snapshot: &EngineSnapshot) -> Option<Self> {
        let selected = || snapshot.file_list.get(usize::try_from(snapshot.current_file_index).ok()?);
        let draft = match kind {
            DialogKind::Project => Draft::Project(ProjectDraft::new(MetaData {
                title: snapshot.meta.title.clone(), author: snapshot.meta.author.clone(), year: snapshot.meta.year.clone(),
                hint: snapshot.meta.hint.clone(), reader: snapshot.meta.reader.clone(), format_audio: snapshot.settings.format_audio.clone(),
                normalize: snapshot.settings.normalize, cover: snapshot.settings.cover.clone(),
                section_split: snapshot.settings.section_split, denoise: snapshot.settings.denoise,
            })),
            DialogKind::Chunk => {
                if snapshot.is_recording { return None; }
                let row = selected()?;
                Draft::Chunk { data: ChunkSettingsData {
                    ui_index: snapshot.current_file_index, title: row.title.clone(), author: row.author.clone(), year: row.year.clone(), hint: row.hint.clone(),
                }, path: row.path.clone() }
            }
            DialogKind::Markers => Draft::Markers(MarkerDrafts::new(&snapshot.markers)),
            DialogKind::Delete => {
                if snapshot.is_recording { return None; }
                let row = selected()?;
                Draft::Delete { chunk: ChunkId(row.path.clone()), message: format!("Recording #{}: {}\n{}", row.index, row.title, row.path) }
            }
            DialogKind::Toc => Draft::Text { title: "Table of contents", text: snapshot.toc_text.clone() },
            DialogKind::Shortcuts => Draft::Text { title: "Keyboard shortcuts", text: create_shortcuts(snapshot) },
        };

        Some(Self { draft, pending: None, error: None, status: String::new(), open: true, last_size: None })
    }

    /// Whether the host should retain and draw this draft.
    pub fn is_open(&self) -> bool { self.open }

    /// A save in flight prevents duplicate saves, cancellation and closing the native window.
    pub fn is_busy(&self) -> bool { self.pending.is_some() }

    /// Remember the request only after the engine accepted it.
    pub fn start_request(&mut self, request_id: u64, command: EngineCommand) {
        self.pending = Some((request_id, command));
        self.error = None;
        self.status.clear();
    }

    /// Submission failed before a ticket existed; retain input and allow a retry.
    pub fn fail_submission(&mut self, message: String) {
        self.error = Some(CommandFailure { message: format!("Cannot submit changes: {message}. Your edits are still here."), field: None });
    }

    /// Apply only this modal's matching acknowledgement; other requests belong to its host.
    pub fn accept_reply(&mut self, reply: &EngineEvent) -> bool {
        let EngineEvent::CommandCompleted { request_id, result } = reply;
        if self.pending.as_ref().map(|(pending_id, _)| pending_id) != Some(request_id) { return false; }
        let Some((_, command)) = self.pending.take() else { return false; };
        match result {
            Err(error) => self.error = Some(error.clone()),
            Ok(outcome) => {
                match (&mut self.draft, command) {
                    (Draft::Markers(markers), EngineCommand::SaveProjectEdit(edit)) => match *edit {
                        ProjectEdit::Marker(form) => {
                            if let Ok(saved) = validate_marker_forms(&markers.saved, std::slice::from_ref(&form)) { markers.saved = saved; }
                            self.status = format!("Saved: {}", form.marker);
                        }
                        ProjectEdit::RefreshMetadata => self.status = "File metadata refreshed.".into(),
                        _ => {}
                    },
                    _ => self.open = false,
                }
                if let CommandOutcome::Saved { warning: Some(warning) } = outcome {
                    self.open = true;
                    self.error = Some(CommandFailure { message: format!("Saved, but {warning}"), field: None });
                }
            }
        }

        true
    }

    /// Render without I/O. A returned command must be submitted once and acknowledged.
    pub fn show(&mut self, context: &Context) -> Option<EngineCommand> {
        let busy = self.is_busy();
        let escape = context.input(|input| input.key_pressed(egui::Key::Escape));
        let mut save = false;
        let mut refresh = false;
        let mut cancel = false;
        let id = egui::Id::unique("edit-dialog");
        // Keep hit targets stable when validation/status text changes the modal's height.
        let area = egui::Modal::default_area(id).anchor(egui::Align2::CENTER_TOP, egui::vec2(0.0, 16.0));
        let response = egui::Modal::new(id).area(area).show(context, |ui| {
            ui.set_width((context.content_rect().width() - 48.0).clamp(240.0, 640.0));
            let title = match &self.draft {
                Draft::Project(_) => "Project Settings", Draft::Chunk { .. } => "Chunk Settings", Draft::Markers(_) => "Markers",
                Draft::Delete { .. } => "Delete recording?", Draft::Text { title, .. } => title,
            };
            ui.horizontal(|ui| {
                ui.heading(title);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let close = ui.add_enabled(!busy, egui::Button::new("X"));
                    close.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, !busy, "Close dialog"));
                    cancel = close.clicked();
                });
            });
            ui.separator();
            ui.add_enabled_ui(!busy, |ui| {
                match &mut self.draft {
                    Draft::Project(project) => project.show_tabs(ui),
                    Draft::Markers(markers) => markers.show_selector(ui, &mut self.error),
                    _ => {}
                }
                let status_height = if self.status.is_empty() { 0.0 }
                    else { ui.text_style_height(&egui::TextStyle::Body) + ui.spacing().item_spacing.y };
                let available_height = (context.content_rect().bottom() - ui.cursor().top() - status_height - 76.0).max(0.0);
                let feedback_height = if self.error.is_some() {
                    ((context.content_rect().height() * 0.18).min(72.0) + 8.0).min(available_height)
                } else { 0.0 };
                // Scroll regions yield space to the actions when larger text or feedback appears.
                let body_height = (available_height - feedback_height).min(460.0);
                egui::ScrollArea::vertical().id_salt("dialog-body").min_scrolled_height(0.0).max_height(body_height)
                    .auto_shrink([false, true]).show(ui, |ui| {
                        match &mut self.draft {
                            Draft::Project(project) => project.show(ui, self.error.as_ref()),
                            Draft::Chunk { data, .. } => metadata::show_chunk(ui, data, self.error.as_ref()),
                            Draft::Markers(markers) => markers.show(ui, &mut self.error, &mut self.status),
                            Draft::Delete { message, .. } => { ui.label(message.as_str()); }
                            Draft::Text { text, .. } => { ui.label(text.as_str()); }
                        }
                    });
                if self.error.is_some() {
                    egui::ScrollArea::vertical().id_salt("dialog-feedback").min_scrolled_height(0.0)
                        .max_height((feedback_height - 8.0).max(0.0))
                        .show(ui, |ui| fields::show_error(ui, self.error.as_ref()));
                }
                if !self.status.is_empty() { ui.add(egui::Label::new(&self.status).truncate()); }
                ui.separator();
                ui.horizontal_wrapped(|ui| {
                    if matches!(self.draft, Draft::Markers(_)) { refresh = ui.button("Update meta").clicked(); }
                    let readonly = matches!(self.draft, Draft::Text { .. });
                    cancel |= ui.button(if readonly { "Close" } else { "Cancel" }).clicked();
                    if !readonly {
                        let has_selection = !matches!(&self.draft, Draft::Markers(markers) if !markers.drafts.contains_key(&markers.selected));
                        save = ui.add_enabled(has_selection, egui::Button::new(if matches!(self.draft, Draft::Delete { .. }) { "Delete" } else { "Save" })).clicked();
                    }
                });
            });
            if busy { ui.label("Saving…"); }
        });
        fields::settle_modal_layout(context, response.response.rect.size(), &mut self.last_size);
        if !busy && (cancel || (escape && response.is_top_modal && !response.any_popup_open)) { self.open = false; }
        if !self.open || busy { return None; }
        if refresh { return Some(EngineCommand::SaveProjectEdit(Box::new(ProjectEdit::RefreshMetadata))); }
        if !save { return None; }
        self.error = None;
        let edit = match &self.draft {
            Draft::Project(project) => ProjectEdit::Meta(project.data.clone()),
            Draft::Chunk { data, path } => ProjectEdit::Chunk { data: data.clone(), expected_path: path.clone() },
            Draft::Markers(markers) => {
                let form = markers.drafts.get(&markers.selected)?;
                if let Err(error) = validate_marker_forms(&markers.saved, std::slice::from_ref(form)) {
                    self.error = Some(CommandFailure {
                        field: error.downcast_ref::<FieldError>().map(|field| field.field.into()), message: error.to_string(),
                    });
                    context.request_repaint();
                    return None;
                }
                ProjectEdit::Marker(form.clone())
            }
            Draft::Delete { chunk, .. } => return Some(EngineCommand::DeleteChunk { chunk: chunk.clone() }),
            Draft::Text { .. } => return None,
        };

        Some(EngineCommand::SaveProjectEdit(Box::new(edit)))
    }
}

fn create_shortcuts(snapshot: &EngineSnapshot) -> String {
    let keys = &snapshot.settings.keys;
    let mut lines = vec![
        format!("{} — Start/restart recording", keys.record), format!("{} — Phrase OK, new recording", keys.ok),
        format!("{} — Stop recording or playback", keys.stop), format!("{} / Down — Previous recording", keys.prev),
        format!("{} / Up — Next recording", keys.next), format!("{} — Playback", keys.play),
        format!("{} — Next chapter", keys.chapter_next), format!("{} — Previous chapter", keys.chapter_prev),
        format!("{} — Update mode", keys.mode_update), format!("{} — Insert mode", keys.mode_insert),
        format!("{} — Delete recording", keys.delete),
        "F1 — Keyboard shortcuts\nF4 — Markers\nF5 — Project\nF8 — Export\nF10 — Table of contents".into(),
    ];
    let mut markers: Vec<_> = snapshot.markers.iter().filter_map(|(name, marker)| marker.shortcut.as_ref().map(|shortcut| (shortcut, name))).collect();
    markers.sort();
    lines.extend(markers.into_iter().map(|(shortcut, name)| format!("{shortcut} — marker \"{name}\"")));

    lines.join("\n")
}
