//! Remote control.
//!
//! Viewer side: [`Forwarder`] turns mouse and keyboard events over the remote
//! screen into [`InputEvent`]s.
//!
//! Host side: [`Injector`] replays them on this PC with the Windows
//! `SendInput` API, and releases anything still held when control ends so no
//! key or mouse button stays stuck down.

use eframe::egui::{self, Event, MouseWheelUnit, PointerButton, Pos2, Rect};
use rd_core::media::{InputEvent, KeyModifiers, MouseButton};

// ---------------------------------------------------------------------------
// Viewer
// ---------------------------------------------------------------------------

/// Points per wheel "line" for trackpads that report pixel deltas.
const POINTS_PER_NOTCH: f32 = 50.0;
const WHEEL_NOTCH: f32 = 120.0;

/// Per-session viewer state for forwarding input.
#[derive(Default)]
pub struct Forwarder {
    buttons_down: [bool; 3],
    modifiers: KeyModifiers,
}

impl Forwarder {
    /// Converts this frame's egui events into input for the host. `shown` is
    /// where the remote screen is drawn; `hovered` is whether the pointer is
    /// over it (and not over another widget).
    pub fn forward(
        &mut self,
        events: &[Event],
        modifiers: egui::Modifiers,
        shown: Rect,
        hovered: bool,
        mut send: impl FnMut(InputEvent),
    ) {
        // Moves are coalesced: only the latest position before each other
        // event (and at the end of the frame) is sent.
        let mut pending_move = None;

        for event in events {
            match event {
                Event::PointerMoved(pos) => {
                    if (hovered && shown.contains(*pos)) || self.any_button_down() {
                        pending_move = Some(normalize(*pos, shown));
                    }
                }
                Event::PointerButton { pos, button, pressed, modifiers } => {
                    let Some((index, button)) = map_button(*button) else {
                        continue;
                    };
                    // Presses must start on the remote screen; releases always
                    // follow a press we forwarded, wherever the pointer is now.
                    let wanted = if *pressed {
                        hovered && shown.contains(*pos)
                    } else {
                        self.buttons_down[index]
                    };
                    if !wanted {
                        continue;
                    }
                    pending_move = Some(normalize(*pos, shown));
                    flush_move(&mut pending_move, &mut send);
                    self.buttons_down[index] = *pressed;
                    send(InputEvent::MouseButton {
                        button,
                        down: *pressed,
                        modifiers: convert_modifiers(*modifiers),
                    });
                }
                Event::MouseWheel { unit, delta, .. } if hovered => {
                    let notches = match unit {
                        MouseWheelUnit::Point => *delta / POINTS_PER_NOTCH,
                        MouseWheelUnit::Line => *delta,
                        MouseWheelUnit::Page => *delta * 3.0,
                    };
                    // egui's positive x moves content right (scrolls left);
                    // Windows' positive horizontal wheel scrolls right.
                    let dx = (-notches.x * WHEEL_NOTCH).round() as i32;
                    let dy = (notches.y * WHEEL_NOTCH).round() as i32;
                    if dx != 0 || dy != 0 {
                        flush_move(&mut pending_move, &mut send);
                        send(InputEvent::Wheel { dx, dy });
                    }
                }
                Event::Key { key, physical_key, pressed, modifiers, .. } => {
                    // The physical key matches what a real keyboard would send,
                    // so the host's own keyboard layout decides the character.
                    if let Some(vk) = virtual_key(physical_key.unwrap_or(*key)) {
                        flush_move(&mut pending_move, &mut send);
                        send(InputEvent::Key {
                            vk,
                            down: *pressed,
                            modifiers: convert_modifiers(*modifiers),
                        });
                    }
                }
                // egui turns these shortcuts into clipboard events instead of
                // key events; send them on as the shortcut itself.
                Event::Copy | Event::Cut | Event::Paste(_) => {
                    let vk = match event {
                        Event::Copy => VK_C,
                        Event::Cut => VK_X,
                        _ => VK_V,
                    };
                    let modifiers = KeyModifiers { ctrl: true, ..convert_modifiers(modifiers) };
                    flush_move(&mut pending_move, &mut send);
                    send(InputEvent::Key { vk, down: true, modifiers });
                    send(InputEvent::Key { vk, down: false, modifiers });
                }
                Event::WindowFocused(false) => {
                    flush_move(&mut pending_move, &mut send);
                    self.release_all(&mut send);
                }
                _ => {}
            }
        }
        flush_move(&mut pending_move, &mut send);

        let modifiers = convert_modifiers(modifiers);
        if modifiers != self.modifiers {
            self.modifiers = modifiers;
            send(InputEvent::Modifiers { modifiers });
        }
    }

    /// Lets go of everything, e.g. when control is switched off.
    pub fn release_all(&mut self, send: &mut dyn FnMut(InputEvent)) {
        *self = Self::default();
        send(InputEvent::ReleaseAll);
    }

    fn any_button_down(&self) -> bool {
        self.buttons_down.iter().any(|&down| down)
    }
}

fn flush_move(pending: &mut Option<(u16, u16)>, send: &mut dyn FnMut(InputEvent)) {
    if let Some((x, y)) = pending.take() {
        send(InputEvent::MouseMove { x, y });
    }
}

fn normalize(pos: Pos2, shown: Rect) -> (u16, u16) {
    let fraction = |value: f32, min: f32, size: f32| {
        let f = if size > 0.0 { (value - min) / size } else { 0.0 };
        (f.clamp(0.0, 1.0) * 65535.0).round() as u16
    };
    (
        fraction(pos.x, shown.min.x, shown.width()),
        fraction(pos.y, shown.min.y, shown.height()),
    )
}

fn map_button(button: PointerButton) -> Option<(usize, MouseButton)> {
    match button {
        PointerButton::Primary => Some((0, MouseButton::Left)),
        PointerButton::Secondary => Some((1, MouseButton::Right)),
        PointerButton::Middle => Some((2, MouseButton::Middle)),
        _ => None,
    }
}

fn convert_modifiers(modifiers: egui::Modifiers) -> KeyModifiers {
    KeyModifiers {
        ctrl: modifiers.ctrl,
        shift: modifiers.shift,
        alt: modifiers.alt,
    }
}

const VK_C: u16 = 0x43;
const VK_V: u16 = 0x56;
const VK_X: u16 = 0x58;

/// Windows virtual-key code for an egui key (by US layout position).
pub fn virtual_key(key: egui::Key) -> Option<u16> {
    use egui::Key::*;
    let letters = [A, B, C, D, E, F, G, H, I, J, K, L, M, N, O, P, Q, R, S, T, U, V, W, X, Y, Z];
    if let Some(i) = letters.iter().position(|&k| k == key) {
        return Some(0x41 + i as u16);
    }
    let digits = [Num0, Num1, Num2, Num3, Num4, Num5, Num6, Num7, Num8, Num9];
    if let Some(i) = digits.iter().position(|&k| k == key) {
        return Some(0x30 + i as u16);
    }
    let functions = [
        F1, F2, F3, F4, F5, F6, F7, F8, F9, F10, F11, F12, F13, F14, F15, F16, F17, F18, F19, F20,
        F21, F22, F23, F24,
    ];
    if let Some(i) = functions.iter().position(|&k| k == key) {
        return Some(0x70 + i as u16);
    }
    Some(match key {
        Backspace => 0x08,
        Tab => 0x09,
        Enter => 0x0D,
        Escape => 0x1B,
        Space => 0x20,
        PageUp => 0x21,
        PageDown => 0x22,
        End => 0x23,
        Home => 0x24,
        ArrowLeft => 0x25,
        ArrowUp => 0x26,
        ArrowRight => 0x27,
        ArrowDown => 0x28,
        Insert => 0x2D,
        Delete => 0x2E,
        Semicolon | Colon => 0xBA,
        Equals | Plus => 0xBB,
        Comma => 0xBC,
        Minus => 0xBD,
        Period => 0xBE,
        Slash | Questionmark => 0xBF,
        Backtick => 0xC0,
        OpenBracket | OpenCurlyBracket => 0xDB,
        Backslash | Pipe => 0xDC,
        CloseBracket | CloseCurlyBracket => 0xDD,
        Quote => 0xDE,
        _ => return None,
    })
}

// ---------------------------------------------------------------------------
// Host
// ---------------------------------------------------------------------------

const VK_LSHIFT: u16 = 0xA0;
const VK_LCONTROL: u16 = 0xA2;
const VK_LMENU: u16 = 0xA4;

/// Where injected input goes. Real input on Windows; a recorder in tests.
pub trait InputSink {
    fn key(&mut self, vk: u16, down: bool);
    fn mouse_move(&mut self, x: u16, y: u16);
    fn mouse_button(&mut self, button: MouseButton, down: bool);
    fn wheel(&mut self, horizontal: bool, amount: i32);
}

/// Replays viewer input, keeping track of what is held down.
pub struct Injector<S: InputSink> {
    sink: S,
    modifiers: KeyModifiers,
    keys: Vec<u16>,
    buttons: Vec<MouseButton>,
}

impl<S: InputSink> Injector<S> {
    pub fn new(sink: S) -> Self {
        Self { sink, modifiers: KeyModifiers::default(), keys: Vec::new(), buttons: Vec::new() }
    }

    pub fn apply(&mut self, event: InputEvent) {
        match event {
            InputEvent::MouseMove { x, y } => self.sink.mouse_move(x, y),
            InputEvent::MouseButton { button, down, modifiers } => {
                self.sync_modifiers(modifiers);
                self.buttons.retain(|&b| b != button);
                if down {
                    self.buttons.push(button);
                }
                self.sink.mouse_button(button, down);
            }
            InputEvent::Wheel { dx, dy } => {
                if dy != 0 {
                    self.sink.wheel(false, dy);
                }
                if dx != 0 {
                    self.sink.wheel(true, dx);
                }
            }
            InputEvent::Key { vk, down, modifiers } => {
                if matches!(vk, VK_LSHIFT | VK_LCONTROL | VK_LMENU) {
                    return; // Modifiers are only changed through `sync_modifiers`.
                }
                self.sync_modifiers(modifiers);
                self.keys.retain(|&k| k != vk);
                if down {
                    self.keys.push(vk);
                }
                self.sink.key(vk, down);
            }
            InputEvent::Modifiers { modifiers } => self.sync_modifiers(modifiers),
            InputEvent::ReleaseAll => self.release_all(),
        }
    }

    pub fn release_all(&mut self) {
        for vk in std::mem::take(&mut self.keys) {
            self.sink.key(vk, false);
        }
        for button in std::mem::take(&mut self.buttons) {
            self.sink.mouse_button(button, false);
        }
        self.sync_modifiers(KeyModifiers::default());
    }

    fn sync_modifiers(&mut self, wanted: KeyModifiers) {
        let have = self.modifiers;
        for (held, want, vk) in [
            (have.ctrl, wanted.ctrl, VK_LCONTROL),
            (have.shift, wanted.shift, VK_LSHIFT),
            (have.alt, wanted.alt, VK_LMENU),
        ] {
            if held != want {
                self.sink.key(vk, want);
            }
        }
        self.modifiers = wanted;
    }
}

impl<S: InputSink> Drop for Injector<S> {
    fn drop(&mut self) {
        self.release_all();
    }
}

/// Sends input to this PC through `SendInput`.
pub struct SystemInput;

#[cfg(windows)]
impl InputSink for SystemInput {
    fn key(&mut self, vk: u16, down: bool) {
        use windows_sys::Win32::UI::Input::KeyboardAndMouse::*;
        // SAFETY: MapVirtualKeyW has no preconditions.
        let scan = unsafe { MapVirtualKeyW(u32::from(vk), MAPVK_VK_TO_VSC) } as u16;
        let mut flags = if down { 0 } else { KEYEVENTF_KEYUP };
        if is_extended_key(vk) {
            flags |= KEYEVENTF_EXTENDEDKEY;
        }
        send(INPUT {
            r#type: INPUT_KEYBOARD,
            Anonymous: INPUT_0 {
                ki: KEYBDINPUT { wVk: vk, wScan: scan, dwFlags: flags, time: 0, dwExtraInfo: 0 },
            },
        });
    }

    fn mouse_move(&mut self, x: u16, y: u16) {
        use windows_sys::Win32::UI::Input::KeyboardAndMouse::*;
        // Absolute coordinates without VIRTUALDESK cover the primary monitor,
        // which is the one being shared.
        mouse(i32::from(x), i32::from(y), 0, MOUSEEVENTF_MOVE | MOUSEEVENTF_ABSOLUTE);
    }

    fn mouse_button(&mut self, button: MouseButton, down: bool) {
        use windows_sys::Win32::UI::Input::KeyboardAndMouse::*;
        let flags = match (button, down) {
            (MouseButton::Left, true) => MOUSEEVENTF_LEFTDOWN,
            (MouseButton::Left, false) => MOUSEEVENTF_LEFTUP,
            (MouseButton::Right, true) => MOUSEEVENTF_RIGHTDOWN,
            (MouseButton::Right, false) => MOUSEEVENTF_RIGHTUP,
            (MouseButton::Middle, true) => MOUSEEVENTF_MIDDLEDOWN,
            (MouseButton::Middle, false) => MOUSEEVENTF_MIDDLEUP,
        };
        mouse(0, 0, 0, flags);
    }

    fn wheel(&mut self, horizontal: bool, amount: i32) {
        use windows_sys::Win32::UI::Input::KeyboardAndMouse::*;
        let flags = if horizontal { MOUSEEVENTF_HWHEEL } else { MOUSEEVENTF_WHEEL };
        // mouseData is a signed amount stored in an unsigned field.
        mouse(0, 0, amount as u32, flags);
    }
}

#[cfg(windows)]
fn mouse(dx: i32, dy: i32, data: u32, flags: u32) {
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::*;
    send(INPUT {
        r#type: INPUT_MOUSE,
        Anonymous: INPUT_0 {
            mi: MOUSEINPUT { dx, dy, mouseData: data, dwFlags: flags, time: 0, dwExtraInfo: 0 },
        },
    });
}

#[cfg(windows)]
fn send(input: windows_sys::Win32::UI::Input::KeyboardAndMouse::INPUT) {
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::{INPUT, SendInput};
    // SAFETY: one fully initialised INPUT, with its correct size.
    unsafe { SendInput(1, &input, size_of::<INPUT>() as i32) };
}

/// Keys that live in the "extended" block (arrows, Insert/Delete, Home/End, …).
#[cfg(windows)]
fn is_extended_key(vk: u16) -> bool {
    matches!(vk, 0x21..=0x28 | 0x2D | 0x2E | 0x5B | 0x5C | 0x5D | 0x6F | 0x90 | 0xA3 | 0xA5)
}

#[cfg(not(windows))]
impl InputSink for SystemInput {
    fn key(&mut self, _: u16, _: bool) {}
    fn mouse_move(&mut self, _: u16, _: u16) {}
    fn mouse_button(&mut self, _: MouseButton, _: bool) {}
    fn wheel(&mut self, _: bool, _: i32) {}
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::{Modifiers, pos2};

    #[derive(Default)]
    struct Recorder(Vec<String>);

    impl InputSink for &mut Recorder {
        fn key(&mut self, vk: u16, down: bool) {
            self.0.push(format!("key {vk:#x} {down}"));
        }
        fn mouse_move(&mut self, x: u16, y: u16) {
            self.0.push(format!("move {x} {y}"));
        }
        fn mouse_button(&mut self, button: MouseButton, down: bool) {
            self.0.push(format!("button {button:?} {down}"));
        }
        fn wheel(&mut self, horizontal: bool, amount: i32) {
            self.0.push(format!("wheel {horizontal} {amount}"));
        }
    }

    #[test]
    fn shortcut_presses_and_releases_modifiers() {
        let mut recorder = Recorder::default();
        {
            let mut injector = Injector::new(&mut recorder);
            let ctrl = KeyModifiers { ctrl: true, ..Default::default() };
            injector.apply(InputEvent::Key { vk: VK_C, down: true, modifiers: ctrl });
            injector.apply(InputEvent::Key { vk: VK_C, down: false, modifiers: ctrl });
            injector.apply(InputEvent::Modifiers { modifiers: KeyModifiers::default() });
        }
        assert_eq!(
            recorder.0,
            ["key 0xa2 true", "key 0x43 true", "key 0x43 false", "key 0xa2 false"]
        );
    }

    #[test]
    fn held_input_is_released_when_control_ends() {
        let mut recorder = Recorder::default();
        {
            let mut injector = Injector::new(&mut recorder);
            let shift = KeyModifiers { shift: true, ..Default::default() };
            injector.apply(InputEvent::Key { vk: 0x41, down: true, modifiers: shift });
            injector.apply(InputEvent::MouseButton {
                button: MouseButton::Left,
                down: true,
                modifiers: shift,
            });
        } // dropped here, as when the session ends
        assert_eq!(
            recorder.0,
            [
                "key 0xa0 true",
                "key 0x41 true",
                "button Left true",
                "key 0x41 false",
                "button Left false",
                "key 0xa0 false",
            ]
        );
    }

    #[test]
    fn viewer_maps_clicks_onto_the_remote_screen() {
        let shown = Rect::from_min_max(pos2(100.0, 100.0), pos2(300.0, 200.0));
        let mut forwarder = Forwarder::default();
        let mut sent = Vec::new();
        let events = [
            Event::PointerMoved(pos2(200.0, 150.0)),
            Event::PointerButton {
                pos: pos2(300.0, 200.0),
                button: PointerButton::Primary,
                pressed: true,
                modifiers: Modifiers::NONE,
            },
            // Released outside the screen: still forwarded.
            Event::PointerButton {
                pos: pos2(500.0, 500.0),
                button: PointerButton::Primary,
                pressed: false,
                modifiers: Modifiers::NONE,
            },
            // Pressed outside the screen: ignored.
            Event::PointerButton {
                pos: pos2(10.0, 10.0),
                button: PointerButton::Primary,
                pressed: true,
                modifiers: Modifiers::NONE,
            },
        ];
        forwarder.forward(&events, Modifiers::NONE, shown, true, |e| sent.push(e));
        let up = |down| InputEvent::MouseButton {
            button: MouseButton::Left,
            down,
            modifiers: KeyModifiers::default(),
        };
        assert_eq!(
            sent,
            [
                InputEvent::MouseMove { x: 65535, y: 65535 },
                up(true),
                InputEvent::MouseMove { x: 65535, y: 65535 },
                up(false),
            ]
        );
    }

    #[test]
    fn keys_map_to_windows_codes() {
        assert_eq!(virtual_key(egui::Key::A), Some(0x41));
        assert_eq!(virtual_key(egui::Key::Num7), Some(0x37));
        assert_eq!(virtual_key(egui::Key::F12), Some(0x7B));
        assert_eq!(virtual_key(egui::Key::ArrowDown), Some(0x28));
        assert_eq!(virtual_key(egui::Key::F30), None);
    }
}
