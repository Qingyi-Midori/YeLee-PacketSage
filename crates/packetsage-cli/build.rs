//! Build metadata injection (CLI 收口工程规格书 §5 / §13).
//!
//! The crate deliberately ships no third party build dependency: `git`, the
//! rustc version and the clock are enough to fill the `version` fields.

use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

fn main() {
    // `PACKETSAGE_BUILD_TIME` must describe *this* build. Declaring only
    // `rerun-if-changed=build.rs` (the previous state) disabled Cargo's default
    // "rerun when the package changes" rule, so the stamp froze at the first
    // build in a target directory and every later binary lied about its age.
    // Watch the crate's inputs plus the workspace crates it links against;
    // adding a dependency crate means adding its `src` here.
    for path in [
        "build.rs",
        "Cargo.toml",
        "src",
        "../packetsage-core/src",
        "../packetsage-protocol/src",
        "../packetsage-rules/src",
        "../packetsage-storage/src",
    ] {
        println!("cargo:rerun-if-changed={path}");
    }
    println!("cargo:rerun-if-env-changed=SOURCE_DATE_EPOCH");

    emit("PACKETSAGE_GIT_HASH", &git_hash());
    emit("PACKETSAGE_BUILD_TIME", &build_time());
    emit(
        "PACKETSAGE_PROFILE",
        &std::env::var("PROFILE").unwrap_or_else(|_| "unknown".to_owned()),
    );
    emit(
        "PACKETSAGE_TARGET",
        &std::env::var("TARGET").unwrap_or_else(|_| "unknown".to_owned()),
    );
    emit("PACKETSAGE_RUSTC", &rustc_version());
}

fn emit(key: &str, value: &str) {
    println!("cargo:rustc-env={key}={value}");
}

/// `git rev-parse --short=8 HEAD`, plus `-dirty` when the tree has changes.
///
/// Anything that goes wrong (tarball build, no git installed) degrades to
/// `unknown` instead of failing the build (§5).
fn git_hash() -> String {
    let dir = std::env::var("CARGO_MANIFEST_DIR").unwrap_or_else(|_| ".".to_owned());
    let rev = run(&dir, &["rev-parse", "--short=8", "HEAD"]);
    let Some(rev) = rev else {
        return "unknown".to_owned();
    };
    match run(&dir, &["status", "--porcelain", "--untracked-files=no"]) {
        Some(status) if !status.is_empty() => format!("{rev}-dirty"),
        _ => rev,
    }
}

fn run(dir: &str, args: &[&str]) -> Option<String> {
    let output = Command::new("git")
        .args(args)
        .current_dir(dir)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    if text.is_empty() {
        None
    } else {
        Some(text)
    }
}

/// RFC3339 UTC build timestamp, honouring `SOURCE_DATE_EPOCH`.
fn build_time() -> String {
    let seconds = std::env::var("SOURCE_DATE_EPOCH")
        .ok()
        .and_then(|raw| raw.trim().parse::<u64>().ok())
        .unwrap_or_else(|| {
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0)
        });
    rfc3339(seconds)
}

/// `YYYY-MM-DDTHH:MM:SSZ` (Howard Hinnant's civil-from-days).
#[allow(clippy::integer_division)] // calendar arithmetic: every division is exact
fn rfc3339(seconds: u64) -> String {
    let days = (seconds / 86_400) as i64;
    let time = seconds % 86_400;
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = if m <= 2 { y + 1 } else { y };
    format!(
        "{year:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z",
        time / 3_600,
        (time % 3_600) / 60,
        time % 60
    )
}

fn rustc_version() -> String {
    let output = Command::new(std::env::var("RUSTC").unwrap_or_else(|_| "rustc".to_owned()))
        .arg("--version")
        .output();
    match output {
        Ok(output) if output.status.success() => String::from_utf8_lossy(&output.stdout)
            .split_whitespace()
            .nth(1)
            .unwrap_or("unknown")
            .to_owned(),
        _ => "unknown".to_owned(),
    }
}
