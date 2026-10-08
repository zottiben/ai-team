import { lazy, Suspense, useEffect, useRef, useState } from "react";

import { Popover } from "./Popover";
import { useWorkspaceSetup } from "./WorkspaceSetup";
import type { ChatDetail } from "./chat-api";
import { approveWorkspace, cancelWorkspace, chatWorkspaces, requestWorkspace, workspaceRequests, type WorkspaceChoice, type WorkspaceRequest } from "./chat-workspace-api";

const Details = lazy(() => import("./ChatWorkspaceDetails"));

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
  return <>
    <button type="button" ref={button} className="button chat-checkout-picker" aria-label="Choose checkout" aria-expanded={open} title={workspace ?? "Choose a project checkout"} disabled={disabled && !pending} onClick={() => setOpen(!open)}>
      {setup.pending ? "Worktree setup in progress" : pending ? "Worktree change waiting" : workspace ? workspace.split("/").at(-1) : "Local"}
    </button>
    {open && <Popover anchor={button} label="Chat checkout" width={640} onClose={() => setOpen(false)} className="chat-checkout-popover">
      <Suspense fallback={<p className="faint">Loading checkout choices…</p>}><Details
        workspace={workspace} detail={detail} choices={choices} target={target}
        problem={problem} busy={busy} disabled={disabled} pending={pending} setup={setup}
        onTarget={setTarget} onClose={() => { setOpen(false); button.current?.focus(); }}
        onSelect={path => { onSelect(path); setOpen(false); }}
        onRequest={() => {
          if (detail) void act(() => requestWorkspace(detail.id, target));
          else { onSelect(target); setOpen(false); }
        }}
        onApprove={() => detail && pending && void act(async () => { await approveWorkspace(detail.id, pending.id, detail.rev); setOpen(false); })}
        onCancel={() => detail && pending && void act(() => cancelWorkspace(detail.id, pending.id))}
      /></Suspense>
    </Popover>}
  </>;
}
