use super::{
    apply_normalize_wav_to_wav, compute_normalize_gain_from_wav, encode_wav_to_mp3,
    process_wav_in_chunks, quantize_pcm16_in_place, read_audio_file_to_samples,
    read_audio_from_bytes, resample, resample_and_convert_channels,
    stream_merge_wav_segments_with_crossfade, write_samples_to_wav, StreamingWavMerge,
};
use crate::project::export_workspace::ExportCancelled;
use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

fn ignore_progress(_: u64, _: u64) -> anyhow::Result<()> {
    Ok(())
}

struct TestDir(PathBuf);

impl TestDir {
    fn new() -> Self {
        static NEXT_ID: AtomicU64 = AtomicU64::new(0);
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "iamreader-processing-{}-{}-{}",
            std::process::id(),
            timestamp,
            NEXT_ID.fetch_add(1, Ordering::Relaxed),
        ));
        std::fs::create_dir(&path).unwrap();
        Self(path)
    }
}

impl Drop for TestDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn decode_wav_from_file_and_bytes(
    samples: &[f32],
    input_rate: u32,
    input_channels: u16,
    output_rate: u32,
    output_channels: u16,
) -> [Vec<f32>; 2] {
    let spec = hound::WavSpec {
        channels: input_channels,
        sample_rate: input_rate,
        bits_per_sample: 32,
        sample_format: hound::SampleFormat::Float,
    };
    let mut cursor = Cursor::new(Vec::new());
    let mut writer = hound::WavWriter::new(&mut cursor, spec).unwrap();
    for &sample in samples {
        writer.write_sample(sample).unwrap();
    }
    writer.finalize().unwrap();
    let bytes = cursor.into_inner();

    let dir = TestDir::new();
    let path = dir.0.join("input.wav");
    std::fs::write(&path, &bytes).unwrap();
    [
        read_audio_file_to_samples(&path, output_rate, output_channels).unwrap(),
        read_audio_from_bytes(&bytes, "input.wav", output_rate, output_channels).unwrap(),
    ]
}

fn assert_samples_close(actual: &[f32], expected: &[f32]) {
    assert_eq!(actual.len(), expected.len());
    for (index, (&actual, &expected)) in actual.iter().zip(expected).enumerate() {
        assert!(
            (actual - expected).abs() < 1e-6,
            "sample {index}: expected {expected}, got {actual}",
        );
    }
}

#[test]
fn wav_loaders_keep_stereo_channels_independent_when_upsampling() {
    for output in decode_wav_from_file_and_bytes(&[1.0, -1.0, 1.0, -1.0], 4, 2, 8, 2) {
        assert_samples_close(&output, &[1.0, -1.0, 1.0, -1.0, 1.0, -1.0, 1.0, -1.0]);
    }
}

#[test]
fn wav_loaders_interpolate_within_each_channel() {
    for output in decode_wav_from_file_and_bytes(&[0.0, 1.0, 1.0, -1.0, 0.0, 1.0], 4, 2, 8, 2) {
        assert_samples_close(
            &output,
            &[0.0, 1.0, 0.5, 0.0, 1.0, -1.0, 0.5, 0.0, 0.0, 1.0, 0.0, 1.0],
        );
    }
}

#[test]
fn wav_loaders_preserve_samples_at_the_same_rate() {
    let input = [0.25, -0.5, 0.75, -0.125];
    for output in decode_wav_from_file_and_bytes(&input, 44100, 2, 44100, 2) {
        assert_eq!(output, input);
    }
}

#[test]
fn wav_loaders_produce_complete_frames_when_downsampling() {
    let input = [0.25, -0.5].repeat(5);
    for output in decode_wav_from_file_and_bytes(&input, 4, 2, 3, 2) {
        // Five input frames * 3/4 => three complete output frames, not seven samples.
        assert_samples_close(&output, &[0.25, -0.5].repeat(3));
    }
}

#[test]
fn wav_loaders_convert_channel_counts_and_frame_counts() {
    let cases = [
        (vec![0.5], vec![0.5, 0.5]),
        (vec![0.5, -0.5], vec![0.0]),
        (vec![0.25, 0.5, 0.75], vec![0.25, 0.5]),
        (vec![0.25, 0.5], vec![0.25, 0.5, 0.5]),
    ];
    for (input_frame, output_frame) in cases {
        for output_rate in [3, 4] {
            let expected_frames = 5 * output_rate as usize / 4;
            let input = input_frame.repeat(5);
            for output in decode_wav_from_file_and_bytes(
                &input,
                4,
                input_frame.len() as u16,
                output_rate,
                output_frame.len() as u16,
            ) {
                assert_samples_close(&output, &output_frame.repeat(expected_frames));
            }
        }
    }
}

#[test]
fn mono_resample_keeps_its_existing_api_and_interpolation() {
    let input = [0.0, 1.0, 0.0];
    let expected = [0.0, 0.5, 1.0, 0.5, 0.0, 0.0];
    assert_samples_close(&resample(&input, 4, 8), &expected);
    assert_samples_close(
        &resample_and_convert_channels(&input, 4, 1, 8, 1).unwrap(),
        &expected,
    );
    assert_eq!(resample(&input, 4, 4), input);
}

#[test]
fn resampling_empty_audio_returns_no_frames() {
    for output_rate in [4, 8] {
        assert!(resample_and_convert_channels(&[], 4, 2, output_rate, 1)
            .unwrap()
            .is_empty());
        assert!(resample(&[], 4, output_rate).is_empty());
        for output in decode_wav_from_file_and_bytes(&[], 4, 2, output_rate, 2) {
            assert!(output.is_empty());
        }
    }
}

#[test]
fn resampling_rejects_invalid_formats_and_incomplete_frames() {
    assert!(resample_and_convert_channels(&[0.0, 0.0], 0, 2, 4, 2).is_err());
    assert!(resample_and_convert_channels(&[0.0, 0.0], 4, 2, 0, 2).is_err());
    assert!(resample_and_convert_channels(&[0.0, 0.0], 4, 0, 4, 2).is_err());
    assert!(resample_and_convert_channels(&[0.0, 0.0], 4, 2, 4, 0).is_err());
    assert!(resample_and_convert_channels(&[0.0], 4, 2, 4, 2).is_err());
}

#[test]
fn normalize_gain_amplifies_quiet_audio_towards_target_rms() {
    let dir = TestDir::new();
    let path = dir.0.join("quiet.wav");
    write_samples_to_wav(&[0.02; 128], &path, 16000, 2).unwrap();
    let decoded = read_audio_file_to_samples(&path, 16000, 2).unwrap();
    let gain = compute_normalize_gain_from_wav(&path, 2, &mut ignore_progress)
        .unwrap()
        .unwrap();
    let target_rms = 10.0_f32.powf(-20.5 / 20.0);
    assert!(gain > 1.0);
    assert!((decoded[0] * gain - target_rms).abs() < 1e-6);
}

#[test]
fn normalize_gain_preserves_the_maximum_gain_limit() {
    let dir = TestDir::new();
    let path = dir.0.join("very_quiet.wav");
    write_samples_to_wav(&[0.001; 128], &path, 16000, 1).unwrap();
    assert_eq!(
        compute_normalize_gain_from_wav(&path, 1, &mut ignore_progress).unwrap(),
        Some(10.0)
    );
}

#[test]
fn normalize_gain_respects_peak_headroom_when_amplifying() {
    let dir = TestDir::new();
    let input_path = dir.0.join("peaked.wav");
    let output_path = dir.0.join("normalized.wav");
    let mut input = vec![0.01; 4096];
    input[0] = -0.5;
    write_samples_to_wav(&input, &input_path, 16000, 2).unwrap();
    let decoded = read_audio_file_to_samples(&input_path, 16000, 2).unwrap();
    let peak = decoded
        .iter()
        .map(|sample| sample.abs())
        .fold(0.0_f32, f32::max);
    let peak_limit = 10.0_f32.powf(-3.0 / 20.0);
    let gain = compute_normalize_gain_from_wav(&input_path, 2, &mut ignore_progress)
        .unwrap()
        .unwrap();
    assert!(gain > 1.0);
    assert!((peak * gain - peak_limit).abs() < 1e-6);

    apply_normalize_wav_to_wav(
        &input_path,
        &output_path,
        gain,
        16000,
        2,
        &mut ignore_progress,
    )
    .unwrap();
    let output = read_export_samples(&output_path);
    assert_eq!(output.len(), input.len());
    assert!(output.iter().all(|sample| sample.abs() <= peak_limit));
    // Retain the existing tanh policy and allow for the final PCM16 quantization.
    assert!((output[0] - (decoded[0] * gain).tanh()).abs() < 2.0 / 32768.0);
}

#[test]
fn normalize_gain_still_attenuates_loud_audio() {
    let dir = TestDir::new();
    let path = dir.0.join("loud.wav");
    write_samples_to_wav(&[0.8; 128], &path, 16000, 1).unwrap();
    let gain = compute_normalize_gain_from_wav(&path, 1, &mut ignore_progress)
        .unwrap()
        .unwrap();
    assert!(gain > 0.0 && gain < 1.0);
}

#[test]
fn normalize_gain_returns_none_for_silence_and_empty_audio() {
    let dir = TestDir::new();
    let path = dir.0.join("silent.wav");
    for samples in [&[][..], &[0.0; 128][..]] {
        write_samples_to_wav(samples, &path, 16000, 2).unwrap();
        assert_eq!(
            compute_normalize_gain_from_wav(&path, 2, &mut ignore_progress).unwrap(),
            None
        );
    }
}

fn read_export_samples(path: &Path) -> Vec<f32> {
    let mut reader = crate::audio::export_wav::ExportWavReader::open(path).unwrap();
    let mut pcm = [0i16; 8192];
    let mut result = Vec::new();
    loop {
        let count = reader.read_samples(&mut pcm).unwrap();
        if count == 0 {
            break;
        }
        result.extend(pcm[..count].iter().map(|s| f32::from(*s) / 32768.0));
    }
    result
}

fn assert_merged_wav(path: &Path, sample_rate: u32, channels: u16, expected: &[f32]) {
    let info = crate::audio::export_wav::inspect(path).unwrap();
    assert!(info.is_rf64);
    assert_eq!(info.sample_rate, sample_rate);
    assert_eq!(info.channels, channels);
    let samples = read_export_samples(path);
    assert_eq!(samples.len(), expected.len());
    assert_eq!(samples.len() % channels as usize, 0);
    for (index, (&sample, &expected)) in samples.iter().zip(expected).enumerate() {
        // Account for PCM16 quantization of both the fixture and merged output.
        assert!(
            (sample - expected).abs() <= 2.0 / 32768.0,
            "sample {index}: expected {expected}, got {sample}",
        );
    }
}

#[test]
fn merge_empty_segments_returns_empty_without_creating_output() {
    let dir = TestDir::new();
    let output = dir.0.join("merged.wav");
    let segments: [PathBuf; 0] = [];
    assert!(stream_merge_wav_segments_with_crossfade(
        &segments,
        2,
        &output,
        8000,
        1,
        &mut ignore_progress
    )
    .unwrap()
    .is_empty());
    assert!(!output.exists());
}

#[test]
fn merge_single_segment_preserves_mono_and_stereo_samples() {
    let dir = TestDir::new();
    let input = dir.0.join("input.wav");
    let output = dir.0.join("merged.wav");
    let samples = [0.125, -0.25, 0.5, -0.75];
    for channels in [1, 2] {
        write_samples_to_wav(&samples, &input, 8000, channels).unwrap();
        let lengths = stream_merge_wav_segments_with_crossfade(
            &[&input],
            16,
            &output,
            8000,
            channels,
            &mut ignore_progress,
        )
        .unwrap();
        assert_eq!(lengths, vec![samples.len()]);
        assert_merged_wav(&output, 8000, channels, &samples);
    }
}

#[test]
fn merge_multiple_segments_crossfades_stereo_reproducibly() {
    let dir = TestDir::new();
    let segments = [
        dir.0.join("first.wav"),
        dir.0.join("second.wav"),
        dir.0.join("third.wav"),
    ];
    let frames = [[-0.5, 0.5], [0.25, -0.25], [0.5, -0.5]];
    for (path, frame) in segments.iter().zip(frames) {
        write_samples_to_wav(&frame.repeat(4), path, 8000, 2).unwrap();
    }
    let output = dir.0.join("merged.wav");
    let repeated_output = dir.0.join("repeated.wav");
    for path in [&output, &repeated_output] {
        let lengths = stream_merge_wav_segments_with_crossfade(
            &segments,
            4,
            path,
            8000,
            2,
            &mut ignore_progress,
        )
        .unwrap();
        assert_eq!(lengths, vec![8, 8, 8]);
        assert_merged_wav(
            path,
            8000,
            2,
            &[
                -0.5, 0.5, -0.5, 0.5, -0.5, 0.5, -0.125, 0.125, 0.25, -0.25, 0.375, -0.375, 0.5,
                -0.5, 0.5, -0.5,
            ],
        );
    }
    assert_eq!(
        std::fs::read(&output).unwrap(),
        std::fs::read(&repeated_output).unwrap()
    );
}

#[test]
fn merge_without_crossfade_concatenates_segments() {
    let dir = TestDir::new();
    let segments = [dir.0.join("first.wav"), dir.0.join("second.wav")];
    write_samples_to_wav(&[0.25, -0.25], &segments[0], 8000, 1).unwrap();
    write_samples_to_wav(&[0.5, -0.5, 0.0], &segments[1], 8000, 1).unwrap();
    let output = dir.0.join("merged.wav");
    let lengths = stream_merge_wav_segments_with_crossfade(
        &segments,
        0,
        &output,
        8000,
        1,
        &mut ignore_progress,
    )
    .unwrap();
    assert_eq!(lengths, vec![2, 3]);
    assert_merged_wav(&output, 8000, 1, &[0.25, -0.25, 0.5, -0.5, 0.0]);
}

#[test]
fn merge_short_middle_segment_does_not_drop_final_tail() {
    let dir = TestDir::new();
    let segments = [
        dir.0.join("first.wav"),
        dir.0.join("short.wav"),
        dir.0.join("last.wav"),
    ];
    write_samples_to_wav(&[0.25; 4], &segments[0], 8000, 1).unwrap();
    write_samples_to_wav(&[0.5; 2], &segments[1], 8000, 1).unwrap();
    write_samples_to_wav(&[0.75; 4], &segments[2], 8000, 1).unwrap();
    let output = dir.0.join("merged.wav");
    let lengths = stream_merge_wav_segments_with_crossfade(
        &segments,
        2,
        &output,
        8000,
        1,
        &mut ignore_progress,
    )
    .unwrap();
    assert_eq!(lengths, vec![4, 2, 4]);
    assert_merged_wav(
        &output,
        8000,
        1,
        &[0.25, 0.25, 0.25, 0.375, 0.75, 0.75, 0.75, 0.75],
    );
}

#[test]
fn merge_rejects_invalid_output_format_without_touching_output() {
    let dir = TestDir::new();
    let input = dir.0.join("input.wav");
    write_samples_to_wav(&[0.25; 4], &input, 8000, 1).unwrap();
    let output = dir.0.join("merged.wav");
    let sentinel = b"previous output";
    std::fs::write(&output, sentinel).unwrap();
    for (sample_rate, channels) in [(0, 1), (8000, 0), (0, 0)] {
        for segments in [&[][..], &[&input][..]] {
            assert!(stream_merge_wav_segments_with_crossfade(
                segments,
                2,
                &output,
                sample_rate,
                channels,
                &mut ignore_progress
            )
            .is_err());
            assert_eq!(std::fs::read(&output).unwrap(), sentinel);
        }
    }
}

#[test]
fn merge_reports_missing_and_corrupt_segments() {
    let dir = TestDir::new();
    let input = dir.0.join("input.wav");
    write_samples_to_wav(&[0.25; 4], &input, 8000, 1).unwrap();
    let corrupt = dir.0.join("corrupt.wav");
    std::fs::write(&corrupt, b"not a WAV file").unwrap();
    let missing = dir.0.join("missing.wav");
    let output = dir.0.join("merged.wav");
    for invalid in [&missing, &corrupt] {
        let error = stream_merge_wav_segments_with_crossfade(
            &[&input, invalid],
            2,
            &output,
            8000,
            1,
            &mut ignore_progress,
        )
        .unwrap_err();
        let message = format!("{error:#}");
        assert!(message.contains("Failed to read segment"), "{message}");
        assert!(message.contains(invalid.to_str().unwrap()), "{message}");
    }
}

#[test]
fn merge_reports_output_creation_failure() {
    let dir = TestDir::new();
    let input = dir.0.join("input.wav");
    write_samples_to_wav(&[0.25; 4], &input, 8000, 1).unwrap();
    let error = stream_merge_wav_segments_with_crossfade(
        &[&input],
        2,
        &dir.0,
        8000,
        1,
        &mut ignore_progress,
    )
    .unwrap_err();
    let message = format!("{error:#}");
    assert!(message.contains("Failed to create WAV"), "{message}");
    assert!(message.contains(dir.0.to_str().unwrap()), "{message}");
}

#[test]
fn merge_matches_crossfade_across_multiple_read_blocks_and_channel_boundaries() {
    let dir = TestDir::new();
    let segments = [dir.0.join("first.wav"), dir.0.join("second.wav")];
    let output = dir.0.join("merged.wav");
    let reference = dir.0.join("reference.wav");
    for (channels, crossfade_samples) in [(2, 10002), (3, 25)] {
        let first = (0..10003 * channels as usize)
            .map(|index| ((index * 37 % 1800) as f32 - 900.0) / 1000.0)
            .collect::<Vec<_>>();
        let second = (0..11005 * channels as usize)
            .map(|index| ((index * 71 % 1800) as f32 - 900.0) / 1000.0)
            .collect::<Vec<_>>();
        write_samples_to_wav(&first, &segments[0], 8000, channels).unwrap();
        write_samples_to_wav(&second, &segments[1], 8000, channels).unwrap();
        let first = read_audio_file_to_samples(&segments[0], 8000, channels).unwrap();
        let second = read_audio_file_to_samples(&segments[1], 8000, channels).unwrap();
        let (mut expected, remainder) =
            reference_crossfade(&first, &second, crossfade_samples, channels);
        expected.extend_from_slice(&remainder);
        write_samples_to_wav(&expected, &reference, 8000, channels).unwrap();

        let lengths = stream_merge_wav_segments_with_crossfade(
            &segments,
            crossfade_samples,
            &output,
            8000,
            channels,
            &mut ignore_progress,
        )
        .unwrap();
        assert_eq!(lengths, vec![first.len(), second.len()]);
        assert_eq!(
            expected.len(),
            first.len() + second.len() - crossfade_samples / channels as usize * channels as usize
        );
        assert_eq!(
            read_export_samples(&output),
            read_export_samples(&reference),
            "channels={channels}, crossfade_samples={crossfade_samples}",
        );
    }
}

#[test]
fn merge_empty_segments_preserve_the_existing_crossfade_barrier_semantics() {
    let dir = TestDir::new();
    let first = vec![0.25; 4];
    let second = vec![0.5; 4];
    let crossfaded = vec![0.25, 0.25, 0.25, 0.375, 0.5, 0.5];
    let cases = [
        (vec![vec![]], vec![]),
        (vec![vec![], vec![]], vec![]),
        (
            vec![first.clone(), vec![], second.clone()],
            vec![0.25, 0.25, 0.25, 0.25, 0.5, 0.5, 0.5, 0.5],
        ),
        (
            vec![vec![], first.clone(), second.clone()],
            crossfaded.clone(),
        ),
        (vec![first, second, vec![]], crossfaded),
    ];
    for (case_index, (inputs, expected)) in cases.into_iter().enumerate() {
        let segments = inputs
            .iter()
            .enumerate()
            .map(|(index, samples)| {
                let path = dir.0.join(format!("{case_index}-{index}.wav"));
                write_samples_to_wav(samples, &path, 8000, 1).unwrap();
                path
            })
            .collect::<Vec<_>>();
        let output = dir.0.join("merged.wav");
        let lengths = stream_merge_wav_segments_with_crossfade(
            &segments,
            2,
            &output,
            8000,
            1,
            &mut ignore_progress,
        )
        .unwrap();
        assert_eq!(lengths, inputs.iter().map(Vec::len).collect::<Vec<_>>());
        assert_merged_wav(&output, 8000, 1, &expected);
    }
}

#[test]
fn merge_short_head_keeps_the_unmixed_prefix_of_the_previous_tail() {
    let dir = TestDir::new();
    let segments = [dir.0.join("first.wav"), dir.0.join("short.wav")];
    write_samples_to_wav(&[0.125, 0.25, 0.375, 0.5], &segments[0], 8000, 1).unwrap();
    write_samples_to_wav(&[0.75], &segments[1], 8000, 1).unwrap();
    let output = dir.0.join("merged.wav");
    let lengths = stream_merge_wav_segments_with_crossfade(
        &segments,
        3,
        &output,
        8000,
        1,
        &mut ignore_progress,
    )
    .unwrap();
    assert_eq!(lengths, vec![4, 1]);
    // The one-frame overlap uses fade_in=0, retaining the original fade convention.
    assert_merged_wav(&output, 8000, 1, &[0.125, 0.25, 0.375, 0.5]);
}

#[test]
fn merge_rejects_segments_that_are_not_in_the_prepared_format() {
    let dir = TestDir::new();
    let input = dir.0.join("input.wav");
    let output = dir.0.join("merged.wav");
    for (sample_rate, channels, bits_per_sample, sample_format) in [
        (16000, 1, 16, hound::SampleFormat::Int),
        (8000, 2, 16, hound::SampleFormat::Int),
        (8000, 1, 24, hound::SampleFormat::Int),
        (8000, 1, 32, hound::SampleFormat::Float),
    ] {
        let mut writer = hound::WavWriter::create(
            &input,
            hound::WavSpec {
                sample_rate,
                channels,
                bits_per_sample,
                sample_format,
            },
        )
        .unwrap();
        for _ in 0..channels {
            match sample_format {
                hound::SampleFormat::Int => writer.write_sample(100_i32).unwrap(),
                hound::SampleFormat::Float => writer.write_sample(0.25_f32).unwrap(),
            }
        }
        writer.finalize().unwrap();
        let error = stream_merge_wav_segments_with_crossfade(
            &[&input],
            2,
            &output,
            8000,
            1,
            &mut ignore_progress,
        )
        .unwrap_err();
        let message = format!("{error:#}");
        assert!(
            message.contains("Prepared segment must be PCM16 WAV"),
            "{message}"
        );
        assert!(message.contains(input.to_str().unwrap()), "{message}");
    }
}

#[test]
fn merge_reports_truncated_samples_after_a_complete_read_block() {
    let dir = TestDir::new();
    let input = dir.0.join("truncated.wav");
    write_samples_to_wav(&vec![0.25; 20000], &input, 8000, 2).unwrap();
    let length = std::fs::metadata(&input).unwrap().len();
    std::fs::OpenOptions::new()
        .write(true)
        .open(&input)
        .unwrap()
        .set_len(length - 2)
        .unwrap();
    let output = dir.0.join("merged.wav");
    let error = stream_merge_wav_segments_with_crossfade(
        &[&input],
        4,
        &output,
        8000,
        2,
        &mut ignore_progress,
    )
    .unwrap_err();
    let message = format!("{error:#}");
    assert!(
        message.contains("Failed to read segment sample"),
        "{message}"
    );
    assert!(message.contains(input.to_str().unwrap()), "{message}");
}

#[test]
fn streaming_normalization_keeps_channels_across_block_boundaries() {
    let dir = TestDir::new();
    let input = dir.0.join("three-channels.wav");
    let output = dir.0.join("normalized.wav");
    let samples = [0.1, 0.2, 0.3].repeat(90001);
    write_samples_to_wav(&samples, &input, 8000, 3).unwrap();
    let source = read_audio_file_to_samples(&input, 8000, 3).unwrap();
    let expected_rms = source[..3].iter().sum::<f32>() / 3.0;
    let expected_gain = 10.0_f32.powf(-20.5 / 20.0) / expected_rms;
    let gain = compute_normalize_gain_from_wav(&input, 3, &mut ignore_progress)
        .unwrap()
        .unwrap();
    assert!(
        (gain - expected_gain).abs() < 1e-5,
        "{gain} vs {expected_gain}"
    );
    apply_normalize_wav_to_wav(&input, &output, gain, 8000, 3, &mut ignore_progress).unwrap();
    let normalized = read_export_samples(&output);
    assert_eq!(source.len(), normalized.len());
    for (actual, original) in normalized.iter().zip(source.iter()) {
        let expected = ((original * gain).tanh() * 32767.0).round() / 32768.0;
        assert!((actual - expected).abs() < 1e-6);
    }
}

#[test]
fn streaming_rejects_invalid_block_and_normalization_format_before_output_changes() {
    let dir = TestDir::new();
    let input = dir.0.join("input.wav");
    let output = dir.0.join("output.wav");
    write_samples_to_wav(&[0.25; 8], &input, 8000, 2).unwrap();
    assert!(super::process_wav_in_chunks(&input, 0, |_| Ok(()), &mut ignore_progress).is_err());
    assert!(compute_normalize_gain_from_wav(&input, 0, &mut ignore_progress).is_err());
    assert!(compute_normalize_gain_from_wav(&input, 1, &mut ignore_progress).is_err());
    for (gain, rate, channels) in [(1.0, 8000, 0), (1.0, 16000, 2), (f32::NAN, 8000, 2)] {
        std::fs::write(&output, b"previous").unwrap();
        assert!(apply_normalize_wav_to_wav(
            &input,
            &output,
            gain,
            rate,
            channels,
            &mut ignore_progress
        )
        .is_err());
        assert_eq!(std::fs::read(&output).unwrap(), b"previous");
    }
}

#[test]
fn prepared_wav_invalid_format_does_not_touch_existing_output() {
    let dir = TestDir::new();
    let path = dir.0.join("prepared.wav");
    for (rate, channels) in [(0, 1), (8000, 0), (8000, u16::MAX), (u32::MAX, 2)] {
        std::fs::write(&path, b"unchanged").unwrap();
        assert!(write_samples_to_wav(&[], &path, rate, channels).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), b"unchanged");
    }
}

fn assert_progress_counts(updates: &[(u64, u64)], expected_total: u64) {
    assert_eq!(updates.first(), Some(&(0, 0)));
    assert_eq!(updates.last(), Some(&(expected_total, expected_total)));
    let mut known_total = false;
    let mut previous_done = 0;
    for &(done, total) in updates {
        if total == 0 {
            assert!(!known_total, "Total reverted to unknown: {updates:?}");
            assert_eq!(done, 0);
        } else {
            known_total = true;
            assert_eq!(total, expected_total);
        }
        assert!(done >= previous_done && done <= total, "{updates:?}");
        previous_done = done;
    }
}

#[test]
fn streaming_pre_cancel_does_not_read_headers_or_touch_existing_output() {
    let dir = TestDir::new();
    let missing = dir.0.join("missing.wav");
    let output = dir.0.join("previous-output");
    std::fs::write(&output, b"accepted output").unwrap();
    for operation in 0..5 {
        let mut callbacks = 0;
        let mut cancel = |done, total| {
            callbacks += 1;
            assert_eq!((done, total), (0, 0));
            Err(ExportCancelled.into())
        };
        let error = match operation {
            0 => process_wav_in_chunks(&missing, 16, |_| panic!("consumer ran"), &mut cancel),
            1 => compute_normalize_gain_from_wav(&missing, 2, &mut cancel).map(|_| ()),
            2 => apply_normalize_wav_to_wav(&missing, &output, 1.0, 44100, 2, &mut cancel),
            3 => stream_merge_wav_segments_with_crossfade(
                &[&missing],
                4,
                &output,
                44100,
                2,
                &mut cancel,
            )
            .map(|_| ()),
            _ => encode_wav_to_mp3(&missing, &output, 44100, 2, &mut cancel),
        }
        .unwrap_err();
        assert!(error.is::<ExportCancelled>(), "{error:#}");
        assert_eq!(callbacks, 1);
        assert_eq!(std::fs::read(&output).unwrap(), b"accepted output");
    }
}

#[test]
fn streaming_cancel_after_preflight_does_not_create_output() {
    let dir = TestDir::new();
    let input = dir.0.join("input.wav");
    let output = dir.0.join("previous-output");
    write_samples_to_wav(&[0.25; 20], &input, 44100, 2).unwrap();
    std::fs::write(&output, b"accepted output").unwrap();
    for operation in 0..3 {
        let mut known_total_callbacks = 0;
        let mut cancel = |done, total| {
            assert_eq!(done, 0);
            if total > 0 {
                assert_eq!(total, 20);
                known_total_callbacks += 1;
                return Err(ExportCancelled.into());
            }
            Ok(())
        };
        let error = match operation {
            0 => apply_normalize_wav_to_wav(&input, &output, 1.0, 44100, 2, &mut cancel),
            1 => stream_merge_wav_segments_with_crossfade(
                &[&input],
                4,
                &output,
                44100,
                2,
                &mut cancel,
            )
            .map(|_| ()),
            _ => encode_wav_to_mp3(&input, &output, 44100, 2, &mut cancel),
        }
        .unwrap_err();
        assert!(error.is::<ExportCancelled>(), "{error:#}");
        assert_eq!(known_total_callbacks, 1);
        assert_eq!(std::fs::read(&output).unwrap(), b"accepted output");
    }
}

#[test]
fn chunk_progress_cancellation_stops_after_exactly_three_complete_frames_blocks() {
    let dir = TestDir::new();
    let input = dir.0.join("three-channels.wav");
    write_samples_to_wav(&[0.1, 0.2, 0.3].repeat(20), &input, 44100, 3).unwrap();
    let mut blocks = Vec::new();
    let mut updates = Vec::new();
    let error = process_wav_in_chunks(
        &input,
        8,
        |chunk| {
            blocks.push(chunk.len());
            Ok(())
        },
        &mut |done, total| {
            updates.push((done, total));
            if done == 18 {
                return Err(ExportCancelled.into());
            }
            Ok(())
        },
    )
    .unwrap_err();
    assert!(error.is::<ExportCancelled>());
    assert_eq!(blocks, vec![6, 6, 6]);
    assert_eq!(updates.last(), Some(&(18, 60)));
}

#[test]
fn chunk_cancellation_after_read_does_not_run_the_consumer() {
    let dir = TestDir::new();
    let input = dir.0.join("input.wav");
    write_samples_to_wav(&[0.25; 20], &input, 44100, 2).unwrap();
    let mut checkpoints_with_known_total = 0;
    let error = process_wav_in_chunks(
        &input,
        8,
        |_| panic!("consumer ran after cancellation"),
        &mut |done, total| {
            assert_eq!(done, 0);
            if total > 0 {
                checkpoints_with_known_total += 1;
                // Header, before reading the first block, after reading that block.
                if checkpoints_with_known_total == 3 {
                    return Err(ExportCancelled.into());
                }
            }
            Ok(())
        },
    )
    .unwrap_err();
    assert!(error.is::<ExportCancelled>());
    assert_eq!(checkpoints_with_known_total, 3);
}

#[test]
fn merge_cancellation_after_two_blocks_does_not_finalize_the_partial_output() {
    let dir = TestDir::new();
    let input = dir.0.join("input.wav");
    let output = dir.0.join("partial.wav");
    write_samples_to_wav(&vec![0.25; 60000], &input, 44100, 2).unwrap();
    let mut updates = Vec::new();
    let error = stream_merge_wav_segments_with_crossfade(
        &[&input],
        4,
        &output,
        44100,
        2,
        &mut |done, total| {
            updates.push((done, total));
            if done >= 2 * 8192 {
                return Err(ExportCancelled.into());
            }
            Ok(())
        },
    )
    .unwrap_err();
    assert!(error.is::<ExportCancelled>());
    assert_eq!(updates.last(), Some(&(2 * 8192, 60000)));
    assert!(crate::audio::export_wav::inspect(&output).is_err());
    let bytes = std::fs::metadata(&output).unwrap().len();
    assert!(bytes > 80 && bytes < 80 + 60000 * 2);
}

#[test]
fn merge_retained_tail_writes_have_bounded_cancellation_checkpoints() {
    let dir = TestDir::new();
    let segments = [dir.0.join("input.wav"), dir.0.join("empty.wav")];
    let output = dir.0.join("partial.wav");
    write_samples_to_wav(&vec![0.25; 60000], &segments[0], 44100, 1).unwrap();
    write_samples_to_wav(&[], &segments[1], 44100, 1).unwrap();
    let mut checkpoints_after_all_input = 0;
    let error = stream_merge_wav_segments_with_crossfade(
        &segments,
        60000,
        &output,
        44100,
        1,
        &mut |done, total| {
            if total > 0 && done == total {
                checkpoints_after_all_input += 1;
                if checkpoints_after_all_input == 10 {
                    return Err(ExportCancelled.into());
                }
            }
            Ok(())
        },
    )
    .unwrap_err();
    assert!(error.is::<ExportCancelled>());
    assert_eq!(checkpoints_after_all_input, 10);
    assert!(crate::audio::export_wav::inspect(&output).is_err());
    let bytes = std::fs::metadata(&output).unwrap().len();
    assert!(bytes > 80 && bytes < 80 + 60000 * 2);
}

#[test]
fn normalization_passes_cancel_after_two_frame_aligned_blocks() {
    let dir = TestDir::new();
    let input = dir.0.join("three-channels.wav");
    let output = dir.0.join("partial.wav");
    let total = 600000;
    let block = 256 * 1024 / 3 * 3;
    write_samples_to_wav(&[0.1, 0.2, 0.3].repeat(total / 3), &input, 44100, 3).unwrap();
    for render in [false, true] {
        let mut updates = Vec::new();
        let mut cancel = |done, total| {
            updates.push((done, total));
            if done >= 2 * block as u64 {
                return Err(ExportCancelled.into());
            }
            Ok(())
        };
        let result = if render {
            apply_normalize_wav_to_wav(&input, &output, 0.5, 44100, 3, &mut cancel)
        } else {
            compute_normalize_gain_from_wav(&input, 3, &mut cancel).map(|_| ())
        };
        assert!(result.unwrap_err().is::<ExportCancelled>());
        assert_eq!(updates.last(), Some(&(2 * block as u64, total as u64)));
        if render {
            assert!(crate::audio::export_wav::inspect(&output).is_err());
            assert!(std::fs::metadata(&output).unwrap().len() < 80 + total as u64 * 2);
        }
    }
}

#[test]
fn mp3_cancellation_stops_after_two_blocks_or_before_encoding_the_final_remainder() {
    let dir = TestDir::new();
    let input = dir.0.join("input.wav");
    let output = dir.0.join("partial.mp3");
    for (total, cancel_at) in [(10000, 2 * 1152 * 2), (200, 200)] {
        write_samples_to_wav(&vec![0.25; total], &input, 44100, 2).unwrap();
        let mut updates = Vec::new();
        let error = encode_wav_to_mp3(&input, &output, 44100, 2, &mut |done, total| {
            updates.push((done, total));
            if done >= cancel_at {
                return Err(ExportCancelled.into());
            }
            Ok(())
        })
        .unwrap_err();
        assert!(error.is::<ExportCancelled>());
        assert_eq!(updates.last(), Some(&(cancel_at, total as u64)));
        if total == 200 {
            // Input did not fill a LAME frame: cancellation must precede remainder/flush.
            assert_eq!(std::fs::metadata(&output).unwrap().len(), 0);
        }
    }
}

#[test]
fn successful_streaming_progress_is_monotonic_and_preserves_pcm_and_mp3_audio() {
    let dir = TestDir::new();
    let segments = [dir.0.join("first.wav"), dir.0.join("second.wav")];
    let merged = dir.0.join("merged.wav");
    let normalized = dir.0.join("normalized.wav");
    let reference = dir.0.join("reference.wav");
    let mp3 = dir.0.join("book.mp3");
    write_samples_to_wav(&[0.1, -0.05].repeat(15001), &segments[0], 44100, 2).unwrap();
    write_samples_to_wav(&[0.2, -0.1].repeat(14003), &segments[1], 44100, 2).unwrap();
    let mut updates = Vec::new();
    let lengths = stream_merge_wav_segments_with_crossfade(
        &segments,
        4,
        &merged,
        44100,
        2,
        &mut |done, total| {
            updates.push((done, total));
            Ok(())
        },
    )
    .unwrap();
    let input_total = lengths.iter().sum::<usize>() as u64;
    assert_progress_counts(&updates, input_total);
    let merged_total = input_total - 4;
    assert_eq!(read_export_samples(&merged).len() as u64, merged_total);

    updates.clear();
    let gain = compute_normalize_gain_from_wav(&merged, 2, &mut |done, total| {
        updates.push((done, total));
        Ok(())
    })
    .unwrap()
    .unwrap();
    assert_progress_counts(&updates, merged_total);
    assert_eq!(
        Some(gain),
        compute_normalize_gain_from_wav(&merged, 2, &mut ignore_progress).unwrap()
    );

    updates.clear();
    apply_normalize_wav_to_wav(&merged, &normalized, gain, 44100, 2, &mut |done, total| {
        updates.push((done, total));
        Ok(())
    })
    .unwrap();
    assert_progress_counts(&updates, merged_total);
    apply_normalize_wav_to_wav(&merged, &reference, gain, 44100, 2, &mut ignore_progress).unwrap();
    assert_eq!(
        read_export_samples(&normalized),
        read_export_samples(&reference)
    );

    updates.clear();
    encode_wav_to_mp3(&normalized, &mp3, 44100, 2, &mut |done, total| {
        updates.push((done, total));
        Ok(())
    })
    .unwrap();
    assert_progress_counts(&updates, merged_total);
    let decoded = read_audio_file_to_samples(&mp3, 44100, 2).unwrap();
    assert!(!decoded.is_empty() && decoded.len().is_multiple_of(2));
    assert!(decoded.iter().all(|sample| sample.is_finite()));
    assert!(decoded.iter().any(|sample| sample.abs() > 0.01));
}

#[test]
fn in_place_quantization_exactly_matches_pcm16_wav_roundtrip_at_boundaries() {
    let dir = TestDir::new();
    let path = dir.0.join("roundtrip.wav");
    let boundary = [
        f32::NAN,
        f32::INFINITY,
        f32::NEG_INFINITY,
        f32::MAX,
        f32::MIN,
        -2.0,
        -32768.0 / 32767.0,
        -1.0,
        -0.5,
        -1.5 / 32767.0,
        -0.5 / 32767.0,
        -f32::MIN_POSITIVE,
        -0.0,
        0.0,
        f32::MIN_POSITIVE,
        0.5 / 32767.0,
        1.5 / 32767.0,
        0.5,
        1.0,
        2.0,
    ];
    for channels in [1, 2, 3] {
        let mut samples = boundary.repeat(usize::from(channels));
        // Also cover every PCM16 value with fractional-LSB offsets on both sides.
        for value in i16::MIN..=i16::MAX {
            for offset in [-0.51, -0.5, -0.49, 0.0, 0.49, 0.5, 0.51] {
                samples.extend(std::iter::repeat_n(
                    (f32::from(value) + offset) / 32767.0,
                    usize::from(channels),
                ));
            }
        }
        write_samples_to_wav(&samples, &path, 44100, channels).unwrap();
        let expected = read_audio_file_to_samples(&path, 44100, channels).unwrap();
        let allocation = samples.as_ptr();
        quantize_pcm16_in_place(&mut samples);
        assert_eq!(samples.as_ptr(), allocation);
        assert_eq!(samples, expected);
        assert_eq!(&samples[..3], &[-1.0, 32767.0 / 32768.0, -1.0]);
        assert!(samples.iter().all(|sample| sample.is_finite()));
    }
    quantize_pcm16_in_place(&mut []);
}

#[test]
fn memory_merge_exactly_matches_prepared_wav_batch_for_channels_and_segment_edges() {
    // Frame lengths include empty barriers, fully overlapped short segments,
    // unaligned crossfade requests and heads/bodies/tails spanning 8192-sample blocks.
    let cases: &[(usize, &[usize])] = &[
        (16, &[1]),
        (2, &[4, 2, 4]),
        (2, &[4, 0, 4]),
        (2, &[0, 4, 4, 0]),
        (99, &[1, 1, 1]),
        (0, &[4, 5, 3]),
        (4, &[0, 0]),
        (8193, &[8191, 8192, 8193]),
        (10_002, &[10_003, 11_005]),
    ];
    let boundaries = [
        f32::NAN,
        f32::INFINITY,
        f32::NEG_INFINITY,
        -2.0,
        -1.0,
        -0.5 / 32767.0,
        -0.0,
        0.0,
        0.5 / 32767.0,
        1.0,
        2.0,
    ];
    for channels in [1, 2, 3] {
        for &(crossfade, frame_counts) in cases {
            let dir = TestDir::new();
            let inputs: Vec<Vec<f32>> = frame_counts
                .iter()
                .enumerate()
                .map(|(segment, &frames)| {
                    (0..frames * usize::from(channels))
                        .map(|index| {
                            if index % 13 == 0 {
                                boundaries[(index / 13 + segment) % boundaries.len()]
                            } else {
                                ((index * 37 + segment * 73) % 2001) as f32 / 1000.0 - 1.0
                            }
                        })
                        .collect()
                })
                .collect();
            let original_bits: Vec<Vec<u32>> = inputs
                .iter()
                .map(|samples| samples.iter().map(|sample| sample.to_bits()).collect())
                .collect();
            let segments: Vec<_> = inputs
                .iter()
                .enumerate()
                .map(|(index, samples)| {
                    let path = dir.0.join(format!("input-{index}.wav"));
                    write_samples_to_wav(samples, &path, 44100, channels).unwrap();
                    path
                })
                .collect();
            let batch = dir.0.join("batch.wav");
            let lengths = stream_merge_wav_segments_with_crossfade(
                &segments,
                crossfade,
                &batch,
                44100,
                channels,
                &mut ignore_progress,
            )
            .unwrap();
            let output = dir.0.join("memory.wav");
            let mut merge = StreamingWavMerge::create(&output, crossfade, 44100, channels).unwrap();
            for (input, &expected_len) in inputs.iter().zip(&lengths) {
                let mut updates = Vec::new();
                assert_eq!(
                    merge
                        .append_samples(input, &mut |done, total| {
                            updates.push((done, total));
                            Ok(())
                        })
                        .unwrap(),
                    expected_len
                );
                assert_progress_counts(&updates, expected_len as u64);
                for adjacent in updates.windows(2) {
                    assert!(adjacent[1].0 - adjacent[0].0 <= 8192);
                }
            }
            merge.finish(&mut ignore_progress).unwrap();
            assert_eq!(
                std::fs::read(&output).unwrap(),
                std::fs::read(&batch).unwrap(),
                "channels={channels}, crossfade={crossfade}, frames={frame_counts:?}"
            );
            for (samples, expected_bits) in inputs.iter().zip(original_bits) {
                assert_eq!(
                    samples
                        .iter()
                        .map(|sample| sample.to_bits())
                        .collect::<Vec<_>>(),
                    expected_bits
                );
            }
        }
    }
}

#[test]
fn memory_merge_cancellation_at_block_checkpoints_poisons_the_section() {
    for cancel_after in [0, 8192, 16384] {
        let dir = TestDir::new();
        let output = dir.0.join("partial.wav");
        let samples = vec![0.25; 60_000];
        let mut merge = StreamingWavMerge::create(&output, 6, 44100, 3).unwrap();
        let mut updates = Vec::new();
        let error = merge
            .append_samples(&samples, &mut |done, total| {
                updates.push((done, total));
                if done >= cancel_after {
                    return Err(ExportCancelled.into());
                }
                Ok(())
            })
            .unwrap_err();
        assert!(error.is::<ExportCancelled>());
        assert_eq!(updates.last().unwrap().0, cancel_after);
        assert!(merge
            .append_samples(&samples, &mut |_, _| panic!(
                "poisoned merge called progress"
            ))
            .is_err());
        assert!(merge
            .finish(&mut |_, _| panic!("poisoned merge attempted to finish"))
            .is_err());
        if cancel_after == 0 {
            // An untouched RF64 header is a valid empty WAV; the poisoned API,
            // not header parsing, prevents publication as a successful section.
            assert_eq!(
                crate::audio::export_wav::inspect(&output).unwrap().frames,
                0
            );
        } else {
            assert!(crate::audio::export_wav::inspect(&output).is_err());
        }
        assert_eq!(
            std::fs::metadata(&output).unwrap().len(),
            80 + cancel_after * 2
        );
    }
}

#[test]
fn memory_merge_rejects_incomplete_frames_and_cannot_finalize_previous_tail() {
    for channels in [2, 3] {
        let dir = TestDir::new();
        let output = dir.0.join("partial.wav");
        let mut merge = StreamingWavMerge::create(&output, 12, 44100, channels).unwrap();
        merge
            .append_samples(&[0.25; 6], &mut ignore_progress)
            .unwrap();
        let mut updates = Vec::new();
        let error = merge
            .append_samples(&[0.5], &mut |done, total| {
                updates.push((done, total));
                Ok(())
            })
            .unwrap_err();
        assert!(format!("{error:#}").contains("complete audio frames"));
        assert_eq!(updates, [(0, 0)]);
        assert!(merge.append_samples(&[], &mut ignore_progress).is_err());
        assert!(merge.finish(&mut ignore_progress).is_err());
        assert_eq!(std::fs::metadata(&output).unwrap().len(), 80);
        assert_eq!(
            crate::audio::export_wav::inspect(&output).unwrap().frames,
            0
        );
    }
}

#[test]
fn incremental_merge_preserves_pcm_when_each_input_is_removed_after_append() {
    let cases = [
        (
            2,
            16,
            vec![vec![0.125, -0.25, 0.5, -0.75]],
            vec![0.125, -0.25, 0.5, -0.75],
        ),
        (
            1,
            2,
            vec![vec![0.25; 4], vec![0.5; 2], vec![0.75; 4]],
            vec![0.25, 0.25, 0.25, 0.375, 0.75, 0.75, 0.75, 0.75],
        ),
        (
            1,
            2,
            vec![vec![0.25; 4], vec![], vec![0.5; 4]],
            vec![0.25, 0.25, 0.25, 0.25, 0.5, 0.5, 0.5, 0.5],
        ),
        (
            1,
            2,
            vec![vec![], vec![0.25; 4], vec![0.5; 4], vec![]],
            vec![0.25, 0.25, 0.25, 0.375, 0.5, 0.5],
        ),
        (
            1,
            99,
            vec![vec![0.25], vec![0.5], vec![0.75]],
            vec![0.25, 0.75],
        ),
        (
            2,
            0,
            vec![vec![0.125, -0.25], vec![0.5, -0.75]],
            vec![0.125, -0.25, 0.5, -0.75],
        ),
        (2, 4, vec![vec![], vec![]], vec![]),
    ];
    for (channels, crossfade, inputs, expected) in cases {
        let dir = TestDir::new();
        let segments = inputs
            .iter()
            .enumerate()
            .map(|(index, samples)| {
                let path = dir.0.join(format!("input-{index}.wav"));
                write_samples_to_wav(samples, &path, 44100, channels).unwrap();
                path
            })
            .collect::<Vec<_>>();
        let batch = dir.0.join("batch.wav");
        let lengths = stream_merge_wav_segments_with_crossfade(
            &segments,
            crossfade,
            &batch,
            44100,
            channels,
            &mut ignore_progress,
        )
        .unwrap();
        let output = dir.0.join("incremental.wav");
        let mut merge = StreamingWavMerge::create(&output, crossfade, 44100, channels).unwrap();
        #[cfg(unix)]
        let output_inode = {
            use std::os::unix::fs::MetadataExt;
            std::fs::metadata(&output).unwrap().ino()
        };
        for (path, &expected_len) in segments.iter().zip(&lengths) {
            let mut updates = Vec::new();
            let len = merge
                .append(path, &mut |done, total| {
                    updates.push((done, total));
                    Ok(())
                })
                .unwrap();
            assert_eq!(len, expected_len);
            assert_progress_counts(&updates, expected_len as u64);
            std::fs::remove_file(path).unwrap();
            assert!(!path.exists());
            #[cfg(unix)]
            {
                use std::os::unix::fs::MetadataExt;
                assert_eq!(std::fs::metadata(&output).unwrap().ino(), output_inode);
            }
        }
        merge
            .finish(&mut |done, total| {
                assert_eq!((done, total), (0, 0));
                Ok(())
            })
            .unwrap();
        assert_merged_wav(&output, 44100, channels, &expected);
        assert_eq!(
            std::fs::read(&output).unwrap(),
            std::fs::read(&batch).unwrap()
        );
    }
}

#[test]
fn incremental_merge_matches_the_reference_across_blocks_after_input_deletion() {
    let dir = TestDir::new();
    let segments = [dir.0.join("first.wav"), dir.0.join("second.wav")];
    let output = dir.0.join("merged.wav");
    let reference = dir.0.join("reference.wav");
    let channels = 3;
    let crossfade = 10002;
    let first = (0..30009)
        .map(|index| ((index * 37 % 1800) as f32 - 900.0) / 1000.0)
        .collect::<Vec<_>>();
    let second = (0..33015)
        .map(|index| ((index * 71 % 1800) as f32 - 900.0) / 1000.0)
        .collect::<Vec<_>>();
    write_samples_to_wav(&first, &segments[0], 44100, channels).unwrap();
    write_samples_to_wav(&second, &segments[1], 44100, channels).unwrap();
    let first = read_audio_file_to_samples(&segments[0], 44100, channels).unwrap();
    let second = read_audio_file_to_samples(&segments[1], 44100, channels).unwrap();
    let (mut expected, remainder) = reference_crossfade(&first, &second, crossfade, channels);
    expected.extend_from_slice(&remainder);
    write_samples_to_wav(&expected, &reference, 44100, channels).unwrap();

    let mut merge = StreamingWavMerge::create(&output, crossfade, 44100, channels).unwrap();
    for (path, expected_len) in segments.iter().zip([first.len(), second.len()]) {
        assert_eq!(
            merge.append(path, &mut ignore_progress).unwrap(),
            expected_len
        );
        std::fs::remove_file(path).unwrap();
    }
    merge.finish(&mut ignore_progress).unwrap();
    assert_eq!(
        read_export_samples(&output),
        read_audio_file_to_samples(&reference, 44100, channels).unwrap()
    );
}

#[test]
fn incremental_append_cancellation_prevents_retry_and_successful_finish() {
    let dir = TestDir::new();
    let input = dir.0.join("input.wav");
    let output = dir.0.join("partial.wav");
    write_samples_to_wav(&vec![0.25; 60000], &input, 44100, 2).unwrap();
    let mut merge = StreamingWavMerge::create(&output, 4, 44100, 2).unwrap();
    let error = merge
        .append(&input, &mut |done, _| {
            if done >= 8192 {
                return Err(ExportCancelled.into());
            }
            Ok(())
        })
        .unwrap_err();
    assert!(error.is::<ExportCancelled>());
    assert!(merge.append(&input, &mut ignore_progress).is_err());
    assert!(merge.finish(&mut ignore_progress).is_err());
    assert!(crate::audio::export_wav::inspect(&output).is_err());
}

#[test]
fn incremental_append_input_error_cannot_finalize_an_incomplete_section() {
    let dir = TestDir::new();
    let input = dir.0.join("input.wav");
    let corrupt = dir.0.join("corrupt.wav");
    let missing = dir.0.join("missing.wav");
    let output = dir.0.join("partial.wav");
    write_samples_to_wav(&[0.25; 16], &input, 44100, 2).unwrap();
    std::fs::write(&corrupt, b"not a WAV").unwrap();
    for invalid in [&corrupt, &missing] {
        let mut merge = StreamingWavMerge::create(&output, 4, 44100, 2).unwrap();
        merge.append(&input, &mut ignore_progress).unwrap();
        assert!(merge.append(invalid, &mut ignore_progress).is_err());
        assert!(merge.finish(&mut ignore_progress).is_err());
        assert!(crate::audio::export_wav::inspect(&output).is_err());
    }
}

#[test]
fn incremental_finish_cancellation_stops_at_a_bounded_tail_write() {
    let dir = TestDir::new();
    let input = dir.0.join("input.wav");
    let output = dir.0.join("partial.wav");
    write_samples_to_wav(&vec![0.25; 60000], &input, 44100, 1).unwrap();
    let mut merge = StreamingWavMerge::create(&output, 60000, 44100, 1).unwrap();
    merge.append(&input, &mut ignore_progress).unwrap();
    std::fs::remove_file(&input).unwrap();
    let mut checkpoints = 0;
    let error = merge
        .finish(&mut |done, total| {
            assert_eq!((done, total), (0, 0));
            checkpoints += 1;
            if checkpoints == 6 {
                return Err(ExportCancelled.into());
            }
            Ok(())
        })
        .unwrap_err();
    assert!(error.is::<ExportCancelled>());
    assert_eq!(checkpoints, 6);
    let bytes = std::fs::metadata(&output).unwrap().len();
    assert!(bytes > 80 && bytes < 80 + 60000 * 2);
    assert!(crate::audio::export_wav::inspect(&output).is_err());
}

// In-memory reference implementation used only to verify the streaming merger.
/// Применяет кроссфейд между концом предыдущего и началом следующего аудио сегмента
/// crossfade_samples - количество сэмплов для кроссфейда (обычно 10-50 мс)
/// Возвращает модифицированные предыдущий и следующий сегменты
fn reference_crossfade(
    prev_samples: &[f32],
    next_samples: &[f32],
    crossfade_samples: usize,
    channels: u16,
) -> (Vec<f32>, Vec<f32>) {
    let channels_usize = channels as usize;

    // Если один из массивов пуст, возвращаем как есть
    if prev_samples.is_empty() || next_samples.is_empty() {
        return (prev_samples.to_vec(), next_samples.to_vec());
    }

    let crossfade_frames = crossfade_samples / channels_usize;

    // Определяем реальную длину кроссфейда
    let prev_available = prev_samples.len() / channels_usize;
    let next_available = next_samples.len() / channels_usize;
    let actual_crossfade = crossfade_frames.min(prev_available).min(next_available);

    if actual_crossfade == 0 {
        // Нет данных для кроссфейда, возвращаем как есть
        return (prev_samples.to_vec(), next_samples.to_vec());
    }

    let crossfade_samples_actual = actual_crossfade * channels_usize;

    // Берем последние сэмплы предыдущего файла
    let prev_tail = &prev_samples[prev_samples.len() - crossfade_samples_actual..];
    // Берем первые сэмплы следующего файла
    let next_head = &next_samples[..crossfade_samples_actual];

    // Применяем кроссфейд: fade-out для предыдущего, fade-in для следующего
    let mut crossfaded = Vec::with_capacity(crossfade_samples_actual);

    for i in 0..actual_crossfade {
        // Линейный переход от 1.0 до 0.0 для предыдущего
        // и от 0.0 до 1.0 для следующего
        let fade_out = 1.0 - (i as f32 / actual_crossfade as f32);
        let fade_in = i as f32 / actual_crossfade as f32;

        for ch in 0..channels_usize {
            let prev_idx = i * channels_usize + ch;
            let next_idx = i * channels_usize + ch;
            // Индексы гарантированно в пределах, так как actual_crossfade вычислен с учетом размеров
            let mixed = prev_tail[prev_idx] * fade_out + next_head[next_idx] * fade_in;
            crossfaded.push(mixed);
        }
    }

    // Создаем модифицированные сегменты
    let mut prev_modified = prev_samples[..prev_samples.len() - crossfade_samples_actual].to_vec();
    prev_modified.extend_from_slice(&crossfaded);

    let next_modified = if next_samples.len() > crossfade_samples_actual {
        next_samples[crossfade_samples_actual..].to_vec()
    } else {
        Vec::new()
    };

    (prev_modified, next_modified)
}
