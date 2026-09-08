use std::collections::HashMap;
use std::sync::Arc;

use eframe::egui;
use std::sync::Mutex;

use super::config::{PartyConfig, load_cfg};
use super::screens::Route;
use super::events::AppEventSender;
use super::toasts::Toasts;
use crate::handler::{Handler, scan_handlers};
use crate::launch::LaunchPlan;
use crate::monitor::{Monitor, get_monitors_errorless};
use crate::remote::websocket;
use crate::session::{InstanceId, Session};
use crate::video::egl::EglApi;
use crate::video::pipewire::PipewireInstance;
use crate::input;
use crate::remote::encoder;

pub struct AppState {
    pub options: PartyConfig,

    pub monitors: Vec<Monitor>,
    pub profiles: Vec<String>,

    pub session: Session,
    pub pending_route: Option<Route>,

    pub mode: Mode,

    pub events: AppEventSender,
    pub toasts: Toasts,
    pub active_session: Option<Arc<LaunchPlan>>,

    pub egl: Arc<EglApi>,
    pub pipewire: Option<PipewireInstance>,

    pub input_state: input::InputState,

    pub remote_con: websocket::RemoteConnection,

    // pub encoder: encoder::EncoderRegistry
}

pub enum Mode {
    Full { handlers: Vec<Handler>, selected: usize },
    Lite { handler: Handler }
}

impl Mode {
    pub fn is_lite(&self) -> bool {
        matches!(self, Mode::Lite { .. })
    }

    pub fn home_route(&self) -> Route {
        match self {
            Mode::Full { .. } => Route::Home,
            Mode::Lite { .. } => Route::Instances,
        }
    }

    pub fn active_handler(&self) -> Option<&Handler> {
        match self {
            Mode::Full { handlers, selected } => handlers.get(*selected),
            Mode::Lite { handler } => Some(handler),
        }
    }

    pub fn rescan_handlers(&mut self) {
        if let Mode::Full { handlers, selected } = self {
            *handlers = scan_handlers();
            *selected = (*selected).min(handlers.len().saturating_sub(1));
        }
    }
}

impl AppState {
    pub fn new(
        events: AppEventSender,
        monitors: Vec<Monitor>,
        handler_lite: Option<Handler>,
        egl: Arc<EglApi>,
        ctx: egui::Context,
    ) -> Self {
        let options = load_cfg();
        let mode = match handler_lite {
            Some(handler) => Mode::Lite { handler },
            None => Mode::Full {
                handlers: scan_handlers(),
                selected: 0,
            },
        };

        let pipewire = PipewireInstance::new()
            .inspect_err(|e| eprintln!("Failed to start pipewire thread: {e}"))
            .ok();

        let encoder = encoder::EncoderRegistry::new(pipewire.as_ref().unwrap());

        let input_state = input::InputState::new().unwrap();

        let remote_con = websocket::RemoteConnection::new(Arc::new(Mutex::new(encoder)), ctx.clone()).unwrap();
        remote_con.channel.send(websocket::RemoteCommand::Connect).unwrap();

        Self {
            options,
            monitors,
            profiles: Vec::new(),
            session: Session::default(),
            pending_route: None,
            mode,
            events,
            toasts: Toasts::default(),
            active_session: None,
            egl,
            pipewire,
            input_state,
            remote_con,
            // encoder
        }
    }

    pub fn can_launch(&self) -> bool {
        self.session.can_launch() && self.active_session.is_none()
    }

    pub fn rescan_monitors(&mut self) {
        self.monitors = get_monitors_errorless();
        let max = self.monitors.len().saturating_sub(1);
        for display in &mut self.session.displays {
            display.monitor_idx = display.monitor_idx.min(max);
        }
    }
}
