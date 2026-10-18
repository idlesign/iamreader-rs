use super::{SlintExportObserver, UIState};
use iamreader_engine::project::export_status::{ExportObserver, ExportOutcome, ExportStatus, ExportUpdate};
use std::sync::{Arc, Mutex};
use std::time::Instant;

#[test]
fn apply_the_same_export_lifecycle_in_slint_and_headless_observers() {
    let desktop = Arc::new(Mutex::new(UIState::default()));
    let observer = SlintExportObserver::new(desktop.clone());
    let headless = Mutex::new(ExportStatus::default());
    let finished_at = Instant::now();

    for update in [
        ExportUpdate::Started { cancel_requested: false },
        ExportUpdate::Stage { progress: 0.1, index: 1, detail: "Denoising".into() },
        ExportUpdate::Operation { progress: 0.8, detail: "Encoding".into(), eta_secs: 4.0 },
        ExportUpdate::Publishing,
        ExportUpdate::Cancelling,
        ExportUpdate::Finished {
            at: finished_at,
            outcome: ExportOutcome::Published { path: "export".into(), warnings: vec![] },
        },
    ] {
        observer.update(update.clone()).unwrap();
        headless.update(update).unwrap();

        let desktop = desktop.lock().unwrap();
        let headless = headless.lock().unwrap();
        assert_eq!(desktop.is_compiling, headless.started);
        assert_eq!(desktop.compile_progress, headless.progress);
        assert_eq!(desktop.compile_stage, headless.stage);
        assert_eq!(desktop.dialog_compile_stages_text, headless.stages_text);
        assert_eq!(desktop.dialog_compile_eta_secs, headless.eta_secs);
        assert_eq!(desktop.compile_cancel_requested, headless.cancel_requested);
        assert_eq!(desktop.compile_publishing, headless.publishing);
        assert_eq!(desktop.compile_finished_at, headless.finished_at);
        assert_eq!(desktop.error_message, headless.error_message);
    }
}

#[test]
fn report_queued_slint_cancel_before_the_command_loop_sets_its_token() {
    let desktop = Arc::new(Mutex::new(UIState::default()));
    let observer = SlintExportObserver::new(desktop.clone());
    observer.update(ExportUpdate::Started { cancel_requested: false }).unwrap();
    desktop.lock().unwrap().compile_cancel_requested = true;

    assert!(observer.is_cancellation_requested().unwrap());
    let error = observer.update(ExportUpdate::Publishing).unwrap_err();

    assert!(error.is::<iamreader_engine::project::export_workspace::ExportCancelled>());
    assert_eq!(desktop.lock().unwrap().compile_stage, "Cancelling…");
    assert!(!desktop.lock().unwrap().compile_publishing);
}
