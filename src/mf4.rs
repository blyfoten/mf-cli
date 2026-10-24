//! MiniFuse 4 parameters. See PROTOCOL-MF4.md for how they were found.

use crate::device::BLOCK_LEN;
use std::f64::consts::FRAC_PI_2;

pub const SEL_INST: u8 = 0x00;
pub const SEL_48V: u8 = 0x03;
pub const SEL_LINK34: u8 = 0x06;
const SEL_MIX: u8 = 0x09;
pub const SEL_MASTER: u8 = 0x45;
pub const SEL_INPUT_GAIN: u8 = 0x48;
pub const SEL_MUTE: u8 = 0x4c;
pub const SEL_SOLO: u8 = 0x4f;
const SEL_LAYOUT_MONO: u8 = 0x52;
const SEL_LAYOUT_STEREO: u8 = 0x55;
pub const SEL_SOURCE: u8 = 0x58;

/// Gain value meaning "off" (-128 dB).
pub const OFF: u16 = 0x8000;

pub const SOURCES: [&str; 10] = [
    "in1", "in2", "in3", "in4", "usb1", "usb2", "usb3", "usb4", "usb5", "usb6",
];
pub const BUSES: [&str; 3] = ["out12", "out34", "loop"];
const BUS_TITLES: [&str; 3] = ["Outputs 1&2", "Outputs 3&4", "Loopback"];

pub fn wvalue(selector: u8, channel: u8) -> u16 {
    (selector as u16) << 8 | channel as u16
}

/// First selector of the mixer gains for one side (0 = left, 1 = right) of a bus;
/// the source is written as the channel.
pub fn mix_selector(bus: u8, side: u8) -> u8 {
    SEL_MIX + (bus * 2 + side) * 10
}

pub fn parse_source(s: &str) -> Option<u8> {
    SOURCES.iter().position(|&n| n == s).map(|i| i as u8)
}

pub fn parse_bus(s: &str) -> Option<u8> {
    match s {
        "loopback" => Some(2),
        _ => BUSES.iter().position(|&n| n == s).map(|i| i as u8),
    }
}

/// Source selector value for `bus`, as offered by Control Center.
pub fn parse_output_source(bus: u8, s: &str) -> Option<u16> {
    match (bus, s) {
        (0, "direct") => Some(0),
        (0, "mixer") => Some(1),
        (1, "direct") => Some(2),
        (1, "mixer") => Some(3),
        (2, "usb56") => Some(4),
        (2, "cue2mix") => Some(5),
        (2, "out12") => Some(6),
        (2, "out34") => Some(7),
        _ => None,
    }
}

fn output_source_name(value: u16) -> String {
    match value {
        0 => "USB1/2 direct (mixer bypassed)".into(),
        2 => "USB3/4 direct (mixer bypassed)".into(),
        1 | 3 => "mixer".into(),
        4 => "USB5/6".into(),
        5 => "Cue2Mix".into(),
        6 => "OUT1/2".into(),
        7 => "OUT3/4".into(),
        v => format!("unknown ({})", v),
    }
}

/// dB in 1/256 steps, truncated like Control Center does; `None` is off.
pub fn db_to_raw(db: Option<f64>) -> u16 {
    match db {
        Some(db) if db > -128.0 => (db * 256.0).trunc().clamp(-32767.0, 32767.0) as i16 as u16,
        _ => OFF,
    }
}

pub fn raw_to_db(raw: u16) -> Option<f64> {
    (raw != OFF).then(|| raw as i16 as f64 / 256.0)
}

/// Left and right mixer gains for a fader level and a balance from -100 (left) to 100 (right),
/// using the constant-power law of Control Center (centre = -3.01 dB on both sides).
pub fn mix_gains(level: Option<f64>, pan: f64) -> (u16, u16) {
    let Some(level) = level else {
        return (OFF, OFF);
    };
    let theta = (pan.clamp(-100.0, 100.0) + 100.0) / 200.0 * FRAC_PI_2;
    let side = |g: f64| {
        if g < 1e-4 {
            OFF
        } else {
            db_to_raw(Some(level + 20.0 * g.log10()))
        }
    };
    (side(theta.cos()), side(theta.sin()))
}

/// Inverse of `mix_gains`: fader level (`None` if both sides are off) and balance.
pub fn level_and_pan(left: u16, right: u16) -> (Option<f64>, f64) {
    let lin = |raw| raw_to_db(raw).map_or(0.0, |db| 10f64.powf(db / 20.0));
    let (l, r) = (lin(left), lin(right));
    if l == 0.0 && r == 0.0 {
        return (None, 0.0);
    }
    let level = 20.0 * (l * l + r * r).sqrt().log10();
    let pan = r.atan2(l) / FRAC_PI_2 * 200.0 - 100.0;
    (Some(level), pan)
}

pub struct Block(Vec<u8>);

impl Block {
    pub fn new(data: Vec<u8>) -> Block {
        assert_eq!(data.len(), BLOCK_LEN);
        Block(data)
    }

    pub fn get(&self, selector: u8) -> u16 {
        let i = selector as usize * 2;
        u16::from_le_bytes([self.0[i], self.0[i + 1]])
    }

    fn bit(&self, selector: u8, bit: u8) -> bool {
        self.get(selector) & (1 << bit) != 0
    }

    pub fn strip(&self, bus: u8, src: u8) -> (Option<f64>, f64) {
        level_and_pan(
            self.get(mix_selector(bus, 0) + src),
            self.get(mix_selector(bus, 1) + src),
        )
    }

    /// Whether Control Center shows a strip for `src` in the mixer of `bus`.
    fn visible(&self, bus: u8, src: u8) -> bool {
        (src < 4 && self.bit(SEL_LAYOUT_MONO + bus, src)) || self.bit(SEL_LAYOUT_STEREO + bus, src / 2)
    }

    pub fn dump(&self) {
        for sel in 0..(BLOCK_LEN / 2) as u8 {
            println!("sel 0x{:02x}  0x{:04x}", sel, self.get(sel));
        }
    }

    pub fn print_status(&self) {
        let on_off = |b: bool| if b { "on" } else { "off" };
        let db = |raw: u16| raw_to_db(raw).map_or("off".to_string(), |d| format!("{:+.1} dB", d));

        println!("INST          in1 {}, in2 {}", on_off(self.bit(SEL_INST, 0)), on_off(self.bit(SEL_INST, 1)));
        println!("48V           {}", on_off(self.get(SEL_48V) != 0));
        println!(
            "Input gain    in3 {}, in4 {}{}",
            db(self.get(SEL_INPUT_GAIN + 2)),
            db(self.get(SEL_INPUT_GAIN + 3)),
            if self.get(SEL_LINK34) != 0 { " (linked)" } else { "" }
        );

        for bus in 0..3u8 {
            println!();
            println!(
                "{} ({}): source {}, master {}",
                BUS_TITLES[bus as usize],
                BUSES[bus as usize],
                output_source_name(self.get(SEL_SOURCE + bus)),
                db(self.get(SEL_MASTER + bus))
            );
            println!("    source   level     balance  mute  solo");
            for src in 0..SOURCES.len() as u8 {
                let (level, pan) = self.strip(bus, src);
                let level = level.map_or("off".to_string(), |l| format!("{:+.1} dB", l));
                let pan = match pan.round() {
                    p if p.abs() < 1.0 => "C".to_string(),
                    p if p < 0.0 => format!("L{}", -p),
                    p => format!("R{}", p),
                };
                println!(
                    "    {:<8} {:<9} {:<8} {:<5} {:<5}{}",
                    SOURCES[src as usize],
                    level,
                    pan,
                    if self.bit(SEL_MUTE + bus, src) { "M" } else { "-" },
                    if self.bit(SEL_SOLO + bus, src) { "S" } else { "-" },
                    if self.visible(bus, src) { "" } else { "  (not shown in Control Center)" }
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn centre_matches_control_center() {
        // Captured: a centred strip at 0 dB is stored as 0xfcfe on both sides.
        assert_eq!(mix_gains(Some(0.0), 0.0), (0xfcfe, 0xfcfe));
    }

    #[test]
    fn fader_ends_match_control_center() {
        // Captured: +12 dB -> 2301, -60 dB -> -16130 when centred.
        assert_eq!(mix_gains(Some(12.0), 0.0).0 as i16, 2301);
        assert_eq!(mix_gains(Some(-60.0), 0.0).0 as i16, -16130);
    }

    #[test]
    fn hard_pan_turns_other_side_off() {
        assert_eq!(mix_gains(Some(0.0), -100.0), (0x0000, OFF));
        assert_eq!(mix_gains(Some(0.0), 100.0), (OFF, 0x0000));
    }

    #[test]
    fn level_and_pan_round_trip() {
        for &(level, pan) in &[(0.0, 0.0), (-6.0, -50.0), (6.0, 30.0), (-20.0, 100.0)] {
            let (l, r) = mix_gains(Some(level), pan);
            let (level2, pan2) = level_and_pan(l, r);
            assert!((level2.unwrap() - level).abs() < 0.05, "{level} -> {level2:?}");
            assert!((pan2 - pan).abs() < 0.5, "{pan} -> {pan2}");
        }
        assert_eq!(level_and_pan(OFF, OFF).0, None);
    }

    #[test]
    fn db_conversion() {
        assert_eq!(db_to_raw(Some(24.0)), 0x1800);
        assert_eq!(db_to_raw(Some(-24.0)), 0xe800);
        assert_eq!(db_to_raw(Some(-64.0)), 0xc000);
        assert_eq!(db_to_raw(None), OFF);
        assert_eq!(raw_to_db(0xfcfe).map(|d| (d * 100.0).round()), Some(-301.0));
    }

    /// Settings block captured from a MiniFuse 4 with factory defaults.
    const DEFAULT_BLOCK: &str = "000000000000000000000100000000000400fefcfefcfefcfefc000000c4fefcfefcfefcfefcfefcfefcfefcfefc00c40000fefcfefcfefcfefcfefcfefcfefcfefcfefcfefc000000c4fefcfefcfefcfefcfefcfefcfefcfefc00c40000fefcfefcfefcfefcfefcfefcfefcfefcfefcfefc000000c4fefcfefcfefcfefcfefcfefcfefcfefc00c400000000000000000000000000000000cfff3fffffff0000000000000300030000000400080010000100030004000000000000000d60";

    fn default_block() -> Block {
        let bytes = (0..DEFAULT_BLOCK.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&DEFAULT_BLOCK[i..i + 2], 16).unwrap())
            .collect();
        Block::new(bytes)
    }

    #[test]
    fn default_block_decodes() {
        let b = default_block();
        assert_eq!(b.get(SEL_48V), 0);
        assert_eq!(b.get(SEL_MUTE), 0xffcf); // only USB 1/2 audible on Outputs 1&2
        assert_eq!(b.get(SEL_SOURCE), 1); // Outputs 1&2 from the mixer
        let (level, pan) = b.strip(0, 0); // Analog 1 -> Outputs 1&2, centred at 0 dB
        assert!(level.unwrap().abs() < 0.01 && pan.abs() < 0.5);
        let (level, pan) = b.strip(0, 4); // USB 1 -> Outputs 1&2, hard left
        assert!(level.unwrap().abs() < 0.01 && pan < -99.0);
        b.print_status(); // must not panic; see output with --nocapture
    }

    #[test]
    fn selectors_match_captures() {
        assert_eq!(wvalue(mix_selector(0, 0), 0), 0x0900); // Analog 1 -> Out 1 (balance capture)
        assert_eq!(wvalue(mix_selector(0, 1), 0), 0x1300); // Analog 1 -> Out 2
        assert_eq!(wvalue(mix_selector(1, 0), 2), 0x1d02); // Analog 3 -> Out 3 (strip capture)
        assert_eq!(wvalue(mix_selector(1, 1), 3), 0x2703); // Analog 4 -> Out 4
        assert_eq!(wvalue(SEL_MUTE + 1, 3), 0x4d03);
        assert_eq!(wvalue(SEL_INPUT_GAIN, 2), 0x4802);
    }
}
