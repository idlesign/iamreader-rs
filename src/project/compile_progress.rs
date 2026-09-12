//! Bounded UI updates for a single operation in the pre-publication export stage.
//!
//! The caller owns operation boundaries and calls `finish` only after success. App's
//! single-flight compilation worker also owns the adapter lifetime: an adapter must
//! not outlive its job or be reused after a new job has begun.

use super::export_workspace::{check_cancel, ExportCancelled};
use crate::ui::ui::UIState;
use crate::utils::format::format_duration;
use anyhow::{ensure, Result};
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const UPDATE_INTERVAL: Duration = Duration::from_millis(100);
const ETA_MIN_ELAPSED: Duration = Duration::from_millis(500);
const PREPUBLICATION_END: f32 = 0.95;

#[derive(Clone, Copy, Debug)]
pub enum ProgressUnit {
    /// Counts are interleaved PCM samples, not frames or bytes.
    Audio {
        sample_rate: u32,
        channels: u16,
    },
    Bytes,
}

#[derive(Clone, Debug)]
pub struct Operation {
    pub label: String,
    /// Overall normalized range; publication and the final 100% belong to the caller.
    pub start: f32,
    pub end: f32,
    pub unit: ProgressUnit,
}

pub struct CompileProgress<'a> {
    ui: Option<&'a Arc<Mutex<UIState>>>,
    cancel: Option<&'a AtomicBool>,
    operation: Operation,
    started: Instant,
    last_update: Option<Instant>,
    last_displayed_total: Option<u64>,
    displayed_complete: bool,
    last_report: (u64, u64),
}

impl<'a> CompileProgress<'a> {
    /// Construct one adapter per operation (for example normalization's peak scan).
    /// Invalid ranges/units are rejected by `report` or `finish`, without changing UI.
    pub fn new(
        ui: Option<&'a Arc<Mutex<UIState>>>,
        cancel: Option<&'a AtomicBool>,
        operation: Operation,
    ) -> Self {
        Self {
            ui,
            cancel,
            operation,
            started: Instant::now(),
            last_update: None,
            last_displayed_total: None,
            displayed_complete: false,
            last_report: (0, 0),
        }
    }

    /// Check atomic cancellation on every block, but lock/update UI at most 10 Hz.
    /// The first report, an unknown-to-known total and completion bypass throttling.
    /// `(0, 0)` means an unknown total, never successful completion.
    pub fn report(&mut self, done: u64, total: u64) -> Result<()> {
        self.update(done, total, false)
    }

    /// Force this operation's successful end, after the operation itself returned Ok.
    /// This never declares the export published or sets the overall progress to 1.
    pub fn finish(&mut self) -> Result<()> {
        let (_, total) = self.last_report;
        self.update(total, total, true)
    }

    fn update(&mut self, done: u64, total: u64, finished: bool) -> Result<()> {
        // Cancellation must not depend on the GUI refresh interval or a known total.
        let atomic_cancelled = check_cancel(self.cancel).is_err();
        let now = Instant::now();
        let complete = finished || (total > 0 && done == total);
        let update_due = atomic_cancelled
            || finished
            || self
                .last_update
                .is_none_or(|last| now.duration_since(last) >= UPDATE_INTERVAL)
            || (self.last_displayed_total == Some(0) && total > 0)
            || (complete && !self.displayed_complete);

        if atomic_cancelled {
            if let Some(ui) = self.ui {
                if let Ok(mut state) = ui.lock() {
                    if state.is_compiling
                        && !state.compile_publishing
                        && state.compile_finished_at.is_none()
                    {
                        state.compile_cancel_requested = true;
                        state.compile_stage = "Cancelling…".into();
                        state.dialog_compile_eta_secs = -1.0;
                    }
                }
            }
            return Err(ExportCancelled.into());
        }

        ensure!(done <= total, "Operation progress exceeds its total");
        ensure!(
            self.operation.start.is_finite()
                && self.operation.end.is_finite()
                && self.operation.start >= 0.0
                && self.operation.start <= self.operation.end
                && self.operation.end <= PREPUBLICATION_END,
            "Operation progress range must be within 0..=0.95"
        );
        if let ProgressUnit::Audio {
            sample_rate,
            channels,
        } = self.operation.unit
        {
            ensure!(
                sample_rate > 0 && channels > 0,
                "Audio progress requires a nonzero sample rate and channel count"
            );
        }
        self.last_report = (done, total);
        if !update_due {
            return Ok(());
        }
        self.last_update = Some(now);

        let Some(ui) = self.ui else {
            return Ok(());
        };
        let mut state = ui
            .lock()
            .map_err(|_| anyhow::anyhow!("Cannot lock compilation UI state"))?;
        if !state.is_compiling || state.compile_publishing || state.compile_finished_at.is_some() {
            return Ok(());
        }
        // UI cancellation can be queued before App has set the shared atomic flag.
        if state.compile_cancel_requested || check_cancel(self.cancel).is_err() {
            state.compile_cancel_requested = true;
            state.compile_stage = "Cancelling…".into();
            state.dialog_compile_eta_secs = -1.0;
            return Err(ExportCancelled.into());
        }

        let fraction = if complete {
            1.0
        } else if total > 0 {
            done as f64 / total as f64
        } else {
            0.0
        };
        let progress = (self.operation.start as f64
            + (self.operation.end as f64 - self.operation.start as f64) * fraction)
            as f32;
        state.compile_progress = state.compile_progress.max(progress);
        let percent = if complete {
            100
        } else if total > 0 {
            // Integer arithmetic avoids displaying 100% for u64::MAX - 1 of u64::MAX.
            (u128::from(done) * 100 / u128::from(total)) as u32
        } else {
            0
        };
        let detail = if total == 0 {
            format!(
                "{}: {}",
                self.operation.label,
                if finished { "completed" } else { "working…" }
            )
        } else {
            format!(
                "{}: {}% · {} / {}",
                self.operation.label,
                percent,
                self.format_count(done),
                self.format_count(total)
            )
        };
        state.dialog_compile_stages_text = format!(
            "1. prepare ✓\n2. denoise ✓\n3. process markers ✓\n4. normalize; write → {detail}"
        );
        state.compile_stage = detail;

        let elapsed = self.started.elapsed();
        state.dialog_compile_eta_secs =
            if !complete && elapsed >= ETA_MIN_ELAPSED && done > 0 && total > done {
                (elapsed.as_secs_f64() * (total - done) as f64 / done as f64).min(f32::MAX as f64)
                    as f32
            } else {
                -1.0
            };
        self.last_displayed_total = Some(total);
        self.displayed_complete = complete;
        // Start the interval at the actual update, not before a contended UI lock.
        self.last_update = Some(Instant::now());
        Ok(())
    }

    fn format_count(&self, count: u64) -> String {
        match self.operation.unit {
            ProgressUnit::Audio {
                sample_rate,
                channels,
            } => {
                let samples_per_second = u64::from(sample_rate) * u64::from(channels);
                format_duration(Duration::from_secs(count / samples_per_second), true)
            }
            ProgressUnit::Bytes => format!("{:.1} MiB", count as f64 / (1024.0 * 1024.0)),
        }
    }
}

#[cfg(test)]
#[path = "../../tests/project/compile_progress_tests.rs"]
mod tests;
