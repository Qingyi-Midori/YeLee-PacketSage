//! Human readable fatal errors (CLI 收口工程规格书 §9).
//!
//! Presentation only: the error types and the exit code mapping are frozen
//! (M2v0.2 §5, 开发文档 §25) and are not touched here. Every template carries
//! an identifier, the reason, and a suggested action.

use std::path::{Path, PathBuf};

use packetsage_core::PacketSageError;

/// Extra identifiers a command knows but the error value does not carry.
#[derive(Debug, Default, Clone)]
pub struct Context {
    /// Capture file being analysed.
    pub path: Option<PathBuf>,
    /// Database URL.
    pub url: Option<String>,
    /// LLM provider.
    pub provider: Option<String>,
    /// LLM model.
    pub model: Option<String>,
    /// Task identifier.
    pub task_id: Option<String>,
}

impl Context {
    /// Sets the capture path.
    #[must_use]
    pub fn with_path(mut self, path: &Path) -> Self {
        self.path = Some(path.to_path_buf());
        self
    }

    /// Sets the database URL.
    #[must_use]
    pub fn with_url(mut self, url: &str) -> Self {
        self.url = Some(url.to_owned());
        self
    }

    /// Sets the provider and model.
    #[must_use]
    pub fn with_llm(mut self, provider: Option<&str>, model: Option<&str>) -> Self {
        self.provider = provider.map(str::to_owned);
        self.model = model.map(str::to_owned);
        self
    }

    /// Sets the task identifier the failing command was working on.
    #[must_use]
    pub fn with_task(mut self, task_id: &str) -> Self {
        self.task_id = Some(task_id.to_owned());
        self
    }
}

/// Renders a fatal error for stderr.
///
/// `verbose` (`-v`) appends the Debug representation, i.e. the full chain.
#[must_use]
pub fn render(error: &PacketSageError, context: &Context, verbose: bool) -> String {
    let mut line = match error {
        PacketSageError::UnsupportedCapture { path, reason } => format!(
            "{}: the first 16 bytes ({}) match no known magic — is this really a pcap/pcapng? ({reason})",
            path.display(),
            header_hex(path)
        ),
        PacketSageError::CaptureCorrupted { offset, reason } => {
            let path = context
                .path
                .as_deref()
                .map_or_else(|| "<capture>".to_owned(), |p| p.display().to_string());
            format!(
                "{path}: structure corrupted at offset={offset} ({reason}); \
                 locate the first bad packet with `packetsage analyze --jsonl`"
            )
        }
        PacketSageError::RuleError(message) => format!(
            "rule error: {message}; the static checks S1-S9 must pass — \
             run `packetsage rules check <path>`"
        ),
        PacketSageError::DatabaseError(message) => {
            let url = context
                .url
                .as_deref()
                .map_or_else(|| "<database>".to_owned(), str::to_owned);
            format!("{url}: {message}; if a migration is missing run `packetsage db migrate`")
        }
        PacketSageError::LlmError(message) => format!(
            "provider={} model={}: {message}; the key is read from \
             $PACKETSAGE_LLM_API_KEY (or $OPENAI_API_KEY) — `packetsage doctor` shows what is set",
            context.provider.as_deref().unwrap_or("mock"),
            context.model.as_deref().unwrap_or("-")
        ),
        PacketSageError::Io(message) => match &context.path {
            Some(path) => format!("{}: {message}", path.display()),
            None => format!("io error: {message}"),
        },
        PacketSageError::InvalidArgument(message) => {
            format!("invalid argument: {message} (see `packetsage --help`)")
        }
        PacketSageError::ToolError(message) => {
            format!("tool error: {message} (the agent degrades, the task still finishes)")
        }
        PacketSageError::DecodeError {
            packet_index,
            layer,
            code,
        } => format!("decode error at packet #{packet_index} ({layer:?}/{code:?})"),
        PacketSageError::Internal(message) => {
            format!("internal error: {message}; re-run with -vv and attach the log to an issue")
        }
    };
    line = match (&context.task_id, line.starts_with("packetsage:")) {
        (_, true) => line,
        (Some(task_id), false) => format!("packetsage: task {task_id}: {line}"),
        (None, false) => format!("packetsage: {line}"),
    };
    if verbose {
        line.push_str(&format!("\n  caused by: {error:?}"));
    }
    line
}

/// RPC-level `CAPTURE_UNAVAILABLE` (ADR-019): cold recovery needs the original file.
#[must_use]
pub fn capture_unavailable(task_id: &str, path: &Path) -> String {
    format!(
        "packetsage: task {task_id} source file {} is no longer readable \
         (cold recovery requires the file at its original path); re-run `packetsage analyze` on it",
        path.display()
    )
}

/// Message used by the panic hook and by the `catch_unwind` guard (§4, §9).
#[must_use]
pub fn panic_message(payload: &(dyn std::any::Any + Send)) -> String {
    let detail = payload
        .downcast_ref::<&str>()
        .map(|s| (*s).to_owned())
        .or_else(|| payload.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "unknown panic".to_owned());
    format!("packetsage: internal error (panic): {detail}; re-run with -vv and attach the log")
}

/// First 16 bytes of a file as lowercase hex, `unreadable` when it cannot be read.
fn header_hex(path: &Path) -> String {
    use std::io::Read;

    let mut file = match std::fs::File::open(path) {
        Ok(file) => file,
        Err(_) => return "unreadable".to_owned(),
    };
    let mut buffer = [0u8; 16];
    let Ok(read) = file.read(&mut buffer) else {
        return "unreadable".to_owned();
    };
    if read == 0 {
        return "empty file".to_owned();
    }
    let mut hex = String::with_capacity(read * 3);
    for (index, byte) in buffer[..read].iter().enumerate() {
        if index > 0 {
            hex.push(' ');
        }
        hex.push_str(&format!("{byte:02x}"));
    }
    hex
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unsupported_capture_shows_the_magic_and_an_action() {
        let error = PacketSageError::UnsupportedCapture {
            path: PathBuf::from("samples/nope.pcap"),
            reason: "unknown magic".to_owned(),
        };
        let text = render(&error, &Context::default(), false);
        assert!(text.contains("samples/nope.pcap"));
        assert!(text.contains("no known magic"));
        assert!(text.starts_with("packetsage: "));
    }

    #[test]
    fn database_error_suggests_migrate() {
        let error = PacketSageError::DatabaseError("no such table".to_owned());
        let text = render(
            &error,
            &Context::default().with_url("sqlite://packetsage.db"),
            false,
        );
        assert!(text.contains("sqlite://packetsage.db"));
        assert!(text.contains("db migrate"));
    }

    #[test]
    fn capture_corruption_points_at_the_offset_and_the_jsonl_trick() {
        let error = PacketSageError::CaptureCorrupted {
            offset: 4096,
            reason: "block length 0xffffffff exceeds the remaining file".to_owned(),
        };
        let text = render(
            &error,
            &Context::default().with_path(Path::new("samples/cut.pcapng")),
            false,
        );
        assert!(text.contains("samples/cut.pcapng"));
        assert!(text.contains("offset=4096"));
        assert!(text.contains("--jsonl"));
    }

    #[test]
    fn rule_errors_name_the_static_check_family() {
        let error = PacketSageError::RuleError("S3: window is not in the whitelist".to_owned());
        let text = render(&error, &Context::default(), false);
        assert!(text.contains("S1-S9"));
        assert!(text.contains("rules check"));
    }

    #[test]
    fn llm_error_names_the_variable_and_never_a_key() {
        let error = PacketSageError::LlmError("401".to_owned());
        let text = render(
            &error,
            &Context::default().with_llm(Some("openai"), Some("gpt-4o-mini")),
            false,
        );
        assert!(text.contains("PACKETSAGE_LLM_API_KEY"));
        assert!(text.contains("gpt-4o-mini"));
    }

    #[test]
    fn verbose_appends_the_chain() {
        let error = PacketSageError::Internal("boom".to_owned());
        let text = render(&error, &Context::default(), true);
        assert!(text.contains("caused by:"));
    }

    #[test]
    fn capture_unavailable_mentions_the_task_and_path() {
        let text = capture_unavailable("01J", Path::new("/data/a.pcap"));
        assert!(text.contains("01J"));
        assert!(text.contains("/data/a.pcap"));
    }
}
