# mf-cli

`mf-cli` is a lightweight Linux command-line utility and kernel module written in Rust and C for controlling hardware features of the Arturia MiniFuse 1/2/4 audio interfaces. It allows you to toggle hardware settings that are normally only accessible via the proprietary "MiniFuse Control Center" software on Windows or macOS.

By default, manipulating USB interfaces from userspace interrupts the active audio stream. `mf-cli` ships with a custom DKMS kernel module that perfectly integrates with the Linux kernel to send commands to the device **seamlessly**, without any audio dropouts!

## Key Features

- Phantom Power (+48V)
- Direct Mono (MiniFuse 1/2)
- Instrument Mode (INST)
- MiniFuse 4: INST on both inputs, input 3/4 gain and link, the Control Center mixer (mute, solo, level, balance, master), output sources and a status readout
- Seamless control without audio interruption (via DKMS kernel module)
- Non-Sudo Operation (both native and via fallback)
- Chain multiple commands at once

## ⚠️ Important Note on Secure Boot

To achieve seamless, interruption-free audio toggling, this tool utilizes a custom DKMS kernel module. If you have **Secure Boot enabled** in your BIOS, the Linux kernel will block this unsigned module from loading.

You will either need to disable Secure Boot or sign the DKMS module with your own Machine Owner Key (MOK). If the module cannot be loaded, `mf-cli` will gracefully fall back to standard userspace USB commands (which may cause a brief interruption to your audio stream).

## Installation

### Arch Linux (AUR) - Recommended

The AUR package automatically builds both the CLI tool and the DKMS kernel module. Install using your favorite AUR helper:

```bash
yay -S mf-cli
```

### NixOS (flake)

Add the flake as an input and enable the NixOS module. It installs the CLI and the udev rule, and builds the kernel module against the kernel you boot:

```nix
{
  inputs.mf-cli.url = "github:nolight132/mf-cli";

  outputs =
    { nixpkgs, mf-cli, ... }:
    {
      nixosConfigurations.myhost = nixpkgs.lib.nixosSystem {
        modules = [
          mf-cli.nixosModules.default
          { hardware.minifuse.enable = true; }
        ];
      };
    };
}
```

Set `hardware.minifuse.kernelModule.enable = false` to skip the kernel module and use the userspace fallback instead.

To try the CLI without installing it, run `nix run github:nolight132/mf-cli -- 48v on`.

### Manual build

Ensure you have `cargo`, `libusb`, `systemd-libs`, `make`, and your system's Linux kernel headers installed.

**1. Build the Rust CLI:**

```bash
git clone https://github.com/nolight132/mf-cli
cd mf-cli
cargo build --release
sudo cp target/release/mf-cli /usr/bin/
```

**2. Build the seamless Kernel Module (Optional, but highly recommended):**

```bash
cd kmod
make
sudo make install
sudo modprobe minifuse_mod
```

## Configuration (Permissions)

`mf-cli` is designed to be used without `sudo`.

- **If using the kernel module:** No configuration is required! The module automatically creates a character device at `/dev/minifuse_cmd` with the proper permissions.
- **If using the userspace fallback (no kernel module):** You must install the provided udev rule to avoid permission denied errors. (The AUR package does this automatically). If installing manually:

1. Copy `99-minifuse.rules` to `/etc/udev/rules.d/`.

2. Reload rules:

```bash
sudo udevadm control --reload-rules && sudo udevadm trigger
```

## Usage

The syntax is straightforward: `mf-cli <target> <on|off>[<target> <on|off> ...]`

`mf-cli` will automatically detect if the kernel module is loaded. If it is, it will route commands seamlessly. If not, it will print a warning and fall back to userspace USB control.

### **Examples:**

#### Toggle Phantom +48V Power

```bash
mf-cli 48v on
mf-cli 48v off
```

#### Toggle Direct Monitoring Mono:

```bash
mf-cli monitor on
mf-cli monitor off
```

#### Toggle Instrument Mode:

```bash
mf-cli inst on
mf-cli inst off
```

#### Chain Multiple Commands:

```bash
mf-cli inst on 48v on monitor off
```

### MiniFuse 4

The MiniFuse 4 has more settings, all of which the Control Center mixer exposes. `mf-cli --help` lists every command. The protocol is documented in [PROTOCOL-MF4.md](PROTOCOL-MF4.md).

Sources are `in1`..`in4` (analog inputs) and `usb1`..`usb6` (computer playback), or the stereo pairs `in12`, `in34`, `usb12`, `usb34` and `usb56`, which act on both channels. Mixers (buses) are `out12`, `out34` and `loop` (Loopback).

```bash
mf-cli status                          # show all settings
mf-cli inst2 on                        # instrument mode on input 2
mf-cli 48v on                          # phantom power (both mic inputs)
mf-cli gain in3 +6 link34 on           # line input 3/4 gain, linked
mf-cli mute in1 out12 off              # hear input 1 on outputs 1&2 (direct monitoring)
mf-cli level in1 out12 -6 pan L        # fader -6 dB, balance hard left
mf-cli mute in34 out12 off             # stereo pair: both line inputs
mf-cli level usb12 out12 -10           # both channels, each keeps its balance
mf-cli solo in2 out12 on
mf-cli master out12 -10
mf-cli source out12 mixer              # or "direct" to bypass the mixer
mf-cli source loop cue2mix             # usb56, cue2mix, out12 or out34
```

`status` and the commands that write levels need the kernel module from this version (it reports `api_version` 2). With an older module loaded, `mf-cli` falls back to userspace USB for those commands.

## License

This project consists of two components, which are licensed separately to comply with Linux kernel standards:

- The **Rust CLI utility** (userspace code) is licensed under the [MIT License](LICENSE).
- The **Linux Kernel Module** (`kmod/` directory) is licensed under the **GNU General Public License v2.0** (GPL-2.0).
