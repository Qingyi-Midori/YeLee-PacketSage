"""YeLee' PacketSage agent package (M4/M5).

The agent never talks to the database: every read and write goes through the
Rust engine's JSONL RPC (ADR-018).
"""

from ._version import package_version
from .engine_client import (
    EngineClient,
    EngineCrashed,
    EngineSpawnError,
    RpcToolError,
    ToolTimeout,
)
from .models import FindingDraft, ToolCallRecord
from .prompts import (
    DEFAULT_PROMPT_VERSION,
    PROMPT_VERSION,
    PROMPTS,
    available_versions,
    render_system_prompt,
)

__all__ = [
    "EngineClient",
    "EngineCrashed",
    "EngineSpawnError",
    "FindingDraft",
    "DEFAULT_PROMPT_VERSION",
    "PROMPTS",
    "PROMPT_VERSION",
    "RpcToolError",
    "ToolCallRecord",
    "ToolTimeout",
    "available_versions",
    "render_system_prompt",
]

__version__ = package_version()
SCHEMA_VERSION = 2
MAX_RESULT_CHARS = 8000
MAX_STREAM_BYTES = 262_144
