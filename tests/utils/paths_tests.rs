use super::{resolve_project_file, stored_recording_path};
use std::fs;

#[test]
fn recording_paths_are_relative_to_the_project_not_the_working_directory() {
    let root = tempfile::tempdir().unwrap();
    let project = root.path().join("book");
    fs::create_dir_all(project.join("chunks")).unwrap();
    let path = project.join("chunks/00001.wav");
    fs::write(&path, b"audio").unwrap();
    assert_eq!(resolve_project_file(&project, "chunks/00001.wav"), path);
    assert_eq!(
        stored_recording_path(&project, &path).unwrap(),
        "chunks/00001.wav"
    );
    assert_eq!(
        resolve_project_file(&project, "chunks/missing.wav"),
        project.join("chunks/missing.wav")
    );
}

#[test]
fn absolute_external_recordings_are_not_rebased() {
    let root = tempfile::tempdir().unwrap();
    let external = root.path().join("external.wav");
    assert_eq!(
        resolve_project_file(&root.path().join("book"), external.to_str().unwrap()),
        external
    );
    assert!(stored_recording_path(&root.path().join("book"), &external).is_err());
}
