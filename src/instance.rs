use std::process::{Child, Command};

use eframe::egui;

use crate::input::DeviceHash;
use crate::layout_manager::LayoutWindows;

#[derive(PartialEq)]
#[derive(Clone)]
pub enum LaunchCompositors {
    River,
    Kwin,
    Native,
}
impl LaunchCompositors {
    pub fn display_name(&self) -> String {
        match self {
            LaunchCompositors::River =>     "River (nested)",
            LaunchCompositors::Kwin =>      "Kwin (nested)",
            LaunchCompositors::Native =>    "Native",
        }.to_string()
    }
    pub fn launch_executable(&self) -> Option<String> {
        match self {
            LaunchCompositors::River =>     Some("river".to_string()),
            LaunchCompositors::Kwin =>      Some("kwin_wayland".to_string()),
            LaunchCompositors::Native =>    None,
        }
    }
}

pub struct LaunchDisplay {
    pub layout: Box<dyn LayoutWindows+Send>,
    pub instances: Vec<Instance>,
    pub nested_compositor: LaunchCompositors,
    pub display_index: usize,
    pub move_handle_sel_idx: Option<usize>,
}

pub struct RunningLaunchDisplay {
    pub layout: Box<dyn LayoutWindows+Send>,
    pub instances: Vec<RunningInstance>,
    pub nested_compositor: LaunchCompositors,
    pub display_index: usize,

    // None until starting the compositor durring launch.
    pub compositor_proc: Option<Child>,
}
impl RunningLaunchDisplay {
    pub fn new(input: &LaunchDisplay) -> Self {
        Self {
            layout:                 input.layout.clone_box(),
            instances:              input.instances.iter().map(|value| {
                                        RunningInstance::new(value)
                                    }).collect(),
            nested_compositor:      input.nested_compositor.clone(),
            display_index:          input.display_index,
            compositor_proc: None,
        }
    }
}


pub struct Instance {
    pub devices: Vec<DeviceHash>,// u64 - device hash
    pub profname: String,
    // pub temp_profile: bool,

    // pub model_editing_idx: Option<usize>,

    // Profidx may be changed or removed... idk if we need it. Its a refrence to the profile list, but this could change.
    // pub profidx: usize,
    // pub instidx: usize,
    // pub monidx: usize,

    pub color: egui::Color32,
}
// impl Instance {
//     pub fn profname_disk(self: &Self) -> String {
//         (if self.temp_profile {"."} else {""}).to_owned() + &self.profname
//     }
// }

pub struct RunningInstance {
    pub devices: Vec<DeviceHash>,// u64 - device hash
    pub profname: String,

    // Populated durring launch only.
    pub command: Option<Command>,
    pub game_proc: Option<Child>,
}

impl RunningInstance {
    pub fn new(input: &Instance) -> Self {
        Self {
            devices:    input.devices.clone(),
            profname:   input.profname.clone(),
            command: None,
            game_proc: None,
        }
    }
}