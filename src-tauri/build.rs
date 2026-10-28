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
    if parts.iter().any(|part| is_stub(part)) {
        // lipo can't join a stub, which is a shell script; a stub runs on either architecture.
        write_stub(&output);
        return;
    }
    let status = Command::new("lipo").arg("-create").args(&parts).arg("-output").arg(&output).status();
    assert!(
        status.as_ref().is_ok_and(|status| status.success()),
        "lipo couldn't join the Apple Intelligence helpers: {status:?}"
    );
    // lipo -create skips an input it doesn't recognise and still succeeds, so check the result.
    let archs = Command::new("lipo")
        .arg("-archs")
        .arg(&output)
        .output()
        .map(|out| String::from_utf8_lossy(&out.stdout).into_owned());
    let archs = archs.unwrap_or_default();
    let words: Vec<&str> = archs.split_whitespace().collect();
    assert!(
        words.contains(&"arm64") && words.contains(&"x86_64"),
        "the universal Apple Intelligence helper is missing an architecture: {archs:?}"
    );
}

/// A stub is the shell script `write_stub` writes; a compiled helper is a Mach-O binary.
fn is_stub(path: &Path) -> bool {
    std::fs::read(path).is_ok_and(|bytes| bytes.starts_with(b"#!"))
}

fn write_stub(path: &Path) {
    std::fs::write(
        path,
        "#!/bin/sh\nprintf '%s\\n' '{\"type\":\"hello\",\"available\":false,\"reason\":\"osTooOld\"}'\nexec cat >/dev/null\n",
    )
    .unwrap();
    Command::new("chmod").arg("+x").arg(path).status().unwrap();
}
