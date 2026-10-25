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

/// Stereo pairs as Control Center shows them, by their first source.
const PAIRS: [(&str, u8); 5] = [("in12", 0), ("in34", 2), ("usb12", 4), ("usb34", 6), ("usb56", 8)];

/// A single source or both channels of a stereo pair.
pub fn parse_sources(s: &str) -> Option<Vec<u8>> {
    if let Some(&(_, first)) = PAIRS.iter().find(|(n, _)| *n == s) {
        return Some(vec![first, first + 1]);
    }
    SOURCES.iter().position(|&n| n == s).map(|i| vec![i as u8])
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

/// Writes that add a strip for `srcs` (one input, or the two channels of a pair) to the mixer
/// of `bus`, or remove it, the way Control Center does: the layout bit only tells it which strips
/// to draw, so it also mutes removed strips, and starts added ones centred at 0 dB, or a pair
/// hard-panned with each channel at 0 dB on its own side.
pub fn strip_writes(bus: u8, srcs: &[u8], add: bool) -> Vec<(u16, u16)> {
    let layout = match srcs {
        [src] => wvalue(SEL_LAYOUT_MONO + bus, *src),
        _ => wvalue(SEL_LAYOUT_STEREO + bus, srcs[0] / 2),
    };
    let mut writes = vec![(layout, add as u16)];
    for (i, &src) in srcs.iter().enumerate() {
        writes.push((wvalue(SEL_SOLO + bus, src), 0));
        if add {
            let pan = if srcs.len() == 1 { 0.0 } else if i == 0 { -100.0 } else { 100.0 };
            let (left, right) = mix_gains(Some(0.0), pan);
            writes.push((wvalue(mix_selector(bus, 0), src), left));
            writes.push((wvalue(mix_selector(bus, 1), src), right));
        } else {
            writes.push((wvalue(SEL_MUTE + bus, src), 1));
        }
    }
    writes
}

/// How a setting is written; see `Block::writes_to`.
enum Param {
    /// One write per bit, with the bit as the channel.
    Bits(u8, u8),
    /// One write per element, with the index as the channel.
    Array(u8, u8),
    /// A single value.
    Plain(u8),
}

/// Every setting Control Center writes, which is what a preset holds.
const PARAMS: [Param; 28] = [
    Param::Bits(SEL_INST, 2),
    Param::Plain(SEL_48V),
    Param::Plain(SEL_LINK34),
    Param::Array(SEL_INPUT_GAIN, 4),
    Param::Array(SEL_MIX, 10),
    Param::Array(SEL_MIX + 10, 10),
    Param::Array(SEL_MIX + 20, 10),
    Param::Array(SEL_MIX + 30, 10),
    Param::Array(SEL_MIX + 40, 10),
    Param::Array(SEL_MIX + 50, 10),
    Param::Plain(SEL_MASTER),
    Param::Plain(SEL_MASTER + 1),
    Param::Plain(SEL_MASTER + 2),
    Param::Bits(SEL_MUTE, 10),
    Param::Bits(SEL_MUTE + 1, 10),
    Param::Bits(SEL_MUTE + 2, 10),
    Param::Bits(SEL_SOLO, 10),
    Param::Bits(SEL_SOLO + 1, 10),
    Param::Bits(SEL_SOLO + 2, 10),
    Param::Bits(SEL_LAYOUT_MONO, 4),
    Param::Bits(SEL_LAYOUT_MONO + 1, 4),
    Param::Bits(SEL_LAYOUT_MONO + 2, 4),
    Param::Bits(SEL_LAYOUT_STEREO, 5),
    Param::Bits(SEL_LAYOUT_STEREO + 1, 5),
    Param::Bits(SEL_LAYOUT_STEREO + 2, 5),
    Param::Plain(SEL_SOURCE),
    Param::Plain(SEL_SOURCE + 1),
    Param::Plain(SEL_SOURCE + 2),
];

impl Param {
    /// Selectors whose value this setting stores.
    fn selectors(&self) -> std::ops::Range<u8> {
        match *self {
            Param::Bits(sel, _) | Param::Plain(sel) => sel..sel + 1,
            Param::Array(base, n) => base..base + n,
        }
    }
}

/// Settings block of a MiniFuse 4 with factory defaults, as it is after power-on.
const FACTORY: &str = "000000000000000000000100000000000400fefcfefcfefcfefc000000c4fefcfefcfefcfefcfefcfefcfefcfefc00c40000fefcfefcfefcfefcfefcfefcfefcfefcfefcfefc000000c4fefcfefcfefcfefcfefcfefcfefcfefc00c40000fefcfefcfefcfefcfefcfefcfefcfefcfefcfefc000000c4fefcfefcfefcfefcfefcfefcfefcfefc00c400000000000000000000000000000000cfff3fffffff0000000000000300030000000400080010000100030004000000000000000d60";

/// Header of a preset file: one `<selector> <value>` line in hex per stored selector follows.
const PRESET_HEADER: &str = "# mf-cli preset for the MiniFuse 4: settings block selector and value, in hex";

/// Length of the level meter reply (request `0xc2`).
pub const METERS_LEN: usize = 64;

/// Level meters in dBFS (-inf when silent): sources 0-9 in the order of `SOURCES`, then the left
/// and right output of each bus at `10 + 2 * bus`. Each is a little-endian 32-bit linear peak
/// with `0x7fffffff` as full scale.
pub fn parse_meters(data: &[u8]) -> [f64; 16] {
    let mut meters = [f64::NEG_INFINITY; 16];
    for (m, word) in meters.iter_mut().zip(data.chunks_exact(4)) {
        let peak = u32::from_le_bytes(word.try_into().unwrap());
        if peak > 0 {
            *m = 20.0 * (peak as f64 / i32::MAX as f64).log10();
        }
    }
    meters
}

#[derive(Clone)]
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

    fn set(&mut self, selector: u8, value: u16) {
        let i = selector as usize * 2;
        if i + 1 < self.0.len() {
            self.0[i..i + 2].copy_from_slice(&value.to_le_bytes());
        }
    }

    pub fn factory() -> Block {
        let bytes = (0..FACTORY.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&FACTORY[i..i + 2], 16).unwrap())
            .collect();
        Block::new(bytes)
    }

    /// Writes that turn the settings of this block into those of `target`, skipping the ones
    /// that are already equal.
    pub fn writes_to(&self, target: &Block) -> Vec<(u16, u16)> {
        let mut writes = Vec::new();
        for param in &PARAMS {
            match *param {
                Param::Bits(sel, n) => {
                    for bit in 0..n {
                        if self.bit(sel, bit) != target.bit(sel, bit) {
                            writes.push((wvalue(sel, bit), target.bit(sel, bit) as u16));
                        }
                    }
                }
                Param::Plain(sel) if self.get(sel) != target.get(sel) => {
                    writes.push((wvalue(sel, 0), target.get(sel)));
                }
                Param::Plain(_) => {}
                Param::Array(base, n) => {
                    for i in 0..n {
                        if self.get(base + i) != target.get(base + i) {
                            writes.push((wvalue(base, i), target.get(base + i)));
                        }
                    }
                }
            }
        }
        writes
    }

    /// Whether every setting is as it is after power-on.
    pub fn is_factory(&self) -> bool {
        self.writes_to(&Block::factory()).is_empty()
    }

    pub fn to_preset(&self) -> String {
        let mut text = format!("{}\n", PRESET_HEADER);
        for sel in PARAMS.iter().flat_map(Param::selectors) {
            text += &format!("{:02x} {:04x}\n", sel, self.get(sel));
        }
        text
    }

    /// Reads a preset; settings it leaves out keep their factory value.
    pub fn from_preset(text: &str) -> Result<Block, String> {
        let mut block = Block::factory();
        let stored: Vec<u8> = PARAMS.iter().flat_map(Param::selectors).collect();
        for (n, line) in text.lines().enumerate() {
            let line = line.split('#').next().unwrap().trim();
            if line.is_empty() {
                continue;
            }
            let bad = || format!("line {}: expected '<selector> <value>' in hex, got '{}'", n + 1, line);
            let mut words = line.split_whitespace();
            let (Some(sel), Some(value), None) = (words.next(), words.next(), words.next()) else {
                return Err(bad());
            };
            let sel = u8::from_str_radix(sel, 16).map_err(|_| bad())?;
            let value = u16::from_str_radix(value, 16).map_err(|_| bad())?;
            if !stored.contains(&sel) {
                return Err(format!("line {}: selector {:02x} is not a setting presets hold", n + 1, sel));
            }
            block.set(sel, value);
        }
        Ok(block)
    }

    pub fn bit(&self, selector: u8, bit: u8) -> bool {
        self.get(selector) & (1 << bit) != 0
    }

    /// Updates the block the way the device does when `value` is written to `wvalue`:
    /// bitmask parameters use the channel as the bit, the others as an array index.
    pub fn apply(&mut self, wvalue: u16, value: u16) {
        let (sel, ch) = ((wvalue >> 8) as u8, wvalue as u8);
        if sel == SEL_INST || (SEL_MUTE..SEL_LAYOUT_STEREO + 3).contains(&sel) {
            let mask = self.get(sel) & !(1 << ch) | ((value != 0) as u16) << ch;
            self.set(sel, mask);
        } else {
            self.set(sel.saturating_add(ch), value);
        }
    }

    /// Strips Control Center shows in the mixer of `bus`: single sources and stereo pairs.
    pub fn strips(&self, bus: u8) -> Vec<Vec<u8>> {
        let mut strips = Vec::new();
        for pair in 0..5u8 {
            let first = pair * 2;
            for src in [first, first + 1] {
                if src < 4 && self.bit(SEL_LAYOUT_MONO + bus, src) {
                    strips.push(vec![src]);
                }
            }
            if self.bit(SEL_LAYOUT_STEREO + bus, pair) {
                strips.push(vec![first, first + 1]);
            }
        }
        strips
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

    #[test]
    fn default_block_decodes() {
        let b = Block::factory();
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
    fn apply_matches_block_layout() {
        let mut b = Block::factory();
        b.apply(wvalue(SEL_MUTE, 1), 0); // unmute Analog 2 on Outputs 1&2
        assert_eq!(b.get(SEL_MUTE), 0xffcd);
        b.apply(wvalue(SEL_INST, 1), 1);
        assert_eq!(b.get(SEL_INST), 0b10);
        b.apply(wvalue(mix_selector(1, 0), 2), 0x1234);
        assert_eq!(b.get(0x1d + 2), 0x1234);
        b.apply(wvalue(SEL_SOURCE, 0), 0);
        assert_eq!(b.get(SEL_SOURCE), 0);
    }

    #[test]
    fn default_layout_strips() {
        let b = Block::factory();
        // Outputs 1&2 by default: Analog 1, Analog 2 and USB 1/2
        assert_eq!(b.strips(0), vec![vec![0], vec![1], vec![4, 5]]);
        assert_eq!(b.strips(2), vec![vec![8, 9]]);
    }

    #[test]
    fn adding_and_removing_strips() {
        let mut b = Block::factory();
        for w in strip_writes(1, &[2, 3], true) {
            b.apply(w.0, w.1);
        }
        // Stereo layout of Outputs 3&4 is selector 0x56, In 3/4 its bit 1
        assert_eq!(strip_writes(1, &[2, 3], true)[0], (0x5601, 1));
        assert!(b.strips(1).contains(&vec![2, 3]));
        assert_eq!(b.get(mix_selector(1, 1) + 2), OFF); // In 3 hard left
        for w in strip_writes(0, &[1], false) {
            b.apply(w.0, w.1);
        }
        assert!(!b.strips(0).contains(&vec![1]));
        assert!(b.bit(SEL_MUTE, 1));
    }

    #[test]
    fn presets_round_trip() {
        let mut b = Block::factory();
        assert!(b.is_factory());
        assert!(b.writes_to(&Block::factory()).is_empty());
        let mut writes = vec![(wvalue(SEL_INST, 1), 1), (wvalue(SEL_48V, 0), 1), (wvalue(SEL_INPUT_GAIN, 2), 0x0600)];
        writes.extend(strip_writes(1, &[2, 3], true));
        writes.push((wvalue(SEL_MUTE + 2, 9), 0));
        writes.push((wvalue(SEL_MASTER + 1, 0), 0xf000));
        writes.push((wvalue(SEL_SOURCE + 2, 0), 5));
        for &(w, v) in &writes {
            b.apply(w, v);
        }
        assert!(!b.is_factory());

        let loaded = Block::from_preset(&b.to_preset()).unwrap();
        assert!(loaded.writes_to(&b).is_empty());
        // Applying the writes to the factory block gives the preset back
        let mut f = Block::factory();
        for (w, v) in f.writes_to(&loaded) {
            f.apply(w, v);
        }
        assert!(f.writes_to(&b).is_empty() && b.writes_to(&f).is_empty());
    }

    #[test]
    fn preset_parsing() {
        let b = Block::from_preset("# comment\n\n03 0001  # 48V on\n4c ffcd\n").unwrap();
        assert_eq!(b.get(SEL_48V), 1);
        assert!(!b.bit(SEL_MUTE, 1));
        assert_eq!(b.get(SEL_SOURCE), 1); // left out: factory value
        assert!(Block::from_preset("03").is_err());
        assert!(Block::from_preset("zz 0001").is_err());
        assert!(Block::from_preset("05 0001").is_err()); // not a setting presets hold
    }

    #[test]
    fn meters_decode() {
        // Captured: guitar on Analog 2, mixed centred to Outputs 1&2 (-3 dB), nothing else playing
        const METERS: &str = "ff4b0200ffcfef0cff8b0000ff90000083070000a9070000000000000000000000000000000000008f4f26097f50260900000000000000000000000000000000";
        let bytes: Vec<u8> = (0..METERS.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&METERS[i..i + 2], 16).unwrap())
            .collect();
        let m = parse_meters(&bytes);
        assert!((m[1] - -19.9).abs() < 0.1, "{}", m[1]);
        assert!((m[1] - m[10] - 3.0).abs() < 0.1, "{}", m[10]);
        assert!((m[10] - m[11]).abs() < 0.01);
        assert_eq!(m[6], f64::NEG_INFINITY);
        assert_eq!(parse_meters(&[0xff, 0xff, 0xff, 0x7f])[0], 0.0);
    }

    #[test]
    fn stereo_pairs() {
        assert_eq!(parse_sources("in34"), Some(vec![2, 3]));
        assert_eq!(parse_sources("usb56"), Some(vec![8, 9]));
        assert_eq!(parse_sources("usb3"), Some(vec![6]));
        assert_eq!(parse_sources("in5"), None);
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
