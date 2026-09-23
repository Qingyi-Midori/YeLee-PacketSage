//! The project's ASCII title (`packetsage --help` opens with it).
//!
//! The art is **hardcoded** here: `banner.txt` lives in this crate and
//! `include_str!` bakes it into the binary at compile time, so neither the
//! binary nor the release tarball has a data file to look for at runtime.
//! The unit tests freeze the art's shape (first line, line count, ASCII-only)
//! so an accidental edit fails here instead of in someone's console.

/// The title, ready to hand to clap's `before_help`.
pub const TITLE: &str = include_str!("banner.txt");

#[cfg(test)]
mod tests {
    use super::*;

    /// Hardcoded first line — the one a reader recognises.
    const FIRST_LINE: &str = "   :::   ::: :::::::::: :::        :::::::::: :::::::::: :::";

    /// Hardcoded line count of the art.
    const LINES: usize = 14;

    #[test]
    fn title_is_present_and_ascii() {
        assert!(
            TITLE.is_ascii(),
            "the title must survive every console code page"
        );
        assert!(!TITLE.ends_with(' '));
    }

    #[test]
    fn title_shape_is_frozen() {
        let lines: Vec<&str> = TITLE.lines().collect();
        assert_eq!(lines.len(), LINES, "the ASCII title changed shape");
        assert_eq!(lines[0], FIRST_LINE, "the ASCII title's first line changed");
    }
}
