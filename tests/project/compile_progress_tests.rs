use super::{CompileProgress, Operation, ProgressUnit};
use crate::project::export_workspace::ExportCancelled;
use crate::ui::ui::UIState;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

fn active_ui() -> Arc<Mutex<UIState>> {
    Arc::new(Mutex::new(UIState {
        is_compiling: true,
        ..UIState::default()
    }))
}

fn bytes_operation(start: f32, end: f32) -> Operation {
    Operation {
        label: "Copying audio".into(),
        start,
        end,
        unit: ProgressUnit::Bytes,
    }
}

#[test]
fn audio_counts_use_interleaved_samples_and_only_the_current_stage_is_active() {
    let ui = active_ui();
    let mut progress = CompileProgress::new(
        Some(&ui),
        None,
        Operation {
            label: "Scanning peak".into(),
            start: 0.5,
            end: 0.7,
            unit: ProgressUnit::Audio {
                sample_rate: 48_000,
                channels: 2,
            },
        },
    );
    progress.report(480_000, 960_000).unwrap();
    let state = ui.lock().unwrap();
    assert!((state.compile_progress - 0.6).abs() < f32::EPSILON);
    assert_eq!(
        state.compile_stage,
        "Scanning peak: 50% · 00:00:05 / 00:00:10"
    );
    let lines: Vec<_> = state.dialog_compile_stages_text.lines().collect();
    assert_eq!(lines.len(), 4);
    assert!(lines[..3].iter().all(|line| line.ends_with('✓')));
    assert_eq!(state.dialog_compile_stages_text.matches('→').count(), 1);
    assert!(lines[3].contains(&state.compile_stage));
    assert_eq!(state.dialog_compile_eta_secs, -1.0);
}

#[test]
fn unknown_total_is_not_complete_and_a_known_total_refreshes_immediately() {
    let ui = active_ui();
    let mut progress = CompileProgress::new(Some(&ui), None, bytes_operation(0.2, 0.4));
    progress.report(0, 0).unwrap();
    {
        let state = ui.lock().unwrap();
        assert_eq!(state.compile_progress, 0.2);
        assert_eq!(state.compile_stage, "Copying audio: working…");
        assert_eq!(state.dialog_compile_eta_secs, -1.0);
    }
    progress.report(1_048_576, 4_194_304).unwrap();
    let state = ui.lock().unwrap();
    assert_eq!(state.compile_progress, 0.25);
    assert_eq!(
        state.compile_stage,
        "Copying audio: 25% · 1.0 MiB / 4.0 MiB"
    );
}

#[test]
fn completion_and_finish_refresh_without_waiting_and_never_publish() {
    let ui = active_ui();
    let mut progress = CompileProgress::new(Some(&ui), None, bytes_operation(0.7, 0.8));
    progress.report(0, 100).unwrap();
    progress.report(100, 100).unwrap();
    assert_eq!(ui.lock().unwrap().compile_progress, 0.8);

    let mut progress = CompileProgress::new(Some(&ui), None, bytes_operation(0.8, 0.95));
    progress.report(10, 100).unwrap();
    progress.finish().unwrap();
    let state = ui.lock().unwrap();
    assert_eq!(state.compile_progress, 0.95);
    assert!(state.compile_stage.contains("100%"));
    assert_eq!(state.dialog_compile_eta_secs, -1.0);
    assert!(state.is_compiling);
    assert!(!state.compile_publishing);
    assert!(state.compile_finished_at.is_none());
}

#[test]
fn intermediate_and_repeated_completion_reports_are_throttled() {
    let ui = active_ui();
    let mut progress = CompileProgress::new(Some(&ui), None, bytes_operation(0.2, 0.4));
    let checkpoint = Instant::now();
    progress.report(1, 100).unwrap();
    let before = ui.lock().unwrap().compile_stage.clone();
    progress.report(2, 100).unwrap();
    let after = ui.lock().unwrap().compile_stage.clone();
    // A suspended test process may legitimately cross the refresh deadline.
    if checkpoint.elapsed() < Duration::from_millis(50) {
        assert_eq!(after, before);
    }
    std::thread::sleep(Duration::from_millis(110));
    progress.report(3, 100).unwrap();
    assert!(ui.lock().unwrap().compile_stage.contains("3%"));

    let checkpoint = Instant::now();
    progress.report(100, 100).unwrap();
    ui.lock().unwrap().compile_stage = "Unchanged on a duplicate completion".into();
    progress.report(100, 100).unwrap();
    let after = ui.lock().unwrap().compile_stage.clone();
    if checkpoint.elapsed() < Duration::from_millis(50) {
        assert_eq!(after, "Unchanged on a duplicate completion");
    }
    progress.finish().unwrap();
    assert!(ui.lock().unwrap().compile_stage.contains("100%"));
}

#[test]
fn explicit_success_can_finish_an_operation_without_a_known_total() {
    let ui = active_ui();
    let mut progress = CompileProgress::new(Some(&ui), None, bytes_operation(0.8, 0.9));
    progress.report(0, 0).unwrap();
    progress.finish().unwrap();
    let state = ui.lock().unwrap();
    assert_eq!(state.compile_progress, 0.9);
    assert_eq!(state.compile_stage, "Copying audio: completed");
}

#[test]
fn overall_progress_does_not_move_backwards_across_operations() {
    let ui = active_ui();
    ui.lock().unwrap().compile_progress = 0.9;
    let mut progress = CompileProgress::new(Some(&ui), None, bytes_operation(0.4, 0.6));
    progress.report(25, 100).unwrap();
    assert_eq!(ui.lock().unwrap().compile_progress, 0.9);
    progress.finish().unwrap();
    assert_eq!(ui.lock().unwrap().compile_progress, 0.9);
}

#[test]
fn invalid_ranges_counts_and_audio_units_do_not_change_ui() {
    let ui = active_ui();
    ui.lock().unwrap().compile_stage = "Unchanged".into();
    for (start, end) in [
        (-0.1, 0.5),
        (0.8, 0.7),
        (0.0, 1.0),
        (f32::NAN, 0.9),
        (0.0, f32::INFINITY),
    ] {
        let mut progress = CompileProgress::new(Some(&ui), None, bytes_operation(start, end));
        assert!(progress.report(0, 1).is_err());
        assert!(progress.finish().is_err());
    }
    let mut progress = CompileProgress::new(Some(&ui), None, bytes_operation(0.0, 0.95));
    assert!(progress.report(1, 0).is_err());
    assert!(progress.report(3, 2).is_err());
    for (sample_rate, channels) in [(0, 2), (48_000, 0)] {
        let mut operation = bytes_operation(0.0, 0.95);
        operation.unit = ProgressUnit::Audio {
            sample_rate,
            channels,
        };
        let mut progress = CompileProgress::new(Some(&ui), None, operation);
        assert!(progress.report(0, 1).is_err());
        assert!(progress.finish().is_err());
    }
    let state = ui.lock().unwrap();
    assert_eq!(state.compile_progress, 0.0);
    assert_eq!(state.compile_stage, "Unchanged");
}

#[test]
fn u64_counts_do_not_overflow_or_round_incomplete_to_one_hundred_percent() {
    let ui = active_ui();
    let mut progress = CompileProgress::new(Some(&ui), None, bytes_operation(0.0, 0.95));
    progress.report(u64::MAX - 1, u64::MAX).unwrap();
    assert!(ui.lock().unwrap().compile_stage.contains("99%"));
    progress.report(u64::MAX, u64::MAX).unwrap();
    let state = ui.lock().unwrap();
    assert!(state.compile_stage.contains("100%"));
    assert!(state.compile_progress <= 0.95);
}

#[test]
fn atomic_cancellation_is_checked_even_between_ui_refreshes_and_without_ui() {
    let ui = active_ui();
    let cancel = AtomicBool::new(false);
    let mut progress = CompileProgress::new(Some(&ui), Some(&cancel), bytes_operation(0.4, 0.8));
    progress.report(10, 100).unwrap();
    let previous = ui.lock().unwrap().compile_progress;
    cancel.store(true, Ordering::SeqCst);
    assert!(progress
        .report(11, 100)
        .unwrap_err()
        .is::<ExportCancelled>());
    assert!(progress.finish().unwrap_err().is::<ExportCancelled>());
    {
        let state = ui.lock().unwrap();
        assert_eq!(state.compile_progress, previous);
        assert_eq!(state.compile_stage, "Cancelling…");
        assert_eq!(state.dialog_compile_eta_secs, -1.0);
        assert!(state.compile_cancel_requested);
    }
    let mut headless = CompileProgress::new(None, Some(&cancel), bytes_operation(0.0, 0.5));
    assert!(headless.report(0, 0).unwrap_err().is::<ExportCancelled>());
    assert!(headless.finish().unwrap_err().is::<ExportCancelled>());
}

#[test]
fn pending_ui_cancellation_is_typed_even_before_the_atomic_flag_is_set() {
    let ui = active_ui();
    let cancel = AtomicBool::new(false);
    {
        let mut state = ui.lock().unwrap();
        state.compile_cancel_requested = true;
        state.compile_stage = "Cancelling…".into();
        state.dialog_compile_eta_secs = 123.0;
    }
    let mut progress = CompileProgress::new(Some(&ui), Some(&cancel), bytes_operation(0.0, 0.5));
    assert!(progress.report(0, 100).unwrap_err().is::<ExportCancelled>());
    assert!(progress.finish().unwrap_err().is::<ExportCancelled>());
    let state = ui.lock().unwrap();
    assert!(!cancel.load(Ordering::SeqCst));
    assert_eq!(state.compile_stage, "Cancelling…");
    assert_eq!(state.compile_progress, 0.0);
    assert_eq!(state.dialog_compile_eta_secs, -1.0);
}

#[test]
fn ui_cancellation_is_observed_at_the_next_refresh_without_an_atomic_update() {
    let ui = active_ui();
    let cancel = AtomicBool::new(false);
    let mut progress = CompileProgress::new(Some(&ui), Some(&cancel), bytes_operation(0.0, 0.5));
    progress.report(1, 100).unwrap();
    ui.lock().unwrap().compile_cancel_requested = true;
    std::thread::sleep(Duration::from_millis(110));
    assert!(progress.report(2, 100).unwrap_err().is::<ExportCancelled>());
    assert!(!cancel.load(Ordering::SeqCst));
    assert_eq!(ui.lock().unwrap().compile_stage, "Cancelling…");
}

#[test]
fn inactive_publishing_and_finished_ui_are_not_overwritten_even_on_cancel() {
    for condition in 0..3 {
        let ui = active_ui();
        {
            let mut state = ui.lock().unwrap();
            state.is_compiling = condition != 0;
            state.compile_publishing = condition == 1;
            state.compile_finished_at = (condition == 2).then(Instant::now);
            state.compile_stage = "Owned by the caller".into();
            state.compile_progress = 0.97;
            state.dialog_compile_eta_secs = 42.0;
            state.dialog_compile_stages_text = "Do not change stages".into();
        }
        let cancel = AtomicBool::new(false);
        let mut progress =
            CompileProgress::new(Some(&ui), Some(&cancel), bytes_operation(0.0, 0.5));
        progress.report(1, 2).unwrap();
        progress.finish().unwrap();
        cancel.store(true, Ordering::SeqCst);
        assert!(progress.report(2, 2).unwrap_err().is::<ExportCancelled>());
        let state = ui.lock().unwrap();
        assert_eq!(state.compile_stage, "Owned by the caller");
        assert_eq!(state.compile_progress, 0.97);
        assert_eq!(state.dialog_compile_stages_text, "Do not change stages");
        assert_eq!(state.dialog_compile_eta_secs, 42.0);
        assert!(!state.compile_cancel_requested);
    }
}

#[test]
fn eta_is_operation_local_and_resets_for_a_new_operation_and_completion() {
    let ui = active_ui();
    let mut progress = CompileProgress::new(Some(&ui), None, bytes_operation(0.2, 0.4));
    progress.report(1, 100).unwrap();
    assert_eq!(ui.lock().unwrap().dialog_compile_eta_secs, -1.0);
    std::thread::sleep(Duration::from_millis(510));
    progress.report(50, 100).unwrap();
    let eta = ui.lock().unwrap().dialog_compile_eta_secs;
    assert!(eta >= 0.5 && eta.is_finite());

    let mut next = CompileProgress::new(Some(&ui), None, bytes_operation(0.4, 0.6));
    next.report(1, 100).unwrap();
    assert_eq!(ui.lock().unwrap().dialog_compile_eta_secs, -1.0);
    next.report(100, 100).unwrap();
    assert_eq!(ui.lock().unwrap().dialog_compile_eta_secs, -1.0);
}
