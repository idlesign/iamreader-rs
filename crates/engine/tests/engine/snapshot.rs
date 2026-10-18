use super::{chunk, Fixture};
use iamreader_engine::engine::EngineCommand;

#[test]
fn expose_incremental_rows_and_signal_lost_patch_history() {
    let mut fixture = Fixture::create();
    fixture.execute(EngineCommand::Play).unwrap();
    let initial = fixture.handle.snapshot().unwrap();
    let initial_cursor = initial.row_update_version();

    fixture.execute(EngineCommand::Select { chunk: Some(chunk(1)), play: true }).unwrap();

    let selected = fixture.handle.snapshot().unwrap();
    let changed_rows = selected.changed_rows_since(initial_cursor).unwrap();
    assert!(changed_rows.contains(&0));
    assert!(changed_rows.contains(&2));
    assert_eq!(selected.file_list_version, initial.file_list_version);
    assert_eq!(selected.changed_rows_since(selected.row_update_version()), Some(vec![]));

    for number in [2, 1].into_iter().cycle().take(70) {
        fixture.execute(EngineCommand::Select { chunk: Some(chunk(number)), play: true }).unwrap();
    }

    let current = fixture.handle.snapshot().unwrap();
    assert_eq!(current.file_list_version, initial.file_list_version);
    assert_eq!(current.changed_rows_since(initial_cursor), None);
    assert_eq!(current.changed_rows_since(current.row_update_version()), Some(vec![]));
}
