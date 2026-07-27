use eframe::egui::{self, Ui};

use crate::app::config::*;
use crate::app::events::spawn_trash_removal;
use crate::app::screens::{NavTab, Panels, Screen};
use crate::app::state::AppState;
use crate::app::toasts::Severity;
use crate::paths::PATH_PARTY;
use crate::util::{open_dir, trash_dir};

#[derive(Default, PartialEq)]
pub enum SettingsTab {
    #[default]
    General,
    Proton,
    Gamescope,
}

#[derive(Default)]
pub struct SettingsScreen {
    tab: SettingsTab,
    info: &'static str,
    confirm_erase: bool,
}

impl Screen for SettingsScreen {
    fn panels(&self, state: &AppState) -> Panels {
        Panels::standard(state).tab(NavTab::Settings).bottom(Panels::INFO_HEIGHT)
    }

    fn bottom_panel(&mut self, _state: &mut AppState, ui: &mut Ui) {
        ui.label(self.info);
    }

    fn ui(&mut self, state: &mut AppState, ui: &mut Ui) {
        self.info = "";
        ui.horizontal(|ui| {
            ui.heading("Settings");
            ui.selectable_value(&mut self.tab, SettingsTab::General, "General");
            ui.selectable_value(&mut self.tab, SettingsTab::Proton, "Proton");
            ui.selectable_value(&mut self.tab, SettingsTab::Gamescope, "Gamescope");
        });
        ui.separator();

        ui.with_layout(egui::Layout::bottom_up(egui::Align::Center), |ui| {
            ui.horizontal(|ui| {
                if ui.button("Save Settings").clicked() {
                    if let Err(e) = save_cfg(&state.options) {
                        state.toasts.push(Severity::Error, "Couldn't save settings", e.to_string());
                    }
                }
                if ui.button("Restore Defaults").clicked() {
                    state.options = PartyConfig::default();
                    state.rescan_input_devices();
                }
            });
            ui.separator();
            ui.with_layout(egui::Layout::top_down(egui::Align::Min), |ui| {
                egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| match self.tab {
                    SettingsTab::General => self.settings_general(state, ui),
                    SettingsTab::Proton => self.settings_proton(state, ui),
                    SettingsTab::Gamescope => self.settings_gamescope(state, ui),
                });
            });
        });

        self.erase_modal(state, ui.ctx());
    }
}

impl SettingsScreen {
    fn erase_modal(&mut self, state: &mut AppState, ctx: &egui::Context) {
        if !self.confirm_erase {
            return;
        }

        let (confirmed, cancelled) = egui::Modal::new(egui::Id::new("erase_prefixes")).show(ctx, |ui| {
            ui.set_max_width(420.0);
            ui.heading("Erase Proton prefix data?");
            ui.label("This will erase all Proton prefixes used by PartyDeck. This shouldn't erase profile/game-specific data, but exercise caution.");
            ui.add_space(8.0);
            ui.horizontal(|ui| (ui.button("Erase").clicked(), ui.button("Cancel").clicked())).inner
        }).inner;

        self.confirm_erase = !(confirmed || cancelled);
        if !confirmed {
            return;
        }

        let prefixes = PATH_PARTY.join("prefixes");
        if prefixes.exists() {
            match trash_dir(&prefixes) {
                Ok(trash) => spawn_trash_removal(&state.events, trash),
                Err(e) => {
                    state.toasts.push(Severity::Error, "Couldn't erase prefix data", e);
                    return;
                }
            }
        }
        state.toasts.push(Severity::Info, "Proton prefix data erased", "");
    }

    fn settings_general(&mut self, state: &mut AppState, ui: &mut Ui) {
        let check_for_app_updates = ui.checkbox(
            &mut state.options.check_for_updates,
            "Check for partydeck updates",
        );
        self.hint(check_for_app_updates.hovered(), "DEFAULT: Enabled\n\nWARNING: CONTACTS GITHUB's SERVERS ON EVERY LAUNCH\nMakes partydeck check online for updates during each launch, and notifies user when available.");

        ui.horizontal(|ui| {
            let filter_label = ui.label("Controller filter");
            let r1 = ui.radio_value(
                &mut state.options.pad_filter_type,
                PadFilterType::All,
                "All controllers",
            );
            let r2 = ui.radio_value(
                &mut state.options.pad_filter_type,
                PadFilterType::NoSteamInput,
                "No Steam Input",
            );
            let r3 = ui.radio_value(
                &mut state.options.pad_filter_type,
                PadFilterType::OnlySteamInput,
                "Only Steam Input",
            );

            let radios = r1 | r2 | r3;
            self.hint(filter_label.hovered() || radios.hovered(), "DEFAULT: No Steam Input\n\nSelect which controllers to filter out. If you use Steam Input to remap controllers, you may want to select \"Only Steam Input\", but be warned that this option is experimental and is known to break certain Proton games.");

            if radios.clicked() {
                state.rescan_input_devices();
            }
        });

        let profile_unique_dirs_check = ui.checkbox(
            &mut state.options.profile_unique_dirs,
            "Unique per-profile environments",
        );
        self.hint(profile_unique_dirs_check.hovered(), "DEFAULT: Enabled\n\nGives each profile their own data directories. For Windows games, this is the C:\\Users\\steamuser folder, for Linux native games this is the HOME directory. Note that disabling this means that PartyDeck instances may potentially modify your game's actual save data on disk.");

        let allow_multiple_instances_on_same_device_check = ui.checkbox(
            &mut state.options.allow_multiple_instances_on_same_device,
            "(Debug) Allow multiple instances from one gamepad",
        );
        self.hint(allow_multiple_instances_on_same_device_check.hovered(), "DEFAULT: Disabled\n\nAllow multiple instances on the same device. This can be useful for testing or when one person wants to control multiple instances.");

        let disable_mount_gamedirs_check = ui.checkbox(
            &mut state.options.disable_mount_gamedirs,
            "(Debug) Force run instances from original game directory",
        );
        self.hint(disable_mount_gamedirs_check.hovered(), "DEFAULT: Disabled\n\nBy default, PartyDeck mounts game directories using fuse-overlayfs to let each instance write to the game's directory without conflicting with each other or affecting the game's installation. In addition, this lets handlers overlay content like mods or config files onto the game directory. Enabling this forces instances to launch from the original game directory without mounting, which will prevent handlers from using built-in mods, but may be useful for diagnosing issues.");

        ui.separator();

        if ui.button("Open PartyDeck Data Folder").clicked() {
            if let Err(e) = open_dir(&PATH_PARTY) {
                state.toasts.push(Severity::Error, "Couldn't open PartyDeck data folder", e);
            }
        }
    }

    fn settings_proton(&mut self, state: &mut AppState, ui: &mut Ui) {
        ui.horizontal(|ui| {
            let proton_ver_label = ui.label("Proton version");
            let proton_ver_editbox = ui.add(
                egui::TextEdit::singleline(&mut state.options.proton_version).hint_text("GE-Proton"),
            );
            self.hint(proton_ver_label.hovered() || proton_ver_editbox.hovered(), "DEFAULT: GE-Proton\n\nSpecify a Proton version. This can be a path, e.g. \"/path/to/proton\" or just a name, e.g. \"GE-Proton\" for the latest version of Proton-GE. If left blank, this will default to \"GE-Proton\". If unsure, leave this blank.");
        });

        let proton_separate_pfxs_check = ui.checkbox(
            &mut state.options.proton_separate_pfxs,
            "Run instances in separate Proton prefixes",
        );
        self.hint(proton_separate_pfxs_check.hovered(), "DEFAULT: Enabled\n\nRuns each instance in separate Proton prefixes. If unsure, leave this checked. Multiple prefixes takes up more disk space, but generally provides better compatibility and fewer issues with Proton-based games.");

        let proton_wow64_check = ui.checkbox(&mut state.options.proton_wow64, "Run Proton in WoW64 mode");
        self.hint(proton_wow64_check.hovered(), "DEFAULT: Enabled\n\nRuns Proton games in the new Wine WoW64 mode. If unsure, leave this checked.");

        if ui.button("Erase All Proton Prefix Data").clicked() {
            self.confirm_erase = true;
        }
    }

    fn settings_gamescope(&mut self, state: &mut AppState, ui: &mut Ui) {
        let gamescope_lowres_fix_check = ui.checkbox(
            &mut state.options.gamescope_fix_lowres,
            "Automatically fix low resolution instances",
        );
        self.hint(gamescope_lowres_fix_check.hovered(), "Many games have graphical problems or even crash when running at resolutions below 600p. If this is enabled, any instances below 600p will automatically be resized before launching.");
    }

    fn hint(&mut self, hovered: bool, text: &'static str) {
        if hovered {
            self.info = text;
        }
    }
}
