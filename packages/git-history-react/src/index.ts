// @surdy/git-history-react — host-agnostic React UI for browsing git history
// and diffs. Data flows in through the injected `GitDataSource` adapter; the
// library imports nothing app-specific.
//
// Styles: GitHistoryPanel.tsx side-effect-imports "./git-history.css", which
// tsup emits to dist/index.css. Hosts also import it explicitly:
//   import "@surdy/git-history-react/style.css";

export { GitHistoryPanel } from "./GitHistoryPanel";
export { GraphRail, MAX_VISIBLE_LANES, railWidth } from "./GraphRail";
export { PierreDiffView } from "./PierreDiffView";

export {
  layoutGraph,
  EMPTY_GRAPH_STATE,
  type GraphEdge,
  type GraphRow,
  type GraphState,
} from "./lib/graph";

export {
  parseRemoteWebUrl,
  commitWebUrl,
  hostLabel,
  type RemoteWebHost,
  type RemoteWebInfo,
} from "./lib/remoteWebUrl";

export { createDiffCache, type DiffCache } from "./lib/diffCache";

export type { GitDataSource, GitHistoryPanelProps } from "./types";
