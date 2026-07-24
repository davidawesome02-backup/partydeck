use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use eframe::egui::{self, Color32, Rect, UiBuilder, vec2};

use crate::app::screens::Screen;
use crate::app::state::AppState;
use crate::app::toasts::{Severity, Toasts};
use crate::launch::LaunchPlan;
use crate::session::InstanceId;
use crate::video::gamescope::InstanceStreamView;

pub struct SessionScreen {
    plan: Arc<LaunchPlan>,
    // Arc/Mutex so the deferred per-display viewport closures can share the
    // views without borrowing the screen.
    views: Arc<Mutex<HashMap<InstanceId, InstanceStreamView>>>,
}

impl SessionScreen {
    pub fn new(plan: Arc<LaunchPlan>) -> Self {
        Self {
            plan,
            views: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Wrap connections handed over by the launch worker in stream views.
    /// Runs mid-frame on the UI thread, where the GL context is current for
    /// the video texture creation.
    fn adopt_pending_streams(&mut self, state: &mut AppState, ctx: &egui::Context) {
        if state.pending_streams.is_empty() {
            return;
        }
        let Some(pipewire) = &state.pipewire else {
            state.pending_streams.clear();
            state.toasts.push(
                Severity::Error,
                "No video streams",
                "PipeWire thread is not running.",
            );
            return;
        };

        let mut views = self.views.lock().unwrap();
        for (id, connection) in state.pending_streams.drain() {
            let display_idx = self
                .plan
                .instances()
                .iter()
                .find(|spec| spec.id == id)
                .map_or(0, |spec| spec.display);
            match InstanceStreamView::new(
                &state.egl,
                connection,
                pipewire.channel.clone(),
                pipewire.streams.clone(),
                ctx,
                session_viewport_id(display_idx),
            ) {
                Ok(view) => {
                    views.insert(id, view);
                }
                Err(err) => state.toasts.push_for(id, Severity::Error, "No video stream", err),
            }
        }
    }
}

impl Screen for SessionScreen {
    fn pinned_monitor(&self) -> Option<String> {
        self.plan.displays().first().map(|display| display.name.clone())
    }

    fn ui(&mut self, state: &mut AppState, ui: &mut egui::Ui) {
        self.adopt_pending_streams(state, ui.ctx());

        display_ui(ui, &self.plan, 0, &state.toasts, &self.views);

        for (idx, display) in self.plan.displays().iter().enumerate().skip(1) {
            let plan = Arc::clone(&self.plan);
            let toasts = state.toasts.clone();
            let views = Arc::clone(&self.views);
            ui.ctx().show_viewport_deferred(
                session_viewport_id(idx),
                egui::ViewportBuilder::default()
                    .with_title(format!("PartyDeck — {}", display.name))
                    .with_monitor_name(display.name.clone())
                    .with_fullscreen(true),
                move |ui, _class| {
                    egui::CentralPanel::default().frame(egui::Frame::NONE).show(ui, |ui| {
                        display_ui(ui, &plan, idx, &toasts, &views);
                    });
                },
            );
        }
    }
}

/// The egui viewport a session display's tiles render in, used both to spawn
/// the extra display viewports and as the repaint target for stream frames.
fn session_viewport_id(display_idx: usize) -> egui::ViewportId {
    if display_idx == 0 {
        egui::ViewportId::ROOT
    } else {
        egui::ViewportId::from_hash_of(("session-display", display_idx))
    }
}

fn display_ui(
    ui: &mut egui::Ui,
    plan: &LaunchPlan,
    display_idx: usize,
    toasts: &Toasts,
    views: &Mutex<HashMap<InstanceId, InstanceStreamView>>,
) {
    let display = &plan.displays()[display_idx];

    let screen = ui.max_rect();
    ui.painter().rect_filled(screen, 0.0, Color32::BLACK);

    let scale_x = screen.width() / display.width as f32;
    let scale_y = screen.height() / display.height as f32;

    let mut views = views.lock().unwrap();
    for spec in plan.instances().iter().filter(|spec| spec.display == display_idx) {
        let tile = Rect::from_min_size(
            screen.min + vec2(spec.rect.x as f32 * scale_x, spec.rect.y as f32 * scale_y),
            vec2(spec.rect.w as f32 * scale_x, spec.rect.h as f32 * scale_y),
        );

        if let Some(view) = views.get_mut(&spec.id) {
            ui.scope_builder(UiBuilder::new().max_rect(tile), |ui| {
                if let Err(err) = view.ui(ui, tile.size()) {
                    eprintln!("[partydeck] Stream error for {}: {err}", spec.profname);
                }
            });
        }

        toasts.show_for_instance(ui.ctx(), spec.id, tile);
    }
}
