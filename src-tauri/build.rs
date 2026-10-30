use std::path::{Path, PathBuf};
use std::process::Command;

fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("macos") {
        build_foundation_models_helper();
    }
    set_version_label();
    tauri_build::build()
}

/// Sets `SUPEREDITR_VERSION_LABEL`, the version Settings shows. A release build shows the plain
/// version; any other build adds the commit it was built from, such as "0.1.0 (abc1234)", so a
/// tester's report names the exact source. The commit comes from `SUPEREDITR_SOURCE_SHA` when a
/// build sets it, otherwise from git; with neither, the label is the plain version.
fn set_version_label() {
    println!("cargo:rerun-if-env-changed=SUPEREDITR_RELEASE_BUILD");
    println!("cargo:rerun-if-env-changed=SUPEREDITR_SOURCE_SHA");
    let version = std::env::var("CARGO_PKG_VERSION").unwrap();
    let commit = if std::env::var_os("SUPEREDITR_RELEASE_BUILD").is_some() { None } else { source_commit() };
    let label = match commit {
        Some(commit) => format!("{version} ({commit})"),
        None => version,
    };
    println!("cargo:rustc-env=SUPEREDITR_VERSION_LABEL={label}");
}

/// The short hash of the commit being built, if it can be found.
fn source_commit() -> Option<String> {
    if let Ok(sha) = std::env::var("SUPEREDITR_SOURCE_SHA") {
        let sha = sha.trim();
        if sha.len() >= 7 && sha.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Some(sha[..7].to_ascii_lowercase());
        }
        println!("cargo:warning=Ignoring SUPEREDITR_SOURCE_SHA, which isn't a commit hash.");
    }
    // Rebuild the label when the checkout moves to another commit. A branch whose ref is only in
    // packed-refs has no file yet, so watch the nearest folder above it, where the next commit
    // writes one. (Watching a missing path would rerun this script on every build.)
    let git_path = |path: &str| git(&["rev-parse", "--path-format=absolute", "--git-path", path]).map(PathBuf::from);
    for file in ["HEAD", "packed-refs"].into_iter().filter_map(git_path) {
        if file.exists() {
            println!("cargo:rerun-if-changed={}", file.display());
        }
    }
    if let (Some(branch), Some(refs)) =
        (git(&["symbolic-ref", "-q", "HEAD"]).and_then(|r| git_path(&r)), git_path("refs"))
        && let Some(existing) = branch.ancestors().take_while(|p| p.starts_with(&refs)).find(|p| p.exists())
    {
        println!("cargo:rerun-if-changed={}", existing.display());
    }
    git(&["rev-parse", "--short=7", "HEAD"])
}

/// Runs git in this crate's directory and returns its trimmed output, if it succeeded.
fn git(args: &[&str]) -> Option<String> {
    let output = Command::new("git").args(args).current_dir(std::env::var("CARGO_MANIFEST_DIR").ok()?).output().ok()?;
    let text = String::from_utf8(output.stdout).ok()?.trim().to_owned();
    (output.status.success() && !text.is_empty()).then_some(text)
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
