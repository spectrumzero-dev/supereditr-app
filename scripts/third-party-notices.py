"""Generate the licences shipped in Settings → About."""

import argparse
import hashlib
import json
import os
import re
import subprocess
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
OUTPUT = ROOT / "ui/src/settings/third-party-notices.json"
UPSTREAM = ROOT / "scripts/third-party-upstream.json"
EXTRA = ROOT / "scripts/third-party-extra.json"
# Files from upstream repositories are kept in one folder per project here, at the path they have in
# that repository, which is how scripts/sync-third-party-licences.py knows where to fetch each from.
UPSTREAM_FILES = "scripts/third-party-upstream"
SYNC = "python3 -I scripts/sync-third-party-licences.py"
RUST_STD = "Rust standard library"
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


def entry(kind, name, version, licence, source, files, fallback=False, upstream=None):
    """`files` lists (shown name, path, origin): origin None for the package's own file, "repository"
    for one taken from the project's repository, "spdx" for a standard text. Files with one name but
    different origins are all kept."""
    if upstream is None and any(origin == "repository" for _, _, origin in files):
        raise ValueError(f"{name} {version} has a file from its repository but no repository to credit")
    seen = set()
    for filename, _, origin in files:
        if (filename, origin) in seen:
            raise ValueError(f"{filename} is listed twice for {name} {version}")
        seen.add((filename, origin))
    return {
        "kind": kind, "name": name, "version": version, "licence": licence, "source": source,
        "fallback": fallback, "upstream": upstream, "files": files,
    }


def blank(value):
    return not isinstance(value, str) or not value.strip()


DRIVE = re.compile(r"[A-Za-z]:")


def listed_files(files, owner, manifest, root=ROOT):
    """Check the `files` listed for `owner` in `manifest` before any is used: a list of plain relative
    paths with forward slashes, each a file inside `root` that is not blank. Returns their paths."""
    if not isinstance(files, list):
        raise ValueError(f"files for {owner} in {manifest} must be a list of paths")
    root = root.resolve()
    paths = []
    for path in files:
        if blank(path):
            raise ValueError(f"files for {owner} in {manifest} has an empty or non-text path: {path!r}")
        if (
            path.startswith("/") or DRIVE.match(path) or "\\" in path
            or any(part in ("", ".", "..") for part in path.split("/"))
        ):
            raise ValueError(
                f"{path}, listed for {owner} in {manifest}, must be a relative path with forward slashes"
                " and no . or .. parts"
            )
        resolved = (root / path).resolve()
        if not resolved.is_relative_to(root):
            raise ValueError(f"{path}, listed for {owner} in {manifest}, is outside the repository")
        if not resolved.is_file():
            raise ValueError(f"{path}, listed for {owner} in {manifest}, does not exist")
        if not resolved.read_text(encoding="utf-8").strip():
            raise ValueError(f"{path}, listed for {owner} in {manifest}, is blank")
        paths.append(path)
    return paths


def project_folder(paths, owner, manifest):
    """The folder below UPSTREAM_FILES holding every one of `paths`, which have passed listed_files.
    Each file's path below it is its path in the upstream repository."""
    parts = [path.split("/") for path in paths]
    folders = {part[2] if part[:2] == UPSTREAM_FILES.split("/") and len(part) > 3 else None for part in parts}
    if len(folders) != 1 or None in folders:
        raise ValueError(f"The files for {owner} in {manifest} must all be in one folder below {UPSTREAM_FILES}/")
    return folders.pop()


def upstream_licences(manifest, root=ROOT):
    """Check the manifest of licence files taken from the repositories of crates whose archives ship
    none. Returns the projects by "name version", and the crates allowed to ship without any."""
    without_licence = manifest["withoutLicence"]
    for crate, reason in without_licence.items():
        if blank(reason):
            raise ValueError(f"{crate} is in withoutLicence in {UPSTREAM.name} without a reason")
    crates = {}
    folders = {}
    for project in manifest["projects"]:
        repository = project.get("repository")
        if blank(repository):
            raise ValueError(f"A project in {UPSTREAM.name} has no repository")
        if not project.get("crates"):
            raise ValueError(f"{repository} in {UPSTREAM.name} lists no crates")
        files = listed_files(project.get("files", []), repository, UPSTREAM.name, root)
        if not any(is_licence(path) for path in files):
            raise ValueError(f"{repository} in {UPSTREAM.name} lists no licence file")
        if len(set(files)) != len(files):
            raise ValueError(f"{repository} in {UPSTREAM.name} lists a file twice")
        folder = project_folder(files, repository, UPSTREAM.name)
        if folder in folders:
            raise ValueError(f"{repository} and {folders[folder]} in {UPSTREAM.name} share the folder {folder}")
        folders[folder] = repository
        if "standardTexts" in project and blank(project["standardTexts"]):
            raise ValueError(f"standardTexts for {repository} in {UPSTREAM.name} gives no reason")
        for crate, commit in project["crates"].items():
            if crate in crates:
                raise ValueError(f"{crate} is listed twice in {UPSTREAM.name}")
            if crate in without_licence:
                raise ValueError(f"{crate} is in both withoutLicence and projects in {UPSTREAM.name}")
            crates[crate] = (project, commit)
    return crates, without_licence


def upstream_files(paths, root=ROOT):
    """Name each file by its path below the folder holding all of the project's files, so two files
    with one name in different folders stay apart."""
    paths = [root / path for path in paths]
    common = Path(os.path.commonpath([path.parent for path in paths]))
    return [(path.relative_to(common).as_posix(), path) for path in paths]


def own_licences(package):
    """A crate's own notice files, and whether one of them is a licence."""
    own = notice_files(Path(package["manifest_path"]).parent, package["license_file"])
    files = [(filename, path, None) for filename, path in own.items()]
    return files, bool(package["license_file"]) or any(is_licence(filename) for filename in own)


def published_commit(package):
    """The commit a crate was published from, as recorded in its archive, or None."""
    vcs = Path(package["manifest_path"]).parent / ".cargo_vcs_info.json"
    return read_json(vcs).get("git", {}).get("sha1") if vcs.is_file() else None


def crate_licences(package, upstream, without_licence, root=ROOT):
    """The licence files for a crate: its own, else those checked in from its repository. A crate's
    own notice files are kept either way."""
    files, has_licence = own_licences(package)
    if has_licence:
        return files, False, None
    crate = f"{package['name']} {package['version']}"
    if not package["license"]:
        raise ValueError(f"No licence files or SPDX expression for {crate}")
    standard = [(filename, path, "spdx") for filename, path in spdx_files(package["license"]).items()]
    if crate in without_licence:
        return files + standard, True, None
    if crate not in upstream:
        raise ValueError(
            f"{crate} ships without a licence file and {UPSTREAM.name} has none for it. Run {SYNC}"
            " to fetch them from its repository."
        )
    project, commit = upstream[crate]
    built_from = published_commit(package)
    if built_from != commit:
        raise ValueError(
            f"{crate} was published from commit {built_from}, but {UPSTREAM.name} records {commit}."
            f" Run {SYNC} to fetch its licence files from the commit it was published from."
        )
    files += [(filename, path, "repository") for filename, path in upstream_files(project["files"], root)]
    if project.get("standardTexts"):
        files += standard
    return files, False, {"repository": project["repository"], "commit": commit}


def toolchain_version():
    match = re.search(r'^channel\s*=\s*"([^"]+)"', (ROOT / "rust-toolchain.toml").read_text(encoding="utf-8"), re.M)
    if not match:
        raise ValueError("No channel in rust-toolchain.toml")
    return match.group(1)


def collect_packages(lock):
    upstream, without_licence = upstream_licences(read_json(UPSTREAM))
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
            f"{UPSTREAM.name} lists crates that are not shipped or now ship their own licence files:"
            f" {', '.join(unused)}. Run {SYNC} to update it."
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

    extra = read_json(EXTRA)
    rust = [package for package in extra if package["name"] == RUST_STD]
    if len(rust) != 1 or rust[0]["version"] != toolchain_version():
        raise ValueError(
            f"scripts/third-party-extra.json must list the {RUST_STD} at the version in rust-toolchain.toml"
            f" ({toolchain_version()}), with its licence files from that release. Run {SYNC}."
        )
    for package in extra:
        if package.get("shippedWith") and package["shippedWith"] not in shipped:
            raise ValueError(
                f"{package['name']} is listed as shipped with {package['shippedWith']}, which is no longer"
                " shipped. Check which version it now ships and update its entry."
            )
        # Shown by their full path, which keeps files with one name apart.
        paths = listed_files(package.get("files"), package["name"], EXTRA.name)
        files = [(path, ROOT / path, None) for path in paths]
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
        # The package's own files come first, then the repository's, then standard texts.
        rank = {None: 0, "repository": 1, "spdx": 2}
        for filename, path, origin in sorted(package["files"], key=lambda file: (rank[file[2]], file[0])):
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
