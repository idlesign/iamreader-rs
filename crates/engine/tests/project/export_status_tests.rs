use super::{ExportObserver, ExportOutcome, ExportStatus, ExportUpdate};
use crate::project::export_workspace::ExportCancelled;
use std::sync::{Arc, Mutex};
use std::time::Instant;

#[test]
fn preserve_queued_cancel_on_worker_start_and_reset_for_a_new_job() {
    let mut status = ExportStatus::default();
    status.apply(ExportUpdate::Started { cancel_requested: false }).unwrap();
    status.apply(ExportUpdate::Cancelling).unwrap();

    status.apply(ExportUpdate::Started { cancel_requested: false }).unwrap();

    assert!(status.cancel_requested);
    assert_eq!(status.stage, "Cancelling…");

    status.apply(ExportUpdate::Finished {
        at: Instant::now(),
        outcome: ExportOutcome::Cancelled,
    }).unwrap();
    status.apply(ExportUpdate::Started { cancel_requested: false }).unwrap();

    assert!(!status.cancel_requested);
    assert!(status.finished_at.is_none());
    assert_eq!(status.stage, "Preparing…");
}

#[test]
fn preserve_monotonic_progress_across_stage_and_operation_updates() {
    let status = Arc::new(Mutex::new(ExportStatus::default()));
    status.update(ExportUpdate::Started { cancel_requested: false }).unwrap();
    status.update(ExportUpdate::Operation {
        progress: 0.8,
        detail: "Encoding".into(),
        eta_secs: 12.0,
    }).unwrap();

    status.update(ExportUpdate::Stage {
        progress: 0.4,
        index: 2,
        detail: "Next fragment".into(),
    }).unwrap();

    let snapshot = status.lock().unwrap();
    assert_eq!(snapshot.progress, 0.8);
    assert_eq!(snapshot.stage, "Next fragment");
    assert_eq!(snapshot.stages_text.matches('→').count(), 1);
    assert!(snapshot.stages_text.contains("3. process markers → Next fragment"));
    assert_eq!(snapshot.eta_secs, -1.0);
}

#[test]
fn reject_pending_cancel_at_publication_and_block_late_cancellation() {
    let mut status = ExportStatus::default();
    status.apply(ExportUpdate::Started { cancel_requested: true }).unwrap();

    let error = status.apply(ExportUpdate::Publishing).unwrap_err();

    assert!(error.is::<ExportCancelled>());
    assert!(error.to_string().contains("cancelled"));
    assert!(!status.publishing);
    assert_eq!(status.stage, "Cancelling…");

    let mut publishing = ExportStatus::default();
    publishing.apply(ExportUpdate::Started { cancel_requested: false }).unwrap();
    publishing.apply(ExportUpdate::Publishing).unwrap();
    let snapshot = publishing.clone();

    publishing.apply(ExportUpdate::Cancelling).unwrap();
    publishing.apply(ExportUpdate::Operation {
        progress: 0.5,
        detail: "Late progress".into(),
        eta_secs: 10.0,
    }).unwrap();

    assert_eq!(publishing, snapshot);
    assert!(!publishing.cancel_requested);
}

#[test]
fn keep_terminal_results_on_late_progress_and_duplicate_completion() {
    let mut status = ExportStatus::default();
    status.apply(ExportUpdate::Started { cancel_requested: false }).unwrap();
    status.apply(ExportUpdate::Finished {
        at: Instant::now(),
        outcome: ExportOutcome::Failed { message: "Disk is full".into() },
    }).unwrap();
    let snapshot = status.clone();

    for update in [
        ExportUpdate::Cancelling,
        ExportUpdate::Operation { progress: 0.9, detail: "Late progress".into(), eta_secs: 1.0 },
        ExportUpdate::Stage { progress: 0.95, index: 3, detail: "Late stage".into() },
        ExportUpdate::Finished { at: Instant::now(), outcome: ExportOutcome::Cancelled },
    ] {
        status.apply(update).unwrap();
    }

    assert_eq!(status, snapshot);
    assert_eq!(status.stage, "Error: Disk is full");
    assert!(status.error_message.contains("previous export unchanged"));
}

#[test]
fn report_published_warnings_without_changing_success() {
    let mut status = ExportStatus::default();
    status.apply(ExportUpdate::Started { cancel_requested: false }).unwrap();
    status.apply(ExportUpdate::Publishing).unwrap();

    status.apply(ExportUpdate::Finished {
        at: Instant::now(),
        outcome: ExportOutcome::Published {
            path: "export".into(),
            warnings: vec!["Old export cleanup failed".into()],
        },
    }).unwrap();

    assert_eq!(status.progress, 1.0);
    assert_eq!(status.stage, "Done: export (with warnings)");
    assert_eq!(status.error_message, "Export was published, but: Old export cleanup failed");
    assert_eq!(status.stages_text.matches('✓').count(), 4);
    assert!(status.finished_at.is_some());
    assert!(!status.publishing);
}

#[test]
fn reject_invalid_progress_without_mutating_the_snapshot() {
    let mut status = ExportStatus::default();
    status.apply(ExportUpdate::Started { cancel_requested: false }).unwrap();
    let snapshot = status.clone();

    for progress in [-0.1, 1.0, f32::NAN, f32::INFINITY] {
        let error = status.apply(ExportUpdate::Operation {
            progress,
            detail: "Invalid".into(),
            eta_secs: -1.0,
        }).unwrap_err();

        assert!(error.to_string().contains("within 0..=0.95"));
        assert_eq!(status, snapshot);
    }

    let error = status.apply(ExportUpdate::Stage {
        progress: 0.5,
        index: 4,
        detail: "Unknown stage".into(),
    }).unwrap_err();

    assert!(error.to_string().contains("Unknown export stage"));
    assert_eq!(status, snapshot);
}
