#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-3.0-or-later
"""Build the opt-in application adapter without linking a Qt implementation."""
from pathlib import Path
import os
import shlex
import subprocess

root = Path(__file__).resolve().parents[1]
out = root / "target/release/libxr-qt-decoration.so"
out.parent.mkdir(parents=True, exist_ok=True)
subprocess.run(shlex.split(os.environ.get("CXX", "c++")) + [
    "-std=c++17", "-shared", "-fPIC", "-O2", "-Wall", "-Wextra",
    "-fno-gnu-unique", "-Wl,-z,relro", "-Wl,-z,lazy",
    "-ffile-prefix-map=" + str(root) + "=/usr/src/xr-wayland",
    "-o", str(out), str(root / "compat/qt-decoration.cpp"), "-ldl",
], check=True)
