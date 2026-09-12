use super::{begin_compile_ui, compile_wav_files_static, save_compiled_file_from_temp, ExportStep};
use crate::audio::export_wav::{inspect, ExportWavWriter};
use crate::project::export_workspace::{ExportCancelled, ExportWorkspace};
use crate::project::project::{Meta, Settings};
use crate::ui::ui::UIState;
use std::collections::HashMap;
use std::path::Path;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};

fn prepared_audio(path: &Path) {
    let mut writer = ExportWavWriter::create(path, 44100, 2).unwrap();
    for index in 0..640_000 {
        writer.write_sample((index % 1000) as i16 - 500).unwrap();
    }
    writer.finalize().unwrap();
}

#[test]
fn cancellation_in_each_long_save_pass_preserves_the_published_book() {
    for (cancel_step, format, normalize) in [
        (ExportStep::Analyze, "wav", true),
        (ExportStep::Normalize, "wav", true),
        (ExportStep::Encode, "mp3", false),
        (ExportStep::Encode, "mp3", true),
    ] {
        let dir = tempfile::tempdir().unwrap();
        let export = dir.path().join("export");
        std::fs::create_dir(&export).unwrap();
        std::fs::write(export.join("00001.wav"), b"old published book").unwrap();
        let workspace = ExportWorkspace::new(dir.path()).unwrap();
        let input = workspace.work_dir().join("combined.wav");
        let output = workspace.ready_dir().join(format!("00001.{format}"));
        prepared_audio(&input);
        let mut events = Vec::new();
        let error = save_compiled_file_from_temp(
            &input,
            &output,
            format,
            44100,
            2,
            normalize,
            &Meta::default(),
            &Settings {
                cover: String::new(),
                ..Settings::default()
            },
            &[],
            &HashMap::new(),
            dir.path(),
            &[],
            &mut |step, done, total| {
                events.push((step, done, total));
                if step == cancel_step && done > 0 && done < total {
                    return Err(ExportCancelled.into());
                }
                Ok(())
            },
        )
        .unwrap_err();
        assert!(error.is::<ExportCancelled>(), "{error:#}");
        assert!(events
            .iter()
            .any(|(step, done, total)| *step == cancel_step && *done > 0 && done < total));
        assert!(!events
            .iter()
            .any(|(step, _, _)| *step == ExportStep::Metadata));
        assert_eq!(
            std::fs::read(export.join("00001.wav")).unwrap(),
            b"old published book"
        );
        workspace.cleanup().unwrap();
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
        assert_eq!(
            std::fs::read(export.join("00001.wav")).unwrap(),
            b"old published book"
        );
    }
}

#[test]
fn save_reports_exact_pass_counts_and_retains_successful_rf64_and_mp3_outputs() {
    for (format, normalize, expected) in [
        ("wav", false, vec![ExportStep::Metadata]),
        (
            "wav",
            true,
            vec![
                ExportStep::Analyze,
                ExportStep::Normalize,
                ExportStep::Metadata,
            ],
        ),
        ("mp3", false, vec![ExportStep::Encode, ExportStep::Metadata]),
        (
            "mp3",
            true,
            vec![
                ExportStep::Analyze,
                ExportStep::Normalize,
                ExportStep::Encode,
                ExportStep::Metadata,
            ],
        ),
    ] {
        let dir = tempfile::tempdir().unwrap();
        let input = dir.path().join("combined.wav");
        let output = dir.path().join(format!("output.{format}"));
        prepared_audio(&input);
        let mut phases = Vec::new();
        let mut last_done = 0;
        let mut known_total = None;
        let mut last_step = None;
        save_compiled_file_from_temp(
            &input,
            &output,
            format,
            44100,
            2,
            normalize,
            &Meta::default(),
            &Settings {
                cover: String::new(),
                ..Settings::default()
            },
            &[],
            &HashMap::new(),
            dir.path(),
            &[],
            &mut |step, done, total| {
                if last_step != Some(step) {
                    if let Some(total) = known_total {
                        assert_eq!(last_done, total);
                    }
                    phases.push(step);
                    last_step = Some(step);
                    last_done = 0;
                    known_total = None;
                }
                if total > 0 {
                    assert!(done >= last_done && done <= total);
                    if let Some(previous) = known_total {
                        assert_eq!(previous, total);
                    }
                    let expected_total = 640_000;
                    assert_eq!(total, expected_total);
                    known_total = Some(total);
                    last_done = done;
                }
                Ok(())
            },
        )
        .unwrap();
        assert_eq!(phases, expected);
        assert!(
            !input.exists(),
            "successful finalization consumes private input"
        );
        if format == "wav" {
            let info = inspect(&output).unwrap();
            assert!(info.is_rf64);
            assert_eq!(info.frames, 320_000);
        } else {
            assert!(std::fs::metadata(&output).unwrap().len() > 1024);
            assert!(id3::Tag::read_from_path(&output).is_ok());
        }
    }
}

#[test]
fn pending_ui_cancel_is_honored_without_waiting_for_the_command_owner() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir(dir.path().join("export")).unwrap();
    let output = dir.path().join("export/00001.wav");
    std::fs::write(&output, b"old book").unwrap();
    let state = Arc::new(Mutex::new(UIState::default()));
    let token = Arc::new(AtomicBool::new(false));
    begin_compile_ui(Some(&state), Some(token.as_ref()));
    state.lock().unwrap().compile_cancel_requested = true;
    let error = compile_wav_files_static(
        vec![],
        dir.path().join("iamreader.json"),
        HashMap::new(),
        Meta::default(),
        Settings::default(),
        Some(state.clone()),
        Some(token),
        false,
    )
    .unwrap_err();
    assert!(error.is::<ExportCancelled>());
    let state = state.lock().unwrap();
    assert!(state.compile_stage.starts_with("Cancelled"));
    assert!(state.compile_finished_at.is_some());
    assert!(!state.compile_publishing);
    assert!(state.compile_progress < 1.0);
    assert_eq!(state.dialog_compile_eta_secs, -1.0);
    assert_eq!(std::fs::read(output).unwrap(), b"old book");
}
