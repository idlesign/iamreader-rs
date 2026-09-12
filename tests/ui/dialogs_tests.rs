use super::{DialogController, DialogReply};
use crate::project::{MarkerSettings, MarkerSettingsData};
use crate::ui::ui::{MainWindow, UIState};
use crate::utils::keyboard::{Action, DialogEdit};
use crossbeam_channel::{Receiver, TryRecvError};
use slint::platform::software_renderer::{MinimalSoftwareWindow, RepaintBufferType};
use slint::platform::{Key, Platform, WindowAdapter, WindowEvent};
use slint::{ComponentHandle, Model};
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::{Arc, Mutex};

struct TestPlatform(Rc<MinimalSoftwareWindow>);

impl Platform for TestPlatform {
    fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, slint::PlatformError> {
        Ok(self.0.clone())
    }
}

struct Fixture {
    window: MainWindow,
    adapter: Rc<MinimalSoftwareWindow>,
    state: Arc<Mutex<UIState>>,
    controller: Rc<RefCell<DialogController>>,
    actions: Receiver<Action>,
}

impl Fixture {
    fn new() -> Self {
        let adapter = MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer);
        slint::platform::set_platform(Box::new(TestPlatform(adapter.clone()))).unwrap();
        let window = MainWindow::new().unwrap();
        let state = Arc::new(Mutex::new(UIState::default()));
        state.lock().unwrap().dialog_marker_snapshot = HashMap::from([
            (
                "alpha".into(),
                MarkerSettings {
                    title: "Alpha saved".into(),
                    hint: "Alpha hint".into(),
                    author: "Hidden author".into(),
                    year: "2001".into(),
                    shortcut: Some("1".into()),
                    ..MarkerSettings::default()
                },
            ),
            (
                "beta".into(),
                MarkerSettings {
                    title: "Beta saved".into(),
                    shortcut: Some("2".into()),
                    ..MarkerSettings::default()
                },
            ),
        ]);
        let (sender, actions) = crossbeam_channel::unbounded();
        let controller = DialogController::install(&window, state.clone(), sender);
        window.show().unwrap();
        adapter.set_size(slint::LogicalSize::new(1000.0, 800.0));
        let fixture = Self {
            window,
            adapter,
            state,
            controller,
            actions,
        };
        fixture.draw();
        fixture
    }

    fn draw(&self) {
        slint::platform::update_timers_and_animations();
        self.adapter.request_redraw();
        let size = self.adapter.size();
        let mut pixels = slint::SharedPixelBuffer::<slint::Rgb8Pixel>::new(size.width, size.height);
        self.adapter.draw_if_needed(|renderer| {
            renderer.render(pixels.make_mut_slice(), size.width as usize);
        });
    }

    fn open_markers(&self) {
        self.state.lock().unwrap().dialog_markers_open = true;
        self.controller.borrow_mut().sync(&self.window);
        self.draw();
        assert!(self.window.get_dialog_markers_open());
    }

    fn open_meta(&self) {
        self.window.set_dialog_project_title("Edited book".into());
        self.window
            .set_dialog_project_author("Edited author".into());
        self.window.set_dialog_project_year("2026".into());
        self.window
            .set_dialog_project_hint("Unsaved book hint".into());
        self.window.set_dialog_project_reader("Reader".into());
        self.window.set_dialog_project_format_audio("wav".into());
        self.window.set_dialog_project_open(true);
        self.draw();
    }

    fn open_chunk(&self) {
        self.controller
            .borrow_mut()
            .open_chunk(&self.window, "first.wav".into());
        self.window.set_dialog_chunk_file_index(1);
        self.window
            .set_dialog_chunk_title("Edited recording".into());
        self.window
            .set_dialog_chunk_author("Recording author".into());
        self.window.set_dialog_chunk_year("2025".into());
        self.window
            .set_dialog_chunk_hint("Unsaved transcription".into());
        self.window.set_dialog_chunk_open(true);
        self.draw();
    }

    fn marker_names(&self) -> Vec<String> {
        let model = self.window.get_dialog_markers_list();
        (0..model.row_count())
            .map(|index| model.row_data(index).unwrap().to_string())
            .collect()
    }

    fn selected_marker(&self) -> String {
        self.window
            .get_dialog_markers_list()
            .row_data(self.window.get_dialog_markers_selected_index() as usize)
            .unwrap()
            .to_string()
    }

    fn save_marker(&self) {
        let window = &self.window;
        window.invoke_dialog_markers_save(
            self.selected_marker().into(),
            window.get_dialog_markers_title(),
            window.get_dialog_markers_hint(),
            window.get_dialog_markers_shortcut(),
            window.get_dialog_markers_begin_audio(),
            window.get_dialog_markers_begin_kind(),
            window.get_dialog_markers_begin_reduction(),
            window.get_dialog_markers_begin_repeat(),
            window.get_dialog_markers_end_audio(),
            window.get_dialog_markers_end_kind(),
            window.get_dialog_markers_end_reduction(),
            window.get_dialog_markers_end_repeat(),
            window.get_dialog_markers_section(),
        );
    }

    fn save_meta(&self) {
        let window = &self.window;
        window.invoke_dialog_project_save(
            window.get_dialog_project_title(),
            window.get_dialog_project_author(),
            window.get_dialog_project_year(),
            window.get_dialog_project_hint(),
            window.get_dialog_project_reader(),
            window.get_dialog_project_format_audio(),
            window.get_dialog_project_normalize(),
            window.get_dialog_project_cover(),
            window.get_dialog_project_section_split(),
            window.get_dialog_project_denoise(),
        );
    }

    fn save_chunk(&self) {
        let window = &self.window;
        window.invoke_dialog_chunk_save(
            window.get_dialog_chunk_file_index(),
            window.get_dialog_chunk_title(),
            window.get_dialog_chunk_author(),
            window.get_dialog_chunk_year(),
            window.get_dialog_chunk_hint(),
        );
    }

    fn next_save(&self) -> (u64, DialogEdit) {
        match self.actions.try_recv().unwrap() {
            Action::SaveDialog { request_id, edit } => (request_id, *edit),
            action => panic!("Unexpected action: {action:?}"),
        }
    }

    fn no_action(&self) {
        assert!(matches!(self.actions.try_recv(), Err(TryRecvError::Empty)));
    }

    fn reply(&self, request_id: u64, result: Result<Option<String>, String>, field: Option<&str>) {
        self.state.lock().unwrap().dialog_save_result = Some(DialogReply {
            request_id,
            result,
            field: field.map(str::to_owned),
        });
        self.controller.borrow_mut().sync(&self.window);
    }

    fn escape(&self) {
        self.draw();
        self.window
            .window()
            .dispatch_event(WindowEvent::KeyPressed {
                text: Key::Escape.into(),
            });
        self.window
            .window()
            .dispatch_event(WindowEvent::KeyReleased {
                text: Key::Escape.into(),
            });
        self.draw();
    }
}

fn with_fixture(test: impl FnOnce(Fixture) + Send + 'static) {
    // Each Slint platform belongs to a fresh thread, without an event loop, display or audio device.
    std::thread::spawn(move || test(Fixture::new()))
        .join()
        .unwrap();
}

fn marker_edit(edit: DialogEdit) -> MarkerSettingsData {
    match edit {
        DialogEdit::Marker(form) => form,
        edit => panic!("Expected one selected marker, got {edit:?}"),
    }
}

#[test]
fn selected_marker_save_preserves_other_invalid_local_drafts_and_stays_open() {
    with_fixture(|fixture| {
        fixture.open_markers();
        assert_eq!(fixture.marker_names(), ["alpha", "beta"]);
        fixture
            .window
            .set_dialog_markers_title("Alpha local draft".into());
        fixture
            .window
            .set_dialog_markers_begin_repeat("invalid unsaved value".into());
        fixture.window.invoke_dialog_markers_marker_selected(1);
        assert_eq!(fixture.window.get_dialog_markers_title(), "Beta saved");
        fixture
            .window
            .set_dialog_markers_title("Beta selected save".into());
        fixture.save_marker();
        let (request, edit) = fixture.next_save();
        let form = marker_edit(edit);
        assert_eq!(form.marker, "beta");
        assert_eq!(form.title, "Beta selected save");
        assert!(fixture.window.get_dialog_busy());
        fixture.no_action();
        fixture.reply(request, Ok(None), None);
        assert!(fixture.window.get_dialog_markers_open());
        assert!(!fixture.window.get_dialog_busy());
        assert!(fixture
            .window
            .get_dialog_markers_status_message()
            .contains("Saved"));
        fixture.window.invoke_dialog_markers_marker_selected(0);
        assert_eq!(
            fixture.window.get_dialog_markers_title(),
            "Alpha local draft"
        );
        assert_eq!(
            fixture.window.get_dialog_markers_begin_repeat(),
            "invalid unsaved value"
        );
        fixture.window.invoke_dialog_markers_marker_selected(1);
        assert_eq!(
            fixture.window.get_dialog_markers_title(),
            "Beta selected save"
        );
        fixture.no_action();
    });
}

#[test]
fn switching_markers_including_current_selection_keeps_all_unsaved_fields() {
    with_fixture(|fixture| {
        fixture.open_markers();
        fixture
            .window
            .set_dialog_markers_title("Changed title".into());
        fixture
            .window
            .set_dialog_markers_hint("Changed hint".into());
        fixture.window.set_dialog_markers_shortcut("9".into());
        fixture
            .window
            .set_dialog_markers_begin_audio("new begin.wav".into());
        fixture
            .window
            .set_dialog_markers_begin_kind("underlay".into());
        fixture
            .window
            .set_dialog_markers_begin_reduction("99".into());
        fixture.window.set_dialog_markers_begin_repeat("-1".into());
        fixture
            .window
            .set_dialog_markers_end_audio("new end.wav".into());
        fixture.window.set_dialog_markers_end_reduction("12".into());
        fixture.window.set_dialog_markers_end_repeat("0".into());
        fixture.window.set_dialog_markers_section(true);
        fixture.window.invoke_dialog_markers_marker_selected(0);
        assert_eq!(fixture.window.get_dialog_markers_title(), "Changed title");
        fixture.window.invoke_dialog_markers_marker_selected(1);
        fixture.window.invoke_dialog_markers_marker_selected(0);
        fixture.save_marker();
        let (_, edit) = fixture.next_save();
        let form = marker_edit(edit);
        assert_eq!(form.marker, "alpha");
        assert_eq!(form.title, "Changed title");
        assert_eq!(form.hint, "Changed hint");
        assert_eq!(form.shortcut, "9");
        assert_eq!(form.begin_audio, "new begin.wav");
        assert_eq!(form.begin_kind, "underlay");
        assert_eq!(form.begin_reduction, "99");
        assert_eq!(form.begin_repeat, "-1");
        assert_eq!(form.end_audio, "new end.wav");
        assert_eq!(form.end_reduction, "12");
        assert_eq!(form.end_repeat, "0");
        assert!(form.section);
        fixture.no_action();
    });
}

#[test]
fn add_is_local_until_save_and_cancel_discards_new_markers_without_disk_changes() {
    with_fixture(|fixture| {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("project.json");
        let committed =
            serde_json::to_vec(&fixture.state.lock().unwrap().dialog_marker_snapshot).unwrap();
        std::fs::write(&path, &committed).unwrap();
        fixture.open_markers();
        fixture
            .window
            .set_dialog_markers_title("Unsaved alpha".into());
        fixture
            .window
            .invoke_dialog_markers_add_marker(" gamma ".into());
        assert_eq!(fixture.marker_names(), ["alpha", "beta", "gamma"]);
        assert_eq!(fixture.selected_marker(), "gamma");
        fixture.window.set_dialog_markers_title("New draft".into());
        fixture.no_action();
        fixture.window.invoke_dialog_markers_cancel();
        assert!(!fixture.window.get_dialog_markers_open());
        assert!(!fixture.state.lock().unwrap().dialog_markers_open);
        fixture.no_action();
        assert_eq!(std::fs::read(&path).unwrap(), committed);
        assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 1);
        fixture.open_markers();
        assert_eq!(fixture.marker_names(), ["alpha", "beta"]);
        assert_eq!(fixture.window.get_dialog_markers_title(), "Alpha saved");
        fixture
            .window
            .invoke_dialog_markers_add_marker("gamma".into());
        fixture
            .window
            .set_dialog_markers_title("Saved new marker".into());
        fixture.save_marker();
        let (_, edit) = fixture.next_save();
        let form = marker_edit(edit);
        assert_eq!(form.marker, "gamma");
        assert_eq!(form.title, "Saved new marker");
        fixture.no_action();
    });
}

#[test]
fn duplicate_alias_shortcut_and_invalid_numbers_are_inline_without_sending_actions() {
    with_fixture(|fixture| {
        fixture.open_markers();
        for alias in ["alpha", " alpha ", " \t"] {
            fixture
                .window
                .invoke_dialog_markers_add_marker(alias.into());
            assert!(!fixture.window.get_dialog_markers_alias_error().is_empty());
            assert_eq!(fixture.marker_names(), ["alpha", "beta"]);
            fixture.no_action();
        }
        for shortcut in ["2", "12", "١"] {
            fixture.window.set_dialog_markers_shortcut(shortcut.into());
            fixture.save_marker();
            assert!(!fixture
                .window
                .get_dialog_markers_shortcut_error()
                .is_empty());
            assert_eq!(fixture.window.get_dialog_markers_shortcut(), shortcut);
            assert!(!fixture.window.get_dialog_busy());
            fixture.no_action();
        }
        fixture.window.set_dialog_markers_shortcut("1".into());
        fixture
            .window
            .set_dialog_markers_begin_reduction("101".into());
        fixture.save_marker();
        assert!(!fixture
            .window
            .get_dialog_markers_begin_reduction_error()
            .is_empty());
        fixture.no_action();
        fixture
            .window
            .set_dialog_markers_begin_reduction("100".into());
        fixture.window.set_dialog_markers_begin_repeat("-1".into());
        fixture.save_marker();
        assert!(!fixture
            .window
            .get_dialog_markers_begin_repeat_error()
            .is_empty());
        fixture.no_action();
        fixture.window.set_dialog_markers_begin_repeat("1".into());
        fixture
            .window
            .set_dialog_markers_end_reduction("bad".into());
        fixture.save_marker();
        assert!(!fixture
            .window
            .get_dialog_markers_end_reduction_error()
            .is_empty());
        fixture.no_action();
        fixture.window.set_dialog_markers_end_reduction("0".into());
        fixture
            .window
            .set_dialog_markers_end_kind("underlay".into());
        fixture.window.set_dialog_markers_end_repeat("-2".into());
        fixture.save_marker();
        assert!(!fixture
            .window
            .get_dialog_markers_end_repeat_error()
            .is_empty());
        assert_eq!(fixture.window.get_dialog_markers_end_repeat(), "-2");
        fixture.no_action();
    });
}

#[test]
fn busy_dialogs_ignore_duplicate_save_escape_and_cancel_until_matching_reply() {
    for kind in 0..3 {
        with_fixture(move |fixture| {
            match kind {
                0 => {
                    fixture.open_meta();
                    fixture.save_meta();
                }
                1 => {
                    fixture.open_chunk();
                    fixture.save_chunk();
                }
                _ => {
                    fixture.open_markers();
                    fixture.save_marker();
                }
            }
            let (request, _) = fixture.next_save();
            assert!(fixture.window.get_dialog_busy());
            match kind {
                0 => {
                    fixture.save_meta();
                    fixture.window.invoke_dialog_project_cancel();
                }
                1 => {
                    fixture.save_chunk();
                    fixture.window.invoke_dialog_chunk_cancel();
                }
                _ => {
                    fixture.save_marker();
                    fixture.window.invoke_dialog_markers_cancel();
                    fixture.window.invoke_dialog_markers_marker_selected(1);
                    fixture
                        .window
                        .invoke_dialog_markers_add_marker("ignored".into());
                    fixture.window.invoke_dialog_markers_update_meta();
                    assert_eq!(fixture.selected_marker(), "alpha");
                    assert_eq!(fixture.marker_names(), ["alpha", "beta"]);
                }
            }
            fixture.escape();
            fixture.no_action();
            let is_open = || match kind {
                0 => fixture.window.get_dialog_project_open(),
                1 => fixture.window.get_dialog_chunk_open(),
                _ => fixture.window.get_dialog_markers_open(),
            };
            assert!(is_open());
            fixture.reply(request + 100, Ok(None), None);
            assert!(fixture.window.get_dialog_busy() && is_open());
            fixture.reply(request, Err("Disk full".into()), None);
            assert!(!fixture.window.get_dialog_busy() && is_open());
            assert!(fixture
                .window
                .get_dialog_error_message()
                .contains("Disk full"));
            fixture.escape();
            assert!(!is_open());
            fixture.no_action();
        });
    }
}

#[test]
fn failed_marker_reply_retains_fields_and_allows_retry_with_a_new_request() {
    with_fixture(|fixture| {
        fixture.open_markers();
        fixture
            .window
            .set_dialog_markers_title("Keep my title".into());
        fixture
            .window
            .set_dialog_markers_hint("Keep my hint".into());
        fixture.save_marker();
        let (first, _) = fixture.next_save();
        fixture.reply(
            first,
            Err("Cannot persist project".into()),
            Some("shortcut"),
        );
        assert!(fixture.window.get_dialog_markers_open());
        assert!(!fixture.window.get_dialog_busy());
        assert_eq!(fixture.window.get_dialog_markers_title(), "Keep my title");
        assert_eq!(fixture.window.get_dialog_markers_hint(), "Keep my hint");
        assert!(!fixture
            .window
            .get_dialog_markers_shortcut_error()
            .is_empty());
        fixture.save_marker();
        let (second, edit) = fixture.next_save();
        assert!(second > first);
        assert_eq!(marker_edit(edit).title, "Keep my title");
        fixture.reply(second, Ok(None), None);
        assert!(fixture.window.get_dialog_markers_open());
        assert!(fixture.window.get_dialog_error_message().is_empty());
        assert!(fixture
            .window
            .get_dialog_markers_shortcut_error()
            .is_empty());
        fixture.no_action();
    });
}

#[test]
fn meta_and_chunk_failures_retain_fields_and_successful_replies_close_them() {
    with_fixture(|fixture| {
        fixture.open_meta();
        fixture.save_meta();
        let (request, edit) = fixture.next_save();
        let data = match edit {
            DialogEdit::Meta(data) => data,
            other => panic!("{other:?}"),
        };
        fixture.reply(request, Err("Read-only project".into()), None);
        assert!(fixture.window.get_dialog_project_open());
        assert_eq!(fixture.window.get_dialog_project_title(), "Edited book");
        assert_eq!(
            fixture.window.get_dialog_project_hint(),
            "Unsaved book hint"
        );
        fixture.save_meta();
        let (request, _) = fixture.next_save();
        {
            let mut state = fixture.state.lock().unwrap();
            state.dialog_project_title = data.title;
            state.dialog_project_author = data.author;
            state.dialog_project_year = data.year;
            state.dialog_project_hint = data.hint;
            state.dialog_project_reader = data.reader;
        }
        fixture.reply(request, Ok(None), None);
        assert!(!fixture.window.get_dialog_project_open());
        assert_eq!(fixture.window.get_meta_title(), "Edited book");

        fixture.open_chunk();
        fixture.save_chunk();
        let (request, _) = fixture.next_save();
        fixture.reply(request, Err("Disk full".into()), None);
        assert!(fixture.window.get_dialog_chunk_open());
        assert_eq!(fixture.window.get_dialog_chunk_title(), "Edited recording");
        assert_eq!(
            fixture.window.get_dialog_chunk_hint(),
            "Unsaved transcription"
        );
        fixture.save_chunk();
        let (request, _) = fixture.next_save();
        fixture.reply(request, Ok(None), None);
        assert!(!fixture.window.get_dialog_chunk_open());
        assert!(!fixture.window.get_dialog_busy());
        fixture.no_action();
    });
}

#[test]
fn chunk_request_keeps_the_path_captured_when_the_dialog_opened() {
    with_fixture(|fixture| {
        fixture.open_chunk();
        fixture.window.set_current_file_index(0);
        fixture.save_chunk();
        let (_, edit) = fixture.next_save();
        match edit {
            DialogEdit::Chunk {
                data,
                expected_path,
            } => {
                assert_eq!(expected_path, "first.wav");
                assert_eq!(data.ui_index, 1);
                assert_eq!(data.title, "Edited recording");
            }
            other => panic!("{other:?}"),
        }
        fixture.no_action();
    });
}

#[test]
fn closed_action_channel_preserves_input_and_does_not_leave_dialog_busy() {
    with_fixture(|fixture| {
        fixture.open_meta();
        drop(fixture.actions);
        let window = &fixture.window;
        window.invoke_dialog_project_save(
            window.get_dialog_project_title(),
            window.get_dialog_project_author(),
            window.get_dialog_project_year(),
            window.get_dialog_project_hint(),
            window.get_dialog_project_reader(),
            window.get_dialog_project_format_audio(),
            window.get_dialog_project_normalize(),
            window.get_dialog_project_cover(),
            window.get_dialog_project_section_split(),
            window.get_dialog_project_denoise(),
        );
        assert!(window.get_dialog_project_open());
        assert!(!window.get_dialog_busy());
        assert_eq!(window.get_dialog_project_title(), "Edited book");
        assert_eq!(window.get_dialog_project_hint(), "Unsaved book hint");
        assert!(window
            .get_dialog_error_message()
            .contains("Cannot submit changes"));
        window.invoke_dialog_project_cancel();
        assert!(!window.get_dialog_project_open());
    });
}

#[test]
fn window_close_is_ignored_during_save_and_allowed_after_the_reply() {
    for successful in [false, true] {
        with_fixture(move |fixture| {
            fixture.open_markers();
            fixture
                .window
                .set_dialog_markers_title("Pending title".into());
            fixture.save_marker();
            let (request, _) = fixture.next_save();
            assert!(fixture.window.get_dialog_busy());
            assert!(fixture.window.window().is_visible());
            fixture
                .window
                .window()
                .dispatch_event(WindowEvent::CloseRequested);
            assert!(fixture.window.window().is_visible());
            assert!(fixture.window.get_dialog_markers_open());
            assert_eq!(fixture.window.get_dialog_markers_title(), "Pending title");
            fixture.no_action();

            let result = if successful {
                Ok(None)
            } else {
                Err("Save failed".into())
            };
            fixture.reply(request, result, None);
            assert!(!fixture.window.get_dialog_busy());
            assert!(fixture.window.window().is_visible());
            fixture
                .window
                .window()
                .dispatch_event(WindowEvent::CloseRequested);
            assert!(!fixture.window.window().is_visible());
            fixture.no_action();
        });
    }
}

#[test]
fn post_save_warning_is_visible_without_discarding_the_dialog_input() {
    with_fixture(|fixture| {
        fixture.open_chunk();
        fixture.save_chunk();
        let (request, _) = fixture.next_save();
        fixture.reply(request, Ok(Some("directory sync failed".into())), None);
        assert!(!fixture.window.get_dialog_busy());
        assert!(fixture.window.get_dialog_chunk_open());
        assert_eq!(fixture.window.get_dialog_chunk_title(), "Edited recording");
        assert!(fixture
            .window
            .get_dialog_error_message()
            .contains("Saved, but"));
        fixture.no_action();
    });
}
