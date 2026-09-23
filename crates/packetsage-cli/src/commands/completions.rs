//! `packetsage completions <shell>` (§6).

use clap::CommandFactory;

use crate::cli::{Cli, CompletionsArgs};
use crate::exit::ExitCode;

/// Writes the completion script for one shell to stdout.
///
/// The output is a pure function of the command tree and the shell name, so
/// generating it twice yields identical bytes (asserted by the CLI tests).
pub fn run(args: &CompletionsArgs) -> ExitCode {
    let mut command = Cli::command();
    let name = command.get_name().to_owned();
    clap_complete::generate(args.shell, &mut command, name, &mut std::io::stdout());
    ExitCode::Success
}
