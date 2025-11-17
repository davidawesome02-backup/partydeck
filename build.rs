use search_path;
use std::{env, path::PathBuf, process::Command};


macro_rules! p {
    ($($tokens:tt)*) => {
        println!("cargo::warning={}",format!($($tokens)*))
    }
}

fn main() {
    p!("Build running");
    let main_dir = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    let local_binaries = main_dir.join("deps").join("built");
    std::fs::create_dir_all(&local_binaries).unwrap();

    let mut path_search = search_path::SearchPath::new("PATH").unwrap();
    path_search.prepend(local_binaries.clone());
    

    let opt_level = std::env::var("OPT_LEVEL").unwrap(); // Opt level to build at (main thing is that at 0 is dev, 3 is release)
    let compile_binaries_debug = opt_level=="0".to_string();
    if std::env::var("CARGO_FEATURE_BUILD_RIVER").is_ok() {
        p!("RIVER HAS NO BUILD SCRIPT CURRENTLY; USE PACKAGE MANAGER VERSION!");
    }
    if std::env::var("CARGO_FEATURE_BUILD_GAMESCOPE").is_ok() {
        p!("GAMESCOPE BUILDING");

        env::set_current_dir(main_dir.join("deps").join("gamescope")).unwrap();
        let mut meson_setup_process = Command::new("meson");
        meson_setup_process.args(["setup", "build/"]);
        if compile_binaries_debug {
            meson_setup_process.arg("--buildtype=debug");
        } else {
            meson_setup_process.arg("--buildtype=release");
        }
        
        if !meson_setup_process.status().expect("Failed to get meson status").success() {
            panic!("Meson failed, see above; build partydeck exited!");
        }

        let mut ninja_compile_command = Command::new("ninja");
        ninja_compile_command.args(["-C", "build/"]);

        if !ninja_compile_command.status().expect("Failed to get ninja status").success() {
            panic!("Ninja build gamescope failed, see above; build partydeck exited!");
        }

        std::fs::copy(
            main_dir.join("deps/gamescope/build/src/gamescope"),
            &local_binaries.join("gamescope")
        ).expect("Failed to copy gamescope to build dir");

        std::fs::copy(
            main_dir.join("deps/gamescope/build/src/gamescopereaper"),
            &local_binaries.join("gamescopereaper")
        ).expect("Failed to copy gamescopereaper to build dir");

        p!("Built gamescope!");
    }
    if std::env::var("CARGO_FEATURE_BUILD_GBE").is_ok() {
        p!("GBE BUILD NOT IMPLEMENTED YET!");
    }


    // Mark that these features may be used in the main program
    println!("cargo::rustc-check-cfg=cfg(HAS_RIVER_DATA)");
    println!("cargo::rustc-check-cfg=cfg(HAS_GAMESCOPE_DATA)");
    println!("cargo::rustc-check-cfg=cfg(HAS_GAMESCOPEREAPER_DATA)");

    if std::env::var("CARGO_FEATURE_EMBED_RIVER").is_ok() {
        p!("RIVER EMBEDED");

        let river_path = path_search.find_file(&PathBuf::from("river")).expect("Failed to find river");

        println!("cargo::rustc-env=RIVER_DATA_PATH={}", river_path.display());
        println!("cargo::rustc-cfg=HAS_RIVER_DATA");
    }
    if std::env::var("CARGO_FEATURE_EMBED_GAMESCOPE").is_ok() {
        p!("GAMESCOPE EMBEDED");

        let gamescope_path = path_search.find_file(&PathBuf::from("gamescope")).expect("Failed to find gamescope");

        println!("cargo::rustc-env=GAMESCOPE_DATA_PATH={}", gamescope_path.display());
        println!("cargo::rustc-cfg=HAS_GAMESCOPE_DATA");

        let gamescopereaper_path = path_search.find_file(&PathBuf::from("gamescopereaper")).expect("Failed to find gamescopereaper");

        println!("cargo::rustc-env=GAMESCOPEREAPER_DATA_PATH={}", gamescopereaper_path.display());
        println!("cargo::rustc-cfg=HAS_GAMESCOPEREAPER_DATA");
    }
    if std::env::var("CARGO_FEATURE_EMBED_GBE").is_ok() {
        // Changed reciently skipping!
        p!("GBE EMBED NOT IMPLEMENTED YET")
    }
}