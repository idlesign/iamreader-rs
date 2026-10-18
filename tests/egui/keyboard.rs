use super::{create_harness, Fixture};
use eframe::egui::{Event, Key, Modifiers};
use egui_kittest::kittest::Queryable;

#[test]
fn route_english_and_russian_keys_once_without_modifier_shortcuts() {
    for text in ["r", "R", "к", "К"] {
        let mut fixture = Fixture::create();
        let mut harness = create_harness(&fixture, [1000.0, 700.0], 1.0);
        harness.input_mut().events.extend([
            Event::Key { key: Key::R, physical_key: Some(Key::R), pressed: true, repeat: false, modifiers: Modifiers::NONE },
            Event::Text(text.into()),
        ]);
        harness.run();
        fixture.engine.tick().unwrap();

        assert!(fixture.handle.snapshot().unwrap().is_recording, "{text}");
        assert_eq!(fixture.audio.borrow().recorded_paths.len(), 1, "{text}");
    }
    let mut fixture = Fixture::create();
    let mut harness = create_harness(&fixture, [1000.0, 700.0], 1.0);
    harness.key_press_modifiers(Modifiers::CTRL, Key::R);
    harness.run();
    fixture.engine.tick().unwrap();
    assert!(fixture.audio.borrow().calls.is_empty());
}

#[test]
fn keep_recording_shortcuts_out_of_text_fields_and_release_focus_on_escape() {
    let mut fixture = Fixture::create();
    let mut harness = create_harness(&fixture, [1000.0, 700.0], 1.0);
    harness.get_by_label("Search").focus();
    harness.get_by_label("Search").type_text("rеуаш");
    harness.run();
    harness.key_press(Key::R);
    harness.run();
    fixture.engine.tick().unwrap();

    assert!(fixture.audio.borrow().calls.is_empty());
    harness.key_press(Key::Escape);
    harness.run();
    harness.key_press(Key::R);
    harness.run();
    fixture.engine.tick().unwrap();

    assert!(fixture.handle.snapshot().unwrap().is_recording);
    assert_eq!(fixture.audio.borrow().recorded_paths.len(), 1);
}

#[test]
fn navigate_with_arrows_and_show_toc_shortcuts_and_marker_keys() {
    let mut fixture = Fixture::create();
    let mut harness = create_harness(&fixture, [1000.0, 800.0], 1.0);
    harness.key_press(Key::ArrowDown);
    harness.run();
    fixture.engine.tick().unwrap();
    assert_eq!(fixture.handle.snapshot().unwrap().selected_chunk, Some(super::support::chunk(2)));
    harness.key_press(Key::ArrowUp);
    harness.run();
    fixture.engine.tick().unwrap();
    assert_eq!(fixture.handle.snapshot().unwrap().selected_chunk, Some(super::support::chunk(3)));
    harness.key_press(Key::Num1);
    harness.run();
    fixture.engine.tick().unwrap();
    assert!(fixture.load_project().files[2].markers.is_empty());
    harness.key_press(Key::F10);
    harness.run();
    assert!(harness.query_by_label("Table of contents").is_some());
    harness.key_press(Key::Escape);
    harness.run();
    harness.get_by_label("Shortcuts F1").click();
    harness.run();
    assert!(harness.query_by_label("Keyboard shortcuts").is_some());
    harness.get_by_label("Close dialog").click();
    harness.run();
    harness.key_press(Key::F1);
    harness.run();
    assert!(harness.query_by_label("Keyboard shortcuts").is_some());
    assert!(harness.query_by_label_contains("1 — marker").is_some());
    harness.get_by_label("Close dialog").click();
    harness.run();
    harness.key_press(Key::F5);
    harness.run();
    harness.get_by_label("Book title").focus();
    harness.get_by_label("Book title").type_text("r7");
    harness.run();
    assert!(!fixture.handle.snapshot().unwrap().is_recording);
    harness.key_press(Key::Escape);
    harness.run();
    assert!(harness.query_by_label("Book title").is_none());
    harness.key_press(Key::R);
    harness.run();
    fixture.engine.tick().unwrap();
    assert!(fixture.handle.snapshot().unwrap().is_recording);
}
