use super::app::{MenuPage, PartyApp, SettingsPage};
use super::config::*;
use crate::app::app::{Display, DisplayCompType, DisplayCompTypeKwinSplit};
use crate::handler::*;
use crate::input::*;
use crate::paths::*;
use crate::profiles::*;
use crate::util::*;
use crate::monitor::get_monitors_errorless;

use dialog::DialogBox;
use eframe::egui::accesskit::SortDirection;
use eframe::egui::{RichText, vec2};
use eframe::egui::{self, Ui};
use egui_extras::StripBuilder;
use rfd::FileDialog;
use std::path::PathBuf;

macro_rules! cur_handler {
    ($self:expr) => {
        &$self.handlers[$self.selected_handler]
    };
}

impl PartyApp {
    pub fn display_page_main(&mut self, ui: &mut Ui) {
        ui.heading("Welcome to PartyDeck");
        ui.separator();
        ui.label("Press SELECT/BACK or Tab to unlock gamepad navigation.");
        ui.label("PartyDeck is in the very early stages of development; as such, you will likely encounter bugs, issues, and strange design decisions.");
        ui.label("For debugging purposes, it's recommended to read terminal output (stdout) for further information on errors.");
        ui.separator();
        ui.horizontal_wrapped(|ui| {
            ui.label("Thank you to");
            ui.hyperlink_to("♥Ko-fi", "https://ko-fi.com/wunner");
            ui.label("supporters:");
        });
        ui.label("Framilano, Jayden, Marc, Max Rei");
        ui.horizontal_wrapped(|ui| {
            ui.label("Thank you to");
            ui.hyperlink_to(" GitHub", "https://github.com/wunnr/partydeck");
            ui.label("contributors/handler creators:")
        });
        ui.horizontal_wrapped(|ui| {
            ui.hyperlink_to("@Blahkaey", "https://github.com/Blahkaey");
            ui.hyperlink_to("@blckink", "https://github.com/blckink");
            ui.hyperlink_to("@davidawesome02", "https://github.com/davidawesome02-backup");
            ui.hyperlink_to("@felipecrs", "https://github.com/felipecrs");
            ui.hyperlink_to("@framilano", "https://github.com/framilano");
            ui.hyperlink_to("@FrancisBernard34", "https://github.com/FrancisBernard34");
            ui.hyperlink_to("@Rudicito", "https://github.com/Rudicito");
            ui.hyperlink_to("@Tau5", "https://github.com/Tau5");
            ui.hyperlink_to("@Twig6943", "https://github.com/Twig6943");
        });
    }

    pub fn display_page_settings(&mut self, ui: &mut Ui) {
        self.infotext.clear();
        ui.horizontal(|ui| {
            ui.heading("Settings");
            ui.selectable_value(&mut self.settings_page, SettingsPage::General, "General");
            ui.selectable_value(&mut self.settings_page, SettingsPage::Proton, "Proton");
            ui.selectable_value(
                &mut self.settings_page,
                SettingsPage::Gamescope,
                "Gamescope",
            );
        });
        ui.separator();

        egui::ScrollArea::vertical()
            .max_height(ui.available_height() - 30.0) // Remove lower menue height from avaliable
            .auto_shrink(false)
            .show(ui, |ui| {
                match self.settings_page {
                    SettingsPage::General => self.display_settings_general(ui),
                    SettingsPage::Proton => self.display_settings_proton(ui),
                    SettingsPage::Gamescope => self.display_settings_gamescope(ui),
                }
        });


        ui.with_layout(egui::Layout::bottom_up(egui::Align::Center), |ui| {
            ui.horizontal(|ui| {
                if ui.button("Save Settings").clicked() {
                    if let Err(e) = save_cfg(&self.options) {
                        msg("Error", &format!("Couldn't save settings: {}", e));
                    }
                }
                if ui.button("Restore Defaults").clicked() {
                    self.options = PartyConfig::default();
                    self.input_devices = scan_input_devices(&self.options.pad_filter_type);
                }
            });
            ui.separator();
        });
    }

    pub fn display_page_profiles(&mut self, ui: &mut Ui) {
        ui.heading("Profiles");
        ui.separator();
        egui::ScrollArea::vertical()
            .max_height(ui.available_height() - 16.0)
            .auto_shrink(false)
            .show(ui, |ui| {
                for profile in &self.profiles {
                    if ui.selectable_value(&mut 0, 1, profile).clicked() {
                        if let Err(_) = std::process::Command::new("xdg-open")
                            .arg(PATH_PARTY.join("profiles").join(profile))
                            .status()
                        {
                            msg("Error", "Couldn't open profile directory!");
                        }
                    };
                }
            });
        if ui.button("New").clicked() {
            if let Some(name) = dialog::Input::new("Enter name (must be alphanumeric):")
                .title("New Profile")
                .show()
                .expect("Could not display dialog box")
            {
                if !name.is_empty() && name.chars().all(char::is_alphanumeric) {
                    create_profile(&name).unwrap();
                } else {
                    msg("Error", "Invalid name");
                }
            }
            self.profiles = scan_profiles(false);
        }
    }

    pub fn display_page_edit_handler(&mut self, ui: &mut Ui) {
        let h = match &mut self.handler_edit {
            Some(handler) => handler,
            None => {
                return;
            }
        };

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

        let mut selected_index = self
            .installed_steamapps
            .iter()
            .position(|game_opt| match (game_opt, &h.steam_appid) {
                (Some(game), Some(appid)) => game.app_id == *appid,
                (None, None) => true,
                _ => false,
            })
            .unwrap_or(0);

        ui.horizontal(|ui| {
            ui.label("Steam App:");
            egui::ComboBox::from_id_salt("appid")
                .wrap()
                .width(200.0)
                .show_index(
                    ui,
                    &mut selected_index,
                    self.installed_steamapps.len(),
                    |i| match &self.installed_steamapps[i] {
                        Some(app) => format!("({}) {}", app.app_id, app.install_dir),
                        None => "None".to_string(),
                    },
                );

            ui.checkbox(&mut h.use_goldberg, "Emulate Steam Client");
        });

        h.steam_appid = match &self.installed_steamapps[selected_index] {
            Some(app) => Some(app.app_id),
            None => None,
        };

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
        }

        if !h.win() {
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
                msg("Handler Specification Version Updated", "Remember to save your changes.");
            }
        }

        ui.with_layout(egui::Layout::bottom_up(egui::Align::Center), |ui| {
            if ui.button("Save").clicked() {
                if let Err(e) = h.save_to_json() {
                    msg("Error saving handler", &format!("{}", e));
                } else {
                    self.handlers = scan_handlers();
                    self.cur_page = MenuPage::Game;
                }
            }
        });
    }

    pub fn display_page_game(&mut self, ui: &mut Ui) {
        ui.horizontal(|ui| {
            ui.image(cur_handler!(self).icon());
            ui.heading(cur_handler!(self).display());
        });

        ui.separator();

        let h = cur_handler!(self);

        ui.horizontal(|ui| {
            let playbtn = ui.add(egui::Button::image_and_text(
                egui::include_image!("../../res/BTN_START.png"),
                "Play",
            ));
            if playbtn.clicked() {
                if h.spec_ver != HANDLER_SPEC_CURRENT_VERSION {
                    let mismatch = match h.spec_ver < HANDLER_SPEC_CURRENT_VERSION {
                        true => "an older",
                        false => "a newer",
                    };
                    let mismatch2 = match h.spec_ver < HANDLER_SPEC_CURRENT_VERSION {
                        true => "Up-to-date handlers can be found by clicking the ⮋ button on the top bar of the launcher.",
                        false => "It is recommended to update PartyDeck to the latest version.",
                    };
                    msg(
                        "Handler version mismatch",
                        &format!("This handler was meant for use with {} version of PartyDeck; you may experience issues or the game may not work at all. {} If everything still works fine, you can prevent this message appearing in the future by editing the handler, updating the spec version and saving.",
                            mismatch, mismatch2
                        )
                    );
                }
                if h.steam_appid.is_none() && h.path_gameroot.is_empty() {
                    msg(
                        "Game root path not found",
                        "Please specify the game's root folder.",
                    );
                    self.handler_edit = Some(h.clone());
                    self.cur_page = MenuPage::EditHandler;
                } else {
                    self.instances.clear();
                    self.input_devices = scan_input_devices(&self.options.pad_filter_type);
                    self.monitors = get_monitors_errorless();
                    self.profiles = scan_profiles(true);
                    self.instance_add_dev = None;
                    self.cur_page = MenuPage::Instances;
                }
            }

            ui.add(egui::Separator::default().vertical());
            if h.win() {
                ui.label(" Proton");
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
    }


    // See origonal dnd impl
    pub fn dnd_drag_source_cust<Payload, R>(
        &mut self,
        self_u: &mut egui::Ui,
        id: egui::Id,
        payload: Payload,
        add_contents: impl FnOnce(&mut egui::Ui) -> R,
    ) -> egui::InnerResponse<R>
    where
        Payload: std::any::Any + Send + Sync,
    {
        let is_being_dragged = self_u.ctx().is_being_dragged(id);

        if is_being_dragged {
            egui::DragAndDrop::set_payload(self_u.ctx(), payload);

            let layer_id = egui::LayerId::new(egui::Order::Tooltip, id);
            let egui::InnerResponse { inner, response } =
                self_u.scope_builder(egui::UiBuilder::new().layer_id(layer_id), add_contents);

            if let Some(pointer_pos) = self_u.ctx().pointer_interact_pos() {
                let delta = pointer_pos - response.rect.left_center() - vec2(10.0, 0.0); // Manual correction factor
                self_u.ctx()
                    .transform_layer_shapes(layer_id, egui::emath::TSTransform::from_translation(delta));
            }

            egui::InnerResponse::new(inner, response)
        } else {
            self_u.scope(add_contents)
        }
    }

    fn instances_display_collumn(&mut self, ui: &mut Ui, col_idx: usize) {
        let display_column = self.testing_displays[col_idx].clone();

        let mut to_be_moved = None;
        let mut to_be_edited = None;

        for (row_idx, mut instan) in display_column.profile_list.into_iter().enumerate() {
            instan.display_idx = col_idx;
            instan.profile_display_idx = row_idx;
            

            let id: eframe::egui::Id = ui.make_persistent_id(&instan.prof_name);
            self.dnd_drag_source_cust(ui, id, instan.clone(), |ui| {
                let visuals = &ui.visuals().widgets.inactive;
                egui::Frame::NONE
                    .fill(visuals.bg_fill)
                    .stroke(visuals.bg_stroke)
                    .corner_radius(visuals.corner_radius)
                    .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.style_mut().visuals.widgets.inactive.bg_fill = egui::Color32::RED;
                        ui.style_mut().spacing.item_spacing.x = 3.0;

                        let test = ui.button("Ｓ").on_hover_text("Move handle");
                        ui.interact(test.rect, id, egui::Sense::drag()).on_hover_cursor(egui::CursorIcon::Grab);

                        // ui.button("🗑").on_hover_text("Remove");
                        ui.scope(|ui| {
                            if col_idx == 0 {
                                ui.style_mut().visuals.widgets.inactive.weak_bg_fill = egui::Color32::DARK_RED;
                                ui.style_mut().visuals.widgets.active.weak_bg_fill = egui::Color32::DARK_RED;
                                ui.style_mut().visuals.widgets.hovered.weak_bg_fill = egui::Color32::DARK_RED;
                            }
                            if ui.button("🗑").on_hover_text("Remove").clicked() {
                                if col_idx == 0 {
                                    // TODO
                                } else {
                                    to_be_moved = Some(row_idx);
                                }
                            }
                        });

                        if ui.button("✏").on_hover_text("Edit").clicked() {
                            to_be_edited = Some(row_idx);
                        } // Use fontdrop.info to find the correct glifs

                        let mut short_name = instan.prof_name.clone();
                        if short_name.len()>=11 {
                            short_name.truncate(8);
                            short_name.push_str("...");
                        }
                        ui.button(short_name).on_hover_text(&instan.prof_name);

                    });
                });
            });
        }

        if let Some(row_idx) = to_be_moved {
            let asd_removed = self.testing_displays[col_idx].profile_list.remove(row_idx);
            self.testing_displays[0].profile_list.push(asd_removed);
        }

        if let Some(row_idx) = to_be_edited {
            self.current_editing_profile = Some([col_idx, row_idx]);
        }
    }

    pub fn display_page_instances(&mut self, ui: &mut Ui) {
        ui.heading("Game instances");
        ui.separator();

        egui::TopBottomPanel::bottom(ui.next_auto_id())
            .resizable(false)
            .exact_height(100.0)
            .frame(egui::Frame::NONE)
            .show_inside(ui, |ui| {

            let dnd_frame: egui::Frame = egui::Frame::group(ui.style())
                    .inner_margin(0);

            dnd_frame.show(ui, |ui| {
                ui.set_height(ui.available_height());
                ui.set_width(ui.available_width());

                ui.label("Unused profiles");
                egui::ScrollArea::vertical().show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    
                    let visual_data = ui.visuals_mut();
                    let old_visual_data = visual_data.widgets.clone(); // Reset dnd zone disabling colors may not be nessisary.
                    visual_data.widgets.inactive.bg_fill = egui::Color32::TRANSPARENT; // DND zone disable colors
                    visual_data.widgets.hovered.bg_fill  = egui::Color32::TRANSPARENT; // DND zone disable colors
                    visual_data.widgets.active.bg_fill   = egui::Color32::TRANSPARENT; // DND zone disable colors
                    let (_, dropped_payload) = ui.dnd_drop_zone::<super::app::DisplayProfile, ()>(dnd_frame, |ui| {
                        ui.visuals_mut().widgets = old_visual_data; // Reset dnd zone disabling colors may not be nessisary.
                        ui.set_width(ui.available_width());
                        ui.set_min_height(ui.available_height());


                        ui.vertical(|ui: &mut Ui| {
                            self.instances_display_collumn(ui, 0);
                        });
                    });

                    if let Some(payload_drop) = dropped_payload {
                        let item = payload_drop;
                        let profile_removed = self.testing_displays[item.display_idx].profile_list.remove(item.profile_display_idx);
                        self.testing_displays[0].profile_list.push(profile_removed);
                    }

                });
            })
        });


        ui.horizontal_top(|ui| {
            egui::ScrollArea::vertical().show(ui, |ui| {
                ui.vertical(|ui| {
                
                    ui.set_height(ui.available_height());
                    ui.set_width(ui.available_width());

                    let total_width_row = (ui.available_width()/(150.0+5.0)).max(1.0) as usize;
                    let mut row_current_count = 0 as usize;

                    egui::Grid::new(ui.next_auto_id())
                        .spacing(egui::Vec2::new(5.0,5.0))
                        .show(ui, |ui| {
                        for (col_idx, display_column) in self.testing_displays.clone().into_iter().enumerate() {
                            if col_idx == 0 { // Ignore the unused bottom section's ones - always index 0.
                                continue;
                            }

                            if row_current_count>=total_width_row {
                                row_current_count = 0;
                                ui.end_row();
                            }
                            row_current_count+=1;

                            let dnd_frame: egui::Frame = egui::Frame::group(ui.style())
                                .inner_margin(0);

                            dnd_frame.show(ui, |ui| {
                                let visual_data = ui.visuals_mut();
                                let old_visual_data = visual_data.widgets.clone(); // Reset dnd zone disabling colors may not be nessisary.
                                visual_data.widgets.inactive.bg_fill = egui::Color32::TRANSPARENT; // DND zone disable colors
                                visual_data.widgets.hovered.bg_fill  = egui::Color32::TRANSPARENT; // DND zone disable colors
                                visual_data.widgets.active.bg_fill   = egui::Color32::TRANSPARENT; // DND zone disable colors
                                
                                ui.set_min_height(100.0);
                                ui.set_width(150.0);
                                let (_, dropped_payload) = ui.dnd_drop_zone::<super::app::DisplayProfile, ()>(dnd_frame, |ui| {
                                    ui.visuals_mut().widgets = old_visual_data; // Reset dnd zone disabling colors may not be nessisary.
                                    ui.set_min_height(ui.available_height());
                                    ui.set_width(ui.available_width());


                                    ui.vertical(|ui| {
                                        ui.horizontal(|ui| {
                                            ui.style_mut().spacing.item_spacing.x = 3.0;

                                            if ui.button("✏").on_hover_text("Edit").clicked() {
                                                self.current_editing_display = col_idx;
                                            }

                                            let mut short_name = display_column.display_name.clone();
                                            if short_name.len()>=20 {
                                                short_name.truncate(17);
                                                short_name.push_str("...");
                                            }
                                            ui.label(short_name);
                                        });

                                        
                                        self.instances_display_collumn(ui ,col_idx);
                                    });
                                });

                                if let Some(payload_drop) = dropped_payload {
                                    let item = payload_drop;
                                    let profile_removed = self.testing_displays[item.display_idx].profile_list.remove(item.profile_display_idx);
                                    self.testing_displays[col_idx].profile_list.push(profile_removed);

                                }
                            });
                        }

                        if row_current_count>=total_width_row {
                            ui.end_row();
                        }
                        
                        ui.scope(|ui| {
                            ui.style_mut().override_text_style = Some(egui::TextStyle::Heading);

                            let add_button = egui::Button::new("➕")
                                .min_size(egui::Vec2 { x: 150.0, y: 100.0 });
                            if ui.add(add_button).clicked() {
                                println!("ADD NEW DISPLAY");
                                self.current_editing_display = self.testing_displays.len();
                                self.testing_displays.push(Display {
                                    display_name: format!("Display - {}", self.testing_displays.len()).clone(),
                                    profile_list: Vec::new(),
                                    comp_type: crate::app::app::DisplayCompType::Native,
                                    kde_split_type: DisplayCompTypeKwinSplit::None,
                                });
                            }
                        });

                    });

                    ui.add_space(5.0);
                
                });
            });
        });


        


        // if loop {
        //     if let Some([edit_col_idx, edit_row_idx]) = self.current_editing_profile {
        //         if edit_col_idx>self.testing_displays.len() {break true;}
        //         let edit_display = &mut self.testing_displays[edit_col_idx];

        //         if edit_row_idx>edit_display.profile_list.len() {break true;}
        //         let profile = &mut edit_display.profile_list[edit_row_idx];

        //         todo!("{:?}",profile); // TODO, process the profile for actual modification
        //     }
            
        //     break false;
        // } {
        //     self.current_editing_profile = None;
        // }

        self.display_page_instanes_edit_displays(ui);
        self.display_page_instances_edit_instance(ui);

    }

    pub fn display_page_instanes_edit_displays(&mut self, ui: &mut Ui) {
        if self.current_editing_display == 0 {
            return;
        }

        egui::Modal::new(ui.next_auto_id()).show(ui.ctx(), |ui| {
            let mut delete_next = false;
            let mut close_next = false;

            // It's okay to borrow display mutably here, as long as we don't remove from self.testing_displays
            let display_index = self.current_editing_display;
            let display = &mut self.testing_displays[display_index];

            ui.horizontal(|ui| {
                ui.scope(|ui| {
                    ui.style_mut().visuals.widgets.inactive.weak_bg_fill = egui::Color32::DARK_RED;
                    ui.style_mut().visuals.widgets.active.weak_bg_fill = egui::Color32::DARK_RED;
                    ui.style_mut().visuals.widgets.hovered.weak_bg_fill = egui::Color32::DARK_RED;
                    if ui.button("🗑").on_hover_text("Remove").clicked() {
                        // Release mutable borrow by handling move outside this closure.
                        delete_next = true;
                    }
                });
                ui.heading("Modify display");
            });

            ui.horizontal(|ui| {
                ui.label("Display name");
                ui.text_edit_singleline(&mut display.display_name);
            });

            let comp_selected_text = match &display.comp_type {
                DisplayCompType::Native => "Native".to_string(),
                DisplayCompType::None => "None (hidden)".to_string(),
                DisplayCompType::Nested(nested) => match nested.as_str() {
                    "river" => "(nested) River".to_string(),
                    "kwin" => "(nested) Kwin".to_string(),
                    other => format!("(nested) {other}"),
                },
                DisplayCompType::KDE => "KDE".to_string(),
            };

            egui::ComboBox::from_label("Window type")
                .selected_text(comp_selected_text)
                .show_ui(ui, |ui| {
                    if ui.selectable_label(display.comp_type == DisplayCompType::Native, "Native").clicked() {
                        display.comp_type = DisplayCompType::Native;
                    }
                    if ui.selectable_label(display.comp_type == DisplayCompType::None, "None (hidden)").clicked() {
                        display.comp_type = DisplayCompType::None;
                    }
                    if ui.selectable_label(display.comp_type == DisplayCompType::Nested("kwin".to_string()), "(nested) Kwin").clicked() {
                        display.comp_type = DisplayCompType::Nested("kwin".to_string());
                    }
                    if ui.selectable_label(display.comp_type == DisplayCompType::Nested("river".to_string()), "(nested) River").clicked() {
                        display.comp_type = DisplayCompType::Nested("river".to_string());
                    }
                    if ui.selectable_label(display.comp_type == DisplayCompType::KDE, "KDE").clicked() {
                        display.comp_type = DisplayCompType::KDE;
                    }
                });

            if display.comp_type == DisplayCompType::KDE || display.comp_type == DisplayCompType::Nested("kwin".to_string()) {
                let kde_split_type_text = match display.kde_split_type {
                    DisplayCompTypeKwinSplit::None => "None",
                    DisplayCompTypeKwinSplit::Vertical => "Vertical",
                    DisplayCompTypeKwinSplit::Horizontal => "Horizontal",
                };
                egui::ComboBox::from_label("Kde split style")
                    .selected_text(kde_split_type_text)
                    .show_ui(ui, |ui| {
                        if ui.selectable_label(display.kde_split_type == DisplayCompTypeKwinSplit::None, "None").clicked() {
                            display.kde_split_type = DisplayCompTypeKwinSplit::None;
                        }
                        if ui.selectable_label(display.kde_split_type == DisplayCompTypeKwinSplit::Vertical, "Vertical").clicked() {
                            display.kde_split_type = DisplayCompTypeKwinSplit::Vertical;
                        }
                        if ui.selectable_label(display.kde_split_type == DisplayCompTypeKwinSplit::Horizontal, "Horizontal").clicked() {
                            display.kde_split_type = DisplayCompTypeKwinSplit::Horizontal;
                        }
                    });
            }

            ui.vertical_centered(|ui| {
                if ui.button("Close").clicked() {
                    close_next = true;
                }
            });
            
            if delete_next {
                let moving_profiles = self.testing_displays[display_index].profile_list.drain(..).collect::<Vec<_>>();
                self.testing_displays[0].profile_list.extend(moving_profiles);
                self.testing_displays.remove(display_index);
                close_next = true;
            }
            if close_next {
                self.current_editing_display = 0;
            }
        });
    }

    pub fn display_page_instances_edit_instance(&mut self, ui: &mut Ui) {
        let Some([col_idx, row_idx]) = self.current_editing_profile else { return; };

        let Some(profile) = self.testing_displays
            .get_mut(col_idx)
            .and_then(|d| d.profile_list.get_mut(row_idx)) 
        else {
            self.current_editing_profile = None;
            return;
        };


        egui::Modal::new(ui.next_auto_id()).show(ui.ctx(), |ui| {
            let mut close_next = false;

            ui.heading("Modify profile");

            ui.horizontal(|ui| {
                ui.label("profile name");
                ui.text_edit_singleline(&mut profile.prof_name);
            });


            ui.label("Devices to use:");
            for cur_device_idx in 0..self.input_devices.len() {
                let cur_device = &self.input_devices[cur_device_idx];

                let cur_device_selected = profile.inputs.contains(&cur_device_idx);
                let mut cur_device_selected_new = cur_device_selected;
                ui.checkbox(&mut cur_device_selected_new, cur_device.name());

                if cur_device_selected_new != cur_device_selected {
                    if cur_device_selected_new {
                        profile.inputs.push(cur_device_idx);
                    } else {
                        profile.inputs.retain(|&val| val != cur_device_idx);
                    }
                }
            }

            // let comp_selected_text = match &display.comp_type {
            //     DisplayCompType::Native => "Native".to_string(),
            //     DisplayCompType::None => "None (hidden)".to_string(),
            //     DisplayCompType::Nested(nested) => match nested.as_str() {
            //         "river" => "(nested) River".to_string(),
            //         "kwin" => "(nested) Kwin".to_string(),
            //         other => format!("(nested) {other}"),
            //     },
            //     DisplayCompType::KDE => "KDE".to_string(),
            // };

            // egui::ComboBox::from_label("Window type")
            //     .selected_text(comp_selected_text)
            //     .show_ui(ui, |ui| {
            //         if ui.selectable_label(display.comp_type == DisplayCompType::Native, "Native").clicked() {
            //             display.comp_type = DisplayCompType::Native;
            //         }
            //         if ui.selectable_label(display.comp_type == DisplayCompType::None, "None (hidden)").clicked() {
            //             display.comp_type = DisplayCompType::None;
            //         }
            //         if ui.selectable_label(display.comp_type == DisplayCompType::Nested("kwin".to_string()), "(nested) Kwin").clicked() {
            //             display.comp_type = DisplayCompType::Nested("kwin".to_string());
            //         }
            //         if ui.selectable_label(display.comp_type == DisplayCompType::Nested("river".to_string()), "(nested) River").clicked() {
            //             display.comp_type = DisplayCompType::Nested("river".to_string());
            //         }
            //         if ui.selectable_label(display.comp_type == DisplayCompType::KDE, "KDE").clicked() {
            //             display.comp_type = DisplayCompType::KDE;
            //         }
            //     });

            // if display.comp_type == DisplayCompType::KDE {
            //     let kde_split_type_text = match display.kde_split_type {
            //         DisplayCompTypeKwinSplit::None => "None",
            //         DisplayCompTypeKwinSplit::Vertical => "Vertical",
            //         DisplayCompTypeKwinSplit::Horizontal => "Horizontal",
            //     };
            //     egui::ComboBox::from_label("Kde split style")
            //         .selected_text(kde_split_type_text)
            //         .show_ui(ui, |ui| {
            //             if ui.selectable_label(display.kde_split_type == DisplayCompTypeKwinSplit::None, "None").clicked() {
            //                 display.kde_split_type = DisplayCompTypeKwinSplit::None;
            //             }
            //             if ui.selectable_label(display.kde_split_type == DisplayCompTypeKwinSplit::Vertical, "Vertical").clicked() {
            //                 display.kde_split_type = DisplayCompTypeKwinSplit::Vertical;
            //             }
            //             if ui.selectable_label(display.kde_split_type == DisplayCompTypeKwinSplit::Horizontal, "Horizontal").clicked() {
            //                 display.kde_split_type = DisplayCompTypeKwinSplit::Horizontal;
            //             }
            //         });
            // }

            ui.vertical_centered(|ui| {
                if ui.button("Close").clicked() {
                    close_next = true;
                }
            });

            if close_next {
                self.current_editing_profile = None;
            }
        });
    }

    pub fn display_settings_general(&mut self, ui: &mut Ui) {
        let check_for_app_updates = ui.checkbox(&mut self.options.check_for_updates, "Check for partydeck updates");
        if check_for_app_updates.hovered() {
            self.infotext = "DEFAULT: Enabled\n\nWARNING: CONTACTS GITHUB's SERVERS ON EVERY LAUNCH\nMakes partydeck check online for updates durring each launch, and notfies user when avaliable.".to_string();
        }

        let enable_kwin_script_check = ui.checkbox(
            &mut self.options.enable_kwin_script,
            "(KDE) Automatically resize/reposition instances using KWin script",
        );
        if enable_kwin_script_check.hovered() {
            self.infotext = "DEFAULT: Enabled\n\n Resizes/repositions instances to fit the screen using a KWin script. If using a desktop environment or window manager other than KDE Plasma, uncheck this; note that you will need to manually resize and reposition the windows.".to_string();
        }

        ui.horizontal(|ui| {
            let split_style_label = ui.label("Split style");
            let r1 = ui.radio_value(
                &mut self.options.vertical_two_player,
                false,
                "Horizontal",
            );
            let r2 = ui.radio_value(
                &mut self.options.vertical_two_player,
                true,
                "Vertical",
            );
            if split_style_label.hovered() || r1.hovered() || r2.hovered() {
                self.infotext =
                    "DEFAULT: Horizontal\n\nChoose whether to split two-player games horizontally (above/below) instead of vertically (side by side).".to_string();
            }
        });

        let mut comp_selected_text = "None".to_owned();
        if let Some(comp_opt) = &self.options.nested_compositor {
            comp_selected_text = match comp_opt.as_str() {
                "river" => "River".to_owned(),
                "kwin_wayland" => "Kwin".to_owned(),
                _=>comp_selected_text,
            }
        }

        egui::ComboBox::from_label("Used nested compositor")
            .selected_text(comp_selected_text)
            .show_ui(ui, |ui| {
                if ui.selectable_label(self.options.nested_compositor == Some("kwin_wayland".to_owned()), "Kwin").clicked() {
                    self.options.nested_compositor = Some("kwin_wayland".to_owned());
                }
                if ui.selectable_label(self.options.nested_compositor == Some("river".to_owned()), "River").clicked() {
                    self.options.nested_compositor = Some("river".to_owned());
                }
                if ui.selectable_label(self.options.nested_compositor == None, "None").clicked() {
                    self.options.nested_compositor = None;
                }
            });

        ui.horizontal(|ui| {
            let filter_label = ui.label("Controller filter");
            let r1 = ui.radio_value(
                &mut self.options.pad_filter_type,
                PadFilterType::All,
                "All controllers",
            );
            let r2 = ui.radio_value(
                &mut self.options.pad_filter_type,
                PadFilterType::NoSteamInput,
                "No Steam Input",
            );
            let r3 = ui.radio_value(
                &mut self.options.pad_filter_type,
                PadFilterType::OnlySteamInput,
                "Only Steam Input",
            );

            if filter_label.hovered() || r1.hovered() || r2.hovered() || r3.hovered() {
                self.infotext = "DEFAULT: No Steam Input\n\nSelect which controllers to filter out. If you use Steam Input to remap controllers, you may want to select \"Only Steam Input\", but be warned that this option is experimental and is known to break certain Proton games.".to_string();
            }

            if r1.clicked() || r2.clicked() || r3.clicked() {
                self.input_devices = scan_input_devices(&self.options.pad_filter_type);
            }
        });

        let profile_unique_dirs_check = ui.checkbox(
            &mut self.options.profile_unique_dirs,
            "Unique per-profile environments",
        );
        if profile_unique_dirs_check.hovered() {
        self.infotext = "DEFAULT: Enabled\n\nGives each profile their own data directories. For Windows games, this is the C:\\Users\\steamuser folder, for Linux native games this is the HOME directory. Note that disabling this means that PartyDeck instances may potentially modify your game's actual save data on disk.".to_string();
        }

        let allow_multiple_instances_on_same_device_check = ui.checkbox(
            &mut self.options.allow_multiple_instances_on_same_device,
            "(Debug) Allow multiple instances from one gamepad",
        );
        if allow_multiple_instances_on_same_device_check.hovered() {
            self.infotext = "DEFAULT: Disabled\n\nAllow multiple instances on the same device. This can be useful for testing or when one person wants to control multiple instances.".to_string();
        }

        let disable_mount_gamedirs_check = ui.checkbox(
            &mut self.options.disable_mount_gamedirs,
            "(Debug) Force run instances from original game directory",
        );
        if disable_mount_gamedirs_check.hovered() {
            self.infotext = "DEFAULT: Disabled\n\nBy default, PartyDeck mounts game directories using fuse-overlayfs to let each instance write to the game's directory without conflicting with each other or affecting the game's installation. In addition, this lets handlers overlay content like mods or config files onto the game directory. Enabling this forces instances to launch from the original game directory without mounting, which will prevent handlers from using built-in mods, but may be useful for diagnosing issues.".to_string();
        }

        ui.separator();

        if ui.button("Open PartyDeck Data Folder").clicked() {
            if let Err(_) = std::process::Command::new("xdg-open")
                .arg(PATH_PARTY.clone())
                .status()
            {
                msg("Error", "Couldn't open PartyDeck Data Folder!");
            }
        }
    }

    pub fn display_settings_proton(&mut self, ui: &mut Ui) {
        ui.horizontal(|ui| {
        let proton_ver_label = ui.label("Proton version");
        let proton_ver_editbox = ui.add(
            egui::TextEdit::singleline(&mut self.options.proton_version)
                .hint_text("GE-Proton"),
        );
        if proton_ver_label.hovered() || proton_ver_editbox.hovered() {
            self.infotext = "DEFAULT: GE-Proton\n\nSpecify a Proton version. This can be a path, e.g. \"/path/to/proton\" or just a name, e.g. \"GE-Proton\" for the latest version of Proton-GE. If left blank, this will default to \"GE-Proton\". If unsure, leave this blank.".to_string();
        }
        });

        let proton_separate_pfxs_check = ui.checkbox(
            &mut self.options.proton_separate_pfxs,
            "Run instances in separate Proton prefixes",
        );
        if proton_separate_pfxs_check.hovered() {
            self.infotext = "DEFAULT: Enabled\n\nRuns each instance in separate Proton prefixes. If unsure, leave this checked. Multiple prefixes takes up more disk space, but generally provides better compatibility and fewer issues with Proton-based games.".to_string();
        }

        let proton_wow64_check = ui.checkbox(
            &mut self.options.proton_wow64,
            "Run Proton in WoW64 mode",
        );
        if proton_wow64_check.hovered() {
            self.infotext = "DEFAULT: Enabled\n\nRuns Proton games in the new Wine WoW64 mode. If unsure, leave this checked.".to_string();
        }

        if ui.button("Erase All Proton Prefix Data").clicked() {
            if yesno(
                "Erase Prefix?",
                "This will erase all Proton prefixes used by PartyDeck. This shouldn't erase profile/game-specific data, but exercise caution. Are you sure?",
            ) && PATH_PARTY.join("prefixes").exists()
            {
                if let Err(err) = std::fs::remove_dir_all(PATH_PARTY.join("prefixes")) {
                    msg("Error", &format!("Couldn't erase pfx data: {}", err));
                } else {
                    msg("Data Erased", "Proton prefix data successfully erased.");
                }
            }
        }
    }

    pub fn display_settings_gamescope(&mut self, ui: &mut Ui) {
        let gamescope_lowres_fix_check = ui.checkbox(
            &mut self.options.gamescope_fix_lowres,
            "Automatically fix low resolution instances",
        );
        let gamescope_sdl_backend_check =
            ui.checkbox(&mut self.options.gamescope_sdl_backend, "Use SDL backend");
        let kbm_support_check = ui.checkbox(
            &mut self.options.kbm_support,
            "Enable keyboard and mouse support through custom Gamescope",
        );
        let resize_support = ui.checkbox(
            &mut self.options.gamescope_resize_support,
            "Enable gamescope resize support",
        );
        let gamescope_force_fullscreen = ui.checkbox(
            &mut self.options.gamescope_force_fullscreen,
            "Force gamescope to fullscreen games",
        );
        let gamescope_force_grab_cursor_check = ui.checkbox(
            &mut self.options.gamescope_force_grab_cursor,
            "Force grab cursor for Gamescope",
        );

        if gamescope_lowres_fix_check.hovered() {
            self.infotext = "Many games have graphical problems or even crash when running at resolutions below 600p. If this is enabled, any instances below 600p will automatically be resized before launching.".to_string();
        }
        if gamescope_sdl_backend_check.hovered() {
            self.infotext = "Runs gamescope sessions using the SDL backend. This is required for multi-monitor support. If unsure, leave this checked. If gamescope sessions only show a black screen or give an error (especially on Nvidia + Wayland), try disabling this.".to_string();
        }
        if resize_support.hovered() {
            self.infotext = "Runs a custom Gamescope build with support for resizing dynamicly (use with river nested compositor).".to_string();
        }
        if kbm_support_check.hovered() {
            self.infotext = "Runs a custom Gamescope build with support for holding keyboards and mice. If you want to use your own Gamescope installation, uncheck this.".to_string();
        }
        if gamescope_force_fullscreen.hovered() {
            self.infotext = "Sets --force-fullscreen-windows on gamescope in order to properly have games size.".to_string();
        }
        if gamescope_force_grab_cursor_check.hovered() {
            self.infotext = "Sets the \"--force-grab-cursor\" flag in Gamescope. This keeps the cursor within the Gamescope window. If unsure, leave this unchecked.".to_string();
        }
    }
}
