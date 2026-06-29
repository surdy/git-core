# @surdy/git-history-react

A host-agnostic React component library for browsing **git history + diffs**. It
renders a lane-based commit graph, a changed-file list, and a syntax-highlighted
diff (via [`@pierre/diffs`](https://www.npmjs.com/package/@pierre/diffs)). It
imports nothing app-specific — all git data flows in through a single injected
adapter, so pterm, madari, Notesmith, or any other host can reuse it.

## Install

```sh
pnpm add @surdy/git-history-react @surdy/git-core-types @pierre/diffs
# peer deps
pnpm add react react-dom
```

## Usage

```tsx
import { GitHistoryPanel, type GitDataSource } from "@surdy/git-history-react";
import "@surdy/git-history-react/style.css";

const dataSource: GitDataSource = {
  log: (repoRoot, { limit, before }) => invoke("git_log", { repoRoot, limit, before }),
  commitFiles: (repoRoot, sha) => invoke("git_commit_files", { repoRoot, sha }),
  commitFileDiff: (repoRoot, sha, path, originalPath) =>
    invoke("git_commit_file_diff", { repoRoot, sha, path, originalPath }),
  remoteUrl: (repoRoot) => invoke("git_remote_url", { repoRoot }), // optional
};

export function HistoryView() {
  return (
    <div style={{ height: "100vh" }}>
      <GitHistoryPanel dataSource={dataSource} repoRoot="/path/to/repo" theme="dark" />
    </div>
  );
}
```

The panel fills its parent, so give the container an explicit height.

## The adapter: `GitDataSource`

The host supplies git data through this interface. Shapes come straight from
[`@surdy/git-core-types`](https://www.npmjs.com/package/@surdy/git-core-types).

| Method | Returns | Purpose |
| --- | --- | --- |
| `log(repoRoot, { limit, before? })` | `GitLogEntry[]` | Commit log, newest-first. `before` = the sha to page after (exclusive). |
| `commitFiles(repoRoot, sha)` | `GitCommitFileChange[]` | Files changed in a commit. |
| `commitFileDiff(repoRoot, sha, path, originalPath)` | `GitDiffContentResult` | Old/new content for one file. |
| `remoteUrl?(repoRoot)` | `string \| null` | Optional. Enables a "view on GitHub/GitLab/Bitbucket" link. |

## Props: `GitHistoryPanelProps`

| Prop | Default | Notes |
| --- | --- | --- |
| `dataSource` | — | Required adapter (above). |
| `repoRoot` | — | Required absolute repo path. Re-loads when it changes. |
| `scopeKey` | `""` | Diff-cache namespace so multiple repos/hosts don't collide. |
| `theme` | `"dark"` | `"light"` / `"dark"`, mapped to shiki `github-light` / `github-dark`. |
| `pageSize` | `50` | Commits per page (initial + "Load more" / on-scroll). |
| `diffLayout` | `"stacked"` | `"stacked"` (unified) or `"split"`. |
| `onSelectCommit?(sha)` | — | Fired when a commit is selected. |
| `resolveFileIcon?(path)` | — | Optional path → icon URL. Without it, a status glyph is shown. |

## Theming (`--gh-*` CSS variables)

This package's CSS is independent of any host token system. Theme it by setting
`--gh-*` variables on a wrapper (or `:root`); every rule ships a literal fallback
so it looks reasonable standalone.

| Variable | Fallback | Role |
| --- | --- | --- |
| `--gh-bg` | `#1e1e1e` | Panel / diff background |
| `--gh-panel` | `#252525` | Commit-list & file-list surface |
| `--gh-text` | `#ddd` | Primary text |
| `--gh-muted` | `#888` | Secondary / metadata text |
| `--gh-border` | `#333` | Dividers and outlines |
| `--gh-accent` | `#60a5fa` | Links, short-sha, active accents |
| `--gh-success` | `#34d399` | Insertions (`+N`) |
| `--gh-danger` | `#f87171` | Deletions (`-N`) and errors |
| `--gh-hover` | `rgba(255,255,255,0.05)` | Row hover surface |
| `--gh-selected` | `rgba(96,165,250,0.16)` | Selected-row surface |
| `--gh-font-mono` | system mono stack | sha / paths / counts |
| `--gh-rail-node-stroke` | `var(--gh-bg)` | Commit-node outline in the graph rail |

```css
.my-host-theme {
  --gh-bg: #0d1117;
  --gh-panel: #161b22;
  --gh-accent: #58a6ff;
}
```

> The commit-graph lane palette in `lib/graph.ts` stays literal — it's data-vis
> color, not chrome.

## Exports

- `GitHistoryPanel` — the full panel.
- `GraphRail`, `MAX_VISIBLE_LANES`, `railWidth` — the SVG commit-graph rail.
- `PierreDiffView` — standalone two-content diff view.
- `layoutGraph`, `EMPTY_GRAPH_STATE` + `GraphEdge` / `GraphRow` / `GraphState`.
- `parseRemoteWebUrl`, `commitWebUrl`, `hostLabel` + `RemoteWebHost` / `RemoteWebInfo`.
- `createDiffCache` + `DiffCache` — LRU-of-6 + inflight-dedupe diff cache factory.
- `type { GitDataSource, GitHistoryPanelProps }`.

Stylesheet: `@surdy/git-history-react/style.css` (also side-effect-imported by
`GitHistoryPanel`).

## License

Apache-2.0
