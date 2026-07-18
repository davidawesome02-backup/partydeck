use eframe::egui::{self, Align2, Color32, FontId, Rect, Stroke, vec2};

use crate::app::screens::Screen;
use crate::app::state::AppState;

pub struct SessionScreen;

impl Screen for SessionScreen {
    fn ui(&mut self, state: &mut AppState, ui: &mut egui::Ui) {
        let Some(plan) = state.active_session.clone() else {
            return;
        };

        let screen = ui.max_rect();
        let painter = ui.painter();
        painter.rect_filled(screen, 0.0, Color32::BLACK);

        // Placeholder tiles
        for spec in plan.instances().iter() {
            let Some(monitor) = state.monitors.get(spec.monitor) else {
                continue;
            };
            let scale_x = screen.width() / monitor.width() as f32;
            let scale_y = screen.height() / monitor.height() as f32;
            let tile = Rect::from_min_size(
                screen.min
                    + vec2(spec.rect.x as f32 * scale_x, spec.rect.y as f32 * scale_y),
                vec2(spec.rect.w as f32 * scale_x, spec.rect.h as f32 * scale_y),
            );

            painter.rect_stroke(tile, 2.0, Stroke::new(2.0, spec.color), egui::StrokeKind::Inside);

            let label = format!("{}", spec.profname);

            painter.text(
                tile.center(),
                Align2::CENTER_CENTER,
                label,
                FontId::proportional(16.0),
                Color32::GRAY,
            );

            state.toasts.show_for_instance(ui.ctx(), spec.id, tile);
        }
    }
}
