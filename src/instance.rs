use std::process::{Child, Command};

use eframe::egui;

use crate::input::DeviceHash;
use crate::layout_manager::LayoutWindows;


pub struct LaunchDisplay {
    pub layout: Box<dyn LayoutWindows>,
    pub instances: Vec<Instance>,
    pub nested_compositor: String,
    pub display_index: usize,

    // None until starting the compositor durring launch.
    pub compositor_proc: Option<Child>,
}

pub struct Instance {
    pub devices: Vec<DeviceHash>,// u64 - device hash
    pub profname: String,
    pub temp: bool,

    pub profidx: usize,
    pub instidx: usize,
    pub monidx: usize,

    pub color: egui::Color32,


    // Populated durring launch only.
    pub command: Option<Command>,
    pub game_proc: Option<Child>,
}