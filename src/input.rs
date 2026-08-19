
use std::{
    collections::{HashMap, VecDeque}, hash::{Hash, Hasher}, num::NonZeroU64, os::fd::{AsFd, BorrowedFd, OwnedFd}, path::PathBuf, sync::{
        Arc, Mutex, mpsc::{Receiver, Sender, channel},
    }, thread::{self, JoinHandle},
};

use evdev::{AbsoluteAxisCode, Device, EventSummary, InputEvent, InputId, KeyCode};
use nix::{poll::{PollFd, PollFlags, PollTimeout, poll}, unistd::dup};
use eframe::egui;

use crate::app::PadFilterType;

const POLL_TIMEOUT_MS: i32 = 5000;

type SharedLeaseId = u64;
type LeaseId = u64;
type DeviceHash = u64;





#[derive(Clone, PartialEq, Copy)]
pub enum DeviceType {
    Gamepad,
    Keyboard,
    Mouse,
    Other,
}

#[derive(Clone)]
pub enum PadButton {
    Left,
    Right,
    Up,
    Down,
    ABtn,
    BBtn,
    XBtn,
    YBtn,
    StartBtn,
    SelectBtn,

    AKey,
    RKey,
    XKey,
    ZKey,

    RightClick,
}



// fastrand::u64(1..);
type DeviceID = NonZeroU64;
type UserID = u64;

// Maybe merge into compute_device_hash_dev
fn compute_device_hash(unique: String, input_id: InputId, name: String) -> DeviceHash {
    use std::collections::hash_map::DefaultHasher;
    let mut hasher = DefaultHasher::new();
    (unique, input_id, name).hash(&mut hasher);
    hasher.finish()
}

fn compute_device_hash_dev(dev: &Device) -> DeviceHash {
    compute_device_hash(
        dev.unique_name().unwrap_or("UNKNOWN").to_string(), 
        dev.input_id(), 
        dev.name().unwrap_or("UNKNOWN").to_string()
    )
}

pub struct DeviceRefrence {
    pub device_id: DeviceID,
    pub user_id: UserID,

    is_alive: bool
}

impl DeviceRefrence {
    pub fn new(state: &mut InputStateInner, dev_path: PathBuf) -> Result<Self, ()> {
        let out = Self::new_nofix(state, dev_path);
        state.fix_users();
        out
    }

    pub fn new_nofix(state: &mut InputStateInner, dev_path: PathBuf) -> Result<Self, ()> {
        let Some(dev_obj) = state.devices.get(&dev_path) else {return Err(())};

        Ok(Self::new_orphan_nofix(state, dev_obj.device_id, dev_obj.fancyname(), dev_obj.hash))
    }

    pub fn new_orphan_nofix(state: &mut InputStateInner, device_id: Option<DeviceID>, name: String, hash: DeviceHash) -> Self {
        let device_id = device_id.unwrap_or_else(|| NonZeroU64::new(fastrand::u64(1..)).unwrap());

        let target = state.targets.entry(device_id).or_insert_with(|| {
            TargetDevice {
                users: HashMap::new(),
                target_name: name,
                target_hash: hash,
                device_id: device_id,
                grabbed: false
            }
        });

        let user_id = fastrand::u64(..);

        target.users.insert(user_id, (false, None, None));

        Self { device_id: device_id, user_id: user_id, is_alive: true }
    }

    pub fn set_input_callback(&mut self, state: &mut InputStateInner, callback: Option<Box<dyn FnMut(Vec<InputEvent>) + Send>>) {
        let Some(tar) = state.targets.get_mut(&self.device_id) else {return;};
        let Some(user) = tar.users.get_mut(&self.user_id) else {return;};

        user.1 = callback;
    }

    pub fn set_dev_change_callback(&mut self, state: &mut InputStateInner, callback: Option<Box<dyn FnMut(PathBuf, bool) + Send>>) {
        let Some(tar) = state.targets.get_mut(&self.device_id) else {return;};
        let Some(user) = tar.users.get_mut(&self.user_id) else {return;};

        user.2 = callback;
    }

    pub fn set_grabbed(&mut self, state: &mut InputStateInner, grabbed: bool) {
        self.set_grabbed_nofix(state, grabbed);
        state.fix_users();
    }
    
    pub fn set_grabbed_nofix(&mut self, state: &mut InputStateInner, grabbed: bool) {
        let Some(tar) = state.targets.get_mut(&self.device_id) else {return;};
        let Some(user) = tar.users.get_mut(&self.user_id) else {return;};

        user.0 = grabbed;
    }

    pub fn pre_drop(&mut self, state: &mut InputStateInner) {
        self.pre_drop_nofix(state);
        state.fix_users();
    }

    pub fn pre_drop_nofix(&mut self, state: &mut InputStateInner) {
        state.targets.get_mut(&self.device_id).and_then(|target| target.users.remove(&self.user_id));
        
        self.is_alive = false;
    }
}

impl Drop for DeviceRefrence {
    fn drop(&mut self) {
        // TODO figure out a better way to do this while not causing a locking issue. 
        // Maybe keep a copy of the ARC so we *can* drop properly, and log this before we maybe hang due to lock contention?
        // Maybe check if the lock is held, and if so, panic instead of hanging?
        if self.is_alive {eprintln!("Attempt to drop DeviceRefrence before calling pre_drop! May be caused by shutdown.");}
    }
}

pub struct InternalDevice {
    pub hash: DeviceHash,
    pub device_id: Option<DeviceID>,

    pub path: PathBuf,
    pub device: Device,

    has_button_held: bool,
    latest_gui_pad: Option<PadButton>,
}

impl InternalDevice {
    fn new(path: PathBuf, dev: Device) -> Self {
        let hash = compute_device_hash_dev(&dev);
        Self {
            hash,
            device_id: None,
            path,
            device: dev,
            has_button_held: false,
            latest_gui_pad: None,
        }
    }

    pub fn name(&self) -> String {
        self.device.name().unwrap_or_else(|| "").to_string()
    }
    
    pub fn device_type(&self) -> DeviceType {
        let device_type = match self.device.supported_keys() {
            Some(keys) => {
                if keys.contains(KeyCode::BTN_SOUTH) {
                    DeviceType::Gamepad
                } else if keys.contains(KeyCode::BTN_LEFT) {
                    DeviceType::Mouse
                } else if keys.contains(KeyCode::KEY_SPACE) {
                    DeviceType::Keyboard
                } else {
                    DeviceType::Other
                }
            }
            None => DeviceType::Other,
        };
        device_type
    }

    pub fn emoji(&self) -> String {
        match self.device_type() {
            DeviceType::Gamepad => "🎮",
            DeviceType::Keyboard => "🖮",
            DeviceType::Mouse => "🖱",
            DeviceType::Other => "",
        }.to_string()
    }

    pub fn fancyname(&self) -> String {
        let name = self.name();
        let name_str = name.as_str();


        match self.device.input_id().vendor() {
            0x045e => "Xbox Controller",
            0x054c => "PS Controller",
            0x057e => "NT Pro Controller",
            0x28de => "Steam Input",
            _ => name_str,
        }.to_string()
    }

    pub fn path(&self) -> &str {
        self.path.to_str().unwrap_or_default()
    }

    pub fn label(&self) -> String {
        let emoji = self.emoji();
        let fancyname = self.fancyname();
        let path_id = self.path().trim_start_matches("/dev/input/event");
        format!(
            "{} {} ({})",
            emoji,
            fancyname,
            path_id
        )
    }
    
    pub fn enabled(&self, filter: &PadFilterType) -> bool {
        let vendor = self.device.input_id().vendor();
        match filter {
            PadFilterType::All => true,
            PadFilterType::NoSteamInput => vendor != 0x28de,
            PadFilterType::OnlySteamInput => vendor == 0x28de,
        }
    }

    pub fn gui_poll(&mut self, events: Vec<InputEvent>) {
        let mut btn: Option<PadButton> = None;

        for event in events {
            let summary = event.destructure();

            match summary {
                EventSummary::Key(_, _, 1) => {
                    self.has_button_held = true;
                }
                EventSummary::Key(_, _, 0) => {
                    self.has_button_held = false;
                }
                _ => {}
            }

            btn = match summary {
                EventSummary::Key(_, KeyCode::BTN_SOUTH, 1) => Some(PadButton::ABtn),
                EventSummary::Key(_, KeyCode::BTN_EAST, 1) => Some(PadButton::BBtn),
                EventSummary::Key(_, KeyCode::BTN_NORTH, 1) => Some(PadButton::XBtn),
                EventSummary::Key(_, KeyCode::BTN_WEST, 1) => Some(PadButton::YBtn),
                EventSummary::Key(_, KeyCode::BTN_START, 1) => Some(PadButton::StartBtn),
                EventSummary::Key(_, KeyCode::BTN_SELECT, 1) => Some(PadButton::SelectBtn),
                EventSummary::AbsoluteAxis(_, AbsoluteAxisCode::ABS_HAT0X, -1) => {
                    Some(PadButton::Left)
                }
                EventSummary::AbsoluteAxis(_, AbsoluteAxisCode::ABS_HAT0X, 1) => {
                    Some(PadButton::Right)
                }
                EventSummary::AbsoluteAxis(_, AbsoluteAxisCode::ABS_HAT0Y, -1) => {
                    Some(PadButton::Up)
                }
                EventSummary::AbsoluteAxis(_, AbsoluteAxisCode::ABS_HAT0Y, 1) => {
                    Some(PadButton::Down)
                }
                //keyboard
                EventSummary::Key(_, KeyCode::KEY_A, 1) => Some(PadButton::AKey),
                EventSummary::Key(_, KeyCode::KEY_R, 1) => Some(PadButton::RKey),
                EventSummary::Key(_, KeyCode::KEY_X, 1) => Some(PadButton::XKey),
                EventSummary::Key(_, KeyCode::KEY_Z, 1) => Some(PadButton::ZKey),
                //mouse
                EventSummary::Key(_, KeyCode::BTN_RIGHT, 1) => Some(PadButton::RightClick),
                _ => btn,
            };
        }

        self.latest_gui_pad = btn;
    }

    pub fn has_button_held(&self) -> bool {return self.has_button_held;}

    pub fn latest_gui_pad(&self) -> Option<PadButton> {return self.latest_gui_pad.clone();}
}


pub struct TargetDevice {
    // Bool of grabbed, then input events we have to handle
    pub users: HashMap<UserID, (bool, Option<Box<dyn FnMut(Vec<InputEvent>) + Send>>, Option<Box<dyn FnMut(PathBuf, bool) + Send>>)>,

    pub target_name: String, // Only used for display
    pub target_hash: DeviceHash,

    pub device_id: DeviceID,

    grabbed: bool, // Last status
}

pub struct InputStateInner {
    pub devices: HashMap<PathBuf, InternalDevice>,
    pub targets: HashMap<DeviceID, TargetDevice>,
    // pub users: HashMap<UserID, DeviceID>, // Maybe remove, just a convience for now.
    pub ctx: egui::Context,
}

impl InputStateInner {
    fn new(ctx: egui::Context) -> Self {
        Self {
            devices: HashMap::new(),
            targets: HashMap::new(),
            ctx,
        }
    }

    fn fetch_and_dispatch_events(&mut self, path: PathBuf, ctx: &egui::Context) -> Result<(), ()> {
        let dev = self.devices.get_mut(&path).ok_or(())?; // Just ignore for now.

        let events = 
            dev.device.fetch_events()
            .and_then(|i| Ok(i.collect::<Vec<InputEvent>>()))
            .map_err(|_| ())?; // Just ignore for now.

        if events.is_empty() {return Ok(());}

        if let Some(device_id) = dev.device_id {
            if let Some(target) = self.targets.get_mut(&device_id) {
                for user in target.users.values_mut() {
                    if let Some(cb) = &mut user.1 {cb(events.clone())};
                }
            }
        }

        dev.gui_poll(events);

        Ok(())
    }


    pub fn fix_users(&mut self) {
        let bound_targets = self.devices.values().filter_map(|dev| dev.device_id).collect::<Vec<DeviceID>>();

        self.targets.retain(|_device_id, target| { !target.users.is_empty() });

        for dev in self.devices.values_mut() {
            if let Some(device_id) = dev.device_id {
                // Demote a device if users disapear.
                if !self.targets.keys().any(|x| *x == device_id) {
                    dev.device_id = None
                }
            }
            
            if dev.device_id.is_none() {
                // Find someone to promote to this device.
                // loop over targets and find one matching our settings that is not already in bound_targets.contains

                for target in self.targets.iter_mut() {
                    if !bound_targets.contains(target.0) && target.1.target_hash == dev.hash {
                        dev.device_id = Some(*target.0);
                        // TODO call to promote function for this device ID.
                        target.1.users.iter_mut().for_each(|u| {
                            if let Some(cb) = &mut u.1.2 {
                                cb(dev.path.clone(), true);
                            }
                        });
                        break;
                    }
                }
            }

            // Update device grab status here
            if let Some(device_id) = dev.device_id {
                if let Some(target) = self.targets.get(&device_id) {
                    let _ = if target.grabbed {dev.device.grab()} else {dev.device.ungrab()}; // TODO ADD BACK
                }
            }
        }

        for target in self.targets.values_mut() {
            let users_holding_grab = target.users.values().any(|g| g.0);
            if users_holding_grab != target.grabbed {
                //TODO grab here
                target.grabbed = users_holding_grab;

                for dev in self.devices.values_mut() {
                    let _ = if target.grabbed {dev.device.grab()} else {dev.device.ungrab()}; // TODO ADD BACK
                }

                // Update connected devices
            }
        } 
        // Not sure if I should do "grab / regrab logic here, but I think so". We just make fix_users called every time we grab, ungrab, new dev, rm dev, new user, rm user.
    }

    pub fn add_device_obj(&mut self, path: PathBuf, dev: Device) {
        self.add_device_obj_nofix(path, dev);
        self.fix_users();
    }
    pub fn add_device_obj_nofix(&mut self, path: PathBuf, dev: Device) {
        self.devices.insert(path.clone(), InternalDevice::new(path, dev));
    }

    pub fn remove_device_obj(&mut self, path: &PathBuf) {
        self.remove_device_obj_nofix(path);
        self.fix_users();
    }
    pub fn remove_device_obj_nofix(&mut self, path: &PathBuf) {
        let dev = self.devices.remove(path);

        if 
            let Some(dev) = dev && 
            let Some(dev_id) = dev.device_id &&
            let Some(target) = self.targets.get_mut(&dev_id)
        {
            target.users.iter_mut().for_each(|u| {
                if let Some(cb) = &mut u.1.2 {
                    cb(path.clone(), false);
                }
            });
        }
    }
}


pub struct InputState {
    inner: Arc<Mutex<InputStateInner>>,
    shutdown_tx: Sender<()>,
    _thread: JoinHandle<()>,
}

impl InputState {
    pub fn new(ctx: egui::Context) -> Result<Self, Box<dyn std::error::Error>> {

        let inner = Arc::new(Mutex::new(InputStateInner::new(ctx.clone())));
        let (shutdown_tx, shutdown_rx) = channel();
        let thread_inner = Arc::clone(&inner);

        let thread = thread::spawn(move || {
            // create udev monitor in thread
            let monitor = udev::MonitorBuilder::new().and_then(|b| b.match_subsystem("input")).and_then(|b| b.listen()).unwrap();


            // scan existing devices
            if let Ok(mut guard) = thread_inner.lock() {
                for devpath in evdev::enumerate() {
                    if let Ok(device) = Device::open(&devpath.0) {
                        let _ = device.set_nonblocking(true);
                        guard.add_device_obj_nofix(devpath.0.clone(), device);
                    }
                }
                guard.fix_users();
            }

            // run loop
            loop {
                // check shutdown
                match shutdown_rx.try_recv() {
                    Ok(()) | Err(std::sync::mpsc::TryRecvError::Disconnected) => break,
                    Err(std::sync::mpsc::TryRecvError::Empty) => {}
                }

                
                // non-blocking iterate
                while let Some(event) = monitor.iter().next() {
                    match event.action().and_then(|a| a.to_str()) {
                        Some("add") => {
                            if let Some(devnode) = event.devnode() {
                                if let Ok(device) = Device::open(devnode) {
                                    let _ = device.set_nonblocking(true);
                                    if let Ok(mut guard) = thread_inner.lock() {
                                        guard.add_device_obj(devnode.to_path_buf(), device);
                                    }
                                }
                            }
                        }
                        Some("remove") => {
                            if let Some(devnode) = event.devnode() {
                                if let Ok(mut guard) = thread_inner.lock() {
                                    guard.remove_device_obj(&devnode.to_path_buf());
                                }
                            }
                        }
                        _ => {}
                    }
                }

                // snapshot monitor fd if present
                let monitor_fd = monitor.as_fd();

                let mut fds: Vec<(PathBuf, OwnedFd)> = Vec::new();
                let mut poll_fds: Vec<PollFd> = Vec::new();

                if let Ok(guard) = thread_inner.lock() {
                    // Build FD snapshot
                    for dev in guard.devices.iter() {
                        let fd = dup(dev.1.device.as_fd()).unwrap();
                        fds.push((dev.0.clone(), fd));
                    }

                    // Build poll vector: optional monitor fd first
                    poll_fds.push(PollFd::new(monitor_fd, PollFlags::POLLIN));

                    for &(_, ref fd) in fds.iter() {
                        poll_fds.push(PollFd::new(fd.as_fd(), PollFlags::POLLIN));
                    }

                    // Blocking poll with timeout
                }

                // Poll importantly done without keeping the guard lock so we dont block main thread.
                let _ = poll(&mut poll_fds, PollTimeout::try_from(POLL_TIMEOUT_MS as i32).unwrap()).unwrap();


                if let Ok(mut guard) = thread_inner.lock() {

                    // For devices that fired, map index -> path and dispatch
                    for (idx, poll_fd) in poll_fds.iter().enumerate().skip(1) { // Skip monitor FD
                        if let Some(revents) = poll_fd.revents() {
                            if revents.intersects(PollFlags::POLLIN | PollFlags::POLLERR | PollFlags::POLLHUP) {
                                // map idx-device_start_idx -> fds index
                                let fds_idx = idx - 1; // Skip monitor FD
                                if let Some((path, _fd)) = fds.get(fds_idx) {
                                    // find device entry by path and dispatch
                                    
                                    if guard.fetch_and_dispatch_events(path.clone(), &ctx).is_err() {
                                        // Todo remove device because it gon :(
                                    }
                                }
                            }
                        }
                    }

                    // TODO REMOVE THIS SHOULD NOT BE NEEDED, LEFT IN CASE I FORGOT SOMEWHERE ELSE.
                    guard.fix_users();
                }
            }
        });

        Ok(Self {
            inner,
            shutdown_tx,
            _thread: thread,
        })
    }

    pub fn inner(&mut self) -> std::sync::MutexGuard<'_, InputStateInner> {
        self.inner.lock().unwrap()
    }

    pub fn clone_inner(&mut self) -> Arc<Mutex<InputStateInner>> {
        self.inner.clone()
    }
}