use std::sync::mpsc;
use std::path::PathBuf;

use eframe::egui;

use super::state::AppState;
use super::toasts::Severity;
use crate::session::InstanceId;
use crate::util::remove_trash;
// use crate::video::gamescope::GamescopeConnection;

/// Status feedback from background threads to the UI.
pub enum AppEvent {
    UpdateAvailable(String),
    Toast(Severity, String, String, Option<InstanceId>),
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

pub fn spawn_trash_removal(events: &AppEventSender, trash: PathBuf) {
    let events = events.clone();
    std::thread::spawn(move || {
        if let Err(e) = remove_trash(&trash) {
            events.send(AppEvent::Toast(
                Severity::Error,
                "Failed to remove files".into(),
                e,
                None,
            ));
        }
    });
}
