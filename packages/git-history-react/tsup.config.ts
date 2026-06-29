import { defineConfig } from "tsup";

export default defineConfig({
  entry: ["src/index.ts"],
  format: ["esm"],
  dts: true,
  sourcemap: true,
  clean: true,
  // Keep the host's copies of React and the diff engine out of the bundle.
  external: ["react", "react-dom", "@pierre/diffs"],
  // We import "./git-history.css" from GitHistoryPanel.tsx; with injectStyle
  // disabled tsup/esbuild emits it as a sibling file -> dist/index.css.
  injectStyle: false,
});
