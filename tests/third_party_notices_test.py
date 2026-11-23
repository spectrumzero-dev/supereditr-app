"""Tests for scripts/third-party-notices.py that need neither Cargo nor npm. Run by scripts/check.sh."""

import importlib.util
import json
import sys
import tempfile
import unittest
from pathlib import Path

SCRIPT = Path(__file__).resolve().parent.parent / "scripts/third-party-notices.py"
# Keep the import from writing a __pycache__ folder into scripts/.
sys.dont_write_bytecode = True
spec = importlib.util.spec_from_file_location("third_party_notices", SCRIPT)
notices = importlib.util.module_from_spec(spec)
spec.loader.exec_module(notices)

COMMIT = "0123456789abcdef0123456789abcdef01234567"


class Fixture(unittest.TestCase):
    def setUp(self):
        directory = tempfile.TemporaryDirectory()
        self.addCleanup(directory.cleanup)
        self.root = Path(directory.name)

    def write(self, path, text="text\n"):
        path = self.root / path
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(text, encoding="utf-8")
        return path

    def manifest(self, files, without_licence=None, **project):
        return {
            "withoutLicence": without_licence or {},
            "projects": [{"repository": "https://example.com/demo", "files": files, "crates": {"demo 1.0.0": COMMIT}, **project}],
        }

    def crate(self, own=(), licence="MIT"):
        """A crate archive with the given files, published from COMMIT."""
        for name in own:
            self.write(f"crate/{name}")
        self.write("crate/.cargo_vcs_info.json", json.dumps({"git": {"sha1": COMMIT}}))
        return {
            "name": "demo", "version": "1.0.0", "license": licence, "license_file": None,
            "manifest_path": str(self.write("crate/Cargo.toml")),
        }

    def licences(self, manifest, package):
        upstream, without_licence = notices.upstream_licences(manifest, self.root)
        files, fallback, source = notices.crate_licences(package, upstream, without_licence, self.root)
        listed = notices.entry("crate", "demo", "1.0.0", package["license"], "", files, fallback, source)
        return sorted(((filename, origin) for filename, _, origin in listed["files"]), key=lambda file: (file[0], file[1] or ""))


class ManifestTest(Fixture):
    def test_a_project_without_files_fails(self):
        with self.assertRaisesRegex(ValueError, "lists no licence file"):
            notices.upstream_licences(self.manifest([]), self.root)

    def test_a_project_with_only_notice_files_fails(self):
        self.write("up/NOTICE")
        with self.assertRaisesRegex(ValueError, "lists no licence file"):
            notices.upstream_licences(self.manifest(["up/NOTICE"]), self.root)

    def test_a_missing_file_fails(self):
        self.write("up/LICENSE")
        with self.assertRaisesRegex(ValueError, "up/NOTICE, listed for .* does not exist"):
            notices.upstream_licences(self.manifest(["up/LICENSE", "up/NOTICE"]), self.root)

    def test_a_crate_without_licence_needs_a_reason(self):
        self.write("up/LICENSE")
        for reason in ("", "  ", None):
            with self.subTest(reason=reason), self.assertRaisesRegex(ValueError, "without a reason"):
                notices.upstream_licences(self.manifest(["up/LICENSE"], {"other 1.0.0": reason}), self.root)

    def test_standard_texts_need_a_reason(self):
        self.write("up/LICENSE.md")
        with self.assertRaisesRegex(ValueError, "standardTexts .* gives no reason"):
            notices.upstream_licences(self.manifest(["up/LICENSE.md"], standardTexts=" "), self.root)

    def test_a_licence_file_and_its_notices_pass(self):
        self.write("up/LICENSE.md")
        self.write("up/NOTICES.md")
        upstream, _ = notices.upstream_licences(self.manifest(["up/LICENSE.md", "up/NOTICES.md"]), self.root)
        self.assertEqual(list(upstream), ["demo 1.0.0"])

    def test_the_checked_in_manifest_is_valid(self):
        manifest = notices.read_json(notices.UPSTREAM)
        upstream, _ = notices.upstream_licences(manifest)
        self.assertTrue(upstream)

    def test_a_blank_licence_file_fails(self):
        self.write("up/LICENSE", " \n\t\n")
        with self.assertRaisesRegex(ValueError, "up/LICENSE, listed for https://example.com/demo .* is blank"):
            notices.upstream_licences(self.manifest(["up/LICENSE"]), self.root)

    def test_a_path_out_of_the_repository_fails(self):
        self.write("LICENSE")
        root = self.root / "repo"
        root.mkdir()
        with self.assertRaisesRegex(ValueError, r"\.\./LICENSE, listed for https://example.com/demo .* no \. or \.\. parts"):
            notices.upstream_licences(self.manifest(["../LICENSE"]), root)

    def test_files_given_as_an_object_fail(self):
        self.write("LICENSE")
        with self.assertRaisesRegex(ValueError, "files for https://example.com/demo .* must be a list"):
            notices.upstream_licences(self.manifest({"LICENSE": False}), self.root)

    def test_a_path_that_is_not_text_fails(self):
        self.write("LICENSE")
        for path in (1, None, "", "  "):
            with self.subTest(path=path), self.assertRaisesRegex(
                ValueError, "files for https://example.com/demo .* empty or non-text path"
            ):
                notices.upstream_licences(self.manifest(["LICENSE", path]), self.root)

    def test_a_path_that_is_not_plain_and_relative_fails(self):
        self.write("up/LICENSE")
        for path in ("up\\LICENSE", str(self.root / "up/LICENSE"), "C:/up/LICENSE", "./up/LICENSE", "up//LICENSE"):
            with self.subTest(path=path), self.assertRaisesRegex(
                ValueError, "listed for https://example.com/demo .* must be a relative path"
            ):
                notices.upstream_licences(self.manifest([path]), self.root)

    def test_a_link_out_of_the_repository_fails(self):
        outside = self.write("LICENSE")
        root = self.root / "repo"
        root.mkdir()
        try:
            (root / "LICENSE").symlink_to(outside)
        except OSError:
            self.skipTest("This system cannot create symbolic links")
        with self.assertRaisesRegex(ValueError, "LICENSE, listed for https://example.com/demo .* outside the repository"):
            notices.upstream_licences(self.manifest(["LICENSE"]), root)

    def test_vendored_files_are_checked_the_same_way(self):
        self.write("LICENSE", "")
        with self.assertRaisesRegex(ValueError, "LICENSE, listed for gitleaks in third-party-extra.json, is blank"):
            notices.listed_files(["LICENSE"], "gitleaks", "third-party-extra.json", self.root)
        with self.assertRaisesRegex(ValueError, "files for gitleaks in third-party-extra.json must be a list"):
            notices.listed_files("LICENSE", "gitleaks", "third-party-extra.json", self.root)

    def test_the_checked_in_vendored_files_are_valid(self):
        for package in notices.read_json(notices.ROOT / "scripts/third-party-extra.json"):
            notices.listed_files(package["files"], package["name"], "third-party-extra.json")


class FilesTest(Fixture):
    def test_the_archive_notice_and_the_repository_notice_are_both_kept(self):
        self.write("up/LICENSE")
        self.write("up/NOTICE")
        files = self.licences(self.manifest(["up/LICENSE", "up/NOTICE"]), self.crate(own=["NOTICE"]))
        self.assertEqual(files, [("LICENSE", "repository"), ("NOTICE", None), ("NOTICE", "repository")])

    def test_repository_files_with_one_name_are_both_kept(self):
        self.write("up/LICENSE")
        self.write("up/vendored/LICENSE")
        files = self.licences(self.manifest(["up/LICENSE", "up/vendored/LICENSE"]), self.crate())
        self.assertEqual(files, [("LICENSE", "repository"), ("vendored/LICENSE", "repository")])

    def test_a_standard_text_named_like_a_repository_file_is_kept(self):
        self.write("up/LICENSE.md")
        self.write("up/MIT.txt")
        manifest = self.manifest(["up/LICENSE.md", "up/MIT.txt"], standardTexts="It names the licences only.")
        files = self.licences(manifest, self.crate())
        self.assertEqual(files, [("LICENSE.md", "repository"), ("MIT.txt", "repository"), ("MIT.txt", "spdx")])

    def test_an_allowed_crate_keeps_its_notice_beside_the_standard_text(self):
        self.write("up/LICENSE")
        manifest = self.manifest(["up/LICENSE"], {"demo 1.0.0": "Its repository has no licence file."})
        manifest["projects"][0]["crates"] = {"other 1.0.0": COMMIT}
        files = self.licences(manifest, self.crate(own=["NOTICE"]))
        self.assertEqual(files, [("MIT.txt", "spdx"), ("NOTICE", None)])

    def test_a_file_listed_twice_fails(self):
        path = self.write("LICENSE")
        with self.assertRaisesRegex(ValueError, "LICENSE is listed twice for demo 1.0.0"):
            notices.entry("vendored", "demo", "1.0.0", "MIT", "", [("LICENSE", path, None)] * 2)

    def test_a_repository_file_without_a_repository_fails(self):
        path = self.write("LICENSE")
        with self.assertRaisesRegex(ValueError, "demo 1.0.0 has a file from its repository but no repository"):
            notices.entry("crate", "demo", "1.0.0", "MIT", "", [("LICENSE", path, "repository")])


if __name__ == "__main__":
    unittest.main()
