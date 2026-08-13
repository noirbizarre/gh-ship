// See `src/lib.rs` for rationale: rich miette diagnostics are
// intentionally larger than clippy's default threshold.
#![allow(clippy::result_large_err)]

use std::process::ExitCode;

use clap::Parser;
use miette::{MietteHandlerOpts, Result};

use gh_ship::cli::{Cli, Command};
use gh_ship::style::Theme;

mod commands;

/// Exit codes are part of the CLI contract: CI pipelines branch on them.
///
/// `nothing to release` is deliberately *not* a distinct code — it is a
/// success, and scripting it as a failure is the wrong default.
mod exit {
    /// Everything worked.
    pub const OK: u8 = 0;
    /// Something went wrong.
    pub const FAILURE: u8 = 1;
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let verbose = cli.verbose > 0 || std::env::var_os("RUST_BACKTRACE").is_some();
    install_miette_hook(verbose);

    match run(cli) {
        Ok(()) => ExitCode::from(exit::OK),
        Err(report) => {
            eprintln!("{report:?}");
            ExitCode::from(exit::FAILURE)
        }
    }
}

/// Install miette's fancy diagnostic handler.
///
/// When `verbose` is set the renderer prints the full cause chain;
/// otherwise it shows only the primary diagnostic. Colour is left to
/// miette's own auto-detection so `NO_COLOR` / `TERM=dumb` keep
/// producing plain ASCII for snapshots.
fn install_miette_hook(verbose: bool) {
    let _ = miette::set_hook(Box::new(move |_| {
        let mut opts = MietteHandlerOpts::new();
        opts = if verbose {
            opts.with_cause_chain()
        } else {
            opts.without_cause_chain()
        };
        Box::new(opts.build())
    }));
}

fn run(cli: Cli) -> Result<()> {
    let theme = Theme::auto();

    match &cli.command {
        Command::Init(args) => commands::init::run(&cli, args, theme),
        Command::Validate(args) => commands::validate::run(&cli, args, theme),
        Command::Preview(args) => commands::preview::run(&cli, args, theme),
        Command::Prepare(args) => commands::prepare::run(&cli, args, theme),
        Command::Status(args) => commands::status::run(&cli, args, theme),
        Command::Release(args) => commands::release::run(&cli, args, theme),
        Command::Sign(args) => commands::sign::run(&cli, args, theme),
    }
}
