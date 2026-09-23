"""The single source of the package version.

``pyproject.toml`` (PEP 621) is authoritative. Installed distributions answer
through :mod:`importlib.metadata`; a source checkout without an install falls
back to reading ``agent/pyproject.toml`` itself, so the version is never
duplicated in two places that could drift (Agent CLI 工程规格书 §2).
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
    return _from_metadata() or _from_source_tree() or UNKNOWN_VERSION
