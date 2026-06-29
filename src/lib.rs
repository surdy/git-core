//! # git-core
//!
//! A host-agnostic **git history, diff, and status engine**. It shells out to
//! the system `git` binary (no libgit2) and produces presentation-agnostic,
//! `serde`-serializable data (commit log, status snapshot, structured diffs).
//!
//! ## Transport seam
//!
//! Command execution is abstracted behind the [`GitTransport`] trait. The crate
//! ships a default [`LocalGit`] transport that spawns the local `git` binary
//! with a hardened environment, bounded output, and per-call timeouts. Hosts
//! with special execution needs (for example, running git inside a WSL distro)
//! provide their own `GitTransport` impl; the [`operations`] layer is unaware of
//! the difference.
//!
//! ## No authorization policy
//!
//! Path authorization and sandboxing are the host's responsibility. The host
//! resolves and authorizes a repository root, then calls the operations with a
//! **trusted** working directory (`&Path`). Generic per-repo path containment
//! ([`is_safe_pathspec`], [`resolve_within_repo`]) lives here and is always
//! applied — it keeps pathspecs inside the repo, independent of host policy.
//!
//! ## Module layout
//!
//! - [`types`] — serde data types returned to hosts.
//! - [`errors`] — [`GitError`] taxonomy and classification.
//! - [`parser`] — pure porcelain-v2 parsing.
//! - [`path_safety`] — per-repo path containment helpers.
//! - [`transport`] — the [`GitTransport`] seam, [`LocalGit`], and the spawn /
//!   drain / timeout / availability machinery (the heart of execution).
//! - [`operations`] — the public git operations built on the transport.

pub mod errors;
pub mod operations;
pub mod parser;
pub mod path_safety;
pub mod transport;
pub mod types;

pub use errors::{GitError, Result};
pub use parser::{parse_porcelain_v2, PorcelainV2};
pub use path_safety::{is_safe_pathspec, resolve_within_repo};
pub use transport::{ensure_git_available, GitTransport, LocalGit};
pub use types::{
    DiscardEntry, GitChangedFile, GitCommitFileChange, GitCommitResult, GitDiffContentResult,
    GitDiffResult, GitLogEntry, GitOutput, GitPanelSnapshot, GitPushResult, GitRepoInfo,
    GitStatusSnapshot, MIN_GIT_VERSION,
};

pub use operations::{
    commit, commit_file_diff, commit_files, diff, diff_content, discard, fetch, log,
    panel_snapshot, pull_ff_only, push, remote_url, resolve_repo, show_commit_diff, stage, status,
    unstage,
};
