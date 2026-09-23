"""The ASCII title is hardcoded into the package (Agent CLI §2).

`packetsage_agent/banner.txt` ships as package data and is what an installed
CLI actually reads; the tests below freeze its shape so an accidental edit
fails here rather than in someone's console.
"""

from __future__ import annotations

import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

from packetsage_agent import banner as banner_module  # noqa: E402

#: Hardcoded first line — the one a reader recognises.
FIRST_LINE = "   :::   ::: :::::::::: :::        :::::::::: :::::::::: :::"

#: Hardcoded line count of the art.
LINES = 14


def test_art_is_present_and_ascii():
    title = banner_module.art()
    assert title
    assert title.isascii(), "the title must survive every console code page"


def test_art_shape_is_frozen():
    lines = banner_module.art().splitlines()
    assert len(lines) == LINES, "the ASCII title changed shape"
    assert lines[0] == FIRST_LINE, "the ASCII title's first line changed"


def test_banner_tagline_is_optional():
    assert banner_module.banner() == banner_module.art()
    assert banner_module.banner("tagline").endswith("tagline")
