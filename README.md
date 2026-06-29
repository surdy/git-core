# git-core

A host-agnostic **git history, diff, and status engine** in Rust, shared across
surdy's apps (pterm, madari, and Notesmith) so the same git plumbing and diff
model isn't reimplemented per app.

This crate is the **Layer 1** of the shared git-history/diff plan: it produces
presentation-agnostic, `serde`-serializable data (commit log, status snapshot,
structured diffs). Each app renders it in its own UI (React for pterm/madari, a
React island inside Svelte for Notesmith) and pairs it with the matching TS type
contract.

## Design

- **Shells out to the system `git` binary** (no libgit2). This inherits the
  user's credential helpers, SSH config, and `~/.gitconfig` for free, and keeps
  the dependency surface tiny. Minimum git version: 2.23.
- **Transport seam.** Command execution is abstracted behind the
  [`GitTransport`] trait. The crate ships a default [`LocalGit`] transport
  (spawns local `git`, hardened env, timeouts, bounded output). Hosts with
  special execution needs — e.g. pterm running git inside a WSL distro — provide
  their own `GitTransport` impl without touching the operations.
- **No workspace/authorization policy.** Path authorization and sandboxing are
  the host's responsibility. The host resolves and authorizes a repo root, then
  calls these operations with a trusted working directory. Generic per-repo path
  safety (`is_safe_pathspec`, `resolve_within_repo`) lives here.
- **Read and write ops.** status, log, show-commit diff, per-file diff,
  staged/working diff content, stage/unstage/discard/commit, and (optional for
  local-only hosts) fetch/pull/push.

## Provenance

The engine is carved out of pterm's mature `src-tauri/src/modules/git` module,
generalized by removing the `WorkspaceRegistry`/`WorkspaceEnv` coupling behind
the `GitTransport` seam.

## Migration path

- **pterm**: implement `GitTransport` for its `WorkspaceEnv` (Local + WSL),
  resolve/authorize paths as today, then delegate to `git-core` operations.
- **Notesmith**: use the default `LocalGit` transport for local-only history,
  diff, status, and auto-commit; never call the remote ops.

## License

Apache-2.0.
