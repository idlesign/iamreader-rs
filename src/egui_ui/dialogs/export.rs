use eframe::egui::{self, Context};
use iamreader_engine::engine::EngineEvent;
use iamreader_engine::project::export_status::ExportStatus;
use std::time::Instant;

#[derive(Default)]
/// A modal view of one export, retaining its result until explicitly dismissed.
pub struct ExportDialog {
    visible: bool,
    observed: (u64, bool, Option<Instant>),
    request: Option<u64>,
    waiting: bool,
    cancelling: bool,
    failure: Option<String>,
    last_size: Option<egui::Vec2>,
}

impl ExportDialog {
    /// Whether the export modal currently covers the application.
    pub fn is_open(&self) -> bool { self.visible }

    /// Show a queued command, with cancellation disabled until the engine starts the job.
    pub fn start_request(&mut self, request_id: u64) {
        self.request = Some(request_id);
        self.waiting = true;
        self.visible = true;
        self.cancelling = false;
        self.failure = None;
    }

    /// Consume only the acknowledgement of this component's queued export request.
    pub fn accept_reply(&mut self, reply: &EngineEvent) -> bool {
        let EngineEvent::CommandCompleted { request_id, result } = reply;
        if self.request != Some(*request_id) { return false; }
        self.request = None;
        if let Err(error) = result {
            self.failure = Some(error.message.clone());
            self.waiting = false;
        }

        true
    }

    /// Observe a new job or terminal transition without reopening a dismissed old result.
    pub fn sync(&mut self, status: &ExportStatus) {
        let identity = (status.job_id, status.started, status.finished_at);
        if identity != self.observed && status.started {
            self.visible = true;
            self.waiting = false;
            self.cancelling = status.cancel_requested;
            self.failure = None;
        }
        self.observed = identity;
    }

    /// Return true for one cancellation request. Terminal results never close themselves.
    pub fn show(&mut self, context: &Context, status: &ExportStatus) -> bool {
        if !self.visible { return false; }
        let terminal = self.failure.is_some() || (!self.waiting && status.finished_at.is_some());
        let can_cancel = !self.waiting && !terminal && status.started && !status.cancel_requested && !self.cancelling && !status.publishing;
        let mut cancel = false;
        let mut close = false;
        let id = egui::Id::unique("export-dialog");
        let area = egui::Modal::default_area(id).anchor(egui::Align2::CENTER_TOP, egui::vec2(0.0, 16.0));
        let response = egui::Modal::new(id).area(area).show(context, |ui| {
            ui.set_width((context.content_rect().width() - 48.0).clamp(240.0, 600.0));
            ui.heading("Export audiobook");
            egui::ScrollArea::vertical().max_height((context.content_rect().height() - 160.0).clamp(40.0, 400.0)).show(ui, |ui| {
                if let Some(failure) = &self.failure {
                    ui.colored_label(ui.visuals().error_fg_color, failure);
                } else if self.waiting {
                    ui.label("Preparing…");
                } else {
                    ui.label(if self.cancelling && !terminal { "Cancelling…" } else { &status.stage });
                    ui.add(egui::ProgressBar::new(status.progress).text(format!("Overall ~ {:.0}%", status.progress * 100.0)));
                    // The bundled text fonts have arrows, but no U+2713 checkmark.
                    ui.label(status.stages_text.replace('✓', "done"));
                    if status.eta_secs >= 0.0 { ui.label(format!("Stage ETA: {:.0} s", status.eta_secs)); }
                    if !status.error_message.is_empty() { ui.colored_label(ui.visuals().error_fg_color, &status.error_message); }
                }
            });
            ui.separator();
            if terminal {
                close = ui.button("Close").clicked();
            } else {
                cancel = ui.add_enabled(can_cancel, egui::Button::new("Cancel export")).clicked();
            }
        });
        super::fields::settle_modal_layout(context, response.response.rect.size(), &mut self.last_size);
        let escape = response.should_close() && !response.backdrop_response.clicked();
        if terminal && (close || escape) { self.visible = false; }
        if can_cancel && (cancel || escape) {
            self.cancelling = true;
            return true;
        }

        false
    }
}
