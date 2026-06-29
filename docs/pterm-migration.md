# Migrating pterm onto the shared git-core layer

This is a hand-off brief for an agent (or human) rewriting **pterm** so it
reuses the shared git layer instead of maintaining its own copy. pterm is where
this code was extracted from, so the migration is mostly *deleting* pterm's
local git engine + UI and wiring it to the shared packages, plus supplying the
two small host-specific adapters (a Rust transport, a TS data source).

> Sibling targets: madari (no git code yet — it just adopts the same packages)
> and Notesmith (Svelte; consumes `@surdy/git-history-react` through a React
> island). This brief is pterm-specific but the adapter shapes are identical.

---

## 1. What the shared layer is

Repo: **`surdy/git-core`** (this repo) — a polyglot monorepo.

| Layer | Package / crate | What it provides | Where it lives |
|---|---|---|---|
| 1. Engine (Rust) | crate `git-core` | Shells out to system `git`; returns serde structs (log, status, structured diffs). Execution behind the `GitTransport` trait (default `LocalGit`). **No** authorization/workspace policy — the host passes a *trusted* repo root. | repo root: `src/`, `Cargo.toml` |
| 2. Types (TS) | `@surdy/git-core-types` | The TS contract, **generated from the Rust structs** via ts-rs. Drift = a compile error. | `packages/git-core-types/` |
| 3. UI (React) | `@surdy/git-history-react` | Host-agnostic history + diff UI (`GitHistoryPanel`, `GraphRail`, `PierreDiffView`, graph layout, diff cache). Gets data through an injected `GitDataSource`. | `packages/git-history-react/` |

The engine was carved out of pterm's `src-tauri/src/modules/git/` by replacing
the `WorkspaceRegistry`/`WorkspaceEnv` coupling with the `GitTransport` seam.
The operation set and parsing are otherwise faithful, so behavior matches.

---

## 2. pterm's current state (what gets replaced)

**Rust** — `pterm/src-tauri/src/modules/git/`:
- `types.rs`, `errors.rs`, `parser.rs`, `process.rs`, `operations.rs`, `utils.rs`
  → **deleted**, replaced by a dependency on the `git-core` crate.
- `commands.rs` (the 17 `#[tauri::command]`s) → **kept, thinned**: still the
  Tauri surface, but each command now (a) builds a transport from `WorkspaceEnv`,
  (b) resolves + authorizes the path via `WorkspaceRegistry` (pterm keeps this
  responsibility), (c) delegates to `git_core::operations::*`.

**TS types** — `pterm/src/lib/native.ts` hand-declares `GitLogEntry`,
`GitCommitFileChange`, `GitDiffContentResult`, etc. → replace with re-exports
from `@surdy/git-core-types` (delete the hand-written duplicates).

**React UI** — replaced by `@surdy/git-history-react`:
- `pterm/src/modules/git-history/**` (`GitHistoryPane` 1036 lines, `GraphRail`,
  `lib/graph.ts`, `lib/remoteWebUrl.ts`, lazy wrappers) and
- `pterm/src/modules/editor/{PierreDiffView,GitDiffPane,GitDiffStack,lib/diffCache}.tsx/ts`
  → replaced by `<GitHistoryPanel>` + a pterm `GitDataSource` adapter. pterm
  keeps its own *chrome* (tabs integration `GitHistoryStack`, settings) around
  the shared panel.

---

## 3. The two adapters pterm must supply

### 3a. Rust: `GitTransport` for pterm's `WorkspaceEnv`

`git-core` ships `LocalGit`. pterm also runs git inside **WSL**
(`WorkspaceEnv::Wsl { distro }` → `wsl.exe -d <distro> --cd <cwd> --exec git …`),
so pterm provides its own transport that switches on `WorkspaceEnv`:

```rust
// pterm/src-tauri/src/modules/git_host/transport.rs (new)
use std::path::Path;
use std::time::Duration;
use git_core::{GitTransport, GitOutput, GitError, Result, LocalGit};
use crate::modules::workspace::WorkspaceEnv;

pub struct PtermTransport { pub env: WorkspaceEnv }

impl GitTransport for PtermTransport {
    fn availability_key(&self) -> String {
        match &self.env {
            WorkspaceEnv::Local => "local".into(),
            WorkspaceEnv::Wsl { distro } => format!("wsl:{}", distro.name),
        }
    }
    fn run(&self, cwd: &Path, args: &[&str], stdin: Option<&[u8]>, timeout: Duration) -> Result<GitOutput> {
        match &self.env {
            WorkspaceEnv::Local => LocalGit.run(cwd, args, stdin, timeout),
            WorkspaceEnv::Wsl { distro } => {
                // Build `wsl.exe -d <distro> --cd <cwd> --exec git <args>` and run it,
                // reusing the same env-hardening / draining / timeout discipline.
                // (Port the WSL spawn branch that used to live in pterm's process.rs.)
                todo!("spawn via wsl.exe; map output into GitOutput")
            }
        }
    }
}
```

Notes:
- The env-hardening, output bounding, timeout-kill, and draining are already in
  `LocalGit`; the WSL branch just changes the program/args. Keep the WSL output
  decoding identical to before.
- `git-core`'s availability probe is cached per `availability_key()`, so the
  WSL key must encode the distro.

### 3b. Rust: thin command glue (authorization stays in pterm)

`pterm/src-tauri/src/modules/git/commands.rs` keeps doing authorization, then
delegates. Before vs after for one command:

```rust
// BEFORE
operations::resolve_repo(registry, &cwd, &workspace).map_err(Into::into)

// AFTER
let dir = registry.authorize(&cwd)?;                 // pterm still authorizes
let transport = PtermTransport { env: workspace };   // build the transport
git_core::operations::resolve_repo(&transport, &dir) // delegate (trusted path)
    .map_err(|e| e.to_string())
```

Apply the same shape to all 17 commands (`git_status`, `git_diff`,
`git_diff_content`, `git_stage`, `git_unstage`, `git_discard`, `git_commit`,
`git_fetch`, `git_pull_ff_only`, `git_push`, `git_log`, `git_show_commit`,
`git_commit_files`, `git_commit_file_diff`, `git_remote_url`,
`git_panel_snapshot`, `git_resolve_repo`). The Tauri command **names and TS
signatures do not change**, so the frontend `invoke(...)` calls keep working —
which is what lets the React swap (3c) be independent of the Rust swap.

`Cargo.toml`: add `git-core = { path = "../../../git-core" }` (or a git/version
dep once published). Delete the `modules/git/{types,errors,parser,process,operations,utils}.rs`
files; keep `commands.rs` + the new `git_host/transport.rs`.

### 3c. TS: `GitDataSource` over pterm's Tauri invokes

`@surdy/git-history-react` needs a `GitDataSource`. pterm implements it with its
existing `native.*` invoke wrappers:

```ts
// pterm/src/modules/git-history/ptermDataSource.ts (new)
import type { GitDataSource } from "@surdy/git-history-react";
import { native } from "@/lib/native";

export const ptermDataSource: GitDataSource = {
  log: (repoRoot, { limit, before }) => native.gitLog(repoRoot, limit, before),
  commitFiles: (repoRoot, sha) => native.gitCommitFiles(repoRoot, sha),
  commitFileDiff: (repoRoot, sha, path, originalPath) =>
    native.gitCommitFileDiff(repoRoot, sha, path, originalPath),
  remoteUrl: (repoRoot) => native.gitRemoteUrl(repoRoot),
};
```
(Adjust to pterm's actual `native` signatures — `gitLog` currently takes
`(repoRoot, limit, before, workspace)`; bind the active workspace in the
closure.)

Then render the shared panel where `GitHistoryPane` used to be:

```tsx
import { GitHistoryPanel } from "@surdy/git-history-react";
import "@surdy/git-history-react/style.css";
import { ptermDataSource } from "./ptermDataSource";

<GitHistoryPanel
  dataSource={ptermDataSource}
  repoRoot={repoRoot}
  scopeKey={currentWorkspaceScopeKey()}
  theme={isDark ? "dark" : "light"}
  diffLayout={diffLayout}
  resolveFileIcon={(p) => fileIconUrl(p)}   // pterm's existing icon resolver
/>
```

**Types swap:** in `native.ts`, delete the local `GitLogEntry` /
`GitCommitFileChange` / `GitDiffContentResult` / … declarations and instead
`export type { … } from "@surdy/git-core-types";`. Anything importing them from
`@/lib/native` keeps compiling.

**Theming:** map pterm's theme onto the panel's `--gh-*` CSS variables (see the
package README) on the panel's wrapper element so it matches pterm's palette.

---

## 4. Suggested order of work

1. **Rust first, behaviorally invisible:** add the `git-core` path dep, write
   `PtermTransport`, rewrite `commands.rs` to authorize-then-delegate, delete the
   old engine files. `cargo build` + `cargo clippy -D warnings` + existing pterm
   git tests green. The frontend is untouched and still works (same command
   names). Commit.
2. **TS types swap:** point `native.ts` git types at `@surdy/git-core-types`.
   `tsc`/`check` green. Commit.
3. **React swap:** add `@surdy/git-history-react`, write `ptermDataSource`,
   replace `GitHistoryPane`/diff components with `<GitHistoryPanel>`, delete the
   now-dead `git-history/**` + `editor/Git*`/`diffCache` files, wire `--gh-*`
   theming, keep the tabs/`GitHistoryStack` chrome. Build + visually verify.
   Commit.

Each step is independently shippable, which de-risks the rewrite.

---

## 5. Validation

- **Rust:** `cd pterm/src-tauri && cargo build && cargo clippy -- -D warnings && cargo test`.
  Manually exercise: resolve repo, status, stage/commit, log paging, show commit
  diff — on both a Local and a WSL workspace (the WSL transport branch is the one
  piece of genuinely new pterm code).
- **TS:** pterm's `tsc`/lint + `pnpm build`.
- **UI:** open the history pane, scroll/paginate, select commits + files, confirm
  the graph rail and Pierre diff render and themes follow pterm.

---

## 6. Deferred / not-yet-in the shared panel

`@surdy/git-history-react`'s `GitHistoryPanel` covers the core flow (paginated
commit list + graph rail, commit → changed files → Pierre diff, remote "view on
host" link). The following pterm-pane features were **not** carried into the
shared component and must either stay as pterm-side chrome around the panel or be
upstreamed into the package later:

- **Commit search popover** — pterm's `GitHistorySearchHandle` / `onSearchHandle`
  deferred-value filtering.
- **Virtualization** — the panel uses a plain scrollable list + on-scroll /
  "Load more" instead of `@tanstack/react-virtual`. Fine for typical history
  depths; revisit if pterm scrolls tens of thousands of commits.
- **Overlay / sticky graph rail** alignment tricks and the fixed table-grid
  header.
- **Keyboard navigation** between commit rows.
- **Chrome**: tooltips, copy-sha button, author-initial avatars, HugeIcons.
- **Working-tree / staging diff path** — `fetchWorkingDiff` / `workingDiffKey`
  are out of scope; the shared `createDiffCache` exposes only `fetchCommitDiff`
  (commit history diffs). pterm's uncommitted-changes/staging view stays a
  pterm concern for now (it can keep calling `git_diff_content` directly).
- **Tabs integration** — pterm's `onOpenCommitFile` (open a diff in a host tab).
  The shared panel shows the diff in an in-panel pane instead; if pterm wants
  tab-opening, add an optional `onOpenFile?(sha, path)` callback to the panel.

What the shared `GitHistoryPanel` **does** deliver: paginated commit list with
the graph rail, commit → changed-file list (optional icons) → per-file Pierre
diff, binary-file placeholder, remote "view on &lt;host&gt;" link, and
loading/empty/error states with real `<button>`s.

If pterm depends on any deferred item, the cleanest path is to add it to
`@surdy/git-history-react` (so madari/Notesmith benefit too) rather than forking
the component back into pterm. Several map to clean prop additions
(`onOpenFile?`, a search handle, a `virtualize?` flag).
