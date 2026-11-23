#!/usr/bin/env python3
"""Refresh the local secret rules from the pinned Gitleaks release (Python 3.11+)."""
import hashlib
import json
from pathlib import Path
import subprocess
import tomllib
from urllib.request import urlopen

TAG = "v8.30.1"
BASE = f"https://raw.githubusercontent.com/gitleaks/gitleaks/{TAG}"
ROOT = Path(__file__).resolve().parents[1]
DEST = ROOT / "crates/core/src/sensitive"


def download(path, digest):
    with urlopen(f"{BASE}/{path}", timeout=30) as response:
        data = response.read()
    if hashlib.sha256(data).hexdigest() != digest:
        raise SystemExit(f"Unexpected contents for {TAG}/{path}; verify the upstream file and checksum.")
    return data.decode("utf-8")


def main():
    config = download("config/gitleaks.toml", "e163e53b9e7e8a8511e77271e2b323ed057759542a6d988258afe3a1fa329caf")
    license_text = download("LICENSE", "e3884b252b3bfc045e55be43a34d1e80da070bc6f804ac95bf4660e97d62ebc6")
    generated = subprocess.run(
        ["cargo", "run", "--quiet", "--locked", "-p", "supereditr-core", "--example", "sync_gitleaks", "--", TAG],
        cwd=ROOT, input=json.dumps(tomllib.loads(config)["rules"]), text=True,
        stdout=subprocess.PIPE, check=True,
    ).stdout
    generated = subprocess.run(
        ["rustfmt", "--edition", "2024", "--config-path", str(ROOT / "rustfmt.toml")],
        input=generated, text=True, stdout=subprocess.PIPE, check=True,
    ).stdout
    (DEST / "gitleaks.rs").write_text(generated, encoding="utf-8", newline="\n")
    (DEST / "GITLEAKS-LICENSE").write_text(license_text, encoding="utf-8", newline="\n")


if __name__ == "__main__":
    main()
