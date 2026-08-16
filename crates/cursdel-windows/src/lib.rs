//! Native Windows deletion engine. Compiles only on `cfg(windows)` targets;
//! see `docs/adr/0001-workspace-architecture.md` for why platform engines
//! are separate crates rather than `cfg`-gated modules inside one crate.

#![cfg(windows)]

// Implemented in a later step.
