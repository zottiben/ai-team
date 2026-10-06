import { useEffect, useRef, useState } from "react";

import { Popover } from "./Popover";
import { WorkspaceSetup, useWorkspaceSetup } from "./WorkspaceSetup";
import type { ChatDetail } from "./chat-api";
import { approveWorkspace, cancelWorkspace, chatWorkspaces, requestWorkspace, workspaceRequests, type WorkspaceChoice, type WorkspaceRequest } from "./chat-workspace-api";

export function ChatWorkspace({ project, detail, selected, tick, disabled, onSelect, onChanged, onPending }: {
  project: string;
  detail: ChatDetail | null;
  selected: string | null;
  tick: number;
  disabled: boolean;
  onSelect: (path: string) => void;
  onChanged: () => void;
  onPending: (pending: boolean) => void;
}) {
  const [open, setOpen] = useState(false);
  const [choices, setChoices] = useState<WorkspaceChoice[]>([]);
  const [pending, setPending] = useState<WorkspaceRequest | null>(null);
  const [target, setTarget] = useState("");
  const [problem, setProblem] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [refresh, setRefresh] = useState(0);
  const button = useRef<HTMLButtonElement | null>(null);
  const working = useRef(false);
  const notified = useRef<number | null>(null);
  const id = detail?.id;
  const setup = useWorkspaceSetup(project, id === undefined, tick);
  useEffect(() => { onPending(pending !== null || setup.pending); }, [pending, setup.pending, onPending]);
  const workspace = detail?.workspace_path ?? selected;
  useEffect(() => {
    if (!id) return;
    let current = true;
    void workspaceRequests(id).then(result => {
      if (!current) return;
      const next = result.requests.find(request => request.state === "pending") ?? null;
      setPending(next);
      if (next && notified.current !== next.id) { notified.current = next.id; setOpen(true); }
    }).catch((error: unknown) => { if (current) setProblem(error instanceof Error ? error.message : String(error)); });
    return () => { current = false; };
  }, [id, tick, refresh, onPending]);
  useEffect(() => {
    if (!open) return;
    let current = true;
    void chatWorkspaces(project).then(found => {
      if (!current) return;
      setChoices(found);
      setTarget(previous => previous || workspace || found[0]?.path || "");
    }).catch((error: unknown) => { if (current) setProblem(error instanceof Error ? error.message : String(error)); });
    return () => { current = false; };
  }, [open, project, workspace, refresh]);
  const act = async (action: () => Promise<unknown>) => {
    if (working.current) return;
    working.current = true;
    setBusy(true); setProblem(null);
    try { await action(); setRefresh(value => value + 1); onChanged(); }
    catch (error: unknown) { setProblem(error instanceof Error ? error.message : String(error)); }
    finally { working.current = false; setBusy(false); }
  };
  const choice = choices.find(choice => choice.path === target);
  return <>
    <button type="button" ref={button} className="button chat-checkout-picker" aria-label="Choose checkout" aria-expanded={open} title={workspace ?? "Choose a project checkout"} disabled={disabled && !pending} onClick={() => setOpen(!open)}>
      {setup.pending ? "Worktree setup in progress" : pending ? "Worktree change waiting" : workspace ? workspace.split("/").at(-1) : "Local"}
    </button>
    {open && <Popover anchor={button} label="Chat checkout" onClose={() => setOpen(false)} className="chat-checkout-popover">
      <strong>Chat checkout</strong>
      <p className="faint">Choose where the next turn works. Files, editor buffers and terminal sessions stay in their original checkout; nothing is moved, reset or replayed. Changing checkouts starts a fresh Pi session with this chat's history.</p>
      {workspace && <p className="mono">Current: {workspace}</p>}
      {problem && <p className="error" role="alert">{problem}</p>}
      {pending ? <>
        <p>Requested checkout: <strong className="mono">{pending.to_path}</strong></p>
        {detail?.active_node_id != null && <p className="faint">Wait for this turn to settle, or stop it, before approving the switch.</p>}
        <button type="button" className="button button--primary" disabled={busy || disabled || !detail || detail.active_node_id !== null || detail.archived} onClick={() => detail && void act(async () => { await approveWorkspace(detail.id, pending.id, detail.rev); setOpen(false); })}>Approve checkout switch</button>
        <button type="button" className="button" disabled={busy} onClick={() => detail && void act(() => cancelWorkspace(detail.id, pending.id))}>Cancel checkout request</button>
      </> : <>
        <label>Worktree<select aria-label="Chat worktree" value={target} disabled={busy} onChange={event => setTarget(event.target.value)}>
          {!choices.length && <option value="">No worktrees loaded</option>}
          {choices.map(choice => <option key={choice.path} value={choice.path} disabled={!!choice.unavailable}>{choice.name}{choice.branch ? ` · ${choice.branch}` : ""}{choice.unavailable ? " · unavailable" : ""}</option>)}
        </select></label>
        {choice && <p className="mono">{choice.path}</p>}
        {!!choice?.processes?.length && <p className="notice">Processes detected: {[...new Set(choice.processes.map(process => process.name))].join(", ")}. Selecting this checkout leaves them running and does not reset files. Coordinate any concurrent edits.</p>}
        {choices.some(choice => choice.unavailable) && <details><summary>Why some checkouts are unavailable</summary><ul>{choices.filter(choice => choice.unavailable).map(choice => <li key={choice.path}><strong>{choice.name} · {choice.branch}</strong>: {choice.unavailable}. Inspect the lease; do not return/reset it merely to select it.</li>)}</ul></details>}
        <button type="button" className="button" disabled={busy || disabled || !choice || !!choice.unavailable || choice.path === workspace} onClick={() => {
          if (detail) void act(() => requestWorkspace(detail.id, target));
          else { onSelect(target); setOpen(false); }
        }}>{detail ? "Review checkout switch" : "Use this checkout"}</button>
        {!detail && <WorkspaceSetup state={setup} disabled={disabled} onSelect={path => { onSelect(path); setOpen(false); }} />}
      </>}
    </Popover>}
  </>;
}
