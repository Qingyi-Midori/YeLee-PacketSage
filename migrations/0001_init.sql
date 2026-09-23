-- PacketSage initial schema (开发文档 §19, M3~M6 §3.7).
-- Both dialects must be able to run this file: no AUTOINCREMENT, no
-- SQLite-only types, timestamps are TEXT in RFC3339.

CREATE TABLE IF NOT EXISTS analysis_tasks (
    id TEXT PRIMARY KEY,
    status TEXT NOT NULL,
    source_path TEXT NOT NULL,
    source_sha256 TEXT NOT NULL,
    started_at TEXT NOT NULL,
    finished_at TEXT,
    packet_count INTEGER,
    byte_count INTEGER,
    error_code TEXT,
    index_mode TEXT NOT NULL DEFAULT 'FULL',
    rules_hash TEXT NOT NULL DEFAULT ''
);

CREATE TABLE IF NOT EXISTS captures (
    task_id TEXT PRIMARY KEY,
    format TEXT NOT NULL,
    first_ts TEXT,
    last_ts TEXT,
    interfaces INTEGER,
    linktypes_json TEXT NOT NULL DEFAULT '[]',
    FOREIGN KEY(task_id) REFERENCES analysis_tasks(id)
);

CREATE TABLE IF NOT EXISTS sessions (
    id TEXT PRIMARY KEY,
    task_id TEXT NOT NULL,
    protocol TEXT NOT NULL,
    src_ip TEXT NOT NULL,
    src_port INTEGER,
    dst_ip TEXT NOT NULL,
    dst_port INTEGER,
    first_ts TEXT NOT NULL,
    last_ts TEXT NOT NULL,
    packets INTEGER NOT NULL,
    bytes INTEGER NOT NULL,
    state TEXT,
    app_protocol TEXT,
    interface_id INTEGER,
    vlan_tag INTEGER,
    direction_basis TEXT NOT NULL DEFAULT 'syn_first',
    FOREIGN KEY(task_id) REFERENCES analysis_tasks(id)
);

CREATE INDEX IF NOT EXISTS sessions_task_bytes ON sessions (task_id, bytes DESC);

CREATE TABLE IF NOT EXISTS alerts (
    id TEXT PRIMARY KEY,
    task_id TEXT NOT NULL,
    rule_id TEXT NOT NULL,
    rule_version INTEGER NOT NULL DEFAULT 1,
    rule_content_hash TEXT NOT NULL DEFAULT '',
    severity TEXT NOT NULL,
    first_packet INTEGER NOT NULL DEFAULT 0,
    last_packet INTEGER NOT NULL DEFAULT 0,
    first_ts TEXT,
    last_ts TEXT,
    src_ip TEXT,
    dst_ip TEXT,
    session_id TEXT,
    group_json TEXT NOT NULL DEFAULT '[]',
    evidence_json TEXT NOT NULL,
    FOREIGN KEY(task_id) REFERENCES analysis_tasks(id)
);

CREATE INDEX IF NOT EXISTS alerts_task_severity ON alerts (task_id, severity);

CREATE TABLE IF NOT EXISTS agent_runs (
    id TEXT PRIMARY KEY,
    task_id TEXT NOT NULL,
    model TEXT NOT NULL,
    status TEXT NOT NULL,
    started_at TEXT NOT NULL,
    finished_at TEXT,
    report_path TEXT,
    prompt_version TEXT NOT NULL DEFAULT 'v1',
    temperature REAL NOT NULL DEFAULT 0,
    tokens_in INTEGER NOT NULL DEFAULT 0,
    tokens_out INTEGER NOT NULL DEFAULT 0,
    cost_cents INTEGER NOT NULL DEFAULT 0,
    FOREIGN KEY(task_id) REFERENCES analysis_tasks(id)
);

CREATE TABLE IF NOT EXISTS tool_calls (
    id TEXT PRIMARY KEY,
    agent_run_id TEXT NOT NULL,
    step INTEGER NOT NULL DEFAULT 0,
    tool_name TEXT NOT NULL,
    args_json TEXT NOT NULL DEFAULT '{}',
    result_summary TEXT,
    status TEXT NOT NULL DEFAULT 'ok',
    duration_ms INTEGER NOT NULL DEFAULT 0,
    created_at TEXT NOT NULL,
    envelope_hash TEXT,
    redactions_json TEXT,
    FOREIGN KEY(agent_run_id) REFERENCES agent_runs(id)
);

CREATE INDEX IF NOT EXISTS tool_calls_run ON tool_calls (agent_run_id, step);

CREATE TABLE IF NOT EXISTS findings (
    id TEXT PRIMARY KEY,
    task_id TEXT NOT NULL,
    title TEXT NOT NULL,
    severity TEXT NOT NULL,
    basis TEXT NOT NULL,
    summary TEXT NOT NULL,
    evidence_json TEXT NOT NULL DEFAULT '[]',
    validator_status TEXT NOT NULL DEFAULT 'accepted',
    FOREIGN KEY(task_id) REFERENCES analysis_tasks(id)
);

CREATE TABLE IF NOT EXISTS artifacts (
    id TEXT PRIMARY KEY,
    task_id TEXT NOT NULL,
    kind TEXT NOT NULL,
    path TEXT NOT NULL,
    sha256 TEXT NOT NULL DEFAULT '',
    created_at TEXT NOT NULL,
    FOREIGN KEY(task_id) REFERENCES analysis_tasks(id)
);
