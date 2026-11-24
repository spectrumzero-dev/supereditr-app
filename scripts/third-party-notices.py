"""Generate the licences shipped in Settings → About."""

import argparse
import hashlib
import json
import re
import subprocess
import tomllib
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
OUTPUT = ROOT / "ui/src/settings/third-party-notices.json"
# Svelte's package contains both the compiler and the browser runtime, and the runtime imports clsx
# and esm-env. Svelte's other dependencies are used only by the compiler. Vite and Rolldown add small
# runtime helpers of their own (module preloading, the chunk runtime) as virtual modules, which the
# check in ui/vite.config.ts cannot see, so they are listed here. That check fails the build when the
# bundle takes a module from a package, or a version of it, missing here.
BUNDLED_JS = ("svelte", "clsx", "esm-env", "vite", "rolldown")
LICENCE_PREFIXES = ("LICENSE", "LICENCE", "COPYING", "UNLICENSE")
NOTICE_PREFIXES = (*LICENCE_PREFIXES, "NOTICE", "COPYRIGHT", "THIRD_PARTY", "THIRDPARTY", "THIRD-PARTY")
DESKTOP_TARGETS = (
    "aarch64-apple-darwin",
    "x86_64-apple-darwin",
    "aarch64-unknown-linux-gnu",
    "x86_64-unknown-linux-gnu",
    "aarch64-pc-windows-msvc",
    "x86_64-pc-windows-msvc",
)
# Cargo can append source paths, proc-macro labels or a deduplication marker.
TREE_PACKAGE = re.compile(r"(\S+) v(\S+)(?: .*)?")


def read_json(path):
    return json.loads(path.read_text(encoding="utf-8"))


def normal_dependencies():
    metadata = json.loads(subprocess.check_output(
        ["cargo", "metadata", "--format-version", "1", "--locked"], cwd=ROOT
    ))
    packages = {}
    workspace = set(metadata["workspace_members"])
    app = next(
        package["name"] for package in metadata["packages"]
        if package["id"] in workspace and package["name"] == "supereditr"
    )
    for package in metadata["packages"]:
        key = (package["name"], package["version"])
        packages.setdefault(key, []).append(package)

    # The metadata graph includes dependencies outside the desktop build. Use
    # Cargo's target-specific trees to decide what to include. Proc macros
    # are built for the host and their dependencies do not ship.
    shipped = set()
    for target in DESKTOP_TARGETS:
        tree = subprocess.check_output([
            "cargo", "tree", "--locked", "-p", app, "-e", "normal,no-proc-macro",
            "--prefix", "none", "--format", "{p}", "--target", target, "--color", "never",
        ], cwd=ROOT, encoding="utf-8")
        for line in tree.splitlines():
            match = TREE_PACKAGE.fullmatch(line)
            if not match:
                raise ValueError(f"Unexpected cargo tree output for {target}: {line}")
            shipped.add(match.groups())

    for name, version in sorted(shipped):
        matches = packages.get((name, version), [])
        if len(matches) != 1:
            raise ValueError(f"Expected one Cargo metadata entry for {name} {version}, found {len(matches)}")
        package = matches[0]
        if package["id"] not in workspace:
            yield package


def notice_files(directory, declared=None):
    paths = {
        path for path in directory.iterdir()
        if path.is_file() and path.name.upper().startswith(NOTICE_PREFIXES)
    }
    if declared:
        paths.add(directory / declared)
    return {path.relative_to(directory).as_posix(): path for path in sorted(paths)}


def spdx_files(expression):
    # Older Cargo manifests also use "/" for alternatives.
    ids = re.split(r"\s+(?:AND|OR)\s+|/", expression.strip("() "))
    return {
        f"{licence}.txt": ROOT / "scripts/third-party-spdx" / f"{licence}.txt"
        for licence in sorted({part.strip("() ") for part in ids})
    }


def is_licence(filename):
    return Path(filename).name.upper().startswith(LICENCE_PREFIXES)


def entry(kind, name, version, licence, source, files, fallback=False):
    return {
        "kind": kind, "name": name, "version": version, "licence": licence, "source": source,
        "fallback": fallback, "files": files,
    }


def standard_texts(licence):
    return [(filename, path, "spdx") for filename, path in spdx_files(licence).items()]


def crate_licences(package):
    directory = Path(package["manifest_path"]).parent
    own = notice_files(directory, package["license_file"])
    files = [(filename, path, None) for filename, path in own.items()]
    if package["license_file"] or any(is_licence(filename) for filename in own):
        return files, False
    if not package["license"]:
        raise ValueError(f"No licence files or SPDX expression for {package['name']} {package['version']}")
    return files + standard_texts(package["license"]), True


def runtime_licences(shipped):
    with (ROOT / "rust-toolchain.toml").open("rb") as toolchain:
        version = tomllib.load(toolchain)["toolchain"]["channel"]
    licence = "MIT OR Apache-2.0"
    yield entry(
        "vendored", "Rust standard library", version, licence,
        f"https://github.com/rust-lang/rust/tree/{version}/library", standard_texts(licence),
    )
    for package in shipped:
        if package["name"] == "webview2-com-sys":
            licence = "BSD-3-Clause"
            yield entry(
                "vendored", "WebView2 loader (Windows)", package["version"], licence,
                "https://www.nuget.org/packages/Microsoft.Web.WebView2", standard_texts(licence),
            )


def collect_packages(lock):
    shipped = list(normal_dependencies())
    for package in shipped:
        files, fallback = crate_licences(package)
        yield entry(
            "crate", package["name"], package["version"], package["license"],
            f"https://crates.io/crates/{package['name']}/{package['version']}", files, fallback,
        )

    dependencies = read_json(ROOT / "ui/package.json").get("dependencies", {})
    for name in sorted(set(dependencies) | set(BUNDLED_JS)):
        relative = f"node_modules/{name}"
        directory = ROOT / "ui" / relative
        package = read_json(directory / "package.json")
        if package["version"] != lock["packages"][relative]["version"]:
            raise ValueError(f"{name} does not match the lockfile. Run npm --prefix ui ci.")
        files = notice_files(directory)
        if not any(is_licence(filename) for filename in files):
            raise ValueError(f"No licence file for {name} {package['version']}")
        yield entry(
            "js", name, package["version"], package.get("license"),
            f"https://www.npmjs.com/package/{name}/v/{package['version']}",
            [(filename, path, None) for filename, path in files.items()],
        )

    for package in read_json(ROOT / "scripts/third-party-extra.json"):
        files = [(path, ROOT / path, None) for path in package["files"]]
        yield entry("vendored", package["name"], package["version"], package["licence"], package["source"], files)

    yield from runtime_licences(shipped)


def generate():
    lock_path = ROOT / "ui/package-lock.json"
    packages = []
    texts = {}
    for package in sorted(
        collect_packages(read_json(lock_path)),
        key=lambda package: (package["kind"], package["name"], package["version"], package["licence"] or ""),
    ):
        name, version = package["name"], package["version"]
        if not package["files"]:
            raise ValueError(f"No licence files or SPDX fallback for {name} {version}")
        notices = []
        for filename, path, origin in sorted(package["files"], key=lambda file: (file[2] is not None, file[0])):
            if not path.is_file():
                raise ValueError(f"Missing {filename} for {name} {version}")
            # Universal newline conversion makes this independent of the checkout OS.
            content = path.read_text(encoding="utf-8")
            text_id = hashlib.sha256(content.encode("utf-8")).hexdigest()
            texts[text_id] = content
            notice = {"file": filename, "text": text_id}
            if origin:
                notice["origin"] = origin
            notices.append(notice)
        listed = {
            "kind": package["kind"], "name": name, "version": version,
            "licence": package["licence"] or "See licence files", "fallback": package["fallback"],
            "source": package["source"], "notices": notices,
        }
        packages.append(listed)
    locks = {
        path.relative_to(ROOT).as_posix(): hashlib.sha256(
            path.read_text(encoding="utf-8").encode("utf-8")
        ).hexdigest()
        for path in (ROOT / "Cargo.lock", lock_path)
    }
    return {
        "generated": "Generated by scripts/third-party-notices.py; do not edit.",
        "lockfiles": locks,
        "packages": packages,
        "texts": dict(sorted(texts.items())),
    }


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path, default=OUTPUT)
    args = parser.parse_args()
    try:
        notices = generate()
        args.output.write_bytes((json.dumps(notices, ensure_ascii=False, indent=2) + "\n").encode("utf-8"))
    except (OSError, ValueError, subprocess.CalledProcessError) as error:
        parser.exit(1, f"{error}\n")


if __name__ == "__main__":
    main()
