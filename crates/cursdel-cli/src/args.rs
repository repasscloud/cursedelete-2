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
}

#[derive(Subcommand, Debug)]
pub enum Command {
    /// Manage CurseDelete's licence activation.
    License {
        #[command(subcommand)]
        action: LicenseAction,
    },
}

#[derive(Subcommand, Debug)]
pub enum LicenseAction {
    /// Show the current licence/activation status.
    Status,
    /// Activate a licence using a License ID and Activation Code from
    /// your purchase email.
    Activate {
        #[arg(long)]
        license_id: String,
        #[arg(long)]
        activation_code: String,
        /// Write an offline activation request file instead of contacting
        /// the licence server directly (for air-gapped machines).
        #[arg(long)]
        offline: bool,
        /// Where to write the offline activation request file. Defaults
        /// to `./offline-activation-request.json`.
        #[arg(long, value_name = "PATH")]
        output: Option<PathBuf>,
    },
    /// Import a signed licence file received from an offline activation
    /// (see `license activate --offline`).
    Import {
        #[arg(value_name = "PATH")]
        license_file: PathBuf,
    },
    /// Free this device's activation so the licence can be activated
    /// elsewhere.
    Deactivate,
    /// Renew the current online activation lease.
    Refresh,
    /// Enroll this machine under an existing licence using a Deployment
    /// Key, for unattended/automated provisioning (Intune, an RMM, a
    /// golden image, a container entrypoint). This is a separate
    /// mechanism from manual `activate`: a Deployment Key is an
    /// enrollment credential, not a licence, and is never itself
    /// persisted or displayed. Exactly one of --deployment-key,
    /// --deployment-key-env, --deployment-key-file, or
    /// --deployment-key-stdin is required.
    Enroll {
        /// The Deployment Key value directly. Convenient for interactive
        /// use and testing; avoid this on shared/logged automation hosts
        /// where process command lines may be visible to other users or
        /// recorded (prefer --deployment-key-env or --deployment-key-file
        /// there).
        #[arg(
            long,
            value_name = "KEY",
            conflicts_with_all = ["deployment_key_env", "deployment_key_file", "deployment_key_stdin"]
        )]
        deployment_key: Option<String>,
        /// Read the Deployment Key from this environment variable, so it
        /// never appears on the process command line.
        #[arg(
            long,
            value_name = "ENV_VAR",
            conflicts_with_all = ["deployment_key", "deployment_key_file", "deployment_key_stdin"]
        )]
        deployment_key_env: Option<String>,
        /// Read the Deployment Key from this file (its contents, trimmed
        /// of surrounding whitespace).
        #[arg(
            long,
            value_name = "PATH",
            conflicts_with_all = ["deployment_key", "deployment_key_env", "deployment_key_stdin"]
        )]
        deployment_key_file: Option<PathBuf>,
        /// Read the Deployment Key from stdin (one line, trimmed).
        #[arg(
            long,
            conflicts_with_all = ["deployment_key", "deployment_key_env", "deployment_key_file"]
        )]
        deployment_key_stdin: bool,
    },
    /// Force-release this machine's seat using a Deployment Key, without
    /// needing the local activation credentials that `license deactivate`
    /// requires. Recovers a machine stranded by a partial/failed `enroll`
    /// (the server issued a seat but this device never persisted, or has
    /// since lost, its local `activation_token` -- see issue #13) so a
    /// retried `enroll` doesn't hit a `409 Conflict` against a seat this
    /// device can no longer identify itself with. Contacts the licence
    /// server directly and is rate-limited more strictly than `enroll`;
    /// use it only when `license enroll` reports the seat is already
    /// active and `license deactivate` reports no local activation.
    /// Exactly one of --deployment-key, --deployment-key-env,
    /// --deployment-key-file, or --deployment-key-stdin is required.
    ForceDeactivate {
        #[arg(
            long,
            value_name = "KEY",
            conflicts_with_all = ["deployment_key_env", "deployment_key_file", "deployment_key_stdin"]
        )]
        deployment_key: Option<String>,
        #[arg(
            long,
            value_name = "ENV_VAR",
            conflicts_with_all = ["deployment_key", "deployment_key_file", "deployment_key_stdin"]
        )]
        deployment_key_env: Option<String>,
        #[arg(
            long,
            value_name = "PATH",
            conflicts_with_all = ["deployment_key", "deployment_key_env", "deployment_key_stdin"]
        )]
        deployment_key_file: Option<PathBuf>,
        #[arg(
            long,
            conflicts_with_all = ["deployment_key", "deployment_key_env", "deployment_key_file"]
        )]
        deployment_key_stdin: bool,
    },
}
