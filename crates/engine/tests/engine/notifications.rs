use super::Fixture;
use iamreader_engine::engine::EngineCommand;
use std::time::Duration;

#[test]
fn coalesce_updates_per_observer_and_remain_silent_when_idle() {
    let mut fixture = Fixture::create();
    let first = fixture.handle.subscribe_updates().unwrap();
    let second = fixture.handle.subscribe_updates().unwrap();
    assert_eq!(first.try_iter().count(), 1);
    assert_eq!(second.try_iter().count(), 1);
    fixture.engine.tick().unwrap();
    first.try_iter().for_each(drop);
    second.try_iter().for_each(drop);
    for _ in 0..10 { fixture.advance(Duration::from_millis(50)); }

    assert!(first.is_empty());
    assert!(second.is_empty());
    for _ in 0..64 { fixture.execute(EngineCommand::Stop).unwrap(); }
    assert_eq!(first.len(), 1);
    assert_eq!(second.len(), 1);
    first.recv().unwrap();
    assert_eq!(second.len(), 1);
    fixture.engine.shutdown().unwrap();
    assert_eq!(first.len(), 1);
    assert!(fixture.handle.snapshot().unwrap().stopped);
    assert_eq!(fixture.audio.borrow().active_recordings, 0);
}

#[test]
fn limit_live_notifications_with_injected_time() {
    let mut fixture = Fixture::create();
    fixture.execute(EngineCommand::StartMonitoring).unwrap();
    let updates = fixture.handle.subscribe_updates().unwrap();
    updates.recv().unwrap();
    fixture.audio.borrow().levels.as_ref().unwrap().send(0.5).unwrap();
    fixture.advance(Duration::from_millis(49));
    assert!(updates.is_empty());
    fixture.advance(Duration::from_millis(1));

    assert_eq!(updates.len(), 1);
    assert!(fixture.handle.snapshot().unwrap().level > 0.0);
    updates.recv().unwrap();
    fixture.advance(Duration::from_millis(50));
    assert!(updates.is_empty());
}

#[test]
fn wake_observers_on_export_progress_and_terminal_result_without_a_gui() {
    let mut fixture = Fixture::create();
    let (observer, entered, release) = super::support::export_observer::create_barrier(false);
    fixture.engine = fixture.engine.with_export_observer(observer);
    let updates = fixture.handle.subscribe_updates().unwrap();
    updates.recv().unwrap();
    fixture.execute(EngineCommand::Compile).unwrap();
    entered.recv().unwrap();
    assert_eq!(updates.try_iter().count(), 1);

    release.send(()).unwrap();
    fixture.engine.wait_for_export().unwrap();

    assert_eq!(updates.len(), 1);
    assert!(fixture.handle.export_status().unwrap().finished_at.is_some());
    assert!(fixture.audio.borrow().calls.is_empty());
}
