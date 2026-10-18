use super::Fixture;
use super::support::export_observer::create_barrier;
use iamreader_engine::engine::EngineCommand;

#[test]
fn export_offline_without_opening_audio_or_initializing_gui() {
    let mut fixture = Fixture::create();
    assert!(fixture.audio.borrow().calls.is_empty());

    fixture.execute(EngineCommand::Compile).unwrap();
    fixture.engine.wait_for_export().unwrap();

    let status = fixture.handle.export_status().unwrap();
    assert_eq!(status.progress, 1.0);
    assert!(status.stage.starts_with("Done:"));
    assert!(status.error_message.is_empty());
    assert!(fixture.directory.path().join("export/00001.wav").is_file());
    assert!(fixture.audio.borrow().calls.is_empty());
}

#[test]
fn reject_duplicate_export_and_cancel_before_publication() {
    let mut fixture = Fixture::create();
    let output = fixture.directory.path().join("export");
    std::fs::create_dir(&output).unwrap();
    std::fs::write(output.join("previous.wav"), b"previous book").unwrap();
    let (observer, entered, release) = create_barrier(false);
    fixture.engine = fixture.engine.with_export_observer(observer);
    fixture.execute(EngineCommand::Compile).unwrap();
    entered.recv().unwrap();

    let error = fixture.execute(EngineCommand::Compile).unwrap_err();
    assert!(error.message.contains("already running"));
    fixture.handle.request_export_cancellation().unwrap();
    assert!(fixture.handle.export_status().unwrap().cancel_requested);
    release.send(()).unwrap();
    fixture.engine.wait_for_export().unwrap();

    assert_eq!(fixture.handle.export_status().unwrap().stage, "Cancelled; previous export unchanged");
    assert_eq!(std::fs::read(output.join("previous.wav")).unwrap(), b"previous book");
    assert_eq!(std::fs::read_dir(output).unwrap().count(), 1);
    assert!(fixture.audio.borrow().calls.is_empty());
}

#[test]
fn ignore_late_cancel_after_the_publication_gate() {
    let mut fixture = Fixture::create();
    let (observer, entered, release) = create_barrier(true);
    fixture.engine = fixture.engine.with_export_observer(observer);
    fixture.execute(EngineCommand::Compile).unwrap();
    entered.recv().unwrap();
    assert!(fixture.handle.export_status().unwrap().publishing);

    fixture.handle.request_export_cancellation().unwrap();
    assert!(!fixture.handle.export_status().unwrap().cancel_requested);
    release.send(()).unwrap();
    fixture.engine.wait_for_export().unwrap();

    assert_eq!(fixture.handle.export_status().unwrap().progress, 1.0);
    assert!(fixture.directory.path().join("export/00001.wav").is_file());
}
