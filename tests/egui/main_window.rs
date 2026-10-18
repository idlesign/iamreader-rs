use super::{create_harness, Fixture};
use eframe::egui;
use egui_kittest::kittest::{By, NodeT, Queryable};
use iamreader_engine::engine::{Engine, EngineCommand};
use iamreader_engine::project::Project;
use std::time::Duration;

#[test]
fn select_stable_chunks_and_search_without_opening_audio() {
    let mut fixture = Fixture::create();
    let mut harness = create_harness(&fixture, [1000.0, 700.0], 1.0);
    harness.get_by_label_contains("Synthetic fragment 1").click();
    harness.run();
    fixture.engine.tick().unwrap();
    harness.run();

    assert_eq!(fixture.handle.snapshot().unwrap().selected_chunk, Some(super::support::chunk(1)));
    assert_eq!(harness.get_by_label_contains("Synthetic fragment 1").accesskit_node().toggled(), Some(egui::accesskit::Toggled::True));
    assert!(fixture.audio.borrow().calls.is_empty());
    harness.get_by_label("Search").focus();
    harness.get_by_label("Search").type_text("текст");
    harness.run();
    harness.get_by_label("Previous match").click();
    harness.run();
    fixture.engine.tick().unwrap();
    harness.run();

    assert_eq!(fixture.handle.snapshot().unwrap().selected_chunk, Some(super::support::chunk(3)));
    harness.get_by_label("Next match").click();
    harness.run();
    fixture.engine.tick().unwrap();
    harness.run();
    assert_eq!(fixture.handle.snapshot().unwrap().selected_chunk, Some(super::support::chunk(1)));
    assert!(fixture.audio.borrow().calls.is_empty());
}

#[test]
fn redraw_waveforms_for_the_selected_stable_chunk() {
    let mut fixture = Fixture::create();
    let mut writer = hound::WavWriter::create(fixture.directory.path().join(super::support::chunk(2).0), hound::WavSpec {
        channels: 1, sample_rate: 48_000, bits_per_sample: 16, sample_format: hound::SampleFormat::Int,
    }).unwrap();
    for _ in 0..480 { writer.write_sample(24_576_i16).unwrap(); }
    writer.finalize().unwrap();
    let mut harness = create_harness(&fixture, [1000.0, 700.0], 1.0);
    let mut amplitudes = Vec::new();

    for number in [1, 2] {
        harness.get_by_label_contains(&format!("Synthetic fragment {number}")).click();
        harness.run();
        fixture.engine.tick().unwrap();
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        loop {
            fixture.advance(Duration::from_millis(50));
            let snapshot = fixture.handle.snapshot().unwrap();
            if !snapshot.waveform_current_loading && !snapshot.waveform_current.is_empty() { break; }
            assert!(std::time::Instant::now() < deadline, "Waveform worker did not finish");
            std::thread::sleep(Duration::from_millis(2));
        }
        harness.run();
        let waveform_rect = harness.get_by_label("Current waveform").rect();
        let peak = harness.output().shapes.iter().filter_map(|clipped| {
            let egui::Shape::LineSegment { points, stroke } = &clipped.shape else { return None; };
            (stroke.color == egui::Color32::from_rgb(173, 216, 230)
                && waveform_rect.contains(points[0]) && waveform_rect.contains(points[1]))
                .then_some((points[1].y - points[0].y).abs())
        }).fold(0.0_f32, f32::max);
        let snapshot = fixture.handle.snapshot().unwrap();
        let expected = snapshot.waveform_current.iter().copied().fold(0.0_f32, f32::max) * (waveform_rect.height() - 2.0);
        assert!((peak - expected).abs() < 0.01, "Painted {peak}, expected {expected}");
        amplitudes.push(peak);
        assert_eq!(fixture.handle.snapshot().unwrap().selected_chunk, Some(super::support::chunk(number)));
    }

    assert!(amplitudes[0] > 0.0);
    assert!(amplitudes[1] > amplitudes[0], "{amplitudes:?}");
    assert!(fixture.audio.borrow().calls.is_empty());
}

#[test]
fn preserve_record_accept_and_mode_controls() {
    let mut fixture = Fixture::create();
    let mut harness = create_harness(&fixture, [1000.0, 700.0], 1.0);
    harness.get_by_label("U").click();
    harness.run();
    fixture.engine.tick().unwrap();
    harness.run();
    assert_eq!(fixture.handle.snapshot().unwrap().recording_mode, "U");
    harness.key_press(egui::Key::R);
    harness.run();
    fixture.engine.tick().unwrap();
    fixture.advance(Duration::from_secs(1));
    harness.run();
    harness.key_press(egui::Key::E);
    harness.run();
    fixture.engine.tick().unwrap();
    harness.run();

    let snapshot = fixture.handle.snapshot().unwrap();
    assert!(!snapshot.is_recording);
    assert_eq!(snapshot.recording_mode, "A");
    assert_eq!(snapshot.chunk_order.len(), 3);
    assert_eq!(snapshot.selected_chunk, Some(super::support::chunk(4)));
    assert_eq!(fixture.audio.borrow().active_recordings, 0);
    harness.get_by_label("I").click();
    harness.run();
    fixture.engine.tick().unwrap();
    harness.run();
    assert_eq!(fixture.handle.snapshot().unwrap().recording_mode, "I");
}

#[test]
fn virtualize_large_lists_and_scroll_only_when_selection_leaves_the_view() {
    let mut fixture = Fixture::create();
    let mut project = Project::load(&fixture.project_path).unwrap();
    let prototype = project.files[0].clone();
    for number in 4..=10_000 {
        let mut row = prototype.clone();
        row.path = format!("synthetic/{number:05}.wav");
        row.title = format!("Synthetic fragment {number}");
        project.files.push(row);
    }
    project.save(&fixture.project_path).unwrap();
    let (audio, control) = super::support::SyntheticAudio::new();
    let (engine, handle) = Engine::open_with_backend(fixture.project_path.clone(), false, Box::new(audio)).unwrap();
    fixture.engine = engine.with_transcription_model(None);
    fixture.handle = handle;
    fixture.audio = control;
    let mut harness = create_harness(&fixture, [1000.0, 700.0], 1.0);

    assert!(harness.query_by_label_contains("Synthetic fragment 1 ·").is_none());
    assert!(harness.query_all(By::new().role(egui::Role::Button).label_contains("Synthetic fragment")).count() < 30);
    fixture.select(1);
    harness.run();

    let selected = harness.get_by_label_contains("Synthetic fragment 1 ·");
    assert!(selected.rect().bottom() <= 700.0);
    assert!(selected.rect().top() >= 0.0);
    assert!(harness.query(By::new().role(egui::Role::Button).label_contains("Synthetic fragment 10000")).is_none());
    assert!(harness.query_all(By::new().role(egui::Role::Button).label_contains("Synthetic fragment")).count() < 30);
    fixture.select(2);
    harness.run();
    assert!(harness.get_by_label_contains("Synthetic fragment 2 ·").rect().bottom() <= 700.0);
}

#[test]
fn draw_cyrillic_in_narrow_hidpi_windows_and_settle_without_an_idle_timer() {
    let mut fixture = Fixture::create();
    let mut metadata = super::support::create_metadata("Книга: Ёжик и чай");
    metadata.author = "Автор: Сергей".into();
    fixture.execute(EngineCommand::SaveProjectEdit(Box::new(iamreader_engine::engine::ProjectEdit::Meta(metadata)))).unwrap();
    let mut harness = create_harness(&fixture, [400.0, 360.0], 2.0);

    assert!(harness.get_by_label_contains("Книга: Ёжик и чай").rect().right() <= 400.0);
    assert!(harness.get_by_label("Shortcuts F1").rect().bottom() <= 360.0);
    assert!(harness.get_by_label("Search").rect().right() <= 400.0);
    assert!(harness.run() <= 2);
    let output = harness.output();
    assert!(output.viewport_output[&egui::ViewportId::ROOT].repaint_delay > Duration::from_secs(60));
}

#[test]
fn preserve_slint_panel_order_table_columns_and_fixed_summary_at_all_window_sizes() {
    let mut fixture = Fixture::create();
    fixture.execute(EngineCommand::SaveProjectEdit(Box::new(iamreader_engine::engine::ProjectEdit::Meta(
        super::support::create_metadata("Весенний вопрос"))))).unwrap();
    let snapshot = fixture.handle.snapshot().unwrap();
    let summary = format!("{} · {} · {}", snapshot.total_files, snapshot.total_duration, snapshot.total_size);

    for (size, scale) in [([400.0, 300.0], 2.0), ([720.0, 560.0], 1.5), ([1200.0, 800.0], 1.0)] {
        let harness = create_harness(&fixture, size, scale);
        let bounds = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::Vec2::from(size));
        for label in ["Project F5", "Markers F4", "TOC F10", "Compile F8", "Shortcuts F1", "A", "U", "I", "Search",
            "Previous match", "Next match", "Previous waveform", "Current waveform", "Input level (dB FS)"] {
            assert!(bounds.contains_rect(harness.get_by_label(label).rect()), "{size:?}: {label}");
        }
        assert!(harness.query_by_label("Actions").is_none());
        assert!(harness.query_by_label("…").is_none());
        assert!(harness.get_by_label("Compile F8").rect().right() < harness.get_by_label("Shortcuts F1").rect().left());
        assert!(harness.get_by_label("Shortcuts F1").rect().right() < harness.get_by_label("A").rect().left());
        let previous = harness.get_by_label("Previous waveform").rect();
        let current = harness.get_by_label("Current waveform").rect();
        let graphs = previous.union(current).union(harness.get_by_label("Input level (dB FS)").rect());
        let bars: Vec<_> = harness.output().shapes.iter().filter_map(|shape| {
            let egui::Shape::Rect(rectangle) = &shape.shape else { return None; };
            let is_bar = rectangle.rect.width() > 100.0
                && [24.0, 34.0].contains(&rectangle.rect.height())
                && !graphs.contains_rect(rectangle.rect)
                && [egui::Color32::from_gray(240), egui::Color32::from_gray(245), egui::Color32::from_gray(232),
                    egui::Color32::from_rgb(208, 232, 255)].contains(&rectangle.fill);
            is_bar.then_some(rectangle.rect)
        }).collect();
        assert!(bars.len() >= 5, "{size:?}: missing panel backgrounds");
        for bar in bars {
            assert!((bar.left() - previous.left()).abs() < 0.1, "{size:?}: uneven left edge {bar:?}");
            assert!((bar.right() - harness.get_by_label("Input level (dB FS)").rect().right()).abs() < 0.1,
                "{size:?}: uneven right edge {bar:?}");
        }
        assert!((harness.get_by_label("Search").rect().left() - previous.left()).abs() < 0.1);
        assert!((harness.get_by_label("Next match").rect().right()
            - harness.get_by_label("Input level (dB FS)").rect().right()).abs() < 0.1);
        assert!((previous.top() - current.top()).abs() < 0.1);
        assert!(previous.right() < current.left());
        assert!(current.right() < harness.get_by_label("Input level (dB FS)").rect().left());
        assert!(harness.get_by_label_contains("Весенний вопрос").rect().bottom() < previous.top());
        assert!(harness.get_by_label(&snapshot.section_titles).rect().bottom() < previous.top());
        assert!(current.bottom() < harness.get_by_label("новый текст").rect().top());
        assert!(harness.get_by_label("новый текст").rect().bottom() < harness.get_by_label("середина").rect().top());
        assert!(harness.get_by_label("середина").rect().bottom() < harness.get_by_label("Search").rect().top());
        let footer = harness.get_by_label(&summary).rect();
        assert!(bounds.contains_rect(footer), "{size:?}: footer {footer:?}");
        assert!(footer.bottom() > size[1] - 32.0, "{size:?}: footer {footer:?}");
        assert!(harness.query_by_label("Record").is_none());
        let text: Vec<_> = harness.output().shapes.iter().filter_map(|shape| {
            let egui::Shape::Text(text) = &shape.shape else { return None; };
            Some(text)
        }).collect();
        for painted in &text {
            assert!(painted.galley.job.sections.iter().all(|section| section.format.font_id.size >= 14.0),
                "{size:?}: small font: {}", painted.galley.text());
        }
        let total_size = text.iter().find(|text| text.galley.text() == snapshot.total_size).unwrap();
        assert!(!total_size.galley.elided, "{size:?}: clipped summary: {}", snapshot.total_size);
        if size[0] >= 720.0 {
            for value in ["Project F5", "Markers F4", "TOC F10", "Compile F8", "Shortcuts F1",
                &snapshot.section_titles, "новый текст", "середина", "00003.wav",
                &snapshot.file_list[0].duration, &snapshot.file_list[0].start_time, &snapshot.file_list[0].size] {
                let painted = text.iter().find(|text| text.galley.text() == value).unwrap_or_else(|| panic!("Missing visible text: {value}"));
                assert!(painted.galley.job.sections.iter().all(|section| section.format.font_id.size >= 16.0), "Small font: {value}");
                assert!(!painted.galley.elided, "{size:?}: clipped text: {value}");
            }
            assert!(text.iter().any(|text| text.galley.text() == "-60"), "{size:?}: missing lower level mark");
        }
    }
    assert!(fixture.audio.borrow().calls.is_empty());
}
