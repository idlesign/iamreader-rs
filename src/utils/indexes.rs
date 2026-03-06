/// Индекс в project.files: 0 = самый старый, последний = самый новый.
/// UI показывает список в обратном порядке: ui_index 0 = самый новый.

/// Конвертирует индекс в project.files в UI-индекс (0 = самый новый).
#[inline]
pub fn orig_to_ui_index(orig: usize, total_files: usize) -> i32 {
    if total_files == 0 || orig >= total_files {
        -1
    } else {
        (total_files - 1 - orig) as i32
    }
}

/// Конвертирует UI-индекс в индекс в project.files.
#[inline]
pub fn ui_to_orig_index(ui_index: i32, total_files: usize) -> Option<usize> {
    if total_files == 0 || ui_index < 0 {
        return None;
    }
    let u = ui_index as usize;
    if u >= total_files {
        return None;
    }
    Some(total_files - 1 - u)
}

/// Ищет ближайший UI-индекс записи с маркером section. next: true = более новая (меньше ui), false = более старая (больше ui).
pub fn find_section_ui_index(
    total_files: usize,
    current_ui: i32,
    next: bool,
    mut has_section: impl FnMut(usize) -> bool,
) -> Option<i32> {
    let range: Box<dyn Iterator<Item = i32>> = if next {
        Box::new((0..current_ui).rev())
    } else {
        Box::new((current_ui + 1)..(total_files as i32))
    };
    for ui in range {
        if let Some(orig) = ui_to_orig_index(ui, total_files) {
            if has_section(orig) {
                return Some(ui);
            }
        }
    }
    None
}
