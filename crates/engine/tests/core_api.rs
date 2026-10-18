use iamreader_engine::audio::export_wav::inspect;
use iamreader_engine::project::compiler::compile_wav_files_static;
use iamreader_engine::project::export_status::{ExportObserver, ExportStatus, ExportUpdate};
use iamreader_engine::project::{Project, ProjectFile};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

fn create_project() -> (tempfile::TempDir, PathBuf, Project) {
    let directory = tempfile::tempdir().unwrap();
    let project_path = directory.path().join("iamreader.json");
    let mut project = Project::load(&project_path).unwrap();
    let chunks_dir = directory.path().join("chunks");
    std::fs::create_dir(&chunks_dir).unwrap();
    let recording = chunks_dir.join("00001.wav");
    let mut writer = hound::WavWriter::create(&recording, hound::WavSpec {
        channels: 1,
        sample_rate: 48_000,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    }).unwrap();

    for _ in 0..480 {
        writer.write_sample(8192_i16).unwrap();
    }
    writer.finalize().unwrap();

    project.settings.cover.clear();
    project.settings.normalize = false;
    project.settings.denoise = false;
    project.markers.clear();
    project.next_chunk_number = 2;
    project.files.push(ProjectFile {
        path: "chunks/00001.wav".into(),
        title: "Synthetic recording".into(),
        author: String::new(),
        year: String::new(),
        hint: String::new(),
        markers: vec![],
        size: std::fs::metadata(recording).unwrap().len(),
        duration_ms: 10,
    });
    project.save(&project_path).unwrap();
    let project = Project::load(&project_path).unwrap();

    (directory, project_path, project)
}

#[test]
fn export_project_without_initializing_gui_or_audio_devices() {
    let (directory, project_path, project) = create_project();
    let status = Arc::new(Mutex::new(ExportStatus::default()));

    compile_wav_files_static(
        project.files, project_path, project.markers, project.meta, project.settings,
        Some(status.clone()), None, false,
    ).unwrap();

    let output = directory.path().join("export/00001.wav");
    let audio = inspect(&output).unwrap();
    let status = status.lock().unwrap();
    assert!(audio.is_rf64);
    assert_eq!(audio.sample_rate, 48_000);
    assert_eq!(audio.channels, 1);
    assert_eq!(audio.frames, 480);
    assert_eq!(status.progress, 1.0);
    assert!(status.finished_at.is_some());
    assert!(!status.publishing);
    assert!(status.error_message.is_empty());
}

enum ObserverFailure {
    BeforePublication,
    AfterPublication,
}

struct FailingObserver {
    status: Mutex<ExportStatus>,
    failure: ObserverFailure,
}

impl ExportObserver for FailingObserver {
    fn update(&self, update: ExportUpdate) -> anyhow::Result<()> {
        let should_fail = matches!(
            (&self.failure, &update),
            (ObserverFailure::BeforePublication, ExportUpdate::Publishing)
                | (ObserverFailure::AfterPublication, ExportUpdate::Finished { .. })
        );

        if should_fail {
            anyhow::bail!("Synthetic observer failure");
        }

        self.status.update(update)
    }

    fn is_cancellation_requested(&self) -> anyhow::Result<bool> {
        self.status.is_cancellation_requested()
    }
}

#[test]
fn preserve_existing_book_on_observer_failure_before_publication() {
    let (directory, project_path, project) = create_project();
    let export_dir = directory.path().join("export");
    std::fs::create_dir(&export_dir).unwrap();
    let previous = export_dir.join("previous.wav");
    std::fs::write(&previous, b"previous published book").unwrap();
    let observer = Arc::new(FailingObserver {
        status: Mutex::new(ExportStatus::default()),
        failure: ObserverFailure::BeforePublication,
    });

    let error = compile_wav_files_static(
        project.files, project_path, project.markers, project.meta, project.settings,
        Some(observer.clone()), None, false,
    ).unwrap_err();

    assert!(error.to_string().contains("Synthetic observer failure"));
    assert_eq!(std::fs::read(previous).unwrap(), b"previous published book");
    assert_eq!(std::fs::read_dir(export_dir).unwrap().count(), 1);
    assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 3);
    let status = observer.status.lock().unwrap();
    assert!(status.finished_at.is_some());
    assert!(status.stage.starts_with("Error:"));
}

#[test]
fn retain_success_on_observer_failure_after_publication() {
    let (directory, project_path, project) = create_project();
    let observer = Arc::new(FailingObserver {
        status: Mutex::new(ExportStatus::default()),
        failure: ObserverFailure::AfterPublication,
    });

    let result = compile_wav_files_static(
        project.files, project_path, project.markers, project.meta, project.settings,
        Some(observer), None, false,
    );

    assert!(result.is_ok());
    assert!(inspect(&directory.path().join("export/00001.wav")).unwrap().is_rf64);
}
