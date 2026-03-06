use std::time::Duration;
use crate::ui::FileInfo;
use crate::project::ProjectFile;

/// Форматирует длительность. При `hours == false`: MM:SS при нуле часов, иначе HH:MM:SS.
/// При `hours == true`: всегда HH:MM:SS (00: при нуле часов).
pub fn format_duration(d: Duration, hours: bool) -> String {
    let secs = d.as_secs();
    let hrs = secs / 3600;
    let minutes = (secs % 3600) / 60;
    let seconds = secs % 60;
    if hours {
        format!("{:02}:{:02}:{:02}", hrs, minutes, seconds)
    } else if hrs == 0 {
        format!("{:02}:{:02}", minutes, seconds)
    } else {
        format!("{:02}:{:02}:{:02}", hrs, minutes, seconds)
    }
}

/// Форматирует размер в байтах в читаемый формат (B, KB, MB, GB)
pub fn format_size(bytes: u64) -> String {
    const UNITS: &[&str] = &["B", "KB", "MB", "GB"];
    let mut size = bytes as f64;
    let mut unit_index = 0;
    
    while size >= 1024.0 && unit_index < UNITS.len() - 1 {
        size /= 1024.0;
        unit_index += 1;
    }
    
    format!("{:.1} {}", size, UNITS[unit_index])
}

/// Разворачивает список файлов и пересчитывает индексы
/// Новые файлы должны быть в начале списка (reverse)
pub fn reverse_and_reindex_file_list(mut file_list: Vec<FileInfo>) -> Vec<FileInfo> {
    file_list.reverse();
    let total_files = file_list.len();
    for (index, file_info) in file_list.iter_mut().enumerate() {
        file_info.index = (total_files - index) as i32;
    }
    file_list
}

const HINT_ELLIPSIS: &str = "[...]";

/// Возвращает (начало, конец) для отображения в UI: всегда видны начальные и конечные слова.
/// Если текст короткий — (текст, ""). Если длинный — (start, end), середина заменена на [...].
pub fn truncate_text_to_start_end(text: &str, max_length: usize) -> (String, String) {
    let char_count = text.chars().count();
    if char_count <= max_length {
        return (text.to_string(), String::new());
    }
    if max_length < 10 {
        let take = max_length.saturating_sub(3);
        let start: String = text.chars().take(take).collect();
        return (format!("{}...", start), String::new());
    }
    let ellipsis_len = HINT_ELLIPSIS.chars().count();
    let available = max_length - ellipsis_len;
    let start_len = available / 2;
    let end_len = available - start_len;
    let start: String = text.chars().take(start_len).collect();
    let end: String = if char_count > end_len {
        text.chars().rev().take(end_len).collect::<Vec<_>>().into_iter().rev().collect()
    } else {
        String::new()
    };
    (start, end)
}

/// Подсказки для текущей и предыдущей записи: (current_start, current_end, prev_start, prev_end).
/// Одно место расчёта для обеих панелей, без дублирования.
pub fn current_and_prev_file_hints(
    files: &[ProjectFile],
    current_index: Option<usize>,
    max_len: usize,
) -> (String, String, String, String) {
    let current = current_index
        .and_then(|idx| files.get(idx))
        .map(|file| truncate_text_to_start_end(&file.hint, max_len))
        .unwrap_or((String::new(), String::new()));
    let prev = current_index
        .and_then(|idx| if idx > 0 { files.get(idx - 1) } else { None })
        .map(|file| truncate_text_to_start_end(&file.hint, max_len))
        .unwrap_or((String::new(), String::new()));
    (current.0, current.1, prev.0, prev.1)
}

/// Формирует строки маркеров с порядковыми номерами для всех файлов за один проход O(n)
pub fn format_markers_with_ordinals_batch(files: &[ProjectFile]) -> Vec<String> {
    let mut marker_counts: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    let mut result = Vec::with_capacity(files.len());
    for file in files {
        let markers_str = if file.markers.is_empty() {
            String::new()
        } else {
            file.markers
                .iter()
                .map(|marker| {
                    let count = marker_counts.get(marker).copied().unwrap_or(0);
                    format!("{}{}", marker, count + 1)
                })
                .collect::<Vec<_>>()
                .join(" ")
        };
        for marker in &file.markers {
            *marker_counts.entry(marker.clone()).or_insert(0) += 1;
        }
        result.push(markers_str);
    }
    result
}

