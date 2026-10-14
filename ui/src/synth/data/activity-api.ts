/** Additive fields on the existing event feed. No new polling/controller or raw payload API. */
export type ActivityObservation = {
  /** One supervisor stream reader. Offsets from different readers cannot be compared. */
  clock_id: string;
  elapsed_ms: number;
  /** Host receive time, before ingestion batching; not a tool execution timestamp. */
  at: string;
};

export type BoundedActivityText = { text: string; truncated: boolean };

export type ToolActivity = {
  call_id: string | null;
  parent_call_id: string | null;
  name: string | null;
  target: (BoundedActivityText & { kind: "path" | "command" | "query" | "pattern" | "url" }) | null;
  outcome: "returned" | "failed" | "unknown" | null;
  /** Explicit Pi execute() timing only. Null is not zero. */
  duration_ms: number | null;
  /** Bounded completed shell output; not a live PTY or a command input surface. */
  output: BoundedActivityText | null;
  /** Reported native edit lines, not net checkout statistics or guessed write counts. */
  edits?: { added: number; removed: number; first_changed_line: number | null; source: "pi_edit_diff" } | null;
};
