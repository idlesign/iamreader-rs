//! Slint consumes engine data and acknowledgements; no project mutation lives here.

use super::dialogs::DialogReply;
use super::ui::UIState;
use super::FileInfo;
use iamreader_engine::engine::snapshot::FragmentRow;
use iamreader_engine::engine::{ChunkId, CommandOutcome, EngineCommand, EngineEvent, EngineHandle, EngineTicket};
use iamreader_engine::project::export_status::ExportStatus;
use crate::action::{translate_action, Action};
use anyhow::Result;
use std::sync::{Arc, Mutex};

pub struct SlintEngineAdapter {
    handle: EngineHandle,
    state: Arc<Mutex<UIState>>,
    pending: Vec<(EngineTicket, Option<u64>, bool)>,
    list_version: Option<u64>,
    row_version: u64,
    project_version: Option<u64>,
    waveform_version: Option<u64>,
    last_export: Option<ExportStatus>,
    last_error: String,
    delete_target: Option<ChunkId>,
}

fn convert_row(row: &FragmentRow) -> FileInfo {
    FileInfo {
        index: row.index, name: row.name.clone().into(), path: row.path.clone().into(),
        markers: row.markers.clone().into(), duration: row.duration.clone().into(),
        start_time: row.start_time.clone().into(), size: row.size.clone().into(),
        is_recording: row.is_recording, is_playing: row.is_playing,
        title: row.title.clone().into(), author: row.author.clone().into(),
        year: row.year.clone().into(), hint: row.hint.clone().into(),
    }
}

impl SlintEngineAdapter {
    /// Attach a frontend without passing its state into the engine.
    pub fn new(handle: EngineHandle, state: Arc<Mutex<UIState>>) -> Self {
        Self {
            handle, state, pending: Vec::new(), list_version: None, row_version: 0,
            project_version: None, waveform_version: None, last_export: None,
            last_error: String::new(), delete_target: None,
        }
    }

    /// Keep window actions local and submit only application commands.
    pub fn dispatch(&mut self, action: Action) -> Result<()> {
        self.poll()?;
        match action {
            Action::OpenMarkerSettings => {
                self.state.lock().map_err(|_| anyhow::anyhow!("UI state lock poisoned"))?.dialog_markers_open = true;
                return Ok(());
            }
            Action::OpenShortcutsDialog => {
                let text = self.handle.read_snapshot(|snapshot| {
                    let keys = &snapshot.settings.keys;
                    let mut lines = vec![
                        format!("{} — Start/restart recording", keys.record), format!("{} — Phrase OK, new recording", keys.ok),
                        format!("{} — Stop recording", keys.stop), format!("{} — Previous recording", keys.prev),
                        format!("{} — Next recording", keys.next), format!("{} — Playback", keys.play),
                        format!("{} — Next chapter", keys.chapter_next), format!("{} — Previous chapter", keys.chapter_prev),
                    ];
                    let mut markers: Vec<_> = snapshot.markers.iter().filter_map(|(name, marker)| {
                        marker.shortcut.as_ref().map(|shortcut| (shortcut, name))
                    }).collect();
                    markers.sort();
                    lines.extend(markers.into_iter().map(|(shortcut, name)| format!("{shortcut} — marker \"{name}\"")));
                    lines.join("\n")
                })?;
                let mut state = self.state.lock().map_err(|_| anyhow::anyhow!("UI state lock poisoned"))?;
                state.dialog_shortcuts_list_text = text;
                state.dialog_shortcuts_open = true;
                return Ok(());
            }
            Action::OpenDeleteChunkDialog => {
                let target = self.handle.read_snapshot(|snapshot| {
                    if snapshot.is_recording { return None; }
                    let target = snapshot.selected_chunk.clone()?;
                    let row = snapshot.file_list.iter().find(|row| row.path == target.0)?;
                    let title = if row.title.is_empty() { "—" } else { &row.title };
                    Some((target, format!("Запись №{}: {}, файл: {}", row.index, title, row.path), snapshot.current_file_index))
                })?;
                if let Some((target, text, index)) = target {
                    self.delete_target = Some(target);
                    let mut state = self.state.lock().map_err(|_| anyhow::anyhow!("UI state lock poisoned"))?;
                    state.dialog_delete_text = text;
                    state.dialog_delete_file_index = index;
                    state.dialog_delete_open = true;
                }
                return Ok(());
            }
            _ => {}
        }
        let dialog_id = match &action { Action::SaveDialog { request_id, .. } => Some(*request_id), _ => None };
        let deleting = matches!(&action, Action::ConfirmDeleteChunk { .. });
        let command = if deleting {
            Some(EngineCommand::DeleteChunk { chunk: self.delete_target.clone().ok_or_else(|| anyhow::anyhow!("No recording selected for deletion"))? })
        } else {
            translate_action(action, &self.handle)?
        };
        if let Some(command) = command {
            match self.handle.submit(command) {
                Ok(ticket) => self.pending.push((ticket, dialog_id, deleting)),
                Err(error) => {
                    if let Some(request_id) = dialog_id {
                        self.state.lock().map_err(|_| anyhow::anyhow!("UI state lock poisoned"))?.dialog_save_result = Some(DialogReply {
                            request_id, result: Err(format!("{error:#}")), field: None,
                        });
                    } else {
                        return Err(error);
                    }
                }
            }
        }

        Ok(())
    }

    /// Apply changed rows and bounded live data, then publish reliable command results.
    pub fn poll(&mut self) -> Result<()> {
        let state_handle = self.state.clone();
        self.handle.read_snapshot(|snapshot| -> Result<()> {
            let mut state = state_handle.lock().map_err(|_| anyhow::anyhow!("UI state lock poisoned"))?;
            if self.list_version != Some(snapshot.file_list_version) {
                state.file_list = snapshot.file_list.iter().map(convert_row).collect();
                state.file_list_version = state.file_list_version.wrapping_add(1);
                state.file_list_dirty_rows.clear();
                state.section_titles.clone_from(&snapshot.section_titles);
                state.dialog_toc_list_text.clone_from(&snapshot.toc_text);
                self.list_version = Some(snapshot.file_list_version);
            } else if self.row_version != snapshot.row_update_version() {
                let indices = snapshot.changed_rows_since(self.row_version)
                    .unwrap_or_else(|| (0..snapshot.file_list.len()).collect());
                for index in indices {
                    if let (Some(target), Some(source)) = (state.file_list.get_mut(index), snapshot.file_list.get(index)) {
                        *target = convert_row(source);
                        state.mark_file_list_row_changed(index);
                    }
                }
            }
            self.row_version = snapshot.row_update_version();
            if snapshot.is_recording {
                if let Ok(index) = usize::try_from(snapshot.current_file_index) {
                    if let (Some(target), Some(source)) = (state.file_list.get_mut(index), snapshot.file_list.get(index)) {
                        target.duration = source.duration.clone().into();
                    }
                }
            }
            state.current_file_index = snapshot.current_file_index;
            state.current_file_name.clone_from(&snapshot.current_file_name);
            state.total_files.clone_from(&snapshot.total_files);
            state.total_duration.clone_from(&snapshot.total_duration);
            state.total_size.clone_from(&snapshot.total_size);
            state.is_recording = snapshot.is_recording;
            state.playback_position = snapshot.playback_position;
            state.level = snapshot.level;
            state.recording_mode.clone_from(&snapshot.recording_mode);
            state.hintbox_current_start.clone_from(&snapshot.hintbox_current_start);
            state.hintbox_current_end.clone_from(&snapshot.hintbox_current_end);
            state.hintbox_prev_start.clone_from(&snapshot.hintbox_prev_start);
            state.hintbox_prev_end.clone_from(&snapshot.hintbox_prev_end);
            state.hintbox_prev.clone_from(&snapshot.hintbox_prev);
            if self.waveform_version != Some(snapshot.waveform_version) || snapshot.is_recording {
                state.waveform_current.clone_from(&snapshot.waveform_current);
                state.waveform_prev.clone_from(&snapshot.waveform_prev);
                state.waveform_current_loading = snapshot.waveform_current_loading;
                state.waveform_prev_loading = snapshot.waveform_prev_loading;
                state.waveform_version = state.waveform_version.wrapping_add(1);
                self.waveform_version = Some(snapshot.waveform_version);
            }
            if self.project_version != Some(snapshot.project_version) {
                state.dialog_project_title.clone_from(&snapshot.meta.title);
                state.dialog_project_author.clone_from(&snapshot.meta.author);
                state.dialog_project_year.clone_from(&snapshot.meta.year);
                state.dialog_project_hint.clone_from(&snapshot.meta.hint);
                state.dialog_project_reader.clone_from(&snapshot.meta.reader);
                state.dialog_project_format_audio.clone_from(&snapshot.settings.format_audio);
                state.dialog_project_normalize = snapshot.settings.normalize;
                state.dialog_project_cover.clone_from(&snapshot.settings.cover);
                state.dialog_project_section_split = snapshot.settings.section_split;
                state.dialog_project_denoise = snapshot.settings.denoise;
                state.dialog_marker_snapshot.clone_from(&snapshot.markers);
                self.project_version = Some(snapshot.project_version);
            }
            if self.last_error != snapshot.error_message {
                state.error_message.clone_from(&snapshot.error_message);
                self.last_error.clone_from(&snapshot.error_message);
            }
            Ok(())
        })??;
        let export = self.handle.export_status()?;
        if self.last_export.as_ref() != Some(&export) {
            let mut state = self.state.lock().map_err(|_| anyhow::anyhow!("UI state lock poisoned"))?;
            state.is_compiling = export.started;
            state.compile_progress = export.progress;
            state.compile_stage.clone_from(&export.stage);
            state.dialog_compile_stages_text.clone_from(&export.stages_text);
            state.dialog_compile_eta_secs = export.eta_secs;
            state.compile_cancel_requested = export.cancel_requested;
            state.compile_publishing = export.publishing;
            state.compile_finished_at = export.finished_at;
            if !export.error_message.is_empty() { state.error_message.clone_from(&export.error_message); }
            self.last_export = Some(export);
        }
        let mut index = 0;
        while index < self.pending.len() {
            let event = self.pending[index].0.try_recv();
            if matches!(event, Ok(None)) { index += 1; continue; }
            let (_, dialog_id, deleting) = self.pending.remove(index);
            let result = match event {
                Ok(Some(EngineEvent::CommandCompleted { result, .. })) => result,
                Err(error) => Err(iamreader_engine::engine::CommandFailure { message: format!("{error:#}"), field: None }),
                Ok(None) => unreachable!("Pending acknowledgements remain queued"),
            };
            let mut state = self.state.lock().map_err(|_| anyhow::anyhow!("UI state lock poisoned"))?;
            if let Some(request_id) = dialog_id {
                let field = result.as_ref().err().and_then(|error| error.field.clone());
                state.dialog_save_result = Some(DialogReply {
                    request_id, field,
                    result: result.map(|outcome| match outcome { CommandOutcome::Saved { warning } => warning, _ => None }).map_err(|error| error.message),
                });
            } else if let Err(error) = result {
                state.error_message = error.message;
            } else if deleting {
                state.dialog_delete_open = false;
                self.delete_target = None;
            }
        }

        Ok(())
    }
}
