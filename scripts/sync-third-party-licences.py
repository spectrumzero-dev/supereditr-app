#!/usr/bin/env python3
"""Refresh the licence files kept in scripts/third-party-upstream/ and the manifests listing them, after
a change to Cargo.lock or rust-toolchain.toml.

For each shipped crate whose archive has no licence file, it fetches the files listed for its project in
scripts/third-party-upstream.json from the repository at the commit the crate was published from, and
records that commit. A crate that newly needs files gets a project of its own, with the licence and
notice files found at its repository's root. The Rust standard library's files are fetched at the tag of
the toolchain in rust-toolchain.toml. Everything is fetched and checked before anything is written, so a
failure leaves the working tree as it was. Only GitHub repositories are fetched, from
raw.githubusercontent.com, and nothing fetched is run. scripts/check.sh does not run this script; it
runs offline.
"""

import importlib.util
import json
import sys
import tempfile
from pathlib import Path
from urllib.error import HTTPError
from urllib.parse import urlsplit
from urllib.request import urlopen

# Keep the import from writing a __pycache__ folder into scripts/.
sys.dont_write_bytecode = True
spec = importlib.util.spec_from_file_location(
    "third_party_notices", Path(__file__).resolve().parent / "third-party-notices.py"
)
notices = importlib.util.module_from_spec(spec)
spec.loader.exec_module(notices)

RUST_REPOSITORY = "https://github.com/rust-lang/rust"
# The files looked for at a repository's root when one of its crates first needs them.
ROOT_FILES = tuple(
    f"{stem}{suffix}"
    for stem in ("LICENSE", "LICENCE", "COPYING", "UNLICENSE")
    for suffix in ("", ".md", ".txt", "-MIT", "-MIT.md", "-MIT.txt", "-APACHE", "-APACHE.md", "-APACHE.txt")
) + ("COPYRIGHT", "NOTICE", "NOTICE.md", "NOTICE.txt", "NOTICES.md")


def fetch(url):
    """The bytes at `url`, or None when it does not exist."""
    try:
        with urlopen(url, timeout=30) as response:
            return response.read()
    except HTTPError as error:
        if error.code == 404:
            return None
        raise


def github(repository):
    """`repository` as https://github.com/owner/name, or None when it is not a GitHub repository."""
    parts = urlsplit((repository or "").strip())
    segments = [segment for segment in parts.path.split("/") if segment]
    if parts.scheme not in ("https", "http") or parts.netloc.lower() not in ("github.com", "www.github.com"):
        return None
    if len(segments) < 2:
        return None
    return f"https://github.com/{segments[0]}/{segments[1].removesuffix('.git')}"


class Fetcher:
    """Fetches each file once and checks that it is UTF-8 text that is not blank."""

    def __init__(self, fetch):
        self.fetch = fetch
        self.cache = {}

    def get(self, repository, ref, path, owner):
        normal = github(repository)
        if normal is None:
            raise ValueError(f"{repository}, for {owner}, is not a GitHub repository, so its files cannot be fetched")
        url = f"https://raw.githubusercontent.com/{normal.removeprefix('https://github.com/')}/{ref}/{path}"
        if url not in self.cache:
            self.cache[url] = self.fetch(url)
        data = self.cache[url]
        if data is None:
            return None
        try:
            text = data.decode("utf-8")
        except UnicodeDecodeError:
            raise ValueError(f"{path} from {repository} at {ref}, for {owner}, is not UTF-8 text") from None
        if not text.strip():
            raise ValueError(f"{path} from {repository} at {ref}, for {owner}, is blank")
        return data

    def same_at_each(self, repository, refs, path, owner):
        """The file at every one of `refs`, which must be identical; None when it is missing at all."""
        found = {ref: self.get(repository, ref, path, owner) for ref in refs}
        if len(set(found.values())) > 1:
            listed = ", ".join(f"{ref} ({'missing' if data is None else 'present'})" for ref, data in found.items())
            raise ValueError(
                f"{path} in {repository}, for {owner}, differs between the commits its crates were published from:"
                f" {listed}. Check these crates' licence files by hand."
            )
        return found[refs[0]]


def needs_files(packages):
    """The shipped crates whose archives have no licence file, by "name version"."""
    needed = {}
    for package in packages:
        _, has_licence = notices.own_licences(package)
        if not has_licence:
            needed[f"{package['name']} {package['version']}"] = package
    return needed


def plan(manifest, extra, packages, toolchain, fetch, root=notices.ROOT):
    """Work out the new manifests and files. Returns (files, removed, manifest, extra, report): `files`
    maps each path below `root` to its new bytes and `removed` lists paths to delete. Writes nothing."""
    notices.upstream_licences(manifest, root)
    before = {crate: commit for project in manifest["projects"] for crate, commit in project["crates"].items()}
    fetcher = Fetcher(fetch)
    report = []
    projects = [dict(project, crates={}) for project in manifest["projects"]]
    by_name = {crate.split()[0]: project for old, project in zip(manifest["projects"], projects) for crate in old["crates"]}
    by_repository = {github(project["repository"]): project for project in projects}
    allowed = {crate.split()[0]: reason for crate, reason in manifest["withoutLicence"].items()}
    without_licence = {}
    new = {}
    for crate, package in sorted(needs_files(packages).items()):
        name = package["name"]
        if name in allowed:
            without_licence[crate] = allowed[name]
            continue
        commit = notices.published_commit(package)
        repository = github(package.get("repository"))
        project = by_name.get(name) or by_repository.get(repository)
        if commit is None:
            raise ValueError(
                f"{crate} ships without a licence file and its archive does not record the commit it was published"
                f" from. Check in its licence files and list them in {notices.UPSTREAM.name} by hand."
            )
        if project is not None:
            project["crates"][crate] = commit
        elif repository is None:
            raise ValueError(
                f"{crate} ships without a licence file and names no GitHub repository ({package.get('repository')})."
                f" Check in its licence files and list them in {notices.UPSTREAM.name} by hand, or add it to"
                " withoutLicence with the reason its repository has none."
            )
        else:
            new.setdefault(repository, {})[crate] = commit

    files = {}
    removed = []
    for project in projects:
        repository = project["repository"]
        if not project["crates"]:
            report.append(f"Dropped {repository}: none of its crates needs its files now")
            removed += project["files"]
            continue
        folder = notices.project_folder(project["files"], repository, notices.UPSTREAM.name)
        commits = sorted(set(project["crates"].values()))
        for path in project["files"]:
            below = path.removeprefix(f"{notices.UPSTREAM_FILES}/{folder}/")
            data = fetcher.same_at_each(repository, commits, below, repository)
            if data is None:
                raise ValueError(
                    f"{below} is not in {repository} at {', '.join(commits)}. Update its files in"
                    f" {notices.UPSTREAM.name} by hand."
                )
            files[path] = data

    folders = {notices.project_folder(project["files"], project["repository"], notices.UPSTREAM.name) for project in projects}
    for repository, crates in sorted(new.items()):
        folder = repository.rsplit("/", 1)[1]
        if folder in folders or (root / notices.UPSTREAM_FILES / folder).exists():
            raise ValueError(
                f"{', '.join(crates)} needs licence files from {repository}, but {notices.UPSTREAM_FILES}/{folder}"
                " is taken. Add the project by hand."
            )
        folders.add(folder)
        commits = sorted(set(crates.values()))
        found = {}
        for name in ROOT_FILES:
            data = fetcher.same_at_each(repository, commits, name, ", ".join(crates))
            if data is not None:
                found[f"{notices.UPSTREAM_FILES}/{folder}/{name}"] = data
        if not any(notices.is_licence(path) for path in found):
            raise ValueError(
                f"{', '.join(crates)} ships without a licence file, and none was found at the root of {repository}"
                f" at {', '.join(commits)}. Check in its licence files and list them in {notices.UPSTREAM.name}"
                " by hand, or add the crates to withoutLicence with the reason the repository has none."
            )
        files.update(found)
        projects.append({"repository": repository, "files": sorted(found), "crates": crates})
        report.append(
            f"Added {repository} for {', '.join(crates)} with {', '.join(sorted(found))}. Check that these are"
            " the crates' licence files."
        )

    extra = [dict(package) for package in extra]
    rust = next(package for package in extra if package["name"] == notices.RUST_STD)
    if rust["version"] != toolchain:
        report.append(f"{notices.RUST_STD}: {rust['version']} -> {toolchain}")
    rust["version"] = toolchain
    rust["source"] = f"{RUST_REPOSITORY}/tree/{toolchain}/library"
    folder = notices.project_folder(rust["files"], notices.RUST_STD, notices.EXTRA.name)
    for path in rust["files"]:
        below = path.removeprefix(f"{notices.UPSTREAM_FILES}/{folder}/")
        data = fetcher.get(RUST_REPOSITORY, toolchain, below, notices.RUST_STD)
        if data is None:
            raise ValueError(f"{below} is not in {RUST_REPOSITORY} at {toolchain}")
        files[path] = data

    kept = [project for project in projects if project["crates"]]
    for project in kept:
        project["crates"] = dict(sorted(project["crates"].items()))
    manifest = {
        "withoutLicence": dict(sorted(without_licence.items())),
        "projects": sorted(kept, key=lambda project: min(project["crates"])),
    }
    after = {crate: commit for project in kept for crate, commit in project["crates"].items()}
    for crate in sorted(set(before) | set(after)):
        if before.get(crate) != after.get(crate):
            report.append(f"{crate}: {before.get(crate) or 'not listed'} -> {after.get(crate) or 'not listed'}")
    with tempfile.TemporaryDirectory() as staged:
        staged = Path(staged)
        for path, data in files.items():
            (staged / path).parent.mkdir(parents=True, exist_ok=True)
            (staged / path).write_bytes(data)
        notices.upstream_licences(manifest, staged)
        notices.listed_files(rust["files"], notices.RUST_STD, notices.EXTRA.name, staged)
    return files, sorted(set(removed) - set(files)), manifest, extra, report


def manifest_bytes(manifest):
    return (json.dumps(manifest, ensure_ascii=False, indent=2) + "\n").encode("utf-8")


def extra_bytes(extra):
    lines = ",\n".join(f"  {json.dumps(package, ensure_ascii=False)}" for package in extra)
    return f"[\n{lines}\n]\n".encode("utf-8")


def apply(files, removed, manifest, extra, root=notices.ROOT):
    """Write what `plan` worked out. Returns the paths that changed."""
    files = dict(files)
    files[notices.UPSTREAM.relative_to(notices.ROOT).as_posix()] = manifest_bytes(manifest)
    files[notices.EXTRA.relative_to(notices.ROOT).as_posix()] = extra_bytes(extra)
    changed = []
    for path, data in sorted(files.items()):
        target = root / path
        if target.is_file() and target.read_bytes() == data:
            continue
        target.parent.mkdir(parents=True, exist_ok=True)
        target.write_bytes(data)
        changed.append(path)
    for path in removed:
        target = root / path
        if target.is_file():
            target.unlink()
            changed.append(path)
            if not any(target.parent.iterdir()):
                target.parent.rmdir()
    return changed


def main():
    try:
        result = plan(
            notices.read_json(notices.UPSTREAM), notices.read_json(notices.EXTRA),
            list(notices.normal_dependencies()), notices.toolchain_version(), fetch,
        )
        *_, report = result
        changed = apply(*result[:4])
    except (OSError, ValueError, notices.subprocess.CalledProcessError) as error:
        sys.exit(f"{error}\nNothing was changed.")
    for line in report:
        print(line)
    for path in changed:
        print(f"Updated {path}")
    if not changed:
        print("The licence files from upstream repositories are up to date.")
    else:
        print("Now run python3 -I scripts/third-party-notices.py and review the diff.")


if __name__ == "__main__":
    main()
