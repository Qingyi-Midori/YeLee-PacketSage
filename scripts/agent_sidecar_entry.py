"""PyInstaller entry point for the Agent sidecar (U2).

`packetsage_agent/__main__.py` uses a relative import, which cannot be the
top-level script of a frozen bundle (`ImportError: attempted relative import
with no known parent package`). This file is the same entry point the console
script uses (`packetsage_agent.cli:main`), spelled as an absolute import.

    python scripts/build_agent_sidecar.py     # freezes this file
"""

from __future__ import annotations

from packetsage_agent.cli import main

if __name__ == "__main__":
    raise SystemExit(main())
