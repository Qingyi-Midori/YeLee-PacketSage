//! Styling policy for the human readable streams (§3.2).
//!
//! Three rules, in order:
//!
//! 1. `--jsonl` machine streams are never styled;
//! 2. a non-empty `$NO_COLOR` (no-color.org) disables styling globally;
//! 3. otherwise styling follows the terminal attached to the stream.

use std::io::IsTerminal;

/// Styling decisions for this process.
#[derive(Debug, Clone, Copy)]
pub struct Palette {
    enabled: bool,
}

impl Palette {
    /// Decides whether ANSI escapes may be written, for the given stream.
    #[must_use]
    pub fn new(no_color_flag: bool, machine_stream: bool, stream_is_tty: bool) -> Self {
        Self::with_env(
            no_color_flag,
            machine_stream,
            stream_is_tty,
            no_color_requested(),
        )
    }

    /// Pure variant of [`Palette::new`], so tests do not depend on the
    /// environment they happen to run in.
    #[must_use]
    pub fn with_env(
        no_color_flag: bool,
        machine_stream: bool,
        stream_is_tty: bool,
        no_color_env: bool,
    ) -> Self {
        Self {
            enabled: !no_color_flag && !machine_stream && !no_color_env && stream_is_tty,
        }
    }

    /// Styling for stdout of the current process.
    #[must_use]
    pub fn stdout(no_color_flag: bool, machine_stream: bool) -> Self {
        Self::new(
            no_color_flag,
            machine_stream,
            std::io::stdout().is_terminal(),
        )
    }

    /// Wraps `text` in an SGR sequence when styling is on.
    #[must_use]
    pub fn paint(self, sgr: &str, text: &str) -> String {
        if self.enabled {
            format!("\x1b[{sgr}m{text}\x1b[0m")
        } else {
            text.to_owned()
        }
    }

    /// Green `OK` marker.
    #[must_use]
    pub fn ok(self, text: &str) -> String {
        self.paint("32", text)
    }

    /// Red marker.
    #[must_use]
    pub fn bad(self, text: &str) -> String {
        self.paint("31", text)
    }

    /// Yellow marker.
    #[must_use]
    pub fn warn(self, text: &str) -> String {
        self.paint("33", text)
    }
}

/// True when `$NO_COLOR` is set to a non-empty value (no-color.org).
#[must_use]
pub fn no_color_requested() -> bool {
    std::env::var_os("NO_COLOR").is_some_and(|value| !value.is_empty())
}

/// True when stderr is a terminal (progress cadence, §3.3).
#[must_use]
pub fn stderr_is_tty() -> bool {
    std::io::stderr().is_terminal()
}

/// True when stdin is a terminal (confirmation prompts, §8.2).
#[must_use]
pub fn stdin_is_tty() -> bool {
    std::io::stdin().is_terminal()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn machine_stream_never_gets_escapes() {
        let palette = Palette::with_env(false, true, true, false);
        assert_eq!(palette.ok("OK"), "OK");
        assert!(!palette.ok("OK").contains('\x1b'));
    }

    #[test]
    fn no_color_flag_wins_over_a_tty() {
        let palette = Palette::with_env(true, false, true, false);
        assert_eq!(palette.bad("X"), "X");
    }

    #[test]
    fn a_pipe_is_not_styled() {
        let palette = Palette::with_env(false, false, false, false);
        assert_eq!(palette.warn("X"), "X");
    }

    #[test]
    fn a_tty_without_no_color_is_styled() {
        let palette = Palette::with_env(false, false, true, false);
        assert!(palette.ok("OK").contains("\x1b[32m"));
    }

    #[test]
    fn the_no_color_variable_wins_over_a_tty() {
        let palette = Palette::with_env(false, false, true, true);
        assert_eq!(palette.ok("OK"), "OK");
    }
}
