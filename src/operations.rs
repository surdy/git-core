use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::errors::{GitError, Result};
use crate::parser::parse_porcelain_v2;
use crate::path_safety::resolve_within_repo;
use crate::transport::{
    ensure_git_available, ensure_success, git_show_text, git_stdout_line_opt, git_stdout_lines,
    read_text_file, run_git, GitTransport,
};
use crate::types::{
    DiscardEntry, GitCommitFileChange, GitCommitResult, GitDiffContentResult, GitDiffResult,
    GitLogEntry, GitOutput, GitPanelSnapshot, GitPushResult, GitRepoInfo, GitStatusSnapshot,
    TextSource, DEFAULT_TIMEOUT_SECS, NETWORK_TIMEOUT_SECS,
};

fn run_args(
    transport: &dyn GitTransport,
    cwd: &Path,
    args: &[String],
    timeout_secs: u64,
) -> Result<GitOutput> {
    let refs: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
    run_git(transport, cwd, &refs, timeout_secs)
}

fn path_display(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

fn split_upstream(upstream: &str) -> (Option<String>, Option<String>) {
    match upstream.split_once('/') {
        Some((remote, branch)) => (Some(remote.to_string()), Some(branch.to_string())),
        None => (None, Some(upstream.to_string())),
    }
}

/// Resolve the repository root for a trusted directory. The host is responsible
/// for resolving/authorizing `cwd` before calling.
pub fn resolve_repo(transport: &dyn GitTransport, cwd: &Path) -> Result<Option<GitRepoInfo>> {
    ensure_git_available(transport)?;
    let Some(root_line) = git_stdout_line_opt(transport, cwd, &["rev-parse", "--show-toplevel"])?
    else {
        return Ok(None);
    };
    let root = PathBuf::from(&root_line);

    let head = match git_stdout_lines(transport, &root, &["rev-parse", "--abbrev-ref", "HEAD"])?
        .into_iter()
        .next()
    {
        Some(h) => h,
        None => git_stdout_line_opt(transport, &root, &["symbolic-ref", "--short", "HEAD"])?
            .ok_or(GitError::CommandFailed {
                context: "failed to resolve HEAD",
                detail: String::new(),
            })?,
    };

    let upstream = git_stdout_line_opt(
        transport,
        &root,
        &["rev-parse", "--abbrev-ref", "--symbolic-full-name", "@{u}"],
    )?;

    Ok(Some(GitRepoInfo {
        repo_root: root_line,
        branch: head.clone(),
        upstream,
        is_detached: head == "HEAD",
    }))
}

/// Combined repo + status snapshot for a trusted directory inside a repo.
pub fn panel_snapshot(transport: &dyn GitTransport, cwd: &Path) -> Result<GitPanelSnapshot> {
    ensure_git_available(transport)?;
    let Some(root_line) = git_stdout_line_opt(transport, cwd, &["rev-parse", "--show-toplevel"])?
    else {
        return Ok(GitPanelSnapshot {
            repo: None,
            status: None,
        });
    };
    let root = PathBuf::from(&root_line);

    let status = status_inner(transport, &root)?;
    let repo = GitRepoInfo {
        repo_root: status.repo_root.clone(),
        branch: status.branch.clone(),
        upstream: status.upstream.clone(),
        is_detached: status.is_detached,
    };
    Ok(GitPanelSnapshot {
        repo: Some(repo),
        status: Some(status),
    })
}

pub fn status(transport: &dyn GitTransport, repo_root: &Path) -> Result<GitStatusSnapshot> {
    ensure_git_available(transport)?;
    status_inner(transport, repo_root)
}

fn status_inner(transport: &dyn GitTransport, repo_root: &Path) -> Result<GitStatusSnapshot> {
    let output = run_git(
        transport,
        repo_root,
        &[
            "status",
            "--porcelain=v2",
            "--branch",
            "-z",
            "--untracked-files=all",
        ],
        DEFAULT_TIMEOUT_SECS,
    )?;
    ensure_success(&output, "git status failed")?;

    let parsed = parse_porcelain_v2(&output.stdout);

    Ok(GitStatusSnapshot {
        repo_root: path_display(repo_root),
        branch: parsed.branch,
        upstream: parsed.upstream,
        ahead: parsed.ahead,
        behind: parsed.behind,
        is_detached: parsed.is_detached,
        truncated: output.truncated,
        changed_files: parsed.files,
    })
}

pub fn diff(
    transport: &dyn GitTransport,
    repo_root: &Path,
    path: Option<&str>,
    staged: bool,
) -> Result<GitDiffResult> {
    ensure_git_available(transport)?;
    diff_inner(transport, repo_root, path, staged)
}

fn diff_inner(
    transport: &dyn GitTransport,
    repo_root: &Path,
    path: Option<&str>,
    staged: bool,
) -> Result<GitDiffResult> {
    let mut args: Vec<String> = vec!["diff".into(), "--no-ext-diff".into()];
    if staged {
        args.push("--cached".into());
    }
    let pathspec = match path.filter(|p| !p.is_empty()) {
        Some(p) => Some(pathspec_from_input(repo_root, p)?),
        None => None,
    };
    if let Some(spec) = pathspec.as_ref() {
        args.push("--".into());
        args.push(spec.clone());
    }
    let output = run_args(transport, repo_root, &args, DEFAULT_TIMEOUT_SECS)?;
    ensure_success(&output, "git diff failed")?;

    Ok(GitDiffResult {
        diff_text: output.stdout,
        truncated: output.truncated,
    })
}

pub fn diff_content(
    transport: &dyn GitTransport,
    repo_root: &Path,
    path: &str,
    staged: bool,
    original_path: Option<&str>,
) -> Result<GitDiffContentResult> {
    ensure_git_available(transport)?;
    let worktree_path = resolve_within_repo(repo_root, path)?;
    let rel_path = pathspec(repo_root, &worktree_path);

    let original_rel = match original_path {
        Some(orig) if !orig.is_empty() => {
            let resolved = resolve_within_repo(repo_root, orig)?;
            Some(pathspec(repo_root, &resolved))
        }
        _ => None,
    };

    let original = if staged {
        let spec = original_rel.as_deref().unwrap_or(&rel_path);
        git_show_text(transport, repo_root, &format!("HEAD:{spec}"))?
    } else {
        git_show_text(transport, repo_root, &format!(":{rel_path}"))?
    };
    let modified = if staged {
        git_show_text(transport, repo_root, &format!(":{rel_path}"))?
    } else {
        read_text_file(&worktree_path)?
    };
    let patch = diff_inner(transport, repo_root, Some(&rel_path), staged)?;
    let is_binary =
        matches!(original, TextSource::Binary) || matches!(modified, TextSource::Binary);

    Ok(GitDiffContentResult {
        original_content: original.into_text(),
        modified_content: modified.into_text(),
        is_binary,
        fallback_patch: patch.diff_text,
        truncated: patch.truncated,
    })
}

pub fn stage(transport: &dyn GitTransport, repo_root: &Path, paths: &[String]) -> Result<()> {
    ensure_git_available(transport)?;
    if paths.is_empty() {
        return Ok(());
    }
    let resolved = resolve_pathspecs(repo_root, paths)?;
    let mut args: Vec<String> = vec!["add".into(), "--".into()];
    args.extend(resolved);
    let output = run_args(transport, repo_root, &args, DEFAULT_TIMEOUT_SECS)?;
    ensure_success(&output, "git add failed")
}

pub fn unstage(transport: &dyn GitTransport, repo_root: &Path, paths: &[String]) -> Result<()> {
    ensure_git_available(transport)?;
    if paths.is_empty() {
        return Ok(());
    }
    let resolved = resolve_pathspecs(repo_root, paths)?;
    let mut reset_args: Vec<String> = vec!["reset".into(), "HEAD".into(), "--".into()];
    reset_args.extend(resolved.iter().cloned());
    let output = run_args(transport, repo_root, &reset_args, DEFAULT_TIMEOUT_SECS)?;
    if output.exit_code == Some(0) {
        return Ok(());
    }
    if !looks_like_no_head(&output) {
        return ensure_success(&output, "git reset failed");
    }
    let mut rm_args: Vec<String> = vec!["rm".into(), "--cached".into(), "-r".into(), "--".into()];
    rm_args.extend(resolved);
    let output = run_args(transport, repo_root, &rm_args, DEFAULT_TIMEOUT_SECS)?;
    ensure_success(&output, "git rm --cached failed")
}

fn looks_like_no_head(output: &GitOutput) -> bool {
    let stderr = output.stderr.to_ascii_lowercase();
    stderr.contains("ambiguous argument 'head'")
        || stderr.contains("unknown revision")
        || stderr.contains("does not have any commits yet")
        || stderr.contains("bad revision 'head'")
}

pub fn discard(
    transport: &dyn GitTransport,
    repo_root: &Path,
    entries: &[DiscardEntry],
) -> Result<()> {
    ensure_git_available(transport)?;
    if entries.is_empty() {
        return Ok(());
    }

    let mut tracked: Vec<String> = Vec::with_capacity(entries.len());
    let mut untracked: Vec<String> = Vec::new();
    for entry in entries {
        let resolved = pathspec_from_input(repo_root, &entry.path)?;
        if entry.untracked {
            untracked.push(resolved);
        } else {
            tracked.push(resolved);
        }
    }

    if !tracked.is_empty() {
        let mut args: Vec<String> = vec!["restore".into(), "--worktree".into(), "--".into()];
        args.extend(tracked);
        let output = run_args(transport, repo_root, &args, DEFAULT_TIMEOUT_SECS)?;
        ensure_success(&output, "git restore failed")?;
    }

    if !untracked.is_empty() {
        let mut args: Vec<String> = vec!["clean".into(), "-f".into(), "-d".into(), "--".into()];
        args.extend(untracked);
        let output = run_args(transport, repo_root, &args, DEFAULT_TIMEOUT_SECS)?;
        ensure_success(&output, "git clean failed")?;
    }

    Ok(())
}

pub fn commit(
    transport: &dyn GitTransport,
    repo_root: &Path,
    message: &str,
) -> Result<GitCommitResult> {
    ensure_git_available(transport)?;
    let trimmed = message.trim();
    if trimmed.is_empty() {
        return Err(GitError::EmptyCommitMessage);
    }

    // On stdin, not argv: Linux caps a single argument at 128 KB.
    let output = transport.run(
        repo_root,
        &["commit", "-F", "-"],
        Some(trimmed.as_bytes()),
        Duration::from_secs(DEFAULT_TIMEOUT_SECS),
    )?;
    if output.exit_code != Some(0) && nothing_to_commit(&output) {
        return Err(GitError::command("git commit", "nothing staged"));
    }
    ensure_success(&output, "git commit failed")?;

    let combined = git_stdout_lines(
        transport,
        repo_root,
        &["show", "-s", "--format=%H%n%s", "HEAD"],
    )?;
    let sha = combined.first().cloned().ok_or(GitError::CommandFailed {
        context: "failed to resolve commit sha",
        detail: String::new(),
    })?;
    let summary = combined.get(1).cloned().unwrap_or_default();

    Ok(GitCommitResult {
        commit_sha: sha,
        summary,
    })
}

pub fn push(transport: &dyn GitTransport, repo_root: &Path) -> Result<GitPushResult> {
    ensure_git_available(transport)?;

    let upstream = git_stdout_line_opt(
        transport,
        repo_root,
        &["rev-parse", "--abbrev-ref", "--symbolic-full-name", "@{u}"],
    )?;
    if upstream.is_none() {
        return Err(GitError::NoUpstream);
    }

    let output = run_git(transport, repo_root, &["push"], NETWORK_TIMEOUT_SECS)?;
    ensure_success(&output, "git push failed")?;

    let upstream = upstream.unwrap();
    let (remote, branch) = split_upstream(&upstream);
    Ok(GitPushResult {
        remote,
        branch,
        pushed: true,
    })
}

const LOG_FORMAT: &str = "%H%x1f%an%x1f%ae%x1f%at%x1f%P%x1f%s";
const MAX_LOG_LIMIT: u32 = 200;

pub fn log(
    transport: &dyn GitTransport,
    repo_root: &Path,
    limit: u32,
    before_sha: Option<&str>,
) -> Result<Vec<GitLogEntry>> {
    ensure_git_available(transport)?;
    let bounded = limit.clamp(1, MAX_LOG_LIMIT);
    let count_arg = format!("--max-count={bounded}");
    let format_arg = format!("--format={LOG_FORMAT}");
    let cursor = match before_sha {
        Some(sha) if !sha.is_empty() => {
            if !sha_is_safe(sha) {
                return Err(GitError::command("git log", "invalid cursor sha"));
            }
            Some(format!("{sha}^"))
        }
        _ => None,
    };
    let mut args: Vec<String> = vec![
        "log".into(),
        "--no-color".into(),
        "--shortstat".into(),
        count_arg,
        format_arg,
    ];
    if let Some(spec) = cursor {
        args.push(spec);
    }
    let output = run_args(transport, repo_root, &args, DEFAULT_TIMEOUT_SECS)?;
    if output.timed_out {
        return Err(GitError::TimedOut("git log"));
    }
    if output.exit_code != Some(0) {
        let stderr = output.stderr.to_ascii_lowercase();
        if stderr.contains("does not have any commits yet")
            || stderr.contains("bad default revision")
            || stderr.contains("unknown revision")
            || stderr.contains("ambiguous argument 'head'")
        {
            return Ok(Vec::new());
        }
        return ensure_success(&output, "git log failed").map(|_| Vec::new());
    }
    let mut entries: Vec<GitLogEntry> = Vec::with_capacity(bounded as usize);
    // Lines we get back interleave:
    //   <sha>\x1f<author>\x1f<email>\x1f<ts>\x1f<parents>\x1f<subject>
    //   <blank>
    //    5 files changed, 12 insertions(+), 3 deletions(-)
    // Commits without diffstats (root commits, merges with no changes) just
    // skip the shortstat line. Detect commit headers by the presence of
    // the unit-separator we put in the format.
    for raw_line in output.stdout.lines() {
        let line = raw_line.trim_end_matches('\r');
        if line.is_empty() {
            continue;
        }
        if line.contains('\x1f') {
            let mut fields = line.splitn(6, '\x1f');
            let sha = fields.next().unwrap_or("").to_string();
            if !sha_is_safe(&sha) {
                continue;
            }
            let author = fields.next().unwrap_or("").to_string();
            let author_email = fields.next().unwrap_or("").to_string();
            let timestamp = fields.next().unwrap_or("0").parse::<i64>().unwrap_or(0);
            let parents_raw = fields.next().unwrap_or("");
            let parents: Vec<String> = parents_raw
                .split_ascii_whitespace()
                .map(|s| s.to_string())
                .collect();
            let subject = fields.next().unwrap_or("").to_string();
            let short_sha = sha.chars().take(7).collect::<String>();
            entries.push(GitLogEntry {
                sha,
                short_sha,
                author,
                author_email,
                timestamp_secs: timestamp,
                parents,
                subject,
                files_changed: 0,
                insertions: 0,
                deletions: 0,
            });
            continue;
        }
        if let Some(current) = entries.last_mut() {
            if line.contains("file changed") || line.contains("files changed") {
                let (files, ins, del) = parse_shortstat(line);
                current.files_changed = files;
                current.insertions = ins;
                current.deletions = del;
            }
        }
    }
    Ok(entries)
}

pub fn show_commit_diff(
    transport: &dyn GitTransport,
    repo_root: &Path,
    sha: &str,
) -> Result<GitDiffResult> {
    ensure_git_available(transport)?;
    if !sha_is_safe(sha) {
        return Err(GitError::command("git show", "invalid commit identifier"));
    }
    let output = run_git(
        transport,
        repo_root,
        &[
            "show",
            "--no-color",
            "--no-ext-diff",
            "--patch-with-stat",
            sha,
            "--",
        ],
        DEFAULT_TIMEOUT_SECS,
    )?;
    ensure_success(&output, "git show failed")?;
    Ok(GitDiffResult {
        diff_text: output.stdout,
        truncated: output.truncated,
    })
}

fn parse_shortstat(tail: &str) -> (u32, u32, u32) {
    // Looks for a line like " 5 files changed, 12 insertions(+), 3 deletions(-)"
    for line in tail.lines() {
        let trimmed = line.trim();
        if !(trimmed.contains("file changed") || trimmed.contains("files changed")) {
            continue;
        }
        let mut files = 0u32;
        let mut ins = 0u32;
        let mut del = 0u32;
        for part in trimmed.split(',') {
            let part = part.trim();
            let num_str = part.split_ascii_whitespace().next().unwrap_or("0");
            let n: u32 = num_str.parse().unwrap_or(0);
            if part.contains("file") {
                files = n;
            } else if part.contains("insertion") {
                ins = n;
            } else if part.contains("deletion") {
                del = n;
            }
        }
        return (files, ins, del);
    }
    (0, 0, 0)
}

fn sha_is_safe(sha: &str) -> bool {
    !sha.is_empty() && sha.len() <= 64 && sha.chars().all(|c| c.is_ascii_hexdigit())
}

pub fn commit_files(
    transport: &dyn GitTransport,
    repo_root: &Path,
    sha: &str,
) -> Result<Vec<GitCommitFileChange>> {
    ensure_git_available(transport)?;
    if !sha_is_safe(sha) {
        return Err(GitError::command("git diff-tree", "invalid commit sha"));
    }

    let output = run_git(
        transport,
        repo_root,
        &[
            "diff-tree",
            "--no-commit-id",
            "-r",
            "-z",
            "--name-status",
            "--numstat",
            sha,
        ],
        DEFAULT_TIMEOUT_SECS,
    )?;
    ensure_success(&output, "git diff-tree failed")?;

    let bytes = output.stdout.as_bytes();
    let (name_status_bytes, numstat_bytes) = split_name_status_numstat(bytes);
    let mut files = parse_diff_tree_name_status(name_status_bytes);
    apply_numstat(&mut files, numstat_bytes);
    Ok(files)
}

fn split_name_status_numstat(bytes: &[u8]) -> (&[u8], &[u8]) {
    let s = std::str::from_utf8(bytes).unwrap_or("");
    let tokens: Vec<(usize, &str)> = s
        .split('\0')
        .scan(0usize, |off, t| {
            let start = *off;
            *off += t.len() + 1;
            Some((start, t))
        })
        .collect();
    let mut split_at = bytes.len();
    for tok in tokens.iter() {
        if tok.1.contains('\t') {
            split_at = tok.0;
            // Walk back: numstat for R/C with -z emits "<a>\t<r>" then two
            // NUL-separated paths. The two trailing path tokens belong to the
            // numstat block, not name-status.
            break;
        }
    }
    (&bytes[..split_at], &bytes[split_at..])
}

pub fn commit_file_diff(
    transport: &dyn GitTransport,
    repo_root: &Path,
    sha: &str,
    path: &str,
    original_path: Option<&str>,
) -> Result<GitDiffContentResult> {
    ensure_git_available(transport)?;
    if !sha_is_safe(sha) {
        return Err(GitError::command("git show", "invalid commit sha"));
    }
    let resolved = resolve_within_repo(repo_root, path)?;
    let rel = resolved
        .strip_prefix(repo_root)
        .map(|p| p.to_string_lossy().replace('\\', "/"))
        .unwrap_or_else(|_| path.replace('\\', "/"));

    let original_rel = match original_path {
        Some(orig) if !orig.is_empty() => {
            let resolved_orig = resolve_within_repo(repo_root, orig)?;
            resolved_orig
                .strip_prefix(repo_root)
                .map(|p| p.to_string_lossy().replace('\\', "/"))
                .unwrap_or_else(|_| orig.replace('\\', "/"))
        }
        _ => rel.clone(),
    };

    let parent = git_stdout_line_opt(transport, repo_root, &["rev-parse", &format!("{sha}^")])?;
    let original = match parent.as_deref() {
        Some(p) => git_show_text(transport, repo_root, &format!("{p}:{original_rel}"))?,
        None => TextSource::Missing,
    };
    let modified = git_show_text(transport, repo_root, &format!("{sha}:{rel}"))?;

    let mut diff_args: Vec<String> = vec![
        "show".into(),
        "--no-color".into(),
        "--no-ext-diff".into(),
        "--format=".into(),
        "-m".into(),
        "--first-parent".into(),
        sha.to_string(),
        "--".into(),
    ];
    diff_args.push(rel.clone());
    if original_rel != rel {
        diff_args.push(original_rel.clone());
    }
    let patch_output = run_args(transport, repo_root, &diff_args, DEFAULT_TIMEOUT_SECS)?;
    ensure_success(&patch_output, "git show <commit> -- <path> failed")?;

    let is_binary =
        matches!(original, TextSource::Binary) || matches!(modified, TextSource::Binary);

    Ok(GitDiffContentResult {
        original_content: original.into_text(),
        modified_content: modified.into_text(),
        is_binary,
        fallback_patch: patch_output.stdout,
        truncated: patch_output.truncated,
    })
}

pub fn remote_url(
    transport: &dyn GitTransport,
    repo_root: &Path,
    name: &str,
) -> Result<Option<String>> {
    ensure_git_available(transport)?;
    if name.is_empty() || name.len() > 64 || !name.chars().all(is_remote_name_char) {
        return Ok(None);
    }
    git_stdout_line_opt(
        transport,
        repo_root,
        &["config", "--get", &format!("remote.{name}.url")],
    )
}

fn is_remote_name_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.'
}

fn parse_diff_tree_name_status(bytes: &[u8]) -> Vec<GitCommitFileChange> {
    let s = std::str::from_utf8(bytes).unwrap_or("");
    let mut tokens = s.split('\0').filter(|t| !t.is_empty());
    let mut files: Vec<GitCommitFileChange> = Vec::new();
    while let Some(status_tok) = tokens.next() {
        let status_char = status_tok.chars().next().unwrap_or(' ');
        if status_char == 'R' || status_char == 'C' {
            let original = match tokens.next() {
                Some(v) => v.to_string(),
                None => break,
            };
            let new_path = match tokens.next() {
                Some(v) => v.to_string(),
                None => break,
            };
            files.push(GitCommitFileChange {
                path: new_path,
                original_path: Some(original),
                status: status_char.to_string(),
                status_label: status_label_for(status_char),
                added: 0,
                removed: 0,
                is_binary: false,
            });
        } else {
            let path = match tokens.next() {
                Some(v) => v.to_string(),
                None => break,
            };
            files.push(GitCommitFileChange {
                path,
                original_path: None,
                status: status_char.to_string(),
                status_label: status_label_for(status_char),
                added: 0,
                removed: 0,
                is_binary: false,
            });
        }
    }
    files
}

fn apply_numstat(files: &mut [GitCommitFileChange], bytes: &[u8]) {
    let s = std::str::from_utf8(bytes).unwrap_or("");
    let tokens: Vec<&str> = s.split('\0').filter(|t| !t.is_empty()).collect();
    let mut idx = 0;
    while idx < tokens.len() {
        let header = tokens[idx];
        idx += 1;
        let mut cols = header.splitn(3, '\t');
        let added_raw = cols.next().unwrap_or("0");
        let removed_raw = cols.next().unwrap_or("0");
        let inline_path = cols.next().unwrap_or("");
        let is_binary = added_raw == "-" && removed_raw == "-";
        let added: u32 = if is_binary {
            0
        } else {
            added_raw.parse().unwrap_or(0)
        };
        let removed: u32 = if is_binary {
            0
        } else {
            removed_raw.parse().unwrap_or(0)
        };

        let (path, original) = if inline_path.is_empty() {
            let original = tokens.get(idx).map(|s| s.to_string()).unwrap_or_default();
            idx += 1;
            let new_path = tokens.get(idx).map(|s| s.to_string()).unwrap_or_default();
            idx += 1;
            (new_path, Some(original))
        } else {
            (inline_path.to_string(), None)
        };

        if path.is_empty() {
            continue;
        }
        if let Some(file) = files.iter_mut().find(|f| f.path == path) {
            file.added = added;
            file.removed = removed;
            file.is_binary = is_binary;
            if file.original_path.is_none() {
                if let Some(orig) = original {
                    if !orig.is_empty() && orig != file.path {
                        file.original_path = Some(orig);
                    }
                }
            }
        }
    }
}

fn status_label_for(c: char) -> String {
    match c {
        'A' => "Added".into(),
        'M' => "Modified".into(),
        'D' => "Deleted".into(),
        'R' => "Renamed".into(),
        'C' => "Copied".into(),
        'T' => "Type changed".into(),
        'U' => "Unmerged".into(),
        _ => format!("Status {c}"),
    }
}

pub fn fetch(transport: &dyn GitTransport, repo_root: &Path) -> Result<()> {
    ensure_git_available(transport)?;
    let output = run_git(
        transport,
        repo_root,
        &["fetch", "--prune"],
        NETWORK_TIMEOUT_SECS,
    )?;
    ensure_success(&output, "git fetch failed")
}

pub fn pull_ff_only(transport: &dyn GitTransport, repo_root: &Path) -> Result<()> {
    ensure_git_available(transport)?;
    let output = run_git(
        transport,
        repo_root,
        &["pull", "--ff-only"],
        NETWORK_TIMEOUT_SECS,
    )?;
    ensure_success(&output, "git pull --ff-only failed")
}

fn nothing_to_commit(output: &GitOutput) -> bool {
    let stderr = output.stderr.to_ascii_lowercase();
    let stdout = output.stdout.to_ascii_lowercase();
    stderr.contains("nothing to commit") || stdout.contains("nothing to commit")
}

fn resolve_pathspecs(repo_root: &Path, paths: &[String]) -> Result<Vec<String>> {
    let mut out = Vec::with_capacity(paths.len());
    for p in paths {
        out.push(pathspec_from_input(repo_root, p)?);
    }
    Ok(out)
}

fn pathspec_from_input(repo_root: &Path, rel: &str) -> Result<String> {
    let resolved = resolve_within_repo(repo_root, rel)?;
    Ok(pathspec(repo_root, &resolved))
}

fn pathspec(repo_root: &Path, absolute: &Path) -> String {
    absolute
        .strip_prefix(repo_root)
        .map(|rel| rel.to_string_lossy().replace('\\', "/"))
        .unwrap_or_else(|_| absolute.to_string_lossy().replace('\\', "/"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sha_is_safe_accepts_hex() {
        assert!(sha_is_safe("abc123"));
        assert!(sha_is_safe(&"a".repeat(40)));
        assert!(sha_is_safe(&"f".repeat(64)));
    }

    #[test]
    fn sha_is_safe_rejects_non_hex_or_oversize() {
        assert!(!sha_is_safe(""));
        assert!(!sha_is_safe("abcg"));
        assert!(!sha_is_safe("abc 123"));
        assert!(!sha_is_safe(&"a".repeat(65)));
        assert!(!sha_is_safe(";rm -rf /"));
    }

    #[test]
    fn is_remote_name_char_allows_word_and_punct() {
        for c in "abcXYZ012-_.".chars() {
            assert!(is_remote_name_char(c));
        }
        for c in " /:\\?\"'".chars() {
            assert!(!is_remote_name_char(c));
        }
    }

    #[test]
    fn parse_shortstat_pulls_three_counts() {
        let line = " 5 files changed, 12 insertions(+), 3 deletions(-)";
        assert_eq!(parse_shortstat(line), (5, 12, 3));
    }

    #[test]
    fn parse_shortstat_handles_singular_file() {
        let line = " 1 file changed, 1 insertion(+)";
        assert_eq!(parse_shortstat(line), (1, 1, 0));
    }

    #[test]
    fn parse_shortstat_returns_zeros_when_absent() {
        assert_eq!(parse_shortstat("no stat here"), (0, 0, 0));
    }

    #[test]
    fn status_label_for_known_chars() {
        assert_eq!(status_label_for('A'), "Added");
        assert_eq!(status_label_for('M'), "Modified");
        assert_eq!(status_label_for('D'), "Deleted");
        assert_eq!(status_label_for('R'), "Renamed");
        assert_eq!(status_label_for('C'), "Copied");
    }

    #[test]
    fn status_label_for_unknown_falls_back() {
        assert_eq!(status_label_for('X'), "Status X");
    }

    #[test]
    fn looks_like_no_head_recognizes_phrases() {
        let mk = |s: &str| GitOutput {
            stdout: String::new(),
            stderr: s.to_string(),
            exit_code: Some(128),
            timed_out: false,
            truncated: false,
        };
        assert!(looks_like_no_head(&mk(
            "fatal: ambiguous argument 'HEAD': unknown revision"
        )));
        assert!(looks_like_no_head(&mk(
            "fatal: your current branch 'main' does not have any commits yet"
        )));
        assert!(!looks_like_no_head(&mk("fatal: pathspec did not match")));
    }
}
