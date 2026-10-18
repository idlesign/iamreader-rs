use super::{process_file_for_compilation, process_samples_for_compilation};
use crate::audio::export_wav::{inspect, ExportWavReader};
use crate::audio::processing::{quantize_pcm16_in_place, write_samples_to_wav, StreamingWavMerge};
use crate::project::project::{MarkerAsset, MarkerAssets, MarkerSettings, ProjectFile};
use std::collections::HashMap;
use std::fs;
use std::path::Path;

const RATE: u32 = 8_000;

fn ignore_progress(_: u64, _: u64) -> anyhow::Result<()> {
    Ok(())
}

fn project_file(markers: Vec<String>) -> ProjectFile {
    ProjectFile {
        path: "must-not-read-original-recording.wav".into(),
        title: "Fixture".into(),
        author: String::new(),
        year: String::new(),
        hint: String::new(),
        markers,
        size: 0,
        duration_ms: 0,
    }
}

fn asset(name: &str, kind: &str, repeat: Option<i32>, reduction: u8) -> MarkerAsset {
    MarkerAsset {
        audio: name.into(),
        kind: kind.into(),
        reduction: Some(reduction),
        repeat,
    }
}

fn marker(begin: MarkerAsset, end: MarkerAsset) -> MarkerSettings {
    MarkerSettings {
        assets: MarkerAssets { begin, end },
        ..MarkerSettings::default()
    }
}

fn signal(frames: usize, channels: u16, shift: usize) -> Vec<f32> {
    let values = [-1.4, -1.0, -0.83, -0.00002, 0.0, 0.00002, 0.31, 0.97, 1.4];
    (0..frames * usize::from(channels))
        .map(|index| values[(index + shift) % values.len()])
        .collect()
}

fn read_pcm(path: &Path) -> Vec<i16> {
    let mut reader = ExportWavReader::open(path).unwrap();
    let mut samples = Vec::new();
    let mut block = [0; 127];
    loop {
        let read = reader.read_samples(&mut block).unwrap();
        if read == 0 {
            return samples;
        }
        samples.extend_from_slice(&block[..read]);
    }
}

// Reference preserves both historical PCM16 boundaries using real small WAVs.
// The direct path may create only its final RF64, never a prepared/mixed segment.
fn assert_disk_and_samples_match(
    channels: u16,
    settings: Vec<MarkerSettings>,
    crossfade_frames: usize,
) {
    let directory = tempfile::tempdir().unwrap();
    let assets_dir = directory.path().join("assets");
    let disk_dir = directory.path().join("disk-reference");
    let direct_dir = directory.path().join("samples-output");
    for path in [&assets_dir, &disk_dir, &direct_dir] {
        fs::create_dir(path).unwrap();
    }
    let begin_asset = assets_dir.join("begin.wav");
    let end_asset = assets_dir.join("end.wav");
    write_samples_to_wav(&signal(3, channels, 1), &begin_asset, RATE, channels).unwrap();
    write_samples_to_wav(&signal(5, channels, 4), &end_asset, RATE, channels).unwrap();
    let original_begin = fs::read(&begin_asset).unwrap();
    let original_end = fs::read(&end_asset).unwrap();
    let names: Vec<_> = (0..settings.len())
        .map(|index| format!("marker-{index}"))
        .collect();
    let markers: HashMap<_, _> = names.iter().cloned().zip(settings).collect();
    let file = project_file(names);
    let disk_output = disk_dir.join("book.wav");
    let direct_output = direct_dir.join("book.wav");
    let crossfade = crossfade_frames * usize::from(channels);
    let mut disk_merge =
        StreamingWavMerge::create(&disk_output, crossfade, RATE, channels).unwrap();
    let mut direct_merge =
        StreamingWavMerge::create(&direct_output, crossfade, RATE, channels).unwrap();
    let mut expected_samples = 0;
    let mut previous_tail = 0;
    // A one-frame input is shorter than the fade/underlay; an empty middle input
    // exercises the old tail flush/break behavior when no add marker supplies audio.
    for (index, frames) in [17, 1, 0, 3, 35].into_iter().enumerate() {
        let mut raw = signal(frames, channels, index);
        let prepared = disk_dir.join(format!("prepared-{index}.wav"));
        let mixed_path = disk_dir.join(format!("mixed-{index}.wav"));
        write_samples_to_wav(&raw, &prepared, RATE, channels).unwrap();
        let prepared_bytes = fs::read(&prepared).unwrap();
        let disk_mixed =
            process_file_for_compilation(&file, &markers, &assets_dir, &prepared, RATE, channels)
                .unwrap();
        write_samples_to_wav(&disk_mixed, &mixed_path, RATE, channels).unwrap();
        let disk_length = disk_merge
            .append(&mixed_path, &mut ignore_progress)
            .unwrap();

        quantize_pcm16_in_place(&mut raw);
        let direct_mixed =
            process_samples_for_compilation(&file, &markers, &assets_dir, raw, RATE, channels)
                .unwrap();
        assert_eq!(
            direct_mixed, disk_mixed,
            "Marker result differs at input {index}"
        );
        let direct_length = direct_merge
            .append_samples(&direct_mixed, &mut ignore_progress)
            .unwrap();
        assert_eq!(direct_length, disk_length);
        assert_eq!(direct_length, direct_mixed.len());

        let overlap = previous_tail.min(crossfade).min(disk_length);
        expected_samples += disk_length - overlap;
        previous_tail = crossfade.min(disk_length - overlap);
        assert_eq!(fs::read(&prepared).unwrap(), prepared_bytes);
    }
    disk_merge.finish(&mut ignore_progress).unwrap();
    direct_merge.finish(&mut ignore_progress).unwrap();
    let direct_info = inspect(&direct_output).unwrap();
    let disk_info = inspect(&disk_output).unwrap();
    assert!(direct_info.is_rf64 && disk_info.is_rf64);
    assert_eq!(direct_info.frames, disk_info.frames);
    assert_eq!(
        direct_info.frames,
        (expected_samples / usize::from(channels)) as u64
    );
    assert_eq!(read_pcm(&direct_output), read_pcm(&disk_output));
    assert_eq!(
        fs::read(&direct_output).unwrap(),
        fs::read(&disk_output).unwrap()
    );
    assert_eq!(fs::read_dir(&direct_dir).unwrap().count(), 1);
    assert_eq!(fs::read(&begin_asset).unwrap(), original_begin);
    assert_eq!(fs::read(&end_asset).unwrap(), original_end);
}

#[test]
fn direct_pcm_without_markers_matches_empty_short_and_multichannel_disk_segments() {
    for channels in [1, 2, 3] {
        for crossfade_frames in [0, 2, 64] {
            assert_disk_and_samples_match(channels, vec![], crossfade_frames);
        }
    }
}

#[test]
fn direct_pcm_begin_and_end_add_repeats_match_disk_boundaries() {
    for channels in [1, 2, 3] {
        for (begin_repeat, end_repeat) in [(Some(2), Some(3)), (Some(-1), Some(0)), (None, None)] {
            assert_disk_and_samples_match(
                channels,
                vec![marker(
                    asset("begin.wav", "add", begin_repeat, 0),
                    asset("end.wav", "add", end_repeat, 0),
                )],
                2,
            );
        }
    }
}

#[test]
fn direct_pcm_fixed_and_looping_underlays_match_disk_boundaries() {
    for channels in [1, 2, 3] {
        for repeat in [Some(2), Some(-1), None] {
            assert_disk_and_samples_match(
                channels,
                vec![marker(
                    asset("begin.wav", "underlay", repeat, 37),
                    asset("end.wav", "underlay", repeat, 64),
                )],
                2,
            );
        }
    }
}

#[test]
fn direct_pcm_multiple_add_and_underlay_markers_preserve_order_and_roundtrips() {
    assert_disk_and_samples_match(
        2,
        vec![
            marker(
                asset("begin.wav", "add", Some(2), 0),
                asset("end.wav", "underlay", Some(-1), 41),
            ),
            marker(
                asset("begin.wav", "underlay", Some(2), 73),
                asset("end.wav", "add", Some(3), 0),
            ),
        ],
        2,
    );
}

#[test]
fn marker_dsp_preserves_add_order_fixed_underlay_positions_and_looping_behavior() {
    let directory = tempfile::tempdir().unwrap();
    write_samples_to_wav(&[0.25; 4], &directory.path().join("begin.wav"), RATE, 2).unwrap();
    write_samples_to_wav(&[-0.5; 4], &directory.path().join("end.wav"), RATE, 2).unwrap();
    let file = project_file(vec!["marker".into()]);
    for (kind, begin_repeat, end_repeat, expected) in [
        (
            "add",
            2,
            3,
            [vec![0.25; 8], vec![0.0; 12], vec![-0.5; 12]].concat(),
        ),
        (
            "underlay",
            2,
            1,
            [vec![0.5; 4], vec![0.0; 4], vec![-0.5; 4]].concat(),
        ),
        ("underlay", -1, -1, vec![-0.25; 12]),
    ] {
        let markers = HashMap::from([(
            "marker".into(),
            marker(
                asset("begin.wav", kind, Some(begin_repeat), 0),
                asset("end.wav", kind, Some(end_repeat), 0),
            ),
        )]);
        let actual = process_samples_for_compilation(
            &file,
            &markers,
            directory.path(),
            vec![0.0; 12],
            RATE,
            2,
        )
        .unwrap();
        assert_eq!(actual, expected, "{kind}, {begin_repeat}, {end_repeat}");
    }
}

#[test]
fn markerless_processing_returns_the_owned_vec_allocation_including_empty_input() {
    let directory = tempfile::tempdir().unwrap();
    let file = project_file(vec![]);
    for channels in [1, 2, 3] {
        for frames in [0, 5] {
            let mut input = Vec::with_capacity(128);
            input.extend(signal(frames, channels, 1));
            quantize_pcm16_in_place(&mut input);
            let pointer = input.as_ptr();
            let capacity = input.capacity();
            let expected = input.clone();
            let output = process_samples_for_compilation(
                &file,
                &HashMap::new(),
                directory.path(),
                input,
                RATE,
                channels,
            )
            .unwrap();
            assert_eq!(output.as_ptr(), pointer);
            assert_eq!(output.capacity(), capacity);
            assert_eq!(output, expected);
        }
    }
    assert_eq!(fs::read_dir(directory.path()).unwrap().count(), 0);
}

#[test]
fn direct_marker_processing_rejects_invalid_rate_channels_and_partial_frames() {
    let directory = tempfile::tempdir().unwrap();
    let file = project_file(vec![]);
    for (rate, channels, samples) in [
        (0, 2, vec![0.1, 0.2]),
        (RATE, 0, vec![]),
        (RATE, 0, vec![0.1]),
        (RATE, 2, vec![0.1]),
        (RATE, 3, vec![0.1, 0.2, 0.3, 0.4]),
    ] {
        assert!(process_samples_for_compilation(
            &file,
            &HashMap::new(),
            directory.path(),
            samples,
            rate,
            channels,
        )
        .is_err());
    }
    assert_eq!(fs::read_dir(directory.path()).unwrap().count(), 0);
}
