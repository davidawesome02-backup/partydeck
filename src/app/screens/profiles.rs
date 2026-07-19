use eframe::egui::{self, Ui};

use crate::app::screens::{NavTab, Panels, Screen};
use crate::app::state::AppState;
use crate::app::toasts::Severity;
use crate::paths::PATH_PARTY;
use crate::profiles::{create_profile, scan_profiles};
use crate::util::open_dir;

#[derive(Default)]
pub struct ProfilesScreen {
    new_name: Option<String>,
}

impl Screen for ProfilesScreen {
    fn panels(&self, state: &AppState) -> Panels {
        Panels::standard(state).tab(NavTab::Profiles).bottom(Panels::INFO_HEIGHT)
    }

    fn bottom_panel(&mut self, _state: &mut AppState, ui: &mut Ui) {
        ui.label("Create profiles to persistently store game save data, settings, and stats.");
    }

    fn ui(&mut self, state: &mut AppState, ui: &mut Ui) {
        ui.heading("Profiles");
        ui.separator();
        egui::ScrollArea::vertical()
            .max_height(ui.available_height() - 16.0)
            .auto_shrink(false)
            .show(ui, |ui| {
                for profile in &state.profiles {
                    if ui.selectable_label(false, profile).clicked() {
                        if let Err(e) = open_dir(&PATH_PARTY.join("profiles").join(profile)) {
                            state.toasts.push(Severity::Error, "Couldn't open profile directory", e);
                        }
                    };
                }
            });
        if ui.button("New").clicked() {
            self.new_name = Some(String::new());
        }

        self.new_profile_modal(state, ui);
    }
}

impl ProfilesScreen {
    pub fn new(state: &mut AppState) -> Self {
        state.profiles = scan_profiles(false);
        Self::default()
    }

    fn new_profile_modal(&mut self, state: &mut AppState, ui: &mut Ui) {
        let Some(name) = &mut self.new_name else {
            return;
        };

        let mut close = false;
        egui::Modal::new(egui::Id::new("new_profile_modal")).show(ui.ctx(), |ui| {
            ui.heading("New Profile");
            ui.separator();
            ui.label("Enter name (must be alphanumeric):");
            ui.text_edit_singleline(name);
            let valid = !name.is_empty() && name.chars().all(char::is_alphanumeric);
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                if ui.add_enabled(valid, egui::Button::new("Create")).clicked() {
                    match create_profile(name) {
                        Ok(()) => state.profiles = scan_profiles(false),
                        Err(e) => state.toasts.push(Severity::Error, "Couldn't create profile", e.to_string()),
                    }
                    close = true;
                }
                if ui.button("Cancel").clicked() {
                    close = true;
                }
            });
        });
        if close {
            self.new_name = None;
        }
    }
}
