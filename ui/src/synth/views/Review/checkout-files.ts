import { useMemo, useRef, useState } from "react";
import type { Comment, FileDiff } from "../../../api";
import type { FileViewState } from "../../../ReviewFile";
import type { CheckoutFinding, CheckoutState } from "../../../checkout-api";

type Area = CheckoutFinding["area"];
export type CheckoutFile = {
  key: string; area: Area; file: FileDiff; reason: string | null; revision: string;
  reviewKey: string; commentScope: string; epoch: number; workspace: string; fingerprint: string; head: string | null;
  comments: Comment[];
};
export function checkoutFiles(data: CheckoutState, chat: number): CheckoutFile[] {
  const make = (file: FileDiff, area: Area, reason: string | null = null, revision?: string): CheckoutFile => {
    const revisions = area === "staged" ? data.staged_revisions : data.unstaged_revisions;
    const exact = revision ?? (revisions && Object.hasOwn(revisions, file.path) ? revisions[file.path] : undefined);
    return { key: JSON.stringify([area, file.path]), area, file, reason, revision: typeof exact === "string" ? exact : data.fingerprint,
      reviewKey: JSON.stringify(["chat", chat, data.workspace, data.workspace_epoch, data.branch, area]),
      commentScope: JSON.stringify([chat, data.workspace, data.workspace_epoch, data.fingerprint]),
      workspace: data.workspace, epoch: data.workspace_epoch, fingerprint: data.fingerprint, head: data.head,
      comments: data.findings.filter(finding => finding.fingerprint === data.fingerprint && finding.area === area && finding.path === file.path)
        .map(finding => ({ id: finding.id, review_id: 0, parent_id: null, file_path: finding.path, side: finding.side,
          line_start: finding.line, line_end: finding.line, author: "you", body: finding.body, status: "open", created_at: finding.created_at })),
    };
  };
  return [...data.staged.map(file => make(file, "staged")), ...data.unstaged.map(file => make(file, "unstaged")),
    ...data.untracked_files.map(entry => make(entry.file, "untracked", entry.reason, entry.fingerprint))];
}
const identity = (entry: CheckoutFile) => JSON.stringify([entry.reviewKey, entry.revision]);
type DisplayState = FileViewState & { identity: string; revealed: boolean };

/** Keep a FileView in the same keyed section if its unsent draft outlives the diff.
 * This stores presentation snapshots only; a retained entry gets no current sender. */
export function useCheckoutFiles(data: CheckoutState | null, chat: number) {
  const files = useMemo(() => data ? checkoutFiles(data, chat) : [], [data, chat]);
  const retained = useRef(new Map<string, CheckoutFile>());
  const [states, setStates] = useState(new Map<string, DisplayState>());
  const current = new Set(files.map(file => file.key));
  const rows = [...files.map(entry => ({ entry, present: true })), ...[...retained.current.values()]
    .filter(entry => !current.has(entry.key)).map(entry => ({ entry, present: false }))];
  const report = (entry: CheckoutFile, state: FileViewState) => {
    if (state.draft) retained.current.set(entry.key, entry); else retained.current.delete(entry.key);
    const id = identity(entry);
    setStates(previous => {
      const old = previous.get(entry.key);
      if (old?.identity === id && old.draft === state.draft && old.viewed === state.viewed && old.expanded === state.expanded) return previous;
      const next = new Map(previous); next.set(entry.key, { ...state, identity: id, revealed: !!old?.revealed || state.expanded || state.draft });
      for (const key of next.keys()) if (!current.has(key) && !retained.current.has(key)) next.delete(key);
      return next;
    });
  };
  const viewed = new Set(files.filter(entry => entry.area === "unstaged" && states.get(entry.key)?.identity === identity(entry)
    && states.get(entry.key)?.viewed).map(entry => entry.file.path));
  return { rows, report, viewed, state: (entry: CheckoutFile) => states.get(entry.key) };
}
