use eframe::egui::{self, Color32, Popup, RichText, Ui};

use crate::app::events::spawn_trash_removal;
use crate::app::screens::{NavTab, Route};
use crate::app::state::{AppState, Mode};
use crate::app::toasts::Severity;
use crate::handler::{Handler, import_pd2};
use crate::input::DeviceType;
use crate::util::open_dir;

pub fn top_panel(state: &mut AppState, tab: Option<NavTab>, ui: &mut Ui) {
    ui.horizontal(|ui| {
        let hometext = match state.mode.is_lite() {
            true => "▶",
            false => "ℹ",
        };

        let homebtn = ui.add(
            egui::Button::image_and_text(egui::include_image!("../../res/BTN_EAST.png"), hometext)
                .selected(tab == Some(NavTab::Home)),
        );
        if homebtn.clicked() {
            state.pending_route = Some(state.mode.home_route());
        }

        let settingsbtn = ui.add(
            egui::Button::image_and_text(egui::include_image!("../../res/BTN_NORTH.png"), "⛭")
                .selected(tab == Some(NavTab::Settings)),
        );
        if settingsbtn.clicked() {
            state.pending_route = Some(Route::Settings);
        }

        let profilesbtn = ui.add(
            egui::Button::image_and_text(egui::include_image!("../../res/BTN_WEST.png"), "👥")
                .selected(tab == Some(NavTab::Profiles)),
        );
        if profilesbtn.clicked() {
            state.pending_route = Some(Route::Profiles);
        }

        if ui.button("🖵 🔄").clicked() {
            state.rescan_monitors();
        }

        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui.button("❌").clicked() {
                ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
            }
            ui.add(egui::Separator::default().vertical());
            let version_label = match state.options.check_for_updates {
                true => concat!("v", env!("CARGO_PKG_VERSION")),
                false => concat!("(Frozen) v", env!("CARGO_PKG_VERSION")),
            };
            ui.hyperlink_to(version_label, "https://github.com/partydeck/partydeck/releases");
            ui.add(egui::Separator::default().vertical());
            ui.hyperlink_to("⮋", "https://drive.proton.me/urls/D9HBKM18YR#zG8XC8yVy9WL")
                .on_hover_text("Download Game Handlers");
            ui.hyperlink_to("♥", "https://ko-fi.com/wunner")
                .on_hover_text("Support PartyDeck Development");
            ui.hyperlink_to(
                "🖹",
                "https://github.com/partydeck/partydeck/tree/main?tab=License-2-ov-file",
            )
            .on_hover_text("Third-Party Licenses");
            ui.hyperlink_to("\u{e624}", "https://github.com/partydeck/partydeck")
                .on_hover_text("GitHub");
        });
    });
}

#[derive(Default)]
pub struct LeftPanel {
    pending_removal: Option<Handler>,
}

impl LeftPanel {
    pub fn show(&mut self, state: &mut AppState, ui: &mut Ui) {
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            ui.heading("Games");
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button("➕").clicked() {
                    state.pending_route = Some(Route::EditHandler(Handler::default()));
                }
                if ui.button("⬇").clicked() {
                    if let Err(e) = import_pd2() {
                        state.toasts.push(Severity::Error, "Couldn't import handler", e.to_string());
                    } else {
                        state.mode.rescan_handlers();
                    }
                }
                if ui.button("🔄").clicked() {
                    state.mode.rescan_handlers();
                }
            });
        });
        ui.separator();

        let AppState { mode, pending_route, toasts, .. } = state;
        egui::ScrollArea::vertical().show(ui, |ui| {
            let Mode::Full { handlers, selected } = mode else {
                return;
            };
            for (i, handler) in handlers.iter().enumerate() {
                ui.horizontal(|ui| {
                    ui.add(
                        egui::Image::new(handler.icon())
                            .max_width(16.0)
                            .corner_radius(2),
                    );

                    let btn = ui.selectable_value(selected, i, handler.display_clamp());
                    if btn.has_focus() {
                        btn.scroll_to_me(None);
                    }
                    if btn.clicked() {
                        *pending_route = Some(Route::Game);
                    };

                    Popup::context_menu(&btn).show(|ui| {
                        if ui.button("Edit").clicked() {
                            *pending_route = Some(Route::EditHandler(handler.clone()));
                        }
                        if ui.button("Open Folder").clicked() {
                            if let Err(e) = open_dir(&handler.path_handler) {
                                toasts.push(Severity::Error, "Couldn't open handler folder", e);
                            }
                        }
                        if ui.button("Remove").clicked() {
                            self.pending_removal = Some(handler.clone());
                        }
                        if ui.button("Export").clicked() {
                            if let Err(err) = handler.export_pd2() {
                                toasts.push(Severity::Error, "Couldn't export handler", err.to_string());
                            }
                        }
                    });
                });
            }
        });

        self.removal_modal(state, ui.ctx());
    }

    fn removal_modal(&mut self, state: &mut AppState, ctx: &egui::Context) {
        let Some(handler) = &self.pending_removal else { return };

        let (confirmed, cancelled) = egui::Modal::new(egui::Id::new("remove_handler")).show(ctx, |ui| {
            ui.set_max_width(420.0);
            ui.heading(format!("Remove {}?", handler.display()));
            ui.label("This permanently deletes the handler and its files.");
            ui.add_space(8.0);
            ui.horizontal(|ui| (ui.button("Remove").clicked(), ui.button("Cancel").clicked())).inner
        }).inner;

        if confirmed {
            match handler.remove_handler() {
                Ok(trash) => {
                    spawn_trash_removal(&state.events, trash);
                    state.mode.rescan_handlers();
                    if state.mode.active_handler().is_none() {
                        state.pending_route = Some(Route::Home);
                    }
                    state.toasts.push(Severity::Info, "Handler removed", "");
                }
                Err(e) => state.toasts.push(Severity::Error, "Couldn't remove handler", e),
            }
        }
        if confirmed || cancelled {
            self.pending_removal = None;
        }
    }
}

pub fn right_panel(state: &mut AppState, ui: &mut Ui) {
    ui.add_space(6.0);

    ui.heading("Devices");
    ui.separator();

    let input_state = state.input_state.inner();
    for dev in input_state.devices.values() {
        if dev.device_type() == DeviceType::Other {continue;}

        
        let mut dev_text = RichText::new(dev.label()).small();

        if !dev.enabled(&state.options.pad_filter_type) {
            dev_text = dev_text.weak();
        } else if dev.has_button_held() {
            dev_text = dev_text.strong();
        }

        ui.label(dev_text);
    }

    let orphaned_devs = input_state.targets.values().filter(|target| !input_state.devices.values().any(|dev| dev.device_id == Some(target.device_id))).collect::<Vec<_>>();
    if orphaned_devs.len()>0 {ui.separator();}
    
    for dev in orphaned_devs {
        let dev_text = RichText::new(format!("Missing - {}", dev.target_name)).small().weak().color(Color32::LIGHT_GREEN);
        ui.label(dev_text);
    }

    ui.with_layout(egui::Layout::bottom_up(egui::Align::Center), |ui| {
        ui.link("ℹ Incorrect/missing controller mappings in-game?").on_hover_ui(|ui| {
            ui.label("Some native Linux games run using an older version of SDL2 that doesn't support newer controllers; you can edit the handler and change the SDL2 Override setting to \"Steam Runtime\" for older 32-bit games, or \"System Installation\" for 64-bit games.\n\nWindows Unity-based games may not recognize input from PlayStation controllers; the current workaround for this is to use them through Steam Input, and change PartyDeck controller filter setting to \"Only Steam Input\".");
        });
        ui.link("ℹ Devices not being detected?").on_hover_ui(|ui| {
            ui.style_mut().interaction.selectable_labels = true;
            ui.label("Try adding your user to the `input` group.");
            ui.label("In a terminal, enter the following command:");
            ui.horizontal(|ui| {
                ui.code("sudo usermod -aG input $USER");
                if ui.button("📎").clicked() {
                    ui.ctx().copy_text("sudo usermod -aG input $USER".to_string());
                }
            });
        });
    });
}
