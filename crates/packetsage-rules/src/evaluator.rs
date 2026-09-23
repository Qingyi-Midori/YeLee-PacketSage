//! Event driven rule evaluation: sliding windows, late-event tolerance,
//! per-group cooldown, flush and the window budget (M3~M6 §3.3).

use std::collections::{HashMap, VecDeque};

use packetsage_protocol::{
    AlertEvent, AlertEvidence, AppDetail, DecodeErrorEvent, EngineEvent, PacketEvent,
    SessionSummaryEvent, StreamState, TcpFlag, SCHEMA_VERSION,
};

use crate::error::Result;
use crate::loader::{load_dir, load_dir_lenient, CompiledRule, DeadLetter, LoadReport};
use crate::schema::{
    window_ns, GroupField, MatchSpec, Metric, Operator, RuleFile, Scope, Severity,
};

/// Selected built-in rules, embedded in the binary.
pub const BUILTIN_RULES: [(&str, &str); 4] = [
    (
        "NET-TCP-SYN-BURST-001",
        include_str!("builtin/NET-TCP-SYN-BURST-001.yaml"),
    ),
    (
        "NET-TCP-PORT-SWEEP-001",
        include_str!("builtin/NET-TCP-PORT-SWEEP-001.yaml"),
    ),
    (
        "NET-DNS-SUSPICIOUS-001",
        include_str!("builtin/NET-DNS-SUSPICIOUS-001.yaml"),
    ),
    (
        "NET-MALFORMED-BURST-001",
        include_str!("builtin/NET-MALFORMED-BURST-001.yaml"),
    ),
];

/// Memory budget of the window store.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WindowBudget {
    /// Maximum number of distinct `(rule, group)` windows.
    pub max_groups: usize,
    /// Maximum number of retained events per window.
    pub max_events_per_group: usize,
}

impl Default for WindowBudget {
    fn default() -> Self {
        Self {
            max_groups: 50_000,
            max_events_per_group: 4_096,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Sample {
    /// Numeric sample (`bytes`, ports, DNS lengths, entropies).
    Num(f64),
    /// Text sample (`tls.sni`, `http.host`, addresses, codes).
    Text(String),
}

impl Sample {
    fn key(&self) -> String {
        match self {
            Sample::Num(v) => format!("{v}"),
            Sample::Text(v) => v.clone(),
        }
    }

    fn number(&self) -> f64 {
        match self {
            Sample::Num(v) => *v,
            Sample::Text(_) => 0.0,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
struct SampleEvent {
    ts_ns: i128,
    packet_index: u64,
    sample: Sample,
    numerator_hit: bool,
}

#[derive(Debug, Clone)]
struct GroupWindow {
    group: Vec<(String, String)>,
    session_id: Option<String>,
    session_state: Option<StreamState>,
    events: VecDeque<SampleEvent>,
    /// Incremental aggregate state: keeping these up to date turns every metric
    /// evaluation into O(1) instead of a full window scan (window sizes reach
    /// thousands of events, so a scan per packet would be quadratic).
    value_counts: std::collections::HashMap<String, u32>,
    bytes_sum: f64,
    numerator_hits: u64,
    seen_max_ts: i128,
    last_touch_ns: i128,
    cooldown_until: i128,
    degraded: bool,
}

impl GroupWindow {
    fn new(group: Vec<(String, String)>) -> Self {
        Self {
            group,
            session_id: None,
            session_state: None,
            events: VecDeque::new(),
            value_counts: std::collections::HashMap::new(),
            bytes_sum: 0.0,
            numerator_hits: 0,
            seen_max_ts: i128::MIN,
            last_touch_ns: i128::MIN,
            cooldown_until: i128::MIN,
            degraded: false,
        }
    }

    #[allow(clippy::integer_division)] // keep = len / 2 is a deliberate halving
    fn push(&mut self, event: SampleEvent, budget: &WindowBudget) {
        self.seen_max_ts = self.seen_max_ts.max(event.ts_ns);
        self.last_touch_ns = self.last_touch_ns.max(event.ts_ns);
        self.accumulate(&event);
        self.events.push_back(event);
        if self.events.len() > budget.max_events_per_group {
            // Evidence degradation path (M3~M6 §3.3-5): detail is dropped, the
            // alert explicitly declares `degraded: true` with no samples.
            let keep = self.events.len() / 2;
            let drop_count = self.events.len() - keep;
            for _ in 0..drop_count {
                self.events.pop_front();
            }
            self.rebuild_aggregates();
            self.degraded = true;
        }
    }

    fn accumulate(&mut self, event: &SampleEvent) {
        *self.value_counts.entry(event.sample.key()).or_insert(0) += 1;
        self.bytes_sum += event.sample.number();
        if event.numerator_hit {
            self.numerator_hits += 1;
        }
    }

    fn release(&mut self, event: &SampleEvent) {
        let key = event.sample.key();
        if let Some(count) = self.value_counts.get_mut(&key) {
            *count -= 1;
            if *count == 0 {
                self.value_counts.remove(&key);
            }
        }
        self.bytes_sum -= event.sample.number();
        if event.numerator_hit {
            self.numerator_hits = self.numerator_hits.saturating_sub(1);
        }
    }

    fn rebuild_aggregates(&mut self) {
        self.value_counts.clear();
        self.bytes_sum = 0.0;
        self.numerator_hits = 0;
        let replay: Vec<SampleEvent> = self.events.iter().cloned().collect();
        for event in &replay {
            self.accumulate(event);
        }
    }

    /// Drops events older than the window, measured against this group's own
    /// `seen_max_ts` (never the global analysis clock).
    fn prune(&mut self, window_ns: i128) {
        if self.seen_max_ts == i128::MIN {
            return;
        }
        let left_edge = self.seen_max_ts - window_ns;
        while let Some(front) = self.events.front() {
            if front.ts_ns < left_edge {
                if let Some(event) = self.events.pop_front() {
                    self.release(&event);
                }
            } else {
                break;
            }
        }
    }

    fn is_empty(&self) -> bool {
        self.events.is_empty()
    }
}

/// The rule engine.
pub struct RuleEngine {
    rules: Vec<CompiledRule>,
    windows: HashMap<(usize, String), GroupWindow>,
    budget: WindowBudget,
    evictions: u64,
    dead_letters: Vec<DeadLetter>,
    /// Task id of the most recently evaluated event; used by `flush`.
    pub last_task_id: String,
}

impl std::fmt::Debug for RuleEngine {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RuleEngine")
            .field("rules", &self.rules.len())
            .field("windows", &self.windows.len())
            .field("evictions", &self.evictions)
            .finish()
    }
}

impl Default for RuleEngine {
    fn default() -> Self {
        Self::new(WindowBudget::default())
    }
}

impl RuleEngine {
    /// Creates an engine with the built-in rules loaded.
    #[must_use]
    pub fn with_builtin_rules(budget: WindowBudget) -> Self {
        let mut engine = Self::new(budget);
        for (id, text) in BUILTIN_RULES {
            match CompiledRule::from_text(text, std::path::PathBuf::from("<embedded>")) {
                Ok(rule) if rule.file.id == id => engine.rules.push(rule),
                Ok(rule) => tracing::error!(
                    expected = id,
                    found = %rule.file.id,
                    "built-in rule id mismatch"
                ),
                Err(error) => tracing::error!(rule = id, %error, "built-in rule is invalid"),
            }
        }
        engine
    }

    /// Creates an empty engine.
    #[must_use]
    pub fn new(budget: WindowBudget) -> Self {
        Self {
            rules: Vec::new(),
            windows: HashMap::new(),
            budget,
            evictions: 0,
            dead_letters: Vec::new(),
            last_task_id: String::new(),
        }
    }

    /// Loads a rule directory in strict mode.
    ///
    /// # Errors
    /// Returns a rule error when any rule is invalid.
    pub fn load_dir(&mut self, path: &std::path::Path) -> Result<usize> {
        let loaded = load_dir(path)?;
        let count = loaded.len();
        self.rules.extend(loaded);
        Ok(count)
    }

    /// Loads a rule directory in lenient mode.
    ///
    /// # Errors
    /// Returns an IO error only when the directory cannot be listed.
    pub fn load_dir_lenient(&mut self, path: &std::path::Path) -> Result<LoadReport> {
        let report = load_dir_lenient(path, &mut self.rules)?;
        self.dead_letters.extend(report.dead_letters.clone());
        Ok(report)
    }

    /// Adds one rule from text (used by tests and `rules check`).
    ///
    /// # Errors
    /// Returns a rule error when the YAML is invalid.
    pub fn add_rule_text(&mut self, text: &str) -> Result<()> {
        let rule = CompiledRule::from_text(text, std::path::PathBuf::from("<inline>"))?;
        self.rules.push(rule);
        Ok(())
    }

    /// `(loaded, dead-lettered)` rule counts.
    #[must_use]
    pub fn rule_count(&self) -> (usize, usize) {
        (self.rules.len(), self.dead_letters.len())
    }

    /// Loaded rules.
    #[must_use]
    pub fn rules(&self) -> &[CompiledRule] {
        &self.rules
    }

    /// Dead-lettered rules.
    #[must_use]
    pub fn dead_letters(&self) -> &[DeadLetter] {
        &self.dead_letters
    }

    /// Number of windows evicted by the budget.
    #[must_use]
    pub fn window_evictions(&self) -> u64 {
        self.evictions
    }

    /// Evaluates one event.
    pub fn evaluate(&mut self, event: &EngineEvent, now_ns: i128) -> Vec<AlertEvent> {
        let mut alerts = Vec::new();
        for index in 0..self.rules.len() {
            let subject = match (&self.rules[index].file.scope, event) {
                (Scope::Packet, EngineEvent::Packet(packet)) => Subject::Packet(packet),
                (Scope::Packet, EngineEvent::DecodeError(error)) => Subject::Decode(error),
                (Scope::Session, EngineEvent::SessionSummary(summary)) => Subject::Session(summary),
                _ => continue,
            };
            if !matches_subject(&self.rules[index].file, &subject) {
                continue;
            }
            let Some(sample) = sample_of(&self.rules[index].file, &subject) else {
                continue;
            };
            let group = group_of(&self.rules[index].file, &subject);
            let group_name = render_group(&group);
            let key = (index, group_name);
            let window_ns_len = self.rules[index].window_ns;
            let cooldown_ns_len = self.rules[index].cooldown_ns;

            if !self.windows.contains_key(&key) {
                self.make_room(now_ns);
                let mut window = GroupWindow::new(group);
                if let Subject::Session(summary) = &subject {
                    window.session_id = Some(summary.session_id.clone());
                    window.session_state = Some(summary.state);
                }
                self.windows.insert(key.clone(), window);
            }
            let (packet_index, ts_ns) = subject.position();
            let numerator_hit = ratio_hit(&self.rules[index].file, &sample);
            if let Some(window) = self.windows.get_mut(&key) {
                window.push(
                    SampleEvent {
                        ts_ns,
                        packet_index,
                        sample,
                        numerator_hit,
                    },
                    &self.budget,
                );
                window.prune(window_ns_len);
            }
            if let Some(alert) = self.check(event.task_id(), index, &key, now_ns, cooldown_ns_len) {
                alerts.push(alert);
            }
        }
        alerts
    }

    /// Final evaluation before the task is closed.
    ///
    /// Uses the analysis clock as `now` and never synthesises session events.
    pub fn flush(&mut self, now_ns: i128) -> Vec<AlertEvent> {
        self.flush_for_task("", now_ns)
    }

    /// Final evaluation that stamps a task id onto the emitted alerts.
    pub fn flush_for_task(&mut self, task_id: &str, now_ns: i128) -> Vec<AlertEvent> {
        let mut keys: Vec<(usize, String)> = self.windows.keys().cloned().collect();
        keys.sort();
        let mut alerts = Vec::new();
        for key in keys {
            let window_ns_len = self.rules[key.0].window_ns;
            let cooldown_ns_len = self.rules[key.0].cooldown_ns;
            if let Some(window) = self.windows.get_mut(&key) {
                window.prune(window_ns_len);
            }
            if let Some(alert) = self.check(task_id, key.0, &key, now_ns, cooldown_ns_len) {
                alerts.push(alert);
            }
        }
        alerts
    }

    /// Drops the least recently touched window when the budget is exhausted.
    fn make_room(&mut self, now_ns: i128) {
        if self.windows.len() < self.budget.max_groups {
            return;
        }
        let victim = self
            .windows
            .iter()
            .min_by_key(|(_, window)| window.last_touch_ns)
            .map(|(key, _)| key.clone());
        if let Some(victim) = victim {
            self.windows.remove(&victim);
            self.evictions = self.evictions.saturating_add(1);
            tracing::warn!(
                rule = %self.rules[victim.0].file.id,
                group = %victim.1,
                now_ns,
                "rule window evicted by WindowBudget"
            );
        }
    }

    fn check(
        &mut self,
        task_id: &str,
        rule_index: usize,
        key: &(usize, String),
        now_ns: i128,
        cooldown_ns: i128,
    ) -> Option<AlertEvent> {
        let rule = self.rules.get(rule_index)?;
        let window = self.windows.get(key)?;
        if window.is_empty() {
            return None;
        }
        let (value, numerator, denominator) = metric_value(&rule.file, window);
        let threshold = rule.file.threshold.value;
        if !rule.file.threshold.operator.apply(value, threshold) {
            return None;
        }
        if now_ns < window.cooldown_until {
            return None;
        }
        let first = window.events.front()?;
        let last = window.events.back()?;
        let sample_packets: Vec<u64> = if window.degraded {
            Vec::new()
        } else {
            sample_packet_indices(window)
        };
        let evidence = AlertEvidence {
            metric: metric_name(rule.file.threshold.metric).to_owned(),
            value,
            window_ns: rule.window_ns.to_string(),
            operator: rule.file.threshold.operator.as_str().to_owned(),
            threshold,
            sample_packets,
            degraded: window.degraded,
            numerator,
            denominator,
            session_state: window.session_state,
        };
        let alert = AlertEvent {
            schema_version: SCHEMA_VERSION,
            alert_id: new_alert_id(),
            task_id: task_id.to_owned(),
            rule_id: rule.file.id.clone(),
            rule_version: rule.file.version,
            rule_content_hash: rule.short_hash(),
            severity: severity_name(rule.file.severity).to_owned(),
            first_packet: first.packet_index.min(last.packet_index),
            last_packet: first.packet_index.max(last.packet_index),
            first_ts_ns: first.ts_ns.to_string(),
            last_ts_ns: last.ts_ns.to_string(),
            group_key: window.group.clone(),
            rule_name: rule.file.name.clone(),
            evidence,
            session_id: window.session_id.clone(),
        };
        if let Some(window) = self.windows.get_mut(key) {
            window.cooldown_until = now_ns.saturating_add(cooldown_ns);
        }
        Some(alert)
    }
}

fn new_alert_id() -> String {
    packetsage_protocol::alert_id_from_ulid(&ulid::Ulid::new().to_string())
}

/// One event under evaluation.
enum Subject<'a> {
    Packet(&'a PacketEvent),
    Decode(&'a DecodeErrorEvent),
    Session(&'a SessionSummaryEvent),
}

impl Subject<'_> {
    /// Packet index and event timestamp.
    fn position(&self) -> (u64, i128) {
        match self {
            Subject::Packet(packet) => (
                packet.packet_index,
                packet.ts_unix_ns.parse::<i128>().unwrap_or(0),
            ),
            Subject::Decode(error) => (error.packet_index, 0),
            Subject::Session(summary) => (0, summary.first_ts_ns.parse::<i128>().unwrap_or(0)),
        }
    }
}

fn metric_name(metric: Metric) -> &'static str {
    match metric {
        Metric::Count => "count",
        Metric::DistinctCount => "distinct_count",
        Metric::SumBytes => "sum_bytes",
        Metric::Ratio => "ratio",
    }
}

fn severity_name(severity: Severity) -> &'static str {
    severity.as_str()
}

fn render_group(group: &[(String, String)]) -> String {
    group
        .iter()
        .map(|(k, v)| format!("{k}={v}"))
        .collect::<Vec<_>>()
        .join("|")
}

fn flag_set(packet: &PacketEvent) -> Vec<TcpFlag> {
    packet
        .transport
        .as_ref()
        .map(|t| t.flags.clone())
        .unwrap_or_default()
}

fn flag_name(name: &str) -> Option<TcpFlag> {
    match name {
        "FIN" => Some(TcpFlag::Fin),
        "SYN" => Some(TcpFlag::Syn),
        "RST" => Some(TcpFlag::Rst),
        "PSH" => Some(TcpFlag::Psh),
        "ACK" => Some(TcpFlag::Ack),
        "URG" => Some(TcpFlag::Urg),
        "ECE" => Some(TcpFlag::Ece),
        "CWR" => Some(TcpFlag::Cwr),
        _ => None,
    }
}

fn protocol_of(packet: &PacketEvent) -> Option<String> {
    packet.network.as_ref().map(|n| match n.protocol {
        packetsage_protocol::NetProto::Tcp => "tcp".to_owned(),
        packetsage_protocol::NetProto::Udp => "udp".to_owned(),
        packetsage_protocol::NetProto::Icmp => "icmp".to_owned(),
        packetsage_protocol::NetProto::Icmpv6 => "icmpv6".to_owned(),
        packetsage_protocol::NetProto::Arp => "arp".to_owned(),
        packetsage_protocol::NetProto::Ipv4 => "ipv4".to_owned(),
        packetsage_protocol::NetProto::Ipv6 => "ipv6".to_owned(),
        packetsage_protocol::NetProto::Other => "other".to_owned(),
    })
}

fn dns_detail(packet: &PacketEvent) -> Option<&packetsage_protocol::DnsDetail> {
    match packet.application.as_ref().and_then(|a| a.detail.as_ref()) {
        Some(AppDetail::Dns(detail)) => Some(detail),
        _ => None,
    }
}

fn matches_packet(rule: &RuleFile, packet: &PacketEvent) -> bool {
    if !matches_match_spec(&rule.match_filter, packet) {
        return false;
    }
    // Rules that only describe decode errors never match clean packets.
    rule.match_filter.decode_layer.is_none() && rule.match_filter.decode_code.is_none()
}

fn matches_match_spec(spec: &MatchSpec, packet: &PacketEvent) -> bool {
    if let Some(protocol) = &spec.protocol {
        if protocol_of(packet).as_deref() != Some(protocol.as_str()) {
            return false;
        }
    }
    if let Some(flags) = &spec.flags {
        let present = flag_set(packet);
        for name in &flags.contains {
            let Some(flag) = flag_name(name) else {
                return false;
            };
            if !present.contains(&flag) {
                return false;
            }
        }
        for name in &flags.excludes {
            let Some(flag) = flag_name(name) else {
                return false;
            };
            if present.contains(&flag) {
                return false;
            }
        }
    }
    if let Some(condition) = &spec.src_port {
        if !condition.matches(packet.transport.as_ref().and_then(|t| t.src_port)) {
            return false;
        }
    }
    if let Some(condition) = &spec.dst_port {
        if !condition.matches(packet.transport.as_ref().and_then(|t| t.dst_port)) {
            return false;
        }
    }
    if let Some(condition) = &spec.any_port {
        let (src, dst) = packet
            .transport
            .as_ref()
            .map(|t| (t.src_port, t.dst_port))
            .unwrap_or((None, None));
        if !condition.matches(src) && !condition.matches(dst) {
            return false;
        }
    }
    if let Some(condition) = &spec.dns {
        let Some(detail) = dns_detail(packet) else {
            return false;
        };
        if let Some(limit) = condition.qname_len_gt {
            if detail.qname_len.unwrap_or(0) <= limit {
                return false;
            }
        }
        if let Some(limit) = condition.txt_len_gt {
            if detail.txt_len.unwrap_or(0) <= limit {
                return false;
            }
        }
        if let Some(limit) = condition.entropy_gt {
            if detail.entropy.unwrap_or(0.0) <= limit {
                return false;
            }
        }
    }
    true
}

fn matches_decode(rule: &RuleFile, error: &DecodeErrorEvent) -> bool {
    let spec = &rule.match_filter;
    if spec.decode_layer.is_none() && spec.decode_code.is_none() {
        return false;
    }
    if let Some(layer) = &spec.decode_layer {
        if layer != "any" && layer != layer_name(error) {
            return false;
        }
    }
    if let Some(code) = &spec.decode_code {
        if code != code_name(error) {
            return false;
        }
    }
    true
}

fn matches_session(rule: &RuleFile, summary: &SessionSummaryEvent) -> bool {
    if let Some(protocol) = &rule.match_filter.protocol {
        if protocol != &summary.protocol {
            return false;
        }
    }
    true
}

fn matches_subject(rule: &RuleFile, subject: &Subject<'_>) -> bool {
    match subject {
        Subject::Packet(packet) => matches_packet(rule, packet),
        Subject::Decode(error) => matches_decode(rule, error),
        Subject::Session(summary) => matches_session(rule, summary),
    }
}

fn layer_name(error: &DecodeErrorEvent) -> &'static str {
    match error.layer {
        packetsage_protocol::Layer::Link => "link",
        packetsage_protocol::Layer::Vlan => "vlan",
        packetsage_protocol::Layer::Arp => "arp",
        packetsage_protocol::Layer::Ipv4 => "ipv4",
        packetsage_protocol::Layer::Ipv6 => "ipv6",
        packetsage_protocol::Layer::Tcp => "tcp",
        packetsage_protocol::Layer::Udp => "udp",
        packetsage_protocol::Layer::Icmp => "icmp",
        packetsage_protocol::Layer::Icmpv6 => "icmpv6",
        packetsage_protocol::Layer::Dns => "dns",
        packetsage_protocol::Layer::Http => "http",
        packetsage_protocol::Layer::Tls => "tls",
        packetsage_protocol::Layer::Dhcp => "dhcp",
    }
}

fn code_name(error: &DecodeErrorEvent) -> &'static str {
    match error.code {
        packetsage_protocol::DecodeErrorCode::TruncatedHeader => "truncated_header",
        packetsage_protocol::DecodeErrorCode::BadLength => "bad_length",
        packetsage_protocol::DecodeErrorCode::InvalidField => "invalid_field",
        packetsage_protocol::DecodeErrorCode::UnsupportedLinktype => "unsupported_linktype",
        packetsage_protocol::DecodeErrorCode::UnsupportedProtocol => "unsupported_protocol",
    }
}

fn field_text(rule: &RuleFile, subject: &Subject<'_>) -> Option<String> {
    let field = rule.threshold.field.as_deref()?;
    match subject {
        Subject::Packet(packet) => match field {
            "dst_port" => packet
                .transport
                .as_ref()
                .and_then(|t| t.dst_port)
                .map(|p| p.to_string()),
            "src_port" => packet
                .transport
                .as_ref()
                .and_then(|t| t.src_port)
                .map(|p| p.to_string()),
            "src_ip" => packet.network.as_ref().map(|n| n.src.clone()),
            "dst_ip" => packet.network.as_ref().map(|n| n.dst.clone()),
            "tls.sni" => match packet.application.as_ref().and_then(|a| a.detail.as_ref()) {
                Some(AppDetail::Tls(detail)) => detail.sni.clone(),
                _ => None,
            },
            "http.host" => match packet.application.as_ref().and_then(|a| a.detail.as_ref()) {
                Some(AppDetail::Http(detail)) => detail.host.clone(),
                _ => None,
            },
            _ => None,
        },
        Subject::Decode(error) => match field {
            "decode_code" => Some(code_name(error).to_owned()),
            "decode_layer" => Some(layer_name(error).to_owned()),
            _ => None,
        },
        Subject::Session(summary) => match field {
            "src_ip" => Some(summary.src_ip.clone()),
            "dst_ip" => Some(summary.dst_ip.clone()),
            _ => None,
        },
    }
}

fn field_number(rule: &RuleFile, subject: &Subject<'_>) -> Option<f64> {
    let field = rule.threshold.field.as_deref()?;
    match subject {
        Subject::Packet(packet) => match field {
            "bytes" => Some(f64::from(packet.captured_len)),
            "dst_port" => packet
                .transport
                .as_ref()
                .and_then(|t| t.dst_port)
                .map(f64::from),
            "src_port" => packet
                .transport
                .as_ref()
                .and_then(|t| t.src_port)
                .map(f64::from),
            "dns.qname_len" => dns_detail(packet).and_then(|d| d.qname_len).map(f64::from),
            "dns.txt_len" => dns_detail(packet).and_then(|d| d.txt_len).map(f64::from),
            "dns.entropy" => dns_detail(packet).and_then(|d| d.entropy).map(f64::from),
            _ => None,
        },
        Subject::Decode(_) | Subject::Session(_) => None,
    }
}

fn sample_of(rule: &RuleFile, subject: &Subject<'_>) -> Option<Sample> {
    match rule.threshold.metric {
        Metric::Count => Some(Sample::Num(1.0)),
        Metric::SumBytes => match subject {
            Subject::Packet(packet) => Some(Sample::Num(f64::from(packet.captured_len))),
            _ => Some(Sample::Num(0.0)),
        },
        Metric::DistinctCount | Metric::Ratio => {
            if let Some(number) = field_number(rule, subject) {
                Some(Sample::Num(number))
            } else {
                field_text(rule, subject).map(Sample::Text)
            }
        }
    }
}

fn group_of(rule: &RuleFile, subject: &Subject<'_>) -> Vec<(String, String)> {
    let mut group = Vec::new();
    for field in &rule.threshold.group_by {
        let (name, value) = match (field, subject) {
            (GroupField::SrcIp, Subject::Packet(packet)) => (
                "src_ip",
                packet
                    .network
                    .as_ref()
                    .map(|n| n.src.clone())
                    .unwrap_or_default(),
            ),
            (GroupField::DstIp, Subject::Packet(packet)) => (
                "dst_ip",
                packet
                    .network
                    .as_ref()
                    .map(|n| n.dst.clone())
                    .unwrap_or_default(),
            ),
            (GroupField::SrcPort, Subject::Packet(packet)) => (
                "src_port",
                packet
                    .transport
                    .as_ref()
                    .and_then(|t| t.src_port)
                    .map(|p| p.to_string())
                    .unwrap_or_default(),
            ),
            (GroupField::DstPort, Subject::Packet(packet)) => (
                "dst_port",
                packet
                    .transport
                    .as_ref()
                    .and_then(|t| t.dst_port)
                    .map(|p| p.to_string())
                    .unwrap_or_default(),
            ),
            (GroupField::Protocol, Subject::Packet(packet)) => (
                "protocol",
                protocol_of(packet).unwrap_or_else(|| "unknown".to_owned()),
            ),
            (GroupField::Session, Subject::Packet(packet)) => {
                ("session", packet_key(packet).unwrap_or_default())
            }
            (GroupField::DecodeCode, Subject::Decode(error)) => {
                ("decode_code", code_name(error).to_owned())
            }
            (GroupField::SrcIp, Subject::Session(summary)) => ("src_ip", summary.src_ip.clone()),
            (GroupField::DstIp, Subject::Session(summary)) => ("dst_ip", summary.dst_ip.clone()),
            (GroupField::Session, Subject::Session(summary)) => {
                ("session", summary.session_id.clone())
            }
            (GroupField::Protocol, Subject::Session(summary)) => {
                ("protocol", summary.protocol.clone())
            }
            (GroupField::SrcPort, Subject::Session(summary)) => (
                "src_port",
                summary.src_port.map(|p| p.to_string()).unwrap_or_default(),
            ),
            (GroupField::DstPort, Subject::Session(summary)) => (
                "dst_port",
                summary.dst_port.map(|p| p.to_string()).unwrap_or_default(),
            ),
            (field, _) => (group_field_name(*field), String::new()),
        };
        group.push((name.to_owned(), value));
    }
    group
}

fn group_field_name(field: GroupField) -> &'static str {
    match field {
        GroupField::SrcIp => "src_ip",
        GroupField::DstIp => "dst_ip",
        GroupField::SrcPort => "src_port",
        GroupField::DstPort => "dst_port",
        GroupField::Protocol => "protocol",
        GroupField::Session => "session",
        GroupField::DecodeCode => "decode_code",
    }
}

fn packet_key(packet: &PacketEvent) -> Option<String> {
    let network = packet.network.as_ref()?;
    let transport = packet.transport.as_ref()?;
    Some(format!(
        "{}:{}-{}:{}",
        network.src,
        transport.src_port.unwrap_or(0),
        network.dst,
        transport.dst_port.unwrap_or(0)
    ))
}

fn metric_value(rule: &RuleFile, window: &GroupWindow) -> (f64, Option<f64>, Option<f64>) {
    match rule.threshold.metric {
        Metric::Count => (window.events.len() as f64, None, None),
        Metric::SumBytes => (window.bytes_sum, None, None),
        Metric::DistinctCount => (window.value_counts.len() as f64, None, None),
        Metric::Ratio => {
            let denominator = window.events.len() as f64;
            let numerator = window.numerator_hits as f64;
            let value = if denominator == 0.0 {
                0.0
            } else {
                numerator / denominator
            };
            (value, Some(numerator), Some(denominator))
        }
    }
}

/// Computes the ratio numerator condition for a sample.
/// Up to five evenly spaced packet indices inside the window, ascending.
///
/// Sampling instead of sorting keeps the alert path linear in the window size,
/// which matters when a single group holds thousands of events.
#[allow(clippy::integer_division)] // quartile positions: integer arithmetic on purpose
fn sample_packet_indices(window: &GroupWindow) -> Vec<u64> {
    let len = window.events.len();
    if len <= 5 {
        let mut packets: Vec<u64> = window.events.iter().map(|e| e.packet_index).collect();
        packets.sort_unstable();
        packets.dedup();
        return packets;
    }
    let mut packets = Vec::with_capacity(5);
    for position in [0usize, len / 4, len / 2, (len * 3) / 4, len - 1] {
        if let Some(event) = window.events.get(position) {
            packets.push(event.packet_index);
        }
    }
    packets.sort_unstable();
    packets.dedup();
    packets
}

/// Computes the ratio numerator condition for a sample.
#[must_use]
pub fn ratio_hit(rule: &RuleFile, sample: &Sample) -> bool {
    let Some(limit) = rule.threshold.field_gt else {
        return false;
    };
    match sample {
        Sample::Num(value) => *value > limit,
        Sample::Text(_) => false,
    }
}

/// Convenience wrapper used by tests: evaluate a sequence of events.
pub fn evaluate_stream(
    engine: &mut RuleEngine,
    events: &[EngineEvent],
    now_ns: i128,
) -> Vec<AlertEvent> {
    let mut alerts = Vec::new();
    for event in events {
        alerts.extend(engine.evaluate(event, now_ns));
    }
    alerts
}

/// Window length in nanoseconds for a rule text (used by `rules check`).
///
/// # Errors
/// Returns [`crate::error::RuleError`] when the window is outside the whitelist.
pub fn rule_window_ns(text: &str) -> Result<i128> {
    let rule = RuleFile::parse(text)?;
    window_ns(&rule.threshold.window)
}

/// Operator names, exported for reports.
#[must_use]
pub fn operator_name(operator: Operator) -> &'static str {
    operator.as_str()
}
