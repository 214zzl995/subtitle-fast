#[cfg(target_os = "macos")]
fn build_vision_bridge() {
    println!("cargo:rerun-if-changed=src/macos/vision_ocr_bridge.m");
    println!("cargo:rerun-if-env-changed=MACOSX_DEPLOYMENT_TARGET");

    let mut build = cc::Build::new();
    build.file("src/macos/vision_ocr_bridge.m");
    build.flag("-fobjc-arc");
    build.compile("vision_ocr_bridge");

    println!("cargo:rustc-link-lib=framework=Vision");
    println!("cargo:rustc-link-lib=framework=CoreGraphics");
    println!("cargo:rustc-link-lib=framework=CoreFoundation");
    println!("cargo:rustc-link-lib=framework=Foundation");
}

#[cfg(not(target_os = "macos"))]
fn build_vision_bridge() {}

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    build_vision_bridge();
}
