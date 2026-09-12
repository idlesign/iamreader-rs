use super::compile_wav_files_static;
use crate::project::project::{Project, ProjectFile};
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

#[test]
fn missing_source_fails_before_touching_previous_output_or_working_files() {
    let directory = tempfile::tempdir().unwrap();
    let project_path = directory.path().join("iamreader.json");
    let project = Project::load(&project_path).unwrap();
    let output = directory.path().join("export/00001.wav");
    std::fs::create_dir(output.parent().unwrap()).unwrap();
    let working = directory.path().join("chunks/tmp/keep");
    std::fs::create_dir_all(working.parent().unwrap()).unwrap();
    std::fs::write(&output, b"previous export").unwrap();
    std::fs::write(&working, b"previous working file").unwrap();
    let missing = ProjectFile {
        path: "chunks/missing.wav".into(),
        title: String::new(),
        author: String::new(),
        year: String::new(),
        hint: String::new(),
        markers: vec![],
        size: 0,
        duration_ms: 0,
    };
    let error = compile_wav_files_static(
        vec![missing],
        project_path,
        project.markers,
        project.meta,
        project.settings,
        None,
        None,
        false,
    )
    .unwrap_err();
    let message = format!("{error:#}");
    assert!(message.contains("Source recording is missing"));
    assert!(message.contains(directory.path().to_str().unwrap()));
    assert!(message.contains("chunks/missing.wav"));
    assert_eq!(std::fs::read(output).unwrap(), b"previous export");
    assert_eq!(std::fs::read(working).unwrap(), b"previous working file");
}

#[test]
fn cancelled_export_does_not_create_any_files() {
    let directory = tempfile::tempdir().unwrap();
    let project_path = directory.path().join("iamreader.json");
    let project = Project::load(&project_path).unwrap();
    let error = compile_wav_files_static(
        project.files,
        project_path,
        project.markers,
        project.meta,
        project.settings,
        None,
        Some(Arc::new(AtomicBool::new(true))),
        false,
    )
    .unwrap_err();
    assert!(format!("{error:#}").contains("Compilation cancelled"));
    assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 0);
}

fn tiny_project() -> (tempfile::TempDir, std::path::PathBuf, Project) {
    let directory = tempfile::tempdir().unwrap();
    let project_path = directory.path().join("iamreader.json");
    let mut project = Project::load(&project_path).unwrap();
    std::fs::create_dir(directory.path().join("chunks")).unwrap();
    project.settings.cover.clear();
    for marker in project.markers.values_mut() {
        marker.assets.begin.audio.clear();
        marker.assets.end.audio.clear();
    }
    for index in 1..=2 {
        let relative = format!("chunks/{index:05}.wav");
        let path = directory.path().join(&relative);
        let spec = hound::WavSpec {
            channels: 1,
            sample_rate: 16000,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let mut writer = hound::WavWriter::create(&path, spec).unwrap();
        for sample in 0..1600 {
            writer.write_sample((sample % 101 * 50) as i16).unwrap();
        }
        writer.finalize().unwrap();
        project.files.push(ProjectFile {
            path: relative,
            title: format!("Chapter {index}"),
            author: String::new(),
            year: String::new(),
            hint: String::new(),
            markers: vec!["chapter".into()],
            size: std::fs::metadata(path).unwrap().len(),
            duration_ms: 100,
        });
    }
    (directory, project_path, project)
}

fn assert_no_export_workspaces(directory: &std::path::Path) {
    for entry in std::fs::read_dir(directory).unwrap() {
        assert!(!entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with(".iamreader-export-"));
    }
}

#[test]
fn metadata_failure_after_audio_render_preserves_the_previous_export() {
    let (directory, path, mut project) = tiny_project();
    std::fs::create_dir(directory.path().join("export")).unwrap();
    let previous = directory.path().join("export/00001.wav");
    std::fs::write(&previous, b"previous result").unwrap();
    std::fs::create_dir(directory.path().join("invalid-cover")).unwrap();
    project.settings.cover = "invalid-cover".into();
    let ui = Arc::new(std::sync::Mutex::new(crate::ui::ui::UIState::default()));
    let error = compile_wav_files_static(
        project.files,
        path,
        project.markers,
        project.meta,
        project.settings,
        Some(ui.clone()),
        None,
        false,
    )
    .unwrap_err();
    assert!(format!("{error:#}").contains("Failed to read cover file"));
    assert_eq!(std::fs::read(previous).unwrap(), b"previous result");
    assert_no_export_workspaces(directory.path());
    let state = ui.lock().unwrap();
    assert!(state.is_compiling && state.compile_finished_at.is_some());
    assert!(state.compile_stage.starts_with("Error:"));
    assert!(state.error_message.contains("previous export unchanged"));
    assert!(state.compile_progress < 1.0);
}

#[test]
fn truncated_second_voice_fails_without_changing_any_previous_chapter() {
    let (directory, path, project) = tiny_project();
    let export = directory.path().join("export");
    std::fs::create_dir(&export).unwrap();
    for name in ["00001.wav", "00002.wav"] {
        std::fs::write(export.join(name), name.as_bytes()).unwrap();
    }
    let source = directory.path().join("chunks/00002.wav");
    let length = std::fs::metadata(&source).unwrap().len();
    std::fs::OpenOptions::new()
        .write(true)
        .open(&source)
        .unwrap()
        .set_len(length - 400)
        .unwrap();
    let error = compile_wav_files_static(
        project.files,
        path,
        project.markers,
        project.meta,
        project.settings,
        None,
        None,
        false,
    )
    .unwrap_err();
    assert!(format!("{error:#}").contains("Failed to read voice"));
    for name in ["00001.wav", "00002.wav"] {
        assert_eq!(std::fs::read(export.join(name)).unwrap(), name.as_bytes());
    }
    assert_no_export_workspaces(directory.path());
}

#[test]
fn chapter_export_and_single_book_replace_the_complete_managed_directory() {
    use id3::TagLike;
    let (directory, path, mut project) = tiny_project();
    let sentinel = directory.path().join("chunks/tmp/keep");
    std::fs::create_dir_all(sentinel.parent().unwrap()).unwrap();
    std::fs::write(&sentinel, b"not this job").unwrap();
    project.settings.section_split = true;
    let run = |project: &Project| {
        compile_wav_files_static(
            project.files.clone(),
            path.clone(),
            project.markers.clone(),
            project.meta.clone(),
            project.settings.clone(),
            None,
            None,
            false,
        )
        .unwrap()
    };
    run(&project);
    let export = directory.path().join("export");
    assert_eq!(std::fs::read_dir(&export).unwrap().count(), 2);
    for index in 1..=2 {
        let output = export.join(format!("{index:05}.wav"));
        let info = crate::audio::export_wav::inspect(&output).unwrap();
        assert!(info.is_rf64);
        assert_eq!(info.frames, 1600);
        assert_eq!(
            read_export_tags(&output).title(),
            Some(format!("Chapter {index}").as_str())
        );
    }
    project.settings.section_split = false;
    project.settings.normalize = true;
    run(&project);
    assert_eq!(std::fs::read_dir(&export).unwrap().count(), 1);
    assert!(
        crate::audio::export_wav::inspect(&export.join("00001.wav"))
            .unwrap()
            .frames
            > 1600
    );
    assert_eq!(std::fs::read(sentinel).unwrap(), b"not this job");
    assert_no_export_workspaces(directory.path());
    assert!(!directory.path().join("00001.wav").exists());
}

#[test]
fn recordings_inside_managed_export_are_rejected_before_publication() {
    let (directory, path, mut project) = tiny_project();
    std::fs::create_dir(directory.path().join("export")).unwrap();
    let source = directory.path().join("export/source.wav");
    std::fs::copy(directory.path().join("chunks/00001.wav"), &source).unwrap();
    let bytes = std::fs::read(&source).unwrap();
    project.files[0].path = "export/source.wav".into();
    let error = compile_wav_files_static(
        project.files,
        path,
        project.markers,
        project.meta,
        project.settings,
        None,
        None,
        false,
    )
    .unwrap_err();
    assert!(format!("{error:#}").contains("Source recording is inside"));
    assert_eq!(std::fs::read(source).unwrap(), bytes);
    assert_no_export_workspaces(directory.path());
}

#[test]
fn cancelled_and_empty_exports_have_observable_terminal_states() {
    use crate::project::export_workspace::ExportCancelled;
    let (directory, path, project) = tiny_project();
    for cancelled in [true, false] {
        let ui = Arc::new(std::sync::Mutex::new(crate::ui::ui::UIState::default()));
        let error = compile_wav_files_static(
            vec![],
            path.clone(),
            project.markers.clone(),
            project.meta.clone(),
            project.settings.clone(),
            Some(ui.clone()),
            Some(Arc::new(AtomicBool::new(cancelled))),
            false,
        )
        .unwrap_err();
        assert_eq!(error.is::<ExportCancelled>(), cancelled);
        let state = ui.lock().unwrap();
        assert!(state.is_compiling && state.compile_finished_at.is_some());
        assert!(state
            .compile_stage
            .starts_with(if cancelled { "Cancelled" } else { "Error:" }));
        assert_eq!(state.error_message.is_empty(), cancelled);
        assert!(!directory.path().join("export").exists());
    }
}

#[test]
fn worker_initialization_does_not_lose_a_queued_ui_cancel() {
    let state = Arc::new(std::sync::Mutex::new(crate::ui::ui::UIState::default()));
    let cancel = AtomicBool::new(false);
    super::begin_compile_ui(Some(&state), Some(&cancel));
    state.lock().unwrap().compile_cancel_requested = true;
    super::begin_compile_ui(Some(&state), Some(&cancel));
    assert!(state.lock().unwrap().compile_cancel_requested);
    // A completed job's cancellation must not carry into a genuinely new job.
    state.lock().unwrap().compile_finished_at = Some(std::time::Instant::now());
    super::begin_compile_ui(Some(&state), Some(&cancel));
    assert!(!state.lock().unwrap().compile_cancel_requested);
}

#[test]
fn requested_denoise_missing_model_preserves_export() {
    const CHILD: &str = "IAMREADER_TEST_DENOISE_FAILURE_CHILD";
    if std::env::var(CHILD).as_deref() != Ok("1") {
        // Isolate cwd/environment from parallel tests and from the user's models.
        let sandbox = tempfile::tempdir().unwrap();
        std::fs::create_dir(sandbox.path().join("models")).unwrap();
        for per_file in ["0", "1"] {
            let output = std::process::Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "project::compiler::tests::requested_denoise_missing_model_preserves_export",
                    "--nocapture",
                ])
                .current_dir(sandbox.path())
                .env(CHILD, "1")
                .env("IAMREADER_DENOISE_PER_FILE", per_file)
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{}\n{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
        }
        return;
    }
    let (directory, path, mut project) = tiny_project();
    project.settings.denoise = true;
    let export = directory.path().join("export");
    std::fs::create_dir(&export).unwrap();
    std::fs::write(export.join("00001.wav"), b"previous book").unwrap();
    let error = compile_wav_files_static(
        project.files,
        path,
        project.markers,
        project.meta,
        project.settings,
        None,
        None,
        false,
    )
    .unwrap_err();
    let message = format!("{error:#}");
    assert!(
        message.contains("Denoise requested") && message.contains("model could not be loaded"),
        "{message}"
    );
    assert_eq!(
        std::fs::read(export.join("00001.wav")).unwrap(),
        b"previous book"
    );
    assert_no_export_workspaces(directory.path());
}

#[test]
fn compiled_chapters_follow_the_crossfaded_timeline_including_short_and_empty_segments() {
    use std::io::{Read, Seek, SeekFrom};
    for (lengths, expected_positions, expected_frames) in [
        ([4usize, 2, 4], [0u32, 2, 4], 8u64),
        ([4, 0, 4], [0, 4, 4], 8),
        ([10, 10, 10], [0, 8, 16], 26),
    ] {
        let (directory, path, mut project) = tiny_project();
        project.settings.section_split = false;
        project.settings.normalize = false;
        let mut third = project.files[0].clone();
        third.path = "chunks/00003.wav".into();
        project.files.push(third);
        for (file, len) in project.files.iter().zip(lengths) {
            crate::audio::processing::write_samples_to_wav(
                &vec![0.25; len],
                &directory.path().join(&file.path),
                100,
                1,
            )
            .unwrap();
        }
        compile_wav_files_static(
            project.files,
            path,
            project.markers,
            project.meta,
            project.settings,
            None,
            None,
            false,
        )
        .unwrap();
        let output = directory.path().join("export/00001.wav");
        let info = crate::audio::export_wav::inspect(&output).unwrap();
        assert_eq!(info.frames, expected_frames);
        let cue = info.chunks.iter().find(|c| c.id == *b"cue ").unwrap();
        let mut file = std::fs::File::open(&output).unwrap();
        file.seek(SeekFrom::Start(cue.offset + 8)).unwrap();
        let mut bytes = vec![0; cue.size as usize];
        file.read_exact(&mut bytes).unwrap();
        assert_eq!(u32::from_le_bytes(bytes[..4].try_into().unwrap()), 3);
        for (index, expected) in expected_positions.iter().enumerate() {
            let offset = 4 + index * 24 + 20;
            assert_eq!(
                u32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap()),
                *expected
            );
        }
    }
}

fn read_export_tags(path: &std::path::Path) -> id3::Tag {
    use std::io::{Read, Seek, SeekFrom};
    let info = crate::audio::export_wav::inspect(path).unwrap();
    let chunk = info
        .chunks
        .iter()
        .find(|c| c.id.eq_ignore_ascii_case(b"id3 "))
        .unwrap();
    let mut file = std::fs::File::open(path).unwrap();
    file.seek(SeekFrom::Start(chunk.offset + 8)).unwrap();
    let mut bytes = vec![0; chunk.size as usize];
    file.read_exact(&mut bytes).unwrap();
    id3::Tag::read_from2(std::io::Cursor::new(bytes)).unwrap()
}
