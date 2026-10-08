import { useEffect, useState } from "react";
import { WorkspaceSetup, type useWorkspaceSetup } from "./WorkspaceSetup";
import type { ChatDetail } from "./chat-api";
import type { WorkspaceChoice, WorkspaceRequest } from "./chat-workspace-api";

export default function ChatWorkspaceDetails({ workspace, detail, choices, target, problem, busy, disabled, pending, setup, onTarget, onClose, onSelect, onRequest, onApprove, onCancel }: {
  workspace: string | null;
  detail: ChatDetail | null;
  choices: WorkspaceChoice[];
  target: string;
  problem: string | null;
  busy: boolean;
  disabled: boolean;
  pending: WorkspaceRequest | null;
  setup: ReturnType<typeof useWorkspaceSetup>;
  onTarget: (path: string) => void;
  onClose: () => void;
  onSelect: (path: string) => void;
  onRequest: () => void;
  onApprove: () => void;
  onCancel: () => void;
}) {
  const [source, setSource] = useState<"existing" | "new">(setup.pending ? "new" : "existing");
  useEffect(() => { if (setup.pending) setSource("new"); }, [setup.pending]);
  const choice = choices.find(choice => choice.path === target);
  return <>
    <header className="chat-checkout__header">
      <div><h3>Chat checkout</h3><p className="faint">Choose where the next turn works.</p></div>
      <button type="button" className="button" aria-label="Close checkout" onClick={onClose}>Close</button>
    </header>
    {workspace && <div className="chat-checkout__current"><span className="faint">Current checkout</span><code>{workspace}</code></div>}
    <p className="chat-checkout__guidance">Files, editor buffers and terminals stay where they are. Selecting a checkout does not move or reset anything. A switch starts a fresh Pi session with this chat's history.</p>
    {problem && <p className="error" role="alert">{problem}</p>}
    {pending ? <section className="chat-checkout__confirmation" aria-label="Confirm checkout switch">
      <h4>Review the switch</h4>
      <dl><dt>From</dt><dd><code>{pending.from_path}</code></dd><dt>To</dt><dd><code>{pending.to_path}</code></dd></dl>
      {detail?.active_node_id != null && <p className="notice">Wait for this turn to settle, or stop it, before approving the switch.</p>}
      <div className="chat-checkout__actions">
        <button type="button" className="button button--primary" disabled={busy || disabled || !detail || detail.active_node_id !== null || detail.archived} onClick={onApprove}>Approve checkout switch</button>
        <button type="button" className="button" disabled={busy} onClick={onCancel}>Cancel checkout request</button>
      </div>
    </section> : <>
      {!detail && <div className="chat-checkout__tabs" role="group" aria-label="Checkout source">
        <button type="button" className="button" aria-pressed={source === "existing"} onClick={() => setSource("existing")}>Existing checkout</button>
        <button type="button" className="button" aria-pressed={source === "new"} onClick={() => setSource("new")}>Set up a new AWT worktree</button>
      </div>}
      <section className="chat-checkout__pane" hidden={!detail && source !== "existing"} aria-label="Existing checkout">
        <label className="chat-checkout__field">Worktree<select aria-label="Chat worktree" value={target} disabled={busy} onChange={event => onTarget(event.target.value)}>
          {!choices.length && <option value="">No worktrees loaded</option>}
          {choices.map(choice => <option key={choice.path} value={choice.path} disabled={!!choice.unavailable}>{choice.name}{choice.branch ? ` · ${choice.branch}` : ""}{choice.unavailable ? " · unavailable" : ""}</option>)}
        </select></label>
        {choice && <code className="chat-checkout__path">{choice.path}</code>}
        {!!choice?.processes?.length && <p className="notice">Processes detected: {[...new Set(choice.processes.map(process => process.name))].join(", ")}. Selecting this checkout leaves them running and does not reset files. Coordinate any concurrent edits.</p>}
        {choices.some(choice => choice.unavailable) && <details className="chat-checkout__explanation"><summary>Why some checkouts are unavailable</summary><ul>{choices.filter(choice => choice.unavailable).map(choice => <li key={choice.path}><strong>{choice.name} · {choice.branch}</strong>: {choice.unavailable}. Inspect the lease; do not return/reset it merely to select it.</li>)}</ul></details>}
        <div className="chat-checkout__actions"><button type="button" className="button button--primary" disabled={busy || disabled || !choice || !!choice.unavailable || choice.path === workspace} onClick={onRequest}>{detail ? "Review checkout switch" : "Use this checkout"}</button></div>
      </section>
      {!detail && <section className="chat-checkout__pane" hidden={source !== "new"} aria-label="New worktree"><WorkspaceSetup state={setup} disabled={disabled} onSelect={onSelect} /></section>}
    </>}
  </>;
}
