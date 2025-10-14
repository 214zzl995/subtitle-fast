#[cfg(target_os = "macos")]
use std::env;

#[cfg(target_os = "macos")]
fn build_vision_bridge() {
    if env::var("CARGO_FEATURE_ENGINE_VISION").is_err() {
        return;
    }

    println!("cargo:rerun-if-changed=src/backends/vision/vision_bridge.m");
    println!("cargo:rerun-if-env-changed=MACOSX_DEPLOYMENT_TARGET");

    let mut build = cc::Build::new();
    build.file("src/backends/vision/vision_bridge.m");
    build.flag("-fobjc-arc");
    build.compile("vision_ocr_bridge");

    println!("cargo:rustc-link-lib=framework=Vision");
    println!("cargo:rustc-link-lib=framework=CoreGraphics");
    println!("cargo:rustc-link-lib=framework=CoreFoundation");
    println!("cargo:rustc-link-lib=framework=Foundation");
}

#[cfg(not(target_os = "macos"))]
fn build_vision_bridge() {}

#[cfg(target_os = "macos")]
fn build_mlx_bridge() {
    if env::var("CARGO_FEATURE_ENGINE_MLX_VLM").is_err() {
        return;
    }

    println!("cargo:rerun-if-changed=src/backends/mlx/mlx_vlm_bridge.m");

    let mut build = cc::Build::new();
    build.file("src/backends/mlx/mlx_vlm_bridge.m");
    build.compile("mlx_vlm_bridge");
}

#[cfg(not(target_os = "macos"))]
fn build_mlx_bridge() {}

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    build_vision_bridge();
    build_mlx_bridge();
}
