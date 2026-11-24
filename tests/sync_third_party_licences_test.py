"""Tests for scripts/sync-third-party-licences.py, with a stand-in for the network. Run by scripts/check.sh."""

import importlib.util
import json
import sys
import tempfile
import unittest
from pathlib import Path

SCRIPT = Path(__file__).resolve().parent.parent / "scripts/sync-third-party-licences.py"
# Keep the import from writing a __pycache__ folder into scripts/.
sys.dont_write_bytecode = True
spec = importlib.util.spec_from_file_location("sync_third_party_licences", SCRIPT)
sync = importlib.util.module_from_spec(spec)
spec.loader.exec_module(sync)
notices = sync.notices

OLD = "1" * 40
NEW = "2" * 40
OTHER = "3" * 40
REPOSITORY = "https://github.com/quillpoint/demo"
RAW = "https://raw.githubusercontent.com/quillpoint/demo"
RUST_RAW = "https://raw.githubusercontent.com/rust-lang/rust"
LICENCE = "scripts/third-party-upstream/demo/LICENSE"
RUST_LICENCE = "scripts/third-party-upstream/rust/LICENSE-MIT"


class SyncTest(unittest.TestCase):
    def setUp(self):
        directory = tempfile.TemporaryDirectory()
        self.addCleanup(directory.cleanup)
        self.root = Path(directory.name)
        self.upstream = {}
        self.fetched = []
        self.write(LICENCE, "Demo licence, first version\n")
        self.write(RUST_LICENCE, "Rust licence\n")
        self.serve(f"{RAW}/{OLD}/LICENSE", "Demo licence, first version\n")
        self.serve(f"{RUST_RAW}/1.2.3/LICENSE-MIT", "Rust licence\n")
        self.manifest = {
            "withoutLicence": {},
            "projects": [{"repository": REPOSITORY, "files": [LICENCE], "crates": {"demo 1.0.0": OLD}}],
        }
        self.extra = [{
            "name": notices.RUST_STD, "version": "1.2.3", "licence": "MIT OR Apache-2.0",
            "source": "https://github.com/rust-lang/rust/tree/1.2.3/library", "files": [RUST_LICENCE],
        }]
        self.save()

    def save(self):
        self.write(notices.UPSTREAM.relative_to(notices.ROOT), sync.manifest_bytes(self.manifest).decode())
        self.write(notices.EXTRA.relative_to(notices.ROOT), sync.extra_bytes(self.extra).decode())

    def write(self, path, text):
        path = self.root / path
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_bytes(text.encode("utf-8"))
        return path

    def serve(self, url, text):
        self.upstream[url] = text if isinstance(text, bytes) else text.encode("utf-8")

    def fetch(self, url):
        self.fetched.append(url)
        if isinstance(self.upstream.get(url), Exception):
            raise self.upstream[url]
        return self.upstream.get(url)

    def crate(self, name="demo", version="1.0.0", commit=OLD, own=(), repository=REPOSITORY):
        """A crate archive with the given files, published from `commit`."""
        for filename in own:
            self.write(f"registry/{name}-{version}/{filename}", "Its own licence\n")
        if commit:
            self.write(f"registry/{name}-{version}/.cargo_vcs_info.json", json.dumps({"git": {"sha1": commit}}))
        return {
            "name": name, "version": version, "license": "MIT", "license_file": None, "repository": repository,
            "manifest_path": str(self.write(f"registry/{name}-{version}/Cargo.toml", "")),
        }

    def tree(self):
        return {
            path.relative_to(self.root).as_posix(): path.read_bytes()
            for path in sorted(self.root.rglob("*")) if path.is_file()
        }

    def sync(self, packages, toolchain="1.2.3"):
        files, removed, manifest, extra, report = sync.plan(
            self.manifest, self.extra, packages, toolchain, self.fetch, self.root
        )
        changed = sync.apply(files, removed, manifest, extra, self.root)
        return manifest, extra, report, changed

    def text(self, path):
        return (self.root / path).read_text(encoding="utf-8")

    def assertNothingWritten(self, error, *packages, toolchain="1.2.3"):
        before = self.tree()
        with self.assertRaisesRegex((ValueError, OSError), error):
            self.sync(list(packages), toolchain)
        self.assertEqual(self.tree(), before)


class RefreshTest(SyncTest):
    def test_an_up_to_date_tree_is_left_as_it_is(self):
        package = self.crate()
        before = self.tree()
        _, _, report, changed = self.sync([package])
        self.assertEqual((report, changed), ([], []))
        self.assertEqual(self.tree(), before)

    def test_an_upgrade_records_its_commit_and_fetches_the_files_from_it(self):
        self.serve(f"{RAW}/{NEW}/LICENSE", "Demo licence, second version\n")
        manifest, _, report, changed = self.sync([self.crate(version="1.1.0", commit=NEW)])
        self.assertEqual(manifest["projects"][0]["crates"], {"demo 1.1.0": NEW})
        self.assertEqual(self.text(LICENCE), "Demo licence, second version\n")
        self.assertEqual(changed, ["scripts/third-party-upstream.json", LICENCE])
        self.assertIn(f"demo 1.1.0: not listed -> {NEW}", report)

    def test_a_wrong_commit_in_the_manifest_is_corrected(self):
        self.manifest["projects"][0]["crates"]["demo 1.0.0"] = OTHER
        self.save()
        manifest, _, _, changed = self.sync([self.crate()])
        self.assertEqual(manifest["projects"][0]["crates"], {"demo 1.0.0": OLD})
        self.assertEqual(changed, ["scripts/third-party-upstream.json"])

    def test_files_that_differ_between_commits_fail(self):
        self.serve(f"{RAW}/{NEW}/LICENSE", "Demo licence, second version\n")
        packages = [self.crate(), self.crate(name="demo-core", commit=NEW)]
        with self.assertRaisesRegex(ValueError, f"LICENSE in {REPOSITORY}, for {REPOSITORY}, differs .*{OLD}.*{NEW}"):
            self.sync(packages)

    def test_a_crate_no_longer_needing_the_files_is_dropped_with_its_project(self):
        for packages in ([], [self.crate(own=["LICENSE"])]):
            with self.subTest(packages=len(packages)):
                self.setUp()
                manifest, _, report, _ = self.sync(packages)
                self.assertEqual(manifest["projects"], [])
                self.assertFalse((self.root / LICENCE).parent.exists())
                self.assertIn(f"Dropped {REPOSITORY}: none of its crates needs its files now", report)

    def test_one_crate_of_two_is_dropped(self):
        manifest, _, report, _ = self.sync([self.crate(name="demo-core")])
        self.assertEqual(manifest["projects"][0]["crates"], {"demo-core 1.0.0": OLD})
        self.assertIn(f"demo 1.0.0: {OLD} -> not listed", report)

    def test_a_crate_in_without_licence_keeps_its_reason_at_its_new_version(self):
        self.manifest["withoutLicence"] = {"brynlee 0.1.0": "Its repository has no licence file."}
        self.save()
        manifest, *_ = self.sync([self.crate(), self.crate(name="brynlee", version="0.2.0")])
        self.assertEqual(manifest["withoutLicence"], {"brynlee 0.2.0": "Its repository has no licence file."})

    def test_the_rust_standard_library_follows_the_toolchain(self):
        self.serve(f"{RUST_RAW}/1.2.4/LICENSE-MIT", "Rust licence, new year\n")
        _, extra, report, changed = self.sync([self.crate()], toolchain="1.2.4")
        self.assertEqual((extra[0]["version"], extra[0]["source"]), ("1.2.4", "https://github.com/rust-lang/rust/tree/1.2.4/library"))
        self.assertEqual(self.text(RUST_LICENCE), "Rust licence, new year\n")
        self.assertEqual(changed, ["scripts/third-party-extra.json", RUST_LICENCE])
        self.assertIn(f"{notices.RUST_STD}: 1.2.3 -> 1.2.4", report)


class FailureTest(SyncTest):
    def test_a_network_failure_part_way_leaves_nothing_written(self):
        self.serve(f"{RAW}/{NEW}/LICENSE", "Demo licence, second version\n")
        self.upstream[f"{RUST_RAW}/1.2.4/LICENSE-MIT"] = TimeoutError("timed out")
        self.assertNothingWritten("timed out", self.crate(version="1.1.0", commit=NEW), toolchain="1.2.4")

    def test_a_file_missing_upstream_leaves_nothing_written(self):
        self.serve(f"{RAW}/{NEW}/LICENSE", "Demo licence, second version\n")
        self.assertNothingWritten("LICENSE-MIT is not in .* at 1.2.4", self.crate(version="1.1.0", commit=NEW), toolchain="1.2.4")

    def test_a_file_that_is_not_text_or_is_blank_fails(self):
        for content, error in ((b"\xff\xfe", "is not UTF-8 text"), (b" \n\t", "is blank")):
            with self.subTest(error=error):
                self.serve(f"{RAW}/{NEW}/LICENSE", content)
                self.assertNothingWritten(f"LICENSE from {REPOSITORY} at {NEW}, for {REPOSITORY}, {error}", self.crate(commit=NEW))

    def test_a_crate_without_a_recorded_commit_fails(self):
        self.assertNothingWritten("demo 1.0.0 .* does not record the commit", self.crate(commit=None))


class NewProjectTest(SyncTest):
    KYNDAL = "https://raw.githubusercontent.com/quillpoint/kyndal"

    def test_a_crate_newly_without_a_licence_gets_a_project_of_its_own(self):
        self.serve(f"{self.KYNDAL}/{OTHER}/LICENSE-MIT", "Kyndal licence\n")
        self.serve(f"{self.KYNDAL}/{OTHER}/NOTICE", "Kyndal notice\n")
        new = self.crate(name="kyndal", commit=OTHER, repository="https://github.com/quillpoint/kyndal.git")
        manifest, _, report, _ = self.sync([self.crate(), new])
        files = ["scripts/third-party-upstream/kyndal/LICENSE-MIT", "scripts/third-party-upstream/kyndal/NOTICE"]
        self.assertEqual(manifest["projects"][1], {
            "repository": "https://github.com/quillpoint/kyndal", "files": files, "crates": {"kyndal 1.0.0": OTHER},
        })
        self.assertEqual(self.text(files[0]), "Kyndal licence\n")
        self.assertIn(f"Added https://github.com/quillpoint/kyndal for kyndal 1.0.0 with {', '.join(files)}."
                      " Check that these are the crates' licence files.", report)
        self.assertTrue(all(url.startswith("https://raw.githubusercontent.com/quillpoint/") or url.startswith(RUST_RAW) for url in self.fetched))

    def test_a_crate_from_a_listed_repository_joins_its_project(self):
        manifest, *_ = self.sync([self.crate(), self.crate(name="demo-macros", repository=f"{REPOSITORY}/tree/main/macros")])
        self.assertEqual(manifest["projects"][0]["crates"], {"demo 1.0.0": OLD, "demo-macros 1.0.0": OLD})

    def test_a_repository_without_a_licence_file_at_its_root_fails(self):
        self.serve(f"{self.KYNDAL}/{OTHER}/NOTICE", "Kyndal notice\n")
        new = self.crate(name="kyndal", commit=OTHER, repository="https://github.com/quillpoint/kyndal")
        self.assertNothingWritten("kyndal 1.0.0 .* none was found at the root of https://github.com/quillpoint/kyndal", self.crate(), new)

    def test_a_crate_without_a_github_repository_fails(self):
        for repository in (None, "https://example.com/quillpoint/kyndal"):
            with self.subTest(repository=repository):
                new = self.crate(name="kyndal", commit=OTHER, repository=repository)
                self.assertNothingWritten("kyndal 1.0.0 .* names no GitHub repository", self.crate(), new)


if __name__ == "__main__":
    unittest.main()
