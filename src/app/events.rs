use std::sync::{Arc, mpsc};
use std::path::PathBuf;

use eframe::egui;

use super::config::PartyConfig;
use super::screens::Route;
use super::state::AppState;
use super::toasts::Severity;
use crate::handler::Handler;
use crate::launch::{LaunchPlan, run_launch};
use crate::profiles::remove_guest_profiles;
use crate::session::InstanceId;
use crate::util::clear_tmp;

/// Status feedback from background threads to the UI.
pub enum AppEvent {
    UpdateAvailable(String),
    Toast(Severity, String, String, Option<InstanceId>),
    LaunchFinished,
}

impl AppEvent {
    pub fn apply(self, state: &mut AppState) {
        match self {
            AppEvent::UpdateAvailable(version) => state.toasts.push_linked(
                Severity::Info,
                "Update available",
                format!("PartyDeck {version} is available (current: v{}).\nClick to view the release.",
                    env!("CARGO_PKG_VERSION")),
                "https://github.com/partydeck/partydeck/releases/latest",
            ),
            AppEvent::Toast(severity, title, body, target) => match target {
                Some(id) => state.toasts.push_for(id, severity, title, body),
                None => state.toasts.push(severity, title, body),
            },
            AppEvent::LaunchFinished => {
                state.active_session = None;
                state.pending_route = Some(Route::Instances);
                state.toasts.release_targets();
            }
        }
    }
}

#[derive(Clone)]
pub struct AppEventSender {
    tx: mpsc::Sender<AppEvent>,
    ctx: egui::Context,
}

impl AppEventSender {
    pub fn channel(ctx: egui::Context) -> (Self, mpsc::Receiver<AppEvent>) {
        let (tx, rx) = mpsc::channel();
        (Self { tx, ctx }, rx)
    }

    pub fn send(&self, event: AppEvent) {
        let _ = self.tx.send(event);
        self.ctx.request_repaint();
    }
}

pub fn spawn_launch_worker(
    events: &AppEventSender,
    handler: Handler,
    plan: Arc<LaunchPlan>,
    cfg: PartyConfig,
) {
    let events = events.clone();
    std::thread::spawn(move || {
        let result = run_launch(&handler, &plan, &cfg, |spec| {
            events.send(AppEvent::Toast(
                Severity::Info,
                format!("Preparing instance for {}", spec.profname),
                String::new(),
                Some(spec.id),
            ))
        });
        if let Err(err) = result {
            println!("[partydeck] Launch failed: {}", err);
            events.send(AppEvent::Toast(Severity::Error, "Launch failed".into(), err, None));
        }

        if let Err(err) = remove_guest_profiles() {
            println!("[partydeck] Error removing guest profiles: {}", err);
            events.send(AppEvent::Toast(
                Severity::Error,
                "Failed removing guest profiles".into(),
                err.to_string(),
                None,
            ));
        }
        if let Err(err) = clear_tmp() {
            println!("[partydeck] Error removing tmp directory: {}", err);
            events.send(AppEvent::Toast(
                Severity::Error,
                "Failed removing tmp directory".into(),
                err.to_string(),
                None,
            ));
        }

        events.send(AppEvent::LaunchFinished);
    });
}

