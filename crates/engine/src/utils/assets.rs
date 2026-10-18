use rust_embed::RustEmbed;
use std::path::Path;
use anyhow::{Result, Context};

#[derive(RustEmbed)]
#[folder = "assets"]
struct Asset;

pub fn get_asset_file(filename: &str) -> Result<Option<Vec<u8>>> {
    use log::debug;
    
    // Сначала пытаемся прочитать из текущей директории
    let current_dir_path = Path::new(filename);
    if current_dir_path.exists() {
        debug!("[assets] Found file in current directory: {}", filename);
        return Ok(Some(std::fs::read(current_dir_path)
            .with_context(|| format!("Failed to read file from current directory: {}", filename))?));
    }
    
    // Если файла нет в текущей директории, используем внедрённый
    // rust-embed ищет файлы относительно папки assets, поэтому используем только имя файла
    if let Some(file) = Asset::get(filename) {
        debug!("[assets] Found embedded file: {}", filename);
        Ok(Some(file.data.to_vec()))
    } else {
        debug!("[assets] File not found in current directory or embedded assets: {}", filename);
        Ok(None)
    }
}

