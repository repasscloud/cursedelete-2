//! Command-line argument definitions. See `docs/COMMAND_REFERENCE.md` for
//! the user-facing documentation of every flag.

use std::path::PathBuf;

use clap::{Parser, Subcommand};

#[derive(Parser, Debug)]
#[command(
    name = "cursdel",
    version,
    about = "CurseDelete: a native, high-performance deletion engine.",
    long_about = None
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Option<Command>,

    /// Path to delete (file or directory). Required unless a subcommand
    /// (e.g. `license`) is given instead.
    pub target: Option<PathBuf>,

    /// Attempt permission/attribute/ownership/ACL remediation on failure,
    /// where the executing security context has authority to do so.
    #[arg(long)]
    pub force: bool,

    /// Adaptive maximum-throughput deletion with attribute/ownership/ACL
    /// remediation and local lock termination. Does not imply
    /// --close-remote-locks.
    #[arg(long)]
    pub destroy: bool,

    /// 'auto' (default) or a positive integer worker count. A manual
    /// count disables adaptive tuning entirely.
    #[arg(long, value_name = "auto|N", default_value = "auto")]
    pub workers: String,

    /// Delete only files at least this old. Requires a unit: m/h/d/w
    /// (e.g. 2d, 90d, 12h). A bare number is rejected.
    #[arg(long, value_name = "DURATION")]
    pub age: Option<String>,

    /// Which timestamp --age compares against.
    #[arg(
        long,
        value_name = "modified|created|accessed",
        default_value = "modified"
    )]
    pub age_by: String,

    /// Only delete files whose name matches this glob pattern.
    #[arg(long, value_name = "GLOB")]
    pub include: Option<String>,

    /// Never delete files whose name matches this glob pattern (wins over
    /// --include).
    #[arg(long, value_name = "GLOB")]
    pub exclude: Option<String>,

    /// Only delete files at least this size (e.g. 100m, 1g).
    #[arg(long, value_name = "SIZE")]
    pub min_size: Option<String>,

    /// Only delete files at most this size.
    #[arg(long, value_name = "SIZE")]
    pub max_size: Option<String>,

    /// Identify and terminate local processes holding a blocking file
    /// handle, then retry. Never touches CurseDelete itself or critical
    /// system processes.
    #[arg(long)]
    pub kill_locks: bool,

    /// Windows only: administratively close a matching remote SMB open on
    /// a supported Windows file server, then retry. Requires suitable
    /// administrative rights on that server and a Business/Enterprise
    /// licence. Distinct from --kill-locks and never implied by --destroy.
    #[arg(long)]
    pub close_remote_locks: bool,

    /// Plan the operation without deleting or modifying anything.
    #[arg(long)]
    pub dry_run: bool,

    /// Emit a machine-readable JSON report instead of text.
    #[arg(long)]
    pub json: bool,

    /// Suppress the normal summary; only failures/errors are printed.
    #[arg(long)]
    pub quiet: bool,

    /// Increase diagnostic detail. Verbose per-file logging materially
    /// reduces throughput on large trees and should be used for
    /// diagnostics, not routine operation.
    #[arg(long)]
    pub verbose: bool,

    /// Also write the report to this file (in addition to stdout/stderr).
    #[arg(long, value_name = "PATH")]
    pub log: Option<PathBuf>,

    /// Use this licence file in addition to the installed ones (same as
    /// setting CURSDEL_LICENSE_FILE). Never modified.
    #[arg(long, global = true, value_name = "PATH")]
    pub license_file: Option<PathBuf>,
}

#[derive(Subcommand, Debug)]
pub enum Command {
    /// Manage CurseDelete's licence.
    License {
        #[command(subcommand)]
        action: LicenseAction,
    },
}

#[derive(Subcommand, Debug)]
pub enum LicenseAction {
    /// Show which edition is in effect, and why.
    Status {
        /// Emit machine-readable JSON instead of text.
        #[arg(long)]
        json: bool,
    },
    /// Install a signed licence file (offline; no server contact).
    #[command(alias = "import")]
    Install {
        #[arg(value_name = "PATH")]
        path: PathBuf,
        #[command(flatten)]
        scope: ScopeArg,
    },
    /// Activate online with an activation key. Business licences are
    /// registered to this device; other editions are simply downloaded.
    Activate {
        #[arg(value_name = "KEY")]
        activation_key: String,
        #[command(flatten)]
        scope: ScopeArg,
    },
    /// Release this device's Business activation seat on the licensing
    /// server.
    Deactivate {
        #[command(flatten)]
        scope: ScopeArg,
    },
    /// Download the current signed licence (subscription renewal,
    /// refreshed activation certificate, revocation status).
    #[command(alias = "refresh")]
    Renew {
        #[command(flatten)]
        scope: ScopeArg,
    },
}

#[derive(clap::Args, Debug, Clone, Copy)]
pub struct ScopeArg {
    /// Act on the machine-wide licence store (for all users; requires
    /// administrator/root) instead of the current user's.
    #[arg(long)]
    pub system: bool,
}
