//! Путь к каталогу models/: текущая директория или рядом с исполняемым файлом.

use anyhow::{Context, Result};
use std::path::PathBuf;

/// Каталог models/: если в текущей директории есть models/ — он, иначе models/ рядом с исполняемым файлом.
pub fn models_dir() -> Result<PathBuf> {
    if let Ok(cwd) = std::env::current_dir() {
        let cur_models = cwd.join("models");
        if cur_models.is_dir() {
            return Ok(cur_models);
        }
    }
    std::env::current_exe()
        .context("current_exe")?
        .parent()
        .map(PathBuf::from)
        .map(|p| p.join("models"))
        .ok_or_else(|| anyhow::anyhow!("no parent for exe"))
}
