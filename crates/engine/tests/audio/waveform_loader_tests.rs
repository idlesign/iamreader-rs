use super::{WaveformLoader, WaveformSlot};
use std::time::{Duration, Instant};

fn wav(path: &std::path::Path, value: f32) {
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate: 16000,
        bits_per_sample: 32,
        sample_format: hound::SampleFormat::Float,
    };
    let mut writer = hound::WavWriter::create(path, spec).unwrap();
    writer.write_sample(value).unwrap();
    writer.finalize().unwrap();
}

#[test]
fn newest_waveform_request_wins_and_repeated_requests_are_coalesced() {
    let directory = tempfile::tempdir().unwrap();
    let old = directory.path().join("old.wav");
    let new = directory.path().join("new.wav");
    wav(&old, 0.1);
    wav(&new, 0.8);
    let mut loader = WaveformLoader::new(false).unwrap();
    loader.request(WaveformSlot::Current, old);
    loader.request(WaveformSlot::Current, new.clone());
    assert!(!loader.request(WaveformSlot::Current, new));
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if let Some(result) = loader.poll() {
            assert_eq!(result.slot(), WaveformSlot::Current);
            assert_eq!(result.samples.unwrap(), vec![0.8]);
            break;
        }
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(2));
    }
}

#[test]
fn both_waveform_slots_complete_and_a_cleared_slot_is_ignored() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("audio.wav");
    wav(&path, 0.4);
    let mut loader = WaveformLoader::new(false).unwrap();
    loader.request(WaveformSlot::Previous, path.clone());
    loader.request(WaveformSlot::Current, path.clone());
    let mut slots = Vec::new();
    let deadline = Instant::now() + Duration::from_secs(5);
    while slots.len() < 2 {
        if let Some(result) = loader.poll() {
            slots.push(result.slot());
            assert_eq!(result.samples.unwrap(), vec![0.4]);
        }
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(2));
    }
    assert!(slots.contains(&WaveformSlot::Previous) && slots.contains(&WaveformSlot::Current));
    loader.request(WaveformSlot::Current, directory.path().join("missing.wav"));
    loader.clear(WaveformSlot::Current);
    loader.clear(WaveformSlot::Previous);
    // A subsequent valid request also acts as a completion barrier for the old request.
    loader.request(WaveformSlot::Previous, path);
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if let Some(result) = loader.poll() {
            assert_eq!(result.slot(), WaveformSlot::Previous);
            assert!(result.samples.is_ok());
            break;
        }
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(2));
    }
}

#[test]
fn waveform_read_failure_allows_explicit_retry_of_the_same_path() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("missing.wav");
    let mut loader = WaveformLoader::new(false).unwrap();
    assert!(loader.request(WaveformSlot::Current, path.clone()));
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if let Some(result) = loader.poll() {
            assert!(result.samples.unwrap_err().contains("missing.wav"));
            break;
        }
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(2));
    }
    wav(&path, 0.2);
    assert!(loader.request(WaveformSlot::Current, path.clone()));
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if let Some(result) = loader.poll() {
            assert_eq!(result.samples.unwrap(), vec![0.2]);
            break;
        }
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(2));
    }
    // Successful results still coalesce repeated requests instead of rereading the WAV.
    assert!(!loader.request(WaveformSlot::Current, path));
}
