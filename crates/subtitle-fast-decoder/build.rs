#[cfg(any(target_os = "macos", target_os = "windows"))]
use std::env;

#[cfg(target_os = "windows")]
use std::sync::Once;

#[cfg(target_os = "macos")]
fn build_videotoolbox_bridge() {
    if env::var("CARGO_FEATURE_BACKEND_VIDEOTOOLBOX").is_err() {
        return;
    }

    println!("cargo:rerun-if-changed=src/backends/videotoolbox/videotoolbox_bridge.m");
    println!("cargo:rerun-if-env-changed=MACOSX_DEPLOYMENT_TARGET");

    let mut build = cc::Build::new();
    build.file("src/backends/videotoolbox/videotoolbox_bridge.m");
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

#[cfg(target_os = "windows")]
fn build_mft_bridge() {
    if env::var("CARGO_FEATURE_BACKEND_MFT").is_err() {
        return;
    }

    println!("cargo:rerun-if-changed=src/backends/mft/mft_bridge.cpp");

    let mut build = cc::Build::new();
    build.file("src/backends/mft/mft_bridge.cpp");
    build.cpp(true);
    build.flag_if_supported("/std:c++17");
    build.flag_if_supported("-std=c++17");
    build.compile("mft_bridge");

    link_windows_media_deps();
}

#[cfg(target_os = "windows")]
fn build_dxva_bridge() {
    if env::var("CARGO_FEATURE_BACKEND_DXVA").is_err() {
        return;
    }

    println!("cargo:rerun-if-changed=src/backends/dxva/dxva_bridge.cpp");

    let mut build = cc::Build::new();
    build.file("src/backends/dxva/dxva_bridge.cpp");
    build.cpp(true);
    build.flag_if_supported("/std:c++17");
    build.flag_if_supported("-std=c++17");
    build.compile("dxva_bridge");

    link_windows_media_deps();
}

#[cfg(target_os = "windows")]
fn link_windows_media_deps() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        for lib in [
            "mfplat",
            "mf",
            "mfreadwrite",
            "mfuuid",
            "ole32",
            "d3d11",
            "dxgi",
            "bcrypt", 
        ] {
            println!("cargo:rustc-link-lib={lib}");
        }
    });
}

#[cfg(not(target_os = "windows"))]
fn build_mft_bridge() {}

#[cfg(not(target_os = "windows"))]
fn build_dxva_bridge() {}

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    build_videotoolbox_bridge();
    build_mft_bridge();
    build_dxva_bridge();
}
