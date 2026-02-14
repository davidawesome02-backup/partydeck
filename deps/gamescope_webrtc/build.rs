fn main() {
    let mut cfg = cmake::Config::new("./src");

    let dst = cfg.build();

    println!("cargo:rustc-link-search=native={}/lib", dst.display());
    println!("cargo:rustc-link-lib=static=gamescope-webrtc-lib");

    println!("cargo:rustc-link-lib=stdc++");
    

    println!("cargo:rustc-link-lib=pipewire-0.3");
    // Threads::Threads
    println!("cargo:rustc-link-lib=pthread");

    // LibDataChannel::LibDataChannel
    println!("cargo:rustc-link-lib=datachannel");

    println!("cargo:rustc-link-lib=avformat");
    println!("cargo:rustc-link-lib=avcodec");
    println!("cargo:rustc-link-lib=avutil");
    println!("cargo:rustc-link-lib=swscale");
}
