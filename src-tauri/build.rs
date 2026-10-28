use std::path::{Path, PathBuf};
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
    let binaries = manifest.join("binaries");
    let output = binaries.join(format!("supereditr-fm-{target}"));
    println!("cargo:rerun-if-changed={}", source.display());
    std::fs::create_dir_all(&binaries).unwrap();

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
        write_stub(&output);
    }
    join_universal(&binaries);
}

/// A universal build (`tauri build --target universal-apple-darwin`) compiles each architecture on
/// its own, then bundles `supereditr-fm-universal-apple-darwin`. Whichever architecture builds
/// second joins the two helpers into it.
fn join_universal(binaries: &Path) {
    let parts = ["aarch64", "x86_64"].map(|arch| binaries.join(format!("supereditr-fm-{arch}-apple-darwin")));
    if !parts.iter().all(|part| part.exists()) {
        return;
    }
    let output = binaries.join("supereditr-fm-universal-apple-darwin");
    let joined = Command::new("lipo")
        .arg("-create")
        .args(&parts)
        .arg("-output")
        .arg(&output)
        .status()
        .is_ok_and(|status| status.success());
    if !joined {
        // lipo can't join a stub, which is a shell script; a stub runs on either architecture.
        write_stub(&output);
    }
}

fn write_stub(path: &Path) {
    std::fs::write(
        path,
        "#!/bin/sh\nprintf '%s\\n' '{\"type\":\"hello\",\"available\":false,\"reason\":\"osTooOld\"}'\nexec cat >/dev/null\n",
    )
    .unwrap();
    Command::new("chmod").arg("+x").arg(path).status().unwrap();
}
