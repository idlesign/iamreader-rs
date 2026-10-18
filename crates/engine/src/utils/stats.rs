use std::time::Duration;
use std::path::Path;
use anyhow::Result;
use hound;

/// Получает длительность WAV файла
pub fn get_wav_duration(path: &Path) -> Result<Duration> {
    let reader = hound::WavReader::open(path)?;
    let spec = reader.spec();
    let duration_secs = reader.len() as f64 / spec.sample_rate as f64 / spec.channels as f64;
    Ok(Duration::from_secs_f64(duration_secs))
}

/// Размер файла и длительность в мс по данным с диска (для обновления мета).
pub fn get_file_size_and_duration_ms(path: &Path) -> Result<(u64, u64)> {
    let size = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
    let duration_ms = get_wav_duration(path)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    Ok((size, duration_ms))
}

/// Параметры WAV (sample_rate, channels) из файла.
pub fn get_wav_spec(path: &Path) -> Result<(u32, u16)> {
    let reader = hound::WavReader::open(path)?;
    let spec = reader.spec();
    Ok((spec.sample_rate, spec.channels))
}

/// Вычисляет общую длительность всех файлов проекта
pub fn calculate_total_duration(files: &[crate::project::ProjectFile]) -> Duration {
    let mut total = Duration::ZERO;
    for file in files {
        total += Duration::from_millis(file.duration_ms);
    }
    total
}

/// Вычисляет общий размер всех файлов проекта
pub fn calculate_sizes(files: &[crate::project::ProjectFile]) -> u64 {
    let mut total_size = 0u64;
    for file in files {
        total_size += file.size;
    }
    total_size
}

/// Получает свободное место на диске для указанной директории
pub fn get_free_space(chunks_dir: &Path) -> u64 {
    #[cfg(unix)]
    {
        use std::ffi::CString;
        use std::os::raw::c_char;
        
        let path_cstr = match CString::new(chunks_dir.to_string_lossy().as_ref()) {
            Ok(p) => p,
            Err(_) => return 0,
        };
        
        unsafe {
            use libc;
            let mut stat: libc::statvfs = std::mem::zeroed();
            if libc::statvfs(path_cstr.as_ptr() as *const c_char, &mut stat) == 0 {
                (stat.f_bavail as u64) * (stat.f_frsize as u64)
            } else {
                0
            }
        }
    }
    #[cfg(not(unix))]
    {
        0
    }
}

