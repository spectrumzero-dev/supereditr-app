"""Tests for scripts/third-party-notices.py that need neither Cargo nor npm. Run by scripts/check.sh."""

import importlib.util
import sys
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

SCRIPT = Path(__file__).resolve().parent.parent / "scripts/third-party-notices.py"
sys.dont_write_bytecode = True
spec = importlib.util.spec_from_file_location("third_party_notices", SCRIPT)
notices = importlib.util.module_from_spec(spec)
spec.loader.exec_module(notices)


class CrateTest(unittest.TestCase):
    def test_a_crate_without_a_licence_file_keeps_its_notice_with_standard_texts(self):
        with tempfile.TemporaryDirectory() as directory:
            notice = Path(directory) / "NOTICE"
            notice.write_text("Example notice\n", encoding="utf-8")
            package = {
                "name": "demo", "version": "1.0.0", "license": "MIT OR Apache-2.0",
                "license_file": None, "manifest_path": str(notice.with_name("Cargo.toml")),
            }

            files, fallback = notices.crate_licences(package)
            spdx = notices.ROOT / "scripts/third-party-spdx"
            self.assertTrue(fallback)
            self.assertEqual(files, [
                ("NOTICE", notice, None),
                ("Apache-2.0.txt", spdx / "Apache-2.0.txt", "spdx"),
                ("MIT.txt", spdx / "MIT.txt", "spdx"),
            ])

            package["license"] = None
            with self.assertRaisesRegex(ValueError, "No licence files or SPDX expression for demo 1.0.0"):
                notices.crate_licences(package)


class RuntimeTest(unittest.TestCase):
    def test_rust_standard_library_follows_the_toolchain_channel(self):
        with tempfile.TemporaryDirectory() as directory, patch.object(notices, "ROOT", Path(directory)):
            toolchain = notices.ROOT / "rust-toolchain.toml"
            for version in ("1.90.0", "1.91.0"):
                with self.subTest(version=version):
                    toolchain.write_text(f'[toolchain]\nchannel = "{version}"\n', encoding="utf-8")
                    rust, = notices.runtime_licences([])
                    self.assertEqual(rust["name"], "Rust standard library")
                    self.assertEqual(rust["version"], version)
                    self.assertEqual(rust["source"], f"https://github.com/rust-lang/rust/tree/{version}/library")
                    self.assertEqual([(name, origin) for name, _, origin in rust["files"]], [
                        ("Apache-2.0.txt", "spdx"), ("MIT.txt", "spdx"),
                    ])

            toolchain.write_text("[toolchain]\n", encoding="utf-8")
            with self.assertRaisesRegex(ValueError, r"^No channel in rust-toolchain\.toml$"):
                next(notices.runtime_licences([]))

    def test_webview2_loader_follows_the_shipped_crate(self):
        rust, = notices.runtime_licences([{"name": "webview2-com", "version": "2.0.0"}])
        self.assertEqual(rust["name"], "Rust standard library")
        for version in ("2.0.0", "2.1.0"):
            with self.subTest(version=version):
                _, loader = notices.runtime_licences([{"name": "webview2-com-sys", "version": version}])
                self.assertEqual(loader["name"], "WebView2 loader (Windows)")
                self.assertEqual(loader["version"], version)
                self.assertEqual(loader["source"], "https://www.nuget.org/packages/Microsoft.Web.WebView2")
                self.assertEqual([(name, origin) for name, _, origin in loader["files"]], [
                    ("BSD-3-Clause.txt", "spdx"),
                ])


if __name__ == "__main__":
    unittest.main()
