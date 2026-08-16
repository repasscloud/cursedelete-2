//! Shared orchestration for CurseDelete: streaming enumeration/deletion
//! pipeline, adaptive worker pool, target safety validation, filters,
//! retention, and reporting.
//!
//! This crate defines the [`engine::PlatformEngine`] contract that
//! `cursdel-windows`, `cursdel-linux`, and `cursdel-macos` implement, and
//! owns everything that is *not* genuinely platform-specific: see
//! `docs/adr/0001-workspace-architecture.md` for the split.

pub mod adaptive;
pub mod directory_tracker;
pub mod duration;
pub mod engine;
pub mod entry;
pub mod error;
pub mod exit_code;
pub mod filter;
pub mod metrics;
pub mod options;
pub mod pipeline;
pub mod report;
pub mod size;
pub mod sync;
pub mod target;
pub mod walk;

pub use engine::{CancelToken, PlatformEngine};
pub use error::{DeleteFailure, FailureCategory, TargetError};
pub use exit_code::ExitCode;
pub use options::{Mode, OperationOptions, WorkerPolicy};
pub use report::Summary;
pub use target::{validate_target, CanonicalTarget};
