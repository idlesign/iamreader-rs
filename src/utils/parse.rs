/// Парсит строку в u8 для reduction; пустая строка — None.
pub fn parse_reduction(s: &str) -> Option<u8> {
    let t = s.trim();
    if t.is_empty() {
        None
    } else {
        t.parse::<u8>().ok()
    }
}

/// Парсит строку в i32 для repeat; пустая строка — Some(1).
pub fn parse_repeat(s: &str) -> Option<i32> {
    let t = s.trim();
    if t.is_empty() {
        Some(1)
    } else {
        t.parse::<i32>().ok().or(Some(1))
    }
}

/// Возвращает None для пустой/пробельной строки, иначе Some(trimmed).
pub fn parse_optional_string(s: &str) -> Option<String> {
    let t = s.trim();
    if t.is_empty() {
        None
    } else {
        Some(t.to_string())
    }
}
