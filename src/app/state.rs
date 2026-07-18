use std::sync::Arc;

use super::config::{PartyConfig, load_cfg};
use super::screens::Route;
use super::events::AppEventSender;
use super::toasts::Toasts;
use crate::handler::{Handler, scan_handlers};
use crate::input::{InputDevice, scan_input_devices};
use crate::launch::LaunchPlan;
use crate::monitor::{Monitor, get_monitors_errorless};
use crate::session::Session;

pub struct AppState {
    pub options: PartyConfig,

    pub monitors: Vec<Monitor>,
    pub input_devices: Vec<InputDevice>,
    pub profiles: Vec<String>,

    pub session: Session,
    pub pending_route: Option<Route>,

    pub mode: Mode,

    pub events: AppEventSender,
    pub toasts: Toasts,
    pub active_session: Option<Arc<LaunchPlan>>,
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
    ) -> Self {
        let options = load_cfg();
        let input_devices = scan_input_devices(&options.pad_filter_type);
        let mode = match handler_lite {
            Some(handler) => Mode::Lite { handler },
            None => Mode::Full {
                handlers: scan_handlers(),
                selected: 0,
            },
        };

        Self {
            options,
            monitors,
            input_devices,
            profiles: Vec::new(),
            session: Session::default(),
            pending_route: None,
            mode,
            events,
            toasts: Toasts::default(),
            active_session: None,
        }
    }

    pub fn can_launch(&self) -> bool {
        self.session.can_launch() && self.active_session.is_none()
    }

    pub fn rescan_monitors(&mut self) {
        self.monitors = get_monitors_errorless();
        let max = self.monitors.len().saturating_sub(1);
        for display in &mut self.session.displays {
            display.monitor = display.monitor.min(max);
        }
    }

    pub fn rescan_input_devices(&mut self) {
        self.input_devices = scan_input_devices(&self.options.pad_filter_type);
        let present: Vec<_> = self.input_devices.iter().map(|device| device.hash()).collect();
        self.session.retain_devices(&present);
    }
}
