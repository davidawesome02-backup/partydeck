//! Client side of gamescope's custom Wayland protocols.
//!
//! [`GamescopeConnection`] talks to one headless gamescope instance over its
//! Wayland socket: it fetches the PipeWire node id for the video stream and
//! forwards keyboard/mouse input. [`InstanceStreamView`] pairs a connection
//! with a [`PipewireVideo`] widget to display the stream and feed hovered
//! input back into the instance.

use std::collections::{HashMap, HashSet};
use std::io::BufRead;
use std::os::fd::{AsFd, OwnedFd};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, RwLock};

use eframe::egui::{self, Pos2, Rect, Vec2};
use nix::poll::{PollFd, PollFlags, PollTimeout, poll};
use pipewire as pw;
use wayland_client::protocol::wl_registry;
use wayland_client::{Connection, Dispatch, Proxy};

use super::egl::EglApi;
use super::pipewire::{PipewireCommand, PipewireID, PipewireStream};
use super::video::PipewireVideo;

mod gamescope_pipewire_wrapper {
    use wayland_client;

    pub mod __interfaces {
        wayland_scanner::generate_interfaces!("./src/video/gamescope-pipewire.xml");
    }
    use self::__interfaces::*;

    wayland_scanner::generate_client_code!("./src/video/gamescope-pipewire.xml");
}
use gamescope_pipewire_wrapper::gamescope_pipewire::{self, GamescopePipewire};

mod gamescope_input_wrapper {
    use wayland_client;

    pub mod __interfaces {
        wayland_scanner::generate_interfaces!("./src/video/gamescope-input.xml");
    }
    use self::__interfaces::*;

    wayland_scanner::generate_client_code!("./src/video/gamescope-input.xml");
}
use gamescope_input_wrapper::gamescope_input::{self, GamescopeInput};

pub struct GamescopeWaylandState {
    pipewire_interface: Option<GamescopePipewire>,
    input_interface: Option<GamescopeInput>,
    has_data_to_send: bool,

    pub pipewire_node: Option<u32>,
    pub latest_output_size: Rect,

    // Warning, can hardlock if used in callback! I dont like this arc, but I will need to figure out later.
    event_queue: Arc<Mutex<wayland_client::EventQueue<GamescopeWaylandState>>>,
}

impl Dispatch<wl_registry::WlRegistry, ()> for GamescopeWaylandState {
    fn event(
        state: &mut GamescopeWaylandState,
        registry: &wl_registry::WlRegistry,
        event: wl_registry::Event,
        _data: &(),
        _conn: &Connection,
        qh: &wayland_client::QueueHandle<Self>,
    ) {
        match event {
            wl_registry::Event::Global { name, interface, version } => {
                if interface == GamescopePipewire::interface().name {
                    let pipewire_interface = registry.bind::<GamescopePipewire, _, _>(name, version, qh, ());
                    state.pipewire_interface = Some(pipewire_interface);
                }

                if interface == GamescopeInput::interface().name {
                    let input_interface = registry.bind::<GamescopeInput, _, _>(name, version, qh, ());
                    state.input_interface = Some(input_interface);
                }
            }
            wl_registry::Event::GlobalRemove { name: _ } => {}
            _ => {}
        }
    }
}

impl Dispatch<GamescopePipewire, ()> for GamescopeWaylandState {
    fn event(
        state: &mut Self,
        _proxy: &GamescopePipewire,
        event: <GamescopePipewire as wayland_client::Proxy>::Event,
        _: &(),
        _: &Connection,
        _qh: &wayland_client::QueueHandle<Self>,
    ) {
        match event {
            gamescope_pipewire::Event::StreamNode { node_id } => {
                state.pipewire_node = Some(node_id);
            }
        }
    }
}

impl Dispatch<GamescopeInput, ()> for GamescopeWaylandState {
    fn event(
        state: &mut Self,
        _proxy: &GamescopeInput,
        event: <GamescopeInput as wayland_client::Proxy>::Event,
        _: &(),
        _: &Connection,
        _qh: &wayland_client::QueueHandle<Self>,
    ) {
        match event {
            gamescope_input::Event::MousePosition { x: _, y: _ } => {}
            gamescope_input::Event::OutputSize { nestedWidth: nested_width, nestedHeight: nested_height, outputWidth: _, outputHeight: _ } => {
                // Nested size is what mouse coordinates are scaled against;
                // output size is the original rendered content.
                // Lossy, maybe fix later, but who is using a near 32 bit int screen?
                state.latest_output_size = Rect { min: Pos2::ZERO, max: Pos2 { x: nested_width as f32, y: nested_height as f32 } };
            }
        }
    }
}

impl GamescopeWaylandState {
    pub fn new(path: &PathBuf) -> Result<Self, String> {
        // Connect to display
        let unix_socket = std::os::unix::net::UnixStream::connect(path).map_err(|e| format!("Failed to connect to wayland socket: {e}"))?;

        let conn = Connection::from_socket(unix_socket).map_err(|e| format!("Connection error in wayland setup: {e}"))?;
        let event_queue = Arc::new(Mutex::new(conn.new_event_queue()));

        let mut state = Self {
            input_interface: None,
            pipewire_interface: None,
            has_data_to_send: false,

            pipewire_node: None,
            latest_output_size: Rect::ZERO,

            event_queue: event_queue.clone(),
        };

        let mut locked_event_queue = event_queue.lock().map_err(|e| format!("Failed to lock event_queue: {e}"))?;
        let qhandle = locked_event_queue.handle();
        let display = conn.display();

        // Get registry
        display.get_registry(&qhandle, ());

        // Dispatch one roundtrip to query globals
        locked_event_queue.roundtrip(&mut state).map_err(|e| format!("Roundtrip failed: {e}"))?;

        // Get pipewire id in second round trip.
        locked_event_queue.roundtrip(&mut state).map_err(|e| format!("Second roundtrip failed: {e}"))?;

        Ok(state)
    }

    pub fn get_size(&mut self) -> Result<(), String> {
        let input_interface = self.input_interface.as_ref().ok_or("No input interface accessable")?;
        input_interface.get_output_size();
        self.has_data_to_send = true;

        Ok(())
    }

    pub fn send_key(&mut self, key: u32, down: bool) -> Result<(), String> {
        let input_interface = self.input_interface.as_ref().ok_or("No input interface accessable")?;

        input_interface.keyboard_key(key, down as u32);
        self.has_data_to_send = true;

        Ok(())
    }

    pub fn mouse_button(&mut self, button: u32, pressed: bool) -> Result<(), String> {
        let input_interface = self.input_interface.as_ref().ok_or("No input interface accessable")?;
        /* input-event-codes.h
        #define BTN_LEFT		0x110
        #define BTN_RIGHT		0x111
        #define BTN_MIDDLE		0x112
        */

        input_interface.mouse_button(button, pressed as u32);

        self.has_data_to_send = true;

        Ok(())
    }

    pub fn mouse_set(&mut self, pos: Vec2) -> Result<(), String> {
        let input_interface = self.input_interface.as_ref().ok_or("No input interface accessable")?;
        let translated_pos = self.latest_output_size.lerp_inside(pos);
        input_interface.mouse_warp(translated_pos.x as f64, translated_pos.y as f64);

        self.has_data_to_send = true;
        Ok(())
    }

    pub fn mouse_move(&mut self, delta: Vec2) -> Result<(), String> {
        if delta == Vec2::ZERO { return Ok(()); }

        let input_interface = self.input_interface.as_ref().ok_or("No input interface accessable")?;
        let translated_delta = self.latest_output_size.lerp_inside(delta);
        input_interface.mouse_motion(translated_delta.x as f64, translated_delta.y as f64);

        self.has_data_to_send = true;
        Ok(())
    }

    pub fn mouse_scroll(&mut self, scroll: egui::Vec2) -> Result<(), String> {
        if scroll == Vec2::ZERO { return Ok(()); }

        let input_interface = self.input_interface.as_ref().ok_or("No input interface accessable")?;
        input_interface.mouse_scroll((scroll.x * 120.) as i32, (scroll.y * 120.) as i32); // Not best rounding here but should be fine

        self.has_data_to_send = true;
        Ok(())
    }

    pub fn round_trip(&mut self) -> Result<(), String> {
        if self.has_data_to_send == false { return Ok(()); };
        let event_queue = self.event_queue.clone();
        let mut locked_event_queue = event_queue.lock().map_err(|e| format!("Failed to lock event_queue: {e}"))?;
        locked_event_queue.roundtrip(self).map_err(|e| format!("Failed to process round trip: {e}"))?;

        self.has_data_to_send = false;
        Ok(())
    }
}

/// Displays one instance's PipeWire stream and forwards hovered keyboard and
/// mouse input into its gamescope. Must be created on the UI thread while the
/// GL context is current.
pub struct InstanceStreamView {
    video: PipewireVideo,
    wayland_state: GamescopeWaylandState,

    last_keys_down: HashSet<egui::Key>,
    last_pointer_pos: Option<Vec2>,
}

impl InstanceStreamView {
    pub fn new(
        egl: &Arc<EglApi>,
        wayland_state: GamescopeWaylandState,
        sender: pw::channel::Sender<PipewireCommand>,
        streams: Arc<RwLock<HashMap<PipewireID, Arc<RwLock<PipewireStream>>>>>,
        ctx: &egui::Context,
        viewport: egui::ViewportId,
    ) -> Result<Self, String> {
        let Some(pipewire_node) = wayland_state.pipewire_node else { return Err("Failed to get pw node".to_owned()); };
        let video = PipewireVideo::new(egl, pipewire_node, sender, streams, ctx, viewport)
            .map_err(|e| format!("Failed to create video: {e}"))?;

        Ok(Self {
            video,
            wayland_state,
            last_keys_down: HashSet::new(),
            last_pointer_pos: None,
        })
    }

    fn update_keys_down(&mut self, current_keys_down: &HashSet<egui::Key>) -> Result<(), String> {
        for key_down in current_keys_down.difference(&self.last_keys_down) {
            if let Some(translated_key) = egui_key_to_xkb(*key_down) {
                self.wayland_state.send_key(translated_key, true)?;
            }
        }

        for key_up in self.last_keys_down.difference(current_keys_down) {
            if let Some(translated_key) = egui_key_to_xkb(*key_up) {
                self.wayland_state.send_key(translated_key, false)?;
            }
        }

        self.last_keys_down = current_keys_down.clone();

        Ok(())
    }

    pub fn ui(&mut self, ui: &mut egui::Ui, desired_size: egui::Vec2) -> Result<egui::Response, String> {
        let response = self.video.ui(ui, desired_size);

        let current_pointer_state = response.ctx.input(|i| i.pointer.clone());

        let current_keys_down = ui.input(|i| i.keys_down.clone());

        let current_pointer_pos =
            current_pointer_state.latest_pos()
            .filter(|pos| response.rect.contains(*pos))
            .map(|pos|
                (pos - response.rect.left_top()) / (response.rect.right_bottom() - response.rect.left_top())
            );

        let current_pointer_movement =
            current_pointer_state.delta() /
            (response.rect.right_bottom() - response.rect.left_top());

        // Get updated size doesnt need to be called every frame, but ¯\_( *-* )_/¯
        self.wayland_state.get_size()?;

        // Keys may be inacurate due to egui processing, ui.input(|ui| ui.raw.events) is closer to raw I think
        if self.last_pointer_pos.is_none() && let Some(pointer_pos) = current_pointer_pos {
            // Force pos
            self.wayland_state.mouse_set(pointer_pos)?;

            self.update_keys_down(&current_keys_down)?;
        }

        if current_pointer_pos.is_none() && self.last_pointer_pos.is_some() {
            // release keyboard keys
            self.update_keys_down(&HashSet::new())?;
        }

        if current_pointer_pos.is_some() && self.last_pointer_pos.is_some() {
            self.wayland_state.mouse_move(current_pointer_movement)?;

            self.update_keys_down(&current_keys_down)?;
        }

        // Scroll and buttons only go to the hovered tile so input doesn't leak
        // into every instance on screen.
        if current_pointer_pos.is_some() {
            // I have no idea how to do non-smooth scroll.
            self.wayland_state.mouse_scroll(ui.input(|i| i.smooth_scroll_delta()))?;

            /* input-event-codes.h
            #define BTN_LEFT		0x110
            #define BTN_RIGHT		0x111
            #define BTN_MIDDLE		0x112
            */
            for (button, code) in [
                (egui::PointerButton::Primary, 0x110),
                (egui::PointerButton::Secondary, 0x111),
                (egui::PointerButton::Middle, 0x112),
            ] {
                if current_pointer_state.button_pressed(button) { self.wayland_state.mouse_button(code, true)?; }
                if current_pointer_state.button_released(button) { self.wayland_state.mouse_button(code, false)?; }
            }
        }

        self.last_pointer_pos = current_pointer_pos;
        self.wayland_state.round_trip()?; // Only runs if we actually updated anything.

        Ok(response)
    }
}

// modified from (because we have egui key instead of physical key see: NativeKeyCode::Xkb() and how egui "physical key" is derived) /winit-0.30.13/src/platform_impl/linux/common/xkb/keymap.rs as well as /egui-winit/src/lib.rs for "key" type conversion
// fn on_keyboard_input(&mut self, event: &winit::event::KeyEvent) {} is the most important for us.
// This is from pub fn physicalkey_to_scancode(key: PhysicalKey) -> Option<u32>, but translated to egui key as we cant get back to keycode easily.

// converts to linux uapi keycodes so we can send to gamescope, should match https://github.com/torvalds/linux/blob/master/include/uapi/linux/input-event-codes.h as much as we can
// Those in comments are ones in hte phisical key winit enum that dont exist here.
pub fn egui_key_to_xkb(input_key: egui::Key) -> Option<u32> {
    use egui::Key;

    // Not sure why these are translated..? egui shouldnt be translating these, so we just need to get the "unshifted" version of them.
    let input_key = match input_key {
        Key::Exclamationmark => Key::Num1,
        Key::Plus => Key::Equals,
        Key::Pipe => Key::Backslash,
        Key::OpenCurlyBracket => Key::OpenBracket,
        Key::CloseCurlyBracket => Key::CloseBracket,
        Key::Colon => Key::Semicolon,
        Key::Questionmark => Key::Slash,
        Key::Copy => Key::C,
        Key::Paste => Key::P,
        Key::Cut => Key::X,

        other => other
    };

    match input_key {
        Key::Escape => Some(1),
        Key::Num1 => Some(2),
        Key::Num2 => Some(3),
        Key::Num3 => Some(4),
        Key::Num4 => Some(5),
        Key::Num5 => Some(6),
        Key::Num6 => Some(7),
        Key::Num7 => Some(8),
        Key::Num8 => Some(9),
        Key::Num9 => Some(10),
        Key::Num0 => Some(11),
        Key::Minus => Some(12),
        Key::Equals => Some(13),
        Key::Backspace => Some(14),
        Key::Tab => Some(15),
        Key::Q => Some(16),
        Key::W => Some(17),
        Key::E => Some(18),
        Key::R => Some(19),
        Key::T => Some(20),
        Key::Y => Some(21),
        Key::U => Some(22),
        Key::I => Some(23),
        Key::O => Some(24),
        Key::P => Some(25),
        Key::OpenBracket => Some(26),
        Key::CloseBracket => Some(27),
        Key::Enter => Some(28),
        Key::ControlLeft => Some(29),
        Key::A => Some(30),
        Key::S => Some(31),
        Key::D => Some(32),
        Key::F => Some(33),
        Key::G => Some(34),
        Key::H => Some(35),
        Key::J => Some(36),
        Key::K => Some(37),
        Key::L => Some(38),
        Key::Semicolon => Some(39),
        Key::Quote => Some(40),
        Key::Backtick => Some(41),
        Key::ShiftLeft => Some(42),
        Key::Backslash => Some(43),
        Key::Z => Some(44),
        Key::X => Some(45),
        Key::C => Some(46),
        Key::V => Some(47),
        Key::B => Some(48),
        Key::N => Some(49),
        Key::M => Some(50),
        Key::Comma => Some(51),
        Key::Period => Some(52),
        Key::Slash => Some(53),
        Key::ShiftRight => Some(54),
        // Key::NumpadMultiply => Some(55),
        Key::AltLeft => Some(56),
        Key::Space => Some(57),
        // Key::CapsLock => Some(58),
        Key::F1 => Some(59),
        Key::F2 => Some(60),
        Key::F3 => Some(61),
        Key::F4 => Some(62),
        Key::F5 => Some(63),
        Key::F6 => Some(64),
        Key::F7 => Some(65),
        Key::F8 => Some(66),
        Key::F9 => Some(67),
        Key::F10 => Some(68),
        // Key::NumLock => Some(69),
        // Key::ScrollLock => Some(70),
        // Key::Numpad7 => Some(71),
        // Key::Numpad8 => Some(72),
        // Key::Numpad9 => Some(73),
        // Key::NumpadSubtract => Some(74),
        // Key::Numpad4 => Some(75),
        // Key::Numpad5 => Some(76),
        // Key::Numpad6 => Some(77),
        // Key::NumpadAdd => Some(78),
        // Key::Numpad1 => Some(79),
        // Key::Numpad2 => Some(80),
        // Key::Numpad3 => Some(81),
        // Key::Numpad0 => Some(82),
        // Key::NumpadDecimal => Some(83),
        // Key::Lang5 => Some(85),
        Key::IntlBackslash => Some(86),
        Key::F11 => Some(87),
        Key::F12 => Some(88),
        // Key::IntlRo => Some(89),
        // Key::Lang3 => Some(90),
        // Key::Lang4 => Some(91),
        // Key::Convert => Some(92),
        // Key::KanaMode => Some(93),
        // Key::NonConvert => Some(94),
        // Key::NumpadEnter => Some(96),
        Key::ControlRight => Some(97),
        // Key::NumpadDivide => Some(98),
        // Key::PrintScreen => Some(99),
        Key::AltRight => Some(100),
        Key::Home => Some(102),
        Key::ArrowUp => Some(103),
        Key::PageUp => Some(104),
        Key::ArrowLeft => Some(105),
        Key::ArrowRight => Some(106),
        Key::End => Some(107),
        Key::ArrowDown => Some(108),
        Key::PageDown => Some(109),
        Key::Insert => Some(110),
        Key::Delete => Some(111),
        // Key::AudioVolumeMute => Some(113),
        // Key::AudioVolumeDown => Some(114),
        // Key::AudioVolumeUp => Some(115),
        // Key::NumpadEqual => Some(117),
        // Key::Pause => Some(119),
        // Key::NumpadComma => Some(121),
        // Key::Lang1 => Some(122),
        // Key::Lang2 => Some(123),
        // Key::IntlYen => Some(124),
        Key::SuperLeft => Some(125),
        Key::SuperRight => Some(126),
        // Key::ContextMenu => Some(127),
        // Key::MediaTrackNext => Some(163),
        // Key::MediaPlayPause => Some(164),
        // Key::MediaTrackPrevious => Some(165),
        // Key::MediaStop => Some(166),
        Key::F13 => Some(183),
        Key::F14 => Some(184),
        Key::F15 => Some(185),
        Key::F16 => Some(186),
        Key::F17 => Some(187),
        Key::F18 => Some(188),
        Key::F19 => Some(189),
        Key::F20 => Some(190),
        Key::F21 => Some(191),
        Key::F22 => Some(192),
        Key::F23 => Some(193),
        Key::F24 => Some(194),

        _ => None,
    }
}
