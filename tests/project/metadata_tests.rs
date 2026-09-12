use super::write_audio_tags;
use crate::project::project::{MarkerSettings, Meta, ProjectFile};
use id3::{Tag, TagLike};
use std::collections::HashMap;
use std::path::Path;

const SAMPLE_RATE: u32 = 8000;
const CHANNELS: u16 = 2;
const AUDIO_SAMPLES: [i16; 8] = [100, -100, 200, -200, 300, -300, 400, -400];
const COVER_PNG: &[u8] = &[
    0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x48, 0x44, 0x52,
    0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x04, 0x00, 0x00, 0x00, 0xb5, 0x1c, 0x0c,
    0x02, 0x00, 0x00, 0x00, 0x0b, 0x49, 0x44, 0x41, 0x54, 0x78, 0xda, 0x63, 0x64, 0xf8, 0x0f, 0x00,
    0x01, 0x05, 0x01, 0x01, 0x27, 0x18, 0xe3, 0x66, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4e, 0x44,
    0xae, 0x42, 0x60, 0x82,
];

fn project_meta() -> Meta {
    Meta {
        title: "Название книги".to_owned(),
        author: "Автор книги".to_owned(),
        year: "2024".to_owned(),
        hint: String::new(),
        reader: "Чтец книги".to_owned(),
    }
}

fn create_audio(path: &Path) {
    let spec = hound::WavSpec {
        channels: CHANNELS,
        sample_rate: SAMPLE_RATE,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut writer = hound::WavWriter::create(path, spec).unwrap();
    for sample in AUDIO_SAMPLES {
        writer.write_sample(sample).unwrap();
    }
    writer.finalize().unwrap();
}

fn write_project_tags(path: &Path, directory: &Path, cover: &str) -> Tag {
    write_audio_tags(
        path,
        &project_meta(),
        cover,
        &[],
        &HashMap::new(),
        directory,
        &[("One".to_owned(), 0), ("Two".to_owned(), 4)],
        SAMPLE_RATE,
        CHANNELS,
    )
    .unwrap();
    Tag::read_from_path(path).unwrap()
}

fn assert_project_tags(tag: &Tag) {
    let meta = project_meta();
    assert_eq!(tag.title(), Some(meta.title.as_str()));
    assert_eq!(tag.artist(), Some(meta.author.as_str()));
    assert_eq!(tag.date_released().unwrap().year, 2024);
    assert_eq!(tag.album(), Some(meta.title.as_str()));
    assert_eq!(tag.album_artist(), Some(meta.reader.as_str()));
    assert_eq!(
        tag.get("TCOM").unwrap().content().text(),
        Some(meta.reader.as_str())
    );
}

fn riff_chunk<'a>(data: &'a [u8], expected_id: &[u8; 4]) -> &'a [u8] {
    assert_eq!(&data[..4], b"RIFF");
    assert_eq!(&data[8..12], b"WAVE");
    let mut offset = 12;
    while offset + 8 <= data.len() {
        let size = u32::from_le_bytes(data[offset + 4..offset + 8].try_into().unwrap()) as usize;
        let contents = &data[offset + 8..offset + 8 + size];
        if &data[offset..offset + 4] == expected_id {
            return contents;
        }
        offset += 8 + size + size % 2;
    }
    panic!("Missing RIFF chunk {expected_id:?}");
}

fn assert_wav_toc_and_audio(path: &Path) {
    let bytes = std::fs::read(path).unwrap();
    let cues = riff_chunk(&bytes, b"cue ");
    assert_eq!(u32::from_le_bytes(cues[..4].try_into().unwrap()), 2);
    let labels = riff_chunk(&bytes, b"LIST");
    assert_eq!(&labels[..4], b"adtl");
    assert!(labels.windows(4).any(|window| window == b"One\0"));
    assert!(labels.windows(4).any(|window| window == b"Two\0"));

    let mut reader = hound::WavReader::open(path).unwrap();
    assert_eq!(reader.spec().sample_rate, SAMPLE_RATE);
    assert_eq!(reader.spec().channels, CHANNELS);
    assert_eq!(
        reader
            .samples::<i16>()
            .collect::<Result<Vec<_>, _>>()
            .unwrap(),
        AUDIO_SAMPLES,
    );
}

#[derive(Debug)]
struct RiffChunk<'a> {
    id: [u8; 4],
    header_offset: usize,
    payload: &'a [u8],
}

fn read_u32(bytes: &[u8]) -> u32 {
    u32::from_le_bytes(bytes.try_into().unwrap())
}

// Independent test parser: every top-level and adtl child chunk must fit, including its pad.
fn walk_riff_chunks(bytes: &[u8], start: usize, end: usize) -> Vec<RiffChunk<'_>> {
    assert!(start <= end && end <= bytes.len());
    let mut chunks = Vec::new();
    let mut offset = start;
    while offset < end {
        assert!(end - offset >= 8, "Truncated chunk header at {offset}");
        let size = read_u32(&bytes[offset + 4..offset + 8]) as usize;
        let payload_start = offset.checked_add(8).unwrap();
        let payload_end = payload_start.checked_add(size).unwrap();
        let padded_end = payload_end.checked_add(size % 2).unwrap();
        assert!(padded_end <= end, "Chunk at {offset} exceeds its container");
        chunks.push(RiffChunk {
            id: bytes[offset..offset + 4].try_into().unwrap(),
            header_offset: offset,
            payload: &bytes[payload_start..payload_end],
        });
        offset = padded_end;
    }
    assert_eq!(offset, end);
    chunks
}

fn parse_riff_chunks(bytes: &[u8]) -> Vec<RiffChunk<'_>> {
    assert!(bytes.len() >= 12);
    assert_eq!(&bytes[..4], b"RIFF");
    assert_eq!(&bytes[8..12], b"WAVE");
    let riff_end = (read_u32(&bytes[4..8]) as usize).checked_add(8).unwrap();
    assert_eq!(
        riff_end,
        bytes.len(),
        "RIFF size must cover the complete file"
    );
    walk_riff_chunks(bytes, 12, riff_end)
}

fn single_chunk<'a, 'b>(chunks: &'b [RiffChunk<'a>], id: &[u8; 4]) -> &'b RiffChunk<'a> {
    let matching: Vec<_> = chunks.iter().filter(|chunk| &chunk.id == id).collect();
    assert_eq!(matching.len(), 1, "Expected one live {id:?} chunk");
    matching[0]
}

fn assert_cue_points_and_labels(chunks: &[RiffChunk<'_>], expected: &[(String, u64)]) {
    let cue = single_chunk(chunks, b"cue ");
    assert_eq!(read_u32(&cue.payload[..4]) as usize, expected.len());
    assert_eq!(cue.payload.len(), 4 + expected.len() * 24);
    let lists: Vec<_> = chunks
        .iter()
        .filter(|chunk| chunk.id == *b"LIST" && chunk.payload.starts_with(b"adtl"))
        .collect();
    assert_eq!(lists.len(), 1, "Expected one live LIST/adtl chunk");
    let labels = walk_riff_chunks(lists[0].payload, 4, lists[0].payload.len());
    assert_eq!(labels.len(), expected.len());
    for (index, ((title, position_samples), label)) in expected.iter().zip(labels).enumerate() {
        let point = &cue.payload[4 + index * 24..4 + (index + 1) * 24];
        let cue_id = read_u32(&point[..4]);
        assert_eq!(cue_id as usize, index + 1);
        assert_eq!(&point[8..12], b"data");
        assert_eq!(
            read_u32(&point[12..16]),
            0,
            "chunkStart is not a file offset"
        );
        assert_eq!(read_u32(&point[16..20]), 0, "PCM blockStart must be zero");
        assert_eq!(
            u64::from(read_u32(&point[20..24])),
            position_samples / u64::from(CHANNELS),
            "sampleOffset counts interleaved stereo frames, not scalar samples"
        );
        assert_eq!(&label.id, b"labl");
        assert_eq!(read_u32(&label.payload[..4]), cue_id);
        assert_eq!(label.payload.len(), 4 + title.len() + 1);
        assert_eq!(&label.payload[4..label.payload.len() - 1], title.as_bytes());
        assert_eq!(label.payload.last(), Some(&0));
    }
}

fn assert_tagging_rejected_without_changes(
    path: &Path,
    directory: &Path,
    section_markers: &[(String, u64)],
    sample_rate: u32,
    channels: u16,
) {
    let before = std::fs::read(path).unwrap();
    let result = write_audio_tags(
        path,
        &project_meta(),
        "",
        &[],
        &HashMap::new(),
        directory,
        section_markers,
        sample_rate,
        channels,
    );
    assert!(
        result.is_err(),
        "Invalid WAV/tagging input unexpectedly succeeded: {path:?}"
    );
    assert_eq!(
        std::fs::read(path).unwrap(),
        before,
        "Failed validation modified {path:?}"
    );
}

#[test]
fn empty_cover_still_writes_project_tags_and_wav_toc() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("empty-cover.wav");
    create_audio(&path);

    let tag = write_project_tags(&path, directory.path(), "");

    assert_project_tags(&tag);
    assert_eq!(tag.pictures().count(), 0);
    assert_wav_toc_and_audio(&path);
}

#[test]
fn missing_cover_still_writes_project_tags_and_wav_toc() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("missing-cover.wav");
    let missing_cover = directory.path().join("does-not-exist.png");
    create_audio(&path);

    let tag = write_project_tags(&path, directory.path(), missing_cover.to_str().unwrap());

    assert_project_tags(&tag);
    assert_eq!(tag.pictures().count(), 0);
    assert!(!missing_cover.exists());
    assert_wav_toc_and_audio(&path);
}

#[test]
fn existing_cover_is_embedded_alongside_project_tags_and_wav_toc() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("with-cover.wav");
    std::fs::write(directory.path().join("custom.png"), COVER_PNG).unwrap();
    create_audio(&path);

    let tag = write_project_tags(&path, directory.path(), "custom.png");

    assert_project_tags(&tag);
    let pictures: Vec<_> = tag.pictures().collect();
    assert_eq!(pictures.len(), 1);
    assert_eq!(pictures[0].mime_type, "image/png");
    assert_eq!(
        pictures[0].picture_type,
        id3::frame::PictureType::CoverFront
    );
    assert_eq!(pictures[0].data, COVER_PNG);
    assert_wav_toc_and_audio(&path);
}

#[test]
fn missing_cover_preserves_section_metadata_overrides_and_project_reader() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("section.wav");
    let missing_cover = directory.path().join("does-not-exist.png");
    create_audio(&path);
    let meta = project_meta();
    let file = ProjectFile {
        path: "unused-source.wav".to_owned(),
        title: "Название главы".to_owned(),
        author: "Автор главы".to_owned(),
        year: "2021".to_owned(),
        hint: String::new(),
        markers: vec!["chapter".to_owned()],
        size: 0,
        duration_ms: 0,
    };
    let mut markers = HashMap::new();
    markers.insert(
        "chapter".to_owned(),
        MarkerSettings {
            section: true,
            ..MarkerSettings::default()
        },
    );

    write_audio_tags(
        &path,
        &meta,
        missing_cover.to_str().unwrap(),
        &[&file],
        &markers,
        directory.path(),
        &[("One".to_owned(), 0), ("Two".to_owned(), 4)],
        SAMPLE_RATE,
        CHANNELS,
    )
    .unwrap();

    let tag = Tag::read_from_path(&path).unwrap();
    assert_eq!(tag.title(), Some(file.title.as_str()));
    assert_eq!(tag.artist(), Some(file.author.as_str()));
    assert_eq!(tag.date_released().unwrap().year, 2021);
    assert_eq!(tag.album(), Some(meta.title.as_str()));
    assert_eq!(tag.album_artist(), Some(meta.reader.as_str()));
    assert_eq!(
        tag.get("TCOM").unwrap().content().text(),
        Some(meta.reader.as_str())
    );
    assert_eq!(tag.pictures().count(), 0);
    assert_wav_toc_and_audio(&path);
}

#[test]
fn repeated_wav_tagging_replaces_live_metadata_without_moving_audio() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("repeated.wav");
    create_audio(&path);
    let original = std::fs::read(&path).unwrap();
    let original_chunks = parse_riff_chunks(&original);
    let original_data = single_chunk(&original_chunks, b"data");

    for revision in 1..=3 {
        let mut meta = project_meta();
        meta.title = format!("Book revision {revision}");
        let chapters = vec![
            (format!("Opening revision {revision}"), 0),
            (format!("Ending revision {revision}"), 4),
        ];
        write_audio_tags(
            &path,
            &meta,
            "",
            &[],
            &HashMap::new(),
            directory.path(),
            &chapters,
            SAMPLE_RATE,
            CHANNELS,
        )
        .unwrap();

        let bytes = std::fs::read(&path).unwrap();
        let chunks = parse_riff_chunks(&bytes);
        assert_eq!(
            chunks
                .iter()
                .filter(|chunk| chunk.id.eq_ignore_ascii_case(b"id3 "))
                .count(),
            1,
            "Repeated tagging must leave exactly one live ID3 chunk"
        );
        let tag = Tag::read_from_path(&path).unwrap();
        assert_eq!(tag.title(), Some(meta.title.as_str()));
        assert_eq!(tag.album(), Some(meta.title.as_str()));
        assert_cue_points_and_labels(&chunks, &chapters);
        let data = single_chunk(&chunks, b"data");
        assert_eq!(data.header_offset, original_data.header_offset);
        assert_eq!(data.payload, original_data.payload);
        let mut reader = hound::WavReader::open(&path).unwrap();
        assert_eq!(
            reader
                .samples::<i16>()
                .collect::<Result<Vec<_>, _>>()
                .unwrap(),
            AUDIO_SAMPLES
        );
    }
}

#[test]
fn wav_labels_pad_odd_payloads_and_cues_use_stereo_frame_offsets() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("padded-labels.wav");
    create_audio(&path);
    // Four cue-id bytes + two UTF-8 bytes + NUL = seven bytes: next labl needs a pad.
    let chapters = vec![
        ("AB".to_owned(), 0),
        ("Next".to_owned(), 4),
        ("End".to_owned(), 8),
    ];
    write_audio_tags(
        &path,
        &project_meta(),
        "",
        &[],
        &HashMap::new(),
        directory.path(),
        &chapters,
        SAMPLE_RATE,
        CHANNELS,
    )
    .unwrap();

    let bytes = std::fs::read(&path).unwrap();
    let chunks = parse_riff_chunks(&bytes);
    assert_cue_points_and_labels(&chunks, &chapters);
    let list = single_chunk(&chunks, b"LIST");
    let labels = walk_riff_chunks(list.payload, 4, list.payload.len());
    assert_eq!(labels[0].payload.len(), 7);
    assert_eq!(labels[1].header_offset, labels[0].header_offset + 8 + 7 + 1);
    assert_eq!(list.payload[labels[1].header_offset - 1], 0);
    assert_eq!(
        &list.payload[labels[1].header_offset..labels[1].header_offset + 4],
        b"labl"
    );
}

#[test]
fn invalid_riff_chunk_bounds_are_rejected_before_any_tag_write() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("invalid-riff.wav");
    create_audio(&path);
    let original = std::fs::read(&path).unwrap();
    let chunks = parse_riff_chunks(&original);
    let data_offset = single_chunk(&chunks, b"data").header_offset;
    for corruption in [
        "riff-extends-past-file",
        "riff-ends-inside-data",
        "data-extends-past-riff",
        "maximal-data-size",
        "truncated-chunk-header",
        "missing-odd-chunk-pad",
    ] {
        let mut bytes = original.clone();
        match corruption {
            "riff-extends-past-file" => {
                let size = (bytes.len() - 8 + 2) as u32;
                bytes[4..8].copy_from_slice(&size.to_le_bytes());
            }
            "riff-ends-inside-data" => {
                let size = (bytes.len() - 8 - 2) as u32;
                bytes[4..8].copy_from_slice(&size.to_le_bytes());
            }
            "data-extends-past-riff" => {
                let size = (AUDIO_SAMPLES.len() * 2 + 2) as u32;
                bytes[data_offset + 4..data_offset + 8].copy_from_slice(&size.to_le_bytes());
            }
            "maximal-data-size" => {
                bytes[data_offset + 4..data_offset + 8].copy_from_slice(&u32::MAX.to_le_bytes());
            }
            "truncated-chunk-header" => {
                bytes.extend_from_slice(b"JUNK\0");
                let size = (bytes.len() - 8) as u32;
                bytes[4..8].copy_from_slice(&size.to_le_bytes());
            }
            "missing-odd-chunk-pad" => {
                bytes.extend_from_slice(b"note");
                bytes.extend_from_slice(&1_u32.to_le_bytes());
                bytes.push(b'x');
                let size = (bytes.len() - 8) as u32;
                bytes[4..8].copy_from_slice(&size.to_le_bytes());
            }
            _ => unreachable!(),
        }
        let path = directory.path().join(format!("{corruption}.wav"));
        std::fs::write(&path, &bytes).unwrap();
        assert_tagging_rejected_without_changes(
            &path,
            directory.path(),
            &[("Chapter".to_owned(), 0)],
            SAMPLE_RATE,
            CHANNELS,
        );
    }
}

#[test]
fn wav_partial_pcm_frame_is_rejected_before_any_tag_write() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("partial-frame.wav");
    create_audio(&path);
    let mut bytes = std::fs::read(&path).unwrap();
    let chunks = parse_riff_chunks(&bytes);
    let data_offset = single_chunk(&chunks, b"data").header_offset;
    // Preserve valid RIFF boundaries but remove half of the final stereo frame.
    bytes.truncate(bytes.len() - 2);
    bytes[data_offset + 4..data_offset + 8]
        .copy_from_slice(&((AUDIO_SAMPLES.len() * 2 - 2) as u32).to_le_bytes());
    let riff_size = (bytes.len() - 8) as u32;
    bytes[4..8].copy_from_slice(&riff_size.to_le_bytes());
    parse_riff_chunks(&bytes);
    std::fs::write(&path, &bytes).unwrap();

    assert_tagging_rejected_without_changes(
        &path,
        directory.path(),
        &[("Chapter".to_owned(), 0)],
        SAMPLE_RATE,
        CHANNELS,
    );
}

#[test]
fn mismatched_wav_tagging_format_is_rejected_without_modifying_audio() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("format-mismatch.wav");
    create_audio(&path);
    for (sample_rate, channels) in [
        (SAMPLE_RATE, 1),
        (SAMPLE_RATE, 0),
        (4000, CHANNELS),
        (0, CHANNELS),
    ] {
        assert_tagging_rejected_without_changes(
            &path,
            directory.path(),
            &[("Chapter".to_owned(), 0)],
            sample_rate,
            channels,
        );
    }
}

#[test]
fn invalid_wav_chapter_positions_are_rejected_without_modifying_audio() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("invalid-position.wav");
    create_audio(&path);
    for position_samples in [3, AUDIO_SAMPLES.len() as u64 + u64::from(CHANNELS)] {
        assert_tagging_rejected_without_changes(
            &path,
            directory.path(),
            &[("Invalid chapter".to_owned(), position_samples)],
            SAMPLE_RATE,
            CHANNELS,
        );
    }
}

fn create_mp3(path: &Path) {
    let wav = path.with_extension("wav");
    let mut writer = crate::audio::export_wav::ExportWavWriter::create(&wav, 48_000, 2).unwrap();
    for _ in 0..24_000 {
        writer.write_sample(1234).unwrap();
    }
    writer.finalize().unwrap();
    crate::audio::processing::encode_wav_to_mp3(&wav, path, 48_000, 2, &mut |_, _| Ok(())).unwrap();
}

#[test]
fn mp3_chapters_are_readable_unique_extended_texts_with_stereo_timestamps() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("book.mp3");
    create_mp3(&path);
    let mut existing = Tag::new();
    existing.set_text("TCOP", "Existing copyright");
    existing.add_frame(id3::frame::ExtendedText {
        description: "Unrelated user text".into(),
        value: "Preserve me".into(),
    });
    existing.write_to_path(&path, id3::Version::Id3v24).unwrap();
    let chapters = [("Глава первая".into(), 0), ("Глава вторая".into(), 12_000)];
    // Rewriting the same chapter IDs replaces their frames, rather than losing
    // all but the last chapter or accumulating duplicate TXXX frames.
    for _ in 0..2 {
        write_audio_tags(
            &path,
            &project_meta(),
            "",
            &[],
            &HashMap::new(),
            directory.path(),
            &chapters,
            48_000,
            2,
        )
        .unwrap();
        let tag = Tag::read_from_path(&path).unwrap();
        assert_project_tags(&tag);
        assert_eq!(tag.genre(), Some("Audiobook"));
        assert_eq!(
            tag.get("TCOP").unwrap().content().text(),
            Some("Existing copyright")
        );
        assert!(tag
            .get("TSSE")
            .unwrap()
            .content()
            .text()
            .unwrap()
            .starts_with("iamreader "));
        assert!(tag.date_recorded().is_some());
        assert!(tag.get("CHAP").is_none());
        let texts: HashMap<_, _> = tag
            .extended_texts()
            .map(|text| (text.description.as_str(), text.value.as_str()))
            .collect();
        assert_eq!(tag.extended_texts().count(), 3);
        assert_eq!(texts["CHAP:ch01"], "CHAP|ch01|0|Глава первая");
        // 12,000 interleaved stereo samples / (48,000 * 2) = 125 milliseconds.
        assert_eq!(texts["CHAP:ch02"], "CHAP|ch02|125|Глава вторая");
        assert_eq!(texts["Unrelated user text"], "Preserve me");
    }
}

#[test]
fn invalid_mp3_chapter_clock_is_rejected_without_changing_the_file() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("book.mp3");
    create_mp3(&path);
    let before = std::fs::read(&path).unwrap();
    for (rate, channels) in [(0, 2), (48_000, 0)] {
        assert!(write_audio_tags(
            &path,
            &project_meta(),
            "",
            &[],
            &HashMap::new(),
            directory.path(),
            &[("Chapter".into(), 0)],
            rate,
            channels,
        )
        .is_err());
        assert_eq!(std::fs::read(&path).unwrap(), before);
    }
}
