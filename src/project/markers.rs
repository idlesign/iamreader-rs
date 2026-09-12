use crate::audio::processing::{read_audio_file_to_samples, read_audio_from_bytes};
use crate::project::project::{
    MarkerAsset, MarkerSettings, ProcessMarkerAssetContext, ProjectFile,
};
use crate::utils::assets;
use anyhow::{Context, Result};
use log::{debug, warn};
use std::collections::HashMap;
use std::path::Path;

/// Нормализует маркеры (сортирует) файла
pub fn normalize_markers(file: &mut ProjectFile) {
    file.markers.sort();
}

/// Обрабатывает маркер asset (звук для маркера)
pub fn process_marker_asset(
    asset: &MarkerAsset,
    ctx: &mut ProcessMarkerAssetContext,
) -> Result<()> {
    debug!(
        "[process_marker_asset] Processing audio: {}, kind: {}",
        asset.audio, asset.kind
    );
    if asset.audio.is_empty() {
        debug!("[process_marker_asset] Audio is empty, skipping");
        return Ok(());
    }

    // Загрузка: sound_dir → текущая директория → встроенные (assets). Один путь для компиляции и для расчёта длительности (add_duration_samples).
    let sound_path = ctx.sound_dir.join(&asset.audio);
    let sound_samples = if sound_path.exists() {
        debug!("Reading sound file from sound_dir: {:?}", sound_path);
        read_audio_file_to_samples(&sound_path, ctx.sample_rate, ctx.channels)
            .with_context(|| format!("Failed to read sound file: {:?}", sound_path))?
    } else {
        // Если файла нет в sound_dir, проверяем текущую директорию
        let current_dir_path = Path::new(&asset.audio);
        if current_dir_path.exists() {
            debug!(
                "Reading sound file from current directory: {:?}",
                current_dir_path
            );
            read_audio_file_to_samples(current_dir_path, ctx.sample_rate, ctx.channels)
                .with_context(|| {
                    format!(
                        "Failed to read sound file from current directory: {:?}",
                        current_dir_path
                    )
                })?
        } else {
            // Встроенный звук (assets): учитывается и при компиляции, и при расчёте времени начала в списке записей
            debug!("Trying to load embedded sound file: {}", asset.audio);
            if let Some(data) = assets::get_asset_file(&asset.audio)? {
                debug!(
                    "Using embedded sound file: {} ({} bytes)",
                    asset.audio,
                    data.len()
                );
                let samples =
                    read_audio_from_bytes(&data, &asset.audio, ctx.sample_rate, ctx.channels)
                        .with_context(|| {
                            format!("Failed to read embedded sound file: {}", asset.audio)
                        })?;
                debug!(
                    "Loaded embedded sound file {}: {} samples",
                    asset.audio,
                    samples.len()
                );
                samples
            } else {
                warn!("Sound file not found: {:?} (checked sound_dir, current dir, and embedded assets)", sound_path);
                return Ok(());
            }
        }
    };

    debug!(
        "[process_marker_asset] Loaded sound file {}: {} samples, kind={}, repeat={:?}",
        asset.audio,
        sound_samples.len(),
        asset.kind,
        asset.repeat
    );

    if asset.kind == "add" {
        let repeat_count = asset.repeat.unwrap_or(1);
        let add_samples = if repeat_count > 0 {
            sound_samples.len() * (repeat_count as usize)
        } else {
            0
        };
        if let Some(ref mut out) = ctx.add_duration_samples {
            **out += add_samples as u64;
            return Ok(());
        }
        debug!(
            "[process_marker_asset] Adding sound: {}, repeat: {}, samples: {}",
            asset.audio,
            repeat_count,
            sound_samples.len()
        );
        if repeat_count > 0 {
            for _ in 0..repeat_count {
                ctx.sounds.push(sound_samples.clone());
            }
        } else if repeat_count == 0 {
            debug!(
                "[process_marker_asset] Repeat count is 0, skipping sound: {}",
                asset.audio
            );
        } else {
            warn!("Negative repeat value not supported for kind=add, using 1");
            ctx.sounds.push(sound_samples);
        }
    } else if asset.kind == "underlay" || asset.kind == "undelay" {
        let reduction_percent = asset.reduction.unwrap_or(75);
        let reduction_volume = (100.0 - reduction_percent as f32) / 100.0;
        ctx.underlays
            .push((sound_samples, reduction_volume, asset.repeat));
    }
    Ok(())
}

/// Вычисляет длительность звуков "add" (в начале и в конце) для одной записи в миллисекундах.
/// Использует ту же логику загрузки, что и process_marker_asset (без дублирования).
pub fn compute_file_add_durations_ms(
    file: &ProjectFile,
    markers: &HashMap<String, MarkerSettings>,
    sound_dir: &Path,
    sample_rate: u32,
    channels: u16,
) -> Result<(u64, u64)> {
    let mut begin_samples: u64 = 0;
    let mut end_samples: u64 = 0;
    for marker_name in &file.markers {
        if let Some(settings) = markers.get(marker_name) {
            let mut begin_ctx = ProcessMarkerAssetContext {
                sound_dir,
                sample_rate,
                channels,
                sounds: &mut Vec::new(),
                underlays: &mut Vec::new(),
                add_duration_samples: Some(&mut begin_samples),
            };
            process_marker_asset(&settings.assets.begin, &mut begin_ctx)?;
            let mut end_ctx = ProcessMarkerAssetContext {
                sound_dir,
                sample_rate,
                channels,
                sounds: &mut Vec::new(),
                underlays: &mut Vec::new(),
                add_duration_samples: Some(&mut end_samples),
            };
            process_marker_asset(&settings.assets.end, &mut end_ctx)?;
        }
    }
    let sr = sample_rate as u64;
    let ch = channels as u64;
    let begin_ms = begin_samples * 1000 / (sr * ch);
    let end_ms = end_samples * 1000 / (sr * ch);
    Ok((begin_ms, end_ms))
}

/// Эффективная длительность записи в мс (длительность файла + add-звуки маркеров в начале и конце).
pub fn effective_duration_ms(
    file: &ProjectFile,
    markers: &HashMap<String, MarkerSettings>,
    sound_dir: &Path,
    sample_rate: u32,
    channels: u16,
) -> Result<u64> {
    let (begin_ms, end_ms) =
        compute_file_add_durations_ms(file, markers, sound_dir, sample_rate, channels)?;
    Ok(file.duration_ms + begin_ms + end_ms)
}

/// Эффективные длительности для списка записей (для расчёта времени начала каждой).
pub fn compute_effective_durations_ms(
    files: &[ProjectFile],
    markers: &HashMap<String, MarkerSettings>,
    sound_dir: &Path,
    sample_rate: u32,
    channels: u16,
) -> Result<Vec<u64>> {
    let mut out = Vec::with_capacity(files.len());
    for file in files {
        out.push(effective_duration_ms(
            file,
            markers,
            sound_dir,
            sample_rate,
            channels,
        )?);
    }
    Ok(out)
}
