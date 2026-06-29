use std::path::{Path, PathBuf};

use crate::errors::{GitError, Result};

/// Resolve a repo-relative path, guaranteeing the result stays inside
/// `repo_root`. This is per-repo containment only — host-level authorization of
/// `repo_root` itself is the caller's responsibility.
pub fn resolve_within_repo(repo_root: &Path, rel: &str) -> Result<PathBuf> {
    if rel.is_empty() {
        return Err(GitError::InvalidPath(rel.into()));
    }
    if !is_safe_pathspec(rel) {
        return Err(GitError::InvalidPath(rel.into()));
    }
    let joined = repo_root.join(rel);
    let canonical = match std::fs::canonicalize(&joined) {
        Ok(p) => p,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return canonicalize_parent(repo_root, &joined, rel)
        }
        Err(e) => return Err(GitError::Io(e)),
    };
    if !canonical.starts_with(repo_root) {
        return Err(GitError::PathOutsideRepo(canonical));
    }
    Ok(canonical)
}

/// Reject pathspecs that could be misinterpreted by git (`:` revision syntax)
/// or smuggle control characters / NULs.
pub fn is_safe_pathspec(rel: &str) -> bool {
    !rel.is_empty()
        && !rel.contains(':')
        && !rel.contains('\0')
        && !rel.chars().any(|c| (c as u32) < 0x20)
}

fn canonicalize_parent(repo_root: &Path, joined: &Path, rel: &str) -> Result<PathBuf> {
    let parent = joined
        .parent()
        .ok_or_else(|| GitError::InvalidPath(rel.into()))?;
    let canonical_parent = std::fs::canonicalize(parent).map_err(GitError::Io)?;
    if !canonical_parent.starts_with(repo_root) {
        return Err(GitError::PathOutsideRepo(canonical_parent));
    }
    let file_name = joined
        .file_name()
        .ok_or_else(|| GitError::InvalidPath(rel.into()))?;
    Ok(canonical_parent.join(file_name))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn safe_pathspec_accepts_normal_paths() {
        assert!(is_safe_pathspec("src/main.rs"));
        assert!(is_safe_pathspec("a/b/c-d_e.txt"));
        assert!(is_safe_pathspec("folder with spaces/file.md"));
        assert!(is_safe_pathspec("file.with.dots"));
    }

    #[test]
    fn safe_pathspec_rejects_colon() {
        assert!(!is_safe_pathspec("evil:path"));
        assert!(!is_safe_pathspec(":head"));
        assert!(!is_safe_pathspec("a/b:c"));
    }

    #[test]
    fn safe_pathspec_rejects_nul_and_control() {
        assert!(!is_safe_pathspec("foo\0bar"));
        assert!(!is_safe_pathspec("foo\nbar"));
        assert!(!is_safe_pathspec("foo\rbar"));
        assert!(!is_safe_pathspec("foo\tbar"));
    }

    #[test]
    fn safe_pathspec_rejects_empty() {
        assert!(!is_safe_pathspec(""));
    }

    #[test]
    fn resolve_within_repo_rejects_colon_path() {
        let tmp = std::env::temp_dir();
        let err = resolve_within_repo(&tmp, "evil:path");
        assert!(matches!(err, Err(GitError::InvalidPath(_))));
    }

    #[test]
    fn resolve_within_repo_rejects_nul_path() {
        let tmp = std::env::temp_dir();
        let err = resolve_within_repo(&tmp, "evil\0path");
        assert!(matches!(err, Err(GitError::InvalidPath(_))));
    }
}
