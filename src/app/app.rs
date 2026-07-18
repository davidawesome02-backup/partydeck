use std::sync::mpsc;
use eframe::egui;

use super::events::{AppEvent, AppEventSender};
use super::panels;
use super::screens::{Panels, Route, Screen};
use super::state::AppState;
use crate::handler::Handler;
use crate::monitor::Monitor;
use crate::util::check_for_partydeck_update;

pub struct PartyApp {
    state: AppState,
    screen: Box<dyn Screen>,
    events_rx: mpsc::Receiver<AppEvent>,
    launched_fullscreen: bool,
}

impl PartyApp {
    pub fn new(
        ctx: egui::Context,
        monitors: Vec<Monitor>,
        handler_lite: Option<Handler>,
        fullscreen: bool
    ) -> Self {
        let (events, events_rx) = AppEventSender::channel(ctx);
        let mut state = AppState::new(events.clone(), monitors, handler_lite);
        let screen = state.mode.home_route().build(&mut state);

        if state.options.check_for_updates {
            std::thread::spawn(move || {
                if let Some(version) = check_for_partydeck_update() {
                    events.send(AppEvent::UpdateAvailable(version));
                }
            });
        }

        Self {
            state,
            screen,
            events_rx,
            launched_fullscreen: fullscreen,
        }
    }
}

impl eframe::App for PartyApp {
    fn raw_input_hook(&mut self, _ctx: &egui::Context, raw_input: &mut egui::RawInput) {
        if !raw_input.focused {
            return;
        }
        //TODO Add a better gamepad handling system
    }

    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        let state = &mut self.state;

        for event in self.events_rx.try_iter() {
            event.apply(state);
        }

        if let Some(route) = state.pending_route.take() {
            let fullscreen = matches!(route, Route::Session) || self.launched_fullscreen;
            ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(fullscreen));
            self.screen = route.build(state);
        }

        if ctx.input(|input| input.focused) {
            ctx.request_repaint_after(std::time::Duration::from_millis(33)); // 30 fps
        }
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let PartyApp { state, screen, .. } = self;

        let panels = screen.panels(state);

        if panels.top {
            egui::Panel::top("top_panel").show(ui, |ui| {
                panels::top_panel(state, panels.tab, ui);
            });
        }

        if panels.left {
            egui::Panel::left("left_panel")
                .resizable(false)
                .exact_size(200.0)
                .show(ui, |ui| {
                    panels::left_panel(state, ui);
                });
        }

        if panels.right {
            egui::Panel::right("right_panel")
                .resizable(false)
                .exact_size(180.0)
                .show(ui, |ui| {
                    panels::right_panel(state, ui);
                });
        }

        if let Some(height) = panels.bottom {
            egui::Panel::bottom("bottom_panel").exact_size(height).show(ui, |ui| {
                egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
                    screen.bottom_panel(state, ui);
                });
            });
        }

        let central = match panels == Panels::default() {
            true => egui::CentralPanel::default().frame(egui::Frame::NONE),
            false => egui::CentralPanel::default(),
        };
        central.show(ui, |ui| {
            screen.ui(state, ui);
        });

        state.toasts.show(ui.ctx());
    }
}
