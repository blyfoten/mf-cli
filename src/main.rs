mod device;
mod mf4;

use device::{Model, Transport};
use std::{env, process::exit};

const USAGE: &str = "\
Usage: mf-cli <command> [<command> ...]

All models:
  inst <on|off>                  Instrument mode (input 1)
  48v <on|off>                   Phantom power
  monitor <on|off>               Direct Mono (MiniFuse 1/2 only)
  raw <wvalue-hex> <value>       Send a raw parameter write

MiniFuse 4:
  inst2 <on|off>                 Instrument mode on input 2
  gain <in3|in4> <dB>            Software gain of line inputs 3/4 (-24 .. +24)
  link34 <on|off>                Link the gain of inputs 3/4
  mute <source> <bus> <on|off>   Mute a source in a mixer
  solo <source> <bus> <on|off>   Solo a source in a mixer
  level <source> <bus> <dB|off> [pan <-100..100|L|C|R>]
                                 Fader (+12 .. -60) and balance of a source in a mixer
  master <bus> <dB>              Master level of a mixer (-64 .. 0)
  source <out12|out34> <mixer|direct>
  source loop <usb56|cue2mix|out12|out34>
  status                         Show the current settings
  dump                           Show the raw settings block

  <source> is in1..in4 or usb1..usb6, or a stereo pair in12, in34, usb12, usb34
  or usb56 (both channels; each keeps its own balance). <bus> is out12, out34 or loop.

Examples:
  mf-cli inst on
  mf-cli 48v on monitor off
  mf-cli mute in1 out12 off level in1 out12 -6 pan C
";

enum Kind {
    /// On/off writes, which every kernel module version can send.
    Toggle { wvalues: Vec<u16>, on: bool },
    Link34(bool),
    InputGain { input: u8, raw: u16 },
    Level { bus: u8, srcs: Vec<u8>, level: Option<f64>, pan: Option<f64> },
    Write { wvalue: u16, value: u16 },
    Status,
    Dump,
}

struct Cmd {
    label: String,
    kind: Kind,
}

impl Cmd {
    fn needs_block(&self) -> bool {
        matches!(
            self.kind,
            Kind::Link34(true) | Kind::InputGain { .. } | Kind::Level { pan: None, .. } | Kind::Status | Kind::Dump
        )
    }

    fn needs_full_api(&self) -> bool {
        !matches!(self.kind, Kind::Toggle { .. } | Kind::Link34(false))
    }

    fn writes(&self, block: Option<&mf4::Block>) -> Vec<(u16, u16)> {
        let block = || block.expect("settings block was read");
        match &self.kind {
            Kind::Toggle { wvalues, on } => wvalues.iter().map(|&w| (w, *on as u16)).collect(),
            Kind::Link34(on) => {
                let mut w = vec![(mf4::wvalue(mf4::SEL_LINK34, 0), *on as u16)];
                if *on {
                    // Like Control Center: input 4 takes over the gain of input 3.
                    let in3 = block().get(mf4::SEL_INPUT_GAIN + 2);
                    w.push((mf4::wvalue(mf4::SEL_INPUT_GAIN, 3), in3));
                }
                w
            }
            Kind::InputGain { input, raw } => {
                let mut w = vec![(mf4::wvalue(mf4::SEL_INPUT_GAIN, *input), *raw)];
                // The device does not link the gains itself; Control Center writes both.
                if block().get(mf4::SEL_LINK34) != 0 {
                    w.push((mf4::wvalue(mf4::SEL_INPUT_GAIN, input ^ 1), *raw));
                }
                w
            }
            Kind::Level { bus, srcs, level, pan } => srcs
                .iter()
                .flat_map(|&src| {
                    // Each channel keeps its own balance unless one is given.
                    let pan = pan.unwrap_or_else(|| block().strip(*bus, src).1);
                    let (left, right) = mf4::mix_gains(*level, pan);
                    [
                        (mf4::wvalue(mf4::mix_selector(*bus, 0), src), left),
                        (mf4::wvalue(mf4::mix_selector(*bus, 1), src), right),
                    ]
                })
                .collect(),
            Kind::Write { wvalue, value } => vec![(*wvalue, *value)],
            Kind::Status | Kind::Dump => vec![],
        }
    }
}

struct Parser<'a> {
    args: &'a [String],
    pos: usize,
    model: Model,
}

impl<'a> Parser<'a> {
    fn next(&mut self, what: &str) -> Result<&'a str, String> {
        let word = self.args.get(self.pos).ok_or_else(|| {
            format!("'{}' is missing {}", self.args[..self.pos].join(" "), what)
        })?;
        self.pos += 1;
        Ok(word)
    }

    fn peek_is(&self, word: &str) -> bool {
        self.args.get(self.pos).map(String::as_str) == Some(word)
    }

    fn on_off(&mut self) -> Result<bool, String> {
        match self.next("on|off")? {
            "on" => Ok(true),
            "off" => Ok(false),
            s => Err(format!("Unknown state '{}'. Use 'on' or 'off'.", s)),
        }
    }

    fn db(&mut self, min: f64, max: f64) -> Result<f64, String> {
        let s = self.next("a level in dB")?;
        let num = s.strip_suffix("dB").or(s.strip_suffix("db")).unwrap_or(s);
        match num.parse::<f64>() {
            Ok(db) if (min..=max).contains(&db) => Ok(db),
            Ok(_) => Err(format!("Level '{}' is outside {} .. {} dB", s, min, max)),
            Err(_) => Err(format!("'{}' is not a level in dB", s)),
        }
    }

    fn pan(&mut self) -> Result<f64, String> {
        let s = self.next("a balance")?;
        match s {
            "L" | "l" => Ok(-100.0),
            "C" | "c" => Ok(0.0),
            "R" | "r" => Ok(100.0),
            _ => s
                .parse::<f64>()
                .ok()
                .filter(|p| (-100.0..=100.0).contains(p))
                .ok_or_else(|| format!("Balance '{}' must be -100..100, L, C or R", s)),
        }
    }

    fn sources(&mut self) -> Result<Vec<u8>, String> {
        let s = self.next("a source")?;
        mf4::parse_sources(s).ok_or_else(|| {
            format!("Unknown source '{}'. Use in1..in4, usb1..usb6 or a pair such as in12 or usb34.", s)
        })
    }

    fn bus(&mut self) -> Result<u8, String> {
        let s = self.next("a bus")?;
        mf4::parse_bus(s).ok_or_else(|| format!("Unknown bus '{}'. Use out12, out34 or loop.", s))
    }

    fn require_mf4(&self, word: &str) -> Result<(), String> {
        if self.model == Model::Mf4 {
            Ok(())
        } else {
            Err(format!("'{}' is only supported on the MiniFuse 4", word))
        }
    }

    fn command(&mut self) -> Result<Cmd, String> {
        let start = self.pos;
        let word = self.next("a command")?;
        let mf4 = self.model == Model::Mf4;
        let kind = match word {
            "inst" | "inst1" => Kind::Toggle { wvalues: vec![mf4::wvalue(mf4::SEL_INST, 0)], on: self.on_off()? },
            "inst2" => {
                self.require_mf4(word)?;
                Kind::Toggle { wvalues: vec![mf4::wvalue(mf4::SEL_INST, 1)], on: self.on_off()? }
            }
            "48v" => Kind::Toggle {
                wvalues: vec![if mf4 { mf4::wvalue(mf4::SEL_48V, 0) } else { 0x0400 }],
                on: self.on_off()?,
            },
            "monitor" => {
                if mf4 {
                    return Err("'monitor' (Direct Mono) is not known on the MiniFuse 4; use the mixer commands".into());
                }
                Kind::Toggle { wvalues: vec![0x0500], on: self.on_off()? }
            }
            "link34" => {
                self.require_mf4(word)?;
                Kind::Link34(self.on_off()?)
            }
            "gain" => {
                self.require_mf4(word)?;
                let input = match self.next("in3|in4")? {
                    "in3" => 2,
                    "in4" => 3,
                    s => return Err(format!("Only in3 and in4 have software gain, not '{}'", s)),
                };
                Kind::InputGain { input, raw: mf4::db_to_raw(Some(self.db(-24.0, 24.0)?)) }
            }
            "mute" | "solo" => {
                self.require_mf4(word)?;
                let (srcs, bus) = (self.sources()?, self.bus()?);
                let base = if word == "mute" { mf4::SEL_MUTE } else { mf4::SEL_SOLO };
                let wvalues = srcs.iter().map(|&src| mf4::wvalue(base + bus, src)).collect();
                Kind::Toggle { wvalues, on: self.on_off()? }
            }
            "level" => {
                self.require_mf4(word)?;
                let (srcs, bus) = (self.sources()?, self.bus()?);
                let level = if self.peek_is("off") {
                    self.pos += 1;
                    None
                } else {
                    Some(self.db(-60.0, 12.0)?)
                };
                let pan = if self.peek_is("pan") {
                    if srcs.len() > 1 {
                        return Err("Set the balance of each channel of a stereo pair separately, e.g. 'level usb1 out12 0 pan L'".into());
                    }
                    self.pos += 1;
                    Some(self.pan()?)
                } else {
                    None
                };
                Kind::Level { bus, srcs, level, pan }
            }
            "master" => {
                self.require_mf4(word)?;
                let bus = self.bus()?;
                Kind::Write {
                    wvalue: mf4::wvalue(mf4::SEL_MASTER + bus, 0),
                    value: mf4::db_to_raw(Some(self.db(-64.0, 0.0)?)),
                }
            }
            "source" => {
                self.require_mf4(word)?;
                let bus = self.bus()?;
                let s = self.next("a source")?;
                let value = mf4::parse_output_source(bus, s).ok_or_else(|| {
                    format!("Unknown source '{}' for {}", s, mf4::BUSES[bus as usize])
                })?;
                Kind::Write { wvalue: mf4::wvalue(mf4::SEL_SOURCE + bus, 0), value }
            }
            "status" => {
                self.require_mf4(word)?;
                Kind::Status
            }
            "dump" => {
                self.require_mf4(word)?;
                Kind::Dump
            }
            "raw" => {
                let w = self.next("a wValue in hex")?;
                let wvalue = u16::from_str_radix(w.trim_start_matches("0x"), 16)
                    .map_err(|_| format!("'{}' is not a hex wValue", w))?;
                let v = self.next("a value")?;
                let value = match v.strip_prefix("0x") {
                    Some(hex) => i32::from_str_radix(hex, 16).ok(),
                    None => v.parse::<i32>().ok(),
                }
                .filter(|n| (-32768..=65535).contains(n))
                .ok_or_else(|| format!("'{}' is not a 16-bit value", v))?;
                Kind::Write { wvalue, value: value as u16 }
            }
            _ => return Err(format!("Unknown command '{}'. Run 'mf-cli --help' for usage.", word)),
        };
        Ok(Cmd { label: self.args[start..self.pos].join(" "), kind })
    }
}

fn die(msg: &str) -> ! {
    eprintln!("Error: {}", msg);
    exit(1);
}

fn main() {
    let args: Vec<String> = env::args().skip(1).collect();
    if args.is_empty() || args[0] == "-h" || args[0] == "--help" {
        eprint!("{}", USAGE);
        exit(if args.is_empty() { 1 } else { 0 });
    }

    let model = device::detect_model().unwrap_or_else(|| die("No MiniFuse device found"));

    let mut parser = Parser { args: &args, pos: 0, model };
    let mut commands = Vec::new();
    while parser.pos < args.len() {
        commands.push(parser.command().unwrap_or_else(|e| die(&e)));
    }

    let full_api = commands.iter().any(|c| c.needs_full_api() || c.needs_block());
    let mut transport = Transport::open(full_api).unwrap_or_else(|e| die(&e));
    println!("[*] Found {}... applying settings", model.name());

    let mut block: Option<mf4::Block> = None;
    for cmd in &commands {
        if cmd.needs_block() && block.is_none() {
            let data = transport.read_block().unwrap_or_else(|e| die(&e));
            block = Some(mf4::Block::new(data));
        }
        match cmd.kind {
            Kind::Status => block.as_ref().unwrap().print_status(),
            Kind::Dump => block.as_ref().unwrap().dump(),
            _ => {
                for (wvalue, value) in cmd.writes(block.as_ref()) {
                    transport.write(wvalue, value).unwrap_or_else(|e| die(&e));
                }
                println!("[+] {}", cmd.label);
                // Later commands that read settings must see this change.
                block = None;
            }
        }
    }

    transport.finish();
}
