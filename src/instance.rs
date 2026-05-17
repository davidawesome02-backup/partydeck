use std::process::{Child, Command};

use eframe::egui;

use crate::input::DeviceHash;
use crate::layout_manager::LayoutWindows;

pub struct LaunchDisplay {
    pub layout: Box<dyn LayoutWindows+Send>,
    pub instances: Vec<Instance>,
    pub nested_compositor: String,
    pub display_index: usize,
    pub move_handle_sel_idx: Option<usize>,
}

pub struct RunningLaunchDisplay {
    pub layout: Box<dyn LayoutWindows+Send>,
    pub instances: Vec<RunningInstance>,
    pub nested_compositor: String,
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

    // Profidx may be changed or removed... idk if we need it. Its a refrence to the profile list, but this could change.
    // pub profidx: usize,
    // pub instidx: usize,
    // pub monidx: usize,

    pub color: egui::Color32,
}

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