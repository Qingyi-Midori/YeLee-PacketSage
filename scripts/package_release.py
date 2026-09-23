#!/usr/bin/env python3
"""Builds the release tarball described by CLI 收口工程规格书 §12.

    python scripts/package_release.py --binary target/release/packetsage

The archive is named ``packetsage-<version>-<os>-<arch>.tar.gz`` and contains:

* the `packetsage` binary;
* ``rules/builtin/`` — the four embedded rules, so a user can point
  ``--rules-dir`` at an editable copy;
* ``SHA256SUMS`` for every file in the archive;
* ``INSTALL.md`` — the capability matrix of the tarball path (B3): everything
  except `chat`, because the tarball does not ship the Python agent.

The version string is read from `packetsage version`, i.e. from the same single
source of truth as the runtime (§5).
"""

from __future__ import annotations

import argparse
import hashlib
import platform
import re
import shutil
import subprocess
import sys
import tarfile
import tempfile
from pathlib import Path


def version_of(binary: Path) -> str:
    """Reads the version from the binary itself (single source of truth, §5)."""
    result = subprocess.run(
        [str(binary), "version"], capture_output=True, text=True, timeout=60
    )
    if result.returncode != 0:
        raise SystemExit(f"`{binary} version` failed: {result.stderr.strip()}")
    match = re.search(r"packetsage\s+(\S+)", result.stdout)
    if not match:
        raise SystemExit(f"unexpected version output: {result.stdout!r}")
    return match.group(1)


def platform_tag() -> str:
    machine = platform.machine().lower() or "unknown"
    aliases = {"amd64": "x86_64", "arm64": "aarch64"}
    machine = aliases.get(machine, machine)
    if sys.platform.startswith("win"):
        os_name = "windows"
    elif sys.platform == "darwin":
        os_name = "macos"
    else:
        os_name = "linux"
    return f"{os_name}-{machine}"


def sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for chunk in iter(lambda: handle.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def main(argv: list[str] | None = None) -> int:
    root = Path(__file__).resolve().parents[1]
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", default=None, help="path to the built packetsage binary")
    parser.add_argument("--out-dir", default="dist", help="where to write the tarball")
    args = parser.parse_args(argv)

    binary = Path(args.binary) if args.binary else root / "target" / "release" / "packetsage"
    if not binary.exists() and binary.with_suffix(".exe").exists():
        binary = binary.with_suffix(".exe")
    if not binary.exists():
        print(f"binary not found: {binary}", file=sys.stderr)
        return 2

    install_md = root / "INSTALL.md"
    if not install_md.exists():
        print("INSTALL.md is missing at the repository root", file=sys.stderr)
        return 2
    builtin = root / "crates" / "packetsage-rules" / "src" / "builtin"
    if not builtin.is_dir():
        print(f"builtin rules not found: {builtin}", file=sys.stderr)
        return 2

    version = version_of(binary)
    name = f"packetsage-{version}-{platform_tag()}"
    out_dir = root / args.out_dir
    out_dir.mkdir(parents=True, exist_ok=True)
    tarball = out_dir / f"{name}.tar.gz"

    with tempfile.TemporaryDirectory(prefix="packetsage-release-") as staging:
        stage = Path(staging) / name
        (stage / "rules" / "builtin").mkdir(parents=True)
        shutil.copy2(binary, stage / binary.name)
        for rule in sorted(builtin.glob("*.yaml")):
            shutil.copy2(rule, stage / "rules" / "builtin" / rule.name)
        shutil.copy2(install_md, stage / "INSTALL.md")

        sums = []
        for path in sorted(stage.rglob("*")):
            if path.is_file() and path.name != "SHA256SUMS":
                relative = path.relative_to(stage).as_posix()
                sums.append(f"{sha256(path)}  {relative}")
        (stage / "SHA256SUMS").write_text("\n".join(sums) + "\n", encoding="utf-8")

        with tarfile.open(tarball, "w:gz") as archive:
            archive.add(stage, arcname=name)

    print(f"wrote {tarball} ({tarball.stat().st_size} bytes, sha256={sha256(tarball)})")
    print(
        "contents: binary + rules/builtin/ + INSTALL.md + SHA256SUMS "
        "(no Python agent: the tarball path has no `chat` capability — see INSTALL.md)"
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
