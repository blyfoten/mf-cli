//! Mixer window for the MiniFuse 4, laid out like MiniFuse Control Center.
//!
//! A worker thread owns the device: it sends the writes the window queues, reads the level
//! meters every METER_POLL and the settings block every BLOCK_POLL, so changes from the front
//! panel or mf-cli show up too.

use eframe::egui::{self, Color32, RichText};
use mf_cli::device::{self, Model, Transport};
use mf_cli::mf4::{self, Block};
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender, channel};
use std::thread;
use std::time::{Duration, Instant};

const BLOCK_POLL: Duration = Duration::from_millis(250);
const METER_POLL: Duration = Duration::from_millis(40);
/// How fast the meters fall back after a peak, in dB per second.
const METER_FALL: f64 = 30.0;

const ACCENT: Color32 = Color32::from_rgb(0x00, 0xc8, 0xe6);
const MUTE: Color32 = Color32::from_rgb(0xd0, 0x30, 0x30);
const SOLO: Color32 = Color32::from_rgb(0xd0, 0xa0, 0x20);
const OFF_FILL: Color32 = Color32::from_gray(0x48);
const METER_LOW: Color32 = Color32::from_rgb(0x30, 0xc0, 0x50);
const METER_MID: Color32 = Color32::from_rgb(0xe0, 0xc0, 0x30);
const METER_HIGH: Color32 = Color32::from_rgb(0xe0, 0x40, 0x30);

/// Fader positions below FADER_MIN are shown and written as off.
const FADER_MIN: f64 = -60.0;
const FADER_OFF: f64 = -66.0;
const FADER_HEIGHT: f32 = 220.0;
const KNOB: f32 = 34.0;
const METER_W: f32 = 5.0;
const METER_GAP: f32 = 2.0;

const MIX_TICKS: [f64; 9] = [12.0, 6.0, 0.0, -6.0, -12.0, -24.0, -36.0, -48.0, -60.0];
const MASTER_TICKS: [f64; 7] = [0.0, -6.0, -12.0, -24.0, -36.0, -48.0, -64.0];
const INPUT_TICKS: [f64; 10] = [0.0, -6.0, -12.0, -18.0, -24.0, -30.0, -36.0, -42.0, -48.0, -60.0];

/// Writes (wValue, value) tagged with a sequence number.
type Batch = (u64, Vec<(u16, u16)>);

enum Update {
    /// The settings block, read after all writes up to the sequence number were sent.
    Block(u64, Block),
    Meters([f64; 16]),
    Error(String),
}

fn worker(mut transport: Transport, rx: Receiver<Batch>, tx: Sender<Update>, ctx: egui::Context) {
    let meters = transport.has_meters();
    let mut done = 0;
    let mut next_block = Instant::now();
    loop {
        let wait = if meters {
            METER_POLL
        } else {
            next_block.saturating_duration_since(Instant::now())
        };
        let mut updates = Vec::new();
        match rx.recv_timeout(wait) {
            Ok(batch) => {
                // A dragged fader queues many writes; only the latest value of each matters
                let mut pending: Vec<(u16, u16)> = Vec::new();
                for (seq, writes) in std::iter::once(batch).chain(rx.try_iter()) {
                    done = seq;
                    for (w, v) in writes {
                        match pending.iter_mut().find(|(pw, _)| *pw == w) {
                            Some(p) => p.1 = v,
                            None => pending.push((w, v)),
                        }
                    }
                }
                for (w, v) in pending {
                    if let Err(e) = transport.write(w, v) {
                        updates.push(Update::Error(e));
                    }
                }
                next_block = Instant::now();
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => break,
        }

        if meters {
            updates.push(match transport.read_meters() {
                Ok(data) => Update::Meters(mf4::parse_meters(&data)),
                Err(e) => Update::Error(e),
            });
        }
        if Instant::now() >= next_block {
            updates.push(match transport.read_block() {
                Ok(data) => Update::Block(done, Block::new(data)),
                Err(e) => Update::Error(e),
            });
            next_block = Instant::now() + BLOCK_POLL;
        }
        if updates.into_iter().any(|u| tx.send(u).is_err()) {
            break;
        }
        ctx.request_repaint();
    }
    transport.finish();
}

#[derive(PartialEq)]
enum Tab {
    Inputs,
    Outputs,
}

struct App {
    tx: Sender<Batch>,
    rx: Receiver<Update>,
    /// The last block read, with the writes sent since applied to it.
    block: Option<Block>,
    seq: u64,
    /// Meters as shown: the peaks read, falling back at METER_FALL.
    meters: [f64; 16],
    meter_time: Instant,
    has_meters: bool,
    error: Option<String>,
    kmod: bool,
    tab: Tab,
    bus: u8,
    show_all: bool,
}

impl App {
    fn send(&mut self, writes: Vec<(u16, u16)>) {
        if writes.is_empty() {
            return;
        }
        if let Some(b) = &mut self.block {
            for &(w, v) in &writes {
                b.apply(w, v);
            }
        }
        self.seq += 1;
        let _ = self.tx.send((self.seq, writes));
    }

    fn receive(&mut self) {
        while let Ok(update) = self.rx.try_recv() {
            match update {
                // Older reads would undo changes the device has not seen yet
                Update::Block(done, b) if done >= self.seq => {
                    self.block = Some(b);
                    self.error = None;
                }
                Update::Block(..) => {}
                Update::Meters(m) => {
                    let fall = self.meter_time.elapsed().as_secs_f64() * METER_FALL;
                    self.meter_time = Instant::now();
                    for (shown, new) in self.meters.iter_mut().zip(m) {
                        *shown = new.max(*shown - fall);
                    }
                }
                Update::Error(e) => self.error = Some(e),
            }
        }
    }

    /// Meters to draw, or none when they cannot be read.
    fn meters_of(&self, idx: &[usize]) -> Vec<f64> {
        if self.has_meters { idx.iter().map(|&i| self.meters[i]).collect() } else { Vec::new() }
    }
}

fn toggle(ui: &mut egui::Ui, on: bool, text: &str, color: Color32, size: egui::Vec2) -> bool {
    let fill = if on { color } else { OFF_FILL };
    let text = RichText::new(text).color(if on { Color32::WHITE } else { Color32::LIGHT_GRAY });
    ui.add(egui::Button::new(text).fill(fill).min_size(size)).clicked()
}

fn db_text(db: Option<f64>) -> String {
    db.map_or("off".into(), |d| format!("{:+.1} dB", d))
}

fn pan_text(pan: f64) -> String {
    match pan.round() {
        p if p.abs() < 1.0 => "C".into(),
        p if p < 0.0 => format!("L{}", -p),
        p => format!("R{}", p),
    }
}

fn strip_name(srcs: &[u8]) -> String {
    let (kind, n) = if srcs[0] < 4 { ("Analog", 1) } else { ("USB", -3) };
    let num = |s: u8| s as i32 + n;
    match srcs {
        [a] => format!("{} {}", kind, num(*a)),
        [a, b, ..] => format!("{} {}/{}", kind, num(*a), num(*b)),
        [] => String::new(),
    }
}

/// Lays out a row of `width` centred in the available width.
fn centered_row<R>(ui: &mut egui::Ui, width: f32, add: impl FnOnce(&mut egui::Ui) -> R) -> R {
    ui.horizontal(|ui| {
        ui.add_space(((ui.available_width() - width) / 2.0).max(0.0));
        add(ui)
    })
    .inner
}

fn meters_width(n: usize) -> f32 {
    n as f32 * (METER_W + METER_GAP)
}

/// Draws a dB scale right-aligned in `scale` and level meters in `bars`, placing `db` at `y(db)`.
/// Meters below `floor` are empty; `off` labels the bottom of a fader that can be turned off.
fn draw_scale(
    ui: &egui::Ui,
    scale: egui::Rect,
    bars: egui::Rect,
    ticks: &[f64],
    meters: &[f64],
    floor: f64,
    off: Option<f64>,
    y: impl Fn(f64) -> f32,
) {
    let painter = ui.painter();
    let color = Color32::from_gray(0x90);
    let font = egui::FontId::proportional(10.0);
    let marks = ticks.iter().map(|&t| (t, format!("{}", t))).chain(off.map(|o| (o, "∞".into())));
    for (t, text) in marks {
        let pos = egui::pos2(scale.right() - 6.0, y(t));
        painter.text(pos, egui::Align2::RIGHT_CENTER, text, font.clone(), color);
        painter.line_segment([pos + egui::vec2(2.0, 0.0), pos + egui::vec2(6.0, 0.0)], (1.0, color));
    }

    // Green up to -12 dBFS, yellow up to -3, red above
    let bottom = y(floor);
    for (k, &db) in meters.iter().enumerate() {
        let x = bars.left() + k as f32 * (METER_W + METER_GAP);
        let column = |top: f32, bottom: f32| egui::Rect::from_x_y_ranges(x..=x + METER_W, top..=bottom);
        painter.rect_filled(column(y(0.0), bottom), 1.0, Color32::from_gray(0x10));
        let mut from = floor;
        for (to, c) in [(-12.0, METER_LOW), (-3.0, METER_MID), (0.0, METER_HIGH)] {
            let top = db.min(to);
            if top > from {
                painter.rect_filled(column(y(top), y(from)), 0.0, c);
            }
            from = to;
        }
    }
}

/// A vertical fader from `min` to `max` dB with a scale at `ticks` and level meters beside it;
/// returns the new level when it was moved. With `off`, the bottom of the fader is off.
fn fader(
    ui: &mut egui::Ui,
    level: Option<f64>,
    (min, max): (f64, f64),
    off: bool,
    ticks: &[f64],
    meters: &[f64],
) -> Option<Option<f64>> {
    let bottom = if off { FADER_OFF } else { min };
    let mut v = level.unwrap_or(bottom).clamp(bottom, max);
    let gap = ui.spacing().item_spacing.x;
    let width = 30.0 + meters_width(meters.len()) + 18.0 + 2.0 * gap;
    let resp = centered_row(ui, width, |ui| {
        let size = |w| egui::vec2(w, FADER_HEIGHT);
        let (scale, _) = ui.allocate_exact_size(size(30.0), egui::Sense::hover());
        let (bars, _) = ui.allocate_exact_size(size(meters_width(meters.len())), egui::Sense::hover());
        ui.spacing_mut().slider_width = FADER_HEIGHT;
        let slider = egui::Slider::new(&mut v, bottom..=max)
            .vertical()
            .show_value(false)
            .trailing_fill(true)
            .step_by(0.5);
        let resp = ui.add(slider).on_hover_text("Double-click for 0 dB");

        // The handle centre travels the slider height less its radius at each end
        let radius = resp.rect.width() / 2.5;
        let rect = resp.rect;
        let y = |db: f64| {
            let t = ((db.clamp(bottom, max) - bottom) / (max - bottom)) as f32;
            rect.bottom() - radius - t * (rect.height() - 2.0 * radius)
        };
        draw_scale(ui, scale, bars, ticks, meters, bottom, off.then_some(bottom), y);
        resp
    });
    if resp.double_clicked() {
        return Some(Some(0.0f64.clamp(min, max)));
    }
    resp.changed().then(|| (!off || v >= FADER_MIN).then_some(v))
}

/// A knob drawn like Control Center's, with an arc from 0 to the value: drag it, or
/// double-click to return to 0. `max` must be positive and `min` its negative.
fn knob(ui: &mut egui::Ui, value: &mut f64, max: f64, size: f32, hover: String) -> egui::Response {
    let (rect, mut resp) = ui.allocate_exact_size(egui::vec2(size, size), egui::Sense::click_and_drag());
    if resp.dragged() {
        let d = resp.drag_delta();
        *value = (*value + (d.x - d.y) as f64 * max / 66.0).clamp(-max, max);
        resp.mark_changed();
    }
    if resp.double_clicked() {
        *value = 0.0;
        resp.mark_changed();
    }

    // -135° is the minimum, +135° the maximum, measured clockwise from straight up
    let c = rect.center();
    let r = size / 2.0 - 4.0;
    let at = |deg: f64, rad: f32| {
        let a = deg.to_radians() as f32;
        c + egui::vec2(a.sin() * rad, -a.cos() * rad)
    };
    let arc = |from: f64, to: f64| -> Vec<egui::Pos2> {
        (0..=24).map(|i| at(from + (to - from) * i as f64 / 24.0, r + 2.5)).collect()
    };
    let angle = *value / max * 135.0;
    let painter = ui.painter();
    painter.add(egui::Shape::line(arc(-135.0, 135.0), egui::Stroke::new(3.0, Color32::from_gray(0x40))));
    painter.add(egui::Shape::line(arc(0.0, angle), egui::Stroke::new(3.0, ACCENT)));
    painter.circle_filled(c, r - 1.0, Color32::from_gray(0x2a));
    painter.line_segment([c, at(angle, r - 2.0)], egui::Stroke::new(2.0, Color32::WHITE));
    resp.on_hover_text(hover)
}

const PANEL_W: f32 = 180.0;
const PANEL_H: f32 = 400.0;
const INPUT_METER_H: f32 = 260.0;
const GAP_W: f32 = 64.0;

impl App {
    fn inputs(&mut self, ui: &mut egui::Ui, b: &Block) {
        let mut writes = Vec::new();
        ui.vertical_centered(|ui| ui.heading("ANALOG INPUTS"));
        ui.add_space(8.0);
        let btn = egui::vec2(56.0, 26.0);
        // Room on the sides and between the pairs for the buttons shared by two inputs
        let total = 4.0 * PANEL_W + 2.0 * GAP_W + 20.0 + 6.0 * ui.spacing().item_spacing.x + 24.0;
        centered_row(ui, total, |ui| {
            for i in 0..4u8 {
                self.input_panel(ui, b, i, &mut writes);
                let shared = |ui: &mut egui::Ui, add: &mut dyn FnMut(&mut egui::Ui)| {
                    let layout = egui::Layout::bottom_up(egui::Align::Center);
                    ui.allocate_ui_with_layout(egui::vec2(GAP_W, PANEL_H), layout, |ui| {
                        ui.add_space(22.0);
                        add(ui);
                    });
                };
                match i {
                    0 => shared(ui, &mut |ui| {
                        let on = b.get(mf4::SEL_48V) != 0;
                        if toggle(ui, on, "+48V", MUTE, btn) {
                            writes.push((mf4::wvalue(mf4::SEL_48V, 0), !on as u16));
                        }
                    }),
                    2 => shared(ui, &mut |ui| {
                        let on = b.get(mf4::SEL_LINK34) != 0;
                        if toggle(ui, on, "LINK", ACCENT, btn) {
                            writes.push((mf4::wvalue(mf4::SEL_LINK34, 0), !on as u16));
                            if !on {
                                // Like Control Center: input 4 takes over the gain of input 3
                                let in3 = b.get(mf4::SEL_INPUT_GAIN + 2);
                                writes.push((mf4::wvalue(mf4::SEL_INPUT_GAIN, 3), in3));
                            }
                        }
                    }),
                    1 => ui.add_space(20.0),
                    _ => {}
                }
            }
        });
        self.send(writes);
    }

    fn input_panel(&self, ui: &mut egui::Ui, b: &Block, input: u8, writes: &mut Vec<(u16, u16)>) {
        egui::Frame::group(ui.style()).show(ui, |ui| {
            ui.set_width(PANEL_W);
            ui.set_height(PANEL_H);
            ui.vertical_centered(|ui| {
                ui.label(RichText::new(format!("Input {}", input + 1)).size(17.0));
                ui.small(if input < 2 { "XLR / jack combo" } else { "line jack" });
                ui.add_space(10.0);

                let meters = self.meters_of(&[input as usize]);
                let width = 30.0 + meters_width(meters.len().max(1)) + ui.spacing().item_spacing.x;
                centered_row(ui, width, |ui| {
                    let size = |w| egui::vec2(w, INPUT_METER_H);
                    let (scale, _) = ui.allocate_exact_size(size(30.0), egui::Sense::hover());
                    let (bars, _) = ui.allocate_exact_size(size(meters_width(1)), egui::Sense::hover());
                    let y = |db: f64| {
                        let t = (db.clamp(-60.0, 0.0) / -60.0) as f32;
                        bars.top() + 4.0 + t * (bars.height() - 8.0)
                    };
                    draw_scale(ui, scale, bars, &INPUT_TICKS, &meters, -60.0, None, y);
                });
                ui.add_space(12.0);

                if input < 2 {
                    ui.add_space(12.0);
                    let on = b.bit(mf4::SEL_INST, input);
                    if toggle(ui, on, "INST", ACCENT, egui::vec2(64.0, 28.0)) {
                        writes.push((mf4::wvalue(mf4::SEL_INST, input), !on as u16));
                    }
                } else {
                    let raw = b.get(mf4::SEL_INPUT_GAIN + input);
                    let mut gain = mf4::raw_to_db(raw).unwrap_or(0.0);
                    let hover = format!("Gain {:+.1} dB (double-click for 0 dB)", gain);
                    let resp = centered_row(ui, 44.0, |ui| knob(ui, &mut gain, 24.0, 44.0, hover));
                    if resp.changed() {
                        let raw = mf4::db_to_raw(Some(gain));
                        writes.push((mf4::wvalue(mf4::SEL_INPUT_GAIN, input), raw));
                        // The device does not link the gains itself; Control Center writes both
                        if b.get(mf4::SEL_LINK34) != 0 {
                            writes.push((mf4::wvalue(mf4::SEL_INPUT_GAIN, input ^ 1), raw));
                        }
                    }
                    ui.small(format!("{:+.1} dB", gain));
                }
            });
        });
    }

    fn bus_card(&mut self, ui: &mut egui::Ui, b: &Block, bus: u8) {
        let mut writes = Vec::new();
        let selected = self.bus == bus;
        let stroke = if selected { egui::Stroke::new(2.0, ACCENT) } else { egui::Stroke::NONE };
        let resp = egui::Frame::group(ui.style()).stroke(stroke).show(ui, |ui| {
            ui.set_width(170.0);
            let title = ["OUTPUTS 1 & 2", "OUTPUTS 3 & 4", "LOOPBACK"][bus as usize];
            let color = if selected { ACCENT } else { Color32::WHITE };
            if ui.add(egui::Label::new(RichText::new(title).color(color).strong()).sense(egui::Sense::click())).clicked() {
                self.bus = bus;
            }
            let source = b.get(mf4::SEL_SOURCE + bus);
            if bus < 2 {
                // Direct routes USB 1/2 (or 3/4) to the outputs and bypasses the mixer
                let direct = source == bus as u16 * 2;
                let text = if bus == 0 { "USB1/2 direct" } else { "USB3/4 direct" };
                if toggle(ui, direct, text, ACCENT, egui::vec2(150.0, 24.0)) {
                    let value = bus as u16 * 2 + direct as u16;
                    writes.push((mf4::wvalue(mf4::SEL_SOURCE + bus, 0), value));
                }
            } else {
                let names = ["USB5/6", "Cue2Mix", "OUT1/2", "OUT3/4"];
                let current = names.get(source.wrapping_sub(4) as usize).copied().unwrap_or("?");
                egui::ComboBox::from_id_salt("loopback")
                    .selected_text(current)
                    .show_ui(ui, |ui| {
                        for (i, name) in names.iter().enumerate() {
                            let value = 4 + i as u16;
                            if ui.selectable_label(source == value, *name).clicked() {
                                writes.push((mf4::wvalue(mf4::SEL_SOURCE + bus, 0), value));
                            }
                        }
                    });
            }
            if ui.button("Edit mixer ▶").clicked() {
                self.bus = bus;
            }
        });
        if resp.response.interact(egui::Sense::click()).clicked() {
            self.bus = bus;
        }
        self.send(writes);
    }

    /// The "+" menu that adds inputs and computer sources to the mixer, mono or as pairs.
    fn add_menu(&mut self, ui: &mut egui::Ui, b: &Block) {
        let bus = self.bus;
        let used: Vec<u8> = b.strips(bus).concat();
        let mut add = None;
        ui.add_space(30.0);
        ui.menu_button(RichText::new(" + ").size(28.0), |ui| {
            ui.label(RichText::new("ADD CHANNELS TO MIXER").color(ACCENT));
            let mut option = |ui: &mut egui::Ui, srcs: Vec<u8>, text: &str| {
                let free = srcs.iter().all(|s| !used.contains(s));
                let btn = egui::Button::new(text).min_size(egui::vec2(56.0, 28.0));
                if ui.add_enabled(free, btn).clicked() {
                    add = Some(srcs);
                    ui.close_menu();
                }
            };
            ui.separator();
            ui.label("Inputs");
            ui.horizontal(|ui| (0..4).for_each(|s| option(ui, vec![s], &(s + 1).to_string())));
            ui.horizontal(|ui| {
                option(ui, vec![0, 1], "1/2");
                option(ui, vec![2, 3], "3/4");
            });
            ui.separator();
            ui.label("Computer sources");
            ui.horizontal(|ui| {
                option(ui, vec![4, 5], "1/2");
                option(ui, vec![6, 7], "3/4");
                option(ui, vec![8, 9], "5/6");
            });
        })
        .response
        .on_hover_text("Add channels to this mixer");
        if let Some(srcs) = add {
            self.send(mf4::strip_writes(bus, &srcs, true));
        }
    }

    fn strip(&mut self, ui: &mut egui::Ui, b: &Block, srcs: &[u8], removable: bool) {
        let bus = self.bus;
        let mut writes = Vec::new();
        egui::Frame::group(ui.style()).show(ui, |ui| {
            ui.set_width(110.0);
            ui.vertical_centered(|ui| {
                ui.horizontal(|ui| {
                    ui.label(RichText::new(strip_name(srcs)).strong());
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let remove = egui::Button::new(RichText::new("×").size(16.0)).frame(false);
                        if removable && ui.add(remove).on_hover_text("Remove from this mixer (mutes it)").clicked() {
                            writes.extend(mf4::strip_writes(bus, srcs, false));
                        }
                    });
                });
                ui.add_space(4.0);

                // Each channel of a pair keeps its own balance, as in Control Center
                let gains: Vec<(Option<f64>, f64)> = srcs.iter().map(|&s| b.strip(bus, s)).collect();
                let gap = ui.spacing().item_spacing.x;
                let width = srcs.len() as f32 * (KNOB + gap) - gap;
                centered_row(ui, width, |ui| {
                    for (&src, &(level, pan)) in srcs.iter().zip(&gains) {
                        let mut p = pan;
                        let hover = format!("Balance {} (double-click to centre)", pan_text(pan));
                        if knob(ui, &mut p, 100.0, KNOB, hover).changed() && level.is_some() {
                            let (l, r) = mf4::mix_gains(level, p);
                            writes.push((mf4::wvalue(mf4::mix_selector(bus, 0), src), l));
                            writes.push((mf4::wvalue(mf4::mix_selector(bus, 1), src), r));
                        }
                    }
                });
                let pans: Vec<String> = gains.iter().map(|&(_, pan)| pan_text(pan)).collect();
                ui.small(pans.join("   "));

                let small = egui::vec2(28.0, 24.0);
                centered_row(ui, 2.0 * small.x + gap, |ui| {
                    for (base, text, color) in [(mf4::SEL_MUTE, "M", MUTE), (mf4::SEL_SOLO, "S", SOLO)] {
                        let on = b.bit(base + bus, srcs[0]);
                        if toggle(ui, on, text, color, small) {
                            writes.extend(srcs.iter().map(|&s| (mf4::wvalue(base + bus, s), !on as u16)));
                        }
                    }
                });

                let level = gains[0].0;
                let idx: Vec<usize> = srcs.iter().map(|&s| s as usize).collect();
                let meters = self.meters_of(&idx);
                if let Some(new) = fader(ui, level, (FADER_MIN, 12.0), true, &MIX_TICKS, &meters) {
                    for (&src, &(_, pan)) in srcs.iter().zip(&gains) {
                        let (l, r) = mf4::mix_gains(new, pan);
                        writes.push((mf4::wvalue(mf4::mix_selector(bus, 0), src), l));
                        writes.push((mf4::wvalue(mf4::mix_selector(bus, 1), src), r));
                    }
                }
                ui.label(db_text(level));
            });
        });
        self.send(writes);
    }

    fn master(&mut self, ui: &mut egui::Ui, b: &Block) {
        let bus = self.bus;
        egui::Frame::group(ui.style()).show(ui, |ui| {
            ui.set_width(110.0);
            ui.vertical_centered(|ui| {
                ui.label(RichText::new("🔊 Master").strong());
                // Line up the fader with the strips, which have knobs and buttons above theirs
                ui.add_space(KNOB + 48.0);
                let level = mf4::raw_to_db(b.get(mf4::SEL_MASTER + bus));
                let out = 10 + 2 * bus as usize;
                let meters = self.meters_of(&[out, out + 1]);
                if let Some(Some(db)) = fader(ui, level, (-64.0, 0.0), false, &MASTER_TICKS, &meters) {
                    let w = (mf4::wvalue(mf4::SEL_MASTER + bus, 0), mf4::db_to_raw(Some(db)));
                    self.send(vec![w]);
                }
                ui.label(db_text(level));
            });
        });
    }

    fn outputs(&mut self, ui: &mut egui::Ui, b: &Block) {
        egui::SidePanel::left("buses").resizable(false).show_inside(ui, |ui| {
            for bus in 0..3 {
                self.bus_card(ui, b, bus);
                ui.add_space(6.0);
            }
            ui.add_space(6.0);
            ui.checkbox(&mut self.show_all, "Show all sources")
                .on_hover_text("Also show strips Control Center has removed from this mixer");
        });
        egui::SidePanel::right("master").resizable(false).show_inside(ui, |ui| self.master(ui, b));
        egui::CentralPanel::default().show_inside(ui, |ui| {
            let source = b.get(mf4::SEL_SOURCE + self.bus);
            if self.bus < 2 && source == self.bus as u16 * 2 {
                ui.colored_label(SOLO, "These outputs play USB directly; the mixer below is bypassed.");
            }
            let shown = b.strips(self.bus);
            let strips = if self.show_all {
                vec![vec![0], vec![1], vec![2], vec![3], vec![4, 5], vec![6, 7], vec![8, 9]]
            } else {
                shown.clone()
            };
            egui::ScrollArea::horizontal().show(ui, |ui| {
                ui.horizontal_top(|ui| {
                    for srcs in &strips {
                        self.strip(ui, b, srcs, shown.contains(srcs));
                    }
                    self.add_menu(ui, b);
                });
            });
        });
    }
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.receive();

        egui::TopBottomPanel::top("top").show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.label(RichText::new("MINIFUSE").size(20.0).color(Color32::from_rgb(0xb0, 0x90, 0xf0)));
                ui.label("4");
                ui.add_space(40.0);
                for (tab, name) in [(Tab::Inputs, "INPUTS"), (Tab::Outputs, "OUTPUTS")] {
                    let on = self.tab == tab;
                    if toggle(ui, on, name, ACCENT, egui::vec2(90.0, 26.0)) {
                        self.tab = tab;
                    }
                }
                if !self.kmod {
                    ui.colored_label(SOLO, "⚠ No kernel module: audio is interrupted while this is open");
                } else if !self.has_meters {
                    ui.colored_label(SOLO, "Reload the kernel module from kmod/ to see level meters");
                }
            });
        });
        if let Some(e) = &self.error {
            egui::TopBottomPanel::bottom("error").show(ctx, |ui| {
                ui.colored_label(MUTE, e);
            });
        }

        let Some(b) = self.block.clone() else {
            egui::CentralPanel::default().show(ctx, |ui| {
                ui.centered_and_justified(|ui| ui.label("Reading MiniFuse settings…"));
            });
            return;
        };
        egui::CentralPanel::default().show(ctx, |ui| match self.tab {
            Tab::Inputs => self.inputs(ui, &b),
            Tab::Outputs => self.outputs(ui, &b),
        });
    }
}

fn main() -> eframe::Result {
    let fail = |msg: &str| -> ! {
        eprintln!("Error: {}", msg);
        std::process::exit(1);
    };
    match device::detect_model() {
        Some(Model::Mf4) => {}
        Some(m) => fail(&format!("mf-gui only supports the MiniFuse 4, found a {}", m.name())),
        None => fail("No MiniFuse device found"),
    }
    let transport = Transport::open(true).unwrap_or_else(|e| fail(&e));
    let kmod = matches!(transport, Transport::Kmod { .. });
    let has_meters = transport.has_meters();

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("MiniFuse Control")
            .with_app_id("mf-gui")
            .with_inner_size([1000.0, 540.0]),
        ..Default::default()
    };
    eframe::run_native(
        "MiniFuse Control",
        options,
        Box::new(move |cc| {
            let mut visuals = egui::Visuals::dark();
            visuals.selection.bg_fill = ACCENT;
            visuals.widgets.active.bg_fill = ACCENT;
            cc.egui_ctx.set_visuals(visuals);

            let (tx, worker_rx) = channel();
            let (worker_tx, rx) = channel();
            let ctx = cc.egui_ctx.clone();
            thread::spawn(move || worker(transport, worker_rx, worker_tx, ctx));
            Ok(Box::new(App {
                tx,
                rx,
                block: None,
                seq: 0,
                meters: [f64::NEG_INFINITY; 16],
                meter_time: Instant::now(),
                has_meters,
                error: None,
                kmod,
                tab: Tab::Outputs,
                bus: 0,
                show_all: false,
            }))
        }),
    )
}
