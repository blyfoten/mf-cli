use crate::mf4;
use rusb::{DeviceHandle, GlobalContext};
use std::{fs, io::Read, path::Path, thread::sleep, time::Duration};

pub const VID: u16 = 0x1c75;
const PID_MF1: u16 = 0xaf80;
const PID_MF1_MSD: u16 = 0xaf82;
const PID_MF2: u16 = 0xaf90;
const PID_MF4: u16 = 0xaf70;

const KMOD_PATH: &str = "/dev/minifuse_cmd";
const KMOD_METERS_PATH: &str = "/dev/minifuse_meters";
const KMOD_API_PATH: &str = "/sys/module/minifuse_mod/parameters/api_version";

const REQ_SET: u8 = 0x22;
const REQ_GET_BLOCK: u8 = 0xb2;
const REQ_GET_METERS: u8 = 0xc2;
pub const BLOCK_LEN: usize = 190;
const TIMEOUT: Duration = Duration::from_millis(500);

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Model {
    Mf1,
    Mf2,
    Mf4,
}

impl Model {
    fn from_pid(pid: u16) -> Option<Model> {
        match pid {
            PID_MF1 | PID_MF1_MSD => Some(Model::Mf1),
            PID_MF2 => Some(Model::Mf2),
            PID_MF4 => Some(Model::Mf4),
            _ => None,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Model::Mf1 => "MiniFuse 1",
            Model::Mf2 => "MiniFuse 2",
            Model::Mf4 => "MiniFuse 4",
        }
    }
}

/// Finds the connected model through sysfs, which works without opening the device.
pub fn detect_model() -> Option<Model> {
    let read_id = |dir: &Path, file: &str| {
        fs::read_to_string(dir.join(file))
            .ok()
            .and_then(|s| u16::from_str_radix(s.trim(), 16).ok())
    };
    for entry in fs::read_dir("/sys/bus/usb/devices").ok()?.flatten() {
        let dir = entry.path();
        if read_id(&dir, "idVendor") == Some(VID) {
            if let Some(model) = read_id(&dir, "idProduct").and_then(Model::from_pid) {
                return Some(model);
            }
        }
    }
    None
}

pub enum Transport {
    /// The seamless kernel module; `api` 1 only understands on/off writes, 3 adds the meters.
    Kmod { api: u32 },
    /// Userspace fallback, which detaches the audio driver while it runs.
    Usb(DeviceHandle<GlobalContext>),
}

impl Transport {
    /// Opens the kernel module if it can do what is needed, otherwise falls back to userspace USB.
    /// `full_api` means values other than on/off are written or the settings block is read.
    pub fn open(full_api: bool) -> Result<Transport, String> {
        if Path::new(KMOD_PATH).exists() {
            let api = fs::read_to_string(KMOD_API_PATH)
                .ok()
                .and_then(|s| s.trim().parse().ok())
                .unwrap_or(1);
            if !full_api || api >= 2 {
                println!("[*] Kernel module detected. Sending commands seamlessly...");
                return Ok(Transport::Kmod { api });
            }
            println!(
                "[!] The loaded kernel module is too old for this command (rebuild it from kmod/). Falling back to userspace USB (audio may interrupt)..."
            );
        } else {
            println!(
                "[!] Kernel module not found. Falling back to userspace USB (audio may interrupt)..."
            );
        }

        let handle = open_usb().ok_or("No MiniFuse device found or permission denied")?;
        let _ = handle.set_auto_detach_kernel_driver(true);
        if let Err(e) = handle.claim_interface(0) {
            eprintln!("Warning: Could not claim interface: {}", e);
        }
        Ok(Transport::Usb(handle))
    }

    /// Sets one parameter: `wvalue` is `(selector << 8) | channel`.
    pub fn write(&mut self, wvalue: u16, value: u16) -> Result<(), String> {
        match self {
            Transport::Kmod { .. } => fs::write(KMOD_PATH, format!("{:04x} {}", wvalue, value))
                .map_err(|e| format!("Failed to write command to kernel module: {}", e))?,
            Transport::Usb(handle) => {
                handle
                    .write_control(0x21, REQ_SET, wvalue, 0, &value.to_le_bytes(), TIMEOUT)
                    .map_err(|e| format!("Failed to send control command: {}", e))?;
            }
        }
        // Small delay to ensure the hardware processes sequential commands
        sleep(Duration::from_millis(20));
        Ok(())
    }

    /// Reads the whole settings block (MiniFuse 4).
    pub fn read_block(&mut self) -> Result<Vec<u8>, String> {
        let data = match self {
            // The module returns the whole block from the first read only, so read it in one
            // call; fs::read would start with a short probe read and truncate it
            Transport::Kmod { api } if *api >= 2 => {
                let mut buf = vec![0u8; BLOCK_LEN];
                let n = fs::File::open(KMOD_PATH)
                    .and_then(|mut f| f.read(&mut buf))
                    .map_err(|e| format!("Failed to read settings from kernel module: {}", e))?;
                buf.truncate(n);
                buf
            }
            Transport::Kmod { .. } => return Err("Kernel module cannot read settings".into()),
            Transport::Usb(handle) => {
                let mut buf = vec![0u8; BLOCK_LEN];
                let n = handle
                    .read_control(0xa1, REQ_GET_BLOCK, 0, 0, &mut buf, TIMEOUT)
                    .map_err(|e| format!("Failed to read settings: {}", e))?;
                buf.truncate(n);
                buf
            }
        };
        if data.len() != BLOCK_LEN {
            return Err(format!(
                "Unexpected settings block length {} (expected {})",
                data.len(),
                BLOCK_LEN
            ));
        }
        Ok(data)
    }

    pub fn has_meters(&self) -> bool {
        !matches!(self, Transport::Kmod { api } if *api < 3)
    }

    /// Reads the level meters (MiniFuse 4), see `mf4::parse_meters`.
    pub fn read_meters(&mut self) -> Result<Vec<u8>, String> {
        let mut buf = vec![0u8; mf4::METERS_LEN];
        let n = match self {
            Transport::Kmod { api } if *api >= 3 => fs::File::open(KMOD_METERS_PATH)
                .and_then(|mut f| f.read(&mut buf))
                .map_err(|e| format!("Failed to read meters from kernel module: {}", e))?,
            Transport::Kmod { .. } => return Err("Kernel module cannot read meters".into()),
            Transport::Usb(handle) => handle
                .read_control(0xa1, REQ_GET_METERS, 0, 0, &mut buf, TIMEOUT)
                .map_err(|e| format!("Failed to read meters: {}", e))?,
        };
        if n != mf4::METERS_LEN {
            return Err(format!("Unexpected meters length {} (expected {})", n, mf4::METERS_LEN));
        }
        Ok(buf)
    }

    pub fn finish(self) {
        if let Transport::Usb(handle) = self {
            let _ = handle.release_interface(0);
            let _ = handle.reset();
        }
    }
}

fn open_usb() -> Option<DeviceHandle<GlobalContext>> {
    for device in rusb::devices().ok()?.iter() {
        let Ok(desc) = device.device_descriptor() else {
            continue;
        };
        if desc.vendor_id() == VID && Model::from_pid(desc.product_id()).is_some() {
            if let Ok(handle) = device.open() {
                return Some(handle);
            }
        }
    }
    None
}
