import { useMemo } from "react";
import { FileDiff } from "@pierre/diffs/react";
import { parseDiffFromFile, type FileDiffMetadata } from "@pierre/diffs";

type Props = {
  oldCode: string;
  newCode: string;
  filename: string;
  layout?: "stacked" | "split";
  /** Host theme. Mapped to shiki github-dark / github-light. Default "dark". */
  theme?: "light" | "dark";
};

export function PierreDiffView({
  oldCode,
  newCode,
  filename,
  layout = "stacked",
  theme = "dark",
}: Props) {
  const shikiTheme = theme === "light" ? "github-light" : "github-dark";

  // Parse the diff from the two file contents
  const diffMetadata = useMemo((): FileDiffMetadata => {
    return parseDiffFromFile(
      { name: filename, contents: oldCode },
      { name: filename, contents: newCode },
    );
  }, [oldCode, newCode, filename]);

  return (
    <div style={{ height: "100%", width: "100%", overflow: "auto" }}>
      <FileDiff
        fileDiff={diffMetadata}
        options={{
          diffStyle: layout === "split" ? "split" : "unified",
          theme: shikiTheme,
          expandUnchanged: false,
          collapsedContextThreshold: 3,
        }}
      />
    </div>
  );
}
