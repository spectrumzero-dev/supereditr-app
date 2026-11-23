"""Generate the licences shipped in Settings → About."""

import argparse
import hashlib
import json
import re
import subprocess
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
OUTPUT = ROOT / "ui/src/settings/third-party-notices.json"
UPSTREAM = ROOT / "scripts/third-party-upstream.json"
RUST_STD = "Rust standard library"
# Svelte's package contains both the compiler and the browser runtime, and the runtime imports these.
# Svelte's other dependencies are used only by the compiler. ui/vite.config.ts fails the build when
# the bundle takes a module from a package missing here.
BUNDLED_JS = ("svelte", "clsx", "esm-env")
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


def entry(kind, name, version, licence, source, files, fallback=False, upstream=None):
    """`files` maps a shown file name to (path, origin): None for the package's own file, "repository"
    for one taken from the project's repository, "spdx" for a standard text."""
    return {
        "kind": kind, "name": name, "version": version, "licence": licence, "source": source,
        "fallback": fallback, "upstream": upstream, "files": files,
    }


def upstream_licences():
    """Licence files checked in from the repositories of crates whose archives ship none, by "name version"."""
    manifest = read_json(UPSTREAM)
    crates = {}
    for project in manifest["projects"]:
        for crate, commit in project["crates"].items():
            if crate in crates:
                raise ValueError(f"{crate} is listed twice in {UPSTREAM.name}")
            crates[crate] = (project, commit)
    return crates, manifest["withoutLicence"]


def crate_licences(package, upstream, without_licence):
    """The licence files for a crate: its own, else those checked in from its repository."""
    directory = Path(package["manifest_path"]).parent
    own = notice_files(directory, package["license_file"])
    files = {filename: (path, None) for filename, path in own.items()}
    if package["license_file"] or any(filename.upper().startswith(LICENCE_PREFIXES) for filename in own):
        return files, False, None
    crate = f"{package['name']} {package['version']}"
    if not package["license"]:
        raise ValueError(f"No licence files or SPDX expression for {crate}")
    standard = {filename: (path, "spdx") for filename, path in spdx_files(package["license"]).items()}
    if crate in without_licence:
        return {**files, **standard}, True, None
    if crate not in upstream:
        raise ValueError(
            f"{crate} ships without a licence file. Check in the licence files from its repository and"
            f" list them in {UPSTREAM.relative_to(ROOT).as_posix()}, or add the crate to withoutLicence"
            " with the reason its repository has none."
        )
    project, commit = upstream[crate]
    vcs = directory / ".cargo_vcs_info.json"
    built_from = read_json(vcs)["git"]["sha1"] if vcs.is_file() else None
    if built_from != commit:
        raise ValueError(
            f"{crate} was published from commit {built_from}, but {UPSTREAM.name} records {commit}."
            " Take its licence files from the commit it was published from."
        )
    for path in project["files"]:
        files[Path(path).name] = (ROOT / path, "repository")
    if project.get("standardTexts"):
        files.update(standard)
    return files, False, {"repository": project["repository"], "commit": commit}


def toolchain_version():
    match = re.search(r'^channel\s*=\s*"([^"]+)"', (ROOT / "rust-toolchain.toml").read_text(encoding="utf-8"), re.M)
    if not match:
        raise ValueError("No channel in rust-toolchain.toml")
    return match.group(1)


def collect_packages(lock):
    upstream, without_licence = upstream_licences()
    for crate in without_licence:
        if crate in upstream:
            raise ValueError(f"{crate} is in both withoutLicence and projects in {UPSTREAM.name}")
    used = set()
    shipped = set()
    for package in normal_dependencies():
        crate = f"{package['name']} {package['version']}"
        shipped.add(crate)
        files, fallback, source = crate_licences(package, upstream, without_licence)
        if source or fallback:
            used.add(crate)
        yield entry(
            "crate", package["name"], package["version"], package["license"],
            f"https://crates.io/crates/{package['name']}/{package['version']}", files, fallback, source,
        )
    unused = sorted((set(upstream) | set(without_licence)) - used)
    if unused:
        raise ValueError(
            f"{UPSTREAM.name} lists crates that are not shipped or now ship their own licence files: {', '.join(unused)}"
        )

    dependencies = read_json(ROOT / "ui/package.json").get("dependencies", {})
    for name in sorted(set(dependencies) | set(BUNDLED_JS)):
        relative = f"node_modules/{name}"
        directory = ROOT / "ui" / relative
        package = read_json(directory / "package.json")
        if package["version"] != lock["packages"][relative]["version"]:
            raise ValueError(f"{name} does not match the lockfile. Run npm --prefix ui ci.")
        files = notice_files(directory)
        if not any(filename.upper().startswith(LICENCE_PREFIXES) for filename in files):
            raise ValueError(f"No licence file for {name} {package['version']}")
        yield entry(
            "js", name, package["version"], package.get("license"),
            f"https://www.npmjs.com/package/{name}/v/{package['version']}",
            {filename: (path, None) for filename, path in files.items()},
        )

    extra = read_json(ROOT / "scripts/third-party-extra.json")
    rust = [package for package in extra if package["name"] == RUST_STD]
    if len(rust) != 1 or rust[0]["version"] != toolchain_version():
        raise ValueError(
            f"scripts/third-party-extra.json must list the {RUST_STD} at the version in rust-toolchain.toml"
            f" ({toolchain_version()}), with its licence files from that release."
        )
    for package in extra:
        if package.get("shippedWith") and package["shippedWith"] not in shipped:
            raise ValueError(
                f"{package['name']} is listed as shipped with {package['shippedWith']}, which is no longer"
                " shipped. Check which version it now ships and update its entry."
            )
        files = {Path(path).as_posix(): (ROOT / path, None) for path in package["files"]}
        yield entry("vendored", package["name"], package["version"], package["licence"], package["source"], files)


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
        # The package's own and the repository's files come before standard texts.
        for filename, (path, origin) in sorted(
            package["files"].items(), key=lambda item: (item[1][1] == "spdx", item[0])
        ):
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
        if package["upstream"]:
            listed["upstream"] = package["upstream"]
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
