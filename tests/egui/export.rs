use super::{create_harness, set_text, Fixture};
use super::support::export_observer::create_barrier;
use eframe::egui::{Key, Pos2, Rect, Theme, Vec2};
use egui_kittest::{Harness, kittest::{NodeT, Queryable}};
use iamreader::egui_ui::{configure_context, dialogs::ExportDialog};
use iamreader_engine::engine::EngineCommand;
use iamreader_engine::project::export_status::{ExportOutcome, ExportStatus, ExportUpdate};
use std::time::Instant;

#[test]
fn cancel_once_before_publication_keep_the_old_book_and_close_the_result_explicitly() {
    let mut fixture = Fixture::create();
    let output = fixture.directory.path().join("export");
    std::fs::create_dir(&output).unwrap();
    std::fs::write(output.join("previous.wav"), b"previous published book").unwrap();
    let (observer, entered, release) = create_barrier(false);
    fixture.engine = fixture.engine.with_export_observer(observer);
    let mut harness = create_harness(&fixture, [1000.0, 800.0], 1.0);
    harness.key_press(Key::F8);
    harness.run();
    assert!(harness.get_by_label("Cancel export").accesskit_node().is_disabled());
    harness.key_press(Key::F8);
    harness.run();
    fixture.engine.tick().unwrap();
    entered.recv().unwrap();
    harness.run();
    assert!(!harness.get_by_label("Cancel export").accesskit_node().is_disabled());
    harness.get_by_label("Cancel export").click();
    harness.run();
    assert!(fixture.handle.export_status().unwrap().cancel_requested);
    assert!(harness.get_by_label("Cancel export").accesskit_node().is_disabled());
    harness.get_by_label("Cancel export").click();
    harness.key_press(Key::Escape);
    harness.run();
    assert!(harness.query_by_label("Export audiobook").is_some());
    release.send(()).unwrap();
    fixture.engine.wait_for_export().unwrap();
    fixture.engine.tick().unwrap();
    harness.run();

    assert!(harness.query_by_label_contains("Cancelled; previous export unchanged").is_some());
    assert!(harness.query_by_label("Cancel export").is_none());
    assert_eq!(std::fs::read(output.join("previous.wav")).unwrap(), b"previous published book");
    harness.run();
    assert!(harness.query_by_label("Close").is_some());
    harness.key_press(Key::Escape);
    harness.run();
    assert!(harness.query_by_label("Export audiobook").is_none());
    harness.run();
    assert!(harness.query_by_label("Export audiobook").is_none());
    assert!(fixture.audio.borrow().calls.is_empty());
}

#[test]
fn disable_cancel_at_the_publication_gate_and_show_each_new_export_once() {
    let mut fixture = Fixture::create();
    let (observer, entered, release) = create_barrier(true);
    fixture.engine = fixture.engine.with_export_observer(observer);
    let mut harness = create_harness(&fixture, [1000.0, 800.0], 1.0);
    harness.get_by_label("Compile F8").click();
    harness.run();
    fixture.engine.tick().unwrap();
    entered.recv().unwrap();
    harness.run();
    assert!(fixture.handle.export_status().unwrap().publishing);
    assert!(harness.get_by_label("Cancel export").accesskit_node().is_disabled());
    harness.key_press(Key::Escape);
    harness.get_by_label("Cancel export").click();
    harness.run();
    assert!(!fixture.handle.export_status().unwrap().cancel_requested);
    release.send(()).unwrap();
    fixture.engine.wait_for_export().unwrap();
    harness.run();
    assert!(harness.query_by_label_contains("Done:").is_some());
    assert!(fixture.directory.path().join("export/00001.wav").is_file());
    harness.get_by_label("Close").click();
    harness.run();
    assert!(harness.query_by_label("Export audiobook").is_none());
    fixture.execute(EngineCommand::Compile).unwrap();
    fixture.engine.wait_for_export().unwrap();
    harness.run();
    assert!(harness.query_by_label_contains("Done:").is_some());
    harness.get_by_label("Close").click();
    harness.run();
    assert!(harness.query_by_label("Export audiobook").is_none());
}

#[test]
fn retain_external_export_errors_and_restore_the_covered_edit_draft() {
    let mut fixture = Fixture::create();
    let output = fixture.directory.path().join("export");
    std::fs::create_dir(&output).unwrap();
    std::fs::write(output.join("previous.wav"), b"previous published book").unwrap();
    let mut harness = create_harness(&fixture, [1000.0, 800.0], 1.0);
    harness.key_press(Key::F5);
    harness.run();
    set_text(&mut harness, "Book title", "Do not discard this draft");
    let missing_path = fixture.directory.path().join(super::support::chunk(1).0);
    let recording = std::fs::read(&missing_path).unwrap();
    std::fs::remove_file(&missing_path).unwrap();
    fixture.execute(EngineCommand::Compile).unwrap();
    fixture.engine.wait_for_export().unwrap();
    harness.run();

    assert!(harness.query_by_label_contains("Compilation failed; previous export unchanged").is_some());
    assert_eq!(std::fs::read(output.join("previous.wav")).unwrap(), b"previous published book");
    assert!(harness.query_by_label("Book title").is_none());
    harness.get_by_label("Close").click();
    harness.run();
    assert_eq!(harness.get_by_label("Book title").value().as_deref(), Some("Do not discard this draft"));
    harness.get_by_label("Cancel").click();
    harness.run();
    assert_ne!(fixture.load_project().meta.title, "Do not discard this draft");
    std::fs::write(missing_path, recording).unwrap();
    fixture.execute(EngineCommand::Compile).unwrap();
    fixture.engine.wait_for_export().unwrap();
    harness.run();
    assert!(harness.query_by_label_contains("Done:").is_some());
    assert!(harness.query_by_label_contains("Compilation failed").is_none());
}

#[test]
fn keep_published_warnings_and_command_failures_visible_until_explicit_close() {
    let mut status = ExportStatus::default();
    status.apply(ExportUpdate::Started { cancel_requested: false }).unwrap();
    status.apply(ExportUpdate::Finished { at: Instant::now(), outcome: ExportOutcome::Published {
        path: "/synthetic/export".into(), warnings: vec!["Synthetic cleanup warning".into()],
    } }).unwrap();
    let mut dialog = ExportDialog::default();
    dialog.sync(&status);
    let mut harness = Harness::builder().with_size(Vec2::new(400.0, 300.0))
        .with_theme(Theme::Light).with_context_setup(configure_context)
        .build_ui_state(move |ui, dialog: &mut ExportDialog| { assert!(!dialog.show(ui.ctx(), &status)); }, dialog);
    harness.run();
    assert!(harness.query_by_label_contains("Done: /synthetic/export (with warnings)").is_some());
    assert!(harness.query_by_label_contains("Export was published, but: Synthetic cleanup warning").is_some());
    assert!(harness.query_by_label("Cancel export").is_none());
    assert!(Rect::from_min_size(Pos2::ZERO, Vec2::new(400.0, 300.0)).contains_rect(harness.get_by_label("Close").rect()));
    harness.run();
    assert!(harness.state().is_open());
    harness.get_by_label("Close").click();
    harness.run();
    assert!(!harness.state().is_open());

    let mut dialog = ExportDialog::default();
    dialog.start_request(31);
    assert!(dialog.accept_reply(&iamreader_engine::engine::EngineEvent::CommandCompleted {
        request_id: 31, result: Err(iamreader_engine::engine::CommandFailure { message: "Synthetic queue failure".into(), field: None }),
    }));
    let mut harness = Harness::builder().with_theme(Theme::Light).with_context_setup(configure_context).build_ui_state(
        |ui, dialog: &mut ExportDialog| { assert!(!dialog.show(ui.ctx(), &ExportStatus::default())); }, dialog);
    harness.run();
    assert!(harness.query_by_label("Synthetic queue failure").is_some());
    harness.key_press(Key::Escape);
    harness.run();
    assert!(!harness.state().is_open());
}

#[test]
fn recognize_a_new_export_when_terminal_notifications_were_coalesced() {
    let mut status = ExportStatus::default();
    status.apply(ExportUpdate::Started { cancel_requested: false }).unwrap();
    let mut harness = Harness::builder().with_theme(Theme::Light).with_context_setup(configure_context).build_ui_state(
        |ui, state: &mut (ExportDialog, ExportStatus, usize)| {
            state.0.sync(&state.1);
            if state.0.show(ui.ctx(), &state.1) {
                state.1.apply(ExportUpdate::Cancelling).unwrap();
                state.2 += 1;
            }
        }, (ExportDialog::default(), status, 0));
    harness.run();
    harness.get_by_label("Cancel export").click();
    harness.run();
    assert_eq!(harness.state().2, 1);
    assert!(harness.get_by_label("Cancel export").accesskit_node().is_disabled());

    // The event mailbox holds one signal: a hidden window can miss the terminal snapshot.
    let status = &mut harness.state_mut().1;
    status.apply(ExportUpdate::Finished { at: Instant::now(), outcome: ExportOutcome::Cancelled }).unwrap();
    status.apply(ExportUpdate::Started { cancel_requested: false }).unwrap();
    harness.run();

    assert!(!harness.get_by_label("Cancel export").accesskit_node().is_disabled());
    harness.get_by_label("Cancel export").click();
    harness.run();
    assert_eq!(harness.state().2, 2);
}
