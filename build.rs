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
    let source = root.join("resources/markerup_notepad_icon.svg");
    let destination = root.join("icons/icon.png");
    println!("cargo:rerun-if-changed={}", source.display());

    let svg = fs::read(&source).expect("failed to read canonical Markerup app icon SVG");
    let tree = usvg::Tree::from_data(&svg, &usvg::Options::default())
        .expect("failed to parse canonical Markerup app icon SVG");
    let mut pixmap =
        tiny_skia::Pixmap::new(1024, 1024).expect("failed to allocate app icon canvas");
    pixmap.fill(tiny_skia::Color::from_rgba8(234, 244, 255, 255));
    let scale = 1024.0 / tree.size().width();
    resvg::render(
        &tree,
        tiny_skia::Transform::from_scale(scale, scale),
        &mut pixmap.as_mut(),
    );
    fs::create_dir_all(
        destination
            .parent()
            .expect("app icon has no parent directory"),
    )
    .expect("failed to create generated app icon directory");
    pixmap
        .save_png(&destination)
        .expect("failed to write generated app icon PNG");
}
