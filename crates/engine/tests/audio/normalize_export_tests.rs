use super::normalize_export_in_place;
use crate::audio::export_wav::{inspect, ExportWavWriter};
use crate::audio::processing::apply_normalize_wav_to_wav;
use crate::project::export_workspace::ExportCancelled;
use std::fs;
use std::os::unix::fs::{symlink, MetadataExt};
use std::path::Path;

fn ignore_progress(_: u64, _: u64) -> anyhow::Result<()> {
    Ok(())
}

fn write_rf64(path: &Path, channels: u16, frames: usize) {
    let mut writer = ExportWavWriter::create(path, 48_000, channels).unwrap();
    let values = [-32768, -24576, -16384, -1, 0, 1, 16384, 24576, 32767];
    for index in 0..frames * usize::from(channels) {
        writer.write_sample(values[index % values.len()]).unwrap();
    }
    writer.finalize().unwrap();
}

fn pcm_bytes(path: &Path) -> Vec<u8> {
    let info = inspect(path).unwrap();
    let bytes = fs::read(path).unwrap();
    bytes[info.data_offset as usize..(info.data_offset + info.data_len) as usize].to_vec()
}

#[test]
fn in_place_pcm_exactly_matches_separate_output_normalization() {
    let directory = tempfile::tempdir().unwrap();
    for (channels, gain) in [(1, 0.0), (2, 0.5), (3, 2.5), (3, -0.5), (2, 10.0)] {
        let input = directory.path().join("private.wav");
        let reference = directory.path().join("reference.wav");
        write_rf64(&input, channels, 35_001);
        let before = inspect(&input).unwrap();
        let inode = fs::metadata(&input).unwrap().ino();
        apply_normalize_wav_to_wav(
            &input,
            &reference,
            gain,
            48_000,
            channels,
            &mut ignore_progress,
        )
        .unwrap();
        let mut events = Vec::new();
        normalize_export_in_place(&input, gain, 48_000, channels, &mut |done, total| {
            events.push((done, total));
            Ok(())
        })
        .unwrap();
        assert_eq!(pcm_bytes(&input), pcm_bytes(&reference));
        let after = inspect(&input).unwrap();
        assert!(after.is_rf64);
        assert_eq!(after.file_len, before.file_len);
        assert_eq!(after.frames, before.frames);
        assert_eq!(fs::metadata(&input).unwrap().ino(), inode);
        assert_eq!(fs::read_dir(directory.path()).unwrap().count(), 2);
        let samples = before.data_len / 2;
        assert_eq!(events.first(), Some(&(0, 0)));
        assert_eq!(events.last(), Some(&(samples, samples)));
        let mut previous = 0;
        for &(done, total) in &events[1..] {
            assert_eq!(total, samples);
            assert!(done >= previous && done <= total);
            assert!(done.is_multiple_of(u64::from(channels)));
            previous = done;
        }
    }
}

#[test]
fn normalization_preserves_every_byte_outside_pcm_including_shifted_data_and_tags() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("private-with-tags.wav");
    write_rf64(&path, 3, 12_000);
    let original = fs::read(&path).unwrap();
    // Unknown odd-sized chunks before and after data also exercise RIFF padding.
    let mut tagged = original[..72].to_vec();
    tagged.extend_from_slice(b"JUNK\x03\0\0\0abc\0");
    tagged.extend_from_slice(&original[72..]);
    tagged.extend_from_slice(b"id3 \x05\0\0\0tags!\0");
    let riff_size = tagged.len() as u64 - 8;
    tagged[20..28].copy_from_slice(&riff_size.to_le_bytes());
    fs::write(&path, &tagged).unwrap();
    let before = inspect(&path).unwrap();
    assert_eq!(before.data_offset, 92);

    normalize_export_in_place(&path, 0.5, 48_000, 3, &mut ignore_progress).unwrap();
    let after = inspect(&path).unwrap();
    let normalized = fs::read(&path).unwrap();
    let data_start = before.data_offset as usize;
    let data_end = (before.data_offset + before.data_len) as usize;
    assert_eq!(after.file_len, before.file_len);
    assert_eq!(after.data_offset, before.data_offset);
    assert_eq!(after.data_len, before.data_len);
    assert_eq!(after.chunks, before.chunks);
    assert_eq!(&normalized[..data_start], &tagged[..data_start]);
    assert_eq!(&normalized[data_end..], &tagged[data_end..]);
    assert_ne!(
        &normalized[data_start..data_end],
        &tagged[data_start..data_end]
    );
    assert_eq!(fs::read_dir(directory.path()).unwrap().count(), 1);
}

#[test]
fn invalid_gain_format_and_truncated_input_are_rejected_without_writes() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("private.wav");
    write_rf64(&path, 2, 20);
    let original = fs::read(&path).unwrap();
    for (gain, rate, channels) in [
        (f32::NAN, 48_000, 2),
        (f32::INFINITY, 48_000, 2),
        (f32::NEG_INFINITY, 48_000, 2),
        (0.5, 0, 2),
        (0.5, 44_100, 2),
        (0.5, 48_000, 0),
        (0.5, 48_000, 1),
    ] {
        assert!(
            normalize_export_in_place(&path, gain, rate, channels, &mut ignore_progress).is_err()
        );
        assert_eq!(fs::read(&path).unwrap(), original);
    }
    fs::write(&path, &original[..original.len() - 2]).unwrap();
    let truncated = fs::read(&path).unwrap();
    assert!(normalize_export_in_place(&path, 0.5, 48_000, 2, &mut ignore_progress).is_err());
    assert_eq!(fs::read(&path).unwrap(), truncated);

    let mut non_pcm = original.clone();
    non_pcm[56..58].copy_from_slice(&3_u16.to_le_bytes());
    fs::write(&path, &non_pcm).unwrap();
    assert!(normalize_export_in_place(&path, 0.5, 48_000, 2, &mut ignore_progress).is_err());
    assert_eq!(fs::read(&path).unwrap(), non_pcm);
}

#[test]
fn ordinary_riff_is_not_promoted_or_modified() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("ordinary.wav");
    let mut writer = hound::WavWriter::create(
        &path,
        hound::WavSpec {
            channels: 1,
            sample_rate: 48_000,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        },
    )
    .unwrap();
    writer.write_sample(16384_i16).unwrap();
    writer.finalize().unwrap();
    let original = fs::read(&path).unwrap();
    assert!(normalize_export_in_place(&path, 0.5, 48_000, 1, &mut ignore_progress).is_err());
    assert_eq!(fs::read(&path).unwrap(), original);
}

#[test]
fn symlinks_hardlinks_and_nonregular_files_are_rejected() {
    let directory = tempfile::tempdir().unwrap();
    let original_path = directory.path().join("published.wav");
    let link_path = directory.path().join("private.wav");
    write_rf64(&original_path, 2, 20);
    let original = fs::read(&original_path).unwrap();
    symlink(&original_path, &link_path).unwrap();
    assert!(normalize_export_in_place(&link_path, 0.5, 48_000, 2, &mut ignore_progress).is_err());
    fs::remove_file(&link_path).unwrap();
    fs::hard_link(&original_path, &link_path).unwrap();
    assert!(normalize_export_in_place(&link_path, 0.5, 48_000, 2, &mut ignore_progress).is_err());
    assert!(
        normalize_export_in_place(&original_path, 0.5, 48_000, 2, &mut ignore_progress).is_err()
    );
    assert_eq!(fs::read(&original_path).unwrap(), original);
    assert!(
        normalize_export_in_place(directory.path(), 0.5, 48_000, 2, &mut ignore_progress).is_err()
    );
}

#[test]
fn cancellation_before_io_or_at_known_total_leaves_all_bytes_untouched() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("private.wav");
    write_rf64(&path, 2, 20);
    let original = fs::read(&path).unwrap();
    for cancel_after_preflight in [false, true] {
        let error = normalize_export_in_place(&path, 0.5, 48_000, 2, &mut |_, total| {
            if !cancel_after_preflight || total > 0 {
                return Err(ExportCancelled.into());
            }
            Ok(())
        })
        .unwrap_err();
        assert!(error.is::<ExportCancelled>());
        assert_eq!(fs::read(&path).unwrap(), original);
    }
    let missing = directory.path().join("missing.wav");
    let error = normalize_export_in_place(&missing, 0.5, 48_000, 2, &mut |_, _| {
        Err(ExportCancelled.into())
    })
    .unwrap_err();
    assert!(error.is::<ExportCancelled>());
    assert!(!missing.exists());
}

#[test]
fn mid_operation_cancel_is_typed_and_changes_only_complete_pcm_blocks() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("private.wav");
    write_rf64(&path, 3, 40_000);
    let info = inspect(&path).unwrap();
    let original = fs::read(&path).unwrap();
    let mut completed = 0;
    let error = normalize_export_in_place(&path, 0.5, 48_000, 3, &mut |done, _| {
        if done > 0 {
            completed = done;
            return Err(ExportCancelled.into());
        }
        Ok(())
    })
    .unwrap_err();
    assert!(error.is::<ExportCancelled>());
    assert!(completed > 0 && completed < info.data_len / 2);
    assert!(completed.is_multiple_of(3));
    assert!(completed * 2 <= 64 * 1024);
    let partial = fs::read(&path).unwrap();
    let start = info.data_offset as usize;
    let end = start + completed as usize * 2;
    assert_eq!(partial.len(), original.len());
    assert_eq!(&partial[..start], &original[..start]);
    assert_ne!(&partial[start..end], &original[start..end]);
    assert_eq!(&partial[end..], &original[end..]);
    assert!(inspect(&path).unwrap().is_rf64);
    assert_eq!(fs::read_dir(directory.path()).unwrap().count(), 1);
}

#[test]
fn cancellation_after_read_still_leaves_that_block_untouched() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("private.wav");
    write_rf64(&path, 2, 100);
    let original = fs::read(&path).unwrap();
    let mut known_total_checkpoints = 0;
    let error = normalize_export_in_place(&path, 0.5, 48_000, 2, &mut |done, total| {
        assert_eq!(done, 0);
        if total > 0 {
            known_total_checkpoints += 1;
            // Known total, before block read, after block read.
            if known_total_checkpoints == 3 {
                return Err(ExportCancelled.into());
            }
        }
        Ok(())
    })
    .unwrap_err();
    assert!(error.is::<ExportCancelled>());
    assert_eq!(known_total_checkpoints, 3);
    assert_eq!(fs::read(&path).unwrap(), original);
}

#[test]
fn empty_rf64_remains_byte_identical() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("empty.wav");
    write_rf64(&path, 2, 0);
    let original = fs::read(&path).unwrap();
    normalize_export_in_place(&path, 0.5, 48_000, 2, &mut ignore_progress).unwrap();
    assert_eq!(fs::read(&path).unwrap(), original);
    assert!(inspect(&path).unwrap().is_rf64);
}
