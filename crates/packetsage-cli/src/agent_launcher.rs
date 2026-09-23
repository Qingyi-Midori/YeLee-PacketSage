//! `chat` launcher resolution (§2.3, replacing M3~M6v0.2 §4.1's spawn target).
//!
//! Four steps, in order:
//!
//! 1. `$PACKETSAGE_AGENT_BIN` — explicit override, and the hook the CLI tests
//!    use to inject a fake agent;
//! 2. `packetsage-agent` on `PATH`, probed with `--version` (3 s timeout);
//! 3. `./agent/.venv/bin/packetsage-agent` — the development layout;
//! 4. `python -m packetsage_agent` — fallback, with a WARN line.
//!
//! The launcher itself contains no agent logic: it only finds and spawns.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// Probe budget for step 2 (#37: cold start and AV scanning on Windows).
const PROBE_TIMEOUT: Duration = Duration::from_secs(3);

/// A resolved way to start the agent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Launcher {
    /// Program to spawn.
    pub program: PathBuf,
    /// Arguments that come before the sub-command.
    pub prefix: Vec<String>,
    /// Which step matched, for diagnostics.
    pub via: &'static str,
    /// WARN text to print before spawning, when any.
    pub warning: Option<String>,
}

impl Launcher {
    /// Builds the full argument vector for one sub-command.
    #[must_use]
    pub fn command_line(&self, args: &[String]) -> Vec<String> {
        let mut argv = self.prefix.clone();
        argv.extend_from_slice(args);
        argv
    }
}

/// Resolves the launcher, or explains all four failed paths.
///
/// # Errors
/// Returns the troubleshooting text for stderr when every step failed.
pub fn resolve(python: &str) -> Result<Launcher, String> {
    let mut tried: Vec<String> = Vec::new();

    if let Some(raw) = std::env::var_os("PACKETSAGE_AGENT_BIN") {
        let path = PathBuf::from(raw);
        if path.is_file() {
            return Ok(Launcher {
                program: path,
                prefix: Vec::new(),
                via: "$PACKETSAGE_AGENT_BIN",
                warning: None,
            });
        }
        tried.push(format!(
            "$PACKETSAGE_AGENT_BIN={} (not a file)",
            path.display()
        ));
    } else {
        tried.push("$PACKETSAGE_AGENT_BIN (not set)".to_owned());
    }

    if let Some(candidate) = find_on_path("packetsage-agent") {
        if probe(&candidate, &["--version"]) {
            return Ok(Launcher {
                program: candidate,
                prefix: Vec::new(),
                via: "PATH",
                warning: None,
            });
        }
        tried.push(format!(
            "{} (on PATH but `--version` did not answer within {}s)",
            candidate.display(),
            PROBE_TIMEOUT.as_secs()
        ));
    } else {
        tried.push("`packetsage-agent` on PATH (not found)".to_owned());
    }

    let venv = venv_candidate();
    if venv.is_file() {
        return Ok(Launcher {
            program: venv,
            prefix: Vec::new(),
            via: "agent/.venv",
            warning: None,
        });
    }
    tried.push(format!("{} (not found)", venv.display()));

    if let Some(python_launcher) = fallback_python(python) {
        // The fallback only counts when the module really imports. The tarball
        // path has a Python interpreter but no agent package, and that case must
        // end as a launcher failure (exit 3 with the four-path hint, §2.3/T3)
        // rather than as a confusing `No module named packetsage_agent`.
        if probe(&python_launcher, &["-m", "packetsage_agent", "--help"]) {
            return Ok(Launcher {
                program: python_launcher,
                prefix: vec!["-m".to_owned(), "packetsage_agent".to_owned()],
                via: "python -m",
                warning: Some(format!(
                    "packetsage: `packetsage-agent` was not found on PATH; falling back to `{python} -m packetsage_agent`. \
                     Install the console script with `pip install -e ./agent`."
                )),
            });
        }
        tried.push(format!(
            "{python} -m packetsage_agent (the module is not importable)"
        ));
    } else {
        tried.push(format!(
            "{python} -m packetsage_agent (python not runnable)"
        ));
    }

    Err(format!(
        "packetsage: cannot find the Python agent. Tried:\n  - {}\n\
         Install it with `pip install -e ./agent`, or point $PACKETSAGE_AGENT_BIN at the entry point.",
        tried.join("\n  - ")
    ))
}

/// `agent/.venv/bin/packetsage-agent`, or the Windows `Scripts` layout.
#[must_use]
pub fn venv_candidate() -> PathBuf {
    let bin = if cfg!(windows) { "Scripts" } else { "bin" };
    let name = if cfg!(windows) {
        "packetsage-agent.exe"
    } else {
        "packetsage-agent"
    };
    Path::new("agent").join(".venv").join(bin).join(name)
}

/// Looks up `name` (plus `.exe` on Windows) in `PATH` (#29, best-effort).
#[must_use]
pub fn find_on_path(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    let extensions: Vec<String> = if cfg!(windows) {
        std::env::var("PATHEXT")
            .unwrap_or_else(|_| ".COM;.EXE;.BAT;.CMD".to_owned())
            .split(';')
            .map(|ext| ext.trim().to_ascii_lowercase())
            .filter(|ext| !ext.is_empty())
            .collect()
    } else {
        vec![String::new()]
    };
    for directory in std::env::split_paths(&path) {
        if directory.as_os_str().is_empty() {
            continue;
        }
        let direct = directory.join(name);
        if direct.is_file() {
            return Some(direct);
        }
        for extension in &extensions {
            let candidate = directory.join(format!("{name}{extension}"));
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    None
}

/// Runs `<candidate> <args>` and requires a clean exit inside the budget.
fn probe(candidate: &Path, args: &[&str]) -> bool {
    let child = Command::new(candidate)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn();
    let mut child = match child {
        Ok(child) => child,
        Err(_) => return false,
    };
    let deadline = Instant::now() + PROBE_TIMEOUT;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return status.success(),
            Ok(None) if Instant::now() >= deadline => {
                let _ = child.kill();
                let _ = child.wait();
                return false;
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(50)),
            Err(_) => return false,
        }
    }
}

/// Checks that the fallback interpreter exists at all.
fn fallback_python(python: &str) -> Option<PathBuf> {
    if let Some(found) = find_on_path(python) {
        return Some(found);
    }
    let path = PathBuf::from(python);
    path.is_file().then_some(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn venv_candidate_follows_the_host_layout() {
        let path = venv_candidate();
        assert!(path.starts_with("agent"));
        if cfg!(windows) {
            assert!(path.to_string_lossy().contains("Scripts"));
        } else {
            assert!(path.to_string_lossy().contains("bin"));
        }
    }

    #[test]
    fn path_lookup_finds_a_known_binary() {
        // `cargo` is running this test, so it must be on PATH in CI and here.
        assert!(find_on_path("cargo").is_some() || find_on_path("sh").is_some());
    }

    #[test]
    fn command_line_prefixes_the_sub_command() {
        let launcher = Launcher {
            program: PathBuf::from("python"),
            prefix: vec!["-m".to_owned(), "packetsage_agent".to_owned()],
            via: "test",
            warning: None,
        };
        assert_eq!(
            launcher.command_line(&["chat".to_owned()]),
            vec!["-m", "packetsage_agent", "chat"]
        );
    }
}
