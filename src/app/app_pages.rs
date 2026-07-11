use super::app::{MenuPage, PartyApp, SettingsPage};
use super::config::*;
use crate::instance::{Instance, LaunchCompositors, LaunchDisplay};
use crate::layout_manager::{LayoutType, LayoutWindows};
use crate::{handler::*, input, layout_manager};
use crate::input::*;
use crate::monitor::get_monitors_errorless;
use crate::paths::*;
use crate::profiles::*;
use crate::util::*;

use dialog::DialogBox;
use eframe::egui::RichText;
use eframe::egui::{self, Ui};
use eframe::glow::MAX_HEIGHT;
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
            ui.hyperlink_to(
                "@davidawesome02",
                "https://github.com/davidawesome02-backup",
            );
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
            .show(ui, |ui| match self.settings_page {
                SettingsPage::General => self.display_settings_general(ui),
                SettingsPage::Proton => self.display_settings_proton(ui),
                SettingsPage::Gamescope => self.display_settings_gamescope(ui),
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
            ui.checkbox(&mut h.use_mangohud, "Enable MangoHud");
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
                    // TODO: REPLACE

                    // self.instances.clear();
                    self.input_devices = scan_input_devices(&self.options.pad_filter_type);
                    self.sys_monitors = get_monitors_errorless();
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
                let delta = pointer_pos - response.rect.left_top() - egui::vec2(12.0, 12.0); // Manual correction factor
                self_u.ctx()
                    .transform_layer_shapes(layer_id, egui::emath::TSTransform::from_translation(delta));
            }

            egui::InnerResponse::new(inner, response)
        } else {
            self_u.scope(add_contents)
        }
    }

    pub fn display_page_instances(&mut self, ui: &mut Ui) {

        ui.horizontal(|ui| {
            ui.heading("Instances");
            
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let launch_enabled = self.launch_displays.iter().any(|disp| disp.instances.len()>0);

                if (
                    ui.add_enabled(
                        launch_enabled, egui::Button::new("Launch")
                    ).on_disabled_hover_text("Please add instances")
                ).clicked() {
                    println!("Launch not impl");

                    // TODO finish
                    self.prepare_game_launch();
                }
            });
        });


        ui.separator();
        
      

        egui::containers::Panel::bottom(ui.next_auto_id())
            .resizable(false)
            .exact_size(160.0)
            .show_separator_line(false)
            .frame(egui::Frame::NONE)
            .show(ui, |ui| {
                ui.add_space(3.0); // Hack to make it actualy centered (calculated using the 6px separator line default height)
                ui.separator();
                
                egui::ScrollArea::vertical()
                .max_height(ui.available_height()) // Remove lower menue height from avaliable
                .auto_shrink(false)
                .show(ui, |ui| {

                    ui.label(format!("Display {}/{}", self.launch_display_idx+1, self.launch_displays.len()));


                    ui.separator();

                    let mut current_used_profiles_for_others =
                            self.launch_displays.iter().flat_map(|check_display| {
                                check_display.instances.iter().map(move |check_instance| {
                                    check_instance.profname.clone()
                                })
                            }).collect::<std::collections::HashSet<_>>().into_iter().collect::<Vec<_>>();
                    current_used_profiles_for_others.sort();

                    let current_display = &mut self.launch_displays[self.launch_display_idx];

                    if ui.button("New instance").clicked() {

                        let new_prof_name = match fastrand::choice(
                                GUEST_NAMES.iter().filter_map(|guest_name_check| {
                                    let on_disk_name = format!(".{guest_name_check}");

                                    if current_used_profiles_for_others.contains(&on_disk_name) {None} else {Some(on_disk_name)}                                
                                }).collect::<Vec<String>>()
                        ) {
                            Some(a) => a.to_owned(),
                            None => format!(".Auto profile - {}", fastrand::u32(10000..99999)),
                        };

                        current_display.instances.push(
                            Instance {
                                devices: vec![],
                                profname: new_prof_name,
                                color: crate::util::random_new_inst_color(&current_display.instances),
                            }
                        );

                        self.model_temp_modify_profile = Some((self.launch_display_idx, current_display.instances.len()-1));
                    }


                    let current_compositor = &mut current_display.nested_compositor;
                    ui.horizontal(|ui| {
                        ui.label("Compositor");
                        egui::containers::ComboBox::new("CompositorComboBox", "")
                            .selected_text(current_compositor.display_name())
                            .show_ui(ui, |ui| {
                                ui.selectable_value(current_compositor, LaunchCompositors::Native,  LaunchCompositors::Native.display_name());
                                ui.selectable_value(current_compositor, LaunchCompositors::Kwin,    LaunchCompositors::Kwin.display_name()  );
                                ui.selectable_value(current_compositor, LaunchCompositors::River,   LaunchCompositors::River.display_name() );
                            });
                    });


                    let layout_name = current_display.layout.get_type();
                    let mut layout_new_name = current_display.layout.get_type();

                    ui.horizontal(|ui| {
                        ui.label("Layout type:");
                        egui::containers::ComboBox::new("LayoutTypeComboBox", "")
                            .selected_text(format!("{}",layout_new_name))
                            .show_ui(ui, |ui| {
                                ui.selectable_value(&mut layout_new_name, LayoutType::GameLayout, LayoutType::GameLayout.to_string());
                                ui.selectable_value(&mut layout_new_name, LayoutType::FlatLayout, LayoutType::FlatLayout.to_string());
                            });
                    });


                    if layout_new_name != layout_name  {
                        current_display.layout = match layout_new_name {
                            LayoutType::GameLayout => {
                                Box::new(layout_manager::GameLayout {
                                    reverse_direction: false,
                                    ideal_game_width: 16.0,
                                    ideal_game_height: 9.0,
                                })
                            },
                            LayoutType::FlatLayout => {
                                Box::new(layout_manager::FlatLayout {
                                    split_dir_width: false,
                                })
                            },
                        };
                    }
                    

                    current_display.layout.display_editor(ui);
                });
                
            }
        );

        egui::containers::Panel::left(ui.next_auto_id())
            .resizable(false)
            .show_separator_line(false)
            .exact_size(35.0)
            .frame(egui::Frame::NONE)
            .show(ui, |ui| {
                ui.centered_and_justified(|ui| {
                    ui.set_height(25.0);
                    ui.set_width(25.0);
                    let is_first_page = self.launch_display_idx == 0;
                    if ui.button(if is_first_page {"\u{01F6AB}"} else {"⬅"}).clicked() && !is_first_page {
                        self.launch_display_idx-=1;
                    }
                })
            });
        
        // egui::containers::Panel::right(id)
        egui::containers::Panel::right(ui.next_auto_id())
            .resizable(false)
            .show_separator_line(false)
            .frame(egui::Frame::NONE)
            .exact_size(35.0)
            .show(ui, |ui| {

                ui.centered_and_justified(|ui| {
                    ui.set_height(25.0);
                    ui.set_width(25.0);
                    let is_last_display = self.launch_display_idx+1 == self.launch_displays.len();
                    let should_allow_create_new_instance = !self.launch_displays[self.launch_display_idx].instances.is_empty();
                    if ui.button(if is_last_display { if should_allow_create_new_instance {"✚"} else {"\u{01F6AB}"}} else {"➡"}).clicked() {
                        if is_last_display {
                            if should_allow_create_new_instance {
                                self.launch_displays.push(
                                    LaunchDisplay {
                                        layout: Box::new(layout_manager::GameLayout {
                                            reverse_direction: false,
                                            ideal_game_width: 16.0,
                                            ideal_game_height: 9.0,
                                        }),
                                        instances: vec![],
                                        nested_compositor: LaunchCompositors::Kwin,
                                        display_index: 0,
                                        move_handle_sel_idx: None,
                                    }
                                );
                                self.launch_display_idx+=1;
                            }
                        } else {
                            self.launch_display_idx+=1;
                        }
                    }
                })
            });

        ui.centered_and_justified(|ui| {
        egui::Frame::NONE
        .fill(egui::Color32::from_gray(80))
        .inner_margin(2.0)
        .corner_radius(2)
        .show(ui, |ui| {

            // Todo not hardcode.
            let target_res = (self.sys_monitors[0].width(), self.sys_monitors[0].height());
            // let target_res = (1920, 1080);
            
            let aspect_ratio = (target_res.0 as f32)/(target_res.1 as f32);
            let height = (ui.available_width()/aspect_ratio).min(ui.available_height() as f32);
            let width = height*aspect_ratio; 
        
            ui.set_height(height);
            ui.set_width(width);

            let current_display = &self.launch_displays[self.launch_display_idx];


            if current_display.instances.len() == 0 {
                ui.vertical_centered(|ui| {
                    // Todo remove this random spacing. 
                    ui.add_space(ui.available_height()/2.0-15.0);
                    ui.label("No instances, click \"New instance\" to add.");

                    if self.launch_displays.len() > 1 {
                       if ui.button("Remove display").clicked() {
                            self.launch_displays.remove(self.launch_display_idx);
                            
                            self.launch_display_idx = self.launch_display_idx.saturating_sub(1);
                        }
                    }
                });
                
                return;
            }


            let top_left_cursor = ui.cursor().left_top().to_vec2();

            let window_laid_out = current_display.layout.layout(
                current_display.instances.len() as u32, 
                target_res.0, 
                target_res.1
            );

            let mut dropped_swapped_loc: Option<(usize, usize)> = None;
            let mut to_be_edited_instance: Option<usize> = None;
            let mut to_be_removed_instance: Option<usize> = None;

            let mut new_display_sel_handle: Option<usize> = None;



            for idx_instance in 0..window_laid_out.len() {
                let wind_pos = window_laid_out.get(idx_instance).unwrap();

                let current_display = &self.launch_displays[self.launch_display_idx];
                let display_sel_handle = current_display.move_handle_sel_idx.clone();

                let instance = current_display.instances.get(idx_instance).unwrap();
                let instance_profname = instance.profname.clone();
                let instance_color = instance.color.clone();

                let drop_rect = egui::Rect::from_min_size(
                    egui::pos2(
                        (wind_pos.x as f32) * width / (target_res.0 as f32),
                        (wind_pos.y as f32) * height / (target_res.1 as f32),
                    )+top_left_cursor,
                    egui::Vec2::new(
                        (wind_pos.w as f32) * width / (target_res.0 as f32),
                        (wind_pos.h as f32) * height / (target_res.1 as f32)
                    ),
                );


                let _ = ui.scope_builder(
                    egui::UiBuilder::new()
                        .max_rect(drop_rect)
                        .sense(egui::Sense::hover())
                        .layout(egui::Layout::top_down(egui::Align::LEFT)),
                    |ui| {
                        ui.set_width(drop_rect.width());
                        ui.set_height(drop_rect.height());
                        

                        let frame = egui::Frame::default()
                            .corner_radius(2)
                            .stroke(egui::Stroke::new(2.0, egui::Color32::GRAY));
                        
                        let dropped_payload = ui.dnd_drop_zone::<i32, ()>(frame, |ui| {

                            // Should be unique, when adding many displays, add the display to the salt here.
                            let dnd_id: eframe::egui::Id = ui.make_persistent_id(format!("dnd_instance-{idx_instance}"));
                            
                            self.dnd_drag_source_cust(ui, dnd_id, idx_instance as i32, |ui| {
                            egui::Frame::NONE
                                .fill(egui::Color32::from_gray(20))
                                .corner_radius(2)
                                .stroke(egui::Stroke::new(2.0, instance_color))
                                .show(ui, |ui| {
                                    ui.set_width(ui.available_size().x);
                                    ui.set_height(ui.available_size().y);
                                    ui.horizontal(|ui| {
                                        
                                        ui.style_mut().spacing.item_spacing.x = 3.0;


                                        ui.scope(|ui| {
                                            ui.style_mut().visuals.widgets.active.weak_bg_fill = egui::Color32::LIGHT_GREEN;
                                            if display_sel_handle == Some(idx_instance) {
                                                ui.style_mut().visuals.widgets.active.weak_bg_fill = egui::Color32::LIGHT_RED;
                                                ui.style_mut().visuals.widgets.inactive.weak_bg_fill = egui::Color32::DARK_GREEN;
                                            }
                                            let move_handle = ui.button("Ｓ").on_hover_text("Move handle");
                                            ui.interact(move_handle.rect, dnd_id, egui::Sense::drag()).on_hover_cursor(egui::CursorIcon::Grab);
                                            if move_handle.clicked() {
                                                new_display_sel_handle = Some(idx_instance);
                                            }
                                        });
                                        
                                        if ui.button("\u{01F5D1}").on_hover_text("Remove").clicked() {
                                            to_be_removed_instance = Some(idx_instance);
                                        }

                                        if ui.button("✏").on_hover_text("Edit").clicked() {
                                            to_be_edited_instance = Some(idx_instance);
                                        }
                                        
                                        ui.add(egui::Label::new(instance_profname).truncate());
                                    });
                                });
                            });
                            ui.set_width(ui.available_size().x);
                            ui.set_height(ui.available_size().y);
                            
                        });

                        if let Some(dropped_idx) = dropped_payload.1 {
                            dropped_swapped_loc = Some((*dropped_idx as usize, idx_instance));
                        }
                    },
                );
            };


            // WARNING below this line, we may not execute because the ordering of swaps or removals will interupt eachother.
            let current_display = &mut self.launch_displays[self.launch_display_idx];
            
            
            if let Some(remove_idx) = to_be_removed_instance {
                current_display.instances.remove(remove_idx);
                condense_display(&mut self.launch_displays, &mut self.launch_display_idx);
                return;
            }


            if let Some(new_moved_handle) = new_display_sel_handle {
                if let Some(old_moved_handle) = current_display.move_handle_sel_idx {
                    dropped_swapped_loc = Some((new_moved_handle, old_moved_handle));

                    current_display.move_handle_sel_idx = None;
                } else {
                    current_display.move_handle_sel_idx = new_display_sel_handle;
                }
            }

            if let Some(swap_locations) = dropped_swapped_loc {
                current_display.instances.swap(swap_locations.0, swap_locations.1);
            }

            if let Some(edit_instance_loc) = to_be_edited_instance {
                self.model_temp_modify_profile = Some((self.launch_display_idx, edit_instance_loc));
            }

        });
        });

        self.display_page_instanes_edit_displays(ui);
    }



    pub fn display_page_instanes_edit_displays(&mut self, ui: &mut Ui) {
        if self.model_temp_modify_profile.is_none() {return;}
        let prof_loc = self.model_temp_modify_profile.unwrap();


        let saved_profiles = self.profiles.clone();

        let mut current_used_profiles_for_others: Vec<_> =
            self.launch_displays.iter().enumerate().flat_map(|(display_idx, check_display)| {
                check_display.instances.iter().enumerate().filter_map(move |(instance_idx, check_instance)| {
                    if (display_idx, instance_idx) == prof_loc {
                        None
                    } else {
                        Some(check_instance.profname.clone())
                    }
                })
            }).collect::<std::collections::HashSet<_>>().into_iter().collect();
        current_used_profiles_for_others.sort();

        let unused_saved_profiles: Vec<_> = 
            saved_profiles.iter().filter(|x| !current_used_profiles_for_others.contains(x)).collect();


        if prof_loc.0 >= self.launch_displays.len() {self.model_temp_modify_profile = None; return;}
        let disp_editing = &mut self.launch_displays[prof_loc.0];
        
        if prof_loc.1 >= disp_editing.instances.len() {self.model_temp_modify_profile = None; return;}
        // let inst_editing = &mut disp_editing.instances[prof_loc.1];

        // Drop the value because we have to construct it later to avoid borrowing the whole time smh.
        drop(disp_editing);

        egui::Modal::new(ui.next_auto_id()).show(ui.ctx(), |ui| {
            if ui.button("Close").clicked() {
                self.model_temp_modify_profile = None;
                return;
            }


            ui.horizontal(|ui| {
                ui.label("Profile selection:");

                let disp_editing = &mut self.launch_displays[prof_loc.0];
                let inst_editing = &mut disp_editing.instances[prof_loc.1];


                let new_profname = &mut Some(inst_editing.profname.clone());

                egui::containers::ComboBox::new("ProfileSelectionComboBox", "")
                    .selected_text(inst_editing.profname.clone())
                    .show_ui(ui, |ui| {
                        ui.selectable_value(new_profname, None, "New temp");
                        ui.separator();
                        for profile in unused_saved_profiles {
                            ui.selectable_value(new_profname, Some(profile.clone()), profile);
                        }
                        ui.separator();
                        for profile in &current_used_profiles_for_others {
                            ui.selectable_value(new_profname, Some(profile.clone()), profile);
                        }
                    });

                inst_editing.profname = if let Some(accepted_new_profname) = new_profname {
                    accepted_new_profname.to_string()
                } else {
                    match fastrand::choice(
                            GUEST_NAMES.iter().filter_map(|guest_name_check| {
                                let on_disk_name = format!(".{guest_name_check}");

                                if current_used_profiles_for_others.contains(&on_disk_name) {None} else {Some(on_disk_name)}                                
                            }).collect::<Vec<String>>()
                    ) {
                        Some(a) => a.to_owned(),
                        None => format!(".Auto profile - {}", fastrand::u32(10000..99999)),
                    }
                }
            });

            ui.horizontal(|ui| {
                ui.label("Outline color:");

                let disp_editing = &mut self.launch_displays[prof_loc.0];
                let inst_editing = &mut disp_editing.instances[prof_loc.1];

                ui.color_edit_button_srgba(&mut inst_editing.color);
            });

            ui.separator();
            for input_dev in &self.input_devices {
                
                let already_used = self.launch_displays.iter().enumerate().any(|(display_idx, check_display)| {
                    check_display.instances.iter().enumerate().any(move |(instance_idx, check_instance)| {
                        if (display_idx, instance_idx) == prof_loc {return false;}
                        check_instance.devices.contains(&input_dev.hash())
                    })
                });

                let dev_text = RichText::new(format!(
                    "{} {} ({})",
                    input_dev.emoji(),
                    input_dev.fancyname(),
                    input_dev.path().trim_start_matches("/dev/input/event")
                ))
                .small()
                .color(
                    match (input_dev.enabled(), input_dev.has_button_held(), already_used) {
                        (false, _,    false ) => egui::Color32::RED,
                        (false, _,    true  ) => egui::Color32::LIGHT_RED,
                        
                        (true, false, false ) => egui::Color32::GRAY,
                        (true, true,  false ) => egui::Color32::WHITE,

                        (true, false, true  ) => egui::Color32::BLUE,
                        (true, true,  true  ) => egui::Color32::LIGHT_BLUE,
                    }
                );

                let generated_hover_text = match (input_dev.enabled(), input_dev.has_button_held(), already_used) {
                    (false, _,    false ) => "Disabled",
                    (false, _,    true  ) => "Disabled\nAlready used",
                    
                    (true, false, false ) => "Avaliable",
                    (true, true,  false ) => "Avaliable\nInput pressed",

                    (true, false, true  ) => "Already used",
                    (true, true,  true  ) => "Already used\nInput pressed",
                };



                let disp_editing = &mut self.launch_displays[prof_loc.0];
                let inst_editing = &mut disp_editing.instances[prof_loc.1];

                let instance_dev_prev_pos = inst_editing.devices.iter().position(|x| x==&input_dev.hash());
                let mut instance_dev_checked = instance_dev_prev_pos != None;


                ui.checkbox(&mut instance_dev_checked, dev_text).on_hover_text(generated_hover_text);


                match (instance_dev_checked, instance_dev_prev_pos) {
                    (true, None) => {
                        inst_editing.devices.push(input_dev.hash());
                    },
                    (false, Some(remove_idx)) => {
                        inst_editing.devices.swap_remove(remove_idx);
                    },
                    (_, _) => {},
                }

            }

        });
    }

    pub fn display_settings_general(&mut self, ui: &mut Ui) {
        let check_for_app_updates = ui.checkbox(
            &mut self.options.check_for_updates,
            "Check for partydeck updates",
        );
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
                _ => comp_selected_text,
            }
        }

        egui::ComboBox::from_label("Used nested compositor")
            .selected_text(comp_selected_text)
            .show_ui(ui, |ui| {
                if ui
                    .selectable_label(
                        self.options.nested_compositor == Some("kwin_wayland".to_owned()),
                        "Kwin",
                    )
                    .clicked()
                {
                    self.options.nested_compositor = Some("kwin_wayland".to_owned());
                }
                if ui
                    .selectable_label(
                        self.options.nested_compositor == Some("river".to_owned()),
                        "River",
                    )
                    .clicked()
                {
                    self.options.nested_compositor = Some("river".to_owned());
                }
                if ui
                    .selectable_label(self.options.nested_compositor == None, "None")
                    .clicked()
                {
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

        let proton_wow64_check =
            ui.checkbox(&mut self.options.proton_wow64, "Run Proton in WoW64 mode");
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
