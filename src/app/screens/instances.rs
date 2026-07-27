use std::sync::Arc;

use eframe::egui::{self, Color32, RichText, Ui};

use crate::app::config::save_cfg;
use crate::app::screens::{Panels, Route, Screen};
use crate::app::events::spawn_launch_worker;
use crate::app::state::AppState;
use crate::input::{DeviceHash, DeviceInfo, DeviceType};
use crate::launch::LaunchPlan;
use crate::layout::LayoutKind;
use crate::profiles::{next_temp_name, scan_profiles};
use crate::session::{Display, InstanceAction, InstanceId};

pub struct InstancesScreen {
    edit_modal: Option<InstanceId>,
    show_right_panel: bool,
    show_bottom_panel: bool,
}


struct DeviceRow {
    hash: DeviceHash,
    label: String,
    enabled: bool,
    pressed: bool,
    device_type: DeviceType,
    already_used: bool,
}

impl Screen for InstancesScreen {
    fn panels(&self, state: &AppState) -> Panels {
        Panels {
            right: self.show_right_panel,
            bottom: self.show_bottom_panel.then_some(160.0),
            ..Panels::standard(state)
        }
    }

    fn bottom_panel(&mut self, state: &mut AppState, ui: &mut Ui) {
        self.controls(state, ui);
    }

    fn ui(&mut self, state: &mut AppState, ui: &mut Ui) {
        ui.horizontal(|ui| {
            ui.heading("Instances");

            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.add_enabled(state.can_launch(), egui::Button::new("Launch")).clicked() {
                    self.launch(state);
                }
                ui.toggle_value(&mut self.show_right_panel, "🎮")
                    .on_hover_text("Right panel");
                ui.toggle_value(&mut self.show_bottom_panel, "🛠")
                    .on_hover_text("Display controls");
            });
        });

        ui.separator();

        egui::Panel::left("display_nav_left")
            .resizable(false)
            .exact_size(35.0)
            .show_separator_line(false)
            .frame(egui::Frame::NONE)
            .show(ui, |ui| {
                ui.centered_and_justified(|ui| {
                    ui.set_height(25.0);
                    ui.set_width(25.0);
                    let is_first = state.session.selected == 0;

                    let label = if is_first { "\u{01F6AB}" } else { "⬅" };
                    if ui.button(label).clicked() && !is_first {
                        state.session.selected -= 1;
                    }
                });
            });

        egui::Panel::right("display_nav_right")
            .resizable(false)
            .exact_size(35.0)
            .show_separator_line(false)
            .frame(egui::Frame::NONE)
            .show(ui, |ui| {
                ui.centered_and_justified(|ui| {
                    ui.set_height(25.0);
                    ui.set_width(25.0);
                    let is_last = state.session.selected + 1 == state.session.displays.len();
                    let can_create_display = !state.session.selected_display().is_empty();
                    let label = if is_last {
                        if can_create_display { "✚" } else { "\u{01F6AB}" }
                    } else {
                        "➡"
                    };
                    if ui.button(label).clicked() {
                        if is_last {
                            if can_create_display {
                                state.session.displays.push(Display::default());
                                state.session.selected += 1;
                            }
                        } else {
                            state.session.selected += 1;
                        }
                    }
                });
            });

        self.preview(state, ui);
        self.edit_modal(state, ui);
    }
}

impl InstancesScreen {
    pub fn new(state: &mut AppState) -> Self {
        state.profiles = scan_profiles(false);
        let visible = !state.mode.is_lite();
        Self {
            edit_modal: None,
            show_right_panel: visible,
            show_bottom_panel: visible,
        }
    }

    fn clear_selections(&mut self) {
        self.edit_modal = None;
    }

    fn launch(&mut self, state: &mut AppState) {
        let Some(handler) = state.mode.active_handler() else {
            return;
        };
        let handler = handler.clone();
        let cfg = state.options.clone();
        let _ = save_cfg(&cfg);
        let devices: Vec<DeviceInfo> = state.input_devices.iter().map(|device| device.info()).collect();
        // TODO REPLACE!
        let plan = Arc::new(LaunchPlan::build(&state.session, &state.monitors, devices, &cfg));

        state.active_session = Some(plan.clone());
        state.pending_route = Some(Route::Session(plan.clone()));
        // TODO REPLACE!
        // spawn_launch_worker(&state.events, handler, plan, cfg);
    }

    fn open_new_instance(&mut self, state: &mut AppState) {
        self.edit_modal = Some(state.session.add_instance());
    }

    fn remove_selected_display(&mut self, state: &mut AppState) {
        state.session.remove_selected_display();
        self.clear_selections();
    }

    fn preview(&mut self, state: &mut AppState, ui: &mut Ui) {
        ui.centered_and_justified(|ui| {
            egui::Frame::NONE
                .fill(Color32::from_gray(80))
                .inner_margin(2.0)
                .corner_radius(2)
                .show(ui, |ui| {
                    let monitor = &state.monitors[state.session.selected_display().monitor_idx];
                    let target_res = (monitor.width(), monitor.height());
                    let aspect_ratio = target_res.0 as f32 / target_res.1 as f32;
                    let height = (ui.available_width() / aspect_ratio).min(ui.available_height());
                    let width = height * aspect_ratio;
                    ui.set_height(height);
                    ui.set_width(width);

                    let display: &mut Display = state.session.selected_display();
                    if display.is_empty() {
                        let mut remove_display = false;
                        ui.vertical_centered(|ui| {
                            ui.add_space((ui.available_height() / 2.0 - 15.0).max(0.0));
                            ui.label("No instances, click \"New instance\" to add.");
                            if state.session.displays.len() > 1 && ui.button("Remove display").clicked() {
                                remove_display = true;
                            }
                        });
                        if remove_display {
                            self.remove_selected_display(state);
                        }
                    } else {
                        let action = display.editor_ui(ui, width, height, target_res);
                        self.apply_instance_action(state, action);
                    }
                });
        });
    }

    fn apply_instance_action(&mut self, state: &mut AppState, action: InstanceAction) {
        match action {
            InstanceAction::None => {},
            InstanceAction::Edit(id) => self.edit_modal = Some(id),
            InstanceAction::Remove(id) => {
                state.session.remove_instance(id);
                self.clear_selections();
            }
            InstanceAction::Swap(a, b) => state.session.swap(a, b),
        }
    }

    fn controls(&mut self, state: &mut AppState, ui: &mut Ui) {
        let selected = state.session.selected;
        ui.add_space(4.0);
        ui.label(format!("Display {}/{}", selected + 1, state.session.displays.len()));
        ui.separator();

        if ui.button("New instance").clicked() {
            self.open_new_instance(state);
        }

        ui.horizontal(|ui| {
            ui.label("Layout type:");
            let mut layout_kind = state.session.selected_display().layout.kind();
            egui::ComboBox::from_id_salt("setup_layout_kind")
                .selected_text(layout_kind.to_string())
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut layout_kind, LayoutKind::Game, LayoutKind::Game.to_string());
                    ui.selectable_value(&mut layout_kind, LayoutKind::Flat, LayoutKind::Flat.to_string());
                });
            state.session.selected_display_mut().layout.set_kind(layout_kind);
        });

        if state.monitors.len() > 1 {
            ui.horizontal(|ui| {
                ui.label("🖵");
                let mut monitor = state.session.selected_display().monitor_idx;
                egui::ComboBox::from_id_salt("setup_display_monitor")
                    .selected_text(state.monitors[monitor].name())
                    .show_ui(ui, |ui| {
                        for (i, m) in state.monitors.iter().enumerate() {
                            ui.selectable_value(&mut monitor, i, m.name());
                        }
                    });
                state.session.selected_display_mut().monitor_idx = monitor;
            });
        }

        state.session.selected_display_mut().layout.editor(ui);
    }

    fn edit_modal(&mut self, state: &mut AppState, ui: &mut Ui) {
        let Some(id) = self.edit_modal else {
            return;
        };

        let used_by_others = state.session.used_profile_names(Some(id));
        let mut used_by_others_sorted: Vec<_> = used_by_others.iter().cloned().collect();
        used_by_others_sorted.sort();
        let unused_saved_profiles: Vec<_> = state
            .profiles
            .iter()
            .filter(|profile| !used_by_others.contains(*profile))
            .cloned()
            .collect();
        let allow_multiple = state.options.allow_multiple_instances_on_same_device;
        let device_rows = self.device_rows(state, id);

        egui::Modal::new(ui.make_persistent_id("setup_instance_edit_modal")).show(ui.ctx(), |ui| {
            if ui.button("Close").clicked() {
                self.edit_modal = None;
                return;
            }

            let Some(instance) = state.session.instance_mut_by_id(id) else {
                self.edit_modal = None;
                return;
            };

            ui.horizontal(|ui| {
                ui.label("Profile selection:");
                let mut profile_choice = Some(instance.profname.clone());
                egui::ComboBox::from_id_salt("setup_profile_choice")
                    .selected_text(instance.profname.clone())
                    .show_ui(ui, |ui| {
                        ui.selectable_value(&mut profile_choice, None, "New temp");
                        ui.separator();
                        for profile in &unused_saved_profiles {
                            ui.selectable_value(&mut profile_choice, Some(profile.clone()), profile);
                        }
                        ui.separator();
                        for profile in &used_by_others_sorted {
                            ui.selectable_value(&mut profile_choice, Some(profile.clone()), profile);
                        }
                    });

                if profile_choice.as_ref() != Some(&instance.profname) {
                    instance.profname =
                        profile_choice.unwrap_or_else(|| next_temp_name(&used_by_others));
                }
            });

            ui.horizontal(|ui| {
                ui.label("Outline color:");
                ui.color_edit_button_srgba(&mut instance.color);
            });

            ui.separator();
            for row in &device_rows {
                let checked_before = instance.has_device(row.hash);
                let mut checked = checked_before;
                let blocked = !checked_before
                    && !device_assignable(
                        row.enabled,
                        row.already_used,
                        allow_multiple,
                    );

                let dev_text =
                    RichText::new(&row.label).small().color(device_text_color(row, blocked));
                let response = ui
                    .add_enabled(!blocked, egui::Checkbox::new(&mut checked, dev_text))
                    .on_hover_text(device_hover_text(row, blocked));

                if response.changed() {
                    match (checked, checked_before) {
                        (true, false) => instance.devices.push(row.hash),
                        (false, true) => instance.devices.retain(|device| *device != row.hash),
                        _ => {}
                    }
                }
            }
        });
    }

    fn device_rows(&self, state: &AppState, id: InstanceId) -> Vec<DeviceRow> {
        state
            .input_devices
            .iter()
            .map(|device| {
                let hash = device.hash();
                DeviceRow {
                    hash,
                    label: device.label(),
                    enabled: device.enabled(),
                    pressed: device.has_button_held(),
                    device_type: device.device_type(),
                    already_used: state.session.device_used_by_other(hash, id),
                }
            })
            .collect()
    }
}

fn device_assignable(
    enabled: bool,
    used_by_other: bool,
    allow_multiple: bool,
) -> bool {
    let not_duplicate = !used_by_other || allow_multiple;
    enabled && not_duplicate
}

fn device_text_color(row: &DeviceRow, blocked: bool) -> Color32 {
    if blocked {
        return Color32::DARK_GRAY;
    }
    match (row.enabled, row.pressed, row.already_used) {
        (false, _, false) => Color32::RED,
        (false, _, true) => Color32::LIGHT_RED,
        (true, false, false) => Color32::GRAY,
        (true, true, false) => Color32::WHITE,
        (true, false, true) => Color32::BLUE,
        (true, true, true) => Color32::LIGHT_BLUE,
    }
}

fn device_hover_text(row: &DeviceRow, blocked: bool) -> &'static str {
    if blocked {
        return "Unavailable for this instance";
    }

    match (row.enabled, row.pressed, row.already_used) {
        (false, _, false) => "Disabled",
        (false, _, true) => "Disabled\nAlready used",
        (true, false, false) => "Available",
        (true, true, false) => "Available\nInput pressed",
        (true, false, true) => "Already used",
        (true, true, true) => "Already used\nInput pressed",
    }
}
