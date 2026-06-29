use std::collections::HashMap;
use std::io::{Read, Write};
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::sync::{Arc, Mutex, OnceLock};
use std::thread;
use std::time::{Duration, Instant};

use shared_child::SharedChild;

use crate::errors::{GitError, Result};
use crate::types::{
    GitOutput, TextSource, MAX_FILE_BYTES, MAX_OUTPUT_BYTES, MAX_TIMEOUT_SECS, MIN_GIT_VERSION,
};

/// Abstracts how a `git` subprocess is spawned and run for a host.
///
/// The default [`LocalGit`] impl spawns the system `git` binary directly. Hosts
/// with special execution needs — e.g. pterm running git inside a WSL distro —
/// extend execution by providing their own `GitTransport` impl; the operations
/// layer is unaware of the difference.
pub trait GitTransport: Send + Sync {
    /// Stable key identifying this transport, used to cache the git-availability
    /// probe. Distinct execution environments must return distinct keys.
    fn availability_key(&self) -> String;

    /// Spawn `git <args>` with `cwd` as the working directory, feeding `stdin`
    /// if `Some`, and return the captured result (exit status + bounded,
    /// decoded stdout/stderr) or a timeout. The implementation is responsible
    /// for env hardening, output bounding, and killing the process on timeout.
    fn run(
        &self,
        cwd: &Path,
        args: &[&str],
        stdin: Option<&[u8]>,
        timeout: Duration,
    ) -> Result<GitOutput>;
}

/// Default transport: spawns the local `git` binary.
#[derive(Clone, Copy, Debug, Default)]
pub struct LocalGit;

impl GitTransport for LocalGit {
    fn availability_key(&self) -> String {
        "local".into()
    }

    fn run(
        &self,
        cwd: &Path,
        args: &[&str],
        stdin: Option<&[u8]>,
        timeout: Duration,
    ) -> Result<GitOutput> {
        let dur = clamp_timeout(timeout);

        let mut cmd = Command::new("git");
        cmd.args(args);
        if !cwd.as_os_str().is_empty() {
            cmd.current_dir(cwd);
        }
        cmd.env("GIT_TERMINAL_PROMPT", "0")
            .env("GIT_ASKPASS", "")
            .env("SSH_ASKPASS", "")
            .env("GIT_OPTIONAL_LOCKS", "0")
            .env("GCM_INTERACTIVE", "Never")
            .env("GCM_PROVIDER", "")
            .env("LC_ALL", "C")
            .stdin(if stdin.is_some() {
                Stdio::piped()
            } else {
                Stdio::null()
            })
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        hide_console(&mut cmd);

        let child =
            Arc::new(SharedChild::spawn(&mut cmd).map_err(|e| GitError::Spawn(e.to_string()))?);

        if let Some(input) = stdin {
            if let Some(mut pipe) = child.take_stdin() {
                let _ = pipe.write_all(input);
                // Dropping the pipe closes stdin so git observes EOF.
            }
        }

        let mut stdout_pipe = child
            .take_stdout()
            .ok_or_else(|| GitError::Spawn("no stdout pipe".into()))?;
        let mut stderr_pipe = child
            .take_stderr()
            .ok_or_else(|| GitError::Spawn("no stderr pipe".into()))?;

        let stdout_handle = thread::spawn(move || drain(&mut stdout_pipe, 64 * 1024));
        let stderr_handle = thread::spawn(move || drain(&mut stderr_pipe, 4 * 1024));

        let (tx, rx) = mpsc::channel();
        let waiter = Arc::clone(&child);
        thread::spawn(move || {
            let _ = tx.send(waiter.wait());
        });

        let (exit_code, timed_out) = match rx.recv_timeout(dur) {
            Ok(Ok(status)) => (status.code(), false),
            Ok(Err(e)) => return Err(GitError::Io(e)),
            Err(mpsc::RecvTimeoutError::Timeout) => {
                let _ = child.kill();
                let _ = child.wait();
                (None, true)
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                return Err(GitError::Spawn("git wait thread disconnected".into()));
            }
        };

        let (stdout, stdout_truncated) = stdout_handle.join().unwrap_or((Vec::new(), false));
        let (stderr, _stderr_truncated) = stderr_handle.join().unwrap_or((Vec::new(), false));

        Ok(GitOutput {
            stdout: String::from_utf8_lossy(&stdout).into_owned(),
            stderr: String::from_utf8_lossy(&stderr).into_owned(),
            exit_code,
            timed_out,
            truncated: stdout_truncated,
        })
    }
}

/// On Windows, prevent a console window from flashing up for each git call.
/// `0x08000000` is `CREATE_NO_WINDOW`. No-op on other platforms.
#[cfg(windows)]
fn hide_console(cmd: &mut Command) {
    use std::os::windows::process::CommandExt;
    cmd.creation_flags(0x08000000);
}

#[cfg(not(windows))]
fn hide_console(_cmd: &mut Command) {}

fn clamp_timeout(timeout: Duration) -> Duration {
    let max = Duration::from_secs(MAX_TIMEOUT_SECS);
    let min = Duration::from_secs(1);
    timeout.clamp(min, max)
}

// ---------------------------------------------------------------------------
// Availability probe (cached per transport key).
// ---------------------------------------------------------------------------

#[derive(Clone)]
enum Availability {
    Ok,
    NotInstalled,
    TooOld(String),
}

const AVAILABILITY_TTL: Duration = Duration::from_secs(60);

struct AvailabilityCache {
    value: Availability,
    checked_at: Instant,
}

static GIT_AVAILABILITY: OnceLock<Mutex<HashMap<String, AvailabilityCache>>> = OnceLock::new();

fn availability_cell() -> &'static Mutex<HashMap<String, AvailabilityCache>> {
    GIT_AVAILABILITY.get_or_init(|| Mutex::new(HashMap::new()))
}

fn prune_expired_availability_entries(cache: &mut HashMap<String, AvailabilityCache>) {
    cache.retain(|_, entry| entry.checked_at.elapsed() < AVAILABILITY_TTL);
}

/// Ensure the transport's `git` is installed and recent enough
/// (>= [`MIN_GIT_VERSION`]). Results are cached per `availability_key`.
pub fn ensure_git_available(transport: &dyn GitTransport) -> Result<()> {
    let cache_key = transport.availability_key();
    let cached = {
        let mut guard = availability_cell()
            .lock()
            .expect("git availability poisoned");
        prune_expired_availability_entries(&mut guard);
        guard
            .get(&cache_key)
            .filter(|entry| entry.checked_at.elapsed() < AVAILABILITY_TTL)
            .map(|entry| entry.value.clone())
    };
    let value = match cached {
        Some(v) => v,
        None => {
            let fresh = check_git_availability(transport);
            let mut guard = availability_cell()
                .lock()
                .expect("git availability poisoned");
            prune_expired_availability_entries(&mut guard);
            guard.insert(
                cache_key,
                AvailabilityCache {
                    value: fresh.clone(),
                    checked_at: Instant::now(),
                },
            );
            fresh
        }
    };
    match value {
        Availability::Ok => Ok(()),
        Availability::NotInstalled => Err(GitError::NotInstalled),
        Availability::TooOld(v) => Err(GitError::TooOld {
            found: v,
            required: MIN_GIT_VERSION,
        }),
    }
}

fn check_git_availability(transport: &dyn GitTransport) -> Availability {
    let cwd = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
    let output = match transport.run(&cwd, &["--version"], None, Duration::from_secs(10)) {
        Ok(o) => o,
        Err(_) => return Availability::NotInstalled,
    };
    if output.timed_out || output.exit_code != Some(0) {
        return Availability::NotInstalled;
    }
    let version = parse_git_version(output.stdout.trim()).unwrap_or_else(|| "unknown".into());
    if !version_meets_minimum(&version, MIN_GIT_VERSION) {
        return Availability::TooOld(version);
    }
    Availability::Ok
}

fn parse_git_version(line: &str) -> Option<String> {
    line.split_whitespace()
        .find(|tok| tok.chars().next().is_some_and(|c| c.is_ascii_digit()))
        .map(|s| s.split('.').take(3).collect::<Vec<_>>().join("."))
}

fn version_meets_minimum(found: &str, required: &str) -> bool {
    let parse = |s: &str| -> Vec<u32> {
        s.split('.')
            .map(|p| p.parse::<u32>().unwrap_or(0))
            .collect()
    };
    let f = parse(found);
    let r = parse(required);
    for (i, &b) in r.iter().enumerate() {
        let a = f.get(i).copied().unwrap_or(0);
        if a > b {
            return true;
        }
        if a < b {
            return false;
        }
    }
    true
}

// ---------------------------------------------------------------------------
// Higher-level helpers shared by operations.
// ---------------------------------------------------------------------------

/// Run `git <args>` in `cwd` with the given timeout (seconds), no stdin.
pub(crate) fn run_git(
    transport: &dyn GitTransport,
    cwd: &Path,
    args: &[&str],
    timeout_secs: u64,
) -> Result<GitOutput> {
    transport.run(cwd, args, None, Duration::from_secs(timeout_secs))
}

/// Run git and return its `git show`-style text payload, classifying binary
/// content. A non-zero exit is treated as a missing object, matching pterm.
pub(crate) fn git_show_text(
    transport: &dyn GitTransport,
    repo_root: &Path,
    spec: &str,
) -> Result<TextSource> {
    let output = run_git(
        transport,
        repo_root,
        &["show", "--no-textconv", spec],
        crate::types::DEFAULT_TIMEOUT_SECS,
    )?;
    if output.timed_out {
        return Err(GitError::TimedOut("git show"));
    }
    if output.exit_code != Some(0) {
        return Ok(TextSource::Missing);
    }
    Ok(decode_text(output.stdout.into_bytes()))
}

/// Run git and return the first non-empty stdout line, or `None` if the command
/// fails (e.g. no upstream configured). Errors only on timeout/spawn.
pub(crate) fn git_stdout_line_opt(
    transport: &dyn GitTransport,
    cwd: &Path,
    args: &[&str],
) -> Result<Option<String>> {
    let output = run_git(transport, cwd, args, crate::types::DEFAULT_TIMEOUT_SECS)?;
    if output.timed_out {
        return Err(GitError::TimedOut("git command"));
    }
    if output.exit_code != Some(0) {
        return Ok(None);
    }
    let line = output.stdout.lines().next().unwrap_or("").trim();
    if line.is_empty() {
        Ok(None)
    } else {
        Ok(Some(line.to_string()))
    }
}

/// Run git, returning multiple stdout lines (UTF-8). Trailing `\r` stripped.
pub(crate) fn git_stdout_lines(
    transport: &dyn GitTransport,
    cwd: &Path,
    args: &[&str],
) -> Result<Vec<String>> {
    let output = run_git(transport, cwd, args, crate::types::DEFAULT_TIMEOUT_SECS)?;
    if output.timed_out {
        return Err(GitError::TimedOut("git command"));
    }
    if output.exit_code != Some(0) {
        return Ok(Vec::new());
    }
    Ok(output
        .stdout
        .lines()
        .map(|line| line.trim_end_matches('\r').to_string())
        .collect())
}

/// Read a worktree file as text, rejecting symlinks and over-large files.
pub(crate) fn read_text_file(path: &Path) -> Result<TextSource> {
    let meta = match std::fs::symlink_metadata(path) {
        Ok(m) => m,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(TextSource::Missing),
        Err(e) => return Err(GitError::Io(e)),
    };
    if meta.file_type().is_symlink() {
        return Err(GitError::SymlinkRejected(path.to_path_buf()));
    }
    if !meta.is_file() {
        return Ok(TextSource::Missing);
    }
    let size = meta.len();
    if size > MAX_FILE_BYTES {
        return Err(GitError::FileTooLarge {
            path: path.to_path_buf(),
            size,
            max: MAX_FILE_BYTES,
        });
    }
    let bytes = std::fs::read(path)?;
    Ok(decode_text(bytes))
}

/// Convert a non-zero git exit into a structured error, classifying auth/host
/// failures. Succeeds on exit code 0.
pub(crate) fn ensure_success(output: &GitOutput, context: &'static str) -> Result<()> {
    if output.timed_out {
        return Err(GitError::TimedOut(context));
    }
    if output.exit_code == Some(0) {
        return Ok(());
    }
    let stderr = output.stderr.trim().to_string();
    let stdout = output.stdout.trim().to_string();
    if let Some(err) = classify_auth_error(&stderr) {
        return Err(err);
    }
    let detail = if !stderr.is_empty() {
        stderr
    } else if !stdout.is_empty() {
        stdout
    } else {
        "unknown git error".into()
    };
    Err(GitError::CommandFailed { context, detail })
}

fn classify_auth_error(stderr: &str) -> Option<GitError> {
    let lower = stderr.to_ascii_lowercase();
    if lower.contains("could not read username")
        || lower.contains("could not read password")
        || lower.contains("authentication failed")
        || lower.contains("permission denied (publickey)")
        || lower.contains("invalid credentials")
    {
        return Some(GitError::AuthRequired(
            stderr.lines().next().unwrap_or(stderr).to_string(),
        ));
    }
    if lower.contains("host key verification failed") {
        return Some(GitError::HostKeyUnverified);
    }
    None
}

fn decode_text(bytes: Vec<u8>) -> TextSource {
    let sniff_len = bytes.len().min(8192);
    if bytes[..sniff_len].contains(&0) {
        return TextSource::Binary;
    }
    match String::from_utf8(bytes) {
        Ok(text) => TextSource::Text(text),
        Err(e) => TextSource::Text(String::from_utf8_lossy(&e.into_bytes()).into_owned()),
    }
}

fn drain<R: Read>(reader: &mut R, prealloc: usize) -> (Vec<u8>, bool) {
    let mut out: Vec<u8> = Vec::with_capacity(prealloc.min(MAX_OUTPUT_BYTES));
    let mut buf = [0u8; 16 * 1024];
    let mut truncated = false;
    loop {
        match reader.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => {
                if out.len() >= MAX_OUTPUT_BYTES {
                    truncated = true;
                    continue;
                }
                let take = (MAX_OUTPUT_BYTES - out.len()).min(n);
                out.extend_from_slice(&buf[..take]);
                if take < n {
                    truncated = true;
                }
            }
            Err(_) => break,
        }
    }
    (out, truncated)
}

#[cfg(test)]
mod tests {
    use super::{
        parse_git_version, prune_expired_availability_entries, version_meets_minimum, Availability,
        AvailabilityCache, AVAILABILITY_TTL,
    };
    use std::collections::HashMap;
    use std::time::{Duration, Instant};

    #[test]
    fn extracts_simple_version() {
        assert_eq!(
            parse_git_version("git version 2.42.0"),
            Some("2.42.0".into())
        );
    }

    #[test]
    fn extracts_apple_version() {
        assert_eq!(
            parse_git_version("git version 2.39.3 (Apple Git-145)"),
            Some("2.39.3".into())
        );
    }

    #[test]
    fn version_compare() {
        assert!(version_meets_minimum("2.23.0", "2.23"));
        assert!(version_meets_minimum("2.40.1", "2.23"));
        assert!(version_meets_minimum("3.0.0", "2.23"));
        assert!(!version_meets_minimum("2.22.0", "2.23"));
        assert!(!version_meets_minimum("1.9.5", "2.23"));
        // patch component must not regress the comparison
        assert!(version_meets_minimum("2.23.5", "2.23.4"));
        assert!(!version_meets_minimum("2.23.3", "2.23.4"));
    }

    #[test]
    fn prunes_expired_availability_entries() {
        let mut cache = HashMap::from([
            (
                "local".to_string(),
                AvailabilityCache {
                    value: Availability::Ok,
                    checked_at: Instant::now(),
                },
            ),
            (
                "custom".to_string(),
                AvailabilityCache {
                    value: Availability::NotInstalled,
                    checked_at: Instant::now() - AVAILABILITY_TTL - Duration::from_secs(1),
                },
            ),
        ]);

        prune_expired_availability_entries(&mut cache);

        assert!(cache.contains_key("local"));
        assert!(!cache.contains_key("custom"));
    }
}
