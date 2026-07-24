use std::sync::Arc;

use eframe::egui::{self, Color32, Rect, vec2};

use crate::app::screens::Screen;
use crate::app::state::AppState;
use crate::app::toasts::Toasts;
use crate::launch::LaunchPlan;

pub struct SessionScreen {
    plan: Arc<LaunchPlan>,
}

impl SessionScreen {
    pub fn new(plan: Arc<LaunchPlan>) -> Self {
        Self { plan }
    }
}

impl Screen for SessionScreen {
    fn pinned_monitor(&self) -> Option<String> {
        self.plan.displays().first().map(|display| display.name.clone())
    }

    fn ui(&mut self, state: &mut AppState, ui: &mut egui::Ui) {
        display_ui(ui, &self.plan, 0, &state.toasts);

        for (idx, display) in self.plan.displays().iter().enumerate().skip(1) {
            let plan = Arc::clone(&self.plan);
            let toasts = state.toasts.clone();
            ui.ctx().show_viewport_deferred(
                egui::ViewportId::from_hash_of(("session-display", idx)),
                egui::ViewportBuilder::default()
                    .with_title(format!("PartyDeck — {}", display.name))
                    .with_monitor_name(display.name.clone())
                    .with_fullscreen(true),
                move |ui, _class| {
                    egui::CentralPanel::default().frame(egui::Frame::NONE).show(ui, |ui| {
                        display_ui(ui, &plan, idx, &toasts);
                    });
                },
            );
        }
    }
}

fn display_ui(ui: &mut egui::Ui, plan: &LaunchPlan, display_idx: usize, toasts: &Toasts) {
    let display = &plan.displays()[display_idx];

    let screen = ui.max_rect();
    let painter = ui.painter();
    painter.rect_filled(screen, 0.0, Color32::BLACK);

    let scale_x = screen.width() / display.width as f32;
    let scale_y = screen.height() / display.height as f32;

    for spec in plan.instances().iter().filter(|spec| spec.display == display_idx) {
        let tile = Rect::from_min_size(
            screen.min + vec2(spec.rect.x as f32 * scale_x, spec.rect.y as f32 * scale_y),
            vec2(spec.rect.w as f32 * scale_x, spec.rect.h as f32 * scale_y),
        );

        toasts.show_for_instance(ui.ctx(), spec.id, tile);
    }
}
