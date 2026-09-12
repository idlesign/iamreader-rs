//! Путь к каталогу models/: текущая директория или рядом с исполняемым файлом.

use anyhow::{Context, Result};
use std::path::{Path, PathBuf};

/// Resolve recording paths against the project, never the process working directory.
/// Absolute paths explicitly refer to external files and are left unchanged.
pub fn resolve_project_file(project_dir: &Path, stored: &str) -> PathBuf {
    project_dir.join(stored)
}

/// New recordings are portable: their paths are relative to the project directory.
pub fn stored_recording_path(project_dir: &Path, recording: &Path) -> Result<String> {
    let relative = recording
        .strip_prefix(project_dir)
        .context("Recording is outside the project directory")?;
    Ok(relative
        .to_str()
        .context("Recording path is not UTF-8")?
        .to_owned())
}

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

#[cfg(test)]
#[path = "paths_tests.rs"]
mod tests;
