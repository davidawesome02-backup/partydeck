// use std::{
//     collections::{HashMap, VecDeque, hash_map::DefaultHasher}, hash::{Hash, Hasher}, os::fd::{AsFd, OwnedFd}, path::PathBuf, sync::{
//         Arc, Mutex, atomic::AtomicI64, mpsc::{Receiver, Sender, channel},
//     }, thread::{self, JoinHandle}
// };

// use evdev::{Device, InputEvent, KeyCode, EventSummary, AbsoluteAxisCode};
// use nix::{poll::{PollFd, PollFlags, PollTimeout, poll}, unistd::dup};

// use crate::app::PadFilterType;
// use eframe::egui;

// const POLL_TIMEOUT_MS: i32 = 5000;


// #[derive(Clone, PartialEq, Copy)]
// pub enum DeviceType {
//     Gamepad,
//     Keyboard,
//     Mouse,
//     Other,
// }

// #[derive(Clone)]
// pub enum PadButton {
//     Left,
//     Right,
//     Up,
//     Down,
//     ABtn,
//     BBtn,
//     XBtn,
//     YBtn,
//     StartBtn,
//     SelectBtn,

//     AKey,
//     RKey,
//     XKey,
//     ZKey,

//     RightClick,
// }


// // ---------------------- Device snapshot & hashing ----------------------------

// type DeviceHash = u64;

// /// Accept either a textual input_id (when callers pass strings) or a Debug-format of InputId
// fn compute_device_hash_dev(dev: &mut Device) -> DeviceHash {
//     // Convert input_id into a stable string representation to hash it together with name/unique
//     let input_id_s = format!("{:?}", dev.input_id());
//     compute_device_hash(dev.unique_name(), &input_id_s, dev.name())
// }

// fn compute_device_hash(unique: Option<&str>, input_id: &str, name: Option<&str>) -> DeviceHash {
//     let mut hasher = DefaultHasher::new();
//     (unique, input_id, name).hash(&mut hasher);
//     hasher.finish()
// }

// // ---------------------- Lease types (user-facing) ----------------------------

// /// Internal lease state (shared between threads)
// pub struct InnerDeviceLease {
//     /// Path of the assigned device when assigned, or None when orphaned
//     pub device_path: Mutex<Option<PathBuf>>,
//     /// Device identity/hash used for matching
//     pub device_hash: DeviceHash,
//     /// Optional little user payload that the caller can store (for this demo a u64)
//     pub user_data: Mutex<egui::ViewportId>,

//     pub input_events_data: Mutex<VecDeque<InputEvent>>,
// }

// #[derive(Clone)]
// pub struct DeviceLease {
//     inner: Arc<InnerDeviceLease>,
// }

// impl DeviceLease {
//     /// Create a lease tied to a hash (optionally with a concrete path already assigned)
//     pub fn new_with_path(hash: DeviceHash, path: Option<PathBuf>, user_data: egui::ViewportId) -> Self {
//         let inner = Arc::new(InnerDeviceLease {
//             device_path: Mutex::new(path),
//             device_hash: hash,
//             user_data: Mutex::new(user_data),
//             input_events_data: Mutex::new(VecDeque::new())
//         });
//         Self { inner }
//     }

//     /// Create an orphaned lease (no device assigned yet)
//     pub fn new_orphan(hash: DeviceHash, user_data: egui::ViewportId) -> Self {
//         Self::new_with_path(hash, None, user_data)
//     }

//     /// Read the current device path (None if not assigned)
//     pub fn device_path(&self) -> Option<PathBuf> {
//         self.inner.device_path.lock().unwrap().clone()
//     }

//     /// Set the device path (usually called by the input thread)
//     fn set_device_path(&self, p: Option<PathBuf>) {
//         *self.inner.device_path.lock().unwrap() = p;
//     }

//     /// Update user_data
//     pub fn set_user_data(&self, v: egui::ViewportId) {
//         *self.inner.user_data.lock().unwrap() = v;
//     }

//     /// Read user_data
//     pub fn user_data(&self) -> egui::ViewportId {
//         *self.inner.user_data.lock().unwrap()
//     }

//     /// Device hash associated with this lease
//     pub fn device_hash(&self) -> DeviceHash {
//         self.inner.device_hash
//     }

//     pub fn inner(&mut self) -> Arc<InnerDeviceLease> {
//         self.inner.clone()
//     }
// }

// // ---------------------- Input handler (runs on the input thread) --------------

// struct InputDeviceInternal {
//     device: Arc<Mutex<Device>>,
//     hash: DeviceHash,
//     path: PathBuf,
//     has_button_held: bool,
//     latest_gui_pad: Option<PadButton>,
// }
// impl InputDeviceInternal {
//     fn name(&self) -> String {
//         let dev = self.device.lock().unwrap();
//         dev.name().unwrap_or_else(|| "").to_string()
//     }
//     fn device_type(&mut self) -> DeviceType {
//         let dev = self.device.lock().unwrap();
//         let device_type = match dev.supported_keys() {
//             Some(keys) => {
//                 if keys.contains(KeyCode::BTN_SOUTH) {
//                     DeviceType::Gamepad
//                 } else if keys.contains(KeyCode::BTN_LEFT) {
//                     DeviceType::Mouse
//                 } else if keys.contains(KeyCode::KEY_SPACE) {
//                     DeviceType::Keyboard
//                 } else {
//                     DeviceType::Other
//                 }
//             }
//             None => DeviceType::Other,
//         };
//         device_type
//     }
//     fn emoji(&mut self) -> String {
//         match self.device_type() {
//             DeviceType::Gamepad => "🎮",
//             DeviceType::Keyboard => "🖮",
//             DeviceType::Mouse => "🖱",
//             DeviceType::Other => "",
//         }.to_string()
//     }
//     fn fancyname(&self) -> String {
//         let name = self.name();
//         let name_str = name.as_str();

//         let dev = self.device.lock().unwrap();

//         match dev.input_id().vendor() {
//             0x045e => "Xbox Controller",
//             0x054c => "PS Controller",
//             0x057e => "NT Pro Controller",
//             0x28de => "Steam Input",
//             _ => name_str,
//         }.to_string()
//     }
//     fn path(&self) -> &str {
//         self.path.to_str().unwrap_or_default()
//     }
//     pub fn label(&mut self) -> String {
//         let emoji = self.emoji();
//         let fancyname = self.fancyname();
//         let path_id = self.path().trim_start_matches("/dev/input/event");
//         format!(
//             "{} {} ({})",
//             emoji,
//             fancyname,
//             path_id
//         )
//     }
//     pub fn enabled(&self, filter: &PadFilterType) -> bool {
//         let dev = self.device.lock().unwrap();
//         match filter {
//             PadFilterType::All => true,
//             PadFilterType::NoSteamInput => dev.input_id().vendor() != 0x28de,
//             PadFilterType::OnlySteamInput => dev.input_id().vendor() == 0x28de,
//         }
//     }

//     pub fn gui_poll(&mut self, events: Vec<InputEvent>) {
//         let mut btn: Option<PadButton> = None;

//         for event in events {
//             let summary = event.destructure();

//             match summary {
//                 EventSummary::Key(_, _, 1) => {
//                     self.has_button_held = true;
//                 }
//                 EventSummary::Key(_, _, 0) => {
//                     self.has_button_held = false;
//                 }
//                 _ => {}
//             }

//             btn = match summary {
//                 EventSummary::Key(_, KeyCode::BTN_SOUTH, 1) => Some(PadButton::ABtn),
//                 EventSummary::Key(_, KeyCode::BTN_EAST, 1) => Some(PadButton::BBtn),
//                 EventSummary::Key(_, KeyCode::BTN_NORTH, 1) => Some(PadButton::XBtn),
//                 EventSummary::Key(_, KeyCode::BTN_WEST, 1) => Some(PadButton::YBtn),
//                 EventSummary::Key(_, KeyCode::BTN_START, 1) => Some(PadButton::StartBtn),
//                 EventSummary::Key(_, KeyCode::BTN_SELECT, 1) => Some(PadButton::SelectBtn),
//                 EventSummary::AbsoluteAxis(_, AbsoluteAxisCode::ABS_HAT0X, -1) => {
//                     Some(PadButton::Left)
//                 }
//                 EventSummary::AbsoluteAxis(_, AbsoluteAxisCode::ABS_HAT0X, 1) => {
//                     Some(PadButton::Right)
//                 }
//                 EventSummary::AbsoluteAxis(_, AbsoluteAxisCode::ABS_HAT0Y, -1) => {
//                     Some(PadButton::Up)
//                 }
//                 EventSummary::AbsoluteAxis(_, AbsoluteAxisCode::ABS_HAT0Y, 1) => {
//                     Some(PadButton::Down)
//                 }
//                 //keyboard
//                 EventSummary::Key(_, KeyCode::KEY_A, 1) => Some(PadButton::AKey),
//                 EventSummary::Key(_, KeyCode::KEY_R, 1) => Some(PadButton::RKey),
//                 EventSummary::Key(_, KeyCode::KEY_X, 1) => Some(PadButton::XKey),
//                 EventSummary::Key(_, KeyCode::KEY_Z, 1) => Some(PadButton::ZKey),
//                 //mouse
//                 EventSummary::Key(_, KeyCode::BTN_RIGHT, 1) => Some(PadButton::RightClick),
//                 _ => btn,
//             };
//         }

//         self.latest_gui_pad = btn;
//     }

//     pub fn has_button_held(&mut self) -> bool {return self.has_button_held;}

//     pub fn latest_gui_pad(&mut self) -> Option<PadButton> {return self.latest_gui_pad.clone();}

// }

// struct InputHandler {
//     /// owns the real Device objects and manages poll/fetching
//     devices: Arc<Mutex<HashMap<PathBuf, InputDeviceInternal>>>,

//     /// Shared lease list (main thread pushes new leases into this; input thread updates indices)
//     shared_leases: Arc<Mutex<Vec<Arc<InnerDeviceLease>>>>,

//     /// udev monitor socket for hotplug
//     udev_monitor: udev::MonitorSocket,

//     ctx: egui::Context,
// }

// impl InputHandler {
//     fn new(
//         shared_devices: Arc<Mutex<HashMap<PathBuf, InputDeviceInternal>>>,
//         shared_leases: Arc<Mutex<Vec<Arc<InnerDeviceLease>>>>,
//         ctx: egui::Context,
//     ) -> Result<Self, Box<dyn std::error::Error>> {
//         let monitor = udev::MonitorBuilder::new()?
//             .match_subsystem("input")?
//             .listen()?;

//         Ok(Self {
//             devices: shared_devices,
//             shared_leases,
//             udev_monitor: monitor,
//             ctx,
//         })
//     }

//     /// Scan and open existing evdev devices, populate internal state and shared snapshot.
//     fn scan_existing_devices(&mut self) -> Result<(), Box<dyn std::error::Error>> {
//         let mut dev_map = self.devices.lock().unwrap();

//         for devpath in evdev::enumerate() {
//             if let Ok(mut dev) = Device::open(&devpath.0) {
//                 dev.set_nonblocking(true)?;

//                 let path = devpath.0.clone();
//                 let hash = compute_device_hash_dev(&mut dev);
//                 dev_map.insert(
//                     path.clone(),
//                     InputDeviceInternal {
//                         hash,
//                         device: Arc::new(Mutex::new(dev)),
//                         path,
//                         has_button_held: false,
//                         latest_gui_pad: None
//                     },
//                 );
//             }
//         }
//         Ok(())
//     }

//     /// Called when udev sends an add event
//     fn handle_device_add(&mut self, dev: Device, path: PathBuf) {
//         let mut dev = dev;
//         let hash = compute_device_hash_dev(&mut dev);

//         let arc_dev = Arc::new(Mutex::new(dev));
//         {
//             let mut dev_map = self.devices.lock().unwrap();
//             dev_map.insert(
//                 path.clone(),
//                 InputDeviceInternal {
//                     hash,
//                     device: arc_dev,
//                     path: path.clone(),
//                     has_button_held: false,
//                     latest_gui_pad: None
//                 },
//             );
//         }

//         // Assign orphaned leases that match this device hash
//         let leases = self.shared_leases.lock().unwrap();
//         for lease_arc in leases.iter() {
//             if lease_arc.device_hash == hash {
//                 let mut guard = lease_arc.device_path.lock().unwrap();
//                 if guard.is_none() {
//                     *guard = Some(path.clone());
//                 }
//             }
//         }
//     }

//     /// Called when a device is removed
//     fn handle_device_remove(&mut self, removed_path: &PathBuf) {
//         // Remove entry and capture its hash
//         let removed_hash_opt = {
//             let mut dev_map = self.devices.lock().unwrap();
//             dev_map.remove(removed_path).map(|d| d.hash)
//         };

//         let removed_hash = match removed_hash_opt {
//             Some(h) => h,
//             None => return,
//         };

//         // Find replacement with same hash (pick any remaining path)
//         let replacement_path_opt = {
//             let dev_map = self.devices.lock().unwrap();
//             dev_map
//                 .iter()
//                 .find_map(|(p, d)| if d.hash == removed_hash { Some(p.clone()) } else { None })
//         };

//         // Update leases:
//         let leases = self.shared_leases.lock().unwrap();
//         for lease_arc in leases.iter() {
//             if lease_arc.device_hash == removed_hash {
//                 let mut idx_guard = lease_arc.device_path.lock().unwrap();
//                 if let Some(cur_path) = idx_guard.clone() {
//                     if cur_path == *removed_path {
//                         // If a replacement exists, move to it. Otherwise set to None (orphan)
//                         *idx_guard = replacement_path_opt.clone();
//                     }
//                 }
//             }
//         }
//     }

//     /// Handle udev events
//     fn handle_hotplug_event(&mut self) -> Result<(), Box<dyn std::error::Error>> {
//         while let Some(event) = self.udev_monitor.iter().next() {
//             match event.action().and_then(|a| a.to_str()) {
//                 Some("add") => {
//                     if let Some(devnode) = event.devnode() {
//                         if let Ok(device) = Device::open(devnode) {
//                             device.set_nonblocking(true)?;
//                             self.handle_device_add(device, devnode.to_path_buf());
//                         }
//                     }
//                 }
//                 Some("remove") => {
//                     if let Some(devnode) = event.devnode() {
//                         self.handle_device_remove(&devnode.to_path_buf());
//                     }
//                 }
//                 _ => {}
//             }
//         }
//         Ok(())
//     }

//     /// Poll devices and monitor, process events and hotplug
//     fn handle_device_events(&mut self) -> Result<(), Box<dyn std::error::Error>> {
//         // Build a short snapshot of (path, raw_fd) while holding locks briefly.
//         // We will drop locks before calling poll to avoid long-held locks.
//         let mut fds: Vec<(PathBuf, OwnedFd)> = Vec::new();

//         // snapshot device fds
//         {
//             let dev_map = self.devices.lock().unwrap();
//             for (path, d) in dev_map.iter() {
//                 // Lock the device only briefly to get a raw fd
//                 let dev_guard = d.device.lock().unwrap();
//                 let cloned_fd = dup(dev_guard.as_fd())?;
//                 fds.push((path.clone(), cloned_fd));
//             }
//         }

//         // Build pollfd vector: monitor first, then devices
//         let mut poll_fds = Vec::with_capacity(1 + fds.len());
//         let monitor_fd = dup(self.udev_monitor)?;
//         poll_fds.push(PollFd::new(monitor_fd.as_fd(), PollFlags::POLLIN));

//         for &(_, fd) in fds.iter() {
//             poll_fds.push(PollFd::new(fd.as_fd(), PollFlags::POLLIN));
//         }

//         // blocking poll with timeout (we already dropped the device map lock)
//         let _ = poll(&mut poll_fds, PollTimeout::from(POLL_TIMEOUT_MS as i32))?;
//         // let _ = poll(&mut poll_fds, PollTimeout::from(std::time::Duration::from_millis(POLL_TIMEOUT_MS as u64)))?;

//         // check udev monitor
//         if let Some(revents) = poll_fds.get(0).and_then(|p| p.revents()) {
//             if revents.contains(PollFlags::POLLIN) {
//                 self.handle_hotplug_event()?;
//             }
//         }

//         // check device events (poll_fds index 1 corresponds to fds[0], etc.)
//         for (i, poll_fd) in poll_fds.iter().enumerate().skip(1) {
//             if let Some(revents) = poll_fd.revents() {
//                 if revents.intersects(PollFlags::POLLIN | PollFlags::POLLHUP | PollFlags::POLLERR) {
//                     // map i-1 -> path
//                     if let Some((path, _fd)) = fds.get(i - 1) {
//                         self.process_device_events(path.clone());
//                     }
//                 }
//             }
//         }

//         Ok(())
//     }

//     /// Fetch events for a device and notify leases if applicable (simple placeholder)
//     fn process_device_events(&mut self, device_path: PathBuf) {
//         // Get the device Arc if present
//         let should_remove = {
//             // lock devices map briefly to get the Arc
//             let mut dev_map = self.devices.lock().unwrap();
//             let dev_internal_opt = dev_map.get_mut(&device_path);


//             match dev_internal_opt {
//                 Some(dev_internal) => {
//                     let dev_arc = Arc::clone(&dev_internal.device);

//                     let leases_lock = self.shared_leases.lock().unwrap();
//                     let active_leases =
//                         leases_lock.iter().filter(|dev_in_lease| {
//                             dev_in_lease.device_hash == dev_internal.hash
//                         });

//                     // lock the device to fetch events
//                     let mut dev = dev_arc.lock().unwrap();
//                     match dev.fetch_events() {
//                         Ok(events) => {

//                             let input_events = events.collect::<Vec<InputEvent>>();
//                             for lease in active_leases {
//                                 lease.input_events_data.lock().unwrap().extend(input_events.clone());
//                                 self.ctx.request_repaint_once_for(*lease.user_data.lock().unwrap());
//                             }

//                             dev_internal.gui_poll(input_events);

//                             false
//                         }
//                         Err(_) => true,
//                     }
//                 }
//                 None => false, // device already gone
//             }
//         };

//         if should_remove {
//             // remove by path (this will update leases)
//             self.handle_device_remove(&device_path);
//         }
//     }

//     fn run(mut self, receiver: Receiver<()>) -> Result<(), Box<dyn std::error::Error>> {
//         self.scan_existing_devices()?;

//         loop {
//             // Check for shutdown without blocking
//             match receiver.try_recv() {
//                 Ok(()) | Err(std::sync::mpsc::TryRecvError::Disconnected) => break,
//                 Err(std::sync::mpsc::TryRecvError::Empty) => {}
//             }

//             // Poll devices & udev
//             self.handle_device_events()?;
//         }

//         Ok(())
//     }
// }

// // ---------------------- Thread-safe public wrapper --------------------------

// pub struct InputState {
//     /// devices map (path -> internal device entry)
//     devices: Arc<Mutex<HashMap<PathBuf, InputDeviceInternal>>>,

//     /// quick lookup from hash -> path for fast matching
//     devices_by_hash: Arc<Mutex<HashMap<DeviceHash, PathBuf>>>,

//     /// shared leases list (main thread creates leases, input thread updates indices)
//     leases: Arc<Mutex<Vec<Arc<InnerDeviceLease>>>>,

//     /// channel to request shutdown of the input thread
//     shutdown_tx: Sender<()>,

//     /// handle for the input thread
//     _thread: JoinHandle<()>,
// }

// impl InputState {
//     pub fn new(ctx: egui::Context) -> Result<Self, Box<dyn std::error::Error>> {
//         let devices = Arc::new(Mutex::new(HashMap::new()));
//         let devices_by_hash = Arc::new(Mutex::new(HashMap::new()));
//         let leases = Arc::new(Mutex::new(Vec::new()));
//         let (shutdown_tx, shutdown_rx) = channel();

//         let devices_clone = Arc::clone(&devices);
//         let leases_clone = Arc::clone(&leases);

//         // Note: InputHandler will mutate devices HashMap; to keep devices_by_hash in sync we
//         // update devices_by_hash from the main thread whenever we create leases by path or when needed.
//         // Alternatively you can pass devices_by_hash into InputHandler too and update there.
//         let thread = thread::spawn(move || {
//             let handler = InputHandler::new(devices_clone, leases_clone, ctx).unwrap();
//             // The handler will manage devices stored under the shared HashMap.
//             handler.run(shutdown_rx).unwrap()
//         });

//         Ok(Self {
//             devices,
//             devices_by_hash,
//             leases,
//             shutdown_tx,
//             _thread: thread,
//         })
//     }

//     /// Find a device PathBuf from given device info (unique/input_id/name), returns Option<PathBuf>
//     /// The inputs should be the same values you would get from device.unique_name(), device.input_id(), device.name()
//     pub fn get_path_from_info(
//         &self,
//         unique_name: Option<&str>,
//         input_id: &str,
//         name: Option<&str>,
//     ) -> Option<PathBuf> {
//         let hash = compute_device_hash(unique_name, input_id, name);
//         // Fast path: lookup in devices_by_hash
//         if let Some(p) = self.devices_by_hash.lock().unwrap().get(&hash) {
//             return Some(p.clone());
//         }

//         // Fallback: scan devices map (rare)
//         let dev_map = self.devices.lock().unwrap();
//         dev_map
//             .iter()
//             .find_map(|(path, entry)| if entry.hash == hash { Some(path.clone()) } else { None })
//     }

//     /// Create a lease by a known path.
//     /// This creates a DeviceLease, registers its Arc with the shared lease list (so the input thread can update it),
//     /// and returns the user-facing DeviceLease immediately.
//     pub fn create_lease_by_path(&self, path: PathBuf, user_data: egui::ViewportId) -> Option<DeviceLease> {
//         let devices = self.devices.lock().unwrap();
//         let entry = devices.get(&path)?;
//         let hash = entry.hash;
//         let lease = DeviceLease::new_with_path(hash, Some(path.clone()), user_data);

//         // push the Arc into the shared lease list so the input thread can see & update it
//         self.leases
//             .lock()
//             .unwrap()
//             .push(Arc::clone(&lease.inner));

//         // maintain devices_by_hash mapping for faster lookups later
//         self.devices_by_hash
//             .lock()
//             .unwrap()
//             .insert(hash, path.clone());

//         Some(lease)
//     }

//     /// Create a lease from device info (unique, input_id, name). If matching device exists, assign its path.
//     /// Otherwise create an orphan lease (device_path None) that will be assigned later when a matching device appears.
//     pub fn create_lease_from_info(
//         &self,
//         unique_name: Option<&str>,
//         input_id: &str,
//         name: Option<&str>,
//         user_data: egui::ViewportId,
//     ) -> DeviceLease {
//         let hash = compute_device_hash(unique_name, input_id, name);
//         // Try fast lookup
//         let path_opt = {
//             let by_hash = self.devices_by_hash.lock().unwrap();
//             by_hash.get(&hash).cloned()
//         }
//         .or_else(|| {
//             let dev_map = self.devices.lock().unwrap();
//             dev_map
//                 .iter()
//                 .find_map(|(p, e)| if e.hash == hash { Some(p.clone()) } else { None })
//         });

//         let lease = match path_opt.clone() {
//             Some(p) => DeviceLease::new_with_path(hash, Some(p.clone()), user_data),
//             None => DeviceLease::new_orphan(hash, user_data),
//         };

//         // register with shared lease list
//         self.leases
//             .lock()
//             .unwrap()
//             .push(Arc::clone(&lease.inner));

//         // if we found a path, make sure devices_by_hash is populated
//         if let Some(p) = path_opt {
//             self.devices_by_hash.lock().unwrap().insert(hash, p);
//         }

//         lease
//     }

//     /// Return an Arc<Mutex<Device>> for the lease if it is assigned to a device.
//     /// The caller can then try_lock() on the Arc's Mutex to avoid blocking long.
//     pub fn get_device_for_lease(&self, lease: &DeviceLease) -> Option<Arc<Mutex<Device>>> {
//         let path_opt = lease.device_path();
//         if let Some(path) = path_opt {
//             let dev_map = self.devices.lock().unwrap();
//             dev_map.get(&path).map(|d| Arc::clone(&d.device))
//         } else {
//             None
//         }
//     }

//     pub fn devices(&mut self) -> Arc<Mutex<HashMap<PathBuf, InputDeviceInternal>>> {return Arc::clone(&self.devices);}    
//     pub fn leases(&mut self) -> Arc<Mutex<Vec<Arc<InnerDeviceLease>>>> {return Arc::clone(&self.leases);}
    
// }

// impl Drop for InputState {
//     fn drop(&mut self) {
//         // request the input thread to shutdown
//         let _ = self.shutdown_tx.send(());
//     }
// }













// // Wanted version
// // Each device has a single possible sharedLease. Those shared leases may or may not have a device that is related.
// // When a device is removed, the shared lease is moved to a orphaned vec, so it can be put back to the next device that matches 
// // The name, uniquename, etc of an added device. These should NEVER merge any sharedLeases, only move them arround.
// // Multiple shared leases can have hte same target name, uniquename, etc. but should be pushed to distinct devices if we disconnect adn reconnect.
// // For times we want to use the same exact device for multiple refrences, this is why you can have a lease, that points to one of these sharedleases.
// // This allows for multiple profiles to point to a shared single device we want to listen to, or distinct ones with similar charastics.
// // We should also implement many of the extra features implemented above, where this is intended to run on its own thread as done avove, and having many of the same
// // Functions in internalDevice so we can get the fancy name, if buttons are held, etc.
// // we shoud do both from existing devices as well as newly connected devices as above, where orphans are fixed with the fix orphand function after add or remove.
// // On remove, move the device into the orhpan_leases object, on add, just let the fix function find you.
// // we should always increment and keep the leaseids up to date properly so we dont overlap unless expressly requrested.
// // Add sync primitives as needed to make this work.
// // Use the following code as a outline for how this shoud be designed, where the above is a working, but bad API version.
// // Make this work, feeling free to rename variables.

// struct InputState {
//     devices: Arc<Mutex<Vec<Arc<InternalDevice>>>>,
//     orphan_leases: Vec<SharedLease>, // Can be a Hashmap if easier.
//     latest_shared_lease_id: u64, // Increment for each newly created sharedlease, and kept above all passed `new_orphan` ids by doing `next_dev_id = Max(previous_dev_max_id, requested_dev_id)+1`
//     latest_lease_id: u64, // Increment for each newly created lease
// }
// impl InputState {
//     pub fn fix_orphans(&mut self) {
//         // Attempt to find unused devs for each in the orphan list that match the certain device info.
//         // Called on device added, or removed, or after we add a bunch of `new_orphan` calls. `new_orphan`
//     }
// }

// struct InternalDevice {
//     dev: evdev::Device,
//     lease: Option<Arc<SharedLease>>,
// }
// impl InternalDevice {
//     pub fn update_grabbed_status(&mut self) {
//         // Get lease and check if its grabbed. If it is None or grabbed says false, we ungrab. Otherwise we grab.
//     }
// }

// struct SharedLease {
//     shared_lease_id: u64,

//     // Hashmap of lease_id to ViewportID for us to update, a list of inputs, and a boolean of if we requested to grab the device.
//     users_info: HashMap<u64, (ViewportID, VecDeque<InputEvent>, bool)>, // Use length as number of users, if goes to 0, unbind this object from the device, and let rust destory it after all arcs die.

//     // To search for unused.
//     dev_name: String,
//     dev_input_id: evdev::InputId,
//     dev_unique_name: String,

// }
// impl SharedLease {
//     pub fn update_grabbed_status() {
//         // Find the device and issue a `update_grabbed_status` on it.
//     }
//     pub fn get_grabbed_status() -> bool {
//         // Loop over users_info and get the number of "grabbers", if >0 we reqturn true.
//     }

//     pub fn on_drop(&mut self) {
//         // Check if users_info length is 0, then remove from internalDevice and let that removal drop us from the arc.
//     }

//     pub fn on_input(&mut self, evts: Vec<InputEvent>) {
//         // Update the hashmap events, extending by the evts vec. 
//         // Should then call 
//         // self.ctx.request_repaint_once_for(viewportID ); for each  users_info
//     }
// }

// struct DevLease {
//     lease: Arc<SharedLease>,
//     lease_id: u64,
// }

// impl DevLease {
//     pub fn new(dev: Arc<InternalDevice>) -> Self {
//         // Get a device refrence, making an new struct if needed.
//     }
//     pub fn new_orphan(shared_lease_id: Option<u64>, dev_name: String, dev_input_id: evdev::InputId, dev_unique_name: String) -> Self {
//         // Will attempt to place into that shared_lease_id, if all other data matches, if mismatched or None, create new ID for it and bind to unused.
//         // If a device with the same shared_lease_id is already allocated as non-orphan, just merge it with that one. This is just called "new_orphan" because it can 
//         // Create orphans if the lease is not exactly equal to one that already exists.

//     }
//     pub fn grab(&mut self, grabbed: bool) {
//         // Update SharedLease 
//     }
// }

// impl Drop for DevLease {
//     fn drop(&mut self) {
//         // Drop sharedLease HashMap then call its `on_drop` function so we can have it check if it should be destoryed.
//     }
// }












use std::{
    collections::{HashMap, VecDeque}, hash::{Hash, Hasher}, os::fd::{AsFd, BorrowedFd, OwnedFd}, path::PathBuf, sync::{
        Arc, Mutex, atomic::{AtomicU64, Ordering}, mpsc::{Receiver, Sender, channel},
    }, thread::{self, JoinHandle}, time::Duration,
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


fn compute_device_hash(unique: String, input_id: InputId, name: String) -> DeviceHash {
    use std::collections::hash_map::DefaultHasher;
    let mut hasher = DefaultHasher::new();
    (unique, input_id, name).hash(&mut hasher);
    hasher.finish()
}

fn compute_device_hash_dev(dev: &mut Device) -> DeviceHash {
    compute_device_hash(
        dev.unique_name().unwrap_or("UNKNOWN").to_string(), 
        dev.input_id(), 
        dev.name().unwrap_or("UNKNOWN").to_string()
    )
}
fn compute_device_hash_lease(lease: &SharedLease) -> DeviceHash {
    compute_device_hash(
        lease.dev_unique_name.clone(),
        lease.dev_input_id.clone(),
        lease.dev_name.clone(),
    )
}

// ---------------------- SharedLease -----------------

pub struct SharedLease {
    pub id: SharedLeaseId,

    // matching keys (copied for matching)
    pub dev_name: String,
    pub dev_input_id: InputId,
    pub dev_unique_name: String,

    // users_info: lease_id -> (ViewportId, pending events, grabbed_requested)
    pub users_info: HashMap<LeaseId, (egui::ViewportId, VecDeque<InputEvent>, bool)>,

    // convenience: assigned device path if bound
    pub assigned_device_path: Option<PathBuf>,
}

impl SharedLease {
    fn new(
        id: SharedLeaseId,
        dev_name: String,
        dev_input_id: InputId,
        dev_unique_name: String,
    ) -> Self {
        Self {
            id,
            dev_name,
            dev_input_id,
            dev_unique_name,
            users_info: HashMap::new(),
            assigned_device_path: None,
        }
    }

    pub fn add_user(&mut self, lease_id: LeaseId, viewport: egui::ViewportId, grabbed: bool) {
        self.users_info.entry(lease_id).or_insert((viewport, VecDeque::new(), grabbed));
    }

    pub fn remove_user(&mut self, lease_id: LeaseId) {
        self.users_info.remove(&lease_id);
    }

    pub fn set_user_grab(&mut self, lease_id: LeaseId, grabbed: bool) {
        if let Some(v) = self.users_info.get_mut(&lease_id) {
            v.2 = grabbed;
        }
    }

    pub fn get_grabbed_status(&self) -> bool {
        self.users_info.values().any(|(_, _, g)| *g)
    }

    pub fn is_empty(&self) -> bool {
        self.users_info.is_empty()
    }

    /// Called by the input thread when device events arrive; append events for each user and request repaint.
    pub fn on_input(&mut self, events: &Vec<InputEvent>, ctx: &egui::Context) {
        if events.is_empty() { return; }
        for (_, (viewport, q, _)) in self.users_info.iter_mut() {
            q.extend(events.clone());
            ctx.request_repaint_once_for(*viewport);
        }
    }

    /// Pop up to `max` events for a specific lease/user.
    pub fn pop_events_for(&mut self, lease_id: LeaseId, max: usize) -> Vec<InputEvent> {
        if let Some((_, q, _)) = self.users_info.get_mut(&lease_id) {
            let mut out = Vec::new();
            for _ in 0..max {
                if let Some(ev) = q.pop_front() { out.push(ev) } else { break; }
            }
            out
        } else {
            Vec::new()
        }
    }
}

// ---------------------- InternalDevice ------------------------

pub struct InternalDevice {
    pub path: PathBuf,
    pub hash: DeviceHash,
    pub device: Arc<Mutex<Device>>,
    /// optional bound shared lease
    pub lease: Option<Arc<Mutex<SharedLease>>>,
    pub grabbed: bool,


    has_button_held: bool,
    latest_gui_pad: Option<PadButton>,
}

impl InternalDevice {
    fn new(path: PathBuf, dev: Device, hash: DeviceHash) -> Self {
        Self {
            path,
            hash,
            device: Arc::new(Mutex::new(dev)),
            lease: None,
            grabbed: false,
            has_button_held: false,
            latest_gui_pad: None,
        }
    }

    fn update_grabbed_status(&mut self) {
        let should_grab = match &self.lease {
            Some(lease_arc) => {
                let lease = lease_arc.lock().unwrap();
                lease.get_grabbed_status()
            }
            None => false,
        };

        if should_grab != self.grabbed {
            if let Ok(mut dev) = self.device.lock() {
                if should_grab {dev.grab();} else {dev.ungrab();}
            }
            self.grabbed = should_grab;
        }
    }

    /// Fetch events and dispatch to its bound SharedLease (if any). Return Err(()) if device fetch failed.
    fn fetch_and_dispatch_events(&mut self, ctx: &egui::Context) -> Result<(), ()> {
        let dev_arc = Arc::clone(&self.device);
        let mut dev = dev_arc.lock().map_err(|_| ())?;
        match dev.fetch_events() {
            Ok(iter) => {
                let events = iter.collect::<Vec<InputEvent>>();
                if !events.is_empty() {
                    if let Some(lease_arc) = &self.lease {
                        let mut lease = lease_arc.lock().unwrap();
                        lease.on_input(&events, ctx);
                    }
                    self.gui_poll(events);
                }
                Ok(())
            }
            Err(_) => Err(()),
        }
    }
    

    fn name(&self) -> String {
        let dev = self.device.lock().unwrap();
        dev.name().unwrap_or_else(|| "").to_string()
    }
    
    fn device_type(&mut self) -> DeviceType {
        let dev = self.device.lock().unwrap();
        let device_type = match dev.supported_keys() {
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

    fn emoji(&mut self) -> String {
        match self.device_type() {
            DeviceType::Gamepad => "🎮",
            DeviceType::Keyboard => "🖮",
            DeviceType::Mouse => "🖱",
            DeviceType::Other => "",
        }.to_string()
    }

    fn fancyname(&self) -> String {
        let name = self.name();
        let name_str = name.as_str();

        let dev = self.device.lock().unwrap();

        match dev.input_id().vendor() {
            0x045e => "Xbox Controller",
            0x054c => "PS Controller",
            0x057e => "NT Pro Controller",
            0x28de => "Steam Input",
            _ => name_str,
        }.to_string()
    }

    fn path(&self) -> &str {
        self.path.to_str().unwrap_or_default()
    }

    pub fn label(&mut self) -> String {
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
        let dev = self.device.lock().unwrap();
        match filter {
            PadFilterType::All => true,
            PadFilterType::NoSteamInput => dev.input_id().vendor() != 0x28de,
            PadFilterType::OnlySteamInput => dev.input_id().vendor() == 0x28de,
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

    pub fn has_button_held(&mut self) -> bool {return self.has_button_held;}

    pub fn latest_gui_pad(&mut self) -> Option<PadButton> {return self.latest_gui_pad.clone();}
}

// ---------------------- Manager (InputStateInner) --------------------------

pub struct InputStateInner {
    pub devices: Vec<Arc<Mutex<InternalDevice>>>,
    pub shared_leases: HashMap<SharedLeaseId, Arc<Mutex<SharedLease>>>,
    pub orphan_leases: Vec<Arc<Mutex<SharedLease>>>,
    pub ctx: egui::Context,
}

impl InputStateInner {
    fn new(ctx: egui::Context) -> Self {
        Self {
            devices: Vec::new(),
            shared_leases: HashMap::new(),
            orphan_leases: Vec::new(),
            ctx,
        }
    }

    /// Try find an unused device that matches the shared lease's hash.
    fn find_matching_unused_device(&mut self, lease: &SharedLease) -> Option<Arc<Mutex<InternalDevice>>> {
        let target_hash = compute_device_hash_lease(lease);

        self.devices.iter()
            .find(|dev_arc| {
                if let Ok(dev_guard) = dev_arc.lock() {
                    dev_guard.lease.is_none() && dev_guard.hash == target_hash
                } else { false }
            })
            .cloned()
    }

    fn bind_lease_to_device(&mut self, lease_arc: &Arc<Mutex<SharedLease>>, dev_arc: &Arc<Mutex<InternalDevice>>) {
        if let Ok(mut dev) = dev_arc.lock() {
            dev.lease = Some(Arc::clone(lease_arc));
            dev.update_grabbed_status();
            if let Ok(mut lease) = lease_arc.lock() {
                lease.assigned_device_path = Some(dev.path.clone());
            }
        }
    }

    /// When a device is added: create InternalDevice and attempt to bind a matching orphan lease.
    fn add_device_obj(&mut self, path: PathBuf, mut dev: Device) {
        let hash = compute_device_hash_dev(&mut dev);
        let dev_arc = Arc::new(Mutex::new(InternalDevice::new(path.clone(), dev, hash)));
        // try to find a matching orphan lease
        if let Some((idx, lease_arc)) = self.orphan_leases.iter().enumerate()
            .find(|(_, l)| {
                if let Ok(lease) = l.lock() {
                    let target_hash = compute_device_hash_lease(&lease);
                    target_hash == hash
                } else { false }
            })
            .map(|(i, l)| (i, Arc::clone(l)))
        {
            // bind the orphan lease to this device
            self.bind_lease_to_device(&lease_arc, &dev_arc);
            // remove from orphan list
            let _ = self.orphan_leases.swap_remove(idx);
        }
        self.devices.push(dev_arc);
    }

    /// Remove device by path; if bound, make its SharedLease orphaned again.
    fn remove_device_by_path(&mut self, path: &PathBuf) {
        if let Some(pos) = self.devices.iter().position(|d| {
            d.lock().map(|g| g.path == *path).unwrap_or(false)
        }) {
            // if bound, orphan the lease
            if let Ok(mut dev) = self.devices[pos].lock() {
                if let Some(lease_arc) = dev.lease.take() {
                    if let Ok(mut lease) = lease_arc.lock() {
                        lease.assigned_device_path = None;
                    }
                    self.orphan_leases.push(lease_arc);
                }
            }
            self.devices.swap_remove(pos);
        }
    }

    /// Attempt to bind all orphan leases to available devices.

    fn fix_orphans(&mut self) {
        let mut i = 0;
        while i < self.orphan_leases.len() {
            // Cheaply clone the Arc to end the borrow on `self` immediately
            let lease_arc = self.orphan_leases[i].clone();
            
            let bound = {
                let lease = lease_arc.lock().unwrap();
                self.find_matching_unused_device(&lease)
            };
            
            if let Some(dev_arc) = bound {
                self.bind_lease_to_device(&lease_arc, &dev_arc);
                let _ = self.orphan_leases.swap_remove(i);
            } else {
                i += 1;
            }
        }
    }

    /// Poll devices and dispatch events. This function expects to be called from the input thread.
    fn poll_and_dispatch(&mut self, monitor_fd: BorrowedFd<'_>) -> Result<(), Box<dyn std::error::Error>> {
        // Build FD snapshot
        let mut fds: Vec<(PathBuf, OwnedFd)> = Vec::new();
        for dev_arc in self.devices.iter() {
            if let Ok(dev_guard) = dev_arc.lock() {
                if let Ok(device_guard) = dev_guard.device.lock() {
                    let fd = dup(device_guard.as_fd())?;
                    fds.push((dev_guard.path.clone(), fd));
                }
            }
        }

        // Build poll vector: optional monitor fd first
        let mut poll_fds: Vec<PollFd> = Vec::new();
        poll_fds.push(PollFd::new(monitor_fd, PollFlags::POLLIN));

        for &(_, ref fd) in fds.iter() {
            poll_fds.push(PollFd::new(fd.as_fd(), PollFlags::POLLIN));
        }

        // Blocking poll with timeout
        let _ = poll(&mut poll_fds, PollTimeout::try_from(POLL_TIMEOUT_MS as i32)?)?;


        // For devices that fired, map index -> path and dispatch
        for (idx, poll_fd) in poll_fds.iter().enumerate().skip(1) { // Skip monitor FD
            if let Some(revents) = poll_fd.revents() {
                if revents.intersects(PollFlags::POLLIN | PollFlags::POLLERR | PollFlags::POLLHUP) {
                    // map idx-device_start_idx -> fds index
                    let fds_idx = idx - 1; // Skip monitor FD
                    if let Some((path, _fd)) = fds.get(fds_idx) {
                        // find device entry by path and dispatch
                        if let Some(dev_arc) = self.devices.iter().find(|d| d.lock().map(|g| g.path == *path).unwrap_or(false)).cloned() {
                            if let Ok(mut dev) = dev_arc.lock() {
                                if dev.fetch_and_dispatch_events(&self.ctx).is_err() {
                                    // device failed: remove
                                    let p = dev.path.clone();
                                    drop(dev);
                                    self.remove_device_by_path(&p);
                                }
                            }
                        }
                    }
                }
            }
        }

        thread::sleep_ms(10);

        Ok(())
    }
}

// ---------------------- Public wrapper --------------------------

pub struct InputState {
    inner: Arc<Mutex<InputStateInner>>,
    latest_shared_lease_id: AtomicU64,
    latest_lease_id: AtomicU64,
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
                        guard.add_device_obj(devpath.0.clone(), device);
                    }
                }
                guard.fix_orphans();
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
                                    guard.remove_device_by_path(&devnode.to_path_buf());
                                }
                            }
                        }
                        _ => {}
                    }
                }

                // snapshot monitor fd if present
                let monitor_fd = monitor.as_fd();

                // Poll + dispatch (we lock inner inside)

                // THIS IS THE LOCKING BUG DAVID TODO: FIX!
                if let Ok(mut guard) = thread_inner.lock() {
                    let _ = guard.poll_and_dispatch(monitor_fd);
                    // fix orphans (in case new devices bound)
                    guard.fix_orphans();
                }
            }
        });

        Ok(Self {
            inner,
            latest_shared_lease_id: AtomicU64::new(1),
            latest_lease_id: AtomicU64::new(1),
            shutdown_tx,
            _thread: thread,
        })
    }

    /// Create or reuse a shared lease and return a DevLease handle.
    pub fn new_dev_lease(
        &self,
        requested_shared_id: Option<SharedLeaseId>,
        dev_name: String,
        dev_input_id: InputId,
        dev_unique_name: String,
        viewport: egui::ViewportId,
        grabbed: bool,
    ) -> DevLease {
        let lease_arc: Arc<Mutex<SharedLease>>;
        let lease_id = self.latest_lease_id.fetch_add(1, Ordering::SeqCst);

        {
            let mut inner = self.inner.lock().unwrap();

            // Try requested id
            if let Some(req_id) = requested_shared_id {
                if let Some(existing) = inner.shared_leases.get(&req_id) {
                    lease_arc = Arc::clone(existing);
                } else {
                    // not found; create new below
                    let new_shared_id = self.latest_shared_lease_id.fetch_add(1, Ordering::SeqCst);
                    let s = SharedLease::new(new_shared_id, dev_name.clone(), dev_input_id.clone(), dev_unique_name.clone());
                    let s_arc = Arc::new(Mutex::new(s));
                    inner.shared_leases.insert(new_shared_id, Arc::clone(&s_arc));
                    inner.orphan_leases.push(Arc::clone(&s_arc));
                    lease_arc = s_arc;
                }
            } else {
                // find existing match by metadata
                if let Some((_, existing)) = inner.shared_leases.iter()
                    .find(|(_, arc)| {
                        if let Ok(l) = arc.lock() {
                            l.dev_name == dev_name && l.dev_unique_name == dev_unique_name && l.dev_input_id == dev_input_id
                        } else { false }
                    }) {
                    lease_arc = Arc::clone(existing);
                } else {
                    // create new shared lease
                    let new_shared_id = self.latest_shared_lease_id.fetch_add(1, Ordering::SeqCst);
                    let s = SharedLease::new(new_shared_id, dev_name.clone(), dev_input_id.clone(), dev_unique_name.clone());
                    let s_arc = Arc::new(Mutex::new(s));
                    inner.shared_leases.insert(new_shared_id, Arc::clone(&s_arc));
                    inner.orphan_leases.push(Arc::clone(&s_arc));
                    lease_arc = s_arc;
                }
            }

            // register user
            let mut lease = lease_arc.lock().unwrap();
            lease.add_user(lease_id, viewport, grabbed);

            // attempt immediate binding
            inner.fix_orphans();
        }

        DevLease {
            manager: Arc::clone(&self.inner),
            shared_lease_id: lease_arc.lock().unwrap().id,
            lease_id,
        }
    }

    pub fn new_dev_lease_from_dev(
        &self,
        dev: &mut InternalDevice,
        viewport: egui::ViewportId,
        grabbed: bool,
    ) -> DevLease {
        let lease_id = self.latest_lease_id.fetch_add(1, Ordering::SeqCst);

        if let Some(lease) = &dev.lease {
            let mut locked_shared_lease = lease.lock().unwrap();
            locked_shared_lease.add_user(lease_id, viewport, grabbed);

            return DevLease { manager: self.inner.clone(), shared_lease_id: locked_shared_lease.id, lease_id };
        } else {
            let mut inner = self.inner.lock().unwrap();

            let new_shared_id = self.latest_shared_lease_id.fetch_add(1, Ordering::SeqCst);
            
            let device_locked = dev.device.lock().unwrap();

            let sh_lease = SharedLease::new(
                new_shared_id, 
                device_locked.name().unwrap_or("UNKNOWN").to_string(), 
                device_locked.input_id(), 
                device_locked.unique_name().unwrap_or("UNKNOWN").to_string()
            );
            
            let sh_lease_arc = Arc::new(Mutex::new(sh_lease));
            inner.shared_leases.insert(new_shared_id, Arc::clone(&sh_lease_arc));

            dev.lease = Some(sh_lease_arc);

            return DevLease { manager: self.inner.clone(), shared_lease_id: new_shared_id, lease_id };
        }
    }

    pub fn fix_orphans(&self) {
        if let Ok(mut inner) = self.inner.lock() {
            inner.fix_orphans();
        }
    }

    pub fn get_shared_lease(&self, id: SharedLeaseId) -> Option<Arc<Mutex<SharedLease>>> {
        self.inner.lock().unwrap().shared_leases.get(&id).cloned()
    }

    pub fn devices(&self) -> Vec<Arc<Mutex<InternalDevice>>> {
        self.inner.lock().unwrap().devices.clone()
    }
}

impl Drop for InputState {
    fn drop(&mut self) {
        let _ = self.shutdown_tx.send(());
        // join thread is not required here; handle if you want to wait for clean exit.
    }
}

// ---------------------- DevLease ----------------

pub struct DevLease {
    manager: Arc<Mutex<InputStateInner>>, // strong Arc as requested (no shim)
    shared_lease_id: SharedLeaseId, // Todo maybe just include an arc instead of the ID as we already are making the arc.
    lease_id: LeaseId,
}

impl DevLease {
    pub fn set_grabbed(&self, grabbed: bool) {
        let mgr = self.manager.lock().unwrap();
        if let Some(lease_arc) = mgr.shared_leases.get(&self.shared_lease_id) {
            lease_arc.lock().unwrap().set_user_grab(self.lease_id, grabbed);
            // update device grabbed state
            for dev_arc in mgr.devices.iter() {
                if let Ok(mut dev) = dev_arc.lock() {
                    if let Some(larc) = &dev.lease {
                        if larc.lock().unwrap().id == self.shared_lease_id {
                            dev.update_grabbed_status();
                        }
                    }
                }
            }
        }
    }

    pub fn pop_events(&self, max: usize) -> Vec<InputEvent> {
        let mgr = self.manager.lock().unwrap();
        if let Some(lease_arc) = mgr.shared_leases.get(&self.shared_lease_id) {
            return lease_arc.lock().unwrap().pop_events_for(self.lease_id, max);
        }
        Vec::new()
    }

    pub fn lease_id(&self) -> LeaseId {
        return self.lease_id;
    }
    pub fn shared_lease_id(&self) -> SharedLeaseId {
        return self.shared_lease_id;
    }
}

impl Drop for DevLease {
    fn drop(&mut self) {
        let mut mgr = self.manager.lock().unwrap();
        if let Some(lease_arc) = mgr.shared_leases.get(&self.shared_lease_id) {
            lease_arc.lock().unwrap().remove_user(self.lease_id);
            if lease_arc.lock().unwrap().is_empty() {
                // unbind from device (if any) and remove lease
                mgr.unbind_lease_from_device_internal(self.shared_lease_id);
                mgr.shared_leases.remove(&self.shared_lease_id);
            }
        }
    }
}

// Helper method on InputStateInner to unbind a shared lease by id (used in Drop)
impl InputStateInner {
    fn unbind_lease_from_device_internal(&mut self, lease_id: SharedLeaseId) {
        // find any device bound to this lease and clear it, then push lease to orphan list
        let mut maybe_lease_arc: Option<Arc<Mutex<SharedLease>>> = None;
        for dev_arc in self.devices.iter() {
            if let Ok(mut dev) = dev_arc.lock() {
                if let Some(larc) = &dev.lease {
                    if let Ok(l) = larc.lock() {
                        if l.id == lease_id {
                            // remove binding
                            maybe_lease_arc = Some(Arc::clone(larc));
                        }
                    }
                }
                // Removing binding p2
                if maybe_lease_arc.is_some() {
                    dev.lease = None;
                    dev.update_grabbed_status();
                    break;
                }
            }
        }
        if let Some(larc) = maybe_lease_arc {
            if let Ok(mut lease) = larc.lock() {
                lease.assigned_device_path = None;
            }
            self.orphan_leases.push(larc);
        }
    }
}