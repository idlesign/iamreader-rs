use crate::project::project::{ProjectFile, Meta, MarkerSettings, ProcessMarkerAssetContext, Settings};
use crate::project::markers::process_marker_asset;
use crate::project::metadata::write_audio_tags;
use crate::audio::processing::{
    read_audio_file_to_samples, mix_audio, write_samples_to_wav, resample_and_convert_channels,
    stream_merge_wav_segments_with_crossfade, compute_normalize_gain_from_wav,
    apply_normalize_wav_to_wav, encode_wav_to_mp3,
};
use crate::audio::denoise;
use crate::ui::ui::UIState;
use std::path::{Path, PathBuf};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;
use anyhow::{Result, Context};
use hound;
use log::{info, warn, debug};

/// Индексы стадий: 0 prepare, 1 denoise, 2 process markers, 3 normalize/write
fn set_compile_ui(
    ui_state: Option<&Arc<Mutex<UIState>>>,
    progress: f32,
    stage_index: u32,
    stage_detail: &str,
    start: &Instant,
    initial_eta_secs: f32,
) {
    if let Some(ref ui_state) = ui_state {
        if let Ok(mut state) = ui_state.lock() {
            state.compile_progress = progress;
            state.compile_stage = stage_detail.to_string();
            state.dialog_compile_eta_secs = if progress > 0.01 {
                let elapsed = start.elapsed().as_secs_f32();
                let total = elapsed / progress;
                (total - elapsed).max(0.0)
            } else {
                initial_eta_secs
            };
            let stage_names = ["prepare", "denoise", "process markers", "normalize; write"];
            let mut lines: Vec<String> = Vec::new();
            for (i, name) in stage_names.iter().enumerate() {
                let i = i as u32;
                let (prefix, suffix) = if i < stage_index {
                    ("✓", "")
                } else if i == stage_index {
                    ("→", stage_detail)
                } else {
                    (" ", "")
                };
                let line = if suffix.is_empty() {
                    format!("{}. {} {}", i + 1, name, prefix)
                } else {
                    format!("{}. {} {} {}", i + 1, name, prefix, suffix)
                };
                lines.push(line);
            }
            state.dialog_compile_stages_text = lines.join("\n");
        }
    }
}

/// Диапазоны индексов файлов по секциям выхода. При section_split каждая секция — отдельный выходной файл.
fn section_ranges(
    files: &[&ProjectFile],
    markers: &HashMap<String, MarkerSettings>,
    section_split: bool,
) -> Vec<(usize, usize)> {
    if files.is_empty() {
        return Vec::new();
    }
    if !section_split {
        return vec![(0, files.len())];
    }
    let mut ranges = Vec::new();
    let mut start = 0;
    for (i, file) in files.iter().enumerate() {
        let starts_section = file.markers.iter().any(|m| {
            markers.get(m).map(|s| s.section).unwrap_or(false)
        });
        if starts_section && i > start {
            ranges.push((start, i));
            start = i;
        }
    }
    ranges.push((start, files.len()));
    ranges
}

/// Компилирует WAV файлы в финальный аудиофайл (этап 1 — denoise голоса, этап 2 — микс в temp, этап 3 — нормализация и запись).
/// При cancel == Some(Arc) проверяет флаг в циклах и выходит с ошибкой при установке (временные файлы не удаляются).
pub fn compile_wav_files_static(
    files: Vec<ProjectFile>,
    project_path: PathBuf,
    markers: HashMap<String, MarkerSettings>,
    meta: Meta,
    settings: Settings,
    ui_state: Option<Arc<Mutex<UIState>>>,
    cancel: Option<Arc<AtomicBool>>,
    _debug: bool,
) -> Result<()> {
    let check_cancel = |c: &Option<Arc<AtomicBool>>| -> Result<()> {
        if c.as_ref().map(|a| a.load(Ordering::Relaxed)).unwrap_or(false) {
            anyhow::bail!("Compilation cancelled");
        }
        Ok(())
    };
    let format = settings.format_audio.as_str();
    info!("Starting compilation of {} files", format);

    let files_to_compile: Vec<&ProjectFile> = files
        .iter()
        .filter(|f| Path::new(&f.path).exists())
        .collect();

    if files_to_compile.is_empty() {
        warn!("No files to compile");
        return Ok(());
    }

    let total_files = files_to_compile.len();
    let output_dir = project_path.parent().unwrap_or(Path::new("."));
    let sound_dir = output_dir;
    let tmp_dir = output_dir.join("chunks").join("tmp");

    let first_reader = hound::WavReader::open(Path::new(&files_to_compile[0].path))
        .with_context(|| format!("Failed to open first file: {:?}", files_to_compile[0].path))?;
    let spec = first_reader.spec();

    for file in &files_to_compile[1..] {
        let path = Path::new(&file.path);
        let reader = hound::WavReader::open(path)
            .with_context(|| format!("Failed to open file: {:?}", path))?;
        let file_spec = reader.spec();
        if file_spec.channels != spec.channels
            || file_spec.sample_rate != spec.sample_rate
            || file_spec.bits_per_sample != spec.bits_per_sample
            || file_spec.sample_format != spec.sample_format
        {
            warn!("File {:?} has different format, may cause issues", path);
        }
    }

    let output_sample_rate = if format == "mp3" { 44100 } else { spec.sample_rate };
    let output_channels = if format == "mp3" { 2 } else { spec.channels };
    let file_extension = if format == "mp3" { "mp3" } else { "wav" };
    let crossfade_samples = (output_sample_rate as f32 * 20.0 / 1000.0) as usize * output_channels as usize;

    let total_duration_ms: u64 = files_to_compile.iter().map(|f| f.duration_ms).sum();
    let initial_eta_secs = (total_duration_ms as f32 / 1000.0) * 1.5;
    let start = Instant::now();
    if let Some(ref ui_state) = ui_state {
        if let Ok(mut state) = ui_state.lock() {
            state.is_compiling = true;
            state.compile_finished_at = None;
            state.compile_progress = 0.0;
            state.compile_stage = "prepare".to_string();
            state.dialog_compile_eta_secs = initial_eta_secs;
            state.dialog_compile_stages_text = "1. prepare →\n2. denoise\n3. process markers\n4. normalize; write".to_string();
        }
    }
    check_cancel(&cancel)?;

    let _ = std::fs::remove_dir_all(&tmp_dir);
    std::fs::create_dir_all(&tmp_dir)
        .with_context(|| format!("Failed to create tmp dir: {:?}", tmp_dir))?;

    let sections = section_ranges(&files_to_compile, &markers, settings.section_split);
    const STAGE_1: f32 = 1.0 / 3.0;
    const STAGE_2: f32 = 1.0 / 3.0;
    const STAGE_3: f32 = 1.0 / 3.0;

    // Этап 1: denoise каждой записи, запись в denoised_XXXXX.wav.
    // IAMREADER_DENOISE_PER_FILE=1 — одна сессия на файл (для проверки гипотезы о состоянии модели).
    let denoise_per_file = std::env::var("IAMREADER_DENOISE_PER_FILE").ok().as_deref() == Some("1");
    let mut denoise_session = if settings.denoise && !denoise_per_file {
        match denoise::create_denoise_session() {
            Ok(s) => {
                info!("Denoise model loaded (once for all {} files)", total_files);
                Some(s)
            }
            Err(e) => {
                warn!("Denoise skipped (model not found): {}", e);
                None
            }
        }
    } else {
        if settings.denoise && denoise_per_file {
            info!("Denoise: one session per file (IAMREADER_DENOISE_PER_FILE=1)");
        }
        None
    };
    for (index, file) in files_to_compile.iter().enumerate() {
        let path = Path::new(&file.path);
        let voice = read_audio_file_to_samples(path, spec.sample_rate, spec.channels)
            .with_context(|| format!("Failed to read voice: {:?}", path))?;
        let denoised = if settings.denoise {
            if let Some(ref mut session) = denoise_session {
                denoise::apply_denoise_with_session(session, &voice, spec.sample_rate, spec.channels)?
            } else if denoise_per_file {
                denoise::apply_denoise(&voice, spec.sample_rate, spec.channels)?
            } else {
                voice.clone()
            }
        } else {
            voice.clone()
        };
        let voice_out = if spec.sample_rate != output_sample_rate || spec.channels != output_channels {
            resample_and_convert_channels(&denoised, spec.sample_rate, spec.channels, output_sample_rate, output_channels)?
        } else {
            denoised
        };
        let denoised_path = tmp_dir.join(format!("denoised_{:05}.wav", index + 1));
        write_samples_to_wav(&voice_out, &denoised_path, output_sample_rate, output_channels)?;
        if let Ok(keep_dir) = std::env::var("IAMREADER_KEEP_DENOISED") {
            let keep_path = Path::new(&keep_dir);
            let _ = std::fs::create_dir_all(keep_path);
            if let Some(name) = denoised_path.file_name() {
                let _ = std::fs::copy(&denoised_path, keep_path.join(name));
            }
        }
        set_compile_ui(
            ui_state.as_ref(),
            (index + 1) as f32 / total_files as f32 * STAGE_1,
            1,
            &format!("{}/{}", index + 1, total_files),
            &start,
            initial_eta_secs,
        );
        info!("Stage 1: {}/{}", index + 1, total_files);
        check_cancel(&cancel)?;
    }

    // Этап 2: микс (звуки/подложки) → chunks/tmp/mixed_{sec}_{idx}.wav, затем удаление denoised секции
    let mut files_done = 0;
    for (sec_idx, &(s, e)) in sections.iter().enumerate() {
        for (local_idx, i) in (s..e).enumerate() {
            let file = files_to_compile[i];
            let denoised_path = tmp_dir.join(format!("denoised_{:05}.wav", i + 1));
            let voice_path = if denoised_path.exists() {
                denoised_path
            } else {
                PathBuf::from(&file.path)
            };
            let file_samples = process_file_for_compilation(file, &markers, sound_dir, &voice_path, output_sample_rate, output_channels)?;
            let mixed_path = tmp_dir.join(format!("mixed_{}_{}.wav", sec_idx, local_idx));
            write_samples_to_wav(&file_samples, &mixed_path, output_sample_rate, output_channels)?;
            files_done += 1;
            set_compile_ui(
                ui_state.as_ref(),
                STAGE_1 + (files_done as f32 / total_files as f32) * STAGE_2,
                2,
                &format!("{}/{}", files_done, total_files),
                &start,
                initial_eta_secs,
            );
            check_cancel(&cancel)?;
        }
        for i in s..e {
            let p = tmp_dir.join(format!("denoised_{:05}.wav", i + 1));
            let _ = std::fs::remove_file(&p);
        }
    }

    // Этап 3: чтение mixed_*.wav, кроссфейд, нормализация, запись финала, удаление mixed
    let total_sections = sections.len();
    for (sec_idx, &(s, e)) in sections.iter().enumerate() {
        check_cancel(&cancel)?;
        let stage_detail = format!("section {}/{}", sec_idx + 1, total_sections);
        set_compile_ui(
            ui_state.as_ref(),
            STAGE_1 + STAGE_2 + (sec_idx as f32 / total_sections.max(1) as f32) * STAGE_3,
            3,
            &stage_detail,
            &start,
            initial_eta_secs,
        );
        let file_count = e - s;
        let mut mixed_paths: Vec<PathBuf> = (0..file_count)
            .map(|local_idx| tmp_dir.join(format!("mixed_{}_{}.wav", sec_idx, local_idx)))
            .collect();
        mixed_paths.retain(|p| p.exists());
        if mixed_paths.is_empty() {
            continue;
        }
        let total_sections_f = total_sections.max(1) as f32;
        set_compile_ui(
            ui_state.as_ref(),
            STAGE_1 + STAGE_2 + (sec_idx as f32 + 0.25) / total_sections_f * STAGE_3,
            3,
            &format!("section {}/{} merge", sec_idx + 1, total_sections),
            &start,
            initial_eta_secs,
        );
        let temp_combined = tmp_dir.join(format!("combined_{}.wav", sec_idx));
        let segment_lengths = stream_merge_wav_segments_with_crossfade(
            &mixed_paths,
            crossfade_samples,
            &temp_combined,
            output_sample_rate,
            output_channels,
        )?;
        set_compile_ui(
            ui_state.as_ref(),
            STAGE_1 + STAGE_2 + (sec_idx as f32 + 0.6) / total_sections_f * STAGE_3,
            3,
            &format!("section {}/{} normalize; write", sec_idx + 1, total_sections),
            &start,
            initial_eta_secs,
        );
        let section_files: Vec<&ProjectFile> = files_to_compile[s..e].to_vec();
        let mut section_markers: Vec<(String, u64)> = Vec::new();
        if !settings.section_split {
            let mut pos: u64 = 0;
            for (idx, &len) in segment_lengths.iter().enumerate() {
                if let Some(file) = section_files.get(idx) {
                    for marker_name in &file.markers {
                        if let Some(ms) = markers.get(marker_name) {
                            if ms.section && !ms.title.is_empty() {
                                section_markers.push((ms.title.clone(), pos));
                            }
                        }
                    }
                }
                pos += len as u64;
            }
        }
        let output_path = output_dir.join(format!("{:05}.{}", sec_idx + 1, file_extension));
        save_compiled_file_from_temp(
            &temp_combined,
            &output_path,
            format,
            output_sample_rate,
            output_channels,
            settings.normalize,
            &meta,
            &settings,
            &section_files[..],
            &markers,
            output_dir,
            &section_markers,
        )?;
        let _ = std::fs::remove_file(&temp_combined);
        for p in &mixed_paths {
            let _ = std::fs::remove_file(p);
        }
        set_compile_ui(
            ui_state.as_ref(),
            STAGE_1 + STAGE_2 + (sec_idx + 1) as f32 / total_sections.max(1) as f32 * STAGE_3,
            3,
            &format!("section {}/{} done", sec_idx + 1, total_sections),
            &start,
            initial_eta_secs,
        );
        info!("Stage 3: saved {:?}", output_path);
    }

    info!("Compilation completed: {} output files", sections.len());
    if let Some(ref ui_state) = ui_state {
        if let Ok(mut state) = ui_state.lock() {
            state.compile_progress = 1.0;
            state.compile_stage = "Done".to_string();
            state.dialog_compile_stages_text = "1. prepare ✓\n2. denoise ✓\n3. process markers ✓\n4. normalize; write ✓".to_string();
            state.dialog_compile_eta_secs = -1.0;
            state.compile_finished_at = Some(Instant::now());
        }
    }
    Ok(())
}

/// Обрабатывает файл для компиляции: читает голос из voice_path (уже в целевом формате), добавляет звуки и подложки по маркерам.
pub fn process_file_for_compilation(
    file: &ProjectFile,
    markers: &HashMap<String, MarkerSettings>,
    sound_dir: &Path,
    voice_path: &Path,
    output_sample_rate: u32,
    output_channels: u16,
) -> Result<Vec<f32>> {
    let mut main_samples = read_audio_file_to_samples(voice_path, output_sample_rate, output_channels)
        .with_context(|| format!("Failed to read audio file: {:?}", voice_path))?;
    
    // Обрабатываем маркеры
    let mut sounds_before = Vec::new();
    let mut sounds_after = Vec::new();
    let mut underlays_begin: Vec<(Vec<f32>, f32, Option<i32>)> = Vec::new();
    let mut underlays_end: Vec<(Vec<f32>, f32, Option<i32>)> = Vec::new();
    
    for marker in &file.markers {
        if let Some(marker_settings) = markers.get(marker) {
            // Обрабатываем begin asset
            let mut begin_ctx = ProcessMarkerAssetContext {
                sound_dir: &sound_dir,
                sample_rate: output_sample_rate,
                channels: output_channels,
                sounds: &mut sounds_before,
                underlays: &mut underlays_begin,
                add_duration_samples: None,
            };
            process_marker_asset(&marker_settings.assets.begin, &mut begin_ctx)?;
            // Обрабатываем end asset
            let mut end_ctx = ProcessMarkerAssetContext {
                sound_dir: &sound_dir,
                sample_rate: output_sample_rate,
                channels: output_channels,
                sounds: &mut sounds_after,
                underlays: &mut underlays_end,
                add_duration_samples: None,
            };
            process_marker_asset(&marker_settings.assets.end, &mut end_ctx)?;
        }
    }
    
    for (underlay, volume, repeat) in &underlays_begin {
        let repeat_count = repeat.unwrap_or(1);
        if repeat_count == -1 {
            let main_len = main_samples.len();
            let overlay_len = underlay.len();
            if overlay_len > 0 {
                for i in 0..main_len {
                    let overlay_pos = i % overlay_len;
                    main_samples[i] = (main_samples[i] + underlay[overlay_pos] * volume).max(-1.0).min(1.0);
                }
            }
        } else if repeat_count > 0 {
            for _ in 0..repeat_count {
                main_samples = mix_audio(&main_samples, underlay, *volume, 0);
            }
        } else {
            if repeat_count < -1 {
                warn!("Invalid repeat value {} for underlay, ignoring", repeat_count);
            }
        }
    }
    
    for (underlay, volume, repeat) in &underlays_end {
        let repeat_count = repeat.unwrap_or(1);
        let overlay_len = underlay.len();
        let main_len = main_samples.len();
        
        if repeat_count == -1 {
            if overlay_len > 0 {
                for i in 0..main_len {
                    let overlay_pos = i % overlay_len;
                    main_samples[i] = (main_samples[i] + underlay[overlay_pos] * volume).max(-1.0).min(1.0);
                }
            }
        } else if repeat_count > 0 {
            for _ in 0..repeat_count {
                if overlay_len <= main_len {
                    let start_offset = main_len - overlay_len;
                    main_samples = mix_audio(&main_samples, underlay, *volume, start_offset);
                } else {
                    warn!("Underlay sound is longer than main file, starting from beginning");
                    main_samples = mix_audio(&main_samples, underlay, *volume, 0);
                }
            }
        } else {
            if repeat_count < -1 {
                warn!("Invalid repeat value {} for underlay, ignoring", repeat_count);
            }
        }
    }
    
    // Собираем финальную последовательность: звуки перед + основной + звуки после
    let mut final_samples = Vec::new();
    
    // Добавляем звуки перед
    for sound in &sounds_before {
        final_samples.extend_from_slice(sound);
    }
    
    // Добавляем основной файл
    final_samples.extend_from_slice(&main_samples);
    
    // Добавляем звуки после
    for sound in &sounds_after {
        final_samples.extend_from_slice(sound);
    }
    
    // Убеждаемся, что количество сэмплов кратно количеству каналов
    let channels = output_channels as usize;
    let remainder = final_samples.len() % channels;
    if remainder != 0 {
        debug!("Adjusting sample count: {} -> {} (removing {} samples to make it divisible by {} channels)", 
            final_samples.len(), final_samples.len() - remainder, remainder, channels);
        final_samples.truncate(final_samples.len() - remainder);
    }
    
    Ok(final_samples)
}

/// Сохраняет скомпилированный файл из временного WAV (потоковая нормализация: два прохода без загрузки всего файла в память).
pub fn save_compiled_file_from_temp(
    temp_wav_path: &Path,
    output_path: &Path,
    format: &str,
    sample_rate: u32,
    channels: u16,
    normalize: bool,
    meta: &Meta,
    settings: &Settings,
    files: &[&ProjectFile],
    markers: &HashMap<String, MarkerSettings>,
    project_dir: &Path,
    section_markers: &[(String, u64)],
) -> Result<()> {
    if format == "mp3" {
        if normalize {
            let gain = compute_normalize_gain_from_wav(temp_wav_path, channels)?
                .unwrap_or(1.0);
            let temp_norm = project_dir.join("chunks").join("tmp").join("normalized_output.wav");
            apply_normalize_wav_to_wav(temp_wav_path, &temp_norm, gain, sample_rate, channels)?;
            encode_wav_to_mp3(&temp_norm, output_path, sample_rate, channels)?;
            let _ = std::fs::remove_file(&temp_norm);
        } else {
            encode_wav_to_mp3(temp_wav_path, output_path, sample_rate, channels)?;
        }
    } else {
        if normalize {
            let gain = compute_normalize_gain_from_wav(temp_wav_path, channels)?
                .unwrap_or(1.0);
            apply_normalize_wav_to_wav(temp_wav_path, output_path, gain, sample_rate, channels)?;
        } else {
            std::fs::copy(temp_wav_path, output_path)
                .with_context(|| format!("Failed to copy temp to output: {:?}", output_path))?;
        }
    }
    write_audio_tags(output_path, meta, &settings.cover, files, markers, project_dir, section_markers, sample_rate, channels)?;
    Ok(())
}

