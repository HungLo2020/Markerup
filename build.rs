fn main() {
    generate_app_icon();
    normalize_generated_ios_app_icons();
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

/// Tauri's icon generator deliberately writes RGBA PNGs, including for iOS
/// icons generated from an SVG with an opaque `--ios-color` background. Apple
/// rejects App Store icons that merely *have* an alpha channel. Composite the
/// generated RGBA catalog over Tauri's configured opaque iOS background, then
/// re-encode it as RGB. The source SVG remains canonical; this only changes a
/// generated Apple build artifact.
fn normalize_generated_ios_app_icons() {
    use png::{BitDepth, ColorType, Decoder, Encoder, Transformations};
    use std::{
        env, fs,
        fs::File,
        io::{BufReader, BufWriter},
        path::PathBuf,
    };

    if env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("ios") {
        return;
    }

    let root = PathBuf::from(env::var("CARGO_MANIFEST_DIR").expect("missing manifest directory"));
    let iconset = root.join("gen/apple/Assets.xcassets/AppIcon.appiconset");
    let Ok(entries) = fs::read_dir(&iconset) else {
        return;
    };

    for entry in entries {
        let path = entry.expect("failed to inspect generated iOS icon").path();
        if path
            .extension()
            .is_some_and(|extension| extension.eq_ignore_ascii_case("png"))
        {
            let input = File::open(&path).expect("failed to read generated iOS app icon");
            let mut decoder = Decoder::new(BufReader::new(input));
            decoder.set_transformations(Transformations::EXPAND | Transformations::STRIP_16);
            let mut reader = decoder
                .read_info()
                .expect("failed to decode generated iOS app icon");
            let mut pixels = vec![0; reader.output_buffer_size()];
            let info = reader
                .next_frame(&mut pixels)
                .expect("failed to read generated iOS app icon pixels");
            let pixels = &pixels[..info.buffer_size()];

            let rgb = match info.color_type {
                ColorType::Rgb => continue,
                ColorType::Rgba => pixels
                    .chunks_exact(4)
                    .map(|rgba| {
                        // Match `--ios-color '#eaf4ff'` in the iOS workflows.
                        // SVG antialiasing legitimately leaves fractional alpha
                        // at icon edges; blending it here retains those edges
                        // while making the final AppIcon fully opaque.
                        const BACKGROUND: [u8; 3] = [234, 244, 255];
                        let alpha = u16::from(rgba[3]);
                        let pixel: [u8; 3] = std::array::from_fn(|channel| {
                            let foreground = u16::from(rgba[channel]);
                            let background = u16::from(BACKGROUND[channel]);
                            ((foreground * alpha + background * (255 - alpha) + 127) / 255) as u8
                        });
                        pixel
                    })
                    .flatten()
                    .collect::<Vec<_>>(),
                _ => panic!(
                    "generated iOS app icon {} decoded to unsupported {:?} pixels",
                    path.display(),
                    info.color_type
                ),
            };

            let output = File::create(&path).expect("failed to rewrite generated iOS app icon");
            let mut encoder = Encoder::new(BufWriter::new(output), info.width, info.height);
            encoder.set_color(ColorType::Rgb);
            encoder.set_depth(BitDepth::Eight);
            encoder
                .write_header()
                .expect("failed to write generated iOS app icon header")
                .write_image_data(&rgb)
                .expect("failed to write opaque generated iOS app icon");
        }
    }
}

fn generate_app_icon() {
    use resvg::{tiny_skia, usvg};
    use std::{env, fs, path::PathBuf};

    let root = PathBuf::from(env::var("CARGO_MANIFEST_DIR").expect("missing manifest directory"));
    let source = root.join("resources/markerup_notepad_icon.svg");
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
