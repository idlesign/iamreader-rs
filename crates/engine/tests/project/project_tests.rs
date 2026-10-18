use super::{Project, ProjectFile};
use std::fs;
use std::path::Path;
use std::sync::{Arc, Barrier};

fn fragment(path: &str, hint: &str) -> ProjectFile {
    ProjectFile {
        path: path.to_owned(),
        title: String::new(),
        author: String::new(),
        year: String::new(),
        hint: hint.to_owned(),
        markers: Vec::new(),
        size: 0,
        duration_ms: 0,
    }
}

#[test]
fn save_load_round_trip_preserves_project_data() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("iamreader.json");
    let mut project = Project::load(&path).unwrap();
    assert_eq!(project.next_chunk_number, 1);
    project.files.push(fragment("chunks/00001.wav", "Текст"));
    project.next_chunk_number = 42;
    project.meta.title = "Книга".to_owned();
    project.stats.record_length = 1234;
    project.save(&path).unwrap();
    let loaded = Project::load(&path).unwrap();
    assert_eq!(loaded.meta.title, "Книга");
    assert_eq!(loaded.files[0].hint, "Текст");
    assert_eq!(loaded.stats.record_length, 1234);
    assert_eq!(loaded.next_chunk_number, 42);
    assert_eq!(fs::read_dir(directory.path()).unwrap().count(), 1);
}

#[test]
fn misplaced_export_settings_are_rejected_instead_of_silently_ignored() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("iamreader.json");
    let project = Project::load(&path).unwrap();
    let mut value = serde_json::to_value(project).unwrap();
    value["meta"]["denoise"] = true.into();
    fs::write(&path, serde_json::to_vec(&value).unwrap()).unwrap();
    let error = Project::load(&path).unwrap_err();
    assert!(format!("{error:#}").contains("unknown field `denoise`"));
}

#[test]
fn repeated_saves_do_not_leave_trailing_old_json_or_temporary_files() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("iamreader.json");
    let mut project = Project::load(&path).unwrap();
    project.meta.title = "long title ".repeat(1000);
    project.save(&path).unwrap();
    project.meta.title = "short".to_owned();
    project.save(&path).unwrap();
    assert_eq!(Project::load(&path).unwrap().meta.title, "short");
    assert_eq!(fs::read_dir(directory.path()).unwrap().count(), 1);
}

#[test]
fn readers_never_observe_truncated_json_during_saves() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("iamreader.json");
    let mut project = Project::load(&path).unwrap();
    project.meta.title = "a".repeat(4096);
    project.save(&path).unwrap();
    let barrier = Arc::new(Barrier::new(2));
    let reader_path = path.clone();
    let reader_barrier = barrier.clone();
    let reader = std::thread::spawn(move || {
        reader_barrier.wait();
        for _ in 0..300 {
            let loaded = Project::load(&reader_path).unwrap();
            assert!(loaded.meta.title == "a".repeat(4096) || loaded.meta.title == "b");
        }
    });
    barrier.wait();
    for i in 0..40 {
        project.meta.title = if i % 2 == 0 {
            "b".to_owned()
        } else {
            "a".repeat(4096)
        };
        project.save(&path).unwrap();
    }
    reader.join().unwrap();
}

#[test]
fn failed_persist_does_not_remove_the_target_or_leave_temporary_files() {
    let directory = tempfile::tempdir().unwrap();
    let project = Project::load(&directory.path().join("absent.json")).unwrap();
    let target = directory.path().join("not-a-file");
    fs::create_dir(&target).unwrap();
    fs::write(target.join("keep"), "unchanged").unwrap();
    assert!(project.save(&target).is_err());
    assert_eq!(
        fs::read_to_string(target.join("keep")).unwrap(),
        "unchanged"
    );
    assert_eq!(fs::read_dir(directory.path()).unwrap().count(), 1);
}

#[cfg(unix)]
#[test]
fn saving_through_a_symlink_keeps_the_link_and_updates_its_target() {
    use std::os::unix::fs::{symlink, PermissionsExt};
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("iamreader.json");
    let link = directory.path().join("alias.json");
    let mut project = Project::load(&path).unwrap();
    project.save(&path).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o640)).unwrap();
    symlink(&path, &link).unwrap();
    project.meta.title = "updated".to_owned();
    project.save(&link).unwrap();
    assert!(fs::symlink_metadata(&link)
        .unwrap()
        .file_type()
        .is_symlink());
    assert_eq!(Project::load(&path).unwrap().meta.title, "updated");
    assert_eq!(
        fs::metadata(&path).unwrap().permissions().mode() & 0o777,
        0o640
    );
}

#[test]
fn middle_deletion_preserves_the_saved_counter_after_restart() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("iamreader.json");
    let mut project = Project::load(&path).unwrap();
    project.files = vec![
        fragment("00001.wav", ""),
        fragment("00002.wav", ""),
        fragment("00003.wav", ""),
    ];
    project.next_chunk_number = 4;
    project.save(&path).unwrap();
    let mut reopened = Project::load(&path).unwrap();
    assert_eq!(reopened.remove_file_at(1).as_deref(), Some("00002.wav"));
    reopened.save(&path).unwrap();

    let mut reopened_again = Project::load(&path).unwrap();
    let first = reopened_again.get_next_file_path(directory.path()).unwrap();
    let second = reopened_again.get_next_file_path(directory.path()).unwrap();
    assert_eq!(first.file_name().unwrap(), "00004.wav");
    assert_eq!(second.file_name().unwrap(), "00005.wav");
}

#[test]
fn unrelated_high_numbered_files_do_not_change_the_saved_counter() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("iamreader.json");
    let mut project = Project::load(&path).unwrap();
    project.next_chunk_number = 4;
    project.save(&path).unwrap();
    fs::write(directory.path().join("99999.wav"), b"unrelated").unwrap();

    let mut reopened = Project::load(&path).unwrap();
    let next = reopened.get_next_file_path(directory.path()).unwrap();
    assert_eq!(next.file_name().unwrap(), "00004.wav");
    assert_eq!(
        fs::read(directory.path().join("99999.wav")).unwrap(),
        b"unrelated"
    );
}

#[test]
fn deleting_the_tail_preserves_the_saved_counter_after_restart() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("iamreader.json");
    let mut project = Project::load(&path).unwrap();
    project.files = vec![fragment("00001.wav", ""), fragment("00003.wav", "")];
    project.next_chunk_number = 4;
    project.save(&path).unwrap();
    let mut reopened = Project::load(&path).unwrap();
    reopened.remove_files_from_index(1);
    reopened.save(&path).unwrap();
    let mut reopened_again = Project::load(&path).unwrap();
    assert_eq!(
        reopened_again
            .get_next_file_path(directory.path())
            .unwrap()
            .file_name()
            .unwrap(),
        "00004.wav"
    );
}

#[test]
fn saved_counter_survives_rejected_takes_and_restart() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("iamreader.json");
    let mut project = Project::load(&path).unwrap();
    assert_eq!(
        project
            .get_next_file_path(directory.path())
            .unwrap()
            .file_name()
            .unwrap(),
        "00001.wav"
    );
    project.save(&path).unwrap();
    let mut reopened = Project::load(&path).unwrap();
    assert_eq!(
        reopened
            .get_next_file_path(directory.path())
            .unwrap()
            .file_name()
            .unwrap(),
        "00002.wav"
    );
}

#[test]
fn occupied_wav_candidates_are_skipped_without_modification() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("iamreader.json");
    let mut project = Project::load(&path).unwrap();
    fs::write(directory.path().join("00001.wav"), b"keep first").unwrap();
    fs::write(directory.path().join("00002.wav"), b"keep second").unwrap();
    assert_eq!(
        project
            .get_next_file_path(directory.path())
            .unwrap()
            .file_name()
            .unwrap(),
        "00003.wav"
    );
    assert_eq!(
        fs::read(directory.path().join("00001.wav")).unwrap(),
        b"keep first"
    );
    assert_eq!(
        fs::read(directory.path().join("00002.wav")).unwrap(),
        b"keep second"
    );
    project.save(&path).unwrap();
    let mut reopened = Project::load(&path).unwrap();
    assert_eq!(
        reopened
            .get_next_file_path(directory.path())
            .unwrap()
            .file_name()
            .unwrap(),
        "00004.wav"
    );
}

#[test]
fn json_without_the_required_number_counter_is_rejected() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("iamreader.json");
    let project = Project::load(&path).unwrap();
    let mut json = serde_json::to_value(project).unwrap();
    json.as_object_mut().unwrap().remove("next_chunk_number");
    fs::write(&path, serde_json::to_vec(&json).unwrap()).unwrap();
    let error = Project::load(&path).unwrap_err();
    assert!(format!("{error:#}").contains("missing field `next_chunk_number`"));
}

#[test]
fn zero_number_counter_is_rejected_without_migration() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("iamreader.json");
    let mut project = Project::load(&path).unwrap();
    project.next_chunk_number = 0;
    let error = project.get_next_file_path(directory.path()).unwrap_err();
    assert!(format!("{error:#}").contains("next_chunk_number must be positive"));
    assert_eq!(project.next_chunk_number, 0);

    fs::write(&path, serde_json::to_vec(&project).unwrap()).unwrap();
    let error = Project::load(&path).unwrap_err();
    assert!(format!("{error:#}").contains("next_chunk_number must be positive"));
}

#[test]
fn transcription_follows_file_identity_after_an_index_shift() {
    let directory = tempfile::tempdir().unwrap();
    let mut project = Project::load(&directory.path().join("iamreader.json")).unwrap();
    project.files = vec![
        fragment("a.wav", ""),
        fragment("b.wav", ""),
        fragment("c.wav", ""),
    ];
    project.remove_file_at(0);
    assert!(project.apply_transcription(Path::new("b.wav"), "", "B"));
    assert_eq!(project.files[0].hint, "B");
    assert_eq!(project.files[1].hint, "");
}

#[test]
fn stale_transcription_does_not_overwrite_manual_text_or_a_replacement() {
    let directory = tempfile::tempdir().unwrap();
    let mut project = Project::load(&directory.path().join("iamreader.json")).unwrap();
    project.files = vec![fragment("new.wav", "manual")];
    assert!(!project.apply_transcription(Path::new("old.wav"), "", "old result"));
    assert!(!project.apply_transcription(Path::new("new.wav"), "", "late result"));
    assert_eq!(project.files[0].hint, "manual");
}

#[test]
fn transcription_preserves_other_project_changes_and_ignores_duplicates() {
    let directory = tempfile::tempdir().unwrap();
    let mut project = Project::load(&directory.path().join("iamreader.json")).unwrap();
    project.files = vec![fragment("a.wav", "")];
    project.meta.title = "edited title".to_owned();
    project.stats.record_length = 90;
    project.files.push(fragment("b.wav", "new recording"));
    assert!(project.apply_transcription(Path::new("a.wav"), "", "A"));
    assert!(!project.apply_transcription(Path::new("a.wav"), "", "duplicate"));
    assert_eq!(project.meta.title, "edited title");
    assert_eq!(project.stats.record_length, 90);
    assert_eq!(project.files[1].hint, "new recording");
}

#[test]
fn missing_project_loads_defaults_without_creating_a_file() {
    let directory = tempfile::tempdir().unwrap();
    for path in [
        directory.path().join("iamreader.json"),
        directory.path().join("new/book/iamreader.json"),
    ] {
        let project = Project::load(&path).unwrap();
        assert!(project.files.is_empty());
        assert_eq!(project.next_chunk_number, 1);
        assert_eq!(project.meta.title, "Untitled");
        assert_eq!(project.settings.keys.record, "r");
        assert!(!project.markers.is_empty());
        assert!(!path.exists());
    }
}

#[test]
fn missing_default_project_can_be_loaded_and_saved_from_a_new_working_directory() {
    const CHILD: &str = "IAMREADER_TEST_NEW_PROJECT_CHILD";
    if std::env::var(CHILD).as_deref() != Ok("1") {
        // Keep cwd changes isolated from concurrently running tests.
        let directory = tempfile::tempdir().unwrap();
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "project::project::tests::missing_default_project_can_be_loaded_and_saved_from_a_new_working_directory",
            ])
            .current_dir(directory.path())
            .env(CHILD, "1")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(directory.path().join("iamreader.json").is_file());
        return;
    }

    let path = Path::new("iamreader.json");
    assert!(!path.exists());
    let mut project = Project::load(path).unwrap();
    assert!(project.files.is_empty());
    assert!(!path.exists());
    project.meta.title = "New book".to_owned();
    project.save(path).unwrap();
    assert_eq!(Project::load(path).unwrap().meta.title, "New book");
}
