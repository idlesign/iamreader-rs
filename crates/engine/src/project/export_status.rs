//! Observable export state shared by desktop and headless callers.

use super::export_workspace::ExportCancelled;
use anyhow::{ensure, Result};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Instant;

/// Terminal result, independent of how a caller displays it.
#[derive(Clone, Debug, PartialEq)]
pub enum ExportOutcome {
    Published { path: PathBuf, warnings: Vec<String> },
    Cancelled,
    Failed { message: String },
}

/// A synchronous change to one export job.
#[derive(Clone, Debug, PartialEq)]
pub enum ExportUpdate {
    Started { cancel_requested: bool },
    Stage { progress: f32, index: u32, detail: String },
    Operation { progress: f32, detail: String, eta_secs: f32 },
    Cancelling,
    Publishing,
    Finished { at: Instant, outcome: ExportOutcome },
}

/// A caller's view of the current or most recently completed export.
#[derive(Clone, Debug, PartialEq)]
pub struct ExportStatus {
    /// Stable across owner/worker initialization, distinct for each subsequent job.
    pub job_id: u64,
    pub started: bool,
    pub progress: f32,
    pub stage: String,
    pub stages_text: String,
    /// Negative when an estimate is unavailable.
    pub eta_secs: f32,
    pub cancel_requested: bool,
    pub publishing: bool,
    pub finished_at: Option<Instant>,
    pub error_message: String,
}

impl Default for ExportStatus {
    fn default() -> Self {
        Self {
            job_id: 0,
            started: false,
            progress: 0.0,
            stage: String::new(),
            stages_text: String::new(),
            eta_secs: -1.0,
            cancel_requested: false,
            publishing: false,
            finished_at: None,
            error_message: String::new(),
        }
    }
}

impl ExportStatus {
    /// Apply an update without letting late progress replace a terminal result.
    pub fn apply(&mut self, update: ExportUpdate) -> Result<()> {
        match update {
            ExportUpdate::Started { cancel_requested } => {
                if !self.started || self.finished_at.is_some() {
                    self.job_id = self.job_id.wrapping_add(1);
                }
                let queued_cancel = self.started
                    && self.finished_at.is_none()
                    && self.cancel_requested;
                self.started = true;
                self.finished_at = None;
                self.publishing = false;
                self.cancel_requested = queued_cancel || cancel_requested;
                self.progress = 0.0;
                self.error_message.clear();
                self.stage = if self.cancel_requested { "Cancelling…" } else { "Preparing…" }.into();
                self.stages_text =
                    "1. prepare →\n2. denoise\n3. process markers\n4. normalize; write".into();
                self.eta_secs = -1.0;
            }
            ExportUpdate::Stage { progress, index, detail } => {
                ensure!(index < 4, "Unknown export stage: {index}");
                self.validate_progress(progress)?;

                if !self.can_report_progress() {
                    return Ok(());
                }
                self.check_pending_cancel()?;
                self.progress = self.progress.max(progress);
                self.stage = detail.clone();
                self.eta_secs = -1.0;
                self.stages_text = ["prepare", "denoise", "process markers", "normalize; write"]
                    .iter()
                    .enumerate()
                    .map(|(stage_index, name)| {
                        if (stage_index as u32) < index {
                            format!("{}. {} ✓", stage_index + 1, name)
                        } else if stage_index as u32 == index {
                            format!("{}. {} → {}", stage_index + 1, name, detail)
                        } else {
                            format!("{}. {}  ", stage_index + 1, name)
                        }
                    })
                    .collect::<Vec<_>>()
                    .join("\n");
            }
            ExportUpdate::Operation { progress, detail, eta_secs } => {
                self.validate_progress(progress)?;
                ensure!(eta_secs.is_finite(), "Invalid export time estimate");

                if !self.can_report_progress() {
                    return Ok(());
                }
                self.check_pending_cancel()?;
                self.progress = self.progress.max(progress);
                self.stages_text = format!(
                    "1. prepare ✓\n2. denoise ✓\n3. process markers ✓\n4. normalize; write → {detail}"
                );
                self.stage = detail;
                self.eta_secs = eta_secs;
            }
            ExportUpdate::Cancelling => {
                if self.can_report_progress() {
                    self.cancel_requested = true;
                    self.stage = "Cancelling…".into();
                    self.eta_secs = -1.0;
                }
            }
            ExportUpdate::Publishing => {
                ensure!(self.started && self.finished_at.is_none(), "No active export to publish");
                self.check_pending_cancel()?;
                self.publishing = true;
                self.stage = "Publishing…".into();
                self.eta_secs = -1.0;
            }
            ExportUpdate::Finished { at, outcome } => {
                if self.finished_at.is_some() {
                    return Ok(());
                }
                self.publishing = false;
                self.finished_at = Some(at);
                self.eta_secs = -1.0;

                match outcome {
                    ExportOutcome::Published { path, warnings } => {
                        self.progress = 1.0;
                        self.stage = format!("Done: {}", path.display());
                        self.stages_text =
                            "1. prepare ✓\n2. denoise ✓\n3. process markers ✓\n4. normalize; write ✓".into();

                        if !warnings.is_empty() {
                            self.stage.push_str(" (with warnings)");
                            self.error_message =
                                format!("Export was published, but: {}", warnings.join("; "));
                        }
                    }
                    ExportOutcome::Cancelled => {
                        self.stage = "Cancelled; previous export unchanged".into();
                    }
                    ExportOutcome::Failed { message } => {
                        self.stage = format!("Error: {message}");
                        self.error_message =
                            format!("Compilation failed; previous export unchanged: {message}");
                    }
                }
            }
        }

        Ok(())
    }

    fn can_report_progress(&self) -> bool {
        self.started && !self.publishing && self.finished_at.is_none()
    }

    fn validate_progress(&self, progress: f32) -> Result<()> {
        ensure!(
            progress.is_finite() && (0.0..=0.95).contains(&progress),
            "Pre-publication export progress must be within 0..=0.95"
        );

        Ok(())
    }

    fn check_pending_cancel(&mut self) -> Result<()> {
        if self.cancel_requested {
            self.stage = "Cancelling…".into();
            self.eta_secs = -1.0;
            return Err(ExportCancelled.into());
        }

        Ok(())
    }
}

/// Receives export updates without depending on a particular frontend.
/// Implementations serialize updates with cancellation requests; `Publishing`
/// must reject a pending cancellation before opening the publication boundary.
pub trait ExportObserver: Send + Sync {
    /// Apply one update, reporting failures rather than hiding them.
    fn update(&self, update: ExportUpdate) -> Result<()>;

    /// Check a queued cancellation even before the command loop handles it.
    fn is_cancellation_requested(&self) -> Result<bool>;
}

impl ExportObserver for Mutex<ExportStatus> {
    fn update(&self, update: ExportUpdate) -> Result<()> {
        self.lock()
            .map_err(|_| anyhow::anyhow!("Export status lock poisoned"))?
            .apply(update)
    }

    fn is_cancellation_requested(&self) -> Result<bool> {
        Ok(self.lock()
            .map_err(|_| anyhow::anyhow!("Export status lock poisoned"))?
            .cancel_requested)
    }
}

impl<Observer: ExportObserver + ?Sized> ExportObserver for Arc<Observer> {
    fn update(&self, update: ExportUpdate) -> Result<()> {
        self.as_ref().update(update)
    }

    fn is_cancellation_requested(&self) -> Result<bool> {
        self.as_ref().is_cancellation_requested()
    }
}

#[cfg(test)]
#[path = "../../tests/project/export_status_tests.rs"]
mod tests;
