"""The ASCII title of the CLI.

The art is **hardcoded** in this package (`banner.txt`, declared as package data
in `pyproject.toml`) so an *installed* CLI still has it with no file to find on
disk. Nothing here is dynamic — the banner never carries capture data.
"""

from __future__ import annotations

from importlib.resources import files

#: File name inside this package.
ASSET = "banner.txt"


def art(*, trim: bool = True) -> str:
    """The title as text; ``trim`` drops the art's trailing spaces and lines."""
    try:
        text = files(__package__).joinpath(ASSET).read_text(encoding="utf-8")
    except (FileNotFoundError, ModuleNotFoundError, OSError):  # pragma: no cover
        return ""
    lines = text.splitlines()
    if trim:
        lines = [line.rstrip() for line in lines]
    while lines and not lines[-1]:
        lines.pop()
    return "\n".join(lines)


def banner(tagline: str | None = None) -> str:
    """The title, optionally followed by one tagline line."""
    body = art()
    if tagline and body:
        return f"{body}\n{tagline}"
    return body or (tagline or "")
