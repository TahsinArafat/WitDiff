pub mod config;
pub mod framework;
pub mod git;
pub mod goanalysis;
pub mod goscript;
pub mod inline;
pub mod integrity;
pub mod javaanalysis;
pub mod javasummary;
pub mod model;
pub mod mutation;
pub mod mutation_runner;
pub mod pyanalysis;
pub mod pyscript;
pub mod runner;
pub mod rustanalysis;
pub mod selection;
pub mod testshape;
pub mod verify;

pub use config::Config;
pub use git::{GitRepo, WorktreeGuard};
pub use model::*;
pub use runner::CommandSpec;
pub use verify::{inspect_repository, verify_repository, VerifyOptions};

/// Convenience alias so downstream crates and tests do not need to depend on
/// `anyhow` directly just to name the error type of a fallible core operation.
pub type Result<T> = anyhow::Result<T>;
