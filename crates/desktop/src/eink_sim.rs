//! Simulated e-ink screen for the desktop frontend (`--features eink-sim`).
//!
//! Runs pbAnki on the same `inkview-slint` backend as the PocketBook (its
//! presenter, damage policy and update policy), with the InkView screen
//! replaced by [`SimScreen`]: each update reaches the window only after the
//! time a panel would take, quantized to the update's gray levels, and the
//! panel reports busy meanwhile. Timings default to inkshelf's InkPad 3
//! measurements (GC16 full ~600 ms, GL16/partial ~490 ms, DU ~265 ms) and can
//! be overridden with `PBANKI_EINK_SIM_MS="full=600,partial=490,fast=265"`.

use std::collections::VecDeque;
use std::ffi::c_int;
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use inkview::Event as InkEvent;
use inkview::event::Key;
use inkview::screen::{BB8, UpdFlags, UpdateKind, Waveform};
use inkview_slint::{EinkScreen, Event};
use minifb::{KeyRepeat, MouseButton, MouseMode, Window, WindowOptions};

/// The PB700/PB710 panel.
pub const WIDTH: usize = 1264;
pub const HEIGHT: usize = 1680;
const DPI: u32 = 300;

/// Update durations by class, in milliseconds.
#[derive(Clone, Copy, Debug)]
struct Timings {
    /// GC16 full-screen (FullUpdate, SoftUpdate, ex:gc16, ex:init).
    full: u64,
    /// GL16-class region updates (PartialUpdate*, ex:gl16, ...).
    partial: u64,
    /// DU-class fast updates (DynamicUpdate*, PartialUpdateDU4, ex:du, ...).
    fast: u64,
}

impl Timings {
    fn from_env() -> Self {
        let mut timings = Self {
            full: 600,
            partial: 490,
            fast: 265,
        };
        if let Ok(spec) = std::env::var("PBANKI_EINK_SIM_MS") {
            for entry in spec.split(',') {
                let Some((key, ms)) = entry.split_once('=') else {
                    continue;
                };
                let Ok(ms) = ms.trim().parse() else {
                    continue;
                };
                match key.trim() {
                    "full" => timings.full = ms,
                    "partial" => timings.partial = ms,
                    "fast" => timings.fast = ms,
                    other => eprintln!("[eink-sim] ignoring PBANKI_EINK_SIM_MS key '{other}'"),
                }
            }
        }
        timings
    }
}

/// How an update looks and how long it takes.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Look {
    ms: u64,
    /// Gray levels the waveform can show.
    levels: u8,
    /// Flashes black before showing the new content.
    flash: bool,
}

fn named_look(kind: UpdateKind, t: Timings) -> Look {
    match kind {
        UpdateKind::Full | UpdateKind::FullHq => Look {
            ms: t.full,
            levels: 16,
            flash: true,
        },
        UpdateKind::Soft | UpdateKind::SoftHq => Look {
            ms: t.full,
            levels: 16,
            flash: false,
        },
        UpdateKind::Partial | UpdateKind::PartialHq => Look {
            ms: t.partial,
            levels: 16,
            flash: false,
        },
        UpdateKind::PartialBw => Look {
            ms: t.partial,
            levels: 2,
            flash: false,
        },
        UpdateKind::PartialDu4 | UpdateKind::Dynamic => Look {
            ms: t.fast,
            levels: 4,
            flash: false,
        },
        UpdateKind::DynamicBw => Look {
            ms: t.fast,
            levels: 2,
            flash: false,
        },
    }
}

fn waveform_look(waveform: Waveform, t: Timings) -> Look {
    let (ms, levels, flash) = match waveform {
        Waveform::Init => (t.full * 2, 16, true),
        Waveform::Gc16 | Waveform::Gcc16 => (t.full, 16, true),
        Waveform::Gl16 | Waveform::Glr16 | Waveform::Gln16 => (t.partial, 16, false),
        Waveform::Gc4 | Waveform::Du4 => (t.fast, 4, false),
        Waveform::Du => (t.fast, 2, false),
        Waveform::A2 | Waveform::A2In | Waveform::A2Out => (t.fast / 2, 2, false),
    };
    Look { ms, levels, flash }
}

/// Maps a gray value to the nearest of `levels` evenly spaced grays.
fn quantize(v: u8, levels: u8) -> u8 {
    if levels >= 16 {
        // 16 grays: 0, 17, ..., 255.
        return (v / 17) * 17 + if v % 17 > 8 { 17 } else { 0 };
    }
    let steps = u32::from(levels.max(2)) - 1;
    let level = (u32::from(v) * steps + 127) / 255;
    (level * 255 / steps) as u8
}

struct PendingUpdate {
    x: usize,
    y: usize,
    w: usize,
    h: usize,
    /// Quantized snapshot of the framebuffer region at issue time.
    pixels: Vec<u8>,
    started: Instant,
    done_at: Instant,
    flash: bool,
}

/// What the window shows, shared with the window thread.
pub struct Panel {
    /// The pixels currently "on the glass".
    visible: Vec<u8>,
    pending: VecDeque<PendingUpdate>,
}

impl Panel {
    pub fn new() -> Arc<Mutex<Self>> {
        Arc::new(Mutex::new(Self {
            visible: vec![255; WIDTH * HEIGHT],
            pending: VecDeque::new(),
        }))
    }

    /// Applies finished updates; returns the frame to show (flashing regions
    /// drawn black for the first half of their update).
    fn frame(&mut self, now: Instant, out: &mut [u32]) {
        while let Some(update) = self.pending.front() {
            if update.done_at > now {
                break;
            }
            let update = self.pending.pop_front().unwrap();
            for row in 0..update.h {
                let dst = (update.y + row) * WIDTH + update.x;
                let src = row * update.w;
                self.visible[dst..dst + update.w]
                    .copy_from_slice(&update.pixels[src..src + update.w]);
            }
        }
        for (o, &v) in out.iter_mut().zip(&self.visible) {
            let v = u32::from(v);
            *o = (v << 16) | (v << 8) | v;
        }
        for update in &self.pending {
            if update.flash && now < update.started + (update.done_at - update.started) / 2 {
                for row in 0..update.h {
                    let start = (update.y + row) * WIDTH + update.x;
                    out[start..start + update.w].fill(0);
                }
            }
        }
    }
}

/// The app framebuffer plus a simulated panel.
pub struct SimScreen {
    framebuffer: Vec<BB8>,
    panel: Arc<Mutex<Panel>>,
    timings: Timings,
}

impl SimScreen {
    pub fn new(panel: Arc<Mutex<Panel>>) -> Self {
        let timings = Timings::from_env();
        eprintln!("[eink-sim] {WIDTH}x{HEIGHT} at {DPI} dpi, timings {timings:?}");
        Self {
            framebuffer: vec![BB8(255); WIDTH * HEIGHT],
            panel,
            timings,
        }
    }

    fn start(&mut self, look: Look, x: i32, y: i32, w: u32, h: u32, full_screen: bool) {
        let (x, y, w, h) = if full_screen {
            (0, 0, WIDTH, HEIGHT)
        } else {
            let x = (x.max(0) as usize).min(WIDTH);
            let y = (y.max(0) as usize).min(HEIGHT);
            (x, y, (w as usize).min(WIDTH - x), (h as usize).min(HEIGHT - y))
        };
        if w == 0 || h == 0 {
            return;
        }
        let mut pixels = Vec::with_capacity(w * h);
        for row in y..y + h {
            let start = row * WIDTH + x;
            pixels.extend(
                self.framebuffer[start..start + w]
                    .iter()
                    .map(|p| quantize(p.0, look.levels)),
            );
        }
        let started = Instant::now();
        let mut panel = self.panel.lock().unwrap();
        // Updates complete in order, as one controller would run them.
        let after = panel.pending.back().map_or(started, |u| u.done_at.max(started));
        panel.pending.push_back(PendingUpdate {
            x,
            y,
            w,
            h,
            pixels,
            started: after,
            done_at: after + Duration::from_millis(look.ms),
            flash: look.flash,
        });
    }
}

impl EinkScreen for SimScreen {
    type Pixel = BB8;

    fn width(&self) -> usize {
        WIDTH
    }

    fn height(&self) -> usize {
        HEIGHT
    }

    fn dpi(&self) -> u32 {
        DPI
    }

    fn scale(&self) -> f32 {
        1.0
    }

    fn copy_rect(
        &mut self,
        x: usize,
        y: usize,
        w: usize,
        h: usize,
        src: &[BB8],
        src_stride: usize,
        src_origin: (usize, usize),
    ) {
        let (ox, oy) = src_origin;
        let x_end = (x + w).min(WIDTH).min(ox + src_stride);
        if x >= x_end || x < ox || y < oy {
            return;
        }
        let len = x_end - x;
        for row in y..(y + h).min(HEIGHT) {
            let s = (row - oy) * src_stride + (x - ox);
            let Some(src_row) = src.get(s..s + len) else {
                return;
            };
            let d = row * WIDTH + x;
            self.framebuffer[d..d + len].copy_from_slice(src_row);
        }
    }

    fn update(&mut self, kind: UpdateKind, x: i32, y: i32, w: u32, h: u32) {
        let look = named_look(kind, self.timings);
        self.start(look, x, y, w, h, kind.is_full_screen());
    }

    fn update_ex(
        &mut self,
        waveform: Waveform,
        flags: UpdFlags,
        x: i32,
        y: i32,
        w: u32,
        h: u32,
    ) -> bool {
        let look = waveform_look(waveform, self.timings);
        let full = flags.0 & UpdFlags::FULLSCREEN.0 != 0;
        self.start(look, x, y, w, h, full);
        true
    }

    fn is_updating(&mut self) -> bool {
        let now = Instant::now();
        self.panel
            .lock()
            .unwrap()
            .pending
            .iter()
            .any(|u| u.done_at > now)
    }

    fn wake_event_loop(&self) -> Option<c_int> {
        None
    }

    fn waveform_times(&self) -> Option<[u16; 16]> {
        let mut times = [0u16; 16];
        for waveform in Waveform::ALL {
            times[waveform as usize] = waveform_look(waveform, self.timings).ms as u16;
        }
        Some(times)
    }
}

fn key_for(key: minifb::Key) -> Option<Key> {
    Some(match key {
        minifb::Key::Up => Key::Up,
        minifb::Key::Down => Key::Down,
        minifb::Key::Left => Key::Left,
        minifb::Key::Right => Key::Right,
        minifb::Key::Enter | minifb::Key::Space => Key::Ok,
        minifb::Key::Backspace | minifb::Key::Escape => Key::Back,
        minifb::Key::PageUp => Key::Prev,
        minifb::Key::PageDown => Key::Next,
        minifb::Key::Home => Key::Home,
        minifb::Key::M => Key::Menu,
        _ => return None,
    })
}

/// Runs the simulator window on the calling (main) thread until it is
/// closed: shows the panel and turns mouse and keys into InkView events.
pub fn run_window(panel: Arc<Mutex<Panel>>, events: Sender<Event>) {
    let send = |evt: InkEvent| {
        let _ = events.send(Event::InkviewEvent(evt));
    };
    // Half size fits a laptop screen; the buffer stays full resolution.
    let mut window = Window::new(
        "pbAnki (simulated e-ink 1264x1680, shown at 1/2)",
        WIDTH / 2,
        HEIGHT / 2,
        WindowOptions {
            scale_mode: minifb::ScaleMode::Stretch,
            ..WindowOptions::default()
        },
    )
    .expect("cannot open the simulator window");
    window.set_target_fps(60);

    send(InkEvent::Init);
    send(InkEvent::Show);

    let mut frame = vec![0u32; WIDTH * HEIGHT];
    let mut pressed: Option<(i32, i32)> = None;
    while window.is_open() {
        panel.lock().unwrap().frame(Instant::now(), &mut frame);
        if window.update_with_buffer(&frame, WIDTH, HEIGHT).is_err() {
            break;
        }

        let (win_w, win_h) = window.get_size();
        let pos = window
            .get_unscaled_mouse_pos(MouseMode::Clamp)
            .map(|(x, y)| {
                (
                    (x * WIDTH as f32 / win_w.max(1) as f32) as i32,
                    (y * HEIGHT as f32 / win_h.max(1) as f32) as i32,
                )
            });
        let down = window.get_mouse_down(MouseButton::Left);
        match (down, pressed, pos) {
            (true, None, Some((x, y))) => {
                send(InkEvent::PointerDown { x, y });
                pressed = Some((x, y));
            }
            (true, Some(last), Some((x, y))) if last != (x, y) => {
                send(InkEvent::PointerMove { x, y });
                pressed = Some((x, y));
            }
            (false, Some((x, y)), pos) => {
                let (x, y) = pos.unwrap_or((x, y));
                send(InkEvent::PointerUp { x, y });
                pressed = None;
            }
            _ => {}
        }

        for key in window.get_keys_pressed(KeyRepeat::No) {
            if let Some(key) = key_for(key) {
                send(InkEvent::KeyDown { key });
            }
        }
        for key in window.get_keys_released() {
            if let Some(key) = key_for(key) {
                send(InkEvent::KeyUp { key });
            }
        }
    }
    let _ = events.send(Event::Quit);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quantize_snaps_to_waveform_grays() {
        assert_eq!(quantize(0, 2), 0);
        assert_eq!(quantize(200, 2), 255);
        assert_eq!(quantize(100, 2), 0);
        assert_eq!(quantize(100, 4), 85);
        assert_eq!(quantize(130, 16), 136);
        assert_eq!(quantize(255, 16), 255);
    }

    #[test]
    fn updates_finish_in_order_and_report_busy() {
        let panel = Panel::new();
        let mut screen = SimScreen {
            framebuffer: vec![BB8(0); WIDTH * HEIGHT],
            panel: panel.clone(),
            timings: Timings {
                full: 30,
                partial: 20,
                fast: 10,
            },
        };
        screen.update(UpdateKind::Partial, 0, 0, 10, 10);
        screen.update(UpdateKind::Dynamic, 0, 0, 10, 10);
        assert!(screen.is_updating());
        let first_done = panel.lock().unwrap().pending[0].done_at;
        let second_done = panel.lock().unwrap().pending[1].done_at;
        assert!(second_done > first_done);

        let mut out = vec![0u32; WIDTH * HEIGHT];
        panel.lock().unwrap().frame(second_done, &mut out);
        assert!(panel.lock().unwrap().pending.is_empty());
        assert_eq!(out[0], 0);
        assert_eq!(out[WIDTH * 20], 0x00FF_FFFF);
    }
}
