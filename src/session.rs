use std::collections::HashSet;

use eframe::egui::Color32;

use crate::input::DeviceHash;
use crate::layout::{Layout, WindowPosition};
use crate::profiles::next_temp_name;
use crate::util::next_instance_color;

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct InstanceId(u64);

pub struct Instance {
    pub id: InstanceId,
    pub devices: Vec<DeviceHash>,
    pub profname: String,
    pub color: Color32,
}

impl Instance {
    pub fn has_device(&self, hash: DeviceHash) -> bool {
        self.devices.contains(&hash)
    }
}

#[derive(Default)]
pub struct Display {
    pub monitor: usize,
    pub layout: Layout,
    pub instances: Vec<Instance>,
}

impl Display {
    pub fn is_empty(&self) -> bool {
        self.instances.is_empty()
    }

    pub fn window_positions(&self, width: u32, height: u32) -> Vec<WindowPosition> {
        self.layout.windows(self.instances.len(), width, height)
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

    pub fn selected_display(&self) -> &Display {
        &self.displays[self.selected]
    }

    pub fn selected_display_mut(&mut self) -> &mut Display {
        &mut self.displays[self.selected]
    }

    pub fn device_used_by_other(&self, hash: DeviceHash, exclude: InstanceId) -> bool {
        self.all_instances()
            .any(|instance| instance.id != exclude && instance.has_device(hash))
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
        });
        id
    }

    pub fn retain_devices(&mut self, present: &[DeviceHash]) {
        for instance in self.all_instances_mut() {
            instance.devices.retain(|hash| present.contains(hash));
        }
    }

    pub fn remove_instance(&mut self, id: InstanceId) {
        for display in &mut self.displays {
            if let Some(i) = display.instances.iter().position(|instance| instance.id == id) {
                display.instances.remove(i);
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
