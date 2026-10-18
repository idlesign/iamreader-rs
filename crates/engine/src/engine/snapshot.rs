//! GUI-independent project projection. Display rows retain the existing reverse order.

use super::command::ChunkId;
use crate::project::project::{Meta, Settings};
use crate::project::MarkerSettings;
use std::collections::{HashMap, VecDeque};

#[derive(Clone, Debug, Default, PartialEq)]
pub struct FragmentRow {
    pub index: i32,
    pub name: String,
    pub path: String,
    pub markers: String,
    pub duration: String,
    pub start_time: String,
    pub size: String,
    pub is_recording: bool,
    pub is_playing: bool,
    pub title: String,
    pub author: String,
    pub year: String,
    pub hint: String,
}

#[derive(Clone, Debug, Default)]
pub struct EngineSnapshot {
    pub meta: Meta,
    pub settings: Settings,
    pub markers: HashMap<String, MarkerSettings>,
    pub project_version: u64,
    pub chunk_order: Vec<ChunkId>,
    pub selected_chunk: Option<ChunkId>,
    pub waveform_prev: Vec<f32>,
    pub waveform_current: Vec<f32>,
    pub waveform_version: u64,
    pub waveform_current_loading: bool,
    pub waveform_prev_loading: bool,
    pub level: f32,
    pub file_list: Vec<FragmentRow>,
    pub current_file_index: i32,
    pub current_file_name: String,
    pub total_files: String,
    pub total_duration: String,
    pub total_size: String,
    pub is_recording: bool,
    pub playback_position: f32,
    pub hintbox_current_start: String,
    pub hintbox_current_end: String,
    pub hintbox_prev: String,
    pub hintbox_prev_start: String,
    pub hintbox_prev_end: String,
    pub section_titles: String,
    pub toc_text: String,
    pub file_list_version: u64,
    pub recording_mode: String,
    pub error_message: String,
    pub stopped: bool,
    pub(crate) row_update_version: u64,
    pub(crate) row_updates: VecDeque<(u64, usize)>,
}

impl EngineSnapshot {
    /// Return the cursor for incremental row updates, separate from structural changes.
    pub fn row_update_version(&self) -> u64 {
        self.row_update_version
    }

    /// Return rows changed after a cursor, or None when the bounded history was lost.
    /// Callers must rebuild after structural changes to `file_list_version` as well.
    pub fn changed_rows_since(&self, previous_version: u64) -> Option<Vec<usize>> {
        if previous_version > self.row_update_version
            || self.row_updates.front().is_some_and(|(version, _)| *version > previous_version.saturating_add(1))
        {
            return None;
        }

        Some(self.row_updates.iter()
            .filter(|(version, _)| *version > previous_version)
            .map(|(_, index)| *index).collect())
    }

    /// Keep a bounded patch history; slow observers can rebuild from the full snapshot.
    pub(crate) fn mark_file_list_row_changed(&mut self, index: usize) {
        if index < self.file_list.len() {
            self.row_update_version = self.row_update_version.wrapping_add(1);
            self.row_updates.push_back((self.row_update_version, index));
            if self.row_updates.len() > 128 {
                self.row_updates.pop_front();
            }
        }
    }
}
