use super::{get_cache_path, read_waveform_samples, remove_waveform_cache};
use std::fs;
use std::path::Path;
use std::time::{Duration, UNIX_EPOCH};

fn write_float_wav(path: &Path, channels: u16, samples: &[f32]) {
    let spec = hound::WavSpec {
        channels,
        sample_rate: 8000,
        bits_per_sample: 32,
        sample_format: hound::SampleFormat::Float,
    };
    let mut writer = hound::WavWriter::create(path, spec).unwrap();
    for &sample in samples {
        writer.write_sample(sample).unwrap();
    }
    writer.finalize().unwrap();
}

fn set_mtime(path: &Path, seconds: u64) {
    let times = fs::FileTimes::new().set_modified(UNIX_EPOCH + Duration::from_secs(seconds));
    fs::File::options()
        .write(true)
        .open(path)
        .unwrap()
        .set_times(times)
        .unwrap();
}

fn assert_peaks(actual: &[f32], expected: &[f32]) {
    assert_eq!(actual.len(), expected.len());
    for (&actual, &expected) in actual.iter().zip(expected) {
        assert!(
            (actual - expected).abs() < 1e-6,
            "expected {expected}, got {actual}"
        );
    }
}

#[test]
fn final_peak_is_included_when_frames_do_not_divide_evenly_into_buckets() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("tail.wav");
    write_float_wav(&path, 1, &[0.0, 0.0, 0.0, 0.0, 1.0]);
    assert_peaks(
        &read_waveform_samples(&path, 2, false).unwrap(),
        &[0.0, 1.0],
    );
}

#[test]
fn stereo_waveform_uses_only_the_left_channel() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("stereo.wav");
    write_float_wav(
        &path,
        2,
        &[0.1, 1.0, -0.2, 1.0, 0.3, 1.0, -0.4, 1.0, 0.5, 1.0],
    );
    assert_peaks(
        &read_waveform_samples(&path, 2, false).unwrap(),
        &[0.3, 0.5],
    );
}

#[test]
fn signed_pcm_minimum_is_safe_for_all_supported_bit_depths() {
    let directory = tempfile::tempdir().unwrap();
    for bits in [8, 16, 24, 32] {
        let path = directory.path().join(format!("pcm-{bits}.wav"));
        let spec = hound::WavSpec {
            channels: 1,
            sample_rate: 8000,
            bits_per_sample: bits,
            sample_format: hound::SampleFormat::Int,
        };
        let mut writer = hound::WavWriter::create(&path, spec).unwrap();
        let minimum = (-(1_i64 << (bits - 1))) as i32;
        writer.write_sample(minimum).unwrap();
        writer.write_sample(0_i32).unwrap();
        writer.finalize().unwrap();
        assert_peaks(
            &read_waveform_samples(&path, 2, false).unwrap(),
            &[1.0, 0.0],
        );
    }
}

#[test]
fn zero_limit_empty_audio_and_limits_above_frame_count_are_supported() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("limits.wav");
    write_float_wav(&path, 1, &[-0.25, 0.5]);
    assert!(read_waveform_samples(&path, 0, false).unwrap().is_empty());
    assert_peaks(
        &read_waveform_samples(&path, 100, false).unwrap(),
        &[0.25, 0.5],
    );
    write_float_wav(&path, 2, &[]);
    assert!(read_waveform_samples(&path, 100, false).unwrap().is_empty());
}

#[test]
fn cache_is_rebuilt_for_a_different_requested_resolution() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("resolution.wav");
    write_float_wav(&path, 1, &[0.1, 0.2, 0.3, 0.4]);
    assert_peaks(
        &read_waveform_samples(&path, 2, false).unwrap(),
        &[0.2, 0.4],
    );
    assert_peaks(
        &read_waveform_samples(&path, 4, false).unwrap(),
        &[0.1, 0.2, 0.3, 0.4],
    );
    assert_peaks(&read_waveform_samples(&path, 1, false).unwrap(), &[0.4]);
}

#[test]
fn cache_is_invalidated_when_same_sized_wav_has_a_new_mtime() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("replaced.wav");
    write_float_wav(&path, 1, &[0.1, 0.2]);
    set_mtime(&path, 1_700_000_000);
    let original_size = fs::metadata(&path).unwrap().len();
    assert_peaks(&read_waveform_samples(&path, 1, false).unwrap(), &[0.2]);

    write_float_wav(&path, 1, &[0.8, 0.9]);
    set_mtime(&path, 1_700_000_001);
    assert_eq!(fs::metadata(&path).unwrap().len(), original_size);
    assert_peaks(&read_waveform_samples(&path, 1, false).unwrap(), &[0.9]);
}

#[test]
fn cache_is_invalidated_when_wav_size_changes_even_with_the_same_mtime() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("resized.wav");
    write_float_wav(&path, 1, &[0.1, 0.2]);
    set_mtime(&path, 1_700_000_000);
    assert_peaks(&read_waveform_samples(&path, 1, false).unwrap(), &[0.2]);

    write_float_wav(&path, 1, &[0.1, 0.2, 0.9]);
    set_mtime(&path, 1_700_000_000);
    assert_peaks(&read_waveform_samples(&path, 1, false).unwrap(), &[0.9]);
}

#[test]
fn deleted_source_never_uses_an_existing_cache() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("deleted.wav");
    write_float_wav(&path, 1, &[0.5]);
    read_waveform_samples(&path, 1, false).unwrap();
    assert!(get_cache_path(&path).exists());
    fs::remove_file(&path).unwrap();
    assert!(read_waveform_samples(&path, 1, false).is_err());
}

#[test]
fn legacy_raw_cache_is_ignored_and_replaced() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("legacy.wav");
    write_float_wav(&path, 1, &[0.25]);
    let cache_path = get_cache_path(&path);
    fs::create_dir_all(cache_path.parent().unwrap()).unwrap();
    fs::write(&cache_path, 1.0_f32.to_le_bytes()).unwrap();

    assert_peaks(&read_waveform_samples(&path, 1, false).unwrap(), &[0.25]);
    assert!(fs::metadata(&cache_path).unwrap().len() > 4);
}

#[test]
fn truncated_and_non_finite_cache_payloads_are_rebuilt() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("corrupt.wav");
    write_float_wav(&path, 1, &[0.25, 0.5]);
    read_waveform_samples(&path, 2, false).unwrap();
    let cache_path = get_cache_path(&path);
    let mut bytes = fs::read(&cache_path).unwrap();
    bytes.pop();
    fs::write(&cache_path, bytes).unwrap();
    assert_peaks(
        &read_waveform_samples(&path, 2, false).unwrap(),
        &[0.25, 0.5],
    );

    let mut bytes = fs::read(&cache_path).unwrap();
    let last_sample = bytes.len() - 4;
    bytes[last_sample..].copy_from_slice(&f32::NAN.to_le_bytes());
    fs::write(&cache_path, bytes).unwrap();
    assert_peaks(
        &read_waveform_samples(&path, 2, false).unwrap(),
        &[0.25, 0.5],
    );
}

#[test]
fn valid_cache_is_reused_without_rewriting_and_removal_is_idempotent() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("cached.wav");
    write_float_wav(&path, 1, &[0.25, 0.5]);
    read_waveform_samples(&path, 2, false).unwrap();
    let cache_path = get_cache_path(&path);
    set_mtime(&cache_path, 1_700_000_000);
    let cached_mtime = fs::metadata(&cache_path).unwrap().modified().unwrap();

    assert_peaks(
        &read_waveform_samples(&path, 2, false).unwrap(),
        &[0.25, 0.5],
    );
    assert_eq!(
        fs::metadata(&cache_path).unwrap().modified().unwrap(),
        cached_mtime
    );
    assert_eq!(
        fs::read_dir(cache_path.parent().unwrap()).unwrap().count(),
        1
    );
    remove_waveform_cache(&path).unwrap();
    assert!(!cache_path.exists());
    remove_waveform_cache(&path).unwrap();
}

#[test]
fn cache_limit_is_validated_even_when_output_length_would_be_unchanged() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("large-limits.wav");
    write_float_wav(&path, 1, &[0.25, 0.5]);
    read_waveform_samples(&path, 100, false).unwrap();
    let cache_path = get_cache_path(&path);
    set_mtime(&cache_path, 1_700_000_000);
    let old_mtime = fs::metadata(&cache_path).unwrap().modified().unwrap();

    assert_peaks(
        &read_waveform_samples(&path, 200, false).unwrap(),
        &[0.25, 0.5],
    );
    assert_ne!(
        fs::metadata(&cache_path).unwrap().modified().unwrap(),
        old_mtime
    );
}

#[test]
fn unknown_cache_version_is_ignored() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("unknown-version.wav");
    write_float_wav(&path, 1, &[0.25]);
    read_waveform_samples(&path, 1, false).unwrap();
    let cache_path = get_cache_path(&path);
    let mut bytes = fs::read(&cache_path).unwrap();
    bytes[8..12].copy_from_slice(&u32::MAX.to_le_bytes());
    let last_sample = bytes.len() - 4;
    bytes[last_sample..].copy_from_slice(&0.75_f32.to_le_bytes());
    fs::write(&cache_path, bytes).unwrap();

    assert_peaks(&read_waveform_samples(&path, 1, false).unwrap(), &[0.25]);
}

#[test]
fn inability_to_create_a_cache_does_not_prevent_waveform_reading() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("uncacheable.wav");
    write_float_wav(&path, 1, &[0.25, 0.5]);
    fs::write(directory.path().join("__cache__"), b"not a directory").unwrap();

    assert_peaks(
        &read_waveform_samples(&path, 2, false).unwrap(),
        &[0.25, 0.5],
    );
}
