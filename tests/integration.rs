use std::path::{Path, PathBuf};
use std::time::Duration;

use git_core::transport::GitTransport;
use git_core::{
    commit, diff, diff_content, ensure_git_available, log, resolve_repo, show_commit_diff, stage,
    status, LocalGit,
};

/// Skip (not fail) when git is missing or too old. On a properly provisioned
/// machine git >= 2.23 is present, so these tests actually run.
fn git_ready() -> bool {
    match ensure_git_available(&LocalGit) {
        Ok(()) => true,
        Err(e) => {
            eprintln!("skipping integration test: {e}");
            false
        }
    }
}

/// Run a setup git command (init/config) through the same transport under test.
fn setup_git(repo: &Path, args: &[&str]) {
    let out = LocalGit
        .run(repo, args, None, Duration::from_secs(30))
        .expect("git command spawns");
    assert!(out.success(), "git {args:?} failed: {}", out.stderr);
}

/// Create an initialized repo with a committer identity in a temp dir, returning
/// the canonicalized repo root (hosts are expected to pass trusted, resolved
/// paths; canonicalizing here mirrors that and keeps macOS /var symlinks happy).
fn init_repo() -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().expect("tempdir");
    let repo = std::fs::canonicalize(dir.path()).expect("canonicalize repo");
    setup_git(&repo, &["init"]);
    setup_git(&repo, &["config", "user.email", "test@example.com"]);
    setup_git(&repo, &["config", "user.name", "Test User"]);
    setup_git(&repo, &["config", "commit.gpgsign", "false"]);
    (dir, repo)
}

#[test]
fn end_to_end_status_log_show_and_diff() {
    if !git_ready() {
        return;
    }
    let (_guard, repo) = init_repo();

    // --- Untracked file shows up in status ---------------------------------
    std::fs::write(repo.join("a.txt"), "line1\nline2\n").unwrap();

    let snap = status(&LocalGit, &repo).expect("status");
    let entry = snap
        .changed_files
        .iter()
        .find(|f| f.path == "a.txt")
        .expect("a.txt in status");
    assert!(entry.untracked, "fresh file is untracked");
    assert!(entry.unstaged);

    // --- Stage + commit through git-core's own ops -------------------------
    stage(&LocalGit, &repo, &["a.txt".to_string()]).expect("stage");
    let first = commit(&LocalGit, &repo, "add a").expect("commit");
    assert_eq!(first.summary, "add a");
    assert!(!first.commit_sha.is_empty());

    let entries = log(&LocalGit, &repo, 30, None).expect("log");
    assert_eq!(entries.len(), 1, "one commit so far");
    assert_eq!(entries[0].subject, "add a");
    assert_eq!(entries[0].sha, first.commit_sha);
    assert!(entries[0].files_changed >= 1);
    assert!(entries[0].insertions >= 2);

    // --- Modify the file: status reflects an unstaged modification ---------
    std::fs::write(repo.join("a.txt"), "line1\nCHANGED\nline2\nline3\n").unwrap();

    let snap = status(&LocalGit, &repo).expect("status after edit");
    let entry = snap
        .changed_files
        .iter()
        .find(|f| f.path == "a.txt")
        .expect("a.txt modified");
    assert_eq!(entry.status_label, "Modified");
    assert!(entry.unstaged);
    assert!(!entry.untracked);

    // --- Per-file working-tree diff shows added/removed lines --------------
    let d = diff(&LocalGit, &repo, Some("a.txt"), false).expect("diff");
    assert!(d.diff_text.contains("+CHANGED"), "diff:\n{}", d.diff_text);
    assert!(d.diff_text.contains("+line3"), "diff:\n{}", d.diff_text);

    // --- Structured diff content: original vs modified ---------------------
    let content = diff_content(&LocalGit, &repo, "a.txt", false, None).expect("diff_content");
    assert!(!content.is_binary);
    assert!(content.original_content.contains("line2"));
    assert!(!content.original_content.contains("CHANGED"));
    assert!(content.modified_content.contains("CHANGED"));
    assert!(content.modified_content.contains("line3"));

    // --- Commit the change, then inspect via log + show_commit_diff --------
    stage(&LocalGit, &repo, &["a.txt".to_string()]).expect("stage 2");
    let second = commit(&LocalGit, &repo, "change a").expect("commit 2");

    let entries = log(&LocalGit, &repo, 30, None).expect("log 2");
    assert_eq!(entries.len(), 2, "two commits now");
    assert_eq!(entries[0].subject, "change a");
    assert_eq!(entries[1].subject, "add a");
    assert_eq!(entries[0].parents, vec![first.commit_sha.clone()]);

    let show = show_commit_diff(&LocalGit, &repo, &second.commit_sha).expect("show");
    assert!(
        show.diff_text.contains("CHANGED"),
        "show:\n{}",
        show.diff_text
    );
    assert!(show.diff_text.contains("a.txt"));
}

#[test]
fn resolve_repo_finds_root_from_subdir() {
    if !git_ready() {
        return;
    }
    let (_guard, repo) = init_repo();
    std::fs::write(repo.join("a.txt"), "x\n").unwrap();
    stage(&LocalGit, &repo, &["a.txt".to_string()]).expect("stage");
    commit(&LocalGit, &repo, "init").expect("commit");

    let sub = repo.join("nested");
    std::fs::create_dir_all(&sub).unwrap();

    let info = resolve_repo(&LocalGit, &sub)
        .expect("resolve_repo")
        .expect("inside a repo");
    let resolved_root = std::fs::canonicalize(&info.repo_root).expect("canonicalize root");
    assert_eq!(resolved_root, repo);
    assert!(!info.is_detached);
}

#[test]
fn commit_accepts_a_message_over_the_linux_argument_limit() {
    if !git_ready() {
        return;
    }
    let (_guard, repo) = init_repo();
    std::fs::write(repo.join("a.txt"), "x\n").unwrap();
    stage(&LocalGit, &repo, &["a.txt".to_string()]).expect("stage");
    let body = "line of a long commit body\n".repeat(8_000);
    let message = format!("big\n\n{body}");
    assert!(message.len() > 128 * 1024);

    let result = commit(&LocalGit, &repo, &message).expect("commit");
    assert_eq!(result.summary, "big");
}

#[test]
fn resolve_repo_outside_repo_returns_none() {
    if !git_ready() {
        return;
    }
    let dir = tempfile::tempdir().expect("tempdir");
    let plain = std::fs::canonicalize(dir.path()).expect("canonicalize");
    let info = resolve_repo(&LocalGit, &plain).expect("resolve_repo ok");
    assert!(info.is_none(), "a non-repo directory resolves to None");
}
