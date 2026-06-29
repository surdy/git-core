import type {
  GitLogEntry,
  GitCommitFileChange,
  GitDiffContentResult,
} from "@surdy/git-core-types";

/** How the host fetches git data. Each app (pterm, madari, Notesmith) supplies its own. */
export interface GitDataSource {
  /** Commit log, newest-first. `before` = load the page after this sha (exclusive) for pagination. */
  log(repoRoot: string, opts: { limit: number; before?: string }): Promise<GitLogEntry[]>;
  /** Files changed in a commit. */
  commitFiles(repoRoot: string, sha: string): Promise<GitCommitFileChange[]>;
  /** Old/new content for one file in a commit, for the diff view. */
  commitFileDiff(repoRoot: string, sha: string, path: string, originalPath: string | null): Promise<GitDiffContentResult>;
  /** Optional: remote URL, used to render a "view on GitHub/GitLab/Bitbucket" link. */
  remoteUrl?(repoRoot: string): Promise<string | null>;
}

export interface GitHistoryPanelProps {
  dataSource: GitDataSource;
  repoRoot: string;
  /** Cache namespace so multiple repos/hosts don't collide. Default "". */
  scopeKey?: string;
  /** Diff theme. Default "dark". */
  theme?: "light" | "dark";
  /** Commits per page. Default 50. */
  pageSize?: number;
  /** Diff layout. Default "stacked". */
  diffLayout?: "stacked" | "split";
  /** Fired when the user selects a commit. */
  onSelectCommit?(sha: string): void;
  /** Optional file-icon resolver (path -> icon URL). When absent, no icons. */
  resolveFileIcon?(path: string): string | undefined;
}
