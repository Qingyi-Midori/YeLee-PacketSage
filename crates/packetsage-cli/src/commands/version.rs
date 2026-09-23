//! `packetsage version` (§5).

use crate::cli::VersionArgs;
use crate::exit::ExitCode;

/// Prints the version line, or the stable JSON form with `--json`.
pub fn run(args: &VersionArgs) -> ExitCode {
    if args.json {
        println!("{}", crate::version::json());
    } else {
        println!("{}", crate::version::line());
    }
    ExitCode::Success
}
