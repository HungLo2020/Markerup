fn main() {
    generate_app_icon();
    tauri_build::build();

    let target_is_ios = std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("ios");
    let host_is_apple = std::env::var("HOST").is_ok_and(|host| host.contains("apple"));
    if target_is_ios && host_is_apple {
        println!("cargo:rustc-link-lib=framework=UniformTypeIdentifiers");
        println!("cargo:rustc-link-lib=framework=Security");
        cc::Build::new()
            .file("ios/MarkerupIOSBridge.m")
            .flag("-fobjc-arc")
            .compile("markerup_ios_bridge");
    } else if target_is_ios {
        println!(
            "cargo:warning=Skipping UIKit bridge compilation on a non-Apple host; use macOS/Xcode for an iOS app build"
        );
    }
}

fn generate_app_icon() {
    use resvg::{tiny_skia, usvg};
    use std::{env, fs, path::PathBuf};

    let root = PathBuf::from(env::var("CARGO_MANIFEST_DIR").expect("missing manifest directory"));
    let source = root.join("resources/markerup_notepad_transparent.svg");
    let destination_dir = root.join("icons");
    println!("cargo:rerun-if-changed={}", source.display());

    let svg = fs::read(&source).expect("failed to read canonical Markerup app icon SVG");
    let tree = usvg::Tree::from_data(&svg, &usvg::Options::default())
        .expect("failed to parse canonical Markerup app icon SVG");
    fs::create_dir_all(&destination_dir).expect("failed to create generated app icon directory");

    // These are Tauri's standard Linux desktop icon sizes.  Keep all of them
    // generated from the SVG so desktop environments can select a native-size
    // image instead of relying on a 1024px fallback.
    for (name, size) in [
        ("32x32.png", 32),
        ("64x64.png", 64),
        ("128x128.png", 128),
        ("128x128@2x.png", 256),
        ("icon.png", 512),
    ] {
        let mut pixmap =
            tiny_skia::Pixmap::new(size, size).expect("failed to allocate app icon canvas");
        pixmap.fill(tiny_skia::Color::from_rgba8(234, 244, 255, 255));
        let scale = size as f32 / tree.size().width();
        resvg::render(
            &tree,
            tiny_skia::Transform::from_scale(scale, scale),
            &mut pixmap.as_mut(),
        );
        pixmap
            .save_png(destination_dir.join(name))
            .expect("failed to write generated app icon PNG");
    }
}
