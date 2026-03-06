use std::path::{Path, PathBuf};
use anyhow::{Result, Context};
use std::fs;

/// Получает путь к файлу кеша для заданного WAV файла
pub fn get_cache_path(wav_path: &Path) -> PathBuf {
    let wav_file_name = wav_path
        .file_name()
        .and_then(|n| n.to_str())
        .map(|s| s.to_string())
        .unwrap_or_else(|| "unknown.wav".to_string());
    
    // Заменяем расширение на .wc
    let cache_file_name = if wav_file_name.ends_with(".wav") {
        wav_file_name.replace(".wav", ".wc")
    } else {
        format!("{}.wc", wav_file_name)
    };
    
    // Получаем родительскую директорию (chunks) и создаем путь к __cache__
    let parent = wav_path.parent().unwrap_or(Path::new("."));
    parent.join("__cache__").join(cache_file_name)
}

/// Загружает волновой график из кеша
pub fn load_waveform_cache(cache_path: &Path) -> Option<Vec<f32>> {
    if !cache_path.exists() {
        return None;
    }
    
    match fs::read(cache_path) {
        Ok(bytes) => {
            // Проверяем, что размер данных кратен размеру f32 (4 байта)
            if bytes.len() % 4 != 0 {
                log::warn!("Invalid cache file size: {} bytes (not divisible by 4)", bytes.len());
                return None;
            }
            
            // Преобразуем байты в Vec<f32>
            let samples: Vec<f32> = bytes
                .chunks_exact(4)
                .map(|chunk| {
                    let bytes_array: [u8; 4] = chunk.try_into().unwrap();
                    f32::from_le_bytes(bytes_array)
                })
                .collect();
            
            if samples.is_empty() {
                None
            } else {
                Some(samples)
            }
        }
        Err(e) => {
            log::warn!("Failed to read cache file {:?}: {}", cache_path, e);
            None
        }
    }
}

/// Сохраняет волновой график в кеш
pub fn save_waveform_cache(cache_path: &Path, data: &[f32]) -> Result<()> {
    // Создаем директорию кеша, если её нет
    if let Some(parent) = cache_path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("Failed to create cache directory: {:?}", parent))?;
    }
    
    // Преобразуем Vec<f32> в байты (little-endian)
    let bytes: Vec<u8> = data
        .iter()
        .flat_map(|&sample| sample.to_le_bytes().to_vec())
        .collect();
    
    // Записываем в файл
    fs::write(cache_path, bytes)
        .with_context(|| format!("Failed to write cache file: {:?}", cache_path))?;
    
    Ok(())
}

/// Удаляет файл кеша для заданного WAV файла
pub fn remove_waveform_cache(wav_path: &Path) -> Result<()> {
    let cache_path = get_cache_path(wav_path);
    if cache_path.exists() {
        fs::remove_file(&cache_path)
            .with_context(|| format!("Failed to remove cache file: {:?}", cache_path))?;
    }
    Ok(())
}

/// Читает waveform samples из файла
pub fn read_waveform_samples(path: &Path, max_samples: usize, debug: bool) -> Result<Vec<f32>> {
    // Сначала пробуем загрузить из кеша
    let cache_path = get_cache_path(path);
    if let Some(cached_samples) = load_waveform_cache(&cache_path) {
        if debug {
            log::debug!("Loaded waveform from cache: {:?} ({} samples)", cache_path, cached_samples.len());
        }
        return Ok(cached_samples);
    }
    
    // Если кеша нет, вычисляем волновой график
    let reader = hound::WavReader::open(path)?;
    let spec = reader.spec();
    let total_samples = reader.len() as usize / spec.channels as usize;
    
    if total_samples == 0 {
        return Ok(Vec::new());
    }
    
    let window_size = (total_samples / max_samples).max(1);
    let mut samples = Vec::new();
    
    // Читаем все сэмплы в зависимости от формата файла
    // Для стерео берём только левый канал
    match spec.sample_format {
        hound::SampleFormat::Float => {
            let all_samples: Result<Vec<f32>, _> = reader.into_samples::<f32>().collect();
            let all_samples = all_samples?;
            
            // Обрабатываем окнами: для каждого окна берём максимум по модулю
            for window_start in (0..total_samples).step_by(window_size) {
                let window_end = (window_start + window_size).min(total_samples);
                let mut max_amplitude = 0.0f32;
                
                for i in window_start..window_end {
                    let sample_idx = i * spec.channels as usize;
                    if sample_idx < all_samples.len() {
                        let sample = all_samples[sample_idx];
                        let abs_sample = sample.abs();
                        if abs_sample > max_amplitude {
                            max_amplitude = abs_sample;
                        }
                    }
                }
                
                samples.push(max_amplitude);
                if samples.len() >= max_samples {
                    break;
                }
            }
        }
        hound::SampleFormat::Int => {
            let all_samples: Result<Vec<i16>, _> = reader.into_samples::<i16>().collect();
            let all_samples = all_samples?;
            
            // Обрабатываем окнами: для каждого окна берём максимум по модулю
            for window_start in (0..total_samples).step_by(window_size) {
                let window_end = (window_start + window_size).min(total_samples);
                let mut max_amplitude = 0.0f32;
                
                for i in window_start..window_end {
                    let sample_idx = i * spec.channels as usize;
                    if sample_idx < all_samples.len() {
                        let sample = all_samples[sample_idx];
                        let normalized = (sample.abs() as f32) / i16::MAX as f32;
                        if normalized > max_amplitude {
                            max_amplitude = normalized;
                        }
                    }
                }
                
                samples.push(max_amplitude);
                if samples.len() >= max_samples {
                    break;
                }
            }
        }
    }
    
    // Сохраняем в кеш
    if let Err(e) = save_waveform_cache(&cache_path, &samples) {
        if debug {
            log::debug!("Failed to save waveform cache: {:?}", e);
        }
        // Не прерываем выполнение, если не удалось сохранить кеш
    } else if debug {
        log::debug!("Saved waveform to cache: {:?} ({} samples)", cache_path, samples.len());
    }
    
    Ok(samples)
}