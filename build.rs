macro_rules! p {
    ($($tokens:tt)*) => {
        println!("cargo::warning={}",format!($($tokens)*))
    };
}

fn main() {
    p!("Build running");
    
    
    if std::env::var("CARGO_FEATURE_BUILD_RIVER").is_ok() {
        p!("RIVER BUILT")
    }
    if std::env::var("CARGO_FEATURE_BUILD_GAMESCOPE").is_ok() {
        p!("GAMESCOPE BUILT")
    }
    if std::env::var("CARGO_FEATURE_BUILD_GBE").is_ok() {
        p!("GBE BUILT")
    }


    if std::env::var("CARGO_FEATURE_EMBED_RIVER").is_ok() {
        p!("RIVER EMBEDED")
    }
    if std::env::var("CARGO_FEATURE_EMBED_GAMESCOPE").is_ok() {
        p!("GAMESCOPE EMBEDED")
    }
    if std::env::var("CARGO_FEATURE_EMBED_GBE").is_ok() {
        p!("GBE EMBEDED")
    }
}