use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use eframe::egui::{self, Color32, Rect, UiBuilder, vec2};

use crate::app::screens::Screen;
use crate::app::state::AppState;
use crate::app::toasts::{Severity, Toasts};
use crate::handler::Handler;
use crate::launch::LaunchPlan;
use crate::session::{InstanceId, Session};
use crate::video::gamescope::InstanceStreamView;

pub struct SessionScreen {
    session_data: Arc<Mutex<Session>>,
    handler: Handler,
    has_started: bool,
}

impl SessionScreen {
    pub fn new(session_data: Session, handler: Handler) -> Self {
        Self{ session_data: Arc::new(Mutex::new(session_data)), has_started: false, handler }
    }
}

impl Screen for SessionScreen {
    fn ui(&mut self, state: &mut AppState, ui: &mut egui::Ui) {
        let session_data_arc = self.session_data.clone();
        let mut session_data = self.session_data.lock().unwrap();

        if !self.has_started {
            self.has_started = true;
            let next_timeout = &mut Instant::now();
            
            for (idx, display) in session_data.displays.iter_mut().enumerate()  {
                let title = format!("PartyDeck - {}", idx+1);
                // state.egl;
                let Some(ref pipewire_st) = state.pipewire else {
                    return //TODO replace this
                };
                display.start_display(ui, next_timeout, egui::ViewportId::from_hash_of(&title), state.egl.clone(), pipewire_st, state.monitors[display.monitor_idx].clone());
            }
            println!("Starting display!");
        }


        ui.label("Running :D!");
        if ui.button("Murder the games").clicked() {
            for display in session_data.displays.iter_mut() {
                display.instances.iter_mut().for_each(|i| i.kill_game());
            }
        }


        let mut has_alive_session = false;
        for (idx, display) in session_data.displays.iter_mut().enumerate() {
            // println!("Alive: {}", );
            if !display.is_alive() {continue;}

            has_alive_session = true;
            // display.instances.iter().any(|i| let Some(a) = i.launch_data && let Some(proc_dat) = a.gamescope_proc && proc_dat.0.refr().try_wait().unwrap().is_some())
            let cloned_session_data_arc = session_data_arc.clone();
            let toasts = state.toasts.clone();
            let title = format!("PartyDeck - {}", idx+1);
            ui.ctx().show_viewport_deferred(
                egui::ViewportId::from_hash_of(&title),
                egui::ViewportBuilder::default()
                    .with_title(title)
                    .with_monitor_name(state.monitors[display.monitor_idx].name())
                    .with_fullscreen(true),
                move |ui, _class| {
                    egui::CentralPanel::default().frame(egui::Frame::NONE).show(ui, |ui| {
                        display_ui(ui, cloned_session_data_arc.clone(), &toasts, idx);
                    });
                },
            );
        }

        if !has_alive_session {
            state.pending_route = Some(super::Route::Home)
        }

        // Display help / usage stastics for the main window here. Also stop button.
    }

    fn should_capture_ctrl_c(&mut self) -> bool {
        true
    }
}

fn display_ui(ui: &mut egui::Ui, session_data: Arc<Mutex<Session>>, toasts: &Toasts, idx: usize) {
    let mut session_data = session_data.lock().unwrap();
    let display = &mut session_data.displays[idx];
    display.display_ui(ui);

    if !display.is_alive() {ui.request_repaint_of(egui::ViewportId::ROOT);} // TODO can be optimized into display_ui func.
    if ui.input(|i| i.viewport().close_requested()) {
        for inst in display.instances.iter_mut() {
            inst.kill_game();
        }
        ui.request_repaint_of(egui::ViewportId::ROOT);
    }
}