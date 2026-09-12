use super::{inspect, update_sizes, ExportWavReader, ExportWavWriter, WavInfo};
use std::fs::{self, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::Path;

fn empty_wav(path: &Path, channels: u16) {
    ExportWavWriter::create(path, 48_000, channels)
        .unwrap()
        .finalize()
        .unwrap();
}

// Exercise public header finalization with a sparse PCM extent, without writing gigabytes.
fn sparse_wav(path: &Path, data_len: u64, channels: u16) -> WavInfo {
    empty_wav(path, channels);
    let mut info = inspect(path).unwrap();
    info.data_len = data_len;
    info.frames = data_len / (u64::from(channels) * 2);
    let len = info.data_offset + data_len;
    let mut file = OpenOptions::new()
        .read(true)
        .write(true)
        .open(path)
        .unwrap();
    file.set_len(len).unwrap();
    update_sizes(&mut file, &info, len).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        assert!(
            file.metadata().unwrap().blocks() * 512 < 64 * 1024,
            "test requires sparse-file support"
        );
    }
    inspect(path).unwrap()
}

#[test]
fn pcm16_writer_reader_round_trip_and_frame_seek() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("stereo.wav");
    let samples = [i16::MIN, i16::MAX, -1, 0, 42, -42];
    let mut writer = ExportWavWriter::create(&path, 44_100, 2).unwrap();
    for sample in samples {
        writer.write_sample(sample).unwrap();
    }
    writer.finalize().unwrap();
    let info = inspect(&path).unwrap();
    assert_eq!(
        (info.sample_rate, info.channels, info.frames, info.data_len),
        (44_100, 2, 3, 12)
    );
    assert_eq!(info.data_offset, 80);
    assert!(info.is_rf64);
    assert_eq!(info.chunks[0].id, *b"ds64");
    let mut reader = ExportWavReader::open(&path).unwrap();
    let mut block = [0; 4];
    assert_eq!(reader.read_samples(&mut block).unwrap(), 4);
    assert_eq!(block, samples[..4]);
    assert_eq!(reader.read_samples(&mut block).unwrap(), 2);
    assert_eq!(block[..2], samples[4..]);
    assert_eq!(reader.read_samples(&mut block).unwrap(), 0);
    reader.seek_frame(1).unwrap();
    assert_eq!(reader.read_samples(&mut block).unwrap(), 4);
    assert_eq!(block, samples[2..]);
    reader.seek_frame(3).unwrap();
    assert_eq!(reader.read_samples(&mut block).unwrap(), 0);
    assert!(reader.seek_frame(4).is_err());
}

#[test]
fn hound_pcm16_is_readable_but_cannot_promote_to_rf64() {
    let directory = tempfile::tempdir().unwrap();
    for channels in [1, 2, 3] {
        let path = directory.path().join(format!("hound-{channels}.wav"));
        let spec = hound::WavSpec {
            channels,
            sample_rate: 48_000,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let mut writer = hound::WavWriter::create(&path, spec).unwrap();
        for _ in 0..channels {
            writer.write_sample(42_i16).unwrap();
        }
        writer.finalize().unwrap();
        let info = inspect(&path).unwrap();
        assert_eq!(info.frames, 1);
        assert_eq!(info.channels, channels);
        let original = fs::read(&path).unwrap();
        assert!(info.validate_file_len(u64::from(u32::MAX) + 9).is_err());
        let mut file = OpenOptions::new()
            .read(true)
            .write(true)
            .open(&path)
            .unwrap();
        assert!(update_sizes(&mut file, &info, u64::from(u32::MAX) + 9).is_err());
        assert_eq!(fs::read(&path).unwrap(), original);
    }
}

#[test]
fn sparse_rf64_lengths_cross_the_riff_limit_without_pcm_allocation() {
    let directory = tempfile::tempdir().unwrap();
    // RIFF size is file_len - 8, not data_len or file_len itself.
    let largest_small_data = (u64::from(u32::MAX) - 72) & !1;
    for (index, data_len) in [
        largest_small_data,
        largest_small_data + 2,
        u64::from(u32::MAX) + 1,
    ]
    .into_iter()
    .enumerate()
    {
        let path = directory.path().join(format!("boundary-{index}.wav"));
        let info = sparse_wav(&path, data_len, 1);
        assert_eq!(info.data_len, data_len);
        assert_eq!(info.frames, data_len / 2);
        assert!(info.is_rf64);
        let mut header = [0; 80];
        fs::File::open(&path)
            .unwrap()
            .read_exact(&mut header)
            .unwrap();
        if info.is_rf64 {
            assert_eq!(&header[..4], b"RF64");
            assert_eq!(&header[12..16], b"ds64");
            assert_eq!(
                u32::from_le_bytes(header[4..8].try_into().unwrap()),
                u32::MAX
            );
            assert_eq!(
                u32::from_le_bytes(header[76..80].try_into().unwrap()),
                u32::MAX
            );
            assert_eq!(
                u64::from_le_bytes(header[20..28].try_into().unwrap()),
                info.file_len - 8
            );
            assert_eq!(
                u64::from_le_bytes(header[28..36].try_into().unwrap()),
                data_len
            );
            assert_eq!(
                u64::from_le_bytes(header[36..44].try_into().unwrap()),
                info.frames
            );
        }
    }
}

#[test]
fn sparse_frame_seek_exceeds_u32_without_wrapping() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("long.wav");
    let frames = u64::from(u32::MAX) + 2;
    let info = sparse_wav(&path, frames * 4, 2);
    let mut file = OpenOptions::new().write(true).open(&path).unwrap();
    file.seek(SeekFrom::Start(info.data_offset + (frames - 1) * 4))
        .unwrap();
    file.write_all(&[123, 0, 255, 255]).unwrap();
    let mut reader = ExportWavReader::open(&path).unwrap();
    assert_eq!(reader.info().frames, frames);
    reader.seek_frame(frames - 1).unwrap();
    let mut frame = [0; 2];
    assert_eq!(reader.read_samples(&mut frame).unwrap(), 2);
    assert_eq!(frame, [123, -1]);
    assert_eq!(reader.read_samples(&mut frame).unwrap(), 0);
}

#[test]
fn metadata_can_append_and_shrink_rf64_without_pcm_rewrite() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("metadata.wav");
    let largest_small_data = (u64::from(u32::MAX) - 72) & !1;
    let info = sparse_wav(&path, largest_small_data, 1);
    let new_len = info.file_len + 12;
    info.validate_file_len(new_len).unwrap();
    let mut file = OpenOptions::new()
        .read(true)
        .write(true)
        .open(&path)
        .unwrap();
    file.seek(SeekFrom::End(0)).unwrap();
    file.write_all(b"note").unwrap();
    file.write_all(&3_u32.to_le_bytes()).unwrap();
    file.write_all(b"abc\0").unwrap();
    update_sizes(&mut file, &info, new_len).unwrap();
    let tagged = inspect(&path).unwrap();
    assert!(tagged.is_rf64);
    assert_eq!(tagged.data_offset, info.data_offset);
    assert_eq!(tagged.data_len, info.data_len);
    let last = tagged.chunks.last().unwrap();
    assert_eq!(
        (last.id, last.offset, last.size),
        (*b"note", info.file_len, 3)
    );
    tagged.validate_file_len(info.file_len).unwrap();
    file.set_len(info.file_len).unwrap();
    update_sizes(&mut file, &tagged, info.file_len).unwrap();
    let shrunk = inspect(&path).unwrap();
    assert!(shrunk.is_rf64);
    assert_eq!(shrunk.file_len, info.file_len);
    assert_eq!(shrunk.chunks.len(), 3);
}

#[test]
fn invalid_format_partial_frames_and_length_overflows_are_rejected() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("invalid.wav");
    for (rate, channels) in [(0, 1), (48_000, 0), (48_000, u16::MAX), (u32::MAX, 2)] {
        assert!(ExportWavWriter::create(&path, rate, channels).is_err());
        assert!(!path.exists());
    }
    let mut writer = ExportWavWriter::create(&path, 48_000, 2).unwrap();
    writer.write_sample(1).unwrap();
    assert!(writer.finalize().is_err());
    empty_wav(&path, 2);
    let mut info = inspect(&path).unwrap();
    assert!(info.validate_file_len(info.data_offset - 1).is_err());
    info.data_len = 2;
    assert!(info.validate_file_len(info.data_offset + 2).is_err());
    info.data_len = u64::MAX - 3;
    info.frames = info.data_len / 4;
    assert!(info.validate_file_len(u64::MAX).is_err());
}

#[test]
fn truncated_data_headers_and_inconsistent_sizes_fail_inspection() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("truncated.wav");
    for length in [0, 11, 19, 47, 79] {
        empty_wav(&path, 1);
        OpenOptions::new()
            .write(true)
            .open(&path)
            .unwrap()
            .set_len(length)
            .unwrap();
        assert!(inspect(&path).is_err());
    }
    empty_wav(&path, 1);
    let mut file = OpenOptions::new().write(true).open(&path).unwrap();
    file.seek(SeekFrom::Start(76)).unwrap();
    file.write_all(&2_u32.to_le_bytes()).unwrap();
    assert!(inspect(&path).is_err());
    empty_wav(&path, 1);
    let info = inspect(&path).unwrap();
    let mut file = OpenOptions::new()
        .read(true)
        .write(true)
        .open(&path)
        .unwrap();
    update_sizes(&mut file, &info, info.file_len).unwrap();
    file.seek(SeekFrom::Start(20)).unwrap();
    file.write_all(&u64::MAX.to_le_bytes()).unwrap();
    assert!(inspect(&path).is_err());
}

#[cfg(target_os = "linux")]
#[test]
fn libsndfile_independently_reads_sparse_rf64_when_available() {
    use std::ffi::{CStr, CString};
    use std::os::unix::ffi::OsStrExt;
    #[repr(C)]
    #[derive(Default)]
    struct SfInfo {
        frames: i64,
        samplerate: i32,
        channels: i32,
        format: i32,
        sections: i32,
        seekable: i32,
    }
    type SfOpen = unsafe extern "C" fn(*const libc::c_char, i32, *mut SfInfo) -> *mut libc::c_void;
    type SfClose = unsafe extern "C" fn(*mut libc::c_void) -> i32;
    type SfSeek = unsafe extern "C" fn(*mut libc::c_void, i64, i32) -> i64;
    type SfReadShort = unsafe extern "C" fn(*mut libc::c_void, *mut i16, i64) -> i64;
    // An optional system decoder checks the produced container independently of our parser.
    unsafe {
        let library = libc::dlopen(c"libsndfile.so.1".as_ptr(), libc::RTLD_NOW);
        if library.is_null() {
            eprintln!("libsndfile unavailable; independent RF64 check skipped");
            return;
        }
        let symbol = |name: &CStr| {
            let address = libc::dlsym(library, name.as_ptr());
            assert!(!address.is_null(), "missing libsndfile symbol {name:?}");
            address
        };
        let open: SfOpen = std::mem::transmute(symbol(c"sf_open"));
        let close: SfClose = std::mem::transmute(symbol(c"sf_close"));
        let seek: SfSeek = std::mem::transmute(symbol(c"sf_seek"));
        let read: SfReadShort = std::mem::transmute(symbol(c"sf_read_short"));
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("independent.wav");
        let info = sparse_wav(&path, u64::from(u32::MAX) + 1, 2);
        let path = CString::new(path.as_os_str().as_bytes()).unwrap();
        let mut decoded = SfInfo::default();
        let file = open(path.as_ptr(), 0x10, &mut decoded);
        assert!(!file.is_null(), "libsndfile rejected RF64");
        assert_eq!(decoded.frames as u64, info.frames);
        assert_eq!((decoded.samplerate, decoded.channels), (48_000, 2));
        assert_eq!(
            seek(file, decoded.frames - 1, libc::SEEK_SET),
            decoded.frames - 1
        );
        let mut frame = [1_i16; 2];
        assert_eq!(read(file, frame.as_mut_ptr(), 2), 2);
        assert_eq!(frame, [0, 0]);
        assert_eq!(close(file), 0);
        libc::dlclose(library);
    }
}
