//! Presets: MiniFuse 4 settings saved to files in ~/.config/mf-cli/presets, and the one to apply
//! when a MiniFuse 4 in its power-on state is connected.

use crate::mf4::Block;
use std::{env, fs, io::ErrorKind, path::PathBuf};

const EXT: &str = "preset";

fn config_dir() -> Result<PathBuf, String> {
    match env::var_os("XDG_CONFIG_HOME").filter(|d| !d.is_empty()) {
        Some(dir) => Ok(PathBuf::from(dir).join("mf-cli")),
        None => env::var_os("HOME")
            .map(|home| PathBuf::from(home).join(".config/mf-cli"))
            .ok_or_else(|| "Neither XDG_CONFIG_HOME nor HOME is set".into()),
    }
}

fn presets_dir() -> Result<PathBuf, String> {
    Ok(config_dir()?.join("presets"))
}

/// File of a preset: a name in the presets directory, or a path to any file when it has a `/`.
pub fn path(name: &str) -> Result<PathBuf, String> {
    if name.contains('/') {
        return Ok(PathBuf::from(name));
    }
    if name.is_empty() || name.starts_with('.') {
        return Err(format!("'{}' is not a valid preset name", name));
    }
    Ok(presets_dir()?.join(format!("{}.{}", name, EXT)))
}

pub fn save(name: &str, block: &Block) -> Result<PathBuf, String> {
    let path = path(name)?;
    if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
        fs::create_dir_all(dir).map_err(|e| format!("Cannot create {}: {}", dir.display(), e))?;
    }
    fs::write(&path, block.to_preset()).map_err(|e| format!("Cannot write {}: {}", path.display(), e))?;
    Ok(path)
}

pub fn load(name: &str) -> Result<Block, String> {
    let path = path(name)?;
    let text = fs::read_to_string(&path).map_err(|e| match e.kind() {
        ErrorKind::NotFound => format!("No preset '{}' ({} does not exist)", name, path.display()),
        _ => format!("Cannot read {}: {}", path.display(), e),
    })?;
    Block::from_preset(&text).map_err(|e| format!("{}: {}", path.display(), e))
}

pub fn delete(name: &str) -> Result<(), String> {
    let path = path(name)?;
    fs::remove_file(&path).map_err(|e| format!("Cannot delete {}: {}", path.display(), e))
}

/// Names of the saved presets, sorted.
pub fn list() -> Result<Vec<String>, String> {
    let dir = presets_dir()?;
    let entries = match fs::read_dir(&dir) {
        Ok(entries) => entries,
        Err(e) if e.kind() == ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(format!("Cannot read {}: {}", dir.display(), e)),
    };
    let mut names: Vec<String> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e == EXT))
        .filter_map(|p| p.file_stem().map(|s| s.to_string_lossy().into_owned()))
        .collect();
    names.sort();
    Ok(names)
}

fn autoload_file() -> Result<PathBuf, String> {
    Ok(config_dir()?.join("autoload"))
}

/// The preset applied when a MiniFuse 4 in its power-on state is connected.
pub fn autoload() -> Option<String> {
    let name = fs::read_to_string(autoload_file().ok()?).ok()?;
    Some(name.trim().to_string()).filter(|n| !n.is_empty())
}

pub fn set_autoload(name: Option<&str>) -> Result<(), String> {
    let file = autoload_file()?;
    match name {
        Some(name) => {
            // Fail now rather than when the device is connected
            load(name)?;
            fs::create_dir_all(file.parent().unwrap())
                .and_then(|_| fs::write(&file, format!("{}\n", name)))
                .map_err(|e| format!("Cannot write {}: {}", file.display(), e))
        }
        None => match fs::remove_file(&file) {
            Err(e) if e.kind() != ErrorKind::NotFound => Err(format!("Cannot delete {}: {}", file.display(), e)),
            _ => Ok(()),
        },
    }
}
