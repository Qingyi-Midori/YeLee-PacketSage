"""Typed models for tool arguments and findings (M3~M6 §4.3/§4.4).

Pydantic is used when it is importable; otherwise a small dataclass based
validator enforces exactly the same whitelists (severity, basis, arguments).
The behaviour that matters — "an out-of-whitelist severity is rejected and
retried like malformed JSON" — is identical in both paths.
"""

from __future__ import annotations

from collections.abc import Iterable
from dataclasses import dataclass, field
from typing import Any

try:  # pragma: no cover - exercised by whichever branch the host provides

    PYDANTIC_AVAILABLE = True
except Exception:  # pragma: no cover
    PYDANTIC_AVAILABLE = False


SEVERITIES = ("high", "medium", "low", "info")
BASES = ("rule_match", "direct_observation", "correlated_observation", "hypothesis")
VALIDATOR_STATUSES = ("accepted", "downgraded", "rejected")


class ValidationError(ValueError):
    """Raised when a draft or a tool argument fails validation."""


@dataclass
class ToolSpec:
    """One tool exposed to the model."""

    name: str
    args: dict[str, str]
    required: tuple[str, ...] = ()
    description: str = ""
    max_result_chars: int = 8000

    def validate(self, args: dict[str, Any]) -> dict[str, Any]:
        if not isinstance(args, dict):
            raise ValidationError(f"{self.name}: arguments must be an object")
        unknown = set(args) - set(self.args)
        if unknown:
            raise ValidationError(f"{self.name}: unknown argument(s) {sorted(unknown)}")
        missing = [name for name in self.required if name not in args]
        if missing:
            raise ValidationError(f"{self.name}: missing argument(s) {missing}")
        checked: dict[str, Any] = {}
        for name, value in args.items():
            kind = self.args[name]
            checked[name] = _coerce(self.name, name, kind, value)
        return checked

    def json_schema(self) -> dict[str, Any]:
        return {
            "type": "function",
            "function": {
                "name": self.name,
                "description": self.description,
                "parameters": {
                    "type": "object",
                    "properties": {
                        name: {"type": kind} for name, kind in self.args.items()
                    },
                    "required": list(self.required),
                    "additionalProperties": False,
                },
            },
        }


_COERCIONS = {
    "string": lambda value: value if isinstance(value, str) else str(value),
    "integer": lambda value: int(value),
    "number": lambda value: float(value),
    "boolean": lambda value: bool(value),
    "array": lambda value: list(value) if isinstance(value, Iterable) else [value],
    "object": lambda value: dict(value),
}


def _coerce(tool: str, name: str, kind: str, value: Any) -> Any:
    coerce = _COERCIONS.get(kind)
    if coerce is None:
        raise ValidationError(f"{tool}.{name}: unsupported type {kind!r}")
    try:
        if kind == "integer" and isinstance(value, bool):
            raise ValueError("booleans are not integers")
        return coerce(value)
    except (TypeError, ValueError) as exc:
        raise ValidationError(f"{tool}.{name}: {value!r} is not a {kind}") from exc


@dataclass
class EvidenceRef:
    """Evidence anchor: the engine-issued `_id` plus an optional entity id."""

    _id: str
    method: str
    ref_id: str | None = None
    ts_unix_ns: str | None = None
    summary: str | None = None

    def as_dict(self) -> dict[str, Any]:
        payload: dict[str, Any] = {"_id": self._id, "method": self.method}
        if self.ref_id is not None:
            payload["ref_id"] = self.ref_id
        if self.ts_unix_ns is not None:
            payload["ts_unix_ns"] = self.ts_unix_ns
        if self.summary is not None:
            payload["summary"] = self.summary
        return payload


@dataclass
class NumericClaim:
    """A structured number the model wants to state (V4 input)."""

    _id: str
    field: str
    value: float

    def as_dict(self) -> dict[str, Any]:
        return {"_id": self._id, "field": self.field, "value": self.value}


@dataclass
class FindingDraft:
    """A finding proposed by the model."""

    title: str
    severity: str
    basis: str
    summary: str
    evidence: list[EvidenceRef] = field(default_factory=list)
    claims: list[NumericClaim] = field(default_factory=list)

    def validate(self) -> None:
        if not self.title.strip():
            raise ValidationError("finding.title must not be empty")
        if self.severity not in SEVERITIES:
            raise ValidationError(
                f"finding.severity {self.severity!r} is not one of {SEVERITIES}"
            )
        if self.basis not in BASES:
            raise ValidationError(f"finding.basis {self.basis!r} is not one of {BASES}")
        if not self.summary.strip():
            raise ValidationError("finding.summary must not be empty")
        if not self.evidence:
            raise ValidationError("finding.evidence must cite at least one tool result")

    def as_dict(self) -> dict[str, Any]:
        return {
            "title": self.title,
            "severity": self.severity,
            "basis": self.basis,
            "summary": self.summary,
            "evidence": [item.as_dict() for item in self.evidence],
            "claims": [claim.as_dict() for claim in self.claims],
        }


@dataclass
class ToolCallRecord:
    """One entry of the analysis trace (no chain of thought, ADR-007)."""

    step: int
    tool_name: str
    args_json: str
    result_summary: str
    status: str
    duration_ms: int
    tc_id: str | None = None
    #: 决定这次调用的那次 LLM 往返耗时（毫秒）。工具慢还是模型慢，这一列说得清。
    llm_ms: int = 0


#: `ref_id` 的合法分隔符：模型经常把多个 id 写成 `"10,35,60"`（V3 只认单个）。
_REF_ID_SEPARATORS: tuple[str, ...] = ("，", ",", "、", ";", "；", "/", "|")


def split_ref_ids(value: Any) -> list[str | None]:
    """把一个 `ref_id` 拆成若干单个 id（`None` = 引用整个工具结果）。

    只做"把一串拆开"这一件事：每个 id 仍要自己过 V3（not reachable 照样拒），
    所以这不是替模型补引用，而是不让它因为**写法**丢掉一条结论（2026-09-23 实测：
    `"10,35,60"` 被判不可达，整条 finding 被拒）。
    """
    if value is None:
        return [None]
    text = str(value).strip()
    if not text:
        return [None]
    parts = [text]
    for separator in _REF_ID_SEPARATORS:
        parts = [piece for chunk in parts for piece in chunk.split(separator)]
    cleaned: list[str | None] = [piece.strip() for piece in parts if piece.strip()]
    return cleaned or [None]


def parse_finding(payload: dict[str, Any], evidence_lookup: dict[str, str]) -> FindingDraft:
    """Builds a draft from model output, resolving evidence `_id`s."""
    if not isinstance(payload, dict):
        raise ValidationError("finding payload must be an object")
    evidence: list[EvidenceRef] = []
    for item in payload.get("evidence", []):
        if isinstance(item, str):
            item = {"_id": item}
        if not isinstance(item, dict) or "_id" not in item:
            raise ValidationError("evidence entries need an `_id`")
        tc_id = str(item["_id"])
        method = str(item.get("method") or evidence_lookup.get(tc_id, "unknown"))
        for ref_id in split_ref_ids(item.get("ref_id")):
            evidence.append(
                EvidenceRef(
                    _id=tc_id,
                    method=method,
                    ref_id=ref_id,
                    ts_unix_ns=item.get("ts_unix_ns"),
                    summary=item.get("summary"),
                )
            )
    claims: list[NumericClaim] = []
    for claim in payload.get("claims", []) or []:
        if not isinstance(claim, dict):
            raise ValidationError("claims entries must be objects")
        claims.append(
            NumericClaim(
                _id=str(claim["_id"]),
                field=str(claim["field"]),
                value=float(claim["value"]),
            )
        )
    draft = FindingDraft(
        title=str(payload.get("title", "")),
        severity=str(payload.get("severity", "")),
        basis=str(payload.get("basis", "")),
        summary=str(payload.get("summary", "")),
        evidence=evidence,
        claims=claims,
    )
    draft.validate()
    return draft
