use eframe::egui::{self, Ui};

use crate::app::screens::{Panels, Route, Screen};
use crate::app::state::AppState;
use crate::handler::HANDLER_SPEC_CURRENT_VERSION;
use crate::util::msg;

pub struct GameScreen;

impl Screen for GameScreen {
    fn panels(&self, state: &AppState) -> Panels {
        Panels::standard(state).bottom(Panels::INFO_HEIGHT)
    }

    fn bottom_panel(&mut self, state: &mut AppState, ui: &mut Ui) {
        if let Some(h) = state.mode.active_handler() {
            ui.label(&h.info);
        }
    }

    fn ui(&mut self, state: &mut AppState, ui: &mut Ui) {
        let Some(h) = state.mode.active_handler() else {
            return;
        };

        let mut play_clicked = false;

        ui.horizontal(|ui| {
            ui.image(h.icon());
            ui.heading(h.display());
        });

        ui.separator();

        ui.horizontal(|ui| {
            let playbtn = ui.button("Play");
            if playbtn.clicked() {
                play_clicked = true;
            }

            ui.add(egui::Separator::default().vertical());
            if h.win() {
                ui.label("\u{e61f} Proton");
            } else {
                ui.label("🐧 Native");
            }
            if !h.author.is_empty() {
                ui.add(egui::Separator::default().vertical());
                ui.label(format!("Author: {}", h.author));
            }
            if !h.version.is_empty() {
                ui.add(egui::Separator::default().vertical());
                ui.label(format!("Version: {}", h.version));
            }
        });

        egui::ScrollArea::horizontal()
            .max_width(f32::INFINITY)
            .show(ui, |ui| {
                let available_height = ui.available_height();
                ui.horizontal(|ui| {
                    for img in h.img_paths.iter() {
                        ui.add(
                            egui::Image::new(format!("file://{}", img.display()))
                                .fit_to_exact_size(egui::vec2(
                                    available_height * 1.77,
                                    available_height,
                                ))
                                .maintain_aspect_ratio(true),
                        );
                    }
                });
            });

        if play_clicked {
            play(state);
        }
    }
}

fn play(state: &mut AppState) {
    let Some(h) = state.mode.active_handler() else {
        return;
    };

    if h.spec_ver != HANDLER_SPEC_CURRENT_VERSION {
        let (age, advice) = if h.spec_ver < HANDLER_SPEC_CURRENT_VERSION {
            ("an older", "Updated handlers are available via the ⮋ button on the top bar.")
        } else {
            ("a newer", "Consider updating PartyDeck.")
        };
        msg(
            "Handler version mismatch",
            &format!("This handler was made for {age} version of PartyDeck and may not work correctly. {advice} If it runs fine, update the handler's spec version to silence this warning."),
        );
    }

    if h.steam_appid.is_none() && h.path_gameroot.is_empty() {
        msg(
            "Game root path not found",
            "Please specify the game's root folder.",
        );
        state.pending_route = Some(Route::EditHandler(h.clone()));
    } else {
        state.rescan_input_devices();
        state.rescan_monitors();
        state.pending_route = Some(Route::Instances);
    }
}
