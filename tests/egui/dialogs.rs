use super::{create_harness, set_text, Fixture};
use eframe::egui::{self, Key, Vec2};
use egui_kittest::{Harness, kittest::{NodeT, Queryable}};
use iamreader::egui_ui::{configure_context, dialogs::{DialogKind, EditDialog}};
use iamreader_engine::engine::{CommandFailure, CommandOutcome, EngineCommand, EngineEvent, ProjectEdit};

struct ModalFixture {
    dialog: EditDialog,
    sent: Vec<(u64, EngineCommand)>,
    next_id: u64,
}

fn create_modal(fixture: &Fixture, kind: DialogKind, size: [f32; 2]) -> Harness<'static, ModalFixture> {
    let dialog = EditDialog::open(kind, &fixture.handle.snapshot().unwrap()).unwrap();
    let mut harness = Harness::builder().with_size(Vec2::from(size)).with_max_steps(16)
        .with_theme(egui::Theme::Light).with_context_setup(configure_context)
        .build_ui_state(|ui, state: &mut ModalFixture| {
            if !state.dialog.is_open() { return; }
            if let Some(command) = state.dialog.show(ui.ctx()) {
                state.next_id += 1;
                state.sent.push((state.next_id, command.clone()));
                state.dialog.start_request(state.next_id, command);
                ui.ctx().request_repaint();
            }
        }, ModalFixture { dialog, sent: Vec::new(), next_id: 0 });
    harness.run();

    harness
}

fn select_marker(harness: &mut Harness<'_, ModalFixture>, alias: &str) {
    harness.get_by_label("Marker").click();
    harness.run();
    harness.get_by_label(alias).click();
    harness.run();
}

fn reply(harness: &mut Harness<'_, ModalFixture>, request_id: u64, result: Result<CommandOutcome, CommandFailure>) {
    assert!(harness.state_mut().dialog.accept_reply(&EngineEvent::CommandCompleted { request_id, result }));
    harness.run();
}

#[test]
fn keep_independent_marker_drafts_and_save_only_the_selected_one() {
    let fixture = Fixture::create();
    let mut harness = create_modal(&fixture, DialogKind::Markers, [1000.0, 900.0]);
    for (label, value) in [
        ("Title", "Changed chapter"), ("Hint", "Changed hint"), ("Shortcut (0–9)", "9"),
        ("Begin sound audio", "begin.wav"), ("Begin sound kind", "underlay"),
        ("Begin sound reduction", "99"), ("Begin sound repeat", "invalid unsaved count"),
        ("End sound audio", "end.wav"), ("End sound kind", "add"),
        ("End sound reduction", "12"), ("End sound repeat", "0"),
    ] { set_text(&mut harness, label, value); }
    set_text(&mut harness, "New marker alias", " beta ");
    harness.get_by_label("Add").click();
    harness.run();
    set_text(&mut harness, "Title", "Beta saved");
    harness.get_by_label("Save").click();
    harness.run();

    let (request_id, EngineCommand::SaveProjectEdit(edit)) = harness.state().sent.last().unwrap() else { panic!("Expected marker save"); };
    let ProjectEdit::Marker(form) = &**edit else { panic!("Expected marker data"); };
    assert_eq!(form.marker, "beta");
    assert_eq!(form.title, "Beta saved");
    assert!(harness.state().dialog.is_busy());
    let request_id = *request_id;
    reply(&mut harness, request_id, Ok(CommandOutcome::Saved { warning: None }));
    assert!(harness.state().dialog.is_open());
    assert!(harness.get_by_label_contains("Saved: beta").rect().bottom() <= 900.0);
    select_marker(&mut harness, "chapter");

    for (label, value) in [
        ("Title", "Changed chapter"), ("Hint", "Changed hint"), ("Shortcut (0–9)", "9"),
        ("Begin sound audio", "begin.wav"), ("Begin sound kind", "underlay"),
        ("Begin sound reduction", "99"), ("Begin sound repeat", "invalid unsaved count"),
        ("End sound audio", "end.wav"), ("End sound kind", "add"),
        ("End sound reduction", "12"), ("End sound repeat", "0"),
    ] { assert_eq!(harness.get_by_label(label).value().as_deref(), Some(value), "{label}"); }
    select_marker(&mut harness, "chapter");
    assert_eq!(harness.get_by_label("Title").value().as_deref(), Some("Changed chapter"));
    set_text(&mut harness, "Begin sound repeat", "-1");
    harness.get_by_label("Save").click();
    harness.run();
    assert_eq!(harness.state().sent.len(), 2);
}

#[test]
fn validate_aliases_shortcuts_and_asset_numbers_inline_without_submitting() {
    let fixture = Fixture::create();
    let mut harness = create_modal(&fixture, DialogKind::Markers, [1000.0, 900.0]);
    for alias in ["chapter", " chapter ", "  "] {
        set_text(&mut harness, "New marker alias", alias);
        harness.get_by_label("Add").click();
        harness.run();
        assert!(harness.query_by_label_contains(if alias.trim().is_empty() { "Enter a marker alias" } else { "This alias already exists" }).is_some());
        assert!(harness.state().sent.is_empty());
    }
    set_text(&mut harness, "New marker alias", "beta");
    harness.get_by_label("Add").click();
    harness.run();
    for (field, value, error, restored) in [
        ("Shortcut (0–9)", "1", "also assigned", ""),
        ("Shortcut (0–9)", "x", "ASCII digit", ""),
        ("Begin sound kind", "replace", "Choose add or underlay", "add"),
        ("Begin sound reduction", "101", "0 to 100", ""),
        ("Begin sound repeat", "-1", "0 or greater", "1"),
        ("End sound reduction", "abc", "0 to 100", ""),
        ("End sound repeat", "1.5", "whole repeat count", "1"),
    ] {
        set_text(&mut harness, field, value);
        harness.get_by_label("Save").click();
        harness.run();
        assert!(harness.query_all_by_label_contains(error).count() >= 1, "{field}");
        assert!(harness.state().sent.is_empty(), "{field}");
        assert!(!harness.state().dialog.is_busy());
        set_text(&mut harness, field, restored);
    }
}

#[test]
fn ignore_duplicate_save_cancel_escape_and_unrelated_replies_until_acknowledged() {
    let fixture = Fixture::create();
    for kind in [DialogKind::Project, DialogKind::Chunk, DialogKind::Markers] {
        let mut harness = create_modal(&fixture, kind, [800.0, 800.0]);
        harness.get_by_label("Save").click();
        harness.run();
        assert!(harness.state().dialog.is_busy(), "Not busy: {kind:?}, sent: {:?}", harness.state().sent);
        assert!(harness.get_by_label("Save").accesskit_node().is_disabled());
        harness.get_by_label("Save").click();
        harness.get_by_label("Cancel").click();
        harness.key_press(Key::Escape);
        harness.run();
        assert!(harness.state().dialog.is_open());
        assert_eq!(harness.state().sent.len(), 1);
        assert!(!harness.state_mut().dialog.accept_reply(&EngineEvent::CommandCompleted { request_id: 999, result: Ok(CommandOutcome::Completed) }));
        assert!(harness.state().dialog.is_busy());

        reply(&mut harness, 1, Err(CommandFailure { message: "Disk full".into(), field: None }));
        assert!(!harness.state().dialog.is_busy());
        assert!(harness.get_by_label("Disk full").rect().bottom() < 800.0);
        harness.key_press(Key::Escape);
        harness.run();
        assert!(!harness.state().dialog.is_open());
    }
}

#[test]
fn preserve_input_on_failure_and_warning_and_close_metadata_only_after_success() {
    let fixture = Fixture::create();
    for (kind, title_label) in [(DialogKind::Project, "Book title"), (DialogKind::Chunk, "Title"), (DialogKind::Markers, "Title")] {
        let mut harness = create_modal(&fixture, kind, [800.0, 800.0]);
        set_text(&mut harness, title_label, "Keep edited title");
        set_text(&mut harness, "Hint", "Keep edited hint");
        harness.get_by_label("Save").click();
        harness.run();
        reply(&mut harness, 1, Err(CommandFailure { message: "Read-only project".into(), field: None }));
        assert_eq!(harness.get_by_label(title_label).value().as_deref(), Some("Keep edited title"));
        assert_eq!(harness.get_by_label("Hint").value().as_deref(), Some("Keep edited hint"));
        harness.get_by_label("Save").click();
        harness.run();
        reply(&mut harness, 2, Ok(CommandOutcome::Saved { warning: Some("directory sync failed".into()) }));
        assert!(harness.state().dialog.is_open());
        assert!(!harness.state().dialog.is_busy());
        assert!(harness.query_by_label_contains("Saved, but directory sync failed").is_some());
        assert_eq!(harness.get_by_label(title_label).value().as_deref(), Some("Keep edited title"));
        harness.get_by_label("Save").click();
        harness.run();
        reply(&mut harness, 3, Ok(CommandOutcome::Saved { warning: None }));
        assert_eq!(harness.state().dialog.is_open(), matches!(kind, DialogKind::Markers));
        assert_eq!(harness.state().sent.len(), 3);
    }
}

#[test]
fn reach_save_buttons_in_narrow_hidpi_dialogs_and_preserve_project_tabs() {
    let fixture = Fixture::create();
    for size in [[400.0, 300.0], [640.0, 480.0], [800.0, 800.0]] {
        for (kind, label) in [(DialogKind::Project, "Book title"), (DialogKind::Markers, "Marker")] {
            let mut harness = create_modal(&fixture, kind, size);
            harness.set_pixels_per_point(1.25);
            harness.run();
            let save = harness.get_by_label("Save").rect();
            assert!(egui::Rect::from_min_size(egui::Pos2::ZERO, Vec2::from(size)).contains_rect(save), "{kind:?} {size:?}: {save:?}");
            assert!(save.height() >= 32.0, "{kind:?} {size:?}: Save height {}", save.height());
            let painted = harness.output().shapes.iter().find_map(|shape| {
                let egui::Shape::Text(text) = &shape.shape else { return None; };
                (text.galley.text() == label).then_some(text)
            }).unwrap();
            assert!(painted.galley.job.sections.iter().all(|section| section.format.font_id.size >= 16.0), "Small font: {label}");
            harness.get_by_label("Save").click();
            harness.run();
            assert!(harness.state().dialog.is_busy());
            reply(&mut harness, 1, Err(CommandFailure { message: "Synthetic failure with a long path: /temporary/project/cannot/be/saved. Try again after freeing disk space.".into(), field: None }));
            assert!(egui::Rect::from_min_size(egui::Pos2::ZERO, Vec2::from(size)).contains_rect(harness.get_by_label("Save").rect()));
        }
    }
    let mut harness = create_modal(&fixture, DialogKind::Project, [800.0, 800.0]);
    set_text(&mut harness, "Book title", "Книга с вкладками");
    harness.get_by_label("Export").click();
    harness.run();
    harness.get_by_label("MP3").click();
    set_text(&mut harness, "Cover image", "cover-custom.png");
    harness.get_by_label("Split by sections").click();
    harness.get_by_label("Denoise").click();
    harness.run();
    harness.get_by_label("Book").click();
    harness.run();
    assert_eq!(harness.get_by_label("Book title").value().as_deref(), Some("Книга с вкладками"));
    harness.get_by_label("Save").click();
    harness.run();
    let (_, EngineCommand::SaveProjectEdit(edit)) = harness.state().sent.last().unwrap() else { panic!("Expected project save"); };
    let ProjectEdit::Meta(data) = &**edit else { panic!("Expected project metadata"); };
    assert_eq!(data.format_audio, "mp3");
    assert_eq!(data.cover, "cover-custom.png");
    assert!(data.denoise && data.section_split);
}

#[test]
fn retain_the_form_when_the_engine_queue_is_full_and_allow_a_fresh_retry() {
    let mut fixture = Fixture::create();
    let tickets: Vec<_> = (0..64).map(|_| fixture.handle.submit(EngineCommand::Stop).unwrap()).collect();
    let mut harness = create_harness(&fixture, [1000.0, 800.0], 1.0);
    harness.key_press(Key::F5);
    harness.run();
    set_text(&mut harness, "Book title", "Retry this title");
    harness.get_by_label("Save").click();
    harness.run();
    assert!(harness.query_by_label_contains("Cannot submit changes").is_some());
    assert!(!harness.get_by_label("Save").accesskit_node().is_disabled());
    assert_eq!(harness.get_by_label("Book title").value().as_deref(), Some("Retry this title"));
    fixture.engine.tick().unwrap();
    for ticket in tickets { ticket.recv().unwrap(); }
    harness.get_by_label("Save").click();
    harness.run();
    fixture.engine.tick().unwrap();
    harness.run();
    assert!(harness.query_by_label("Book title").is_none());
    assert_eq!(fixture.load_project().meta.title, "Retry this title");
}

#[test]
fn edit_the_double_clicked_row_and_reject_a_structurally_stale_save() {
    let mut fixture = Fixture::create();
    let mut harness = create_harness(&fixture, [1000.0, 800.0], 1.0);
    harness.get_by_label_contains("Synthetic fragment 1").click();
    harness.step();
    harness.get_by_label_contains("Synthetic fragment 1").click();
    harness.run();
    assert_eq!(harness.get_by_label("Title").value().as_deref(), Some("Synthetic fragment 1"));
    set_text(&mut harness, "Title", "Do not save to the wrong row");
    fixture.execute(EngineCommand::DeleteChunk { chunk: super::support::chunk(3) }).unwrap();
    harness.get_by_label("Save").click();
    harness.run();
    fixture.engine.tick().unwrap();
    harness.run();

    assert!(harness.query_by_label_contains("selection is no longer valid").is_some());
    assert_eq!(harness.get_by_label("Title").value().as_deref(), Some("Do not save to the wrong row"));
    assert_eq!(fixture.load_project().files[0].title, "Synthetic fragment 1");
    harness.get_by_label("Close dialog").click();
    harness.run();
    assert!(harness.query_by_label("Chunk Settings").is_none());
}

#[test]
fn persist_real_saves_retry_after_io_failure_and_discard_only_unsaved_marker_additions() {
    let mut fixture = Fixture::create();
    let mut harness = create_harness(&fixture, [1000.0, 800.0], 1.0);
    harness.key_press(Key::F5);
    harness.run();
    set_text(&mut harness, "Book title", "Saved book");
    let backup = fixture.project_path.with_extension("backup");
    std::fs::rename(&fixture.project_path, &backup).unwrap();
    std::fs::create_dir(&fixture.project_path).unwrap();
    harness.get_by_label("Save").click();
    harness.run();
    fixture.engine.tick().unwrap();
    harness.run();
    assert_eq!(harness.get_by_label("Book title").value().as_deref(), Some("Saved book"));
    assert!(!harness.get_by_label("Save").accesskit_node().is_disabled());
    std::fs::remove_dir(&fixture.project_path).unwrap();
    std::fs::rename(backup, &fixture.project_path).unwrap();
    harness.get_by_label("Save").click();
    harness.run();
    fixture.engine.tick().unwrap();
    harness.run();
    assert!(harness.query_by_label("Book title").is_none());
    assert_eq!(fixture.load_project().meta.title, "Saved book");
    harness.key_press(Key::F4);
    harness.run();
    set_text(&mut harness, "New marker alias", "saved-marker");
    harness.get_by_label("Add").click();
    harness.run();
    set_text(&mut harness, "Title", "Committed marker");
    harness.get_by_label("Save").click();
    harness.run();
    fixture.engine.tick().unwrap();
    harness.run();
    assert!(fixture.load_project().markers.contains_key("saved-marker"), "{:?}", fixture.load_project().markers);
    set_text(&mut harness, "New marker alias", "discarded-marker");
    harness.get_by_label("Add").click();
    harness.run();
    harness.get_by_label("Cancel").click();
    harness.run();

    assert_eq!(fixture.load_project().markers["saved-marker"].title, "Committed marker");
    assert!(!fixture.load_project().markers.contains_key("discarded-marker"));
    assert!(fixture.audio.borrow().calls.is_empty());
}

#[test]
fn keep_captured_chunk_and_delete_targets_and_prevent_background_actions() {
    let mut fixture = Fixture::create();
    fixture.select(1);
    let mut harness = create_harness(&fixture, [1000.0, 800.0], 1.0);
    harness.get_by_label_contains("Synthetic fragment 1").click();
    harness.step();
    harness.get_by_label_contains("Synthetic fragment 1").click();
    harness.run();
    set_text(&mut harness, "Title", "Edited first recording");
    set_text(&mut harness, "Hint", "Updated transcription");
    fixture.select(3);
    harness.run();
    for key in [Key::R, Key::Num1, Key::F1, Key::F4, Key::F5, Key::F8, Key::F10, Key::ArrowDown] {
        harness.key_press(key);
        harness.run();
    }
    fixture.engine.tick().unwrap();
    assert!(fixture.audio.borrow().calls.is_empty());
    assert!(harness.query_by_label("Export audiobook").is_none());
    harness.get_by_label("Save").click();
    harness.run();
    fixture.engine.tick().unwrap();
    harness.run();
    assert_eq!(fixture.load_project().files[0].title, "Edited first recording");
    assert_eq!(fixture.load_project().files[0].hint, "Updated transcription");
    harness.key_press(Key::Delete);
    harness.run();
    fixture.select(1);
    harness.run();
    harness.get_by_label("Delete").click();
    harness.run();
    fixture.engine.tick().unwrap();
    harness.run();
    assert!(!fixture.load_project().files.iter().any(|file| file.path == super::support::chunk(3).0));
    assert_eq!(fixture.load_project().files[0].title, "Edited first recording");
}

#[test]
fn reject_close_during_save_then_shutdown_and_release_all_sessions() {
    let mut fixture = Fixture::create();
    fixture.execute(EngineCommand::Play).unwrap();
    let mut harness = create_harness(&fixture, [1000.0, 800.0], 1.0);
    harness.key_press(Key::F5);
    harness.run();
    set_text(&mut harness, "Book title", "Finish before closing");
    harness.get_by_label("Save").click();
    harness.run();
    harness.input_mut().viewports.get_mut(&egui::ViewportId::ROOT).unwrap().events.push(egui::ViewportEvent::Close);
    harness.step();
    assert!(harness.output().viewport_output[&egui::ViewportId::ROOT].commands.contains(&egui::ViewportCommand::CancelClose));
    harness.input_mut().viewports.get_mut(&egui::ViewportId::ROOT).unwrap().events.clear();
    fixture.engine.tick().unwrap();
    harness.run();
    assert!(!fixture.handle.snapshot().unwrap().stopped);
    assert_eq!(fixture.load_project().meta.title, "Finish before closing");
    harness.input_mut().viewports.get_mut(&egui::ViewportId::ROOT).unwrap().events.push(egui::ViewportEvent::Close);
    harness.step();
    fixture.engine.tick().unwrap();
    harness.input_mut().viewports.get_mut(&egui::ViewportId::ROOT).unwrap().events.clear();
    harness.step();

    assert!(fixture.handle.snapshot().unwrap().stopped);
    assert_eq!(fixture.audio.borrow().active_players, 0);
    assert!(harness.output().viewport_output[&egui::ViewportId::ROOT].commands.contains(&egui::ViewportCommand::Close));
}
