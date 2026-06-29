import type { GitDiffContentResult } from "@surdy/git-core-types";
import type { GitDataSource } from "../types";

const DIFF_CACHE_LIMIT = 6;

export type DiffCache = {
  /** Fetch (and cache) the old/new content diff for one file in a commit. */
  fetchCommitDiff(
    repoRoot: string,
    sha: string,
    path: string,
    originalPath: string | null,
  ): Promise<GitDiffContentResult>;
  /** Synchronously read a cached entry by its commit-diff key, if present. */
  getCached(key: string): GitDiffContentResult | undefined;
  /** Build the cache key for a commit-file pair (handy for `getCached`). */
  keyFor(repoRoot: string, sha: string, path: string): string;
};

/**
 * Create an isolated diff cache scoped by `scopeKey` and backed by the given
 * data source. Keeps an LRU of the 6 most-recent diffs plus an in-flight map
 * so concurrent requests for the same file dedupe to a single fetch.
 */
export function createDiffCache(
  source: GitDataSource,
  scopeKey: string,
): DiffCache {
  const inflight = new Map<string, Promise<GitDiffContentResult>>();
  const cache = new Map<string, GitDiffContentResult>();

  function touch(key: string, value: GitDiffContentResult) {
    cache.delete(key);
    cache.set(key, value);
    while (cache.size > DIFF_CACHE_LIMIT) {
      const oldest = cache.keys().next().value;
      if (oldest === undefined) break;
      cache.delete(oldest);
    }
  }

  function getCached(key: string): GitDiffContentResult | undefined {
    const hit = cache.get(key);
    if (hit) {
      cache.delete(key);
      cache.set(key, hit);
    }
    return hit;
  }

  function keyFor(repoRoot: string, sha: string, path: string): string {
    return `${scopeKey}|${repoRoot}|c|${sha}|${path}`;
  }

  async function fetchCommitDiff(
    repoRoot: string,
    sha: string,
    path: string,
    originalPath: string | null,
  ): Promise<GitDiffContentResult> {
    const key = keyFor(repoRoot, sha, path);
    const cached = getCached(key);
    if (cached) return cached;
    const pending = inflight.get(key);
    if (pending) return pending;
    const p = source
      .commitFileDiff(repoRoot, sha, path, originalPath)
      .then((res) => {
        touch(key, res);
        return res;
      })
      .finally(() => {
        inflight.delete(key);
      });
    inflight.set(key, p);
    return p;
  }

  return { fetchCommitDiff, getCached, keyFor };
}
