fn main() {
    slint_build::compile("ui/main.slint").expect("failed to compile Slint UI");

    let target_is_ios = std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("ios");
    let host_is_apple = std::env::var("HOST").is_ok_and(|host| host.contains("apple"));
    if target_is_ios && host_is_apple {
        println!("cargo:rustc-link-lib=framework=UniformTypeIdentifiers");
        cc::Build::new()
            .file("ios/MarkerupIOSBridge.m")
            .flag("-fobjc-arc")
            .compile("markerup_ios_bridge");
    } else if target_is_ios {
        println!("cargo:warning=Skipping UIKit bridge compilation on a non-Apple host; use macOS/Xcode for an iOS app build");
    }
}
