use crate::audio::export_wav::{inspect, update_sizes, WavInfo};
use crate::project::project::{MarkerSettings, Meta, ProjectFile};
use crate::utils::assets;
use anyhow::{Context, Result};
use log::warn;
use std::collections::HashMap;
use std::path::Path;

/// Записывает метаданные (ID3 теги) в аудиофайл
pub fn write_audio_tags(
    output_path: &Path,
    meta: &Meta,
    cover: &str,
    files: &[&ProjectFile],
    markers: &HashMap<String, MarkerSettings>,
    project_dir: &Path,
    section_markers: &[(String, u64)], // (marker_title, position_in_samples)
    sample_rate: u32,
    channels: u16,
) -> Result<()> {
    use chrono::Datelike;
    use id3::frame::PictureType;
    use id3::{Content, Frame, Tag, TagLike};
    use std::fs;

    // Validate the complete WAV container before making any changes. The export
    // workspace owns this file; publication happens only after all metadata succeeds.
    let wav_info = if output_path.extension().and_then(|s| s.to_str()) == Some("wav") {
        let info = inspect(output_path)?;
        anyhow::ensure!(info.sample_rate == sample_rate, "WAV sample rate mismatch");
        anyhow::ensure!(info.channels == channels, "WAV channel count mismatch");
        for (_, position) in section_markers {
            anyhow::ensure!(
                position.is_multiple_of(u64::from(channels)),
                "Marker is not frame-aligned"
            );
            anyhow::ensure!(
                position / u64::from(channels) <= info.frames,
                "Marker exceeds WAV duration"
            );
        }
        Some(info)
    } else {
        None
    };

    // Определяем значения полей метаданных
    // Проверяем, есть ли среди файлов записи с section маркерами
    let mut has_section_marker = false;
    let mut section_file: Option<&ProjectFile> = None;

    for file in files.iter().rev() {
        let file_has_section = file.markers.iter().any(|marker_name| {
            markers
                .get(marker_name)
                .map(|settings| settings.section)
                .unwrap_or(false)
        });
        if file_has_section {
            has_section_marker = true;
            section_file = Some(file);
            break; // Берем самую свежую запись с section маркером
        }
    }

    // Определяем значения полей
    let title = match section_file {
        Some(sf) if has_section_marker && !sf.title.is_empty() => sf.title.clone(),
        _ => meta.title.clone(),
    };
    let artist = match section_file {
        Some(sf) if has_section_marker && !sf.author.is_empty() => sf.author.clone(),
        _ => meta.author.clone(),
    };
    let original_release_date = match section_file {
        Some(sf) if has_section_marker && !sf.year.is_empty() => sf.year.clone(),
        _ => meta.year.clone(),
    };

    let year = chrono::Local::now().year();
    let composer = meta.reader.clone();
    let album_artist = meta.reader.clone();
    let album = meta.title.clone();
    let genre = "Audiobook".to_string();
    let software = format!("iamreader {}", env!("CARGO_PKG_VERSION"));

    // Открываем или создаем теги
    let mut tag = if let Some(info) = &wav_info {
        read_wav_id3(output_path, info)?
    } else {
        Tag::read_from_path(output_path).unwrap_or_else(|_| Tag::new())
    };

    // Устанавливаем теги
    tag.set_title(title);
    tag.set_artist(artist);
    if !original_release_date.is_empty() {
        if let Ok(year_num) = original_release_date.parse::<i32>() {
            tag.set_date_released(id3::Timestamp {
                year: year_num,
                month: None,
                day: None,
                hour: None,
                minute: None,
                second: None,
            });
        }
    }
    tag.set_date_recorded(id3::Timestamp {
        year,
        month: None,
        day: None,
        hour: None,
        minute: None,
        second: None,
    });

    // Composer через кастомный фрейм
    if !composer.is_empty() {
        tag.add_frame(Frame::with_content("TCOM", Content::Text(composer)));
    }

    tag.set_album_artist(album_artist);
    tag.set_album(album);
    tag.set_genre(genre);

    // Добавляем software tag (TSSE)
    tag.add_frame(Frame::with_content("TSSE", Content::Text(software)));

    // Загружаем обложку, если указана
    if !cover.is_empty() {
        let cover_path = project_dir.join(cover);
        let cover_data = if cover_path.exists() {
            fs::read(&cover_path)
                .with_context(|| format!("Failed to read cover file: {:?}", cover_path))?
        } else {
            // Пытаемся использовать внедрённую обложку
            if let Some(data) = assets::get_asset_file(cover)? {
                data
            } else {
                warn!(
                    "Cover file not found: {:?} (also checked embedded assets)",
                    cover
                );
                // A missing optional cover must not skip the tags or WAV table of contents.
                Vec::new()
            }
        };

        if !cover_data.is_empty() {
            // Определяем MIME тип по расширению
            let mime_type = if cover_path.extension().and_then(|s| s.to_str()) == Some("png") {
                "image/png"
            } else if cover_path.extension().and_then(|s| s.to_str()) == Some("jpg")
                || cover_path.extension().and_then(|s| s.to_str()) == Some("jpeg")
            {
                "image/jpeg"
            } else {
                "image/jpeg" // По умолчанию
            };

            let picture = id3::frame::Picture {
                mime_type: mime_type.to_string(),
                picture_type: PictureType::CoverFront,
                description: String::new(),
                data: cover_data,
            };
            tag.add_frame(Frame::with_content("APIC", Content::Picture(picture)));
        }
    }

    // Сохраняем существующее пользовательское представление глав в валидных TXXX.
    // Это не стандартные CHAP frames: описание различает главы при добавлении в Tag.
    if !section_markers.is_empty()
        && output_path.extension().and_then(|s| s.to_str()) == Some("mp3")
    {
        anyhow::ensure!(
            sample_rate > 0 && channels > 0,
            "MP3 chapter timestamps require a nonzero sample rate and channel count"
        );
        let samples_per_second = u128::from(sample_rate) * u128::from(channels);
        for (index, (title, position_samples)) in section_markers.iter().enumerate() {
            // Позиции содержат interleaved samples всех каналов; миллисекунды
            // вычисляются целочисленно без потери точности или насыщения до u32.
            let position_ms = u128::from(*position_samples) * 1000 / samples_per_second;
            let chapter_id = format!("ch{:02}", index + 1);
            tag.add_frame(id3::frame::ExtendedText {
                description: format!("CHAP:{chapter_id}"),
                value: format!("CHAP|{chapter_id}|{position_ms}|{title}"),
            });
        }
    }

    if let Some(info) = &wav_info {
        write_wav_metadata(output_path, info, &tag, section_markers)?;
    } else {
        tag.write_to_path(output_path, id3::Version::Id3v24)
            .with_context(|| format!("Failed to write ID3 tags to: {:?}", output_path))?;
    }
    Ok(())
}

// Only tag bytes are read: id3's container writer must NOT see RF64, which it
// mistakes for a plain file and would prefix with ID3, invalidating the header.
fn read_wav_id3(path: &Path, info: &WavInfo) -> Result<id3::Tag> {
    use std::io::{Read, Seek, SeekFrom};
    let Some(chunk) = info
        .chunks
        .iter()
        .find(|c| c.id.eq_ignore_ascii_case(b"id3 "))
    else {
        return Ok(id3::Tag::new());
    };
    let mut file = std::fs::File::open(path)?;
    file.seek(SeekFrom::Start(chunk.offset + 8))?;
    let mut bytes = Vec::new();
    file.take(chunk.size).read_to_end(&mut bytes)?;
    Ok(id3::Tag::read_from2(std::io::Cursor::new(bytes)).unwrap_or_else(|_| id3::Tag::new()))
}

fn append_chunk(target: &mut Vec<u8>, id: &[u8; 4], contents: &[u8]) -> Result<()> {
    let size = u32::try_from(contents.len()).context("WAV metadata chunk exceeds 32-bit size")?;
    anyhow::ensure!(
        size != u32::MAX,
        "WAV metadata chunk size uses reserved sentinel"
    );
    target.extend_from_slice(id);
    target.extend_from_slice(&size.to_le_bytes());
    target.extend_from_slice(contents);
    if !size.is_multiple_of(2) {
        target.push(0);
    }
    Ok(())
}

/// Metadata is proportional to the tag/TOC, never to PCM length. Audio is neither
/// copied nor shifted. Only private staged exports may be modified by this helper.
fn write_wav_metadata(
    path: &Path,
    info: &WavInfo,
    tag: &id3::Tag,
    section_markers: &[(String, u64)],
) -> Result<()> {
    use std::fs::OpenOptions;
    use std::io::{Read, Seek, SeekFrom, Write};

    let mut prepared = Vec::new();
    let mut id3_bytes = Vec::new();
    tag.write_to(&mut id3_bytes, id3::Version::Id3v24)?;
    append_chunk(&mut prepared, b"id3 ", &id3_bytes)?;

    let count = u32::try_from(section_markers.len()).context("Too many WAV markers")?;
    let needs_wide_markers = section_markers
        .iter()
        .any(|(_, pos)| pos / u64::from(info.channels) > u64::from(u32::MAX));
    let mut cues = Vec::new();
    let mut labels = b"adtl".to_vec();
    let mut wide_markers = Vec::new();
    // Do not publish a misleading partial legacy TOC when 32-bit positions are
    // insufficient. r64m carries the complete 64-bit TOC (EBU Tech 3306, Annex A.4).
    if count > 0 && !needs_wide_markers {
        cues.extend_from_slice(&count.to_le_bytes());
    }
    for (index, (title, position)) in section_markers.iter().enumerate() {
        anyhow::ensure!(!title.contains('\0'), "WAV marker title contains NUL");
        let id = u32::try_from(index + 1)?;
        let frame = position / u64::from(info.channels);
        if !needs_wide_markers {
            let frame = u32::try_from(frame)?;
            cues.extend_from_slice(&id.to_le_bytes());
            cues.extend_from_slice(&frame.to_le_bytes());
            cues.extend_from_slice(b"data");
            cues.extend_from_slice(&0u32.to_le_bytes()); // chunkStart: only used by wave lists
            cues.extend_from_slice(&0u32.to_le_bytes()); // blockStart: not used by PCM
            cues.extend_from_slice(&frame.to_le_bytes());
        }
        let mut label = id.to_le_bytes().to_vec();
        label.extend_from_slice(title.as_bytes());
        label.push(0);
        append_chunk(&mut labels, b"labl", &label)?;
        if needs_wide_markers {
            let mut entry = [0u8; 320];
            // valid + UTF-8; long strings refer to the unabridged LIST/labl entry.
            let flags = if title.len() <= 255 { 0x11u32 } else { 0x19u32 };
            entry[..4].copy_from_slice(&flags.to_le_bytes());
            entry[4..12].copy_from_slice(&frame.to_le_bytes());
            if title.len() <= 255 {
                entry[28..28 + title.len()].copy_from_slice(title.as_bytes());
            } else {
                entry[284..288].copy_from_slice(&id.to_le_bytes());
            }
            wide_markers.extend_from_slice(&entry);
        }
    }
    if !cues.is_empty() {
        append_chunk(&mut prepared, b"cue ", &cues)?;
    }
    if count > 0 {
        append_chunk(&mut prepared, b"LIST", &labels)?;
    }
    if !wide_markers.is_empty() {
        append_chunk(&mut prepared, b"r64m", &wide_markers)?;
    }

    let mut file = OpenOptions::new().read(true).write(true).open(path)?;
    let mut obsolete = Vec::new();
    for chunk in &info.chunks {
        let owned = if chunk.id.eq_ignore_ascii_case(b"LIST") && chunk.size >= 4 {
            file.seek(SeekFrom::Start(chunk.offset + 8))?;
            let mut kind = [0u8; 4];
            file.read_exact(&mut kind)?;
            kind == *b"adtl"
        } else {
            chunk.id.eq_ignore_ascii_case(b"id3 ") || chunk.id == *b"cue " || chunk.id == *b"r64m"
        };
        if owned {
            obsolete.push(chunk.offset);
        }
    }
    // Reuse a trailing metadata region instead of growing on every tag update.
    let mut append_at = info.file_len;
    for chunk in info.chunks.iter().rev() {
        if obsolete.binary_search(&chunk.offset).is_err() {
            break;
        }
        append_at = chunk.offset;
    }
    let new_len = append_at
        .checked_add(u64::try_from(prepared.len())?)
        .context("WAV size overflow")?;
    info.validate_file_len(new_len)?;
    file.seek(SeekFrom::Start(append_at))?;
    file.write_all(&prepared)
        .context("Failed to write WAV metadata")?;
    file.set_len(new_len)?;
    for offset in obsolete.into_iter().filter(|offset| *offset < append_at) {
        file.seek(SeekFrom::Start(offset))?;
        file.write_all(b"JUNK")?;
    }
    update_sizes(&mut file, info, new_len)?;
    file.flush()?;
    Ok(())
}

#[cfg(test)]
#[path = "../../tests/project/metadata_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "../../tests/project/metadata_large_tests.rs"]
mod large_tests;
