//! Sparse fixtures exercise real 64-bit offsets without generating a long book or PCM buffers.
use super::write_audio_tags;
use crate::audio::export_wav::{inspect, ExportWavReader};
use crate::project::project::Meta;
use id3::TagLike;
use std::collections::HashMap;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::Path;

fn sparse_rf64(path: &Path, frames: u64) {
    let data_len = frames * 2;
    let mut file = File::create(path).unwrap();
    file.write_all(b"RF64\xff\xff\xff\xffWAVEds64").unwrap();
    file.write_all(&28u32.to_le_bytes()).unwrap();
    file.write_all(&(72 + data_len).to_le_bytes()).unwrap();
    file.write_all(&data_len.to_le_bytes()).unwrap();
    file.write_all(&frames.to_le_bytes()).unwrap();
    file.write_all(&0u32.to_le_bytes()).unwrap();
    file.write_all(b"fmt ").unwrap();
    file.write_all(&16u32.to_le_bytes()).unwrap();
    file.write_all(&1u16.to_le_bytes()).unwrap();
    file.write_all(&1u16.to_le_bytes()).unwrap();
    file.write_all(&48000u32.to_le_bytes()).unwrap();
    file.write_all(&96000u32.to_le_bytes()).unwrap();
    file.write_all(&2u16.to_le_bytes()).unwrap();
    file.write_all(&16u16.to_le_bytes()).unwrap();
    file.write_all(b"data\xff\xff\xff\xff").unwrap();
    file.set_len(80 + data_len).unwrap();
    file.write_all(&1234i16.to_le_bytes()).unwrap();
    file.seek(SeekFrom::End(-2)).unwrap();
    file.write_all(&(-2345i16).to_le_bytes()).unwrap();
}

#[test]
fn rf64_metadata_uses_64_bit_positions_without_reading_or_moving_audio() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("long.wav");
    let frames = u64::from(u32::MAX) + 32;
    sparse_rf64(&path, frames);
    let meta = Meta {
        title: "Long book".into(),
        ..Meta::default()
    };
    let long_title = "Глава".repeat(70);
    let markers = [("Начало".into(), 0), (long_title, frames - 1)];

    for _ in 0..2 {
        write_audio_tags(
            &path,
            &meta,
            "",
            &[],
            &HashMap::new(),
            directory.path(),
            &markers,
            48000,
            1,
        )
        .unwrap();
        let info = inspect(&path).unwrap();
        assert!(info.is_rf64);
        assert_eq!(info.frames, frames);
        assert_eq!(info.data_offset, 80);
        assert_eq!(info.data_len, frames * 2);
        let chunk = info.chunks.iter().find(|c| c.id == *b"r64m").unwrap();
        assert_eq!(chunk.size, 640);
        assert!(!info.chunks.iter().any(|c| c.id == *b"cue "));
        let mut file = File::open(&path).unwrap();
        file.seek(SeekFrom::Start(chunk.offset + 8)).unwrap();
        let mut entries = [0u8; 640];
        file.read_exact(&mut entries).unwrap();
        assert_eq!(u32::from_le_bytes(entries[..4].try_into().unwrap()), 0x11);
        assert_eq!(
            u32::from_le_bytes(entries[320..324].try_into().unwrap()),
            0x19
        );
        assert_eq!(
            u64::from_le_bytes(entries[324..332].try_into().unwrap()),
            frames - 1
        );
        assert_eq!(u32::from_le_bytes(entries[604..608].try_into().unwrap()), 2);
        let tag_chunk = info.chunks.iter().find(|c| c.id == *b"id3 ").unwrap();
        file.seek(SeekFrom::Start(tag_chunk.offset + 8)).unwrap();
        let mut tag_bytes = vec![0; tag_chunk.size as usize];
        file.read_exact(&mut tag_bytes).unwrap();
        let tag = id3::Tag::read_from2(std::io::Cursor::new(tag_bytes)).unwrap();
        assert_eq!(tag.title(), Some("Long book"));
        let mut reader = ExportWavReader::open(&path).unwrap();
        let mut sample = [0i16; 1];
        assert_eq!(reader.read_samples(&mut sample).unwrap(), 1);
        assert_eq!(sample, [1234]);
        reader.seek_frame(frames - 1).unwrap();
        assert_eq!(reader.read_samples(&mut sample).unwrap(), 1);
        assert_eq!(sample, [-2345]);
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            assert!(std::fs::metadata(&path).unwrap().blocks() * 512 < 1024 * 1024);
        }
    }
    // Opt-in diagnostic artifact for independent ffprobe/libsndfile checks.
    if let Some(output) = std::env::var_os("IAMREADER_RF64_PROBE_DIR") {
        let output = std::path::PathBuf::from(output);
        std::fs::create_dir_all(&output).unwrap();
        std::fs::rename(&path, output.join("long-tagged.wav")).unwrap();
    }
}
