use super::{check_cancel, stage_export_file, ExportCancelled, ExportWorkspace};
use std::fs;
use std::os::unix::fs::{symlink, MetadataExt};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Barrier};

fn existing_export(project: &Path) -> PathBuf {
    let export = project.join("export");
    fs::create_dir(&export).unwrap();
    fs::write(export.join("old.txt"), b"accepted export").unwrap();
    export
}

#[test]
fn first_publication_contains_only_ready_files_and_cleans_work_directory() {
    let project = tempfile::tempdir().unwrap();
    let workspace = ExportWorkspace::new(project.path()).unwrap();
    let temporary = workspace.work_dir().parent().unwrap().to_path_buf();
    assert!(temporary
        .file_name()
        .unwrap()
        .to_string_lossy()
        .starts_with(".iamreader-export-"));
    assert!(workspace.ready_dir().is_dir());
    fs::write(workspace.work_dir().join("intermediate.wav"), b"temporary").unwrap();
    fs::write(workspace.ready_dir().join("book.mp3"), b"finished audio").unwrap();

    let published = workspace.publish(None, 1).unwrap();
    assert_eq!(published.path, project.path().join("export"));
    assert!(published.warnings.is_empty());
    assert_eq!(
        fs::read(published.path.join("book.mp3")).unwrap(),
        b"finished audio"
    );
    assert_eq!(fs::read_dir(&published.path).unwrap().count(), 1);
    assert!(!temporary.exists());
    assert_eq!(fs::read_dir(project.path()).unwrap().count(), 1);
}

#[test]
fn replacement_publishes_whole_directory_without_a_missing_export_window() {
    let project = tempfile::tempdir().unwrap();
    let export = project.path().join("export");
    fs::create_dir(&export).unwrap();
    fs::write(export.join("chapter-1.txt"), b"old chapter").unwrap();
    fs::write(export.join("chapter-2.txt"), b"obsolete chapter").unwrap();
    fs::write(export.join("extra.txt"), b"old extra file").unwrap();
    let workspace = ExportWorkspace::new(project.path()).unwrap();
    let temporary = workspace.ready_dir().parent().unwrap().to_path_buf();
    fs::write(workspace.ready_dir().join("chapter-1.txt"), b"new chapter").unwrap();

    let stop = Arc::new(AtomicBool::new(false));
    let barrier = Arc::new(Barrier::new(2));
    let reader_stop = stop.clone();
    let reader_barrier = barrier.clone();
    let reader_path = export.clone();
    let reader = std::thread::spawn(move || {
        assert!(fs::symlink_metadata(&reader_path).unwrap().is_dir());
        reader_barrier.wait();
        while !reader_stop.load(Ordering::SeqCst) {
            // Atomic directory publication guarantees the destination name has no gap;
            // separate reads of children are not a multi-file snapshot transaction.
            assert!(fs::symlink_metadata(&reader_path).unwrap().is_dir());
        }
    });
    barrier.wait();
    let result = workspace.publish(None, 1);
    stop.store(true, Ordering::SeqCst);
    reader.join().unwrap();
    let published = result.unwrap();
    assert!(published.warnings.is_empty());
    assert_eq!(
        fs::read(export.join("chapter-1.txt")).unwrap(),
        b"new chapter"
    );
    assert!(!export.join("chapter-2.txt").exists());
    assert!(!export.join("extra.txt").exists());
    assert_eq!(fs::read_dir(&export).unwrap().count(), 1);
    assert!(!temporary.exists());
    assert_eq!(fs::read_dir(project.path()).unwrap().count(), 1);
}

#[test]
fn cleanup_and_drop_leave_existing_export_untouched() {
    let project = tempfile::tempdir().unwrap();
    let export = existing_export(project.path());
    let workspace = ExportWorkspace::new(project.path()).unwrap();
    let temporary = workspace.work_dir().parent().unwrap().to_path_buf();
    fs::write(workspace.ready_dir().join("new.txt"), b"not published").unwrap();
    workspace.cleanup().unwrap();
    assert!(!temporary.exists());
    assert_eq!(
        fs::read(export.join("old.txt")).unwrap(),
        b"accepted export"
    );

    let workspace = ExportWorkspace::new(project.path()).unwrap();
    let temporary = workspace.work_dir().parent().unwrap().to_path_buf();
    fs::write(workspace.work_dir().join("partial.wav"), b"not ready").unwrap();
    drop(workspace);
    assert!(!temporary.exists());
    assert_eq!(
        fs::read(export.join("old.txt")).unwrap(),
        b"accepted export"
    );
    assert_eq!(fs::read_dir(&export).unwrap().count(), 1);
}

#[test]
fn empty_ready_directory_is_rejected_without_replacing_export() {
    let project = tempfile::tempdir().unwrap();
    let export = existing_export(project.path());
    let workspace = ExportWorkspace::new(project.path()).unwrap();
    let temporary = workspace.ready_dir().parent().unwrap().to_path_buf();
    fs::write(workspace.work_dir().join("unfinished.wav"), b"work only").unwrap();
    let error = workspace.publish(None, 1).unwrap_err();
    assert!(format!("{error:#}").contains("empty"));
    assert_eq!(
        fs::read(export.join("old.txt")).unwrap(),
        b"accepted export"
    );
    assert!(!temporary.exists());
}

#[test]
fn missing_one_of_two_prepared_chapters_preserves_the_existing_export() {
    let project = tempfile::tempdir().unwrap();
    let export = existing_export(project.path());
    let workspace = ExportWorkspace::new(project.path()).unwrap();
    let temporary = workspace.ready_dir().parent().unwrap().to_path_buf();
    fs::write(workspace.ready_dir().join("00001.txt"), b"first chapter").unwrap();
    let missing = workspace.ready_dir().join("00002.txt");
    fs::write(&missing, b"second chapter").unwrap();
    fs::remove_file(&missing).unwrap();

    let error = workspace.publish(None, 2).unwrap_err();
    assert!(format!("{error:#}").contains("expected 2, found 1"));
    assert_eq!(
        fs::read(export.join("old.txt")).unwrap(),
        b"accepted export"
    );
    assert_eq!(fs::read_dir(&export).unwrap().count(), 1);
    assert!(!temporary.exists());
}

#[test]
fn zero_expected_outputs_is_rejected_without_replacing_export() {
    let project = tempfile::tempdir().unwrap();
    let export = existing_export(project.path());
    let workspace = ExportWorkspace::new(project.path()).unwrap();
    let temporary = workspace.ready_dir().parent().unwrap().to_path_buf();
    fs::write(workspace.ready_dir().join("00001.txt"), b"prepared").unwrap();
    let error = workspace.publish(None, 0).unwrap_err();
    assert!(format!("{error:#}").contains("must be positive"));
    assert_eq!(
        fs::read(export.join("old.txt")).unwrap(),
        b"accepted export"
    );
    assert!(!temporary.exists());
}

#[test]
fn prepared_subdirectories_and_symlinks_are_rejected_without_touching_old_export() {
    for use_symlink in [false, true] {
        let project = tempfile::tempdir().unwrap();
        let export = existing_export(project.path());
        let workspace = ExportWorkspace::new(project.path()).unwrap();
        let temporary = workspace.ready_dir().parent().unwrap().to_path_buf();
        fs::write(workspace.ready_dir().join("new.txt"), b"prepared").unwrap();
        let invalid = workspace.ready_dir().join("invalid");
        if use_symlink {
            symlink(export.join("old.txt"), &invalid).unwrap();
        } else {
            fs::create_dir(&invalid).unwrap();
        }
        let error = workspace.publish(None, 1).unwrap_err();
        assert!(format!("{error:#}").contains("only regular files"));
        assert_eq!(
            fs::read(export.join("old.txt")).unwrap(),
            b"accepted export"
        );
        assert_eq!(fs::read_dir(&export).unwrap().count(), 1);
        assert!(!temporary.exists());
    }
}

#[test]
fn existing_export_file_is_rejected_at_creation_and_publication() {
    let project = tempfile::tempdir().unwrap();
    let export = project.path().join("export");
    fs::write(&export, b"not a managed directory").unwrap();
    assert!(ExportWorkspace::new(project.path()).is_err());
    assert_eq!(fs::read(&export).unwrap(), b"not a managed directory");
    assert_eq!(fs::read_dir(project.path()).unwrap().count(), 1);

    fs::remove_file(&export).unwrap();
    let workspace = ExportWorkspace::new(project.path()).unwrap();
    let temporary = workspace.ready_dir().parent().unwrap().to_path_buf();
    fs::write(workspace.ready_dir().join("new.txt"), b"prepared").unwrap();
    fs::write(&export, b"appeared during preparation").unwrap();
    assert!(workspace.publish(None, 1).is_err());
    assert_eq!(fs::read(&export).unwrap(), b"appeared during preparation");
    assert!(!temporary.exists());
}

#[test]
fn export_symlinks_are_rejected_without_following_or_removing_the_target() {
    for target_exists in [false, true] {
        let project = tempfile::tempdir().unwrap();
        let target = project.path().join("outside-export");
        if target_exists {
            fs::create_dir(&target).unwrap();
            fs::write(target.join("keep.txt"), b"untouched").unwrap();
        }
        let export = project.path().join("export");
        symlink(&target, &export).unwrap();
        assert!(ExportWorkspace::new(project.path()).is_err());
        assert!(fs::symlink_metadata(&export)
            .unwrap()
            .file_type()
            .is_symlink());
        fs::remove_file(&export).unwrap();

        let workspace = ExportWorkspace::new(project.path()).unwrap();
        let temporary = workspace.ready_dir().parent().unwrap().to_path_buf();
        fs::write(workspace.ready_dir().join("new.txt"), b"prepared").unwrap();
        symlink(&target, &export).unwrap();
        assert!(workspace.publish(None, 1).is_err());
        assert!(fs::symlink_metadata(&export)
            .unwrap()
            .file_type()
            .is_symlink());
        assert!(!temporary.exists());
        if target_exists {
            assert_eq!(fs::read(target.join("keep.txt")).unwrap(), b"untouched");
        } else {
            assert!(!target.exists());
        }
    }
}

#[test]
fn cancelling_prepared_export_returns_typed_error_and_preserves_existing_export() {
    let project = tempfile::tempdir().unwrap();
    let export = existing_export(project.path());
    let workspace = ExportWorkspace::new(project.path()).unwrap();
    let temporary = workspace.ready_dir().parent().unwrap().to_path_buf();
    fs::write(workspace.ready_dir().join("new.txt"), b"prepared").unwrap();
    let cancel = AtomicBool::new(true);
    let error = workspace.publish(Some(&cancel), 1).unwrap_err();
    assert!(error.is::<ExportCancelled>());
    assert_eq!(error.to_string(), "Compilation cancelled");
    assert_eq!(
        fs::read(export.join("old.txt")).unwrap(),
        b"accepted export"
    );
    assert!(!temporary.exists());
}

#[test]
fn public_cancellation_check_preserves_its_error_type() {
    assert!(check_cancel(None).is_ok());
    let cancel = AtomicBool::new(false);
    assert!(check_cancel(Some(&cancel)).is_ok());
    cancel.store(true, Ordering::SeqCst);
    assert!(check_cancel(Some(&cancel))
        .unwrap_err()
        .is::<ExportCancelled>());
}

#[test]
fn active_workspaces_are_isolated_from_each_other_and_existing_project_directories() {
    let project = tempfile::tempdir().unwrap();
    for directory in ["chunks", "tmp"] {
        fs::create_dir(project.path().join(directory)).unwrap();
        fs::write(project.path().join(directory).join("keep.txt"), directory).unwrap();
    }
    let first = ExportWorkspace::new(project.path()).unwrap();
    let second = ExportWorkspace::new(project.path()).unwrap();
    assert_ne!(first.work_dir(), second.work_dir());
    let second_temporary = second.ready_dir().parent().unwrap().to_path_buf();
    fs::write(first.work_dir().join("first.txt"), b"first job").unwrap();
    fs::write(second.ready_dir().join("second.txt"), b"second job").unwrap();
    first.cleanup().unwrap();
    assert!(second_temporary.exists());
    assert_eq!(
        fs::read(second.ready_dir().join("second.txt")).unwrap(),
        b"second job"
    );
    let published = second.publish(None, 1).unwrap();
    assert!(published.warnings.is_empty());
    assert_eq!(
        fs::read(published.path.join("second.txt")).unwrap(),
        b"second job"
    );
    assert!(!second_temporary.exists());
    for directory in ["chunks", "tmp"] {
        assert_eq!(
            fs::read_to_string(project.path().join(directory).join("keep.txt")).unwrap(),
            directory
        );
    }
    assert_eq!(fs::read_dir(project.path()).unwrap().count(), 3);
}

#[test]
fn export_staging_moves_the_same_inode_without_allocating_another_audio_copy() {
    let project = tempfile::tempdir().unwrap();
    let export = existing_export(project.path());
    let workspace = ExportWorkspace::new(project.path()).unwrap();
    for length in [0, 1, 65_535, 2 * 1024 * 1024 + 37] {
        let bytes: Vec<u8> = (0..length).map(|index| (index % 251) as u8).collect();
        let source = workspace.work_dir().join(format!("source-{length}.wav"));
        let destination = workspace.ready_dir().join(format!("book-{length}.wav"));
        fs::write(&source, &bytes).unwrap();
        let before = fs::metadata(&source).unwrap();
        assert_eq!(before.nlink(), 1);

        stage_export_file(&source, &destination).unwrap();

        assert!(!source.exists());
        assert_eq!(fs::read(&destination).unwrap(), bytes);
        let after = fs::metadata(&destination).unwrap();
        assert_eq!((after.dev(), after.ino()), (before.dev(), before.ino()));
        assert_eq!(
            after.blocks(),
            before.blocks(),
            "Staging must preserve allocated PCM blocks"
        );
        assert_eq!(after.len(), before.len());
        assert_eq!(after.nlink(), 1);
        assert_eq!(
            fs::read(export.join("old.txt")).unwrap(),
            b"accepted export"
        );
    }
    let temporary = workspace.work_dir().parent().unwrap().to_path_buf();
    workspace.cleanup().unwrap();
    assert!(!temporary.exists());
    assert_eq!(
        fs::read(export.join("old.txt")).unwrap(),
        b"accepted export"
    );
}

#[test]
fn export_staging_rejects_a_private_hardlink_to_source_audio() {
    let project = tempfile::tempdir().unwrap();
    let export = existing_export(project.path());
    let original = project.path().join("original.wav");
    fs::write(&original, b"must remain original audio").unwrap();
    let workspace = ExportWorkspace::new(project.path()).unwrap();
    let source = workspace.work_dir().join("private-alias.wav");
    let destination = workspace.ready_dir().join("book.wav");
    fs::hard_link(&original, &source).unwrap();
    let before = fs::metadata(&source).unwrap();
    assert_eq!(before.nlink(), 2);

    assert!(stage_export_file(&source, &destination).is_err());

    assert!(!destination.exists());
    assert_eq!(fs::metadata(&source).unwrap().ino(), before.ino());
    assert_eq!(fs::metadata(&original).unwrap().nlink(), 2);
    assert_eq!(fs::read(&original).unwrap(), b"must remain original audio");
    assert_eq!(
        fs::read(export.join("old.txt")).unwrap(),
        b"accepted export"
    );
    workspace.cleanup().unwrap();
    assert_eq!(fs::metadata(&original).unwrap().nlink(), 1);
    assert_eq!(fs::read(&original).unwrap(), b"must remain original audio");
    assert_eq!(
        fs::read(export.join("old.txt")).unwrap(),
        b"accepted export"
    );
}

#[test]
fn export_staging_never_replaces_existing_files_links_or_directories() {
    for kind in [
        "file",
        "hardlink",
        "symlink",
        "dangling-symlink",
        "directory",
    ] {
        let project = tempfile::tempdir().unwrap();
        let export = existing_export(project.path());
        let workspace = ExportWorkspace::new(project.path()).unwrap();
        let source = workspace.work_dir().join("source.wav");
        let destination = workspace.ready_dir().join("book.wav");
        fs::write(&source, b"prepared private audio").unwrap();
        match kind {
            "file" => fs::write(&destination, b"keep existing destination").unwrap(),
            "hardlink" => fs::hard_link(&source, &destination).unwrap(),
            "symlink" => symlink(export.join("old.txt"), &destination).unwrap(),
            "dangling-symlink" => {
                symlink(workspace.ready_dir().join("missing"), &destination).unwrap()
            }
            "directory" => fs::create_dir(&destination).unwrap(),
            _ => unreachable!(),
        }
        let before = fs::symlink_metadata(&destination).unwrap();
        let before_bytes = before.is_file().then(|| fs::read(&destination).unwrap());
        let before_link = before
            .file_type()
            .is_symlink()
            .then(|| fs::read_link(&destination).unwrap());
        let source_before = fs::metadata(&source).unwrap();

        assert!(stage_export_file(&source, &destination).is_err());

        assert_eq!(fs::read(&source).unwrap(), b"prepared private audio");
        assert_eq!(fs::metadata(&source).unwrap().ino(), source_before.ino());
        assert_eq!(
            fs::metadata(&source).unwrap().nlink(),
            source_before.nlink()
        );
        let after = fs::symlink_metadata(&destination).unwrap();
        assert_eq!(
            (after.dev(), after.ino(), after.len()),
            (before.dev(), before.ino(), before.len())
        );
        assert_eq!(after.file_type(), before.file_type());
        if let Some(bytes) = before_bytes {
            assert_eq!(fs::read(&destination).unwrap(), bytes);
        }
        if let Some(link) = before_link {
            assert_eq!(fs::read_link(&destination).unwrap(), link);
        }
        assert_eq!(
            fs::read(export.join("old.txt")).unwrap(),
            b"accepted export"
        );
        workspace.cleanup().unwrap();
        assert_eq!(
            fs::read(export.join("old.txt")).unwrap(),
            b"accepted export"
        );
        assert_eq!(fs::read_dir(export).unwrap().count(), 1);
    }
}

#[test]
fn export_staging_rejects_the_source_as_destination() {
    let project = tempfile::tempdir().unwrap();
    let export = existing_export(project.path());
    let workspace = ExportWorkspace::new(project.path()).unwrap();
    let source = workspace.work_dir().join("source.wav");
    fs::write(&source, b"source remains in work directory").unwrap();
    let before = fs::metadata(&source).unwrap();
    for destination in [
        source.clone(),
        workspace.work_dir().join(".").join("source.wav"),
    ] {
        assert!(stage_export_file(&source, &destination).is_err());
        assert_eq!(
            fs::read(&source).unwrap(),
            b"source remains in work directory"
        );
        assert_eq!(fs::metadata(&source).unwrap().ino(), before.ino());
        assert_eq!(fs::metadata(&source).unwrap().blocks(), before.blocks());
    }
    assert_eq!(fs::read_dir(workspace.ready_dir()).unwrap().count(), 0);
    assert_eq!(
        fs::read(export.join("old.txt")).unwrap(),
        b"accepted export"
    );
}

#[test]
fn export_staging_rejects_missing_nonregular_and_symlink_sources() {
    for kind in ["missing", "directory", "symlink", "dangling-symlink"] {
        let project = tempfile::tempdir().unwrap();
        let export = existing_export(project.path());
        let workspace = ExportWorkspace::new(project.path()).unwrap();
        let source = workspace.work_dir().join("invalid.wav");
        let destination = workspace.ready_dir().join("book.wav");
        match kind {
            "missing" => {}
            "directory" => fs::create_dir(&source).unwrap(),
            "symlink" => symlink(export.join("old.txt"), &source).unwrap(),
            "dangling-symlink" => symlink(workspace.work_dir().join("missing"), &source).unwrap(),
            _ => unreachable!(),
        }
        let before = fs::symlink_metadata(&source).ok();
        assert!(stage_export_file(&source, &destination).is_err());
        assert!(!destination.exists());
        if let Some(before) = before {
            let after = fs::symlink_metadata(&source).unwrap();
            assert_eq!((after.dev(), after.ino()), (before.dev(), before.ino()));
            assert_eq!(after.file_type(), before.file_type());
        } else {
            assert!(fs::symlink_metadata(&source).is_err());
        }
        assert_eq!(
            fs::read(export.join("old.txt")).unwrap(),
            b"accepted export"
        );
        workspace.cleanup().unwrap();
        assert_eq!(
            fs::read(export.join("old.txt")).unwrap(),
            b"accepted export"
        );
    }
}

#[test]
fn export_staging_failure_keeps_source_when_destination_parent_is_unavailable() {
    for parent_kind in ["missing", "file"] {
        let project = tempfile::tempdir().unwrap();
        let export = existing_export(project.path());
        let workspace = ExportWorkspace::new(project.path()).unwrap();
        let source = workspace.work_dir().join("source.wav");
        fs::write(&source, b"prepared audio stays here after failure").unwrap();
        let before = fs::metadata(&source).unwrap();
        let parent = workspace.ready_dir().join("not-a-directory");
        if parent_kind == "file" {
            fs::write(&parent, b"unrelated existing file").unwrap();
        }
        assert!(stage_export_file(&source, &parent.join("book.wav")).is_err());
        assert_eq!(
            fs::read(&source).unwrap(),
            b"prepared audio stays here after failure"
        );
        let after = fs::metadata(&source).unwrap();
        assert_eq!(
            (after.dev(), after.ino(), after.blocks()),
            (before.dev(), before.ino(), before.blocks())
        );
        if parent_kind == "file" {
            assert_eq!(fs::read(&parent).unwrap(), b"unrelated existing file");
        } else {
            assert!(!parent.exists());
        }
        assert_eq!(
            fs::read(export.join("old.txt")).unwrap(),
            b"accepted export"
        );
        workspace.cleanup().unwrap();
        assert_eq!(
            fs::read(export.join("old.txt")).unwrap(),
            b"accepted export"
        );
    }
}
