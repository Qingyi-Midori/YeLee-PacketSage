"""The agent's prompt texts.

Two versions ship side by side (`《Agent 系统提示词规格 v0.1》` §0.2):

* **v1** — M3~M6 §4.5, kept as the rollback and evaluation baseline;
* **v2** — the current default: R1–R6 non-negotiables, tool strategy,
  investigation discipline, run/chat output blocks, budget and voice.

The texts are *package data* (`prompts_text/*.txt`, extracted verbatim from the
specs) and this module is the only code: `render_system_prompt()` has exactly
two inputs — `task_id` and `mode` — so no capture content, tool result or
user text can ever reach the system prompt (M3~M6 §4.5 防注入面最小化).
"""

from __future__ import annotations

from importlib.resources import files
from typing import Literal

#: The version used when `agent.prompt_version` is not configured (§0.3 S5).
DEFAULT_PROMPT_VERSION = "v2"

#: Directory of the prompt texts inside this package.
TEXT_DIR = "prompts_text"

#: Where the two output blocks of v2 start/stop; the renderer keeps exactly one.
RUN_BLOCK = "## Output \u2014 run mode"
CHAT_BLOCK = "## Output \u2014 chat mode"
TAIL_BLOCK = "## Budget and stopping"

MODES: tuple[str, ...] = ("run", "chat")
Mode = Literal["run", "chat"]


class PromptVersionError(ValueError):
    """An unknown `agent.prompt_version`."""


def _read(name: str) -> str:
    """Reads one prompt text; an empty string when the package data is missing."""
    try:
        return (
            files(__package__)
            .joinpath(TEXT_DIR)
            .joinpath(name)
            .read_text(encoding="utf-8")
        )
    except (FileNotFoundError, ModuleNotFoundError, OSError):  # pragma: no cover
        return ""


V1 = _read("v1.txt")
V2 = _read("v2.txt")
#: The Chinese answer format for chat (§ Source: `System Prompt-v0.1.txt`).
CHAT_ANSWER_FORMAT_ZH = _read("chat_zh.txt")

#: Every version that `agent.prompt_version` may select.
PROMPTS: dict[str, str] = {"v1": V1, "v2": V2}

#: Legacy alias: the version this build ships by default.
PROMPT_VERSION = DEFAULT_PROMPT_VERSION


def available_versions() -> tuple[str, ...]:
    """Configured-selectable versions, newest first."""
    return tuple(sorted(PROMPTS, reverse=True))


def _select_mode(template: str, mode: Mode) -> str:
    """Keeps the run or chat output block; v1 has no blocks to choose from."""
    if RUN_BLOCK not in template:
        return template
    head, rest = template.split(RUN_BLOCK, 1)
    run_body, rest = rest.split(CHAT_BLOCK, 1)
    chat_body, tail = rest.split(TAIL_BLOCK, 1)
    if mode == "run":
        return f"{head}{RUN_BLOCK}{run_body}{TAIL_BLOCK}{tail}"
    chinese = CHAT_ANSWER_FORMAT_ZH.strip()
    zh_block = f"{chinese}\n\n" if chinese else ""
    return f"{head}{CHAT_BLOCK}{chat_body}{zh_block}{TAIL_BLOCK}{tail}"


def render_system_prompt(
    task_id: str,
    mode: Mode = "run",
    version: str | None = None,
) -> str:
    """Renders the system prompt for one task.

    Only `{task_id}` is substituted and only the `mode` block is selected;
    everything else is byte-for-byte static.
    """
    chosen = version or DEFAULT_PROMPT_VERSION
    if chosen not in PROMPTS:
        raise PromptVersionError(
            f"unknown prompt version {chosen!r} — available: "
            f"{', '.join(available_versions())}"
        )
    if mode not in MODES:
        raise PromptVersionError(f"unknown prompt mode {mode!r} — expected run or chat")
    return _select_mode(PROMPTS[chosen], mode).replace("{task_id}", task_id)


def system_prompt(task_id: str) -> str:
    """Run-mode system prompt of the default version (provider convenience)."""
    return render_system_prompt(task_id, "run")


#: One line per tool, injected into the first user message (M3~M6 §4.5).
TOOL_HINTS = (
    "get_capture_summary: low cost overview (packets, bytes, protocols, sessions)",
    "get_protocol_stats: per layer counters, use layer = link|network|transport|application",
    "get_conversations: top sessions by bytes|packets|duration",
    "check_alerts: rule alerts already detected by the deterministic engine",
    "filter_packets: packet indices for a 5-tuple/flag/time filter",
    "inspect_packets: bounded preview of specific packets",
    "reconstruct_stream: bounded TCP payload reconstruction",
    "query_history: stored alerts/findings/sessions",
    "get_task_artifacts: report path and counters",
)

FINALIZE_INSTRUCTION = """Summarise with strict JSON:
{"summary": str,
 "findings": [{"title": str, "severity": high|medium|low|info,
  "basis": rule_match|direct_observation|correlated_observation|hypothesis,
  "summary": str, "evidence": [{"_id": "tc_01J9Z4M8YQ2V7C1W3N5B6D8FGH", "ref_id": "S-000001"}]}]}
`summary` is the answer a human reads first: 2–4 句中文，说清这份抓包到底怎么回事、
依据是哪个工具结果，以及还有什么没查清。不要罗列 finding 标题，也不要把证书/包号堆进去。
Only cite _id values that appear in your tool results. If there is no evidence for a
claim, drop the finding instead of guessing."""

#: `chat` 模式的收尾指令：**回答用户那句话**，而不是汇报"有没有可疑行为"。
#: （用户 2026-09-22 实测：只给 run 版指令时，追问会得到一份 findings 汇报，问题本身没人答。）
CHAT_FINALIZE_INSTRUCTION = """Answer with strict JSON:
{"summary": str, "findings": [...]}
`summary` **就是你的回答**：用用户的语言，2–6 句，直接回答他问的那件事——用到哪些工具结果、
数字是多少、tc 锚点是什么；答不了的部分要明说"抓包里没有能回答这个的字段"，不要绕开问题。
`findings` 只在用户明确要求落成结论时才给，否则给空数组。
Only cite _id values that appear in your tool results."""


def finalize_instruction(mode: str) -> str:
    """按模式给收尾指令（`run` = 汇报结论，`chat` = 回答问题）。"""
    return CHAT_FINALIZE_INSTRUCTION if mode == "chat" else FINALIZE_INSTRUCTION


#: 追问的**兜底回答**指令：run 若因预算/重复调用被提前收尾，模型没机会写回答，
#: agent 会拿这段再问一次（不带工具），保证"问了一定有答案"。
CHAT_ANSWER_ONLY = """Now answer the user's question with strict JSON, **no tool calls**:
{"summary": str}
`summary` 用 2–4 句直接回答；只依据上面出现过的工具结果，数字与 tc 锚点照抄；
答不了就明说"这份抓包里没有能回答这个的字段"。"""


def tool_listing() -> str:
    """Compact tool listing injected into the user message."""
    return "\n".join(f"- {line}" for line in TOOL_HINTS)
