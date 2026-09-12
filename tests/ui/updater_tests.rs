use super::{
    adjacent_section_index, file_list_current_index, update_playing_rows,
    update_recording_row_duration, UiUpdateCache, UiUpdateKey,
};
use crate::app::app::RecordingMode;
use crate::ui::FileInfo;
use std::time::{Duration, Instant};

fn idle_key() -> UiUpdateKey {
    UiUpdateKey {
        current_index: Some(4),
        total_files: 5,
        recording_mode: RecordingMode::Append,
        recording_started: None,
        playing_index: None,
        record_length_ms: 100_000,
    }
}

#[test]
fn unchanged_ticks_do_not_rebuild_project_snapshot() {
    let key = idle_key();
    let mut cache = UiUpdateCache::default();
    assert!(cache.needs_rebuild(key, false));
    cache.record_rebuild(key, Duration::from_secs(100), 4000, vec![]);
    for _ in 0..16 {
        assert!(!cache.needs_rebuild(key, false));
        assert!(!cache.needs_navigation_update(key));
    }
    // Title/hint/marker edits need invalidation even when length stays the same.
    assert!(cache.needs_rebuild(key, true));
}

#[test]
fn structural_and_data_transitions_rebuild() {
    let key = idle_key();
    let mut cache = UiUpdateCache::default();
    cache.record_rebuild(key, Duration::ZERO, 0, vec![]);
    for changed in [
        UiUpdateKey {
            total_files: 6,
            ..key
        },
        UiUpdateKey {
            recording_mode: RecordingMode::Insert,
            ..key
        },
        UiUpdateKey {
            recording_started: Some(Instant::now()),
            ..key
        },
        UiUpdateKey {
            record_length_ms: 100_001,
            ..key
        },
    ] {
        assert!(cache.needs_rebuild(changed, false));
    }
}

#[test]
fn selection_and_playback_transitions_only_need_lightweight_updates() {
    let key = idle_key();
    let mut cache = UiUpdateCache::default();
    cache.record_rebuild(key, Duration::ZERO, 0, vec![0, 3]);
    for changed in [
        UiUpdateKey {
            current_index: Some(1),
            ..key
        },
        UiUpdateKey {
            current_index: Some(1),
            playing_index: Some(1),
            ..key
        },
        UiUpdateKey {
            current_index: Some(3),
            playing_index: Some(3),
            ..key
        },
        UiUpdateKey {
            current_index: Some(3),
            ..key
        },
    ] {
        assert!(!cache.needs_rebuild(changed, false));
        assert!(cache.needs_navigation_update(changed));
        cache.record_navigation(changed);
        assert!(!cache.needs_navigation_update(changed));
        assert_eq!(cache.section_indices(), Some([0, 3].as_slice()));
    }
}

#[test]
fn recording_restart_rebuilds_but_elapsed_ticks_do_not() {
    let started = Instant::now();
    let key = UiUpdateKey {
        recording_started: Some(started),
        ..idle_key()
    };
    let mut cache = UiUpdateCache::default();
    cache.record_rebuild(key, Duration::ZERO, 0, vec![]);
    for _ in 0..16 {
        assert!(!cache.needs_rebuild(key, false));
    }
    assert!(cache.needs_rebuild(
        UiUpdateKey {
            recording_started: Some(started + Duration::from_secs(1)),
            ..key
        },
        false
    ));
    // Finishing recording removes temporary rows or restores the U row.
    assert!(cache.needs_rebuild(idle_key(), false));
    assert!(cache.needs_rebuild(
        UiUpdateKey {
            current_index: Some(1),
            ..key
        },
        false,
    ));
}

#[test]
fn recording_modes_select_the_expected_reversed_row() {
    assert_eq!(
        file_list_current_index(3, Some(0), RecordingMode::Append, true),
        0
    );
    assert_eq!(
        file_list_current_index(3, Some(0), RecordingMode::Insert, true),
        2
    );
    assert_eq!(
        file_list_current_index(3, Some(1), RecordingMode::Update, true),
        1
    );
    assert_eq!(
        file_list_current_index(3, Some(0), RecordingMode::Insert, false),
        2
    );
    assert_eq!(
        file_list_current_index(0, Some(0), RecordingMode::Insert, true),
        0
    );
    assert_eq!(
        file_list_current_index(0, None, RecordingMode::Append, false),
        -1
    );
}

#[test]
fn recording_duration_tick_changes_only_the_active_row() {
    for index in [0, 2, 4] {
        let mut rows = vec![FileInfo::default(); 5];
        rows[index].is_recording = true;
        rows[index].duration = "00:00".into();
        assert!(update_recording_row_duration(
            &mut rows,
            index as i32,
            Duration::from_secs(2)
        ));
        assert_eq!(rows[index].duration.as_str(), "00:02");
        assert!(!update_recording_row_duration(
            &mut rows,
            index as i32,
            Duration::from_millis(2050)
        ));
        assert!(rows
            .iter()
            .enumerate()
            .all(|(i, row)| i == index || row.duration.is_empty()));
        assert!(!update_recording_row_duration(
            &mut rows,
            -1,
            Duration::ZERO
        ));
        assert!(!update_recording_row_duration(&mut rows, 5, Duration::ZERO));
    }
}

#[test]
fn playback_patches_only_previous_and_new_reversed_rows() {
    let mut rows = vec![FileInfo::default(); 5];
    assert_eq!(
        update_playing_rows(&mut rows, None, Some(4)),
        [None, Some(0)]
    );
    assert!(rows[0].is_playing);
    assert_eq!(
        update_playing_rows(&mut rows, Some(4), Some(1)),
        [Some(0), Some(3)]
    );
    assert!(!rows[0].is_playing);
    assert!(rows[3].is_playing);
    assert!(rows
        .iter()
        .enumerate()
        .all(|(index, row)| index == 3 || !row.is_playing));
    assert_eq!(
        update_playing_rows(&mut rows, Some(1), Some(1)),
        [None, None]
    );
    assert_eq!(
        update_playing_rows(&mut rows, Some(1), None),
        [Some(3), None]
    );
    assert!(rows.iter().all(|row| !row.is_playing));
    assert_eq!(update_playing_rows(&mut rows, None, Some(5)), [None, None]);
    assert_eq!(update_playing_rows(&mut [], Some(0), None), [None, None]);
}

#[test]
fn section_navigation_uses_strict_neighbors_and_refreshes_with_structure() {
    let key = idle_key();
    let mut cache = UiUpdateCache::default();
    assert_eq!(cache.section_indices(), None);
    cache.record_rebuild(key, Duration::ZERO, 0, vec![0, 2, 4]);
    let indices = cache.section_indices().unwrap();
    assert_eq!(adjacent_section_index(indices, 0, false), None);
    assert_eq!(adjacent_section_index(indices, 4, true), None);
    assert_eq!(adjacent_section_index(indices, 2, false), Some(0));
    assert_eq!(adjacent_section_index(indices, 2, true), Some(4));
    assert_eq!(adjacent_section_index(indices, 3, false), Some(2));
    assert_eq!(adjacent_section_index(indices, 3, true), Some(4));
    cache.record_rebuild(key, Duration::ZERO, 0, vec![1]);
    assert_eq!(
        adjacent_section_index(cache.section_indices().unwrap(), 0, true),
        Some(1)
    );
    assert_eq!(adjacent_section_index(&[], 0, true), None);
    assert_eq!(adjacent_section_index(&[], 0, false), None);
}
