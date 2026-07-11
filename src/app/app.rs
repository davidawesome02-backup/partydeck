use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::sleep;

use super::config::*;
// use crate::video::app_wrapper::CreationContext;
use crate::video::pipewire::PipewireInstance;
use crate::video::video::PipewireVideo;
use crate::handler::*;
use crate::input::*;
use crate::instance::*;
use crate::launch::*;
use crate::layout_manager;
use crate::monitor::Monitor;
use crate::profiles::*;
use crate::util::*;

use eframe::egui::{self, Key, Ui, ViewportId};
use crate::video::egl::EglApi;

#[derive(Eq, PartialEq)]
pub enum MenuPage {
    Home,
    Settings,
    Profiles,
    EditHandler,
    Game,
    Instances,
}

#[derive(Eq, PartialEq)]
pub enum SettingsPage {
    General,
    Proton,
    Gamescope,
}

pub struct PartyApp {
    pub installed_steamapps: Vec<Option<steamlocate::App>>,
    pub needs_update: Arc<AtomicBool>,
    pub options: PartyConfig,
    pub cur_page: MenuPage,
    pub settings_page: SettingsPage,
    pub infotext: String,

    pub sys_monitors: Vec<Monitor>,
    pub input_devices: Vec<InputDevice>,
    pub profiles: Vec<String>,

    pub instance_add_dev: Option<usize>,
    
    pub launch_displays: Vec<LaunchDisplay>,
    pub launch_display_idx: usize,

    pub model_temp_modify_profile: Option<(usize, usize)>,

    pub handlers: Vec<Handler>,
    pub selected_handler: usize,
    pub handler_edit: Option<Handler>,
    pub handler_lite: Option<Handler>,

    pub loading_msg: Option<String>,
    pub loading_since: Option<std::time::Instant>,
    #[allow(dead_code)]
    pub task: Option<std::thread::JoinHandle<()>>,

    pub pipewire_context: Option<PipewireInstance>,

    pub temp_window_open: Option<Arc<Mutex<(PipewireVideo, bool, VecDeque<f32>)>>>,
    /// Shared EGL API — constructed once from the GL context and shared with all video players.
    egl: std::sync::Arc<EglApi>,

    // pub current_editing_instance: Option<(LaunchDisplay)>, // Not sure if this should be a LaunchDisplay or a index into existing or what...
}

macro_rules! cur_handler {
    ($self:expr) => {
        &$self.handlers[$self.selected_handler]
    };
}

impl PartyApp {
    pub fn new(monitors: Vec<Monitor>, handler_lite: Option<Handler>, egl: std::sync::Arc<EglApi>) -> Self {
        let options = load_cfg();
        let input_devices = scan_input_devices(&options.pad_filter_type);
        let handlers = match handler_lite {
            Some(_) => Vec::new(),
            None => scan_handlers(),
        };
        let cur_page = match handler_lite {
            Some(_) => MenuPage::Instances,
            None => MenuPage::Home,
        };

        let pipewire_context = PipewireInstance::new().inspect_err(|e| {eprintln!("Failed to start pipewire thread: {e}")}).ok(); 

        // let temp_window_open = (PipewireVideo::new(&cc.clone(), 70, pipewire_context?.sender, streams), false);

        let mut app = Self {
            installed_steamapps: get_installed_steamapps(),
            needs_update: Arc::new(AtomicBool::new(false)),
            options,
            cur_page,
            settings_page: SettingsPage::General,
            infotext: String::new(),
            sys_monitors: monitors,
            input_devices,
            instance_add_dev: None,
            handlers,
            selected_handler: 0,
            handler_edit: None,
            handler_lite,
            profiles: scan_profiles(false),
            loading_msg: None,
            loading_since: None,
            task: None,
            launch_displays: vec![
                LaunchDisplay{
                    layout: Box::new(layout_manager::FlatLayout{
                        split_dir_width: true,
                    }),
                    instances: vec![
                        // Instance {
                        //     devices: vec![],
                        //     profname: "Profile 1".to_string(),
                        //     color: egui::Color32::RED
                        // },
                        // Instance {
                        //     devices: vec![],
                        //     profname: "Profile 2".to_string(),
                        //     color: egui::Color32::BLUE
                        // }
                    ],
                    nested_compositor: LaunchCompositors::Kwin,
                    display_index: 0,
                    move_handle_sel_idx: None
                }
            ],
            launch_display_idx: 0,
            model_temp_modify_profile: None,
            pipewire_context,
            temp_window_open: None,
            egl: egl.clone(),
        };
        
        if let Some(ref pipewire_context) = app.pipewire_context {
            // pass required values to new pipewire video thread.
            if let Ok(pipewire_temp) = PipewireVideo::new(
                &app.egl,
                73,
                pipewire_context.channel.clone(),
                pipewire_context.streams.clone(),
            ) {
                app.temp_window_open = Some(Arc::new(Mutex::new((pipewire_temp, false, VecDeque::new()))));
            }
        }

        if app.options.check_for_updates {
            let needs_update = app.needs_update.clone();
            app.spawn_task("Checking for updates", move || {
                needs_update.store(check_for_partydeck_update(), Ordering::Relaxed);
            });
        }

        app
    }
}

impl eframe::App for PartyApp {
    // fn raw_input_hook(&mut self, _ctx: &egui::Context, raw_input: &mut egui::RawInput) {
    //     if !raw_input.focused || self.task.is_some() {
    //         return;
    //     }
    //     match self.cur_page {
    //         MenuPage::Instances => self.handle_devices_instance_menu(),
    //         _ => self.handle_gamepad_gui(raw_input),
    //     }
    // }

    fn ui(&mut self, ui: &mut Ui, _frame: &mut eframe::Frame) {
        if let Some(temp_window_open) = &self.temp_window_open {
            if temp_window_open.lock().expect("BB").1 {
                let ctx = ui.ctx().clone();
                let viewport_id = ViewportId::from_hash_of("pipewire-video-window");
                let builder = egui::ViewportBuilder::default()
                    .with_title("gamescope stream (egui viewport)")
                    .with_inner_size([960.0, 600.0]);

                // Request fullscreen (optional)
                let builder = builder.with_monitor(0).with_fullscreen(true);

                let temp_window_open_new = temp_window_open.clone();
                ctx.show_viewport_deferred(viewport_id, builder, move |ui, _class| {
                    
                    let mut temp_window_open_new_locked = temp_window_open_new.lock().expect("C");

                    // ui.input(|test: &egui::InputState| {
                    //     test.viewport().events.
                    // })

                    // Honor the native window's close button.
                    if ui.input(|i| i.viewport().close_requested()) {
                        temp_window_open_new_locked.1 = false;
                        // self.video.stop();
                        return;
                    }

                    // Ordinary egui widgets coexist with the video in this window.
                    ui.horizontal(|ui| {

                        let dt = ui.ctx().input(|i| i.unstable_dt);
        
                        temp_window_open_new_locked.2.push_back(dt);
                        if temp_window_open_new_locked.2.len() > 25 {
                            temp_window_open_new_locked.2.pop_front();
                        }

                        // Calculate average frame time
                        let sum: f32 = temp_window_open_new_locked.2.iter().sum();
                        let avg_dt = sum / temp_window_open_new_locked.2.len() as f32;

                        let fps = if avg_dt > 0.0 { 1.0 / avg_dt } else { 0.0 };

                        // Display the counter
                        ui.label(format!("FPS: {:.1}", fps));

                        ui.strong("Live stream");
                        ui.request_repaint();
                        // let (color, text) = if self.video.connected() {
                        //     (egui::Color32::from_rgb(0x3c, 0xb3, 0x71), "connected")
                        // } else {
                        //     (egui::Color32::from_rgb(0xa0, 0xa0, 0xa0), "idle")
                        // };
                        // ui.colored_label(color, text);
                        // if let Some((w, h)) = self.video.native_size() {
                        //     ui.weak(format!("{w}x{h}"));
                        // }
                    });
                    // ui.horizontal(|ui| {
                    //     if ui.button("Play").clicked() {
                    //         self.video.play();
                    //     }
                    //     if ui.button("Stop").clicked() {
                    //         self.video.stop();
                    //     }
                    // });
                    ui.separator();

                    // The video fills the rest, preserving aspect ratio if known.
                    let avail = ui.available_size();
                    let size = avail;
                    // match self.video.native_size() {
                    //     Some((w, h)) if w > 0 && h > 0 => {
                    //         let aspect = w as f32 / h as f32;
                    //         let mut s = avail;
                    //         if s.x / s.y > aspect {
                    //             s.x = s.y * aspect;
                    //         } else {
                    //             s.y = s.x / aspect;
                    //         }
                    //         s
                    //     }
                    //     _ => avail,
                    // };
                    ui.vertical_centered(|ui| {
                        temp_window_open_new_locked.0.ui(ui, size);
                    });
                });
            }
        }


        egui::containers::Panel::top("menu_nav_panel").show(ui, |ui| {
            if self.task.is_some() {
                ui.disable();
            }
            self.display_panel_top(ui);
        });

        if !self.is_lite() {
            egui::containers::Panel::left("games_panel")
                .resizable(false)
                .exact_size(200.0)
                .show(ui, |ui| {
                    if self.task.is_some() {
                        ui.disable();
                    }
                    self.display_panel_left(ui);
                });
        }

        if self.cur_page == MenuPage::Instances {
            egui::Panel::right("devices_panel")
                .resizable(false)
                .exact_size(180.0)
                .show(ui, |ui| {
                    if self.task.is_some() {
                        ui.disable();
                    }
                    let ctx = ui.ctx().clone();
                    self.display_panel_right(ui, &ctx);
                });
        }

        if (self.cur_page != MenuPage::Home) && (self.cur_page != MenuPage::Instances) {
            self.display_panel_bottom(ui);
        }

        egui::CentralPanel::default().show(ui, |ui| {
            if self.task.is_some() {
                ui.disable();
            }
            match self.cur_page {
                MenuPage::Home => self.display_page_main(ui),
                MenuPage::Settings => self.display_page_settings(ui),
                MenuPage::Profiles => self.display_page_profiles(ui),
                MenuPage::EditHandler => self.display_page_edit_handler(ui),
                MenuPage::Game => self.display_page_game(ui),
                MenuPage::Instances => self.display_page_instances(ui),
            }
        });

        if let Some(handle) = self.task.take() {
            if handle.is_finished() {
                let _ = handle.join();
                self.loading_since = None;
                self.loading_msg = None;
            } else {
                self.task = Some(handle);
            }
        }
        if let Some(start) = self.loading_since {
            if start.elapsed() > std::time::Duration::from_secs(60) {
                // Give up waiting after one minute
                self.loading_msg = Some("Operation timed out".to_string());
            }
        }
        if let Some(msg) = &self.loading_msg {
            egui::Area::new("loading".into())
                .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
                .interactable(false)
                .show(ui.ctx(), |ui| {
                    egui::Frame::NONE
                        .fill(egui::Color32::from_rgba_premultiplied(0, 0, 0, 192))
                        .corner_radius(6.0)
                        .inner_margin(egui::Margin::symmetric(16, 12))
                        .show(ui, |ui| {
                            ui.vertical_centered(|ui| {
                                ui.add(egui::widgets::Spinner::new().size(40.0));
                                ui.add_space(8.0);
                                ui.label(msg);
                            });
                        });
                });
        }
        if ui.input(|input| input.focused) {
            ui.request_repaint_after(std::time::Duration::from_millis(33)); // 30 fps
        }
    }
}

impl PartyApp {
    pub fn spawn_task<F>(&mut self, msg: &str, f: F)
    where
        F: FnOnce() + Send + 'static,
    {
        self.loading_msg = Some(msg.to_string());
        self.loading_since = Some(std::time::Instant::now());
        self.task = Some(std::thread::spawn(f));
    }

    pub fn is_lite(&self) -> bool {
        self.handler_lite.is_some()
    }

    fn handle_gamepad_gui(&mut self, raw_input: &mut egui::RawInput) {
        let mut key: Option<egui::Key> = None;

        // let mut input_devices = self.input_devices.lock().unwrap();
        // for pad in input_devices.iter_mut() {
        for pad in &mut self.input_devices {
            if !pad.enabled() {
                continue;
            }
            match pad.poll() {
                Some(PadButton::ABtn) => key = Some(Key::Enter),
                Some(PadButton::BBtn) => {
                    if self.handler_lite.is_some() {
                        self.cur_page = MenuPage::Instances;
                    } else {
                        self.cur_page = MenuPage::Home;
                    }
                }
                Some(PadButton::XBtn) => {
                    self.profiles = scan_profiles(false);
                    self.cur_page = MenuPage::Profiles;
                }
                Some(PadButton::YBtn) => self.cur_page = MenuPage::Settings,
                Some(PadButton::SelectBtn) => key = Some(Key::Tab),
                Some(PadButton::StartBtn) => {
                    if self.cur_page == MenuPage::Game {
                        self.profiles = scan_profiles(true);
                        self.instance_add_dev = None;
                        self.cur_page = MenuPage::Instances;
                    }
                }
                Some(PadButton::Up) => key = Some(Key::ArrowUp),
                Some(PadButton::Down) => key = Some(Key::ArrowDown),
                Some(PadButton::Left) => key = Some(Key::ArrowLeft),
                Some(PadButton::Right) => key = Some(Key::ArrowRight),
                Some(_) => {}
                None => {}
            }
        }

        if let Some(key) = key {
            raw_input.events.push(egui::Event::Key {
                key,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::default(),
            });
        }
    }

    fn handle_devices_instance_menu(&mut self) {
        let mut i = 0;
        while i < self.input_devices.len() {
            if !self.input_devices[i].enabled() {
                i += 1;
                continue;
            }
            match self.input_devices[i].poll() {
                Some(PadButton::ABtn) | Some(PadButton::ZKey) | Some(PadButton::RightClick) => {
                    if self.input_devices[i].device_type() != DeviceType::Gamepad
                        && !self.options.kbm_support
                    {
                        continue;
                    }
                    if !self.options.allow_multiple_instances_on_same_device
                        && self.is_device_in_any_instance(i)
                    {
                        continue;
                    }
                    // Prevent same keyboard/mouse device in multiple instances due to current custom gamescope limitations
                    // TODO: Remove this when custom gamescope supports the same keyboard/mouse device for multiple instances
                    if self.input_devices[i].device_type() != DeviceType::Gamepad
                        && self.is_device_in_any_instance(i)
                    {
                        continue;
                    }

                    // TODO: REPLACE
                    // match self.instance_add_dev {
                    //     Some(inst) => {
                    //         // Add the device in the instance only if it's not already there
                    //         if !self.is_device_in_instance(inst, i) {
                    //             self.instance_add_dev = None;
                    //             self.instances[inst].devices.push(i);
                    //         } else {
                    //             continue;
                    //         }
                    //     }
                    //     None => {
                    //         self.instances.push(Instance {
                    //             devices: vec![i],
                    //             profname: String::new(),
                    //             profselection: 0,
                    //             monitor: 0,
                    //             width: 0,
                    //             height: 0,
                    //         });
                    //     }
                    // }
                }
                Some(PadButton::BBtn) | Some(PadButton::XKey) => {
                    if self.instance_add_dev != None {
                        self.instance_add_dev = None;
                    } else if self.is_device_in_any_instance(i) {
                        self.remove_device(i);
                    // TODO: REPLACE

                    // } else if self.instances.len() < 1 {
                    //     self.cur_page = MenuPage::Game;
                    }
                }
                Some(PadButton::YBtn) | Some(PadButton::AKey) => {
                    if self.instance_add_dev == None {
                        if let Some((instance, _)) = self.find_device_in_instance(i) {
                            self.instance_add_dev = Some(instance);
                        }
                    }
                }
                Some(PadButton::StartBtn) => {
                    // TODO: REPLACE

                    // if self.instances.len() > 0 && self.is_device_in_any_instance(i) {
                    //     self.prepare_game_launch();
                    // }
                }
                _ => {}
            }
            i += 1;
        }
    }

    fn is_device_in_any_instance(&self, dev: usize) -> bool {
        // TODO: REPLACE
        // for instance in &self.instances {
        //     if instance.devices.contains(&dev) {
        //         return true;
        //     }
        // }
        false
    }

    fn is_device_in_instance(&self, instance_index: usize, dev: usize) -> bool {
        // TODO: REPLACE
        // if self.instances[instance_index].devices.contains(&dev) {
        //     return true;
        // }
        false
    }

    fn find_device_in_instance(&mut self, dev: usize) -> Option<(usize, usize)> {
        // TODO: REPLACE

        // for (i, instance) in self.instances.iter().enumerate() {
        //     for (d, device) in instance.devices.iter().enumerate() {
        //         if device == &dev {
        //             return Some((i, d));
        //         }
        //     }
        // }
        None
    }

    fn find_device_in_instance_from_end(&mut self, dev: usize) -> Option<(usize, usize)> {
        // TODO: REPLACE
        // for (i, instance) in self.instances.iter().enumerate().rev() {
        //     for (d, device) in instance.devices.iter().enumerate() {
        //         if device == &dev {
        //             return Some((i, d));
        //         }
        //     }
        // }
        None
    }

    pub fn remove_device(&mut self, dev: usize) {
        // TODO: REPLACE
        
        // if let Some((instance_index, device_index)) = self.find_device_in_instance_from_end(dev) {
        //     self.instances[instance_index].devices.remove(device_index);
        //     if self.instances[instance_index].devices.is_empty() {
        //         self.instances.remove(instance_index);
        //     }
        // }
    }

    pub fn remove_device_instance(&mut self, instance_index: usize, dev: usize) {
        // TODO: REPLACE
        // let device_index = self.instances[instance_index]
        //     .devices
        //     .iter()
        //     .position(|device| device == &dev);

        // if let Some(d) = device_index {
        //     self.instances[instance_index].devices.remove(d);

        //     if self.instances[instance_index].devices.is_empty() {
        //         self.instances.remove(instance_index);
        //     }
        // }
    }

    pub fn prepare_game_launch(&mut self ) {
        let handler = if let Some(h) = self.handler_lite.clone() {
            h
        } else {
            cur_handler!(self).to_owned()
        };


        let cfg = self.options.clone();
        let _ = save_cfg(&cfg);

        self.cur_page = MenuPage::Home;

        let input_devices: Vec<RunningInputDevice> = 
            self.input_devices.iter().map(|value| {
                RunningInputDevice::new(value)
            }).collect();

        let sys_monitors = self.sys_monitors.clone();
        
        let mut launch_displays: Vec<RunningLaunchDisplay> = 
            self.launch_displays.iter().map(|value| {
                RunningLaunchDisplay::new(value)
            }).collect();

        self.spawn_task(
            "Launching...\n\nDon't press any buttons or move any analog sticks or mice.",
            move || {
                sleep(std::time::Duration::from_secs_f32(1.5));


                

                let flattened_instances = &launch_displays.iter().flat_map(
                    |display| &display.instances
                ).collect();

                if let Err(err) = setup_profiles(&handler, &flattened_instances) {
                    println!("[partydeck] Error mounting game directories: {}", err);
                    msg("Failed mounting game directories", &format!("{err}"));
                    return;
                }
                if handler.is_saved_handler()
                    && !cfg.disable_mount_gamedirs
                    && cfg.profile_unique_dirs
                    && let Err(err) = fuse_overlayfs_mount_gamedirs(&handler, &flattened_instances)
                {
                    println!("[partydeck] Error mounting game directories: {}", err);
                    msg("Failed mounting game directories", &format!("{err}"));
                    return;
                }

                if let Err(err) =
                    launch_game(&handler, &input_devices, &mut launch_displays, &cfg, &sys_monitors)
                {
                    println!("[partydeck] Error launching instances: {}", err);
                    msg("Launch Error", &format!("{err}"));
                }
                if cfg.enable_kwin_script {
                    if let Err(err) = layout_manager::kwin_dbus_unload_script() {
                        println!("[partydeck] Error unloading KWin script: {}", err);
                        msg("Failed unloading KWin script", &format!("{err}"));
                    }
                }
                if let Err(err) = remove_guest_profiles() {
                    println!("[partydeck] Error removing guest profiles: {}", err);
                    msg("Failed removing guest profiles", &format!("{err}"));
                }
                if let Err(err) = clear_tmp() {
                    println!("[partydeck] Error removing tmp directory: {}", err);
                    msg("Failed removing tmp directory", &format!("{err}"));
                }
            },
        );
    }
}
