import type { FileDiff } from "../../../api";
import type { CheckoutOperation, CheckoutState } from "../../../checkout-api";

export type HunkBars = { header: string; added: number; removed: number; bars: { added: number; removed: number }[] };
/** Four positional bins of actual displayed lines per hunk, not generated waveform data. */
export function hunkBars(file: FileDiff): HunkBars[] {
  return file.hunks.map(hunk => {
    const bars = Array.from({ length: 4 }, () => ({ added: 0, removed: 0 }));
    let added = 0, removed = 0;
    hunk.lines.forEach((line, index) => {
      const bar = bars[Math.min(3, Math.floor(index * 4 / hunk.lines.length))];
      if (!bar) return;
      if (line.kind === "added") { bar.added++; added++; }
      if (line.kind === "removed") { bar.removed++; removed++; }
    });
    return { header: hunk.header, added, removed, bars };
  });
}
export const barHeight = (lines: number, maximum = 24) => Math.min(maximum, lines * .9);

export function latestOperation(operations: readonly CheckoutOperation[], kinds: readonly CheckoutOperation["snapshot"]["action"]["kind"][]) {
  return operations.reduce<CheckoutOperation | null>((latest, operation) => kinds.includes(operation.snapshot.action.kind)
    && operation.state !== "preview" && (!latest || operation.id > latest.id) ? operation : latest, null);
}
export function receiptScope(operation: CheckoutOperation, state: CheckoutState): "current" | "prior" | "unknown" {
  if (operation.snapshot.workspace_epoch == null) return "unknown";
  return operation.snapshot.workspace === state.workspace && operation.snapshot.workspace_epoch === state.workspace_epoch ? "current" : "prior";
}
export function pushedCommit(operation: CheckoutOperation): string | null {
  return operation.snapshot.action.kind === "push_branch" ? operation.snapshot.action.commit
    : operation.snapshot.action.kind === "push" ? operation.snapshot.head : null;
}
