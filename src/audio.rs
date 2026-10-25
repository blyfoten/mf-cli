//! PipeWire buffer size and sample rate, which set the audio latency. Changed live with
//! `pw-metadata`, and kept across restarts in a PipeWire configuration file.

use crate::preset::config_home;
use std::{fs, io::ErrorKind, path::PathBuf, process::Command};

/// Buffer sizes offered, in frames.
pub const QUANTUMS: [u32; 7] = [32, 64, 128, 256, 512, 1024, 2048];
/// Sample rates the MiniFuse supports.
pub const RATES: [u32; 6] = [44100, 48000, 88200, 96000, 176400, 192000];

/// The clock settings of PipeWire; 0 means PipeWire picks the value itself.
#[derive(Clone, Copy, PartialEq)]
pub struct Settings {
    pub force_quantum: u32,
    pub force_rate: u32,
    /// What PipeWire uses when nothing is forced.
    pub quantum: u32,
    pub rate: u32,
}

impl Settings {
    /// Buffer size in use, as frames and milliseconds.
    pub fn latency(&self) -> (u32, f64) {
        let quantum = if self.force_quantum > 0 { self.force_quantum } else { self.quantum };
        let rate = if self.force_rate > 0 { self.force_rate } else { self.rate };
        (quantum, quantum as f64 * 1000.0 / rate.max(1) as f64)
    }
}

fn pw_metadata(args: &[&str]) -> Result<String, String> {
    let out = Command::new("pw-metadata")
        .args(["-n", "settings", "0"])
        .args(args)
        .output()
        .map_err(|e| format!("Cannot run pw-metadata (is PipeWire installed?): {}", e))?;
    if !out.status.success() {
        return Err(format!("pw-metadata failed: {}", String::from_utf8_lossy(&out.stderr).trim()));
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

pub fn read() -> Result<Settings, String> {
    let text = pw_metadata(&[])?;
    // Lines look like: update: id:0 key:'clock.rate' value:'48000' type:''
    let value = |key: &str| {
        let pattern = format!("key:'{}' value:'", key);
        text.lines()
            .find_map(|line| line.split_once(&pattern))
            .and_then(|(_, rest)| rest.split('\'').next()?.parse().ok())
    };
    match (value("clock.quantum"), value("clock.rate")) {
        (Some(quantum), Some(rate)) => Ok(Settings {
            force_quantum: value("clock.force-quantum").unwrap_or(0),
            force_rate: value("clock.force-rate").unwrap_or(0),
            quantum,
            rate,
        }),
        _ => Err("PipeWire did not report its clock settings".into()),
    }
}

/// Forces the buffer size until PipeWire restarts; 0 lets PipeWire pick it again.
pub fn set_quantum(frames: u32) -> Result<(), String> {
    pw_metadata(&["clock.force-quantum", &frames.to_string()]).map(drop)
}

/// Forces the sample rate until PipeWire restarts; 0 lets PipeWire pick it again.
pub fn set_rate(rate: u32) -> Result<(), String> {
    pw_metadata(&["clock.force-rate", &rate.to_string()]).map(drop)
}

fn conf_path() -> Result<PathBuf, String> {
    Ok(config_home()?.join("pipewire/pipewire.conf.d/50-mf-cli.conf"))
}

/// Writes the forced values to a PipeWire configuration file so they are used after a restart,
/// or removes that file when nothing is forced.
pub fn persist(s: &Settings) -> Result<PathBuf, String> {
    let path = conf_path()?;
    if s.force_quantum == 0 && s.force_rate == 0 {
        return match fs::remove_file(&path) {
            Err(e) if e.kind() != ErrorKind::NotFound => Err(format!("Cannot delete {}: {}", path.display(), e)),
            _ => Ok(path),
        };
    }
    let mut props = String::new();
    if s.force_rate > 0 {
        props += &format!("    default.clock.rate = {}\n", s.force_rate);
        props += &format!("    default.clock.allowed-rates = [ {} ]\n", s.force_rate);
    }
    if s.force_quantum > 0 {
        // Equal limits are how a configuration file forces the buffer size
        for key in ["quantum", "min-quantum", "max-quantum"] {
            props += &format!("    default.clock.{} = {}\n", key, s.force_quantum);
        }
    }
    let text = format!("# Written by mf-gui\ncontext.properties = {{\n{}}}\n", props);
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir).map_err(|e| format!("Cannot create {}: {}", dir.display(), e))?;
    }
    fs::write(&path, text).map_err(|e| format!("Cannot write {}: {}", path.display(), e))?;
    Ok(path)
}

/// Whether the configuration file from `persist` exists.
pub fn persisted() -> bool {
    conf_path().is_ok_and(|p| p.exists())
}
