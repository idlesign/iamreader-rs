//! Maps independent export updates to the existing Slint view state.

use super::ui::UIState;
use iamreader_engine::project::export_status::{ExportObserver, ExportStatus, ExportUpdate};
use anyhow::Result;
use std::sync::{Arc, Mutex};

pub struct SlintExportObserver {
    state: Arc<Mutex<UIState>>,
}

impl SlintExportObserver {
    /// Adapt structured export notifications to a shared Slint view state.
    pub fn new(state: Arc<Mutex<UIState>>) -> Self {
        Self { state }
    }
}

impl ExportObserver for SlintExportObserver {
    fn update(&self, update: ExportUpdate) -> Result<()> {
        let mut state = self.state.lock()
            .map_err(|_| anyhow::anyhow!("Export UI state lock poisoned"))?;
        let mut status = ExportStatus {
            started: state.is_compiling,
            progress: state.compile_progress,
            stage: state.compile_stage.clone(),
            stages_text: state.dialog_compile_stages_text.clone(),
            eta_secs: state.dialog_compile_eta_secs,
            cancel_requested: state.compile_cancel_requested,
            publishing: state.compile_publishing,
            finished_at: state.compile_finished_at,
            error_message: state.error_message.clone(),
        };
        let result = status.apply(update);
        state.is_compiling = status.started;
        state.compile_progress = status.progress;
        state.compile_stage = status.stage;
        state.dialog_compile_stages_text = status.stages_text;
        state.dialog_compile_eta_secs = status.eta_secs;
        state.compile_cancel_requested = status.cancel_requested;
        state.compile_publishing = status.publishing;
        state.compile_finished_at = status.finished_at;
        state.error_message = status.error_message;

        result
    }

    fn is_cancellation_requested(&self) -> Result<bool> {
        Ok(self.state.lock()
            .map_err(|_| anyhow::anyhow!("Export UI state lock poisoned"))?
            .compile_cancel_requested)
    }
}

#[cfg(test)]
#[path = "../../tests/ui/export_adapter_tests.rs"]
mod tests;
