use std::path::PathBuf;

use eframe::egui::{self, Ui};
use rfd::FileDialog;

use crate::app::screens::{Panels, Route, Screen};
use crate::app::state::AppState;
use crate::handler::*;
use crate::paths::*;
use crate::util::*;

pub struct EditHandlerScreen {
    pub handler: Handler,
    pub installed_steamapps: Vec<steamlocate::App>,
}

impl EditHandlerScreen {
    pub fn new(handler: Handler) -> Self {
        Self {
            handler,
            installed_steamapps: get_installed_steamapps(),
        }
    }
}

impl Screen for EditHandlerScreen {
    fn panels(&self, state: &AppState) -> Panels {
        Panels::standard(state).bottom(Panels::INFO_HEIGHT)
    }

    fn bottom_panel(&mut self, _state: &mut AppState, ui: &mut Ui) {
        ui.add(
            egui::TextEdit::multiline(&mut self.handler.info)
                .hint_text("Put game info/instructions here"),
        );
    }

    fn ui(&mut self, state: &mut AppState, ui: &mut Ui) {
        let h = &mut self.handler;

        let header = match h.is_saved_handler() {
            false => "Add Game",
            true => &format!("Edit Handler: {}", h.display()),
        };

        ui.heading(header);
        ui.separator();

        ui.horizontal(|ui| {
            ui.label("Name:");
            ui.add(egui::TextEdit::singleline(&mut h.name).desired_width(150.0));
            ui.label("Author:");
            ui.add(egui::TextEdit::singleline(&mut h.author).desired_width(50.0));
            ui.label("Version:");
            ui.add(egui::TextEdit::singleline(&mut h.version).desired_width(50.0));
            ui.label("Icon:");
            ui.add(egui::Image::new(h.icon()).max_width(16.0).corner_radius(2));
            if h.is_saved_handler() && ui.button("🖼").clicked() {
                if let Some(file) = FileDialog::new()
                    .set_title("Choose Icon:")
                    .set_directory(&*PATH_HOME)
                    .add_filter("PNG Image", &["png"])
                    .pick_file()
                    && let Some(extension) = file.extension()
                    && extension == "png"
                {
                    let dest = h.path_handler.join("icon.png");
                    if let Err(e) = std::fs::copy(file, dest) {
                        eprintln!("Failed to copy icon: {}", e);
                        msg("Error copying icon", &format!("{}", e));
                    }
                }
            }
        });

        ui.separator();

        let selected_label = match h.steam_appid {
            None => "None".to_string(),
            Some(id) => match self.installed_steamapps.iter().find(|app| app.app_id == id) {
                Some(app) => format!("({}) {}", app.app_id, app.install_dir),
                None => format!("⚠ Missing Steam app ({id})"),
            },
        };

        ui.horizontal(|ui| {
            ui.label("Steam App:");
            egui::ComboBox::from_id_salt("appid")
                .wrap()
                .width(200.0)
                .selected_text(selected_label)
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut h.steam_appid, None, "None");
                    for app in &self.installed_steamapps {
                        ui.selectable_value(
                            &mut h.steam_appid,
                            Some(app.app_id),
                            format!("({}) {}", app.app_id, app.install_dir),
                        );
                    }
                });

            ui.checkbox(&mut h.use_goldberg, "Emulate Steam Client");
            ui.checkbox(&mut h.use_mangohud, "Enable MangoHud");
        });

        if h.steam_appid == None {
            ui.horizontal(|ui| {
                ui.label("Game root folder:");
                ui.add_enabled(false, egui::TextEdit::singleline(&mut h.path_gameroot));
                if ui.button("🗁").clicked() {
                    if let Ok(path) = dir_dialog() {
                        h.path_gameroot = path.to_string_lossy().to_string();
                    }
                }
            });
        }

        ui.horizontal(|ui| {
            ui.label("Executable:");
            ui.add_enabled(false, egui::TextEdit::singleline(&mut h.exec));
            if ui.button("🗁").clicked() {
                if let Ok(base_path) = h.get_game_rootpath()
                    && let Ok(path) = file_dialog_relative(&PathBuf::from(base_path))
                {
                    h.exec = path.to_string_lossy().to_string();
                }
            }
        });

        ui.horizontal(|ui| {
            ui.label("Environment variables:");
            ui.add(egui::TextEdit::singleline(&mut h.env));
        });

        ui.horizontal(|ui| {
            ui.label("Arguments:");
            ui.add(egui::TextEdit::singleline(&mut h.args));
        });

        if !h.win() {
            ui.horizontal(|ui| {
                ui.label("SDL2 Override:");
                ui.radio_value(&mut h.sdl2_override, SDL2Override::No, "None");
                ui.radio_value(
                    &mut h.sdl2_override,
                    SDL2Override::Srt,
                    "Steam Runtime (32-bit)",
                );
                ui.radio_value(
                    &mut h.sdl2_override,
                    SDL2Override::Sys,
                    "System Installation",
                );
            });

            ui.horizontal(|ui| {
                ui.label("Linux Runtime:");
                ui.radio_value(&mut h.runtime, "".to_string(), "None");
                ui.radio_value(&mut h.runtime, "scout".to_string(), "1.0 (scout)");
                ui.radio_value(&mut h.runtime, "soldier".to_string(), "2.0 (soldier)");
                ui.radio_value(&mut h.runtime, "sniper".to_string(), "3.0 (sniper)");
                ui.radio_value(&mut h.runtime, "steamrt4".to_string(), "4.0 (steamrt4)");
            });
        }

        if h.spec_ver != HANDLER_SPEC_CURRENT_VERSION {
            if ui.button("Update Handler Specification Version").clicked() {
                h.spec_ver = HANDLER_SPEC_CURRENT_VERSION;
                msg(
                    "Handler Specification Version Updated",
                    "Remember to save your changes.",
                );
            }
        }

        ui.with_layout(egui::Layout::bottom_up(egui::Align::Center), |ui| {
            if ui.button("Save").clicked() {
                if let Err(e) = h.save_to_json() {
                    msg("Error saving handler", &format!("{}", e));
                } else {
                    state.mode.rescan_handlers();
                    state.pending_route = Some(Route::Game);
                }
            }
        });
    }
}
