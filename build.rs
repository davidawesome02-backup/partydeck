use search_path;
use std::path::PathBuf;


macro_rules! p {
    ($($tokens:tt)*) => {
        println!("cargo::warning={}",format!($($tokens)*))
    }
}

fn main() {
    p!("Build running");
    let local_binaries = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap()).join("deps").join("built");
    std::fs::create_dir_all(&local_binaries).unwrap();

    let mut path_search = search_path::SearchPath::new("PATH").unwrap();
    path_search.prepend(local_binaries);
    

    let opt_level = std::env::var("OPT_LEVEL").unwrap(); // Opt level to build at (main thing is that at 0 is dev, 3 is release)
    let _compile_binaries_debug = opt_level=="0".to_string();
    if std::env::var("CARGO_FEATURE_BUILD_RIVER").is_ok() {
        p!("RIVER BUILT");
    }
    if std::env::var("CARGO_FEATURE_BUILD_GAMESCOPE").is_ok() {
        p!("GAMESCOPE BUILT");
    }
    if std::env::var("CARGO_FEATURE_BUILD_GBE").is_ok() {
        p!("GBE BUILT");
    }


    // Mark that these features may be used in the main program
    println!("cargo::rustc-check-cfg=cfg(HAS_RIVER_DATA)");
    println!("cargo::rustc-check-cfg=cfg(HAS_GAMESCOPE_DATA)");

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
    }
    if std::env::var("CARGO_FEATURE_EMBED_GBE").is_ok() {
        // Changed reciently skipping!
    }
}