use super::{chunk, create_metadata, Fixture};
use iamreader_engine::engine::{EngineCommand, EngineEvent, ProjectEdit};
use iamreader_engine::utils::transcription::TranscriptionUpdate;
use std::time::Duration;

#[test]
fn preserve_append_update_insert_workflows_with_injected_time() {
    for (mode, expected_order) in [
        (None, vec![chunk(1), chunk(4)]),
        (Some(EngineCommand::ModeUpdate), vec![chunk(1), chunk(4), chunk(3)]),
        (Some(EngineCommand::ModeInsert), vec![chunk(1), chunk(2), chunk(4), chunk(3)]),
    ] {
        let mut fixture = Fixture::create();
        fixture.select(2);
        if let Some(mode) = mode { fixture.execute(mode).unwrap(); }

        fixture.execute(EngineCommand::Record { duration_secs: Some(1) }).unwrap();
        assert!(fixture.handle.snapshot().unwrap().is_recording);
        assert_eq!(fixture.audio.borrow().active_recordings, 1);
        fixture.advance(Duration::from_secs(1));

        let snapshot = fixture.handle.snapshot().unwrap();
        assert_eq!(snapshot.chunk_order, expected_order);
        assert_eq!(snapshot.recording_mode, "A");
        assert!(!snapshot.is_recording);
        assert_eq!(fixture.audio.borrow().active_recordings, 0);
        assert_eq!(fixture.load_project().next_chunk_number, 5);
        assert_eq!(fixture.load_project().stats.record_length, 10);
        assert_eq!(snapshot.selected_chunk, Some(chunk(4)));
    }
}

#[test]
fn restart_and_discard_takes_without_reusing_reserved_numbers() {
    let mut fixture = Fixture::create();
    fixture.select(2);
    fixture.execute(EngineCommand::ModeUpdate).unwrap();
    fixture.execute(EngineCommand::Record { duration_secs: None }).unwrap();
    fixture.execute(EngineCommand::Record { duration_secs: None }).unwrap();
    fixture.execute(EngineCommand::Stop).unwrap();

    assert!(!fixture.directory.path().join(chunk(4).0).exists());
    assert!(!fixture.directory.path().join(chunk(5).0).exists());
    assert!(fixture.directory.path().join(chunk(2).0).exists());
    assert_eq!(fixture.load_project().next_chunk_number, 6);
    assert_eq!(fixture.audio.borrow().active_recordings, 0);
    fixture.execute(EngineCommand::Record { duration_secs: Some(1) }).unwrap();
    fixture.advance(Duration::from_secs(1));

    let project = fixture.load_project();
    assert_eq!(project.files[1].path, chunk(6).0);
    assert_eq!(project.files[1].title, "Synthetic fragment 2");
    assert_eq!(project.files[1].hint, "середина");
    assert_eq!(project.next_chunk_number, 7);
    assert!(!fixture.directory.path().join(chunk(2).0).exists());
}

#[test]
fn retain_accepted_recording_on_input_and_finalization_failures() {
    let mut fixture = Fixture::create();
    fixture.select(2);
    fixture.execute(EngineCommand::ModeUpdate).unwrap();
    let previous = std::fs::read(fixture.directory.path().join(chunk(2).0)).unwrap();
    fixture.audio.borrow_mut().fail_recording = true;

    let error = fixture.execute(EngineCommand::Record { duration_secs: Some(1) }).unwrap_err();

    assert!(error.message.contains("Synthetic input unavailable"));
    assert_eq!(fixture.load_project().files[1].path, chunk(2).0);
    fixture.audio.borrow_mut().fail_recording = false;
    fixture.audio.borrow_mut().fail_finalization = true;
    fixture.execute(EngineCommand::Record { duration_secs: Some(1) }).unwrap();
    fixture.advance(Duration::from_secs(1));

    assert!(fixture.handle.snapshot().unwrap().error_message.contains("Synthetic recording finalization failure"));
    assert!(!fixture.handle.snapshot().unwrap().is_recording);
    assert_eq!(fixture.load_project().files[1].path, chunk(2).0);
    assert_eq!(std::fs::read(fixture.directory.path().join(chunk(2).0)).unwrap(), previous);
    assert_eq!(fixture.audio.borrow().active_recordings, 0);
}

#[test]
fn preserve_project_and_accepted_wav_when_persistence_fails() {
    for fail_after_start in [false, true] {
        let mut fixture = Fixture::create();
        fixture.select(2);
        fixture.execute(EngineCommand::ModeUpdate).unwrap();
        if fail_after_start { fixture.execute(EngineCommand::Record { duration_secs: Some(1) }).unwrap(); }
        let backup = fixture.directory.path().join("saved-project.json");
        std::fs::rename(&fixture.project_path, &backup).unwrap();
        std::fs::create_dir(&fixture.project_path).unwrap();

        if fail_after_start {
            fixture.advance(Duration::from_secs(1));
            assert!(fixture.handle.snapshot().unwrap().error_message.contains("Could not confirm recording completion"));
        } else {
            let error = fixture.execute(EngineCommand::Record { duration_secs: Some(1) }).unwrap_err();
            assert!(error.message.contains("Could not save the project before recording"));
            assert!(fixture.audio.borrow().calls.is_empty());
        }

        assert!(fixture.directory.path().join(chunk(2).0).is_file());
        assert_eq!(fixture.handle.snapshot().unwrap().chunk_order, vec![chunk(1), chunk(2), chunk(3)]);
        std::fs::remove_dir(&fixture.project_path).unwrap();
        std::fs::rename(backup, &fixture.project_path).unwrap();
    }
}

#[test]
fn navigate_and_search_without_reading_frontend_state() {
    let mut fixture = Fixture::create();
    fixture.select(2);
    let version = fixture.handle.snapshot().unwrap().file_list_version;

    fixture.execute(EngineCommand::SearchHintDown("ТЕКСТ".into())).unwrap();
    assert_eq!(fixture.handle.snapshot().unwrap().selected_chunk, Some(chunk(1)));
    fixture.execute(EngineCommand::SearchHintUp("текст".into())).unwrap();
    assert_eq!(fixture.handle.snapshot().unwrap().selected_chunk, Some(chunk(3)));
    assert_eq!(fixture.handle.snapshot().unwrap().file_list_version, version);
    assert!(fixture.audio.borrow().calls.is_empty());
    fixture.execute(EngineCommand::PrevSect).unwrap();
    assert_eq!(fixture.handle.snapshot().unwrap().selected_chunk, Some(chunk(1)));
    fixture.execute(EngineCommand::NextSect).unwrap();
    assert_eq!(fixture.handle.snapshot().unwrap().selected_chunk, Some(chunk(3)));
    assert_eq!(fixture.audio.borrow().active_players, 1);
    fixture.audio.borrow_mut().playback_finished = true;
    fixture.engine.tick().unwrap();

    assert_eq!(fixture.audio.borrow().active_players, 0);
    assert!(fixture.handle.snapshot().unwrap().file_list.iter().all(|row| !row.is_playing));
}

#[test]
fn acknowledge_validation_and_save_errors_without_committing_drafts() {
    let mut fixture = Fixture::create();
    let before = std::fs::read(&fixture.project_path).unwrap();
    let original_title = fixture.handle.snapshot().unwrap().meta.title;
    let mut invalid = create_metadata("Uncommitted title");
    invalid.format_audio = "unknown".into();

    let error = fixture.execute(EngineCommand::SaveProjectEdit(Box::new(ProjectEdit::Meta(invalid)))).unwrap_err();

    assert_eq!(error.field.as_deref(), Some("format_audio"));
    assert!(error.message.contains("format"));
    assert_eq!(std::fs::read(&fixture.project_path).unwrap(), before);
    assert_eq!(fixture.handle.snapshot().unwrap().meta.title, original_title);
    assert!(fixture.handle.snapshot().unwrap().error_message.is_empty());
    let backup = fixture.directory.path().join("saved-project.json");
    std::fs::rename(&fixture.project_path, &backup).unwrap();
    std::fs::create_dir(&fixture.project_path).unwrap();

    let error = fixture.execute(EngineCommand::SaveProjectEdit(Box::new(ProjectEdit::Meta(create_metadata("Still uncommitted"))))).unwrap_err();

    assert!(error.field.is_none());
    assert!(!error.message.is_empty());
    assert_eq!(fixture.handle.snapshot().unwrap().meta.title, original_title);
    std::fs::remove_dir(&fixture.project_path).unwrap();
    std::fs::rename(backup, &fixture.project_path).unwrap();
}

#[test]
fn preserve_identity_and_reject_stale_transcriptions_after_deletion() {
    let mut fixture = Fixture::create();
    fixture.execute(EngineCommand::DeleteChunk { chunk: chunk(1) }).unwrap();
    fixture.select(2);
    assert_eq!(fixture.handle.snapshot().unwrap().selected_chunk, Some(chunk(2)));
    assert!(fixture.engine.apply_transcription_update(TranscriptionUpdate {
        file_path: chunk(2).0.into(), previous_hint: "середина".into(), text: "Recognized text".into(),
    }).unwrap());
    assert!(!fixture.engine.apply_transcription_update(TranscriptionUpdate {
        file_path: chunk(2).0.into(), previous_hint: "середина".into(), text: "Late duplicate".into(),
    }).unwrap());
    fixture.execute(EngineCommand::DeleteChunk { chunk: chunk(2) }).unwrap();
    assert!(!fixture.engine.apply_transcription_update(TranscriptionUpdate {
        file_path: chunk(2).0.into(), previous_hint: "Recognized text".into(), text: "Deleted source".into(),
    }).unwrap());
    let error = fixture.execute(EngineCommand::Select { chunk: Some(chunk(2)), play: false }).unwrap_err();

    assert!(error.message.contains("no longer available"));
    assert_eq!(fixture.load_project().files.len(), 1);
}

#[test]
fn bound_commands_and_acknowledge_every_accepted_request() {
    let mut fixture = Fixture::create();
    let tickets: Vec<_> = (0..64).map(|_| fixture.handle.submit(EngineCommand::Stop).unwrap()).collect();

    let error = fixture.handle.submit(EngineCommand::Stop).err().unwrap();
    assert!(error.to_string().contains("full"));
    fixture.engine.tick().unwrap();

    for (index, ticket) in tickets.into_iter().enumerate() {
        let EngineEvent::CommandCompleted { request_id, result } = ticket.recv().unwrap();
        assert_eq!(request_id, index as u64 + 1);
        assert!(result.is_ok());
    }
}

#[test]
fn release_playback_and_reject_commands_after_shutdown() {
    let mut fixture = Fixture::create();
    fixture.execute(EngineCommand::Play).unwrap();
    fixture.execute(EngineCommand::Stop).unwrap();
    assert_eq!(fixture.audio.borrow().active_players, 0);
    assert!(!fixture.handle.snapshot().unwrap().file_list.iter().any(|row| row.is_playing));
    fixture.execute(EngineCommand::Play).unwrap();
    let shutdown = fixture.handle.submit(EngineCommand::Shutdown).unwrap();
    let after_shutdown = fixture.handle.submit(EngineCommand::Play).unwrap();

    fixture.engine.tick().unwrap();

    assert!(matches!(shutdown.recv().unwrap(), EngineEvent::CommandCompleted { result: Ok(_), .. }));
    assert!(matches!(after_shutdown.recv().unwrap(), EngineEvent::CommandCompleted { result: Err(_), .. }));
    assert!(fixture.handle.snapshot().unwrap().stopped);
    assert_eq!(fixture.audio.borrow().active_players, 0);
    assert!(fixture.audio.borrow().calls.contains(&"stop_monitoring"));
    assert!(fixture.audio.borrow().calls.contains(&"shutdown"));
    assert!(fixture.handle.submit(EngineCommand::Stop).is_err());
}

#[test]
fn finish_shutdown_even_when_the_audio_backend_reports_an_error() {
    let mut fixture = Fixture::create();
    fixture.execute(EngineCommand::StartMonitoring).unwrap();
    fixture.execute(EngineCommand::Play).unwrap();
    fixture.audio.borrow_mut().fail_shutdown = true;

    let error = fixture.execute(EngineCommand::Shutdown).unwrap_err();

    assert!(error.message.contains("Synthetic backend shutdown failure"));
    assert!(fixture.handle.snapshot().unwrap().stopped);
    assert_eq!(fixture.audio.borrow().active_players, 0);
    assert!(fixture.audio.borrow().levels.is_none());
    assert!(fixture.handle.submit(EngineCommand::Play).is_err());
    fixture.engine.shutdown().unwrap();
    assert_eq!(fixture.audio.borrow().calls.iter().filter(|call| **call == "shutdown").count(), 1);
}

#[test]
fn bound_level_telemetry_and_recording_waveform_without_waiting() {
    let mut fixture = Fixture::create();
    fixture.execute(EngineCommand::StartMonitoring).unwrap();
    let levels = fixture.audio.borrow().levels.clone().unwrap();
    let accepted = (0..1000).filter(|_| levels.try_send(0.5).is_ok()).count();
    assert_eq!(accepted, 8);
    fixture.engine.tick().unwrap();
    assert!(fixture.handle.snapshot().unwrap().level > 0.0);
    fixture.execute(EngineCommand::StopMonitoring).unwrap();
    fixture.execute(EngineCommand::ModeUpdate).unwrap();
    fixture.execute(EngineCommand::Record { duration_secs: None }).unwrap();
    let levels = fixture.audio.borrow().levels.clone().unwrap();

    for _ in 0..200 {
        for _ in 0..8 { levels.try_send(0.5).unwrap(); }
        fixture.advance(Duration::from_millis(50));
    }

    assert_eq!(fixture.handle.snapshot().unwrap().waveform_current.len(), 1500);
    fixture.execute(EngineCommand::Stop).unwrap();
    assert_eq!(fixture.audio.borrow().active_recordings, 0);
}

#[test]
fn confirm_append_take_and_continue_but_finish_edit_modes() {
    let mut fixture = Fixture::create();
    fixture.execute(EngineCommand::Ok).unwrap();
    fixture.execute(EngineCommand::Ok).unwrap();

    assert_eq!(fixture.load_project().files.len(), 4);
    assert!(fixture.handle.snapshot().unwrap().is_recording);
    assert_eq!(fixture.audio.borrow().active_recordings, 1);
    fixture.execute(EngineCommand::Stop).unwrap();
    assert!(!fixture.directory.path().join(chunk(5).0).exists());
    assert_eq!(fixture.load_project().files.last().unwrap().path, chunk(4).0);
    for mode in [EngineCommand::ModeUpdate, EngineCommand::ModeInsert] {
        let mut fixture = Fixture::create();
        fixture.select(2);
        fixture.execute(mode).unwrap();
        fixture.execute(EngineCommand::Record { duration_secs: None }).unwrap();

        fixture.execute(EngineCommand::Ok).unwrap();

        assert!(!fixture.handle.snapshot().unwrap().is_recording);
        assert_eq!(fixture.audio.borrow().active_recordings, 0);
        assert_eq!(fixture.handle.snapshot().unwrap().selected_chunk, Some(chunk(4)));
    }
}

#[test]
fn report_playback_backend_failures_without_leaking_sessions() {
    let mut fixture = Fixture::create();
    fixture.audio.borrow_mut().fail_playback = true;

    let error = fixture.execute(EngineCommand::Play).unwrap_err();

    assert!(error.message.contains("Synthetic output unavailable"));
    assert_eq!(fixture.audio.borrow().active_players, 0);
    assert!(fixture.handle.snapshot().unwrap().file_list.iter().all(|row| !row.is_playing));
    fixture.audio.borrow_mut().fail_playback = false;
    fixture.execute(EngineCommand::Play).unwrap();
    fixture.audio.borrow_mut().fail_stop_playback = true;
    fixture.audio.borrow_mut().playback_finished = true;

    fixture.engine.tick().unwrap();

    assert_eq!(fixture.audio.borrow().active_players, 0);
    let snapshot = fixture.handle.snapshot().unwrap();
    assert!(snapshot.error_message.contains("Synthetic playback stop failure"));
    assert!(snapshot.file_list.iter().all(|row| !row.is_playing));
    assert!(!snapshot.stopped);
}
