use crate::project::project::{ProjectFile, Meta, MarkerSettings};
use crate::utils::assets;
use std::path::Path;
use std::collections::HashMap;
use anyhow::{Result, Context};
use log::warn;

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
    use id3::{Tag, TagLike, Frame, Content};
    use id3::frame::PictureType;
    use std::fs;
    use chrono::Datelike;
    
    // Определяем значения полей метаданных
    // Проверяем, есть ли среди файлов записи с section маркерами
    let mut has_section_marker = false;
    let mut section_file: Option<&ProjectFile> = None;
    
    for file in files.iter().rev() {
        let file_has_section = file.markers.iter().any(|marker_name| {
            markers.get(marker_name)
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
    let mut tag = match Tag::read_from_path(output_path) {
        Ok(t) => t,
        Err(_) => Tag::new(),
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
                warn!("Cover file not found: {:?} (also checked embedded assets)", cover);
                return Ok(());
            }
        };
        
        if !cover_data.is_empty() {
            // Определяем MIME тип по расширению
            let mime_type = if cover_path.extension().and_then(|s| s.to_str()) == Some("png") {
                "image/png"
            } else if cover_path.extension().and_then(|s| s.to_str()) == Some("jpg") || 
                      cover_path.extension().and_then(|s| s.to_str()) == Some("jpeg") {
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
    
    // Добавляем Chapter Frame для MP3, если есть section_markers
    if !section_markers.is_empty() && output_path.extension().and_then(|s| s.to_str()) == Some("mp3") {
        // Для MP3 используем CHAP фрейм (Chapter Frame)
        // CHAP формат: ID3v2.4 Chapter Frame
        // Структура: chapter_id (terminated string), start_time (u32, milliseconds), end_time (u32, milliseconds), 
        // start_offset (u32, bytes), end_offset (u32, bytes), embedded frame list
        for (index, (title, position_samples)) in section_markers.iter().enumerate() {
            // Конвертируем позицию в сэмплах в миллисекунды
            let position_ms = (*position_samples as f64 / sample_rate as f64 * 1000.0) as u32;
            
            // Определяем end_time для текущей главы (начало следующей или конец файла)
            let end_time = if index + 1 < section_markers.len() {
                let next_position_samples = section_markers[index + 1].1;
                (next_position_samples as f64 / sample_rate as f64 * 1000.0) as u32
            } else {
                // Последняя глава - конец файла (нужно будет вычислить из размера файла)
                // Пока используем большое значение, но лучше бы вычислить из размера файла
                u32::MAX
            };
            
            // Создаем CHAP фрейм
            // Формат CHAP: chapter_id (string, null-terminated), start (u32), end (u32), start_offset (u32), end_offset (u32), subframes
            let chapter_id = format!("ch{:02}", index + 1);
            
            // Создаем данные для CHAP фрейма
            let mut chap_data = Vec::new();
            chap_data.extend_from_slice(chapter_id.as_bytes());
            chap_data.push(0); // null terminator
            chap_data.extend_from_slice(&position_ms.to_be_bytes());
            chap_data.extend_from_slice(&end_time.to_be_bytes());
            chap_data.extend_from_slice(&0u32.to_be_bytes()); // start_offset
            chap_data.extend_from_slice(&0u32.to_be_bytes()); // end_offset
            
            // Добавляем TIT2 (Title) subframe для главы
            let title_bytes = title.as_bytes();
            let title_frame_data = vec![
                0x54, 0x49, 0x54, 0x32, // "TIT2"
                ((title_bytes.len() + 1) >> 8) as u8,
                ((title_bytes.len() + 1) & 0xFF) as u8,
                0x00, 0x00, // flags
                0x03, // encoding: UTF-8
            ];
            let mut full_title_frame = title_frame_data;
            full_title_frame.extend_from_slice(title_bytes);
            full_title_frame.push(0); // null terminator
            
            chap_data.extend_from_slice(&full_title_frame);
            
            // Создаем фрейм CHAP
            // Для MP3 используем TXXX фрейм (User defined text) для хранения информации о главах
            // Формат: "CHAP|chapter_id|position_ms|title"
            let chap_text = format!("CHAP|{}|{}|{}", chapter_id, position_ms, title);
            tag.add_frame(Frame::with_content("TXXX", Content::Text(chap_text)));
        }
    }
    
    // Сохраняем теги
    tag.write_to_path(output_path, id3::Version::Id3v24)
        .with_context(|| format!("Failed to write ID3 tags to: {:?}", output_path))?;
    
    // Для WAV файлов добавляем cue chunks, если есть section_markers
    if !section_markers.is_empty() && output_path.extension().and_then(|s| s.to_str()) == Some("wav") {
        add_wav_cue_chunks(output_path, section_markers, sample_rate, channels)?;
    }
    
    Ok(())
}

/// Добавляет cue chunks в WAV файл
fn add_wav_cue_chunks(
    wav_path: &Path,
    section_markers: &[(String, u64)],
    _sample_rate: u32,
    channels: u16,
) -> Result<()> {
    use std::fs::OpenOptions;
    use std::io::{Seek, SeekFrom, Read, Write};
    
    // Открываем файл для чтения и записи
    let mut file = OpenOptions::new()
        .read(true)
        .write(true)
        .open(wav_path)
        .with_context(|| format!("Failed to open WAV file: {:?}", wav_path))?;
    
    // Читаем весь файл
    let mut wav_data = Vec::new();
    file.read_to_end(&mut wav_data)
        .with_context(|| format!("Failed to read WAV file: {:?}", wav_path))?;
    
    // Ищем позицию после "data" chunk, чтобы вставить cue chunk перед ним
    // WAV формат: RIFF header, fmt chunk, data chunk, и другие chunks
    let mut data_pos = None;
    let mut i = 12; // После RIFF header (12 bytes)
    
    while i + 8 <= wav_data.len() {
        let chunk_id = &wav_data[i..i+4];
        let chunk_size = u32::from_le_bytes([
            wav_data[i+4],
            wav_data[i+5],
            wav_data[i+6],
            wav_data[i+7],
        ]) as usize;
        
        if chunk_id == b"data" {
            data_pos = Some(i);
            break;
        }
        
        i += 8 + chunk_size;
        // Выравнивание на 2 байта
        if chunk_size % 2 != 0 {
            i += 1;
        }
    }
    
    if data_pos.is_none() {
        warn!("Could not find 'data' chunk in WAV file, skipping cue chunks");
        return Ok(());
    }
    
    let data_pos = data_pos.unwrap();
    
    // Получаем размер data chunk
    let _data_chunk_size = u32::from_le_bytes([
        wav_data[data_pos + 4],
        wav_data[data_pos + 5],
        wav_data[data_pos + 6],
        wav_data[data_pos + 7],
    ]);
    
    // Создаем cue chunk
    // Cue chunk format:
    // - "cue " (4 bytes)
    // - chunk_size (4 bytes, little-endian)
    // - num_cue_points (4 bytes, little-endian)
    // - для каждой cue point:
    //   - cue_point_id (4 bytes)
    //   - position (4 bytes, sample offset в сэмплах на канал)
    //   - data_chunk_id (4 bytes, "data")
    //   - chunk_start (4 bytes, позиция начала data chunk)
    //   - block_start (4 bytes, обычно 0)
    //   - sample_offset (4 bytes, позиция в сэмплах на канал)
    
    let num_cue_points = section_markers.len() as u32;
    let mut cue_data = Vec::new();
    cue_data.extend_from_slice(b"cue ");
    
    // Размер chunk'а (пока неизвестен, вычислим позже)
    let cue_size_pos = cue_data.len();
    cue_data.extend_from_slice(&0u32.to_le_bytes());
    
    cue_data.extend_from_slice(&num_cue_points.to_le_bytes());
    
    // Вычисляем размеры chunks для правильного вычисления chunk_start
    // Пока создаем cue points с временным chunk_start, потом обновим
    let mut cue_points_data = Vec::new();
    for (index, (_, position_samples)) in section_markers.iter().enumerate() {
        let cue_point_id = (index + 1) as u32;
        // Позиция должна быть в сэмплах на канал, а не в общем количестве сэмплов
        let position_per_channel = (*position_samples / channels as u64) as u32;
        
        cue_points_data.push((cue_point_id, position_per_channel));
    }
    
    // Добавляем cue points (chunk_start будет обновлен позже)
    for (cue_point_id, position_per_channel) in &cue_points_data {
        cue_data.extend_from_slice(&cue_point_id.to_le_bytes());
        cue_data.extend_from_slice(&position_per_channel.to_le_bytes());
        cue_data.extend_from_slice(b"data");
        cue_data.extend_from_slice(&0u32.to_le_bytes()); // chunk_start - будет обновлен позже
        cue_data.extend_from_slice(&0u32.to_le_bytes()); // block_start - обычно 0
        cue_data.extend_from_slice(&position_per_channel.to_le_bytes()); // sample_offset - позиция в сэмплах на канал
    }
    
    // Обновляем размер chunk'а
    let cue_size = (cue_data.len() - 8) as u32; // минус "cue " и размер
    // Размер chunk'а должен быть четным
    let cue_size_aligned = if cue_size % 2 == 0 { cue_size } else { cue_size + 1 };
    let size_bytes = cue_size_aligned.to_le_bytes();
    cue_data[cue_size_pos] = size_bytes[0];
    cue_data[cue_size_pos + 1] = size_bytes[1];
    cue_data[cue_size_pos + 2] = size_bytes[2];
    cue_data[cue_size_pos + 3] = size_bytes[3];
    
    // Если размер нечетный, добавляем padding байт
    if cue_size % 2 != 0 {
        cue_data.push(0);
    }
    
    // Вычисляем новую позицию data chunk после вставки chunks
    // Размер данных до data chunk + размер cue chunk + размер LIST chunk (пока неизвестен, но вычислим позже)
    // Пока используем приблизительное значение, потом обновим после создания LIST chunk
    let _approx_list_size = 100; // приблизительный размер LIST chunk
    let _new_data_start_approx = data_pos + cue_data.len() + _approx_list_size + 8;
    
    // Создаем LIST chunk с adtl subchunk для меток (labels)
    // LIST chunk format:
    // - "LIST" (4 bytes)
    // - chunk_size (4 bytes)
    // - "adtl" (4 bytes)
    // - для каждой метки:
    //   - "labl" (4 bytes)
    //   - size (4 bytes)
    //   - cue_point_id (4 bytes)
    //   - text (null-terminated string)
    
    let mut list_data = Vec::new();
    list_data.extend_from_slice(b"LIST");
    
    let list_size_pos = list_data.len();
    list_data.extend_from_slice(&0u32.to_le_bytes());
    
    list_data.extend_from_slice(b"adtl");
    
    for (index, (title, _)) in section_markers.iter().enumerate() {
        let cue_point_id = (index + 1) as u32;
        
        // labl subchunk
        list_data.extend_from_slice(b"labl");
        
        // Используем UTF-8 напрямую
        let title_bytes = title.as_bytes();
        
        let labl_size = (4 + title_bytes.len() + 1) as u32; // cue_point_id + text + null terminator
        list_data.extend_from_slice(&labl_size.to_le_bytes());
        list_data.extend_from_slice(&cue_point_id.to_le_bytes());
        list_data.extend_from_slice(title_bytes);
        list_data.push(0); // null terminator
    }
    
    // Обновляем размер LIST chunk'а
    let list_size = (list_data.len() - 8) as u32; // минус "LIST" и размер
    // Размер chunk'а должен быть четным
    let list_size_aligned = if list_size % 2 == 0 { list_size } else { list_size + 1 };
    let list_size_bytes = list_size_aligned.to_le_bytes();
    list_data[list_size_pos] = list_size_bytes[0];
    list_data[list_size_pos + 1] = list_size_bytes[1];
    list_data[list_size_pos + 2] = list_size_bytes[2];
    list_data[list_size_pos + 3] = list_size_bytes[3];
    
    // Если размер нечетный, добавляем padding байт
    if list_size % 2 != 0 {
        list_data.push(0);
    }
    
    // Вычисляем новую позицию data chunk после вставки chunks
    // data_pos - позиция начала "data" chunk в исходном файле (байтовое смещение от начала файла)
    // После вставки cue и LIST chunks перед data chunk, data chunk сдвинется на размер этих chunks
    let inserted_chunks_size = cue_data.len() + list_data.len();
    // chunk_start должен указывать на байтовое смещение от начала файла до начала "data" chunk header (не данных!)
    // После вставки chunks, data chunk будет на позиции data_pos + inserted_chunks_size
    let new_data_start = data_pos + inserted_chunks_size;
    
    // Обновляем chunk_start в cue points в cue_data
    // chunk_start находится на позиции: "cue " (4) + size (4) + num_points (4) + для каждой точки: id (4) + position (4) + "data" (4) = 20 байт от начала cue point
    let cue_point_size = 24; // размер одной cue point (id + position + "data" + chunk_start + block_start + sample_offset)
    for (i, _) in cue_points_data.iter().enumerate() {
        let chunk_start_offset = 8 + 4 + (i * cue_point_size) + 4 + 4 + 4; // "cue " + size + num_points + id + position + "data"
        let chunk_start_bytes = (new_data_start as u32).to_le_bytes();
        cue_data[chunk_start_offset] = chunk_start_bytes[0];
        cue_data[chunk_start_offset + 1] = chunk_start_bytes[1];
        cue_data[chunk_start_offset + 2] = chunk_start_bytes[2];
        cue_data[chunk_start_offset + 3] = chunk_start_bytes[3];
    }
    
    // Вставляем cue и LIST chunks перед data chunk
    let mut new_wav_data = Vec::new();
    new_wav_data.extend_from_slice(&wav_data[..data_pos]);
    new_wav_data.extend_from_slice(&cue_data);
    new_wav_data.extend_from_slice(&list_data);
    new_wav_data.extend_from_slice(&wav_data[data_pos..]);
    
    // Обновляем размер RIFF chunk
    let riff_size = (new_wav_data.len() - 8) as u32; // минус "RIFF" и размер
    let riff_size_bytes = riff_size.to_le_bytes();
    new_wav_data[4] = riff_size_bytes[0];
    new_wav_data[5] = riff_size_bytes[1];
    new_wav_data[6] = riff_size_bytes[2];
    new_wav_data[7] = riff_size_bytes[3];
    
    // Записываем обновленный файл
    file.seek(SeekFrom::Start(0))?;
    file.write_all(&new_wav_data)
        .with_context(|| format!("Failed to write updated WAV file: {:?}", wav_path))?;
    file.set_len(new_wav_data.len() as u64)?;
    
    Ok(())
}

