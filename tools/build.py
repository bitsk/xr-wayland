#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-3.0-or-later
"""Use the fixed SDK version without copying or vendoring its source."""
import json
import os
from pathlib import Path
import re
import shutil
import subprocess

root = Path(__file__).resolve().parents[1]
build = root / "build/cargo-source"
build.mkdir(parents=True, exist_ok=True)
manifest = (root / "Cargo.toml").read_text()
sibling = root.parent / "xr-shell/protocols"
installed = Path("/usr/share/cargo/registry/xr-shell-protocols-0.2.0")
sdk = Path(os.environ.get("XR_SHELL_PROTOCOLS_DIR", sibling if (sibling / "Cargo.toml").is_file() else installed)).resolve()
if not (sdk / "Cargo.toml").is_file():
    raise SystemExit("Install librust-xr-shell-protocols-dev 0.2.0 or set XR_SHELL_PROTOCOLS_DIR")
actual = re.search(r'^version\s*=\s*"([^"]+)"', (sdk / "Cargo.toml").read_text(), re.M).group(1)
if actual != "0.2.0":
    raise SystemExit("Expected xr-shell-protocols 0.2.0, found " + actual)
manifest = manifest.replace('path = "../xr-shell/protocols"', "path = " + json.dumps(str(sdk)))
manifest = manifest.replace('path = "src/main.rs"', "path = " + json.dumps(str(root / "src/main.rs")))
(build / "Cargo.toml").write_text(manifest)
shutil.copyfile(root / "Cargo.lock", build / "Cargo.lock")
environment = os.environ.copy()
environment.setdefault("CARGO_TARGET_DIR", str(root / "target"))
environment.setdefault("CARGO_BUILD_JOBS", "2")
command = ["cargo", "--manifest-path", str(build / "Cargo.toml"), "--offline", "--locked"]
subprocess.run([command[0], "build", *command[1:], "--release"], env=environment, check=True)
metadata = json.loads(subprocess.check_output([command[0], "metadata", *command[1:], "--format-version", "1"], env=environment))
licenses = root / "build/third-party-licenses"
shutil.rmtree(licenses, ignore_errors=True)
licenses.mkdir(parents=True)
index = []
for package in metadata["packages"]:
    if package["name"] == "xr-wayland":
        continue
    source = Path(package["manifest_path"]).parent
    directory = licenses / (package["name"] + "-" + package["version"])
    texts = []
    for file in sorted(source.iterdir()):
        if file.is_file() and file.name.lower().startswith(("license", "licence", "copying", "notice", "copyright")):
            directory.mkdir(exist_ok=True)
            shutil.copyfile(file, directory / file.name)
            texts.append(file.name)
    for folder in sorted(source.iterdir()):
        if folder.is_dir() and folder.name.lower() in ("license", "licenses", "licence", "licences"):
            for file in sorted(folder.rglob("*")):
                if file.is_file():
                    relative = file.relative_to(source)
                    (directory / relative).parent.mkdir(parents=True, exist_ok=True)
                    shutil.copyfile(file, directory / relative)
                    texts.append(str(relative))
    if package["name"] == "xr-shell-protocols" and not texts:
        license_file = source.parent / "LICENSE.md"
        if license_file.is_file():
            directory.mkdir(exist_ok=True)
            shutil.copyfile(license_file, directory / "LICENSE.md")
            texts.append("LICENSE.md")
    index.append({"name": package["name"], "version": package["version"], "license": package["license"], "texts": texts})
(licenses / "index.json").write_text(json.dumps(index, indent=2) + "\n")

subprocess.run(["python3", str(root / "tools/build-qt-decoration.py")], check=True)
