"""Generate the licences shipped in Settings → About."""

import argparse
import hashlib
import json
import re
import subprocess
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
OUTPUT = ROOT / "ui/src/settings/third-party-notices.json"
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
    # Cargo's target-specific trees to decide what to include.
    shipped = set()
    for target in DESKTOP_TARGETS:
        tree = subprocess.check_output([
            "cargo", "tree", "--locked", "-p", app, "-e", "normal",
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


def collect_packages(lock):
    for package in normal_dependencies():
        directory = Path(package["manifest_path"]).parent
        files = notice_files(directory, package["license_file"])
        has_licence = package["license_file"] or any(
            filename.upper().startswith(LICENCE_PREFIXES) for filename in files
        )
        fallback = not has_licence
        if fallback:
            if not package["license"]:
                raise ValueError(f"No licence files or SPDX expression for {package['name']}")
            files.update(spdx_files(package["license"]))
        yield ("crate", package["name"], package["version"], package["license"], fallback, files)

    # Svelte's package contains both the compiler and the bundled runtime.
    dependencies = read_json(ROOT / "ui/package.json").get("dependencies", {})
    for name in sorted(set(dependencies) | {"svelte"}):
        relative = f"node_modules/{name}"
        directory = ROOT / "ui" / relative
        package = read_json(directory / "package.json")
        if package["version"] != lock["packages"][relative]["version"]:
            raise ValueError(f"{name} does not match the lockfile. Run npm --prefix ui ci.")
        files = notice_files(directory)
        if not any(filename.upper().startswith(LICENCE_PREFIXES) for filename in files):
            raise ValueError(f"No licence file for {name} {package['version']}")
        yield ("js", name, package["version"], package.get("license"), False, files)

    for package in read_json(ROOT / "scripts/third-party-extra.json"):
        files = {Path(path).as_posix(): ROOT / path for path in package["files"]}
        yield ("vendored", package["name"], package["version"], package["licence"], False, files)


def generate():
    lock_path = ROOT / "ui/package-lock.json"
    packages = []
    texts = {}
    for kind, name, version, licence, fallback, files in sorted(
        collect_packages(read_json(lock_path)), key=lambda package: package[:4]
    ):
        if not files:
            raise ValueError(f"No licence files or SPDX fallback for {name} {version}")
        notices = []
        for filename, path in sorted(files.items()):
            if not path.is_file():
                raise ValueError(f"Missing {filename} for {name} {version}")
            # Universal newline conversion makes this independent of the checkout OS.
            content = path.read_text(encoding="utf-8")
            text_id = hashlib.sha256(content.encode("utf-8")).hexdigest()
            texts[text_id] = content
            notices.append({"file": filename, "text": text_id})
        packages.append({
            "kind": kind, "name": name, "version": version,
            "licence": licence or "See licence files", "fallback": fallback, "notices": notices,
        })
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
