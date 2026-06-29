import {
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
  type UIEvent,
} from "react";
import type {
  GitLogEntry,
  GitCommitFileChange,
  GitDiffContentResult,
} from "@surdy/git-core-types";
import type { GitHistoryPanelProps } from "./types";
import {
  EMPTY_GRAPH_STATE,
  layoutGraph,
  type GraphRow,
  type GraphState,
} from "./lib/graph";
import { GraphRail, MAX_VISIBLE_LANES } from "./GraphRail";
import { createDiffCache } from "./lib/diffCache";
import {
  commitWebUrl,
  hostLabel,
  parseRemoteWebUrl,
  type RemoteWebInfo,
} from "./lib/remoteWebUrl";
import { PierreDiffView } from "./PierreDiffView";
import "./git-history.css";

const ROW_HEIGHT = 38;

function normalizeError(error: unknown): string {
  if (typeof error === "string") return error;
  if (error && typeof error === "object" && "message" in error) {
    const message = (error as { message?: unknown }).message;
    if (typeof message === "string") return message;
  }
  return "Unknown error";
}

function relativeTime(secs: number): string {
  if (!secs) return "";
  const deltaSec = Math.floor(Date.now() / 1000) - secs;
  if (deltaSec < 60) return "just now";
  const mins = Math.floor(deltaSec / 60);
  if (mins < 60) return `${mins}m ago`;
  const hours = Math.floor(mins / 60);
  if (hours < 24) return `${hours}h ago`;
  const days = Math.floor(hours / 24);
  if (days < 30) return `${days}d ago`;
  const months = Math.floor(days / 30);
  if (months < 12) return `${months}mo ago`;
  return `${Math.floor(months / 12)}y ago`;
}

function absoluteTime(secs: number): string {
  if (!secs) return "";
  return new Date(secs * 1000).toLocaleString(undefined, {
    year: "numeric",
    month: "short",
    day: "numeric",
    hour: "2-digit",
    minute: "2-digit",
  });
}

function statusGlyph(file: GitCommitFileChange): string {
  const s = file.status.toLowerCase();
  if (s.startsWith("a")) return "A";
  if (s.startsWith("d")) return "D";
  if (s.startsWith("r")) return "R";
  if (s.startsWith("c")) return "C";
  return "M";
}

type FileSelection = {
  file: GitCommitFileChange;
  state:
    | { kind: "loading" }
    | { kind: "loaded"; diff: GitDiffContentResult }
    | { kind: "error"; error: string };
};

export function GitHistoryPanel({
  dataSource,
  repoRoot,
  scopeKey = "",
  theme = "dark",
  pageSize = 50,
  diffLayout = "stacked",
  onSelectCommit,
  resolveFileIcon,
}: GitHistoryPanelProps) {
  const [commits, setCommits] = useState<GitLogEntry[]>([]);
  const [graphRows, setGraphRows] = useState<GraphRow[]>([]);
  const graphStateRef = useRef<GraphState>(EMPTY_GRAPH_STATE);
  const [status, setStatus] = useState<
    "idle" | "initial" | "more" | "error" | "ready"
  >("idle");
  const [error, setError] = useState<string | null>(null);
  const [hasMore, setHasMore] = useState(true);

  const [selectedSha, setSelectedSha] = useState<string | null>(null);
  const [files, setFiles] = useState<
    | { kind: "idle" }
    | { kind: "loading" }
    | { kind: "loaded"; files: GitCommitFileChange[] }
    | { kind: "error"; error: string }
  >({ kind: "idle" });
  const [selectedFile, setSelectedFile] = useState<FileSelection | null>(null);
  const [remote, setRemote] = useState<RemoteWebInfo | null>(null);

  const diffCache = useMemo(
    () => createDiffCache(dataSource, scopeKey),
    [dataSource, scopeKey],
  );

  const maxLaneCount = useMemo(() => {
    let max = 1;
    for (const r of graphRows) max = Math.max(max, r.laneCount);
    return Math.min(max, MAX_VISIBLE_LANES);
  }, [graphRows]);

  // Load the first page whenever the repo changes.
  useEffect(() => {
    let cancelled = false;
    setCommits([]);
    setGraphRows([]);
    graphStateRef.current = EMPTY_GRAPH_STATE;
    setSelectedSha(null);
    setFiles({ kind: "idle" });
    setSelectedFile(null);
    setHasMore(true);
    setError(null);
    setStatus("initial");
    setRemote(null);

    dataSource
      .log(repoRoot, { limit: pageSize })
      .then((page) => {
        if (cancelled) return;
        const { rows, state } = layoutGraph(page, EMPTY_GRAPH_STATE);
        graphStateRef.current = state;
        setCommits(page);
        setGraphRows(rows);
        setHasMore(page.length >= pageSize);
        setStatus("ready");
      })
      .catch((e) => {
        if (cancelled) return;
        setError(normalizeError(e));
        setStatus("error");
      });

    if (dataSource.remoteUrl) {
      dataSource
        .remoteUrl(repoRoot)
        .then((url) => {
          if (!cancelled) setRemote(parseRemoteWebUrl(url));
        })
        .catch(() => {
          if (!cancelled) setRemote(null);
        });
    }

    return () => {
      cancelled = true;
    };
  }, [dataSource, repoRoot, pageSize]);

  const loadMore = useCallback(() => {
    if (status === "more" || !hasMore) return;
    const last = commits[commits.length - 1];
    if (!last) return;
    setStatus("more");
    dataSource
      .log(repoRoot, { limit: pageSize, before: last.sha })
      .then((page) => {
        const { rows, state } = layoutGraph(page, graphStateRef.current);
        graphStateRef.current = state;
        setCommits((prev) => [...prev, ...page]);
        setGraphRows((prev) => [...prev, ...rows]);
        setHasMore(page.length >= pageSize);
        setStatus("ready");
      })
      .catch((e) => {
        setError(normalizeError(e));
        setStatus("error");
      });
  }, [dataSource, repoRoot, pageSize, commits, hasMore, status]);

  const selectCommit = useCallback(
    (sha: string) => {
      setSelectedSha(sha);
      setSelectedFile(null);
      onSelectCommit?.(sha);
      setFiles({ kind: "loading" });
      dataSource
        .commitFiles(repoRoot, sha)
        .then((list) => {
          setFiles({ kind: "loaded", files: list });
        })
        .catch((e) => {
          setFiles({ kind: "error", error: normalizeError(e) });
        });
    },
    [dataSource, repoRoot, onSelectCommit],
  );

  const selectFile = useCallback(
    (file: GitCommitFileChange) => {
      if (!selectedSha) return;
      const sha = selectedSha;
      if (file.isBinary) {
        setSelectedFile({
          file,
          state: {
            kind: "loaded",
            diff: {
              originalContent: "",
              modifiedContent: "",
              isBinary: true,
              fallbackPatch: "",
              truncated: false,
            },
          },
        });
        return;
      }
      setSelectedFile({ file, state: { kind: "loading" } });
      diffCache
        .fetchCommitDiff(repoRoot, sha, file.path, file.originalPath)
        .then((diff) => {
          setSelectedFile((cur) =>
            cur && cur.file.path === file.path
              ? { file, state: { kind: "loaded", diff } }
              : cur,
          );
        })
        .catch((e) => {
          setSelectedFile((cur) =>
            cur && cur.file.path === file.path
              ? { file, state: { kind: "error", error: normalizeError(e) } }
              : cur,
          );
        });
    },
    [diffCache, repoRoot, selectedSha],
  );

  const selectedCommit = useMemo(
    () => commits.find((c) => c.sha === selectedSha) ?? null,
    [commits, selectedSha],
  );

  const onListScroll = useCallback(
    (e: UIEvent<HTMLUListElement>) => {
      const el = e.currentTarget;
      if (el.scrollHeight - el.scrollTop - el.clientHeight < 240) {
        loadMore();
      }
    },
    [loadMore],
  );

  return (
    <div className="gh-root">
      <div className="gh-left">
        {status === "initial" ? (
          <div className="gh-status">Loading history…</div>
        ) : status === "error" && commits.length === 0 ? (
          <div className="gh-status gh-status--error">
            {error ?? "Failed to load history."}
          </div>
        ) : commits.length === 0 ? (
          <div className="gh-status">No commits.</div>
        ) : (
          <ul
            className="gh-list"
            role="listbox"
            aria-label="Commit history"
            onScroll={onListScroll}
          >
            {commits.map((c, i) => {
              const active = c.sha === selectedSha;
              const row = graphRows[i];
              return (
                <li key={c.sha}>
                  <button
                    type="button"
                    role="option"
                    aria-selected={active}
                    className={`gh-item${active ? " gh-item--active" : ""}`}
                    onClick={() => selectCommit(c.sha)}
                  >
                    <span className="gh-item-rail">
                      {row ? (
                        <GraphRail
                          row={row}
                          rowHeight={ROW_HEIGHT}
                          maxLaneCount={maxLaneCount}
                          active={active}
                        />
                      ) : null}
                    </span>
                    <span className="gh-item-body">
                      <span className="gh-item-subject">{c.subject}</span>
                      <span className="gh-item-meta">
                        <span className="gh-sha">{c.shortSha}</span>
                        <span className="gh-item-author">{c.author}</span>
                        <span className="gh-item-when">
                          {relativeTime(c.timestampSecs)}
                        </span>
                      </span>
                    </span>
                    <span className="gh-item-stats">
                      <span className="gh-add">+{c.insertions}</span>
                      <span className="gh-del">-{c.deletions}</span>
                    </span>
                  </button>
                </li>
              );
            })}
          </ul>
        )}
        {commits.length > 0 && hasMore ? (
          <button
            type="button"
            className="gh-loadmore"
            onClick={loadMore}
            disabled={status === "more"}
          >
            {status === "more" ? "Loading…" : "Load more"}
          </button>
        ) : null}
      </div>

      <div className="gh-right">
        {selectedCommit ? (
          <>
            <div className="gh-detail-head">
              <h2 className="gh-detail-subject">{selectedCommit.subject}</h2>
              <div className="gh-detail-meta">
                <span className="gh-sha">{selectedCommit.shortSha}</span>
                <span>{selectedCommit.author}</span>
                <span>{absoluteTime(selectedCommit.timestampSecs)}</span>
                <span className="gh-add">+{selectedCommit.insertions}</span>
                <span className="gh-del">-{selectedCommit.deletions}</span>
                {remote ? (
                  <a
                    className="gh-remote-link"
                    href={commitWebUrl(remote, selectedCommit.sha)}
                    target="_blank"
                    rel="noreferrer noopener"
                  >
                    {hostLabel(remote)}
                  </a>
                ) : null}
              </div>
            </div>
            <div className="gh-detail-body">
              <ul className="gh-files" aria-label="Changed files">
                {files.kind === "loading" ? (
                  <li className="gh-status">Loading files…</li>
                ) : files.kind === "error" ? (
                  <li className="gh-status gh-status--error">{files.error}</li>
                ) : files.kind === "loaded" && files.files.length === 0 ? (
                  <li className="gh-status">No file changes.</li>
                ) : files.kind === "loaded" ? (
                  files.files.map((file) => {
                    const fileActive = selectedFile?.file.path === file.path;
                    const icon = resolveFileIcon?.(file.path);
                    return (
                      <li key={file.path}>
                        <button
                          type="button"
                          className={`gh-file${fileActive ? " gh-file--active" : ""}`}
                          aria-pressed={fileActive}
                          onClick={() => selectFile(file)}
                          title={file.statusLabel}
                        >
                          {icon ? (
                            <img className="gh-file-icon" src={icon} alt="" />
                          ) : (
                            <span className="gh-file-status">
                              {statusGlyph(file)}
                            </span>
                          )}
                          <span className="gh-file-path">{file.path}</span>
                          <span className="gh-file-stats">
                            <span className="gh-add">+{file.added}</span>
                            <span className="gh-del">-{file.removed}</span>
                          </span>
                        </button>
                      </li>
                    );
                  })
                ) : null}
              </ul>
              <div className="gh-diff">
                {!selectedFile ? (
                  <div className="gh-placeholder">
                    Select a file to view its diff.
                  </div>
                ) : selectedFile.state.kind === "loading" ? (
                  <div className="gh-placeholder">Loading diff…</div>
                ) : selectedFile.state.kind === "error" ? (
                  <div className="gh-placeholder">{selectedFile.state.error}</div>
                ) : selectedFile.state.diff.isBinary ? (
                  <div className="gh-placeholder">
                    Binary file — diff not shown.
                  </div>
                ) : (
                  <PierreDiffView
                    oldCode={selectedFile.state.diff.originalContent}
                    newCode={selectedFile.state.diff.modifiedContent}
                    filename={selectedFile.file.path}
                    layout={diffLayout}
                    theme={theme}
                  />
                )}
              </div>
            </div>
          </>
        ) : (
          <div className="gh-placeholder">
            Select a commit to view its changes.
          </div>
        )}
      </div>
    </div>
  );
}
