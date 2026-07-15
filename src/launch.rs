use std::io::{BufRead, Read};
use std::os::fd::{AsFd, AsRawFd, OwnedFd};
use std::path::{Path, PathBuf};
use std::process::{Child, Command};
use std::sync::{Arc, Mutex, RwLock};

use crate::app::{PadFilterType, PartyConfig};
use crate::video::egl::EglApi;
use crate::video::pipewire::{PipewireCommand, PipewireID, PipewireStream};
use crate::video::video::PipewireVideo;
use crate::{handler::*, input};
use crate::input::*;
use crate::instance::*;
use crate::monitor::Monitor;
use crate::paths::*;
use crate::profiles::{create_profile, create_profile_gamesave};
use crate::util::*;
use eframe::egui::{self, Pos2, Rect, Vec2};
use eframe::epaint::tessellator::path;
use nix::libc::close;
use nix::sys::signal::{Signal, kill};
use nix::sys::wait::{WaitStatus, waitpid};
use nix::unistd::Pid;
use tokio::task::JoinSet;
use zbus::conn;
use std::collections::{HashMap, HashSet};
use pipewire as pw;


use crate::layout_manager::{WindowPostion, kwin_dbus_start_script, spawn_comp_and_get_display};

pub fn setup_profiles(
    h: &Handler,
    instances: &Vec<&RunningInstance>,
) -> Result<(), Box<dyn std::error::Error>> {
    // println!("\n[partydeck] Instances:");
    for instance in instances {
        if instance.profname.starts_with(".") {
            create_profile(&instance.profname)?;
        }
        if h.is_saved_handler() {
            create_profile_gamesave(&instance.profname, h)?;
        }
        // println!(
        //     "[partydeck] - Profile: {}, Monitor: {}, Resolution: {}x{}",
        //     instance.profname, instance.monitor, instance.width, instance.height
        // );
    }

    Ok(())
}

pub fn launch_game(
    h: &Handler,
    input_devices: &Vec<RunningInputDevice>,
    displays: &mut Vec<RunningLaunchDisplay>,
    cfg: &PartyConfig,
    real_monitors: &Vec<Monitor>,
) -> Result<(), Box<dyn std::error::Error>> {
   
    // let mut tasks = JoinSet::new();

    start_compositors_and_generate_commands(
        h,
        input_devices,
        displays,
        cfg,
        real_monitors,
    )?;


    // I dont know why &mut * works, but we just accept the rust magic
    for display in &mut *displays {
        if let Some(compositor) = &mut display.compositor_proc {
            if compositor.try_wait()? != None {
                println!("[partydeck] Compositor ({}) died - Skipping instances!", display.nested_compositor.display_name());
                continue;
            }
        }
        println!("[partydeck] Spawning instances for compositor: '{}'...", display.nested_compositor.display_name());

        for instance in &mut display.instances {
            if let Some(command) = &mut instance.command {
                instance.game_proc = Some(
                    command.spawn().expect(
                        &format!("Failed to launch game ({})", command_to_bash_script(command))
                    )
                );
            } else {
                Err("Game missing command to start somehow?")?;
            }
        }
    }

    let _ = tokio::runtime::Runtime::new()?.block_on(async {
        let connection = zbus::Connection::session().await?;
        connection
                .request_name_with_flags(
                    "com.partydeck.layoutManager", 
                    zbus::fdo::RequestNameFlags::ReplaceExisting.into()
                ).await?;
        
        let layout_mgr_displays: Arc<Mutex<Vec<(Box<dyn crate::layout_manager::LayoutWindows + Send>, Vec<u32>)>>> = 
            Arc::new(Mutex::new(
                displays.iter().map(|disp| {
                    (
                        disp.layout.clone_box(),
                        disp.instances.iter().filter_map(|inst| {
                            if let Some(proc) = &inst.game_proc {
                                Some(proc.id())
                            } else {
                                None
                            }
                        }).collect()
                    )
                }).collect()
            ));

        connection.object_server().at("/com/partydeck/layoutManager", LayoutManagerDbus { displays: layout_mgr_displays }).await?;

        println!("[partydeck] D-Bus service started");
        
       loop {
            // Maybe switch to pidfd and `select_all` to avoid waiting here.
            match waitpid(None, Some(nix::sys::wait::WaitPidFlag::WNOHANG))? {
                WaitStatus::Exited(_pid, _) | WaitStatus::Signaled(_pid, _, _) => {
                    if !check_for_and_kill_games(displays)? {
                        break;
                    }
                }
                WaitStatus::StillAlive => {
                    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                }
                _ => {
                    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                }
            }
        }

        Ok::<(), Box<dyn std::error::Error>>(()) 
    });

    // Todo kill all the games and compositors. Should already be done above, but might as well do it again, and force it.


    Ok(())
}


// #[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
// pub struct NumberQuad(pub i32, pub i32, pub i32, pub i32);




struct LayoutManagerDbus {
    displays: Arc<Mutex<Vec<(Box<dyn crate::layout_manager::LayoutWindows + Send>, Vec<u32>)>>>
}

#[zbus::interface(name = "com.partydeck.layoutManager")]
impl LayoutManagerDbus {
    pub fn process_layout(&self, width: i32, height: i32, pids: Vec<zbus::zvariant::OwnedValue>) -> Vec<(u32,u32,u32,u32)> {
        let disp_lock = match self.displays.lock() {
            Ok(disp_lock) => disp_lock,
            Err(_) => {
                eprintln!("Display mutex failed.");
                return Vec::new();
            }
        };
        // println!("PIDS: {:#?}, disp: {:#?}", pids, disp_lock.iter().map(|a| a.1.clone()).collect::<Vec<_>>());

        let pids: Vec<u32> = pids
            .into_iter()
            .filter_map(|val| {
                // This may cause problems if your pids get high enough // TODO refactor if you can find how to
                // Pass JS values as u32 (or actaully underlying u64 instead of just i32).
                val.downcast_ref::<u32>().or_else(|_| val.downcast_ref::<i32>().map(|a| a as u32)).ok()
            }).collect();

        let target_display = disp_lock.iter().find(|disp| {
            disp.1.iter().any(|pid| pids.contains(pid))
        });

        let Some((layout_manager, display_pids)) = target_display else {
            println!("NOT FOUND IN LAYOUT MANAGER");
            return Vec::new();
        };

        let window_count = pids.iter().filter(|pid| display_pids.contains(pid)).count() as u32;
        let mut layout_iter = layout_manager.layout(window_count, width as u32, height as u32).into_iter();

        let mut pid_to_layout: HashMap<u32, _> = HashMap::with_capacity(display_pids.len());

        for pid in display_pids {
            if let Some(pos) = layout_iter.next() {
                pid_to_layout.insert(*pid, pos);
            }
        }

        // println!("Should be returning.");

        pids
            .iter()
            .map(|pid| {
                if let Some(pos) = pid_to_layout.get(pid) {
                    (pos.x, pos.y, pos.w, pos.h)
                } else {
                    (0, 0, 0, 0)
                }
            })
            .collect()
    }
}


pub fn check_for_and_kill_games(
    displays: &mut Vec<RunningLaunchDisplay>
) -> Result<bool, Box<dyn std::error::Error>> {
    let mut has_alive_games = false;
    for display in displays {
        let mut display_has_alive_games = false;

        let comp_alive = if let Some(compositor) = &mut display.compositor_proc {
            compositor.try_wait()? == None
        } else {
            true
        };

        for instance in &mut display.instances {
            if let Some(game_proc) = &mut instance.game_proc {
                if game_proc.try_wait()? != None {continue;}
                if !comp_alive {
                    game_proc.kill()?;
                } else {
                    has_alive_games = true;
                    display_has_alive_games = true;
                }
            }
        }

        if !display_has_alive_games && comp_alive && let Some(compositor) = &mut display.compositor_proc {
            let _ = compositor.kill();
        }
    }

    Ok(has_alive_games)
}

pub fn start_compositors_and_generate_commands(
    h: &Handler,
    input_devices: &Vec<RunningInputDevice>,
    displays: &mut Vec<RunningLaunchDisplay>,
    cfg: &PartyConfig,
    real_monitors: &Vec<Monitor>,
) -> Result<(), Box<dyn std::error::Error>> {
    let win = h.win();
    let exec = Path::new(&h.exec);
    let runtime = h.runtime.as_str();
    let gamescope = match cfg.kbm_support {
        true => BIN_GSC_KBM.as_path(),
        false => Path::new("gamescope"),
    };

    if cfg.kbm_support && !gamescope.exists() {
        return Err("gamescope-kbm is missing. Please reinstall partydeck or disable KBM support.".into());
    }

    if !cfg.kbm_support && pathsearch::find_executable_in_path("gamescope").is_none() {
        return Err("gamescope not found in PATH. Please install gamescope through your distro's package manager.".into());
    }

    if (runtime == "scout" && !PATH_STEAM.join("bin32/steam-runtime/run.sh").exists())
        || (runtime == "soldier"
            && !PATH_STEAM
                .join("steam/steamapps/common/SteamLinuxRuntime_soldier")
                .exists())
        || (runtime == "sniper"
            && !PATH_STEAM
                .join("steam/steamapps/common/SteamLinuxRuntime_sniper")
                .exists()
            && !PATH_STEAM
                .join("steam/steamapps/common/SteamLinuxRuntime_sniper-arm64")
                .exists())
        || (runtime == "steamrt4"
            && !PATH_STEAM
                .join("steam/steamapps/common/SteamLinuxRuntime_4")
                .exists())
    {
        return Err(format!("Steam Runtime {runtime} not found! Runtime must be installed on the same drive that the Steam client is installed on.").into());
    }


    let mut full_count_idx = 0;
    for display in displays {
        let (way_display_name, x11_display_name, compositor_proc, instances_res) =
            if let Some(compositor) = &display.nested_compositor.launch_executable() {
                let (way_name, x11_name, monitor, compositor_proc) =
                    spawn_comp_and_get_display(compositor, &real_monitors[display.display_index])
                        .ok_or("Failed to spawn nested compositor and get display names")?;

                // compositor_proc.try_wait()
                let display_layout = display.layout.layout(display.instances.len() as u32, monitor.width(), monitor.height());

                (Some(way_name), Some(x11_name), Some(compositor_proc), display_layout)
            } else {
                let monitor = &real_monitors[display.display_index];
                let display_layout = display.layout.layout(display.instances.len() as u32, monitor.width(), monitor.height());

                (None, None, None, display_layout)
            };

        std::thread::sleep(std::time::Duration::from_secs_f64(1.0));

        display.compositor_proc = compositor_proc;

        for i in 0..display.instances.len() {

            let mut generated_launch_cmd = generate_launch_command(
                win,
                exec,
                runtime,
                gamescope,
                h,
                input_devices,
                cfg,
                display,
                &display.instances[i],
                i,
                full_count_idx,
                &instances_res[i],
            )?;

            if let Some(ref disp) = way_display_name {generated_launch_cmd.env("WAYLAND_DISPLAY",   disp);}
            if let Some(ref disp) = x11_display_name {generated_launch_cmd.env("DISPLAY",           disp);}

            println!("[partydeck] Instance {}: {}", full_count_idx, command_to_bash_script(&generated_launch_cmd));

            display.instances[i].command = Some(generated_launch_cmd);
            full_count_idx+=1
        }
    }

    Ok(())
}

pub fn generate_launch_command(
        win: bool,
        exec: &Path,
        runtime: &str,
        gamescope: &Path,
        h: &Handler,
        input_devices: &Vec<RunningInputDevice>,
        cfg: &PartyConfig,
        display: &RunningLaunchDisplay,
        instance: &RunningInstance,
        current_display_idx: usize,
        full_count_idx: usize,
        window_size: &WindowPostion,
) -> Result<Command, Box<dyn std::error::Error>>  {
    let gamedir =
        if h.is_saved_handler() && !cfg.disable_mount_gamedirs && cfg.profile_unique_dirs {
            PATH_PARTY.join("tmp").join(format!("game-{}", full_count_idx))
        } else {
            PathBuf::from(h.get_game_rootpath()?)
        };

    if !gamedir.join(exec).exists() {
        return Err(format!("Executable not found: {}", gamedir.join(exec).display()).into());
    }

    let path_exec = gamedir.join(exec);
    let cwd = path_exec.parent().ok_or_else(|| "couldn't get parent")?;

    let path_prof = PATH_PARTY.join("profiles").join(&instance.profname);
    let path_pfx = PATH_PARTY
        .join("prefixes")
        .join(match cfg.proton_separate_pfxs {
            true => (full_count_idx + 1).to_string(),
            false => "1".to_string(),
        });

    let mut cmd = Command::new(gamescope);

    cmd.current_dir(cwd);

    cmd.env("SDL_JOYSTICK_HIDAPI", "0");
    cmd.env("ENABLE_GAMESCOPE_WSI", "0");
    cmd.env("PROTON_DISABLE_HIDRAW", "1");
    if h.sdl2_override != SDL2Override::No {
        let path_sdl = match h.sdl2_override {
            SDL2Override::Srt => {
                PATH_STEAM.join("bin32/steam-runtime/usr/lib/i386-linux-gnu/libSDL2-2.0.so.0")
            }
            SDL2Override::Sys => PathBuf::from("/usr/lib/libSDL2.so"),
            _ => PathBuf::new(),
        };
        cmd.env("SDL_DYNAMIC_API", path_sdl);
    }
    if win {
        let protonpath = match cfg.proton_version.is_empty() {
            true => "GE-Proton",
            false => &cfg.proton_version,
        };

        cmd.env("WINEPREFIX", &path_pfx);
        cmd.env("PROTON_VERB", "run");
        cmd.env("PROTONPATH", protonpath);
        cmd.env("PROTON_DISABLE_HIDRAW", "1");
        if cfg.proton_wow64 {
            cmd.env("PROTON_USE_WOW64", "1");
        }
    }
    if cfg.pad_filter_type != PadFilterType::NoSteamInput {
        cmd.env("SDL_GAMECONTROLLER_ALLOW_STEAM_VIRTUAL_GAMEPAD", "1");
    }
    if cfg.pad_filter_type == PadFilterType::OnlySteamInput {
        cmd.env(
            "SDL_GAMECONTROLLER_IGNORE_DEVICES",
            SDL_GAMECONTROLLER_IGNORE_DEVICES,
        );
    }
    if !h.env.is_empty() {
        for env_var in h.env.split_whitespace() {
            if let Some((key, value)) = env_var.split_once('=') {
                cmd.env(key, value);
            }
        }
    }

    // Gamescope args
    if cfg.gamescope_resize_support {
        cmd.args(["--nested-follow-window-scale", "1"]);
    }
    if cfg.gamescope_force_fullscreen {
        cmd.arg("--force-windows-fullscreen");
    }

    if h.use_mangohud {
        cmd.arg("--mangoapp");
    }

    cmd.args([
        "-W",
        &window_size.w.to_string(),
        "-H",
        &window_size.h.to_string(),
    ]);
    if cfg.gamescope_force_grab_cursor {
        cmd.arg("--force-grab-cursor");
    }
    if cfg.gamescope_sdl_backend { //  && display.nested_compositor != ""
        cmd.arg("--backend=sdl");
        cmd.arg(format!("--display-index={}", display.display_index));
    }

    let input_devices_enabled: Vec<&RunningInputDevice> = instance.devices.iter().filter_map(|dev_find_hash| {
        for dev in input_devices {
            if dev.hash == *dev_find_hash {return Some(dev)}
        }

        None
    }).collect();


    if cfg.kbm_support {
        let mut instance_has_keyboard = false;
        let mut instance_has_mouse = false;
        let mut kbms = String::new();

        for dev in &input_devices_enabled {
            if dev.device_type == DeviceType::Keyboard {
                instance_has_keyboard = true;
            } else if dev.device_type == DeviceType::Mouse {
                instance_has_mouse = true;
            }
            if dev.device_type == DeviceType::Keyboard || dev.device_type == DeviceType::Mouse {
                kbms.push_str(&format!("{},", &dev.path));
            }

        }

        if instance_has_keyboard {
            cmd.arg("--backend-disable-keyboard");
        }
        if instance_has_mouse {
            cmd.arg("--backend-disable-mouse");
        }
        if !kbms.is_empty() {
            cmd.arg(format!("--libinput-hold-dev={}", kbms));
            cmd.arg("--grab");
        }
    }
    cmd.arg("--");

    // Bwrap args
    cmd.arg("bwrap");
    cmd.arg("--die-with-parent");
    cmd.args(["--dev-bind", "/", "/"]);
    cmd.args(["--tmpfs", "/tmp"]);
    // Mask out any gamepads that aren't this player's
    for dev in input_devices {
        if !dev.enabled
            || (
                !input_devices_enabled.iter().any(|dev_en| {dev_en.hash == dev.hash}) 
                && dev.device_type == DeviceType::Gamepad
            )
        {
            cmd.args(["--bind", "/dev/null", &dev.path]);
        }
    }

    if cfg.profile_unique_dirs {
        if win {
            let path_pfx_user = path_pfx.join("drive_c/users/steamuser");
            cmd.arg("--bind")
                .args([&path_prof.join("windata"), &path_pfx_user]);
        } else {
            let path_prof_home = path_prof.join("home");
            cmd.env("HOME", &path_prof_home);
            // Also bind the Steam directory as the Steam runtimes look for HOME/.steam
            if !runtime.is_empty() || h.steam_appid.is_some() {
                cmd.args([
                    "--bind",
                    &PATH_STEAM.to_string_lossy(),
                    &path_prof_home.join(".steam").to_string_lossy(),
                ]);
            }
        }
    }

    for subpath in &h.game_null_paths {
        let game_subpath = gamedir.join(subpath);
        if game_subpath.is_file() {
            cmd.args(["--bind", "/dev/null", &game_subpath.to_string_lossy()]);
        } else if game_subpath.is_dir() {
            cmd.args([
                "--bind",
                &PATH_PARTY.join("tmp/null").to_string_lossy(),
                &game_subpath.to_string_lossy(),
            ]);
        }
    }

    if h.use_goldberg {
        cmd.env("GseAppPath", PATH_PARTY.join("goldberg_data"));
        cmd.env("GseSavePath", path_prof.join("steam"));
        cmd.env("SteamAppUser", instance.profname.clone());
        cmd.env("SteamUser", instance.profname.clone());
        cmd.env("SteamClientLaunch", "1");
        cmd.env("SteamEnv", "1");
        if let Some(appid) = h.steam_appid {
            cmd.env("SteamAppId", &appid.to_string());
            cmd.env("SteamGameId", &appid.to_string());
        }

        let sdk32_link = std::fs::read_link(PATH_STEAM.join("sdk32"))
            .map_err(|e| format!("Failed to read sdk32 link: {}", e))?;
        let sdk64_link = std::fs::read_link(PATH_STEAM.join("sdk64"))
            .map_err(|e| format!("Failed to read sdk64 link: {}", e))?;

        cmd.arg("--bind")
            .args([PATH_RES.join("goldberg/linux32"), sdk32_link]);

        cmd.arg("--bind")
            .args([PATH_RES.join("goldberg/linux64"), sdk64_link]);

        if win {
            cmd.arg("--bind").args([
                PATH_RES.join("goldberg/win"),
                path_pfx.join("drive_c/Program Files (x86)/Steam"),
            ]);
        }
    }

    // Runtime
    if win {
        cmd.arg(&*BIN_UMU_RUN);
    } else {
        match runtime {
            "scout" => {
                cmd.arg(PATH_STEAM.join("bin32/steam-runtime/run.sh"));
            }
            "soldier" => {
                cmd.arg(
                    PATH_STEAM.join(
                        "steam/steamapps/common/SteamLinuxRuntime_soldier/_v2-entry-point",
                    ),
                );
                cmd.arg("--");
            }
            "sniper" => {
                let sniper_path = PATH_STEAM
                    .join("steam/steamapps/common/SteamLinuxRuntime_sniper/_v2-entry-point");
                // old installations of sniper go in a folder named -arm64 even though it is x86_64?
                let sniper_arm_path = PATH_STEAM.join(
                    "steam/steamapps/common/SteamLinuxRuntime_sniper-arm64/_v2-entry-point",
                );
                if sniper_path.exists() {
                    cmd.arg(sniper_path);
                } else if sniper_arm_path.exists() {
                    cmd.arg(sniper_arm_path);
                }
                cmd.arg("--");
            }
            "steamrt4" => {
                cmd.arg(
                    PATH_STEAM
                        .join("steam/steamapps/common/SteamLinuxRuntime_4/_v2-entry-point"),
                );
                cmd.arg("--");
            }
            _ => {}
        };
    }

    cmd.arg(&path_exec);

    for arg in h.args.split_whitespace() {
        let processed_arg = match arg {
            "$PROFILE" => &instance.profname,
            "$WIDTH" => &window_size.w.to_string(),
            "$HEIGHT" => &window_size.h.to_string(),
            "$RESOLUTION" => &format!("{}x{}", window_size.w.to_string(), window_size.h.to_string()),
            "$INSTANCECOUNT" => &display.instances.len().to_string(),
            "$INSTANCENUM" => &current_display_idx.to_string(),
            "$FULLCOUNTIDX" => &full_count_idx.to_string(),
            "$GAMEDIR" => &gamedir.os_fmt(win),
            "$HANDLERDIR" => &h.path_handler.os_fmt(win),
            _ => &String::from(arg).sanitize_path(),
        };
        cmd.arg(processed_arg);
    }

    Ok(cmd)
}


pub fn fuse_overlayfs_mount_gamedirs(
    h: &Handler,
    instances: &Vec<&RunningInstance>,
) -> Result<(), Box<dyn std::error::Error>> {
    let tmp_dir = PATH_PARTY.join("tmp");
    let mut path_lowerdir = h.get_game_rootpath()?;

    let overlay_path = h.path_handler.join("overlay");
    if overlay_path.exists() {
        path_lowerdir = format!("{}:{}", overlay_path.display(), path_lowerdir);
    }

    let gamename = h.handler_dir_name().to_string();

    let mut cmds: Vec<Command> = (0..instances.len())
        .map(|_| Command::new("fuse-overlayfs"))
        .collect();

    for (i, instance) in instances.iter().enumerate() {
        let cmd = &mut cmds[i];

        let path_game_mnt = tmp_dir.join(format!("game-{}", i));
        let path_workdir = tmp_dir.join(format!("work-{}", i));
        let path_prof = PATH_PARTY.join("profiles").join(&instance.profname);
        let path_upperdir = path_prof.join("gamesaves").join(&gamename);

        std::fs::create_dir_all(&path_game_mnt)?;
        std::fs::create_dir_all(&path_workdir)?;

        cmd.arg("-o");
        cmd.arg(format!("lowerdir={}", path_lowerdir));
        cmd.arg("-o");
        cmd.arg(format!("upperdir={}", path_upperdir.display()));
        cmd.arg("-o");
        cmd.arg(format!("workdir={}", path_workdir.display()));
        cmd.arg(&path_game_mnt);
    }

    for cmd in &mut cmds {
        let status = cmd
            .status()
            .map_err(|_| "Fuse-overlayfs executable not found; Please install fuse-overlayfs through your distro's package manager. If you already have it installed (or are on SteamOS, where it should be pre-installed), open up an issue on the GitHub.")?;
        if !status.success() {
            return Err("fuse-overlayfs mount failed.".into());
        }
    }

    Ok(())
}






mod gamescope_pipewire_wrapper {
    use wayland_client;
    use wayland_client::protocol::*;

    pub mod __interfaces {
        wayland_scanner::generate_interfaces!(
            "./src/gamescope-pipewire.xml"
        );
    }
    use self::__interfaces::*;

    wayland_scanner::generate_client_code!(
        "./src/gamescope-pipewire.xml"
    );
}
use gamescope_pipewire_wrapper::gamescope_pipewire::{GamescopePipewire, self};

mod gamescope_input_wrapper {
    use wayland_client;
    use wayland_client::protocol::*;

    pub mod __interfaces {
        wayland_scanner::generate_interfaces!(
            "./src/gamescope-input.xml"
        );
    }
    use self::__interfaces::*;

    wayland_scanner::generate_client_code!(
        "./src/gamescope-input.xml"
    );
}
use gamescope_input_wrapper::gamescope_input::{GamescopeInput, self};



use wayland_client::protocol::wl_registry;
use wayland_client::{Connection, Dispatch, Proxy};

struct GamescopeWaylandState {
    pipewire_interface: Option<GamescopePipewire>,
    input_interface: Option<GamescopeInput>,
    has_data_to_send: bool,

    pub pipewire_node: Option<u32>,
    pub latest_output_size: Rect,

    // Warning, can hardlock if used in callback! I dont like this arc, but I will need to figure out later.
    event_queue: Arc<Mutex<wayland_client::EventQueue<GamescopeWaylandState>>>,
}

impl Dispatch<wl_registry::WlRegistry, ()> for GamescopeWaylandState {
    fn event(
        state: &mut GamescopeWaylandState,
        registry: &wl_registry::WlRegistry,
        event: wl_registry::Event,
        _data: &(),
        _conn: &Connection,
        qh: &wayland_client::QueueHandle<Self>,
    ) {
        match event {
            wl_registry::Event::Global { name, interface, version } => {
                if interface == GamescopePipewire::interface().name {
                    let pipewire_interface = registry.bind::<GamescopePipewire, _, _>(name, version, qh, ());
                    state.pipewire_interface = Some(pipewire_interface);
                }

                if interface == GamescopeInput::interface().name {
                    let input_interface = registry.bind::<GamescopeInput, _, _>(name, version, qh, ());
                    state.input_interface = Some(input_interface);
                }
            }
            wl_registry::Event::GlobalRemove { name: _ } => {}
            _ => {}
        }
    }
}

impl Dispatch<GamescopePipewire, ()> for GamescopeWaylandState {
    fn event(
        state: &mut Self,
        _proxy: &GamescopePipewire,
        event: <GamescopePipewire as wayland_client::Proxy>::Event,
        _: &(),
        _: &Connection,
        qh: &wayland_client::QueueHandle<Self>,
    ) {
        match event {
            gamescope_pipewire::Event::StreamNode { node_id } => {
                // println!("Got node id: {node_id}");
                state.pipewire_node = Some(node_id);
            }
        }
    }
}

impl Dispatch<GamescopeInput, ()> for GamescopeWaylandState {
    fn event(
        state: &mut Self,
        _proxy: &GamescopeInput,
        event: <GamescopeInput as wayland_client::Proxy>::Event,
        _: &(),
        _: &Connection,
        _qh: &wayland_client::QueueHandle<Self>,
    ) {
        match event {
            gamescope_input::Event::MousePosition { x, y } => {
                // state.latest_mouse_potion = Some((x,y));
            }
            gamescope_input::Event::OutputSize { width, height } => {
                // Lossy, maybe fix later, but who is using a near 32 bit int screen?
                state.latest_output_size = Rect{ min: Pos2::ZERO, max: Pos2{x: width as f32, y: height as f32}};
            }
        }
    }
}


impl GamescopeWaylandState {
    pub fn new(path: PathBuf) -> Result<Self, String> {
        // Connect to display
        let unix_socket = std::os::unix::net::UnixStream::connect(path).map_err(|e| format!("Failed to connect to wayland socket: {e}"))?;

        let conn = Connection::from_socket(unix_socket).map_err(|e| format!("Connection error in wayland setup: {e}"))?;
        let event_queue = Arc::new(Mutex::new(conn.new_event_queue()));

        let mut state = Self {
            input_interface: None,
            pipewire_interface: None,
            has_data_to_send: false,

            pipewire_node: None,
            latest_output_size: Rect::ZERO,

            event_queue: event_queue.clone(),
        };

        let mut locked_event_queue = event_queue.lock().map_err(|e| format!("Failed to lock event_queue: {e}"))?;
        let qhandle = locked_event_queue.handle();
        let display = conn.display();

        // Get registry
        display.get_registry(&qhandle, ());

        // Dispatch one roundtrip to query globals
        locked_event_queue.roundtrip(&mut state).map_err(|e| format!("Roundtrip failed: {e}"))?;

        // Get pipewire id in second round trip.
        locked_event_queue.roundtrip(&mut state).map_err(|e| format!("Second roundtrip failed: {e}"))?;

        Ok(state)
    }

    pub fn get_size(&mut self) -> Result<(), String> {
        let input_interface = self.input_interface.as_ref().ok_or("No input interface accessable")?;
        input_interface.get_output_size();
        self.has_data_to_send = true;

        Ok(())
    }

    pub fn send_key(&mut self, key: u32, down: bool) -> Result<(), String> {
        let input_interface = self.input_interface.as_ref().ok_or("No input interface accessable")?;

        input_interface.keyboard_key(key, down as u32);
        self.has_data_to_send = true;
        
        Ok(())
    }

    pub fn mouse_button(&mut self, button: u32, pressed: bool) -> Result<(), String> {
        let input_interface = self.input_interface.as_ref().ok_or("No input interface accessable")?;
        /* input-event-codes.h
        #define BTN_LEFT		0x110
        #define BTN_RIGHT		0x111
        #define BTN_MIDDLE		0x112
        */
        
        input_interface.mouse_button(button, pressed as u32);

        self.has_data_to_send = true;
        
        Ok(())
    }

    pub fn mouse_set(&mut self, pos: Vec2) -> Result<(), String> {
        let input_interface = self.input_interface.as_ref().ok_or("No input interface accessable")?;
        let translated_pos = self.latest_output_size.lerp_inside(pos);
        input_interface.mouse_warp(translated_pos.x as f64, translated_pos.y as f64);

        self.has_data_to_send = true;
        Ok(())
    }

    pub fn mouse_move(&mut self, delta: Vec2) -> Result<(), String> {
        if delta == Vec2::ZERO { return Ok(()); }

        let input_interface = self.input_interface.as_ref().ok_or("No input interface accessable")?;
        let translated_delta = self.latest_output_size.lerp_inside(delta);
        input_interface.mouse_motion(translated_delta.x as f64, translated_delta.y as f64);

        self.has_data_to_send = true;
        Ok(())
    }

    pub fn mouse_scroll(&mut self, scroll: egui::Vec2) -> Result<(), String> {
        let input_interface = self.input_interface.as_ref().ok_or("No input interface accessable")?;
        input_interface.mouse_scroll((scroll.x*120.) as i32, (scroll.y*120.) as i32); // Not best rounding here but should be fine

        self.has_data_to_send = true;
        Ok(())
    }

    pub fn round_trip(&mut self) -> Result<(), String> {
        if self.has_data_to_send == false { return Ok(()); };
        let event_queue = self.event_queue.clone();
        let mut locked_event_queue = event_queue.lock().map_err(|e| format!("Failed to lock event_queue: {e}"))?;
        locked_event_queue.roundtrip(self).map_err(|e| format!("Failed to process round trip: {e}"))?;
        
        self.has_data_to_send = false;
        Ok(())
    }
}





// On drop, kill gamescope to avoid isolated processes.
pub struct ManagedChild(Child);
impl Drop for ManagedChild {
    fn drop(&mut self) {
        println!("Gamescope dropped");
        let _ = self.0.kill();
    }
}


pub struct GamescopeSession {
    video_ui: PipewireVideo,
    pub child_proc: ManagedChild,
    wayland_state: GamescopeWaylandState,

    last_keys_down: HashSet<egui::Key>,
    last_pointer_pos: Option<Vec2>
}

use nix::unistd::pipe;
use nix::poll::{PollFlags, poll, PollFd};

impl GamescopeSession {
    pub fn new(
        egl: &Arc<EglApi>,
        sender: pw::channel::Sender<PipewireCommand>,
        streams: Arc<RwLock<HashMap<PipewireID, Arc<RwLock<PipewireStream>>>>>,
    ) -> Result<Self, String> {
        let (read_fd, write_fd) = pipe().map_err(|e| format!("Pipe failed: {e}"))?;

        let mut child_proc = ManagedChild(Command::new("gamescope")
            .args(["-R", &format!("/proc/self/fd/{}",write_fd.as_raw_fd().to_string())])
            .args(["--composite-cursor", "--force-windows-fullscreen", "--nested-follow-window-scale", "1"])
            .args(["--backend", "headless"])
            .args(["--", "konsole"])
            .spawn()
            .map_err(|e| format!("Failed to create gamescope child: {e}"))?);

        let wayland_response = Self::fd_get_wayland_display(read_fd);

        // Prefer logging that gamescope failed :P
        if let Some(exit_status) = 
            child_proc.0.try_wait()
            .map_err(|e| format!("Get early status failed: {e}"))? {
            return Err(format!("Child process died instantly, code: {:?}", exit_status.code()));
        }

        let wayland_display = wayland_response.map_err(|e| format!("Failed to get wldisplay: {e}"))?;


        let xdg_runtime_dir = std::env::var("XDG_RUNTIME_DIR").map_err(|e| format!("XDG env var not set: {e}"))?; // Follow as I think gamescope does.
        let wayland_socket_path = std::path::PathBuf::from(xdg_runtime_dir).join(wayland_display);

        let mut wayland_state = GamescopeWaylandState::new(wayland_socket_path)?;
        
        wayland_state.get_size()?;
        wayland_state.round_trip()?;


        let pw_target = wayland_state.pipewire_node.ok_or("Unable to get pipewire interface!")?;
        
        Ok(Self {
            video_ui: PipewireVideo::new(egl, pw_target, sender, streams).map_err(|e| format!("Failed to create video: {e}"))?,
            child_proc,
            wayland_state,

            last_keys_down: HashSet::new(),
            last_pointer_pos: None
        })
    }

    fn fd_get_wayland_display(read_fd: OwnedFd) -> Result<String, String> {
        let read_fd_clone = read_fd.try_clone().unwrap();
        let mut pollfds = [
            PollFd::new(read_fd_clone.as_fd(), PollFlags::POLLIN),
        ];

        // 2 second timeout for gamescope to start
        let timeout = nix::poll::PollTimeout::try_from(2000).map_err(|e| format!("Timeout creation failed: {e}"))?;
                
        let file_owned = std::fs::File::from(read_fd);

        // Will return 1 as the kernel sends poll updates as a set of single byte update.
        let poll_bytes_avalib = poll(&mut pollfds, timeout).map_err(|e| format!("Poll failed: {e}"))?;

        if poll_bytes_avalib==0 {
            return Err("Poll failed to read any data... Gamescope may be slow or bugged.".to_owned());
        }

        // Buffer in kernel should be full despite poll_bytes_avalib==1, so use bufreader as a wrapper for reading (avoids blocking, but reads all avalib)
        let mut reader = std::io::BufReader::new(file_owned.try_clone().unwrap());
        let readyfd_buf = reader.fill_buf().map_err(|e| format!("Failed to fill buffer from ready fd: {e}"))?;

        let readyfd_str_buf = String::from_utf8_lossy(readyfd_buf);

        // Format should be ":1 gamescope-0\n" or dprintf( readyPipeFD, "%s %s\n", root_ctx->xwayland_server->get_nested_display_name(), wlserver_get_wl_display_name() );
        let split_readyfd_buf = readyfd_str_buf
            .split_whitespace()
            .collect::<Vec<_>>();

        let wl_display = split_readyfd_buf
            .get(1)
            .ok_or_else(|| format!("Readyfd buffer format invalid: {:?}", readyfd_str_buf))?;

        return Ok(wl_display.to_string());
    }

    fn update_keys_down(&mut self, current_keys_down: &HashSet<egui::Key>) -> Result<(), String> {

        for key_down in current_keys_down.difference(&self.last_keys_down) {
            if let Some(translated_key) = egui_key_to_xkb(*key_down) {
                self.wayland_state.send_key(translated_key, true)?;
            }
        }

        for key_up in self.last_keys_down.difference(&current_keys_down) {
            if let Some(translated_key) = egui_key_to_xkb(*key_up) {
                self.wayland_state.send_key(translated_key, false)?;
            }
        }

        self.last_keys_down = current_keys_down.clone();

        Ok(())
    }

    pub fn ui(&mut self, ui: &mut egui::Ui, desired_size: egui::Vec2) -> Result<egui::Response, String> {
        let response = self.video_ui.ui(ui, desired_size);

        let current_pointer_state = response.ctx.input(|i| i.pointer.clone());
        
        let current_keys_down = ui.input(|i| i.keys_down.clone());
        
        let current_pointer_pos = 
            current_pointer_state.latest_pos()
            .filter(|pos| response.rect.contains(*pos))
            .map(|pos|
                (pos-response.rect.left_top())/(response.rect.right_bottom()-response.rect.left_top())
            );

        let current_pointer_movement = 
            current_pointer_state.delta() / 
            (response.rect.right_bottom()-response.rect.left_top());


        // Get updated size doesnt need to be called every frame, but ¯\_( *-* )_/¯
        self.wayland_state.get_size()?;

        
        // Keys may be inacurate due to egui processing, ui.input(|ui| ui.raw.events) is closer to raw I think
        if self.last_pointer_pos.is_none() && let Some(pointer_pos) = current_pointer_pos {
            // Force pos
            self.wayland_state.mouse_set(pointer_pos)?;

            self.update_keys_down(&current_keys_down)?;
        }

        if current_pointer_pos.is_none() && self.last_pointer_pos.is_some() {
            // release keyboard keys

            self.update_keys_down(&HashSet::new())?;
        }

        if current_pointer_pos.is_some() && self.last_pointer_pos.is_some() {
            self.wayland_state.mouse_move(current_pointer_movement)?;

            self.update_keys_down(&current_keys_down)?;
        }

        // I have no idea how to do non-smooth scroll.
        self.wayland_state.mouse_scroll(ui.input(|i| i.smooth_scroll_delta()))?;

        

        /* input-event-codes.h
        #define BTN_LEFT		0x110
        #define BTN_RIGHT		0x111
        #define BTN_MIDDLE		0x112
        */
        for (button, code) in [
            (egui::PointerButton::Primary,      0x110),
            (egui::PointerButton::Secondary,    0x111),
            (egui::PointerButton::Middle,       0x112),
        ] {
            if current_pointer_state.button_pressed(button) { self.wayland_state.mouse_button(code, true)?; }
            if current_pointer_state.button_released(button) { self.wayland_state.mouse_button(code, false)?; }
        }

        self.last_pointer_pos = current_pointer_pos;
        self.wayland_state.round_trip()?; // Only runs if we actually updated anything.

        Ok(response)
    }
}


const SDL_GAMECONTROLLER_IGNORE_DEVICES: &str = "0x054c/0x0df2,0x054c/0x0df2,0x045e/0x02e3,0x045e/0x0b00,0x045e/0x0b05,0x2dc8/0x6000,0x2dc8/0x6100,0x2dc8/0x6001,0x2dc8/0x6101,0x2dc8/0x6003,0x2dc8/0x6006,0x2dc8/0x6009,0x2dc8/0x6012,0x28de/0x1002,0x28de/0x1003,0x28de/0x1071,0x28de/0x1052,0x28de/0x1042,0x28de/0x1203,0x28de/0x1204,0x28de/0x1205,0x28de/0x1206,0x28de/0x1302,0x28de/0x1303,0x28de/0x1304,0x28de/0x1305,0x0f0d/0x01ab,0x0f0d/0x0196,0x28de/0x12ff,0x28de/0x12fe,0x28de/0x12fd,0x28de/0x12fc,0x28de/0x12fb,0x28de/0x12fa,0x28de/0x12f9,0x28de/0x12f8,0x28de/0x12f7,0x28de/0x12f6,0x28de/0x12f5,0x28de/0x12f4,0x28de/0x12f3,0x28de/0x12f2,0x28de/0x12f1,0x28de/0x12f0,0x0079/0x181a,0x044f/0xb315,0x044f/0xd007,0x046d/0xcad1,0x054c/0x0268,0x056e/0x200f,0x056e/0x2013,0x05b8/0x1004,0x05b8/0x1006,0x06a3/0xf622,0x0738/0x3180,0x0738/0x3250,0x0738/0x3481,0x0738/0x8180,0x0738/0x8838,0x0810/0x0001,0x0810/0x0003,0x0925/0x0005,0x0925/0x8866,0x0925/0x8888,0x0e6f/0x0109,0x0e6f/0x011e,0x0e6f/0x0128,0x0e6f/0x0214,0x0e6f/0x1314,0x0e6f/0x6302,0x0e8f/0x0008,0x0e8f/0x3075,0x0e8f/0x310d,0x0f0d/0x0009,0x0f0d/0x004d,0x0f0d/0x005f,0x0f0d/0x006a,0x0f0d/0x006e,0x0f0d/0x0085,0x0f0d/0x0086,0x0f0d/0x0088,0x0f30/0x1100,0x11ff/0x3331,0x1345/0x1000,0x1345/0x6005,0x146b/0x5500,0x1a34/0x0836,0x20bc/0x5500,0x20d6/0x576d,0x20d6/0xca6d,0x2563/0x0523,0x2563/0x0575,0x25f0/0x83c3,0x25f0/0xc121,0x2c22/0x2003,0x2c22/0x2302,0x2c22/0x2502,0x8380/0x0003,0x8888/0x0308,0x0079/0x181b,0x044f/0xd00e,0x054c/0x05c4,0x054c/0x05c5,0x054c/0x09cc,0x054c/0x0ba0,0x0738/0x8250,0x0738/0x8384,0x0738/0x8480,0x0738/0x8481,0x0c12/0x0e10,0x0c12/0x0e13,0x0c12/0x0e15,0x0c12/0x0e20,0x0c12/0x0ef6,0x0c12/0x1cf6,0x0c12/0x1e10,0x0c12/0x2e18,0x0e6f/0x0203,0x0e6f/0x0207,0x0e6f/0x020a,0x0f0d/0x0055,0x0f0d/0x005e,0x0f0d/0x0066,0x0f0d/0x0084,0x0f0d/0x0087,0x0f0d/0x008a,0x0f0d/0x009c,0x0f0d/0x00a0,0x0f0d/0x00ee,0x0f0d/0x011c,0x0f0d/0x0123,0x0f0d/0x0162,0x11c0/0x4001,0x146b/0x0d01,0x146b/0x0d02,0x146b/0x0d06,0x146b/0x0d08,0x146b/0x0d09,0x146b/0x0d10,0x146b/0x0d10,0x146b/0x0d13,0x146b/0x1103,0x1532/0x0401,0x1532/0x1000,0x1532/0x1004,0x1532/0x1007,0x1532/0x1008,0x1532/0x1009,0x1532/0x100a,0x1532/0x1100,0x20d6/0x792a,0x2c22/0x2000,0x2c22/0x2300,0x2c22/0x2500,0x3285/0x0d16,0x3285/0x0d17,0x7545/0x0104,0x9886/0x0025,0x054c/0x0ce6,0x054c/0x0df2,0x054c/0x0e5f,0x0e6f/0x0209,0x0f0d/0x0163,0x0f0d/0x0184,0x1532/0x100b,0x1532/0x100c,0x1532/0x1012,0x3285/0x0d18,0x3285/0x0d19,0x358a/0x0104,0x0079/0x18d4,0x03eb/0xff02,0x044f/0xb326,0x045e/0x028e,0x045e/0x028f,0x045e/0x0291,0x045e/0x02a0,0x045e/0x02a1,0x045e/0x02a9,0x045e/0x0719,0x046d/0xc21d,0x046d/0xc21e,0x046d/0xc21f,0x046d/0xc242,0x056e/0x2004,0x0738/0x4716,0x0738/0x4718,0x0738/0x4726,0x0738/0x4728,0x0738/0x4736,0x0738/0x4738,0x0738/0x4740,0x0738/0xb726,0x0738/0xbeef,0x0738/0xcb02,0x0738/0xcb03,0x0738/0xf738,0x0955/0x7210,0x0955/0xb400,0x0b05/0x1b4c,0x0e6f/0x0105,0x0e6f/0x0113,0x0e6f/0x011f,0x0e6f/0x0125,0x0e6f/0x0127,0x0e6f/0x0131,0x0e6f/0x0133,0x0e6f/0x0143,0x0e6f/0x0147,0x0e6f/0x0201,0x0e6f/0x0213,0x0e6f/0x021f,0x0e6f/0x0301,0x0e6f/0x0313,0x0e6f/0x0314,0x0e6f/0x0401,0x0e6f/0x0413,0x0e6f/0x0501,0x0e6f/0xf900,0x0f0d/0x000a,0x0f0d/0x000c,0x0f0d/0x000d,0x0f0d/0x0016,0x0f0d/0x001b,0x0f0d/0x008c,0x0f0d/0x00db,0x0f0d/0x011e,0x1038/0x1430,0x1038/0x1431,0x1038/0xb360,0x11c9/0x55f0,0x12ab/0x0004,0x12ab/0x0301,0x12ab/0x0303,0x1430/0x02a0,0x1430/0x4748,0x1430/0xf801,0x146b/0x0601,0x15e4/0x3f00,0x15e4/0x3f0a,0x15e4/0x3f10,0x162e/0xbeef,0x1689/0xfd00,0x1689/0xfd01,0x1689/0xfe00,0x1949/0x041a,0x1bad/0x0002,0x1bad/0x0003,0x1bad/0xf016,0x1bad/0xf018,0x1bad/0xf019,0x1bad/0xf021,0x1bad/0xf023,0x1bad/0xf025,0x1bad/0xf027,0x1bad/0xf028,0x1bad/0xf02e,0x1bad/0xf036,0x1bad/0xf038,0x1bad/0xf039,0x1bad/0xf03a,0x1bad/0xf03d,0x1bad/0xf03e,0x1bad/0xf03f,0x1bad/0xf042,0x1bad/0xf080,0x1bad/0xf501,0x1bad/0xf502,0x1bad/0xf503,0x1bad/0xf504,0x1bad/0xf505,0x1bad/0xf506,0x1bad/0xf900,0x1bad/0xf901,0x1bad/0xf902,0x1bad/0xf903,0x1bad/0xf904,0x1bad/0xf906,0x1bad/0xfa01,0x1bad/0xfd00,0x1bad/0xfd01,0x24c6/0x5000,0x24c6/0x5300,0x24c6/0x5303,0x24c6/0x530a,0x24c6/0x531a,0x24c6/0x5397,0x24c6/0x5500,0x24c6/0x5501,0x24c6/0x5502,0x24c6/0x5503,0x24c6/0x5506,0x24c6/0x550d,0x24c6/0x550e,0x24c6/0x5508,0x24c6/0x5510,0x24c6/0x5b00,0x24c6/0x5b02,0x24c6/0x5b03,0x24c6/0x5d04,0x24c6/0xfafa,0x24c6/0xfafb,0x24c6/0xfafc,0x24c6/0xfafd,0x24c6/0xfafe,0x03f0/0x0495,0x044f/0xd012,0x045e/0x02d1,0x045e/0x02dd,0x045e/0x02e0,0x045e/0x02e3,0x045e/0x02ea,0x045e/0x02fd,0x045e/0x02ff,0x045e/0x0b00,0x045e/0x0b05,0x045e/0x0b0a,0x045e/0x0b0c,0x045e/0x0b12,0x045e/0x0b13,0x045e/0x0b20,0x045e/0x0b21,0x045e/0x0b22,0x0738/0x4a01,0x0e6f/0x0139,0x0e6f/0x013b,0x0e6f/0x013a,0x0e6f/0x0145,0x0e6f/0x0146,0x0e6f/0x015b,0x0e6f/0x015c,0x0e6f/0x015d,0x0e6f/0x015f,0x0e6f/0x0160,0x0e6f/0x0161,0x0e6f/0x0162,0x0e6f/0x0163,0x0e6f/0x0164,0x0e6f/0x0165,0x0e6f/0x0166,0x0e6f/0x0167,0x0e6f/0x0205,0x0e6f/0x0206,0x0e6f/0x0246,0x0e6f/0x0261,0x0e6f/0x0262,0x0e6f/0x02a0,0x0e6f/0x02a1,0x0e6f/0x02a2,0x0e6f/0x02a3,0x0e6f/0x02a4,0x0e6f/0x02a5,0x0e6f/0x02a6,0x0e6f/0x02a7,0x0e6f/0x02a8,0x0e6f/0x02a9,0x0e6f/0x02aa,0x0e6f/0x02ab,0x0e6f/0x02ac,0x0e6f/0x02ad,0x0e6f/0x02ae,0x0e6f/0x02af,0x0e6f/0x02b0,0x0e6f/0x02b1,0x0e6f/0x02b3,0x0e6f/0x02b5,0x0e6f/0x02b6,0x0e6f/0x02bd,0x0e6f/0x02be,0x0e6f/0x02bf,0x0e6f/0x02c0,0x0e6f/0x02c1,0x0e6f/0x02c2,0x0e6f/0x02c3,0x0e6f/0x02c4,0x0e6f/0x02c5,0x0e6f/0x02c6,0x0e6f/0x02c7,0x0e6f/0x02c8,0x0e6f/0x02c9,0x0e6f/0x02ca,0x0e6f/0x02cb,0x0e6f/0x02cd,0x0e6f/0x02ce,0x0e6f/0x02cf,0x0e6f/0x02d5,0x0e6f/0x0346,0x0e6f/0x0446,0x0e6f/0x02da,0x0e6f/0x02d6,0x0e6f/0x02d9,0x0f0d/0x0063,0x0f0d/0x0067,0x0f0d/0x0078,0x0f0d/0x00c5,0x0f0d/0x0150,0x10f5/0x7009,0x10f5/0x7013,0x1532/0x0a00,0x1532/0x0a03,0x1532/0x0a14,0x1532/0x0a15,0x20d6/0x2001,0x20d6/0x2002,0x20d6/0x2003,0x20d6/0x2004,0x20d6/0x2005,0x20d6/0x2006,0x20d6/0x2009,0x20d6/0x200a,0x20d6/0x200b,0x20d6/0x200c,0x20d6/0x200d,0x20d6/0x200e,0x20d6/0x200f,0x20d6/0x2011,0x20d6/0x2012,0x20d6/0x2015,0x20d6/0x2016,0x20d6/0x2017,0x20d6/0x2018,0x20d6/0x2019,0x20d6/0x201a,0x20d6/0x4001,0x20d6/0x4002,0x20d6/0x890b,0x24c6/0x541a,0x24c6/0x542a,0x24c6/0x543a,0x24c6/0x551a,0x24c6/0x561a,0x24c6/0x581a,0x24c6/0x591a,0x24c6/0x592a,0x24c6/0x791a,0x2dc8/0x2002,0x2dc8/0x3106,0x2e24/0x0652,0x2e24/0x1618,0x2e24/0x1688,0x146b/0x0611,0x0000/0x0000,0x045e/0x02a2,0x0e6f/0x1414,0x0e6f/0x0159,0x24c6/0xfaff,0x0f0d/0x006d,0x0f0d/0x00a4,0x0079/0x1832,0x0079/0x187f,0x0079/0x1883,0x03eb/0xff01,0x0c12/0x0ef8,0x046d/0x1000,0x11ff/0x0511,0x1345/0x6006,0x056e/0x2012,0x146b/0x0602,0x0f0d/0x00ae,0x046d/0x0401,0x046d/0x0301,0x046d/0xcaa3,0x046d/0xc261,0x046d/0x0291,0x0079/0x18d3,0x0f0d/0x00b1,0x0001/0x0001,0x0079/0x188e,0x0079/0x187c,0x0079/0x189c,0x0079/0x1874,0x2f24/0x0050,0x2f24/0x002e,0x2f24/0x0091,0x1430/0x0719,0x0f0d/0x00ed,0x0f0d/0x00c0,0x0e6f/0x0152,0x046d/0x1007,0x0e6f/0x02b8,0x0079/0x18a1,0x0000/0x6686,0x12ab/0x0304,0x1430/0x0291,0x1430/0x02a9,0x1430/0x070b,0x1bad/0x028e,0x1bad/0x02a0,0x1bad/0x5500,0x20ab/0x55ef,0x24c6/0x5509,0x2516/0x0069,0x25b1/0x0360,0x2c22/0x2203,0x2f24/0x0011,0x2f24/0x0053,0x2f24/0x00b7,0x046d/0x0000,0x046d/0x1004,0x046d/0x1008,0x046d/0xf301,0x0738/0x02a0,0x0738/0x7263,0x0738/0xb738,0x0738/0xcb29,0x0738/0xf401,0x0079/0x18c2,0x0079/0x18c8,0x0079/0x18cf,0x0c12/0x0e17,0x0c12/0x0e1c,0x0c12/0x0e22,0x0c12/0x0e30,0xd2d2/0xd2d2,0x0d62/0x9a1a,0x0d62/0x9a1b,0x0e00/0x0e00,0x0e6f/0x012a,0x0e6f/0x02b2,0x0f0d/0x0097,0x0f0d/0x00ba,0x0f0d/0x00d8,0x0fff/0x02a1,0x045e/0x0867,0x16d0/0x0f3f,0x2f24/0x008f,0x0e6f/0xf501,0x057e/0x2006,0x057e/0x2067,0x057e/0x2007,0x057e/0x2066,0x057e/0x2008,0x057e/0x2068,0x057e/0x2009,0x057e/0x2069,0x0f0d/0x00c1,0x0f0d/0x0092,0x0f0d/0x00f6,0x0e6f/0x0180,0x0e6f/0x0181,0x0e6f/0x0184,0x0e6f/0x0185,0x0e6f/0x0186,0x0e6f/0x0187,0x0e6f/0x0188,0x0e6f/0x018c,0x0f0d/0x00aa,0x20d6/0xa711,0x20d6/0xa712,0x20d6/0xa713,0x20d6/0xa714,0x20d6/0xa715,0x20d6/0xa716,0x20d6/0xa718,0x33dd/0x0001,0x33dd/0x0002,0x33dd/0x0003,0x0f0d/0x00f0,0x0000/0x11fb,0x28de/0x1101,0x28de/0x1102,0x28de/0x1105,0x28de/0x1106,0x28de/0x1142,0x28de/0x1201,0x28de/0x1202,0x28de/0x1205,0x28de/0x1302,0x28de/0x1303,0x28de/0x1304,0x2dc8/0x9000,0x2dc8/0x3810,0x2dc8/0x0651,0x2dc8/0x9020,0x2dc8/0x9015,0x2dc8/0x2865,0x1235/0xab12,0x2002/0x9000,0x2dc8/0x9001,0x3820/0x0009,0x2dc8/0x3820,0x2dc8/0x2000,0x2dc8/0x2000,0x2810/0x0009,0x2dc8/0x2830,0x2dc8/0x6002,0x2dc8/0x6102,0x1235/0xab20,0x2820/0x0009,0x2dc8/0x301b,0x2dc8/0x3011,0x2dc8/0x3013,0x2dc8/0x9018,0x2dc8/0x3230,0x05a0/0x3232,0x05a0/0x3232,0x2dc8/0x3100,0x2dc8/0x9012,0x2dc8/0x2862,0x0b05/0x4500,0x0b05/0x4500,0x0b05/0x7905,0x0b05/0x7906,0x0010/0x0082,0x1949/0x0402,0x1949/0x0419,0x0171/0x0419,0x0079/0x1830,0x3250/0x1001,0x3250/0x1001,0x3250/0x1002,0x3250/0x1002,0x24c6/0x891b,0x0c12/0x0ef7,0x04b4/0x010a,0xffff/0xffff,0x20e8/0x5860,0x0926/0x8888,0x0e6f/0x0130,0x0079/0x0011,0x1a34/0xf705,0x1949/0x0402,0x3537/0x1097,0x05ac/0x061a,0x25f0/0x83c1,0x18d1/0x9400,0x18d1/0x9400,0x0428/0x4001,0x0e8f/0x1006,0x0e8f/0x0012,0x0f0d/0x0010,0x0f0d/0x0022,0x0f0d/0x006b,0xdead/0xbeef,0x14d8/0x6208,0x0e8f/0x3013,0x04d8/0x0082,0x05fd/0x3000,0x1949/0x0402,0x056e/0x2003,0x0f30/0x0110,0x22ba/0x1020,0x046d/0xc219,0x046d/0xc216,0x046d/0xc216,0x046d/0xc219,0x046d/0xc218,0x046d/0xc211,0x24c6/0x892b,0x24c6/0x892a,0x24c6/0x891a,0x0738/0x5266,0x0738/0x3384,0x0738/0x3480,0x0738/0x8818,0x0078/0x0006,0x045e/0x000e,0x045e/0x0285,0x045e/0x0289,0x045e/0x0289,0x20d6/0x0dad,0x146b/0x0c01,0x0810/0xe501,0x0955/0x7214,0x0955/0x7214,0x124b/0x4d01,0x1345/0x3008,0x0079/0x1843,0x0079/0x1844,0x057e/0x2019,0x057e/0x2019,0x057e/0x201e,0x057e/0x2017,0x057e/0x2017,0x057e/0x2017,0x057e/0x0306,0x057e/0x0330,0x057e/0x0306,0x050d/0x0803,0x2836/0x0001,0x2836/0x0001,0x045e/0x0202,0x11ff/0x3341,0x0e8f/0x0003,0x054c/0x0cda,0x0f30/0x1112,0x2c22/0x2012,0x2c22/0x2010,0x1532/0x0402,0x1532/0x0705,0x1532/0x0900,0x1532/0x0900,0xf000/0x0003,0x0079/0x0011,0x1a34/0x0809,0x7545/0x1122,0x06a3/0xf623,0x06a3/0xff0c,0x06a3/0x040c,0x06a3/0x0109,0x06a3/0x040b,0x06a3/0xf518,0x16c0/0x0487,0x28de/0x11fc,0x0111/0x1431,0x0111/0x1419,0x6666/0x8804,0xf000/0x00f1,0x044f/0xb320,0x044f/0xb323,0x044f/0xb300,0x044f/0xd009,0x044f/0xd008,0x12bd/0xd015,0x14d8/0xcd07,0x0079/0x0011,0x05ac/0x3232,0x0c45/0x4320,0x2717/0x3144,0x16c0/0x05e1,0x6666/0x0667,0x0583/0x2060,0x07b5/0x0315,0x289b/0x0080,0x289b/0x0003,0x289b/0x0060,";


// modified from (because we have egui key instead of physical key see: NativeKeyCode::Xkb() and how egui "physical key" is derived) /winit-0.30.13/src/platform_impl/linux/common/xkb/keymap.rs as well as /egui-winit/src/lib.rs for "key" type conversion
// fn on_keyboard_input(&mut self, event: &winit::event::KeyEvent) {} is the most important for us.
// This is from pub fn physicalkey_to_scancode(key: PhysicalKey) -> Option<u32>, but translated to egui key as we cant get back to keycode easily.

// converts to linux uapi keycodes so we can send to gamescope, should match https://github.com/torvalds/linux/blob/master/include/uapi/linux/input-event-codes.h as much as we can
// Those in comments are ones in hte phisical key winit enum that dont exist here. 
pub fn egui_key_to_xkb(input_key: egui::Key) -> Option<u32> {
    use egui::Key;
    println!("Key: {input_key:?}");

    // Not sure why these are translated..? egui shouldnt be translating these, so we just need to get the "unshifted" version of them.
    let input_key = match input_key {
        Key::Exclamationmark => Key::Num1,
        Key::Plus => Key::Equals,
        Key::Pipe => Key::Backslash,
        Key::OpenCurlyBracket => Key::OpenBracket,
        Key::CloseCurlyBracket => Key::CloseBracket,
        Key::Colon => Key::Semicolon,
        Key::Questionmark => Key::Slash,
        Key::Copy => Key::C,
        Key::Paste => Key::P,
        Key::Cut => Key::X,

        other => other
    };


    match input_key {
        Key::Escape => Some(1),
        Key::Num1 => Some(2),
        Key::Num2 => Some(3),
        Key::Num3 => Some(4),
        Key::Num4 => Some(5),
        Key::Num5 => Some(6),
        Key::Num6 => Some(7),
        Key::Num7 => Some(8),
        Key::Num8 => Some(9),
        Key::Num9 => Some(10),
        Key::Num0 => Some(11),
        Key::Minus => Some(12),
        Key::Equals => Some(13),
        Key::Backspace => Some(14),
        Key::Tab => Some(15),
        Key::Q => Some(16),
        Key::W => Some(17),
        Key::E => Some(18),
        Key::R => Some(19),
        Key::T => Some(20),
        Key::Y => Some(21),
        Key::U => Some(22),
        Key::I => Some(23),
        Key::O => Some(24),
        Key::P => Some(25),
        Key::OpenBracket => Some(26),
        Key::CloseBracket => Some(27),
        Key::Enter => Some(28),
        Key::ControlLeft => Some(29),
        Key::A => Some(30),
        Key::S => Some(31),
        Key::D => Some(32),
        Key::F => Some(33),
        Key::G => Some(34),
        Key::H => Some(35),
        Key::J => Some(36),
        Key::K => Some(37),
        Key::L => Some(38),
        Key::Semicolon => Some(39),
        Key::Quote => Some(40),
        Key::Backtick => Some(41),
        Key::ShiftLeft => Some(42),
        Key::Backslash => Some(43),
        Key::Z => Some(44),
        Key::X => Some(45),
        Key::C => Some(46),
        Key::V => Some(47),
        Key::B => Some(48),
        Key::N => Some(49),
        Key::M => Some(50),
        Key::Comma => Some(51),
        Key::Period => Some(52),
        Key::Slash => Some(53),
        Key::ShiftRight => Some(54),
        // Key::NumpadMultiply => Some(55),
        Key::AltLeft => Some(56),
        Key::Space => Some(57),
        // Key::CapsLock => Some(58),
        Key::F1 => Some(59),
        Key::F2 => Some(60),
        Key::F3 => Some(61),
        Key::F4 => Some(62),
        Key::F5 => Some(63),
        Key::F6 => Some(64),
        Key::F7 => Some(65),
        Key::F8 => Some(66),
        Key::F9 => Some(67),
        Key::F10 => Some(68),
        // Key::NumLock => Some(69),
        // Key::ScrollLock => Some(70),
        // Key::Numpad7 => Some(71),
        // Key::Numpad8 => Some(72),
        // Key::Numpad9 => Some(73),
        // Key::NumpadSubtract => Some(74),
        // Key::Numpad4 => Some(75),
        // Key::Numpad5 => Some(76),
        // Key::Numpad6 => Some(77),
        // Key::NumpadAdd => Some(78),
        // Key::Numpad1 => Some(79),
        // Key::Numpad2 => Some(80),
        // Key::Numpad3 => Some(81),
        // Key::Numpad0 => Some(82),
        // Key::NumpadDecimal => Some(83),
        // Key::Lang5 => Some(85),
        Key::IntlBackslash => Some(86),
        Key::F11 => Some(87),
        Key::F12 => Some(88),
        // Key::IntlRo => Some(89),
        // Key::Lang3 => Some(90),
        // Key::Lang4 => Some(91),
        // Key::Convert => Some(92),
        // Key::KanaMode => Some(93),
        // Key::NonConvert => Some(94),
        // Key::NumpadEnter => Some(96),
        Key::ControlRight => Some(97),
        // Key::NumpadDivide => Some(98),
        // Key::PrintScreen => Some(99),
        Key::AltRight => Some(100),
        Key::Home => Some(102),
        Key::ArrowUp => Some(103),
        Key::PageUp => Some(104),
        Key::ArrowLeft => Some(105),
        Key::ArrowRight => Some(106),
        Key::End => Some(107),
        Key::ArrowDown => Some(108),
        Key::PageDown => Some(109),
        Key::Insert => Some(110),
        Key::Delete => Some(111),
        // Key::AudioVolumeMute => Some(113),
        // Key::AudioVolumeDown => Some(114),
        // Key::AudioVolumeUp => Some(115),
        // Key::NumpadEqual => Some(117),
        // Key::Pause => Some(119),
        // Key::NumpadComma => Some(121),
        // Key::Lang1 => Some(122),
        // Key::Lang2 => Some(123),
        // Key::IntlYen => Some(124),
        Key::SuperLeft => Some(125),
        Key::SuperRight => Some(126),
        // Key::ContextMenu => Some(127),
        // Key::MediaTrackNext => Some(163),
        // Key::MediaPlayPause => Some(164),
        // Key::MediaTrackPrevious => Some(165),
        // Key::MediaStop => Some(166),
        Key::F13 => Some(183),
        Key::F14 => Some(184),
        Key::F15 => Some(185),
        Key::F16 => Some(186),
        Key::F17 => Some(187),
        Key::F18 => Some(188),
        Key::F19 => Some(189),
        Key::F20 => Some(190),
        Key::F21 => Some(191),
        Key::F22 => Some(192),
        Key::F23 => Some(193),
        Key::F24 => Some(194),


        _ => None,
    }
}
/*
!+{}|:?

*/