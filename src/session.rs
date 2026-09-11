use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::io::Read;
use std::os::fd::{AsRawFd, OwnedFd};
use std::process::Command;
use std::sync::{Arc, RwLock, Mutex};

use eframe::egui::{self, Color32};
use evdev::InputEvent;

use crate::input::{DeviceRefrence, InputStateInner};
use crate::layout::{Layout, WindowPosition};
use crate::monitor::Monitor;
use crate::profiles::next_temp_name;
use crate::unshare::{NamespaceSetup, RemoteNamespace};
use crate::util::next_instance_color;
use crate::video::egl::EglApi;
use crate::video::gamescope::{GamescopeWaylandState, InstanceStreamView};
use crate::video::pipewire::{PipewireCommand, PipewireID, PipewireInstance, PipewireStream};
use pipewire as pw;

use std::time::{Duration, Instant};

#[derive(PartialEq)]
pub enum InstanceAction {
    None,
    Edit(InstanceId),
    Remove(InstanceId),
    Swap(InstanceId, InstanceId),
}

pub enum InstanceInputEvt {
    RemoveDev(String),
    AddDev(String),
    InputEvt(InputEvent),
}


#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct InstanceId(pub u64);

pub struct InstanceLaunched {
    pub last_error_dont_retry: Option<InstanceLaunchedStatus>,

    pub start_instant: Instant, // Time to wait for to start gamescope. (any time past this we are running)
    pub gamescope_proc: Option<(RemoteNamespace, OwnedFd, Instant)>,
    pub stream_view: Option<InstanceStreamView>,

    pub input_handler: Arc<Mutex<InputStateInner>>,

    pub egl: Arc<EglApi>,
    pub pw_sender: pw::channel::Sender<PipewireCommand>,
    pub pw_streams: Arc<RwLock<HashMap<PipewireID, Arc<RwLock<PipewireStream>>>>>,
    pub ctx: egui::Context,
    pub viewport_id: egui::ViewportId,

    pub starting_position: WindowPosition,

    pub input_events: Arc<Mutex<Vec<InstanceInputEvt>>>
}

#[derive(PartialEq)]
#[derive(Clone)]
pub enum InstanceLaunchedStatus {
    NotStarted,
    StartTimeout(u64),
    WaitingFD,
    Ready,
    Failed,
    Exited
}

impl std::fmt::Display for InstanceLaunchedStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let status_str = match self {
            Self::NotStarted => "Not started properly",
            Self::StartTimeout(timeout) => &format!("Waiting {}s...", timeout),
            Self::WaitingFD => "Waiting for gamescope to respond",
            Self::Failed => "Failed to start",
            Self::Exited => "Exited",

            Self::Ready => "Ready waiting for frames!",
        };
        write!(f, "{}", status_str)
    }
}

pub struct InstanceSpecificHandler {
    pub pause_between_starts: Option<f64>
}

pub struct Instance {
    pub id: InstanceId,
    pub devices: Vec<DeviceRefrence>,
    pub profname: String,
    pub color: Color32,
    pub handler: InstanceSpecificHandler,

    pub launch_data: Option<InstanceLaunched>
}

// Fn is_alive (used to prevent rendering in layout, but without removing form display vec)
use nix::libc::FIONREAD;
use nix::ioctl_read_bad;
use std::os::unix::io::RawFd;
ioctl_read_bad!(fionread, FIONREAD, nix::libc::c_int);
fn bytes_available(fd: RawFd) -> nix::Result<i32> {
    let mut n: nix::libc::c_int = 0;
    unsafe {
        fionread(fd, &mut n)?;
    }
    Ok(n)
}


impl Instance {
    pub fn edit_ui(&mut self, ui: &mut egui::Ui, tile_rect: egui::Rect) -> InstanceAction {
        let mut action: InstanceAction = InstanceAction::None;
        ui.scope_builder(
            egui::UiBuilder::new()
                .id_salt(self.id)
                .max_rect(tile_rect)
                .sense(egui::Sense::hover())
                .layout(egui::Layout::top_down(egui::Align::LEFT)),
            |ui| {
                ui.set_width(tile_rect.width());
                ui.set_height(tile_rect.height());

                let frame = egui::Frame::default()
                    .corner_radius(2)
                    .stroke(egui::Stroke::new(2.0, Color32::GRAY));

                let dropped_payload = ui.dnd_drop_zone::<InstanceId, ()>(frame, |ui| {
                    let dnd_id = ui.make_persistent_id("setup_instance");

                    dnd_drag_source(ui, dnd_id, self.id, |ui| {
                        egui::Frame::NONE
                            .fill(Color32::from_gray(20))
                            .corner_radius(2)
                            .stroke(egui::Stroke::new(2.0, self.color))
                            .show(ui, |ui| {
                                ui.set_width(ui.available_width());
                                ui.set_height(ui.available_height());
                                ui.horizontal(|ui| {
                                    ui.style_mut().spacing.item_spacing.x = 3.0;

                                    let move_handle = ui.button("\u{01F5D7}").on_hover_text("Move");
                                    ui.interact(move_handle.rect, dnd_id, egui::Sense::drag())
                                        .on_hover_cursor(egui::CursorIcon::Grab);

                                    if ui.button("\u{01F5D1}").on_hover_text("Remove").clicked() {
                                        action = InstanceAction::Remove(self.id);
                                    }

                                    if ui.button("✏").on_hover_text("Edit").clicked() {
                                        action = InstanceAction::Edit(self.id);
                                    }

                                    ui.add(egui::Label::new(self.profname.clone()).truncate());
                                });
                            });
                    });
                    ui.set_width(ui.available_width());
                    ui.set_height(ui.available_height());
                });

                if let Some(dropped) = dropped_payload.1 {
                    if *dropped != self.id {
                        action = InstanceAction::Swap(*dropped, self.id);
                    }
                }
            },
        );

        action
    }

    pub fn start_instance(&mut self, next_timeout: &mut Instant, upper_launch_data: &DisplayLaunched, position: WindowPosition, input_handler: Arc<Mutex<InputStateInner>>) {
        if self.launch_data.is_some() {return;}

        // self.devices.iter_mut().for_each(|d| d.set_grabbed(state, true));

        self.launch_data = Some(InstanceLaunched {
            last_error_dont_retry: None,
            start_instant: next_timeout.clone(),
            gamescope_proc: None,

            input_handler,

            egl: upper_launch_data.egl.clone(),
            pw_sender: upper_launch_data.pw_sender.clone(),
            pw_streams: upper_launch_data.pw_streams.clone(),
            ctx: upper_launch_data.ctx.clone(),
            viewport_id: upper_launch_data.viewport_id.clone(),
            stream_view: None,

            starting_position: position,

            input_events: Arc::new(Mutex::new(Vec::new())),
        });

        if let Some(delay) = self.handler.pause_between_starts {
            *next_timeout+=Duration::from_secs_f64(delay);
        }
    }

    pub fn running_ui(&mut self, ui: &mut egui::Ui, tile_rect: egui::Rect) {

        ui.scope_builder(
            egui::UiBuilder::new()
                .id_salt(self.id)
                .max_rect(tile_rect)
                .sense(egui::Sense::hover())
                .layout(egui::Layout::top_down(egui::Align::LEFT)),
            |ui| {
                ui.set_width(tile_rect.width());
                ui.set_height(tile_rect.height());

                let update_response = self.post_launch_update();
                if update_response != InstanceLaunchedStatus::Ready {
                    if matches!(update_response, InstanceLaunchedStatus::StartTimeout(_) | InstanceLaunchedStatus::WaitingFD) {
                        ui.request_repaint_after_secs(0.1);
                    }

                    ui.centered_and_justified(|ui| ui.label(update_response.to_string()));
                    return;
                }

                // TODO REPLACE WITH REAL DRAW CALLS!
                // ui.centered_and_justified(|ui| ui.label("READY!!"));
                if let Some(a) = &mut self.launch_data && let Some(b) = &mut a.stream_view {
                    let _ = b.ui(ui, egui::Vec2::new(ui.available_width(), ui.available_height())).inspect_err(|e| eprintln!("Ignoring error: {e}"));
                }

        });
    }

    pub fn post_launch_update(&mut self) -> InstanceLaunchedStatus {
        let now = Instant::now();

        let Some(ref mut launch_data) = self.launch_data else {
            return InstanceLaunchedStatus::NotStarted;
        };

        if let Some(ref last_err) = launch_data.last_error_dont_retry {
            launch_data.stream_view = None; // Triggers drop of the view, so we dont keep getting callbacks.
            return last_err.clone();
        }

        if launch_data.start_instant > now {
            return InstanceLaunchedStatus::StartTimeout((launch_data.start_instant-now).as_secs());
        }

        if launch_data.gamescope_proc.is_none() {
            if let Some(launch_fail_msg) = self.launch_game(now).err() {
                eprintln!("Gamescope launch failed: {}", launch_fail_msg);
                let Some(ref mut launch_data) = self.launch_data else {return InstanceLaunchedStatus::NotStarted;};
                launch_data.last_error_dont_retry = Some(InstanceLaunchedStatus::Failed);
                return InstanceLaunchedStatus::Failed;
            }
        }

        // Todo fix this I dont want to attempt to unwrap multiple times if possible, but self.launch_game borrows :|
        let Some(ref mut launch_data) = self.launch_data else {return InstanceLaunchedStatus::NotStarted;};

        let Some(ref mut launch_proc) = launch_data.gamescope_proc else {return InstanceLaunchedStatus::Failed;};

        if !matches!(launch_proc.0.child_pidfd.try_wait(), Ok(None)) {
            // launch_data.

            launch_data.last_error_dont_retry = Some(InstanceLaunchedStatus::Exited);
            
            if let Ok(mut input_handler) = launch_data.input_handler.lock() {
                // input_handler
                for ele in &mut self.devices {
                    ele.set_grabbed(&mut input_handler, false);
                }
            }

            return InstanceLaunchedStatus::Exited;
        }

        if launch_data.stream_view.is_none() {
            if now > launch_proc.2 {
                eprintln!("Timeout waiting failed :(");
                launch_data.last_error_dont_retry = Some(InstanceLaunchedStatus::Failed);
                return InstanceLaunchedStatus::Failed;
            }



            let Ok(bytes_avalib) = bytes_available(launch_proc.1.as_raw_fd()) else {
                eprintln!("Byte length failed to request..?");
                launch_data.last_error_dont_retry = Some(InstanceLaunchedStatus::Failed);
                return InstanceLaunchedStatus::Failed;
            };

            if bytes_avalib<=0 {
                return InstanceLaunchedStatus::WaitingFD;
            }

            let Ok(fd) = launch_proc.1.try_clone() else {
                eprintln!("Failed to clone launch proc fd for reading.");
                launch_data.last_error_dont_retry = Some(InstanceLaunchedStatus::Failed);
                return InstanceLaunchedStatus::Failed;
            };

            let mut file = File::from(fd);
            let mut temp_buf_vec = vec![0; bytes_avalib as usize];
            let buf = temp_buf_vec.as_mut_slice();
            if let Some(file_read_failed) = file.read_exact(buf).err() {
                eprintln!("Gamescope file read failed: {}", file_read_failed);
                launch_data.last_error_dont_retry = Some(InstanceLaunchedStatus::Failed);
                return InstanceLaunchedStatus::Failed;
            }

            let readyfd_str_buf = String::from_utf8_lossy(buf);

            let split_readyfd_buf = readyfd_str_buf
                .split_whitespace()
                .collect::<Vec<_>>();

            let Some(wl_display) = split_readyfd_buf
                .get(1) else {
                    eprintln!("Gamescope FD invlid response");
                    launch_data.last_error_dont_retry = Some(InstanceLaunchedStatus::Failed);
                    return InstanceLaunchedStatus::Failed;
                };



            let Ok(xdg_runtime_dir) = std::env::var("XDG_RUNTIME_DIR") else {
                launch_data.last_error_dont_retry = Some(InstanceLaunchedStatus::Failed);
                return InstanceLaunchedStatus::Failed;
            }; // Follow as I think gamescope does.
            let wayland_socket_path = std::path::PathBuf::from(xdg_runtime_dir).join(wl_display);

            let Ok(mut wayland_state) = GamescopeWaylandState::new(&wayland_socket_path)
                .inspect_err(|e| eprintln!("Failed to connect to wayland server ({wayland_socket_path:#?}): {e}")) else {
                    launch_data.last_error_dont_retry = Some(InstanceLaunchedStatus::Failed);
                    return InstanceLaunchedStatus::Failed;
                };

            // todo fix this if it matters.
            if wayland_state.get_size().and_then(|_| {wayland_state.round_trip()}).is_err() {
                eprintln!("WL state failed to round trip");
                launch_data.last_error_dont_retry = Some(InstanceLaunchedStatus::Failed);
                return InstanceLaunchedStatus::Failed;
            }


            let Ok(stream_view) =
                InstanceStreamView::new(&launch_data.egl.clone(), wayland_state, launch_data.pw_sender.clone(), launch_data.pw_streams.clone(), &launch_data.ctx, launch_data.viewport_id)
                .inspect_err(|e| eprintln!("Failed to start instance view: {e}")) else {
                    launch_data.last_error_dont_retry = Some(InstanceLaunchedStatus::Failed);
                    return InstanceLaunchedStatus::Failed;
                };

            launch_data.stream_view = Some(stream_view);

        }

        for evt in launch_data.input_events.lock().unwrap().drain(..) {
            match evt {
                InstanceInputEvt::RemoveDev(path) => {
                    if launch_proc.0.unbind_device(path.to_string()).is_err() {
                        launch_data.last_error_dont_retry = Some(InstanceLaunchedStatus::Failed);
                        return InstanceLaunchedStatus::Failed;
                    }
                },
                InstanceInputEvt::AddDev(path) => {
                    if launch_proc.0.bind_device(path.to_string()).is_err() {
                        launch_data.last_error_dont_retry = Some(InstanceLaunchedStatus::Failed);
                        return InstanceLaunchedStatus::Failed;
                    }
                },
                InstanceInputEvt::InputEvt(input_event) => {
                    let Some(ref mut stream) = launch_data.stream_view else {continue};
                    if stream.inject_input(input_event).inspect_err(|e| eprintln!("Failed to send input evt: {e}")).is_err() {
                        launch_data.last_error_dont_retry = Some(InstanceLaunchedStatus::Failed);
                        return InstanceLaunchedStatus::Failed;
                    }
                },
            }
        }


        InstanceLaunchedStatus::Ready
    }

    pub fn launch_game(&mut self, now: Instant) -> Result<(), String> {
        // TODO SPAWN GAMESCOPE PROC

        let Some(ref mut launch_data) = self.launch_data else {
            return Ok(());
        };

        let mut input_state = launch_data.input_handler.lock().unwrap();
        // input_handler.targets.iter().filter(|f| f).next();
        let mut used_dev_paths = Vec::new();
        for dev in &mut self.devices {
            dev.set_grabbed(&mut input_state, true);

            let mut dev_is_kbm = false;
            
            for real_dev in input_state.devices_ordered() {
                if real_dev.1.device_id == Some(dev.device_id) {
                    if let Some(path) = real_dev.0.clone().file_name().and_then(|fname| fname.to_str()).map(|fname| String::from(fname)) {
                        used_dev_paths.push(path);
                    }

                    dev_is_kbm = match real_dev.1.device_type() {
                        crate::input::DeviceType::Gamepad => false,
                        crate::input::DeviceType::Keyboard => true,
                        crate::input::DeviceType::Mouse => true,
                        crate::input::DeviceType::Other => false,
                    };

                    break;
                }
            }
            let input_events_clone = launch_data.input_events.clone();
            let ctx = launch_data.ctx.clone();
            let viewport_id = launch_data.viewport_id.clone();
            if dev_is_kbm {
                dev.set_input_callback(&mut input_state, Some(Box::new(move |input_events| {
                    // println!("Held device pressed: {:#?}", input_events);
                    input_events_clone.lock().unwrap().extend(input_events.iter().map(|f| InstanceInputEvt::InputEvt(f.clone())));
                    // ui
                    ctx.request_repaint_once_for(viewport_id);
                })));
            }
        }

        let (ready_read, ready_write) = nix::unistd::pipe().map_err(|e| format!("Pipe creation error: {e}"))?;
        let mut cmd = Command::new("gamescope");
        cmd.args(["-R", &format!("/proc/self/fd/{}", ready_write.as_raw_fd())]);
        cmd.args(["--composite-cursor", "--force-windows-fullscreen", "--nested-follow-window-scale", "1"]);
        cmd.args(["-W", &launch_data.starting_position.w.to_string(), "-w", &launch_data.starting_position.w.to_string()]);
        cmd.args(["-H", &launch_data.starting_position.h.to_string(), "-h", &launch_data.starting_position.h.to_string()]);
        cmd.args(["--backend", "headless"]);
        cmd.args(["--", "konsole"]);

        
        let child_rmt = RemoteNamespace::new(NamespaceSetup{
            cmd,
            input_devs: used_dev_paths
        }).map_err(|e| format!("{e:?}"))?;

        for dev in &mut self.devices {
            let input_events_clone = launch_data.input_events.clone();
            let ctx = launch_data.ctx.clone();
            let viewport_id = launch_data.viewport_id.clone();

            dev.set_dev_change_callback(&mut input_state, Some(Box::new(move |dev_path, is_addition| {
                println!("Dev changed on held device: {:#?} - {:#?}", dev_path, is_addition);
                if let Some(path) = dev_path.file_name().and_then(|fname| fname.to_str()).map(|fname| String::from(fname)) {
                    input_events_clone.lock().unwrap().push(
                        if is_addition {
                            InstanceInputEvt::AddDev(path)
                        } else {
                            InstanceInputEvt::RemoveDev(path)
                        }
                    );
                    ctx.request_repaint_once_for(viewport_id);
                };
            })));
        }

        launch_data.gamescope_proc = Some((child_rmt, ready_read, now+Duration::from_secs(5)));

        Ok(())
    }

    pub fn is_alive_or_starting(&mut self) -> bool {
        let Some(ref mut ld) = self.launch_data else {return true};
        let is_alive = ld.last_error_dont_retry.is_none();
        if !is_alive {
            ld.stream_view = None;
        }

        is_alive
    }

    pub fn kill_game(&mut self) {
        let Some(ref mut ld) = self.launch_data else {return};
        ld.last_error_dont_retry = Some(InstanceLaunchedStatus::Exited);

        let Some(ref mut prgm) = ld.gamescope_proc else {return};
        let _ = prgm.0.child_pidfd.signal(nix::sys::signal::Signal::SIGKILL);

        ld.stream_view = None;
    }
}


pub struct DisplayLaunched {
    pub egl: Arc<EglApi>,
    pub pw_sender: pw::channel::Sender<PipewireCommand>,
    pub pw_streams: Arc<RwLock<HashMap<PipewireID, Arc<RwLock<PipewireStream>>>>>,
    pub ctx: egui::Context,
    pub viewport_id: egui::ViewportId,
}

#[derive(Default)]
pub struct Display {
    pub monitor_idx: usize,
    pub layout: Layout,
    pub instances: Vec<Instance>,

    pub launch_data: Option<DisplayLaunched>, // If this is not none, will run all of the container_ui
}

impl Display {
    pub fn is_empty(&self) -> bool {
        self.instances.is_empty()
    }

    pub fn window_positions(&self, width: u32, height: u32) -> Vec<WindowPosition> {
        self.layout.windows(self.instances.len(), width, height)
    }

    pub fn editor_ui(&mut self, ui: &mut egui::Ui, width: f32, height: f32, target_res: (u32, u32)) -> InstanceAction {
        let top_left_cursor = ui.cursor().left_top().to_vec2();
        let layout = self.window_positions(target_res.0, target_res.1);
        let mut action: InstanceAction = InstanceAction::None;
        for (instance_idx, window) in layout.iter().enumerate() {
            let instance = &mut self.instances[instance_idx];
            let tile_rect = egui::Rect::from_min_size(
                egui::pos2(
                    window.x as f32 * width / target_res.0 as f32,
                    window.y as f32 * height / target_res.1 as f32,
                ) + top_left_cursor,
                egui::vec2(
                    window.w as f32 * width / target_res.0 as f32,
                    window.h as f32 * height / target_res.1 as f32,
                ),
            );

            let new_action = instance.edit_ui(ui, tile_rect);
            if action == InstanceAction::None {action = new_action;}
        }

        action
    }

    pub fn start_display(&mut self, ui: &mut egui::Ui, next_timeout: &mut Instant, viewport_id: egui::ViewportId, egl: Arc<EglApi>, pw: &PipewireInstance, mon: Monitor, input_handler: Arc<Mutex<InputStateInner>>) {
        let new_launch_data = DisplayLaunched {
            egl: egl,
            pw_sender: pw.channel.clone(),
            pw_streams: pw.streams.clone(),
            ctx: ui.ctx().clone(),
            viewport_id,
        };

        let layout = self.window_positions(mon.width(), mon.height());

        self.instances.iter_mut().zip(layout).for_each(|(inst, position)| inst.start_instance(next_timeout, &new_launch_data, position, input_handler.clone()));

        self.launch_data = Some(new_launch_data);


        // *next_timeout+=Duration::from_secs(5);
    }


    pub fn display_ui(&mut self, ui: &mut egui::Ui) {
        let height = ui.available_height();
        let width = ui.available_width();
        let pixels_scale = ui.pixels_per_point();
        let target_res = ((width * pixels_scale) as u32, (height * pixels_scale) as u32); // TODO replace *10 with proper sizing stuff egui scale. We should use the display ratio [see cursor egui input], but for now just using big number.
        let top_left_cursor = ui.cursor().left_top().to_vec2();
        let layout = self.window_positions(target_res.0, target_res.1);

        for (instance_idx, window) in layout.iter().enumerate() {
            let instance = &mut self.instances[instance_idx];
            let tile_rect = egui::Rect::from_min_size(
                egui::pos2(
                    window.x as f32 * width / target_res.0 as f32,
                    window.y as f32 * height / target_res.1 as f32,
                ) + top_left_cursor,
                egui::vec2(
                    window.w as f32 * width / target_res.0 as f32,
                    window.h as f32 * height / target_res.1 as f32,
                ),
            );

            instance.running_ui(ui, tile_rect);
        }
    }

    pub fn is_alive(&mut self) -> bool {
        self.instances.iter_mut().any(|i| {
            i.is_alive_or_starting()
        })
    }
}

pub struct Session {
    pub displays: Vec<Display>,
    pub selected: usize,
    next_instance_id: u64,
}

impl Default for Session {
    fn default() -> Self {
        Self {
            displays: vec![Display::default()],
            selected: 0,
            next_instance_id: 0,
        }
    }
}

impl Session {
    fn all_instances(&self) -> impl Iterator<Item = &Instance> {
        self.displays.iter().flat_map(|display| &display.instances)
    }

    fn all_instances_mut(&mut self) -> impl Iterator<Item = &mut Instance> {
        self.displays.iter_mut().flat_map(|display| &mut display.instances)
    }

    pub fn instance_mut_by_id(&mut self, id: InstanceId) -> Option<&mut Instance> {
        self.all_instances_mut().find(|instance| instance.id == id)
    }

    pub fn selected_display(&mut self) -> &mut Display {
        &mut self.displays[self.selected]
    }

    pub fn selected_display_mut(&mut self) -> &mut Display {
        &mut self.displays[self.selected]
    }

    pub fn add_instance(&mut self) -> InstanceId {
        let profname = next_temp_name(&self.used_profile_names(None));
        let id = InstanceId(self.next_instance_id);
        self.next_instance_id += 1;
        let display = self.selected_display_mut();
        let used: Vec<Color32> = display.instances.iter().map(|instance| instance.color).collect();
        let color = next_instance_color(&used);
        display.instances.push(Instance {
            id,
            devices: Vec::new(),
            profname,
            color,
            handler: InstanceSpecificHandler{pause_between_starts:Some(1.0)},
            launch_data: None,
        });
        id
    }

    pub fn remove_instance(&mut self, in_state: &mut InputStateInner, id: InstanceId) {
        for display in &mut self.displays {
            if let Some(i) = display.instances.iter().position(|instance| instance.id == id) {
                let mut instance = display.instances.remove(i);
                instance.devices.iter_mut().for_each(|d| d.pre_drop_nofix(in_state));
                in_state.fix_users();
                break;
            }
        }
        self.condense_empty();
    }

    pub fn swap(&mut self, a: InstanceId, b: InstanceId) {
        let instances = &mut self.displays[self.selected].instances;
        if let (Some(ia), Some(ib)) = (
            instances.iter().position(|instance| instance.id == a),
            instances.iter().position(|instance| instance.id == b),
        ) {
            instances.swap(ia, ib);
        }
    }

    pub fn used_profile_names(&self, exclude: Option<InstanceId>) -> HashSet<String> {
        self.all_instances()
            .filter(|instance| Some(instance.id) != exclude)
            .map(|instance| instance.profname.clone())
            .collect()
    }

    pub fn remove_selected_display(&mut self) {
        self.displays.remove(self.selected);
        self.selected = self.selected.min(self.displays.len() - 1);
    }

    fn condense_empty(&mut self) {
        let mut i = 0;
        while i < self.displays.len() {
            if i != self.selected && self.displays[i].is_empty() {
                self.displays.remove(i);
                if i < self.selected {
                    self.selected -= 1;
                }
            } else {
                i += 1;
            }
        }
    }

    pub fn can_launch(&self) -> bool {
        self.all_instances().next().is_some()
    }
}



fn dnd_drag_source<R>(
    ui: &mut egui::Ui,
    id: egui::Id,
    payload: InstanceId,
    add_contents: impl FnOnce(&mut egui::Ui) -> R,
) -> egui::InnerResponse<R> {
    let is_being_dragged = ui.ctx().is_being_dragged(id);

    if is_being_dragged {
        egui::DragAndDrop::set_payload(ui.ctx(), payload);

        let layer_id = egui::LayerId::new(egui::Order::Tooltip, id);
        let egui::InnerResponse { inner, response } =
            ui.scope_builder(egui::UiBuilder::new().layer_id(layer_id), add_contents);

        if let Some(pointer_pos) = ui.ctx().pointer_interact_pos() {
            let delta = pointer_pos - response.rect.left_top() - egui::vec2(12.0, 12.0);
            ui.ctx()
                .transform_layer_shapes(layer_id, egui::emath::TSTransform::from_translation(delta));
        }

        egui::InnerResponse::new(inner, response)
    } else {
        ui.scope(add_contents)
    }
}
