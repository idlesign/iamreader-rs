#![cfg(feature = "slint-ui")]

#[path = "../crates/engine/tests/support/mod.rs"]
mod support;

use support::{chunk, create_metadata, Fixture};
use iamreader_engine::engine::{EngineCommand, ProjectEdit};
use iamreader::ui::ui::UIState;
use iamreader::ui::updater::SlintEngineAdapter;
use iamreader::action::Action;
use std::sync::{Arc, Mutex};

#[test]
fn acknowledge_dialog_saves_and_retain_committed_values_on_error() {
    let mut fixture = Fixture::create();
    let state = Arc::new(Mutex::new(UIState::default()));
    let mut adapter = SlintEngineAdapter::new(fixture.handle.clone(), state.clone());
    adapter.poll().unwrap();
    adapter.dispatch(Action::SaveDialog {
        request_id: 71, edit: Box::new(ProjectEdit::Meta(create_metadata("Committed title"))),
    }).unwrap();
    fixture.engine.tick().unwrap();
    adapter.poll().unwrap();

    let reply = state.lock().unwrap().dialog_save_result.take().unwrap();
    assert_eq!(reply.request_id, 71);
    assert_eq!(reply.result, Ok(None));
    assert_eq!(state.lock().unwrap().dialog_project_title, "Committed title");
    let mut invalid = create_metadata("Must not commit");
    invalid.format_audio = "invalid".into();
    adapter.dispatch(Action::SaveDialog { request_id: 72, edit: Box::new(ProjectEdit::Meta(invalid)) }).unwrap();
    fixture.engine.tick().unwrap();
    adapter.poll().unwrap();

    let reply = state.lock().unwrap().dialog_save_result.take().unwrap();
    assert_eq!(reply.request_id, 72);
    assert_eq!(reply.field.as_deref(), Some("format_audio"));
    assert!(reply.result.is_err());
    assert_eq!(state.lock().unwrap().dialog_project_title, "Committed title");
    assert!(state.lock().unwrap().error_message.is_empty());
}

#[test]
fn patch_selection_and_playback_without_rebuilding_slint_rows() {
    let mut fixture = Fixture::create();
    let state = Arc::new(Mutex::new(UIState::default()));
    let mut adapter = SlintEngineAdapter::new(fixture.handle.clone(), state.clone());
    adapter.poll().unwrap();
    let version = state.lock().unwrap().file_list_version;
    state.lock().unwrap().dialog_project_title = "Local draft".into();

    adapter.dispatch(Action::Goto { index: Some(1), play: false }).unwrap();
    fixture.engine.tick().unwrap();
    adapter.poll().unwrap();
    adapter.dispatch(Action::Play).unwrap();
    fixture.engine.tick().unwrap();
    adapter.poll().unwrap();

    let state = state.lock().unwrap();
    assert_eq!(state.file_list_version, version);
    assert_eq!(state.current_file_index, 1);
    assert!(state.file_list[1].is_playing);
    assert!(state.file_list_dirty_rows.contains(&1));
    assert_eq!(state.dialog_project_title, "Local draft");
}

#[test]
fn delete_the_captured_identity_after_other_rows_move() {
    let mut fixture = Fixture::create();
    fixture.select(2);
    let state = Arc::new(Mutex::new(UIState::default()));
    let mut adapter = SlintEngineAdapter::new(fixture.handle.clone(), state.clone());
    adapter.dispatch(Action::OpenDeleteChunkDialog).unwrap();
    assert!(state.lock().unwrap().dialog_delete_open);
    fixture.execute(EngineCommand::DeleteChunk { chunk: chunk(3) }).unwrap();

    adapter.dispatch(Action::ConfirmDeleteChunk { ui_index: 1 }).unwrap();
    fixture.engine.tick().unwrap();
    adapter.poll().unwrap();

    assert_eq!(fixture.handle.snapshot().unwrap().chunk_order, vec![chunk(1)]);
    assert!(!state.lock().unwrap().dialog_delete_open);
}

#[test]
fn keep_window_actions_local_and_report_disconnected_saves() {
    let mut fixture = Fixture::create();
    let state = Arc::new(Mutex::new(UIState::default()));
    let mut adapter = SlintEngineAdapter::new(fixture.handle.clone(), state.clone());

    adapter.dispatch(Action::OpenMarkerSettings).unwrap();
    adapter.dispatch(Action::OpenShortcutsDialog).unwrap();

    assert!(state.lock().unwrap().dialog_markers_open);
    assert!(state.lock().unwrap().dialog_shortcuts_open);
    assert!(state.lock().unwrap().dialog_shortcuts_list_text.contains("chapter"));
    assert!(fixture.audio.borrow().calls.is_empty());
    fixture.engine.shutdown().unwrap();
    adapter.dispatch(Action::SaveDialog { request_id: 73, edit: Box::new(ProjectEdit::Meta(create_metadata("Not saved"))) }).unwrap();

    let reply = state.lock().unwrap().dialog_save_result.take().unwrap();
    assert_eq!(reply.request_id, 73);
    assert!(reply.result.is_err());
}

#[test]
fn cancel_immediately_and_leave_terminal_export_closed_until_the_next_job() {
    let mut fixture = Fixture::create();
    let (observer, entered, release) = support::export_observer::create_barrier(false);
    fixture.engine = fixture.engine.with_export_observer(observer);
    let state = Arc::new(Mutex::new(UIState::default()));
    let mut adapter = SlintEngineAdapter::new(fixture.handle.clone(), state.clone());
    adapter.dispatch(Action::Compile).unwrap();
    fixture.engine.tick().unwrap();
    entered.recv().unwrap();
    adapter.poll().unwrap();
    assert!(state.lock().unwrap().is_compiling);

    adapter.dispatch(Action::CompileCancel).unwrap();
    assert!(fixture.handle.export_status().unwrap().cancel_requested);
    release.send(()).unwrap();
    fixture.engine.wait_for_export().unwrap();
    fixture.engine.tick().unwrap();
    adapter.poll().unwrap();

    assert!(state.lock().unwrap().compile_finished_at.is_some());
    assert!(state.lock().unwrap().compile_stage.starts_with("Cancelled;"));
    state.lock().unwrap().is_compiling = false;
    state.lock().unwrap().compile_finished_at = None;
    adapter.poll().unwrap();
    assert!(!state.lock().unwrap().is_compiling);
}
