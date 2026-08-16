mod args;
mod engine_select;
mod license_cmd;
mod output;

use clap::error::ErrorKind;
use clap::Parser;

use cursdel_core::duration::AgeDuration;
use cursdel_core::engine::CancelToken;
use cursdel_core::exit_code::ExitCode;
use cursdel_core::filter::{AgeBasis, FilterSet, FilterSpec};
use cursdel_core::options::{Mode, OperationOptions, WorkerPolicy};
use cursdel_core::size::ByteSize;

use args::{Cli, Command};

fn main() {
    // `Cli::parse()` would call clap's own `Error::exit()` on a parse
    // failure, which prints the message correctly but always exits with
    // clap's hardcoded usage-error code (2) -- colliding with this
    // product's own frozen, documented meaning of exit code 2
    // ("completed with one or more deletion failures", see
    // `cursdel_core::exit_code::ExitCode`). A script checking
    // `$? -eq 2` to mean "some files failed to delete" must never be
    // misled by an unrelated CLI usage error such as an unknown flag or
    // a missing `license` subcommand, so parse errors are intercepted
    // here and mapped onto this crate's own `CliUsageError` (64) instead
    // -- except `--help`/`--version` output, which clap also routes
    // through the error path but which must still exit 0.
    let cli = match Cli::try_parse() {
        Ok(cli) => cli,
        Err(e) => {
            let _ = e.print();
            // Only an explicit `--help`/`--version` request is a success;
            // clap's `DisplayHelpOnMissingArgumentOrSubcommand` also shows
            // help text but for a genuinely missing required subcommand
            // (e.g. `cursdel license` with no action) -- treated as a
            // usage error here for consistency with this crate's own
            // "no target given" case, which is also `CliUsageError`.
            let code = if matches!(e.kind(), ErrorKind::DisplayHelp | ErrorKind::DisplayVersion) {
                ExitCode::Success
            } else {
                ExitCode::CliUsageError
            };
            std::process::exit(code.code());
        }
    };
    let exit_code = run(cli);
    std::process::exit(exit_code.code());
}

fn run(cli: Cli) -> ExitCode {
    match cli.command {
        Some(Command::License { action }) => license_cmd::run(action),
        None => run_delete(cli),
    }
}

fn run_delete(cli: Cli) -> ExitCode {
    let Some(target) = cli.target.clone() else {
        eprintln!("Error: a target path is required.\n\nUsage: cursdel <path> [options]\nRun 'cursdel --help' for details.");
        return ExitCode::CliUsageError;
    };

    let mode = if cli.destroy {
        Mode::Destroy
    } else if cli.force {
        Mode::Force
    } else {
        Mode::Normal
    };

    let workers = match cli.workers.parse::<WorkerPolicy>() {
        Ok(w) => w,
        Err(e) => {
            eprintln!("Error: {e}");
            return ExitCode::CliUsageError;
        }
    };

    let age = match cli.age.as_deref().map(str::parse::<AgeDuration>) {
        Some(Ok(d)) => Some(d.as_duration()),
        Some(Err(e)) => {
            eprintln!("Error: {e}");
            return ExitCode::CliUsageError;
        }
        None => None,
    };

    let age_basis = match cli.age_by.parse::<AgeBasis>() {
        Ok(b) => b,
        Err(e) => {
            eprintln!("Error: {e}");
            return ExitCode::CliUsageError;
        }
    };

    let min_size = match cli.min_size.as_deref().map(str::parse::<ByteSize>) {
        Some(Ok(v)) => Some(v),
        Some(Err(e)) => {
            eprintln!("Error: --min-size: {e}");
            return ExitCode::CliUsageError;
        }
        None => None,
    };

    let max_size = match cli.max_size.as_deref().map(str::parse::<ByteSize>) {
        Some(Ok(v)) => Some(v),
        Some(Err(e)) => {
            eprintln!("Error: --max-size: {e}");
            return ExitCode::CliUsageError;
        }
        None => None,
    };

    if cli.close_remote_locks {
        let capabilities = license_cmd::current_capabilities();
        if !capabilities.close_remote_locks {
            eprintln!(
                "Error: --close-remote-locks requires a Business or Enterprise licence.\n\
                 Run 'cursdel license status' for details, or 'cursdel license activate' to activate one."
            );
            return ExitCode::LicenseRequired;
        }
    }

    let filter_spec = FilterSpec {
        include: cli.include.clone(),
        exclude: cli.exclude.clone(),
        min_size,
        max_size,
        age,
        age_basis,
    };

    let filters = match FilterSet::build(&filter_spec) {
        Ok(f) => f,
        Err(e) => {
            eprintln!("Error: {e}");
            return ExitCode::CliUsageError;
        }
    };

    let options = OperationOptions {
        target,
        mode,
        workers,
        dry_run: cli.dry_run,
        kill_locks: cli.kill_locks,
        close_remote_locks: cli.close_remote_locks,
        filters: filter_spec,
        json: cli.json,
        quiet: cli.quiet,
        verbose: cli.verbose,
        log_path: cli.log.clone(),
    };

    let cancel = CancelToken::new();
    {
        let cancel_for_handler = cancel.clone();
        // Best-effort: if a handler can't be installed (e.g. already one
        // set in an unusual embedding scenario), CurseDelete still runs
        // to completion normally, it just won't respond to Ctrl+C early.
        let _ = ctrlc::set_handler(move || cancel_for_handler.cancel());
    }

    let engine = engine_select::make_engine();
    let summary = match cursdel_core::pipeline::run(engine.as_ref(), &options, &filters, cancel) {
        Ok(summary) => summary,
        Err(e) => {
            eprintln!("Error: {e}");
            return ExitCode::InvalidArgsOrTarget;
        }
    };

    let exit_code = summary.exit_code();
    output::render(
        &summary,
        options.json,
        options.quiet,
        options.log_path.as_deref(),
    );
    exit_code
}
