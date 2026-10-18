use super::RecordingWriter;

fn spec() -> hound::WavSpec {
    hound::WavSpec {
        channels: 1,
        sample_rate: 44100,
        bits_per_sample: 32,
        sample_format: hound::SampleFormat::Float,
    }
}

#[test]
fn creating_a_recording_never_truncates_an_existing_file() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("00001.wav");
    std::fs::write(&path, b"accepted recording").unwrap();
    assert!(RecordingWriter::create_new(&path, spec()).is_err());
    assert_eq!(std::fs::read(&path).unwrap(), b"accepted recording");
}

#[test]
fn finalized_recording_has_a_readable_header_and_samples() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("00001.wav");
    let mut writer = RecordingWriter::create_new(&path, spec()).unwrap();
    writer.write_sample(0.25);
    writer.write_sample(-0.5);
    writer.finalize().unwrap();
    let mut reader = hound::WavReader::open(&path).unwrap();
    assert_eq!(reader.duration(), 2);
    assert_eq!(
        reader
            .samples::<f32>()
            .collect::<Result<Vec<_>, _>>()
            .unwrap(),
        vec![0.25, -0.5]
    );
}

#[test]
fn sample_write_errors_are_reported_instead_of_successful_finalization() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("00001.wav");
    let mut incompatible = spec();
    incompatible.bits_per_sample = 16;
    incompatible.sample_format = hound::SampleFormat::Int;
    let mut writer = RecordingWriter::create_new(&path, incompatible).unwrap();
    writer.write_sample(0.5);
    assert!(writer.finalize().is_err());
}
