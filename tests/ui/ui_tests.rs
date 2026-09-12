use super::*;
use crate::ui::dialogs::DialogReply;
use crate::utils::keyboard::DialogEdit;
use slint::platform::software_renderer::{MinimalSoftwareWindow, RepaintBufferType};
use slint::platform::{Key, Platform, PointerEventButton, WindowAdapter, WindowEvent};
use slint::ComponentHandle;
use std::rc::Rc;

struct TestPlatform(Rc<MinimalSoftwareWindow>);

impl Platform for TestPlatform {
    fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, slint::PlatformError> {
        Ok(self.0.clone())
    }
}

fn draw(adapter: &MinimalSoftwareWindow) -> slint::SharedPixelBuffer<slint::Rgb8Pixel> {
    slint::platform::update_timers_and_animations();
    adapter.request_redraw();
    let size = adapter.size();
    let mut buffer = slint::SharedPixelBuffer::<slint::Rgb8Pixel>::new(size.width, size.height);
    adapter.draw_if_needed(|renderer| {
        renderer.render(buffer.make_mut_slice(), size.width as usize);
    });
    buffer
}

fn press(window: &MainWindow, key: impl Into<slint::SharedString>) {
    let text = key.into();
    window
        .window()
        .dispatch_event(WindowEvent::KeyPressed { text: text.clone() });
    window
        .window()
        .dispatch_event(WindowEvent::KeyReleased { text });
}

fn next_dialog_save(actions: &Receiver<Action>) -> (u64, DialogEdit) {
    let Action::SaveDialog { request_id, edit } = actions.try_recv().unwrap() else {
        panic!("Expected a dialog save");
    };
    (request_id, *edit)
}

#[test]
#[ignore = "requires a disposable IAMREADER_SMOKE_PROJECT and IAMREADER_SMOKE_IMAGE_DIR"]
fn ui_supplied_project_smoke() {
    use crate::audio::waveform::read_waveform_samples;
    use crate::project::project::Project;
    use crate::utils::paths::resolve_project_file;
    use crate::utils::stats::{calculate_sizes, calculate_total_duration, get_free_space};
    use std::io::Write;

    std::thread::spawn(|| {
        let project_path = std::path::PathBuf::from(
            std::env::var_os("IAMREADER_SMOKE_PROJECT")
                .expect("IAMREADER_SMOKE_PROJECT is required"),
        )
        .canonicalize()
        .unwrap();
        let project_dir = project_path.parent().unwrap();
        let temp_dir = std::env::temp_dir().canonicalize().unwrap();
        assert!(
            project_dir.starts_with(&temp_dir),
            "Smoke requires a disposable project copy under the temporary directory"
        );
        let image_dir = std::path::PathBuf::from(
            std::env::var_os("IAMREADER_SMOKE_IMAGE_DIR")
                .expect("IAMREADER_SMOKE_IMAGE_DIR is required"),
        );
        assert!(
            image_dir.starts_with(&temp_dir),
            "Smoke screenshots must be written under the temporary directory"
        );
        std::fs::create_dir_all(&image_dir).unwrap();

        let project_bytes = std::fs::read(&project_path).unwrap();
        let project = Project::load(&project_path).unwrap();
        let available: Vec<_> = project
            .files
            .iter()
            .filter_map(|entry| {
                let path = resolve_project_file(project_dir, &entry.path)
                    .canonicalize()
                    .ok()?;
                if !path.starts_with(project_dir)
                    || !path.is_file()
                    || !path
                        .extension()
                        .is_some_and(|extension| extension.eq_ignore_ascii_case("wav"))
                {
                    return None;
                }
                let reader = hound::WavReader::open(&path).ok()?;
                if reader.duration() == 0 {
                    return None;
                }
                Some((entry.clone(), path))
            })
            .take(20)
            .collect();
        assert!(
            !available.is_empty(),
            "No supplied WAV entries resolve inside the disposable project copy"
        );
        let files: Vec<_> = available.iter().map(|(entry, _)| entry.clone()).collect();
        let source_stats: Vec<_> = available
            .iter()
            .map(|(_, path)| {
                let metadata = std::fs::metadata(path).unwrap();
                (metadata.len(), metadata.modified().unwrap())
            })
            .collect();

        let adapter = MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer);
        slint::platform::set_platform(Box::new(TestPlatform(adapter.clone()))).unwrap();
        let (tx, rx) = crossbeam_channel::unbounded();
        let mut ui = UI::new(tx, project.settings.keys.clone(), None).unwrap();
        ui._timer.as_ref().unwrap().stop();
        ui.load_meta_from_project(&project.meta, &project.settings)
            .unwrap();
        let mut original_index = files.len() - 1;
        ui.update_file_list(
            &files,
            Some(original_index),
            Duration::ZERO,
            false,
            None,
            "A",
        )
        .unwrap();
        ui.sync_file_list_to_model().unwrap();
        let window = ui.window.as_ref().unwrap().clone_strong();
        window.show().unwrap();
        adapter.set_size(slint::LogicalSize::new(1200.0, 800.0));

        let update_selection = |ui: &mut UI, original_index: usize| {
            ui.set_current_file_index_only(orig_to_ui_index(original_index, files.len()))
                .unwrap();
            ui.update_file_hints(&files, Some(original_index)).unwrap();
            ui.update_status_line(
                Some(original_index),
                files.len(),
                Some(&files[original_index]),
                Duration::from_millis(files[original_index].duration_ms),
                calculate_total_duration(&files),
                project.stats.record_length,
                Duration::ZERO,
                calculate_sizes(&files),
                get_free_space(project_dir),
                false,
                "A",
            )
            .unwrap();
            ui.sync_status_line_to_window().unwrap();
            let current =
                read_waveform_samples(&available[original_index].1, 1500, false).unwrap();
            let previous = original_index
                .checked_sub(1)
                .map(|index| read_waveform_samples(&available[index].1, 1500, false).unwrap())
                .unwrap_or_default();
            ui.update_waveform_current(&current).unwrap();
            ui.update_waveform_prev(&previous).unwrap();
            ui.get_state().lock().unwrap().waveform_version += 1;
        };
        let snapshot = |name: &str| {
            let pixels = draw(&adapter);
            let mut writer =
                std::io::BufWriter::new(std::fs::File::create(image_dir.join(name)).unwrap());
            write!(writer, "P6\n{} {}\n255\n", pixels.width(), pixels.height()).unwrap();
            for pixel in pixels.as_slice() {
                writer.write_all(&[pixel.r, pixel.g, pixel.b]).unwrap();
            }
            writer.flush().unwrap();
        };
        update_selection(&mut ui, original_index);
        ui._timer.as_ref().unwrap().restart();
        std::thread::sleep(Duration::from_millis(45));
        snapshot("main-100.ppm");

        window
            .window()
            .dispatch_event(WindowEvent::ScaleFactorChanged { scale_factor: 1.25 });
        adapter.set_size(slint::LogicalSize::new(640.0, 720.0));
        draw(&adapter);
        let click = |position| {
            window.window().dispatch_event(WindowEvent::PointerPressed {
                position,
                button: PointerEventButton::Left,
            });
            window
                .window()
                .dispatch_event(WindowEvent::PointerReleased {
                    position,
                    button: PointerEventButton::Left,
                });
        };
        click(slint::LogicalPosition::new(
            30.0,
            660.0 - window.get_file_list_visible_height() / 2.0,
        ));
        let Action::Goto {
            index: Some(ui_index),
            play: false,
        } = rx.try_recv().unwrap()
        else {
            panic!("Click must select a supplied fragment");
        };
        assert!(ui_index >= 0 && (ui_index as usize) < files.len());
        original_index = files.len() - ui_index as usize - 1;
        update_selection(&mut ui, original_index);
        std::thread::sleep(Duration::from_millis(45));
        snapshot("main-half-125.ppm");

        press(&window, Key::F5);
        assert!(window.get_dialog_project_open());
        snapshot("project-half-125.ppm");
        click(slint::LogicalPosition::new(230.0, 187.0)); // Export tab
        snapshot("project-export-half-125.ppm");
        click(slint::LogicalPosition::new(300.0, 227.0));
        draw(&adapter);
        click(slint::LogicalPosition::new(300.0, 272.0));
        assert_eq!(window.get_dialog_project_format_audio(), "mp3");
        draw(&adapter);
        click(slint::LogicalPosition::new(300.0, 227.0));
        draw(&adapter);
        click(slint::LogicalPosition::new(300.0, 227.0));
        assert_eq!(window.get_dialog_project_format_audio(), "wav");
        snapshot("project-export-half-125.ppm");
        click(slint::LogicalPosition::new(524.0, 582.0));
        assert!(window.get_dialog_project_open());
        assert!(window.get_dialog_busy());
        let (request_id, DialogEdit::Meta(meta)) = next_dialog_save(&rx)
        else {
            panic!("Project Save must submit metadata, not mutate the source project");
        };
        ui.get_state().lock().unwrap().dialog_save_result = Some(DialogReply {
            request_id,
            result: Ok(None),
            field: None,
        });
        std::thread::sleep(Duration::from_millis(45));
        draw(&adapter);
        assert!(!window.get_dialog_project_open());
        assert_eq!(meta.title, project.meta.title);
        assert_eq!(meta.author, project.meta.author);
        assert_eq!(meta.year, project.meta.year);
        assert_eq!(meta.format_audio, "wav");

        window.invoke_file_double_clicked(ui_index);
        assert!(window.get_dialog_chunk_open());
        snapshot("chunk-half-125.ppm");
        click(slint::LogicalPosition::new(524.0, 532.0));
        assert!(window.get_dialog_chunk_open());
        let (request_id, DialogEdit::Chunk { data: chunk, expected_path }) = next_dialog_save(&rx)
        else {
            panic!("Chunk Save must submit metadata, not mutate the source project");
        };
        assert_eq!(expected_path, files[original_index].path);
        ui.get_state().lock().unwrap().dialog_save_result = Some(DialogReply {
            request_id,
            result: Ok(None),
            field: None,
        });
        std::thread::sleep(Duration::from_millis(45));
        draw(&adapter);
        assert!(!window.get_dialog_chunk_open());
        assert_eq!(chunk.title, files[original_index].title);
        assert_eq!(chunk.author, files[original_index].author);
        assert_eq!(chunk.year, files[original_index].year);
        assert_eq!(chunk.hint, files[original_index].hint);
        assert!(rx.is_empty());

        // Real supplied marker definitions; local drafts and captures never touch JSON/WAV.
        for (width, height, scale_factor, label) in [
            (400.0_f32, 300.0_f32, 1.0, "minimum-100"),
            (400.0, 300.0, 1.25, "minimum-125"),
            (640.0, 480.0, 1.0, "half-100"),
            (640.0, 480.0, 1.25, "half-125"),
            (1200.0, 800.0, 1.0, "wide-100"),
        ] {
            window.window().dispatch_event(WindowEvent::ScaleFactorChanged { scale_factor });
            adapter.set_size(slint::LogicalSize::new(width, height));
            let dw = 520.0_f32.min(width - 20.0);
            let dh = 500.0_f32.min(height - 20.0);
            press(&window, Key::F5);
            draw(&adapter);
            click(slint::LogicalPosition::new((width - dw) / 2.0 + 62.0, (height - dh) / 2.0 + 77.0));
            snapshot(&format!("project-{label}.ppm"));
            press(&window, Key::Escape);
            window.invoke_file_double_clicked(ui_index);
            snapshot(&format!("chunk-{label}.ppm"));
            press(&window, Key::Escape);
            {
                let state = ui.get_state();
                let mut state = state.lock().unwrap();
                state.dialog_marker_snapshot = project.markers.clone();
                state.dialog_markers_open = true;
            }
            std::thread::sleep(Duration::from_millis(45));
            snapshot(&format!("markers-{label}.ppm"));
            assert!(window.get_dialog_markers_open());
            assert_eq!(window.get_dialog_markers_list().row_count(), project.markers.len());
            if label == "half-125" {
                // Exercise the actual selector callback while retaining another marker's draft.
                let marker_width = 900.0_f32.min(width - 20.0);
                let marker_height = 680.0_f32.min(height - 20.0);
                let combo_y = (height - marker_height) / 2.0 + 77.0;
                window.set_dialog_markers_title("Unsaved draft".into());
                click(slint::LogicalPosition::new(300.0, combo_y));
                draw(&adapter);
                click(slint::LogicalPosition::new(300.0, combo_y + 45.0));
                draw(&adapter);
                assert_eq!(window.get_dialog_markers_selected_index(), 1);
                assert_ne!(window.get_dialog_markers_title(), "Unsaved draft");
                click(slint::LogicalPosition::new(300.0, combo_y));
                draw(&adapter);
                click(slint::LogicalPosition::new(300.0, combo_y));
                draw(&adapter);
                assert_eq!(window.get_dialog_markers_selected_index(), 0);
                assert_eq!(window.get_dialog_markers_title(), "Unsaved draft");
                assert!(rx.is_empty());
                assert!(marker_width > 0.0);
            }
            window.window().dispatch_event(WindowEvent::PointerScrolled {
                position: slint::LogicalPosition::new(width / 2.0, height / 2.0), delta_x: 0.0, delta_y: -460.0,
            });
            snapshot(&format!("marker-sounds-{label}.ppm"));
            window.set_dialog_error_message("Cannot save project: disk is full. The draft is still here. Free some space and try Save again. A very long path: /home/reader/books/a-long-book-title/iamreader.json".into());
            snapshot(&format!("marker-error-{label}.ppm"));
            let dw = 900.0_f32.min(width - 20.0);
            let dh = 680.0_f32.min(height - 20.0);
            click(slint::LogicalPosition::new((width + dw) / 2.0 - 148.0, (height + dh) / 2.0 - 28.0));
            assert!(!window.get_dialog_markers_open(), "Cancel must stay reachable at {label}");
            assert!(rx.is_empty());
        }
        ui._timer.as_ref().unwrap().stop();
        adapter.set_size(slint::LogicalSize::new(400.0, 300.0));
        window.set_dialog_toc_list_text("1. Chapter one\n2. A longer chapter title that should wrap without horizontal scrolling\n3. Chapter three".into());
        window.set_dialog_toc_open(true);
        snapshot("toc-minimum.ppm");
        press(&window, Key::Escape);
        window.set_dialog_shortcuts_list_text("r — Record\ne — Confirm\nd — Stop\na / f — Previous / Next\ns — Play\nu — Update mode\ni — Insert mode\nF1 — Shortcuts\nF4 — Markers\nF5 — Project\nF8 — Compile\nF10 — Table of contents".into());
        window.set_dialog_shortcuts_open(true);
        snapshot("shortcuts-minimum.ppm");
        press(&window, Key::Escape);
        window.set_dialog_delete_text("Delete recording 00013.wav? This action cannot be undone.".into());
        window.set_dialog_delete_open(true);
        snapshot("delete-minimum.ppm");
        press(&window, Key::Escape);

        assert_eq!(std::fs::read(&project_path).unwrap(), project_bytes);
        for ((_, path), expected) in available.iter().zip(source_stats) {
            let metadata = std::fs::metadata(path).unwrap();
            assert_eq!((metadata.len(), metadata.modified().unwrap()), expected);
        }
        eprintln!(
            "Read-only software-renderer smoke: {} supplied WAV entries, screenshots in {}",
            files.len(),
            image_dir.display()
        );
    })
    .join()
    .unwrap();
}

#[test]
fn ui_export_outcomes_remain_until_close() {
    use crate::project::compile_progress::{CompileProgress, Operation, ProgressUnit};
    use crate::project::export_workspace::ExportCancelled;
    use std::io::Write;

    std::thread::spawn(|| {
        // Optional software-renderer captures; normal test runs never write images.
        let image_dir = std::env::var_os("IAMREADER_EXPORT_IMAGE_DIR").map(|path| {
            let path = std::path::PathBuf::from(path);
            let temp_dir = std::env::temp_dir().canonicalize().unwrap();
            assert!(
                path.starts_with(&temp_dir)
                    && !path.components().any(|part| matches!(part, std::path::Component::ParentDir)),
                "Export screenshots must be written under the temporary directory"
            );
            let existing_ancestor = path.ancestors().find(|ancestor| ancestor.exists()).unwrap();
            assert!(existing_ancestor.canonicalize().unwrap().starts_with(&temp_dir));
            std::fs::create_dir_all(&path).unwrap();
            let path = path.canonicalize().unwrap();
            assert!(path.starts_with(&temp_dir));
            path
        });
        let scale_factor = if image_dir.is_some() { 1.25 } else { 1.0 };
        let adapter = MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer);
        slint::platform::set_platform(Box::new(TestPlatform(adapter.clone()))).unwrap();
        let (tx, rx) = crossbeam_channel::unbounded();
        let ui = UI::new(tx, KeyBindings::default(), None).unwrap();
        let state = ui.get_state();
        let window = ui.window.as_ref().unwrap();
        window.show().unwrap();
        window.window().dispatch_event(WindowEvent::ScaleFactorChanged { scale_factor });
        adapter.set_size(slint::LogicalSize::new(640.0, 720.0));
        let snapshot = |name: &str, pixels: &slint::SharedPixelBuffer<slint::Rgb8Pixel>| {
            if let Some(image_dir) = &image_dir {
                let mut writer = std::io::BufWriter::new(
                    std::fs::File::create(image_dir.join(name)).unwrap(),
                );
                write!(writer, "P6\n{} {}\n255\n", pixels.width(), pixels.height()).unwrap();
                for pixel in pixels.as_slice() {
                    writer.write_all(&[pixel.r, pixel.g, pixel.b]).unwrap();
                }
                writer.flush().unwrap();
            }
        };
        let tick = || {
            std::thread::sleep(Duration::from_millis(45));
            draw(&adapter)
        };
        let click_action = || {
            let position = slint::LogicalPosition::new(320.0, 485.0);
            window.window().dispatch_event(WindowEvent::PointerPressed {
                position,
                button: PointerEventButton::Left,
            });
            window
                .window()
                .dispatch_event(WindowEvent::PointerReleased {
                    position,
                    button: PointerEventButton::Left,
                });
        };

        window.invoke_dialog_compile_cancel();
        window.invoke_dialog_compile_close();
        assert!(rx.is_empty());
        state.lock().unwrap().is_compiling = true;
        let mut operation = CompileProgress::new(
            Some(&state),
            None,
            Operation {
                label: "Section 2/3 · Analyze level".into(),
                start: 0.2,
                end: 0.6,
                unit: ProgressUnit::Audio { sample_rate: 48_000, channels: 2 },
            },
        );
        operation.report(0, 0).unwrap();
        // Exercise real elapsed-time ETA and interleaved-sample formatting without audio I/O.
        std::thread::sleep(Duration::from_millis(550));
        let total_samples = 48_000 * 2 * 3600;
        let done_samples = total_samples / 4;
        operation.report(done_samples, total_samples).unwrap();
        for (capture_scale, image_name) in [
            (1.0, "export-audio-progress-half-100.ppm"),
            (1.25, "export-audio-progress-half-125.ppm"),
        ] {
            window.window().dispatch_event(WindowEvent::ScaleFactorChanged { scale_factor: capture_scale });
            adapter.set_size(slint::LogicalSize::new(640.0, 480.0));
            let pixels = tick();
            assert_eq!(window.get_compile_stage(), "Section 2/3 · Analyze level: 25% · 00:15:00 / 01:00:00");
            assert!(window.get_dialog_compile_stages_text().contains("00:15:00 / 01:00:00"));
            assert!((window.get_compile_progress() - 0.3).abs() < f32::EPSILON);
            assert!(window.get_dialog_compile_eta_secs().is_finite());
            assert!(window.get_dialog_compile_eta_secs() > 0.0);
            assert!(window.get_dialog_compile_cancel_enabled());
            assert!(!window.get_dialog_compile_finished());
            snapshot(image_name, &pixels);
        }
        window.window().dispatch_event(WindowEvent::ScaleFactorChanged { scale_factor });
        adapter.set_size(slint::LogicalSize::new(640.0, 720.0));
        snapshot("export-running-125.ppm", &tick());
        assert!(window.get_is_compiling());
        assert!(window.get_dialog_compile_cancel_enabled());
        assert!(!window.get_dialog_compile_finished());
        window.invoke_dialog_compile_close();
        assert!(state.lock().unwrap().is_compiling);
        click_action();
        assert_eq!(rx.try_recv().unwrap(), Action::CompileCancel);
        assert!(state.lock().unwrap().compile_cancel_requested);
        assert!(window.get_dialog_compile_cancel_requested());
        assert!(!window.get_dialog_compile_cancel_enabled());
        click_action();
        window.invoke_dialog_compile_cancel();
        press(window, Key::Escape);
        assert!(rx.is_empty());
        assert!(window.get_is_compiling());
        std::thread::sleep(Duration::from_millis(110));
        assert!(operation.report(done_samples, total_samples).unwrap_err().is::<ExportCancelled>());
        tick();
        assert_eq!(window.get_compile_stage(), "Cancelling…");
        assert!(window.get_dialog_compile_eta_secs() < 0.0);

        let mut finished_status_images = Vec::new();
        for (index, (outcome, image_name)) in [
            ("Cancelled: no output published", "export-cancelled-125.ppm"),
            ("Done: /tmp/book/export/book.wav", "export-done-125.ppm"),
            (
                "Error: Cannot publish /tmp/project-with-a-long-name/export/audiobook-with-a-long-title.wav: Disk is full",
                "export-error-125.ppm",
            ),
        ]
        .into_iter()
        .enumerate()
        {
            if index == 1 {
                {
                    let mut state = state.lock().unwrap();
                    state.is_compiling = true;
                    state.compile_publishing = true;
                    state.compile_stage = "Publishing".to_owned();
                }
                tick();
                assert!(window.get_dialog_compile_publishing());
                assert!(!window.get_dialog_compile_cancel_enabled());
                assert!(!window.get_dialog_compile_finished());
                click_action();
                window.invoke_dialog_compile_cancel();
                window.invoke_dialog_compile_close();
                press(window, Key::Escape);
                assert!(rx.is_empty());
                assert!(!state.lock().unwrap().compile_cancel_requested);
                assert!(window.get_is_compiling());
            }
            {
                let mut state = state.lock().unwrap();
                state.is_compiling = true;
                state.compile_stage = outcome.to_owned();
                state.dialog_compile_stages_text =
                    "Reading: done\nNormalization: done\nEncoding: done".to_owned();
                // An old timestamp must not auto-dismiss any terminal result.
                state.compile_finished_at =
                    Some(std::time::Instant::now() - Duration::from_secs(60));
            }
            let pixels = tick();
            snapshot(image_name, &pixels);
            assert!(window.get_is_compiling());
            assert!(window.get_dialog_compile_finished());
            assert!(!window.get_dialog_compile_cancel_enabled());
            assert_eq!(window.get_compile_stage(), outcome);
            // Verify the displayed status changes despite an identical non-empty stages log.
            let physical = |logical: f32| (logical * scale_factor) as usize;
            let status_pixels: Vec<_> = (physical(245.0)..physical(290.0))
                .flat_map(|y| {
                    pixels.as_slice()
                        [y * pixels.width() as usize + physical(100.0)
                            ..y * pixels.width() as usize + physical(540.0)]
                        .iter()
                        .copied()
                })
                .collect();
            if let Some(previous) = finished_status_images.last() {
                assert_ne!(previous, &status_pixels);
            }
            finished_status_images.push(status_pixels);
            tick();
            assert!(window.get_is_compiling());
            assert_eq!(window.get_compile_stage(), outcome);
            window.invoke_dialog_compile_cancel();
            assert!(rx.is_empty());
            if index == 0 {
                press(window, Key::Escape);
            } else {
                click_action();
            }
            assert!(!window.get_is_compiling());
            assert!(!window.get_modal_open());
            assert!(rx.is_empty(), "Close must not dispatch Cancel");
            let state = state.lock().unwrap();
            assert!(!state.is_compiling);
            assert!(state.compile_finished_at.is_none());
            assert!(!state.compile_cancel_requested);
            assert!(!state.compile_publishing);
            assert!(state.compile_stage.is_empty());
            assert!(state.dialog_compile_stages_text.is_empty());
        }
    })
    .join()
    .unwrap();
}

#[test]
fn ui_model_shortcuts_modality_and_viewport() {
    // One dedicated thread owns the Slint platform; no display server or audio device is used.
    std::thread::spawn(|| {
        let adapter = MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer);
        slint::platform::set_platform(Box::new(TestPlatform(adapter.clone()))).unwrap();
        let (tx, rx) = crossbeam_channel::unbounded();
        let ui = UI::new(tx, KeyBindings::default(), None).unwrap();
        ui._timer.as_ref().unwrap().stop();
        let window = ui.window.as_ref().unwrap();
        window.show().unwrap();
        adapter.set_size(slint::LogicalSize::new(1200.0, 800.0));

        let state = ui.get_state();
        state.lock().unwrap().file_list = (0..100)
            .map(|index| FileInfo {
                index,
                name: format!("chunk-{index}").into(),
                ..Default::default()
            })
            .collect();
        ui.sync_file_list_to_model().unwrap();
        let first = FileInfo {
            index: 100,
            name: "newest".into(),
            author: "Original author".into(),
            year: "1984".into(),
            ..Default::default()
        };
        state.lock().unwrap().file_list.insert(0, first.clone());
        ui.sync_file_list_to_model().unwrap();
        assert_eq!(window.get_file_list().row_data(0), Some(first));
        assert_eq!(window.get_file_list().row_data(1).unwrap().name, "chunk-0");
        {
            let mut state = state.lock().unwrap();
            state.file_list[0].index = 101;
            state.file_list[0].name = "renamed".into();
            state.file_list[0].size = "2 MB".into();
        }
        ui.sync_file_list_to_model().unwrap();
        assert_eq!(
            window.get_file_list().row_data(0),
            Some(state.lock().unwrap().file_list[0].clone())
        );
        state.lock().unwrap().file_list.pop();
        ui.sync_file_list_to_model().unwrap();
        assert_eq!(window.get_file_list().row_count(), 100);
        draw(&adapter);

        window.invoke_file_double_clicked(0);
        draw(&adapter);
        assert!(window.get_dialog_chunk_open());
        assert_eq!(window.get_dialog_chunk_author(), "Original author");
        assert_eq!(window.get_dialog_chunk_year(), "1984");
        window.set_dialog_chunk_title("Edited title".into());
        window.set_dialog_chunk_hint("Edited hint".into());
        let position = slint::LogicalPosition::new(804.0, 572.0);
        window.window().dispatch_event(WindowEvent::PointerPressed { position, button: PointerEventButton::Left });
        window.window().dispatch_event(WindowEvent::PointerReleased { position, button: PointerEventButton::Left });
        assert!(window.get_dialog_chunk_open());
        let (request_id, DialogEdit::Chunk { data: settings, .. }) = next_dialog_save(&rx) else {
            panic!("Chunk Save must submit the existing metadata");
        };
        state.lock().unwrap().dialog_save_result = Some(DialogReply { request_id, result: Ok(None), field: None });
        ui._timer.as_ref().unwrap().restart();
        std::thread::sleep(Duration::from_millis(45));
        draw(&adapter);
        ui._timer.as_ref().unwrap().stop();
        assert!(!window.get_dialog_chunk_open());
        assert_eq!(settings.ui_index, 0);
        assert_eq!(settings.title, "Edited title");
        assert_eq!(settings.hint, "Edited hint");
        assert_eq!(settings.author, "Original author");
        assert_eq!(settings.year, "1984");
        assert!(rx.is_empty());
        draw(&adapter);

        let position = slint::LogicalPosition::new(
            30.0,
            740.0 - window.get_file_list_visible_height() / 2.0,
        );
        window.window().dispatch_event(WindowEvent::PointerPressed {
            position,
            button: PointerEventButton::Left,
        });
        window
            .window()
            .dispatch_event(WindowEvent::PointerReleased {
                position,
                button: PointerEventButton::Left,
            });
        assert!(matches!(rx.try_recv().unwrap(), Action::Goto { .. }));
        let shortcuts: [(slint::SharedString, Action); 7] = [
            (Key::UpArrow.into(), Action::Next),
            (Key::DownArrow.into(), Action::Prev),
            (
                "3".into(),
                Action::AddMarker {
                    marker: "3".to_owned(),
                },
            ),
            (Key::F1.into(), Action::OpenShortcutsDialog),
            (Key::F4.into(), Action::OpenMarkerSettings),
            (Key::F8.into(), Action::Compile),
            (Key::Delete.into(), Action::OpenDeleteChunkDialog),
        ];
        for (key, action) in shortcuts {
            press(window, key);
            assert_eq!(rx.try_recv().unwrap(), action);
            assert!(rx.is_empty());
        }

        let dialogs: [fn(&MainWindow, bool); 7] = [
            MainWindow::set_dialog_project_open,
            MainWindow::set_dialog_chunk_open,
            MainWindow::set_dialog_markers_open,
            MainWindow::set_dialog_toc_open,
            MainWindow::set_dialog_shortcuts_open,
            MainWindow::set_dialog_delete_open,
            MainWindow::set_is_compiling,
        ];
        for set_open in dialogs {
            set_open(window, true);
            draw(&adapter);
            assert!(window.get_modal_open());
            let keys: [slint::SharedString; 8] = [
                Key::F1.into(),
                Key::F4.into(),
                Key::F5.into(),
                Key::F8.into(),
                Key::F10.into(),
                Key::UpArrow.into(),
                "3".into(),
                "r".into(),
            ];
            for key in keys {
                press(window, key);
            }
            window.invoke_key_pressed("r".into());
            window.invoke_add_marker_with_normalization("3".into());
            window.invoke_compile();
            window.invoke_dialog_markers_request_open();
            window.invoke_file_clicked(0);
            assert!(rx.is_empty());
            if window.get_is_compiling() {
                state.lock().unwrap().is_compiling = true;
                window.invoke_dialog_compile_cancel();
                assert_eq!(rx.try_recv().unwrap(), Action::CompileCancel);
                let mut state = state.lock().unwrap();
                state.is_compiling = false;
                state.compile_cancel_requested = false;
            }
            set_open(window, false);
            assert!(
                !window.get_modal_open(),
                "a shortcut opened a second dialog"
            );
            draw(&adapter);
        }

        press(window, Key::F5);
        draw(&adapter);
        assert!(window.get_dialog_project_open());
        // Tab traverses Close, Book, Export and then the title, never the background.
        for _ in 0..4 { press(window, Key::Tab); }
        press(window, "r");
        press(window, "7");
        assert_eq!(window.get_dialog_project_title(), "r7");
        assert!(rx.is_empty());
        press(window, Key::Escape);
        assert!(!window.get_dialog_project_open());
        press(window, "r");
        assert_eq!(
            rx.try_recv().unwrap(),
            Action::Record {
                duration_secs: None
            }
        );

        for height in [600.0, 1000.0] {
            adapter.set_size(slint::LogicalSize::new(1200.0, height));
            draw(&adapter);
            window.set_file_list_scroll_viewport_y(0.0);
            ui.set_current_file_index_only(99).unwrap();
            let viewport_height = window.get_file_list_visible_height();
            let scroll_y = window.get_file_list_scroll_viewport_y();
            assert!(viewport_height > 0.0);
            assert!((3000.0 + scroll_y - viewport_height).abs() < 1.0);
        }

        // A half-screen/non-maximized window must keep the dialog and Save button reachable.
        for scale_factor in [1.0, 1.25] {
            window.window().dispatch_event(WindowEvent::ScaleFactorChanged { scale_factor });
            for (width, height) in [(400.0_f32, 300.0_f32), (640.0, 480.0), (800.0, 800.0)] {
                adapter.set_size(slint::LogicalSize::new(width, height));
                for markers in [false, true] {
                    let (dialog_width, dialog_height) = if markers {
                        window.set_dialog_markers_list(slint::ModelRc::new(slint::VecModel::from(vec!["chapter".into()])));
                        window.set_dialog_markers_selected_index(0);
                        window.set_dialog_markers_open(true);
                        (900.0_f32.min(width - 20.0), 680.0_f32.min(height - 20.0))
                    } else {
                        window.set_dialog_project_open(true);
                        (520.0_f32.min(width - 20.0), 500.0_f32.min(height - 20.0))
                    };
                    let pixels = draw(&adapter);
                    let left = (width - dialog_width) / 2.0;
                    let top = (height - dialog_height) / 2.0;
                    for (x, y) in [(left + 5.0, top + 5.0), (left + dialog_width - 5.0, top + dialog_height - 5.0)] {
                        let index = (y * scale_factor) as usize * pixels.width() as usize + (x * scale_factor) as usize;
                        assert_eq!(pixels.as_slice()[index], slint::Rgb8Pixel::new(255, 255, 255));
                    }
                    let position = slint::LogicalPosition::new(left + dialog_width - 56.0, top + dialog_height - 28.0);
                    window.window().dispatch_event(WindowEvent::PointerPressed { position, button: PointerEventButton::Left });
                    window.window().dispatch_event(WindowEvent::PointerReleased { position, button: PointerEventButton::Left });
                    assert!(window.get_dialog_busy(), "Save outside visible dialog: {width}x{height}, scale={scale_factor}, markers={markers}");
                    let Action::SaveDialog { request_id, .. } = rx.try_recv().unwrap() else { panic!("Expected dialog save"); };
                    state.lock().unwrap().dialog_save_result = Some(DialogReply { request_id, result: Ok(None), field: None });
                    ui._timer.as_ref().unwrap().restart();
                    std::thread::sleep(Duration::from_millis(45));
                    draw(&adapter);
                    ui._timer.as_ref().unwrap().stop();
                    if markers { window.invoke_dialog_markers_cancel(); }
                    assert!(!window.get_modal_open());
                    assert!(rx.is_empty());
                }
            }
        }

        {
            let mut state = state.lock().unwrap();
            state.error_message = "Cannot save project\nDisk is full".to_owned();
            state.waveform_prev = vec![0.1, 0.2];
            state.waveform_current = vec![0.3, 0.4];
        }
        ui._timer.as_ref().unwrap().restart();
        std::thread::sleep(Duration::from_millis(45));
        draw(&adapter);
        assert_eq!(
            window.get_error_message(),
            "Cannot save project\nDisk is full"
        );
        assert!(!window.get_modal_open());
        window.invoke_dismiss_error();
        assert!(state.lock().unwrap().error_message.is_empty());
        {
            let mut state = state.lock().unwrap();
            state.is_recording = true;
            state.file_list[99].duration = "00:04".into();
        }
        let version = state.lock().unwrap().file_list_version;
        std::thread::sleep(Duration::from_millis(45));
        draw(&adapter);
        assert!(window.get_error_message().is_empty());
        assert_eq!(
            window.get_file_list().row_data(99).unwrap().duration,
            "00:04"
        );
        assert_eq!(state.lock().unwrap().file_list_version, version);

        {
            let mut state = state.lock().unwrap();
            state.is_recording = false;
            state.waveform_prev = vec![0.5, 0.6];
            state.waveform_current = vec![0.7, 0.8];
            state.file_list_version += 1;
        }
        std::thread::sleep(Duration::from_millis(45));
        draw(&adapter);
        assert_eq!(window.get_waveform_prev().row_data(0), Some(0.5));
        assert_eq!(window.get_waveform_current().row_data(0), Some(0.7));

        // Async results can replace same-length graphs without changing list or selection.
        let list_version = state.lock().unwrap().file_list_version;
        {
            let mut state = state.lock().unwrap();
            state.waveform_prev = vec![0.9, 1.0];
            state.waveform_current = vec![0.2, 0.1];
            state.waveform_version += 1;
        }
        std::thread::sleep(Duration::from_millis(45));
        draw(&adapter);
        assert_eq!(window.get_waveform_prev().row_data(0), Some(0.9));
        assert_eq!(window.get_waveform_current().row_data(0), Some(0.2));
        assert_eq!(state.lock().unwrap().file_list_version, list_version);
        {
            let mut state = state.lock().unwrap();
            state.waveform_prev.clear();
            state.waveform_current.clear();
            state.waveform_prev_loading = true;
            state.waveform_current_loading = true;
            state.waveform_version += 1;
        }
        std::thread::sleep(Duration::from_millis(45));
        draw(&adapter);
        assert!(window.get_waveform_prev_loading());
        assert!(window.get_waveform_current_loading());
        assert_eq!(window.get_waveform_prev().row_count(), 0);
        assert_eq!(window.get_waveform_current().row_count(), 0);
        {
            let mut state = state.lock().unwrap();
            state.waveform_prev = vec![0.8, 0.3];
            state.waveform_current = vec![0.4, 0.6];
            state.waveform_prev_loading = false;
            state.waveform_current_loading = false;
            state.waveform_version += 1;
        }
        std::thread::sleep(Duration::from_millis(45));
        draw(&adapter);
        assert!(!window.get_waveform_prev_loading());
        assert!(!window.get_waveform_current_loading());
        assert_eq!(window.get_waveform_prev().row_data(0), Some(0.8));
        assert_eq!(window.get_waveform_current().row_data(0), Some(0.4));
        assert_eq!(state.lock().unwrap().file_list_version, list_version);

        // Selection/playback patches coalesce between ticks without a full-list invalidation.
        {
            let mut state = state.lock().unwrap();
            state.file_list[0].is_playing = true;
            state.mark_file_list_row_changed(0);
        }
        std::thread::sleep(Duration::from_millis(45));
        draw(&adapter);
        assert!(window.get_file_list().row_data(0).unwrap().is_playing);
        let version = state.lock().unwrap().file_list_version;
        {
            let mut state = state.lock().unwrap();
            state.file_list[0].is_playing = false;
            state.mark_file_list_row_changed(0);
            state.file_list[1].is_playing = true;
            state.mark_file_list_row_changed(1);
            state.file_list[1].is_playing = false;
            state.mark_file_list_row_changed(1);
            state.file_list[2].is_playing = true;
            state.mark_file_list_row_changed(2);
            state.current_file_index = 2;
            state.mark_file_list_row_changed(usize::MAX);
            assert_eq!(state.file_list_dirty_rows.len(), 3);
        }
        std::thread::sleep(Duration::from_millis(45));
        draw(&adapter);
        assert!(!window.get_file_list().row_data(0).unwrap().is_playing);
        assert!(!window.get_file_list().row_data(1).unwrap().is_playing);
        assert!(window.get_file_list().row_data(2).unwrap().is_playing);
        assert_eq!(window.get_current_file_index(), 2);
        assert_eq!(state.lock().unwrap().file_list_version, version);
        assert!(state.lock().unwrap().file_list_dirty_rows.is_empty());

        // A structural snapshot supersedes old patches, including now-out-of-range rows.
        {
            let mut state = state.lock().unwrap();
            state.mark_file_list_row_changed(2);
            state.file_list.truncate(1);
            state.file_list[0].name = "replacement".into();
            state.file_list_version += 1;
        }
        std::thread::sleep(Duration::from_millis(45));
        draw(&adapter);
        assert_eq!(window.get_file_list().row_count(), 1);
        assert_eq!(window.get_file_list().row_data(0).unwrap().name, "replacement");
        assert!(state.lock().unwrap().file_list_dirty_rows.is_empty());
    })
    .join()
    .unwrap();
}
