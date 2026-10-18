use anyhow::Result;
use iamreader_engine::engine::{CommandFailure, EngineCommand, EngineEvent, EngineHandle, EngineSnapshot, EngineTicket};
use iamreader_engine::project::export_status::ExportStatus;

pub(super) struct EngineClient {
    pub handle: EngineHandle,
    pub snapshot: EngineSnapshot,
    pub export: ExportStatus,
    pending: Vec<EngineTicket>,
    row_version: u64,
}

impl EngineClient {
    pub fn new(handle: EngineHandle) -> Result<Self> {
        let snapshot = handle.snapshot()?;
        Ok(Self {
            row_version: snapshot.row_update_version(), snapshot,
            export: handle.export_status()?, handle, pending: Vec::new(),
        })
    }

    pub fn submit(&mut self, command: EngineCommand) -> Result<u64> {
        let ticket = self.handle.submit(command)?;
        let request_id = ticket.request_id;
        self.pending.push(ticket);

        Ok(request_id)
    }

    pub fn poll(&mut self) -> Result<Vec<EngineEvent>> {
        let target = &mut self.snapshot;
        let row_version = &mut self.row_version;
        self.handle.read_snapshot(|source| {
            if target.file_list_version != source.file_list_version {
                target.file_list.clone_from(&source.file_list);
                target.chunk_order.clone_from(&source.chunk_order);
                target.section_titles.clone_from(&source.section_titles);
                target.toc_text.clone_from(&source.toc_text);
                target.file_list_version = source.file_list_version;
            } else if *row_version != source.row_update_version() {
                let indices = source.changed_rows_since(*row_version)
                    .unwrap_or_else(|| (0..source.file_list.len()).collect());
                for index in indices {
                    if let (Some(row), Some(updated)) = (target.file_list.get_mut(index), source.file_list.get(index)) {
                        row.clone_from(updated);
                    }
                }
            }
            // Keep the row cursor without exposing or copying the engine's patch history.
            *row_version = source.row_update_version();
            if source.is_recording {
                if let Some(index) = usize::try_from(source.current_file_index).ok() {
                    if let (Some(row), Some(updated)) = (target.file_list.get_mut(index), source.file_list.get(index)) {
                        row.duration.clone_from(&updated.duration);
                    }
                }
            }
            if target.project_version != source.project_version {
                target.meta.clone_from(&source.meta);
                target.settings.clone_from(&source.settings);
                target.markers.clone_from(&source.markers);
                target.project_version = source.project_version;
            }
            if target.waveform_version != source.waveform_version || source.is_recording {
                target.waveform_prev.clone_from(&source.waveform_prev);
                target.waveform_current.clone_from(&source.waveform_current);
                target.waveform_version = source.waveform_version;
                target.waveform_prev_loading = source.waveform_prev_loading;
                target.waveform_current_loading = source.waveform_current_loading;
            }
            target.selected_chunk.clone_from(&source.selected_chunk);
            target.current_file_index = source.current_file_index;
            target.current_file_name.clone_from(&source.current_file_name);
            target.section_titles.clone_from(&source.section_titles);
            target.total_files.clone_from(&source.total_files);
            target.total_duration.clone_from(&source.total_duration);
            target.total_size.clone_from(&source.total_size);
            target.is_recording = source.is_recording;
            target.playback_position = source.playback_position;
            target.level = source.level;
            target.recording_mode.clone_from(&source.recording_mode);
            target.hintbox_current_start.clone_from(&source.hintbox_current_start);
            target.hintbox_current_end.clone_from(&source.hintbox_current_end);
            target.hintbox_prev.clone_from(&source.hintbox_prev);
            target.hintbox_prev_start.clone_from(&source.hintbox_prev_start);
            target.hintbox_prev_end.clone_from(&source.hintbox_prev_end);
            target.error_message.clone_from(&source.error_message);
            target.stopped = source.stopped;
        })?;
        self.export = self.handle.export_status()?;
        let mut replies = Vec::new();
        let mut index = 0;
        while index < self.pending.len() {
            let ticket = &self.pending[index];
            let reply = ticket.try_recv().unwrap_or_else(|error| Some(EngineEvent::CommandCompleted {
                request_id: ticket.request_id,
                result: Err(CommandFailure { message: format!("{error:#}"), field: None }),
            }));
            match reply {
                Some(event) => {
                    self.pending.remove(index);
                    replies.push(event);
                }
                None => index += 1,
            }
        }

        Ok(replies)
    }
}
