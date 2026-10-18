use super::{
    save_chunk_dialog, save_marker_dialog, save_meta_dialog, validate_marker_form,
    validate_marker_forms, FieldError,
};
use crate::project::project::{
    ChunkSettingsData, MarkerSettings, MarkerSettingsData, MetaData, Project, ProjectFile,
};
use std::collections::HashMap;
use std::fs;
use std::path::Path;

fn form(marker: &str) -> MarkerSettingsData {
    MarkerSettingsData {
        marker: marker.into(),
        title: "Title".into(),
        hint: "Hint".into(),
        shortcut: String::new(),
        begin_audio: "begin.wav".into(),
        begin_kind: "add".into(),
        begin_reduction: String::new(),
        begin_repeat: String::new(),
        end_audio: "end.wav".into(),
        end_kind: "underlay".into(),
        end_reduction: String::new(),
        end_repeat: String::new(),
        section: true,
    }
}

fn fragment(path: &str) -> ProjectFile {
    ProjectFile {
        path: path.into(),
        title: "Old title".into(),
        author: "Old author".into(),
        year: "1999".into(),
        hint: "Old hint".into(),
        markers: vec!["chapter".into()],
        size: 123,
        duration_ms: 456,
    }
}

fn fixture(path: &Path) -> Project {
    let mut project = Project::load(path).unwrap();
    project.files = vec![fragment("first.wav"), fragment("second.wav")];
    project.next_chunk_number = 3;
    project.stats.record_length = 987;
    project.save(path).unwrap();
    project
}

fn metadata() -> MetaData {
    MetaData {
        title: "New title".into(),
        author: "New author".into(),
        year: "2026".into(),
        hint: "New hint".into(),
        reader: "New reader".into(),
        format_audio: "mp3".into(),
        normalize: true,
        cover: "new cover.png".into(),
        section_split: true,
        denoise: true,
    }
}

fn chunk() -> ChunkSettingsData {
    ChunkSettingsData {
        ui_index: 0,
        title: "Updated recording".into(),
        author: "Recording author".into(),
        year: "2025".into(),
        hint: "Edited transcription".into(),
    }
}

fn value(project: &Project) -> serde_json::Value {
    serde_json::to_value(project).unwrap()
}

fn assert_unchanged(project: &Project, before: &serde_json::Value, path: &Path, bytes: &[u8]) {
    assert_eq!(&value(project), before);
    assert_eq!(fs::read(path).unwrap(), bytes);
}

#[test]
fn marker_form_rejects_invalid_numbers_kinds_and_shortcuts_with_field_identity() {
    let cases = [
        ("shortcut", "12"),
        ("shortcut", "a"),
        ("shortcut", "１"),
        ("shortcut", "١"),
        ("begin_kind", "undelay"),
        ("end_kind", ""),
        ("begin_reduction", "-1"),
        ("begin_reduction", "101"),
        ("begin_reduction", "256"),
        ("end_reduction", "1.5"),
        ("end_reduction", "none"),
        ("begin_repeat", "-1"),
        ("begin_repeat", "1.5"),
        ("begin_repeat", "2147483648"),
        ("end_repeat", "-2"),
        ("end_repeat", "loop"),
    ];
    for (field, invalid) in cases {
        let mut draft = form(" chapter ");
        let target = match field {
            "shortcut" => &mut draft.shortcut,
            "begin_kind" => &mut draft.begin_kind,
            "end_kind" => &mut draft.end_kind,
            "begin_reduction" => &mut draft.begin_reduction,
            "end_reduction" => &mut draft.end_reduction,
            "begin_repeat" => &mut draft.begin_repeat,
            "end_repeat" => &mut draft.end_repeat,
            _ => unreachable!(),
        };
        *target = invalid.into();
        let error = validate_marker_form(&draft, None).unwrap_err();
        let error = error.downcast_ref::<FieldError>().unwrap();
        assert_eq!(error.marker, draft.marker);
        assert_eq!(error.field, field);
        assert!(!error.message.is_empty());
    }
}

#[test]
fn marker_form_accepts_blank_and_boundary_values_without_erasing_hidden_metadata() {
    let old = MarkerSettings {
        author: "Hidden author".into(),
        year: "1984".into(),
        ..MarkerSettings::default()
    };
    let mut draft = form(" chapter ");
    draft.shortcut = " \t ".into();
    draft.begin_reduction = " ".into();
    draft.end_reduction = "\t".into();
    let parsed = validate_marker_form(&draft, Some(&old)).unwrap();
    assert_eq!(parsed.author, old.author);
    assert_eq!(parsed.year, old.year);
    assert_eq!(parsed.shortcut, None);
    assert_eq!(parsed.assets.begin.reduction, None);
    assert_eq!(parsed.assets.end.reduction, None);
    assert_eq!(parsed.assets.begin.repeat, Some(1));
    assert_eq!(parsed.assets.end.repeat, Some(1));
    for key in 0..=9 {
        draft.shortcut = format!(" {key} ");
        draft.begin_reduction = " 0 ".into();
        draft.end_reduction = "100".into();
        draft.begin_repeat = "0".into();
        draft.end_repeat = "-1".into();
        let parsed = validate_marker_form(&draft, Some(&old)).unwrap();
        assert_eq!(parsed.shortcut, Some(key.to_string()));
        assert_eq!(parsed.assets.begin.reduction, Some(0));
        assert_eq!(parsed.assets.end.reduction, Some(100));
        assert_eq!(parsed.assets.begin.repeat, Some(0));
        assert_eq!(parsed.assets.end.repeat, Some(-1));
    }
    draft.begin_repeat = i32::MAX.to_string();
    draft.end_repeat = "0".into();
    let parsed = validate_marker_form(&draft, None).unwrap();
    assert_eq!(parsed.assets.begin.repeat, Some(i32::MAX));
    assert_eq!(parsed.assets.end.repeat, Some(0));
    assert!(parsed.author.is_empty() && parsed.year.is_empty());
}

#[test]
fn marker_forms_normalize_aliases_and_reject_empty_or_duplicate_aliases() {
    let empty = HashMap::new();
    let parsed = validate_marker_forms(&empty, &[form(" chapter ")]).unwrap();
    assert!(parsed.contains_key("chapter"));
    assert!(!parsed.contains_key(" chapter "));
    for drafts in [vec![form(" \t")], vec![form("chapter"), form(" chapter ")]] {
        let error = validate_marker_forms(&empty, &drafts).unwrap_err();
        let error = error.downcast_ref::<FieldError>().unwrap();
        assert_eq!(error.field, "marker");
        assert_eq!(error.marker, drafts.last().unwrap().marker);
    }
}

#[test]
fn final_marker_map_rejects_shortcut_collisions_including_unmentioned_markers() {
    let existing = HashMap::from([(
        "untouched".into(),
        MarkerSettings {
            shortcut: Some("5".into()),
            ..MarkerSettings::default()
        },
    )]);
    let mut draft = form(" edited ");
    draft.shortcut = "5".into();
    let error = validate_marker_forms(&existing, &[draft.clone()]).unwrap_err();
    let error = error.downcast_ref::<FieldError>().unwrap();
    assert_eq!(error.marker, draft.marker);
    assert_eq!(error.field, "shortcut");
    let mut other = form("other");
    other.shortcut = "5".into();
    assert!(validate_marker_forms(&HashMap::new(), &[draft, other])
        .unwrap_err()
        .is::<FieldError>());
}

#[test]
fn marker_dialog_saves_shortcut_swap_as_one_candidate_and_preserves_other_markers() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("project.json");
    let mut project = fixture(&path);
    let preserved = MarkerSettings {
        title: "Keep".into(),
        author: "Keep author".into(),
        year: "2001".into(),
        ..MarkerSettings::default()
    };
    project.markers = HashMap::from([
        (
            "a".into(),
            MarkerSettings {
                shortcut: Some("1".into()),
                author: "Author A".into(),
                year: "1998".into(),
                ..MarkerSettings::default()
            },
        ),
        (
            "b".into(),
            MarkerSettings {
                shortcut: Some("2".into()),
                ..MarkerSettings::default()
            },
        ),
        ("keep".into(), preserved.clone()),
    ]);
    project.save(&path).unwrap();
    let mut first = form(" a ");
    first.shortcut = "2".into();
    let mut second = form("b");
    second.shortcut = "1".into();
    let outcome = save_marker_dialog(&mut project, &path, &[first, second]).unwrap();
    assert!(outcome.warning.is_none());
    assert_eq!(project.markers["a"].shortcut.as_deref(), Some("2"));
    assert_eq!(project.markers["b"].shortcut.as_deref(), Some("1"));
    assert_eq!(project.markers["a"].author, "Author A");
    assert_eq!(project.markers["a"].year, "1998");
    assert_eq!(
        serde_json::to_value(&project.markers["keep"]).unwrap(),
        serde_json::to_value(preserved).unwrap()
    );
    assert_eq!(value(&Project::load(&path).unwrap()), value(&project));
    assert_eq!(fs::read_dir(directory.path()).unwrap().count(), 1);
}

#[test]
fn saving_one_marker_does_not_save_or_validate_other_local_drafts() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("project.json");
    let mut project = fixture(&path);
    let untouched = serde_json::to_value(&project.markers["footnote"]).unwrap();
    let mut unsaved_draft = form("footnote");
    unsaved_draft.begin_repeat = "invalid local edit".into();
    assert!(validate_marker_form(&unsaved_draft, project.markers.get("footnote")).is_err());
    let selected = form("chapter");
    save_marker_dialog(&mut project, &path, &[selected]).unwrap();
    assert_eq!(project.markers["chapter"].title, "Title");
    assert_eq!(
        serde_json::to_value(&project.markers["footnote"]).unwrap(),
        untouched
    );
    assert_eq!(value(&Project::load(&path).unwrap()), value(&project));
}

#[test]
fn invalid_second_marker_does_not_partially_save_the_first_draft() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("project.json");
    let mut project = fixture(&path);
    let before = value(&project);
    let bytes = fs::read(&path).unwrap();
    let first = form("chapter");
    let mut second = form("new marker");
    second.end_repeat = "-2".into();
    let error = save_marker_dialog(&mut project, &path, &[first, second]).unwrap_err();
    assert_eq!(
        error.downcast_ref::<FieldError>().unwrap().marker,
        "new marker"
    );
    assert_unchanged(&project, &before, &path, &bytes);
    assert_eq!(fs::read_dir(directory.path()).unwrap().count(), 1);
}

#[test]
fn meta_dialog_persists_all_exposed_fields_and_preserves_other_project_data() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("project.json");
    let mut project = fixture(&path);
    let before = value(&project);
    let draft = metadata();
    let outcome = save_meta_dialog(&mut project, &path, &draft).unwrap();
    assert!(outcome.warning.is_none());
    assert_eq!(project.meta.title, draft.title);
    assert_eq!(project.meta.author, draft.author);
    assert_eq!(project.meta.year, draft.year);
    assert_eq!(project.meta.hint, draft.hint);
    assert_eq!(project.meta.reader, draft.reader);
    assert_eq!(project.settings.format_audio, draft.format_audio);
    assert_eq!(project.settings.cover, draft.cover);
    assert!(
        project.settings.normalize && project.settings.section_split && project.settings.denoise
    );
    let after = value(&project);
    for field in ["files", "markers", "stats", "next_chunk_number"] {
        assert_eq!(before[field], after[field]);
    }
    for field in ["keys", "log"] {
        assert_eq!(before["settings"][field], after["settings"][field]);
    }
    assert_eq!(value(&Project::load(&path).unwrap()), after);
}

#[test]
fn chunk_dialog_checks_stable_path_and_updates_only_the_selected_recording() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("project.json");
    let mut project = fixture(&path);
    let before = value(&project);
    let bytes = fs::read(&path).unwrap();
    let mut draft = chunk();
    for (index, expected) in [(0, "first.wav"), (-1, "second.wav"), (2, "second.wav")] {
        draft.ui_index = index;
        assert!(save_chunk_dialog(&mut project, &path, &draft, expected).is_err());
        assert_unchanged(&project, &before, &path, &bytes);
    }
    draft.ui_index = 0;
    save_chunk_dialog(&mut project, &path, &draft, "second.wav").unwrap();
    let selected = &project.files[1];
    assert_eq!(selected.title, draft.title);
    assert_eq!(selected.author, draft.author);
    assert_eq!(selected.year, draft.year);
    assert_eq!(selected.hint, draft.hint);
    assert_eq!(selected.path, "second.wav");
    assert_eq!(selected.size, 123);
    assert_eq!(selected.duration_ms, 456);
    assert_eq!(selected.markers, ["chapter"]);
    assert_eq!(before["files"][0], value(&project)["files"][0]);
    assert_eq!(value(&Project::load(&path).unwrap()), value(&project));
}

#[test]
fn all_dialog_save_failures_keep_memory_and_existing_project_file_unchanged() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("project.json");
    let mut project = fixture(&path);
    let before = value(&project);
    let bytes = fs::read(&path).unwrap();
    // A regular file cannot be the parent directory of the requested save target.
    let impossible = path.join("child.json");
    assert!(save_marker_dialog(&mut project, &impossible, &[form("chapter")]).is_err());
    assert_unchanged(&project, &before, &path, &bytes);
    assert!(save_meta_dialog(&mut project, &impossible, &metadata()).is_err());
    assert_unchanged(&project, &before, &path, &bytes);
    assert!(save_chunk_dialog(&mut project, &impossible, &chunk(), "second.wav").is_err());
    assert_unchanged(&project, &before, &path, &bytes);
}

#[test]
fn invalid_export_format_does_not_save_metadata() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("project.json");
    let mut project = fixture(&path);
    let before = value(&project);
    let bytes = fs::read(&path).unwrap();
    let mut draft = metadata();
    draft.format_audio = "ogg".into();
    let error = save_meta_dialog(&mut project, &path, &draft).unwrap_err();
    assert_eq!(
        error.downcast_ref::<FieldError>().unwrap().field,
        "format_audio"
    );
    assert_unchanged(&project, &before, &path, &bytes);
}

#[cfg(unix)]
struct RestorePermissions(std::path::PathBuf, fs::Permissions);

#[cfg(unix)]
impl Drop for RestorePermissions {
    fn drop(&mut self) {
        fs::set_permissions(&self.0, self.1.clone()).unwrap();
    }
}

#[cfg(unix)]
#[test]
fn directory_sync_failure_after_rename_reports_warning_and_commits_memory() {
    use std::os::unix::fs::PermissionsExt;
    // Root bypasses directory permission checks; no portable injected I/O API is added for tests.
    if unsafe { libc::geteuid() } == 0 {
        return;
    }
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("project.json");
    let mut project = fixture(&path);
    let _restore = RestorePermissions(
        directory.path().to_path_buf(),
        fs::metadata(directory.path()).unwrap().permissions(),
    );
    // Write+execute permit tempfile/rename and reading a known file. No read permission
    // prevents Project::save from opening the directory for the final sync_all.
    fs::set_permissions(directory.path(), fs::Permissions::from_mode(0o300)).unwrap();
    assert!(fs::File::open(directory.path()).is_err());
    let draft = metadata();
    let outcome = save_meta_dialog(&mut project, &path, &draft).unwrap();
    assert!(outcome.warning.is_some());
    assert_eq!(project.meta.title, draft.title);
    let actual: serde_json::Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    assert_eq!(actual, value(&project));
}

#[cfg(unix)]
#[test]
fn unwritable_directory_preserves_all_dialogs_and_the_existing_json() {
    use std::os::unix::fs::PermissionsExt;
    if unsafe { libc::geteuid() } == 0 {
        return;
    }
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("project.json");
    let mut project = fixture(&path);
    let before = value(&project);
    let bytes = fs::read(&path).unwrap();
    let _restore = RestorePermissions(
        directory.path().to_path_buf(),
        fs::metadata(directory.path()).unwrap().permissions(),
    );
    fs::set_permissions(directory.path(), fs::Permissions::from_mode(0o500)).unwrap();
    assert!(save_marker_dialog(&mut project, &path, &[form("chapter")]).is_err());
    assert_unchanged(&project, &before, &path, &bytes);
    assert!(save_meta_dialog(&mut project, &path, &metadata()).is_err());
    assert_unchanged(&project, &before, &path, &bytes);
    assert!(save_chunk_dialog(&mut project, &path, &chunk(), "second.wav").is_err());
    assert_unchanged(&project, &before, &path, &bytes);
}
