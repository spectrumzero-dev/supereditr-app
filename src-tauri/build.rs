use std::path::PathBuf;
use std::process::Command;

fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("macos") {
        build_foundation_models_helper();
    }
    tauri_build::build()
}

/// Compiles the Apple Intelligence helper (helpers/foundation-models) into
/// `binaries/supereditr-fm-<target>`, which Tauri bundles next to the app executable.
/// It targets macOS 13 with FoundationModels weak-linked, so it runs everywhere SuperEditr
/// does and reports "needs macOS 26" on older systems.
fn build_foundation_models_helper() {
    let manifest = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    let target = std::env::var("TARGET").unwrap();
    let source = manifest.join("../helpers/foundation-models/main.swift");
    let output = manifest.join("binaries").join(format!("supereditr-fm-{target}"));
    println!("cargo:rerun-if-changed={}", source.display());
    std::fs::create_dir_all(output.parent().unwrap()).unwrap();

    let arch = if target.starts_with("aarch64") { "arm64" } else { "x86_64" };
    let compiled = Command::new("xcrun")
        .args(["swiftc", "-O", "-swift-version", "5", "-target", &format!("{arch}-apple-macos13.0")])
        .args(["-Xlinker", "-weak_framework", "-Xlinker", "FoundationModels"])
        .arg(&source)
        .arg("-o")
        .arg(&output)
        .status()
        .is_ok_and(|status| status.success());
    if !compiled {
        // Without a Swift toolchain or SDK that has FoundationModels, ship a stand-in that tells
        // the app the on-device model is unavailable, so everything else still builds and runs.
        println!("cargo:warning=Couldn't compile the Apple Intelligence helper; bundling a stub.");
        std::fs::write(
            &output,
            "#!/bin/sh\nprintf '%s\\n' '{\"type\":\"hello\",\"available\":false,\"reason\":\"osTooOld\"}'\nexec cat >/dev/null\n",
        )
        .unwrap();
        Command::new("chmod").arg("+x").arg(&output).status().unwrap();
    }
}
