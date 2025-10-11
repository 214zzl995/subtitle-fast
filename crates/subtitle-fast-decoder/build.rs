use std::env;

#[cfg(target_os = "macos")]
fn build_videotoolbox_bridge() {
    if env::var("CARGO_FEATURE_BACKEND_VIDEOTOOLBOX").is_err() {
        return;
    }

    println!("cargo:rerun-if-changed=src/backends/videotoolbox_bridge.m");
    println!("cargo:rerun-if-env-changed=MACOSX_DEPLOYMENT_TARGET");

    let mut build = cc::Build::new();
    build.file("src/backends/videotoolbox_bridge.m");
    build.flag("-fobjc-arc");
    build.compile("videotoolbox_bridge");

    println!("cargo:rustc-link-lib=framework=AVFoundation");
    println!("cargo:rustc-link-lib=framework=CoreMedia");
    println!("cargo:rustc-link-lib=framework=CoreVideo");
    println!("cargo:rustc-link-lib=framework=Foundation");
    println!("cargo:rustc-link-lib=framework=CoreFoundation");
}

#[cfg(not(target_os = "macos"))]
fn build_videotoolbox_bridge() {}

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    build_videotoolbox_bridge();
}
