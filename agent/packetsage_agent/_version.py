"""The single source of the package version.

``pyproject.toml`` (PEP 621) is authoritative. 挨着包的 ``pyproject.toml``
（源码树、以及打包时一起塞进 bundle 的那份）**优先**；找不到它才问
:mod:`importlib.metadata`——已安装的发行版总要有个答案，而 editable 安装周围的
源码树就是它自己的那一份。这样版本号永远不会在两个地方各说各话
（Agent CLI 工程规格书 §2）。

顺序为什么是"源树优先"（2026-09-23 修）：反过来写的时候，改完
``agent/pyproject.toml`` 却忘了 ``pip install -e agent``，跑起来的 CLI 与
``agent/tests/test_cli.py::test_version_matches_pyproject`` 都还念着上一次安装时的
旧版本号——一天里踩了两次。
"""

from __future__ import annotations

import re
from pathlib import Path

#: Distribution name in ``pyproject.toml``.
DISTRIBUTION = "packetsage-agent"

_VERSION_RE = re.compile(r'^\s*version\s*=\s*"([^"]+)"', re.MULTILINE)

#: Only used when neither the metadata nor the source ``pyproject.toml`` is
#: readable; it is deliberately not a second source of truth.
UNKNOWN_VERSION = "0.0.0+unknown"


def _from_metadata() -> str | None:
    try:
        from importlib.metadata import PackageNotFoundError, version
    except ImportError:  # pragma: no cover - stdlib since 3.8
        return None
    try:
        return version(DISTRIBUTION)
    except PackageNotFoundError:
        return None
    except Exception:  # pragma: no cover - a broken metadata store
        return None


def _from_source_tree() -> str | None:
    pyproject = Path(__file__).resolve().parent.parent / "pyproject.toml"
    try:
        text = pyproject.read_text(encoding="utf-8")
    except OSError:
        return None
    match = _VERSION_RE.search(text)
    return match.group(1) if match else None


def package_version() -> str:
    """Version of the running agent, or ``0.0.0+unknown``."""
    return _from_source_tree() or _from_metadata() or UNKNOWN_VERSION
