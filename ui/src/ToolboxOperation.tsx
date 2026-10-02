import { useEffect, useState } from "react";
import { Contents } from "./SetupContents";
import { applyOperation, getOperation, operationHistory, type Authority, type Operation, type OperationRecord } from "./toolbox-operations-api";

export function OperationPreview({ preview, onUpdate, onApplied, busy, setBusy }: { preview: Operation; onUpdate: (value: Operation) => void; onApplied?: () => void; busy: boolean; setBusy: (value: boolean) => void }) {
  const [approved, setApproved] = useState(false);
  const [problem, setProblem] = useState<string | null>(null);
  useEffect(() => { setApproved(false); setProblem(null); }, [preview.id, preview.authority.kind]);
  const execute = async () => {
    if (!approved || preview.state !== "preview") return;
    setApproved(false); setBusy(true); setProblem(null);
    try { const result = await applyOperation(preview.authority.kind, preview.id); onUpdate(result); if (result.state === "applied") onApplied?.(); }
    catch (e: unknown) {
      try { const result = await getOperation(preview.authority.kind, preview.id); onUpdate(result); if (result.state === "applied") onApplied?.(); setProblem(String(e)); }
      catch { onUpdate({ ...preview, state: "applying" }); setProblem(`${String(e)}. Status is unknown; inspect its saved status, never replay it.`); }
    }
    finally { setBusy(false); }
  };
  const authority = preview.authority;
  const count = preview.changes.reduce((n, c) => n + c.effects.length, 0) + preview.registrations.length + (preview.scan_roots === null ? 0 : 1);
  return <section aria-label="Scoped toolbox approval">
    <h4>{authority.kind.toUpperCase()} scope · {count} exact changes · {preview.state}</h4>
    {authority.kind === "user" && <p>User home: {authority.home}. Pi agent directory: {authority.pi_agent}. Explicit charter destinations (including any custom path outside HOME): {authority.charter_targets.join(", ") || "none"}. These changes can affect other projects too.</p>}
    {authority.kind === "converge" && <p>Read reference {authority.reference} → change only {authority.target}. Both checkouts are guarded against stale inputs.</p>}
    {preview.warnings.map((warning, i) => <p className="notice" key={i}>{warning}</p>)}
    <p className="faint">Before/after may contain existing private configuration. No files are changed until this saved scope is explicitly approved.</p>
    {preview.changes.map((change) => <div key={change.root}><h5>{change.root}</h5>{change.effects.map((effect) => <details key={effect.path}><summary>{effect.path} — {effect.summary}</summary><div className="toolbox__diff"><div><h5>Before</h5><Contents node={effect.before} /></div><div><h5>After</h5><Contents node={effect.after} /></div></div></details>)}</div>)}
    {preview.registrations.map((p) => <p key={p.project}>{p.name}: {p.status} → {p.next_status}. Keep all files and history. {p.roots.map((r) => `${r.path} (${r.exists ? "present" : "missing"})`).join(", ")}</p>)}
    {preview.scan_roots !== null && <p>Replace remembered discovery roots with: {preview.scan_roots.join(", ") || "none"}. This does not register or change any checkout.</p>}
    {count === 0 && <p>Nothing to change.</p>}
    {preview.state === "preview" && count > 0 && <><label><input type="checkbox" disabled={busy} checked={approved} onChange={(e) => setApproved(e.target.checked)} /> I approve this exact {authority.kind.toUpperCase()} scope</label><button className="button button--primary" disabled={busy || !approved} onClick={() => void execute()}>Apply scoped approval</button></>}
    {preview.state === "applying" && <><p className="error">Applying or interrupted. Inspect the files and saved receipt; this is not certified success and will not be retried automatically.</p><button className="button" disabled={busy} onClick={() => { setBusy(true); void getOperation(authority.kind, preview.id).then(onUpdate).catch((e: unknown) => setProblem(String(e))).finally(() => setBusy(false)); }}>Inspect scoped approval status</button></>}
    {preview.outcome && <p role="status">{preview.outcome.applied.length} recorded changes. {preview.outcome.problem ?? "Applied. Refresh the scan to inspect the result."}</p>}
    {problem && <p role="alert" className="error">{problem}</p>}
  </section>;
}

export function OperationHistory({ kind, refresh = 0 }: { kind: Authority["kind"]; refresh?: number }) {
  const [records, setRecords] = useState<OperationRecord[]>([]);
  const [selected, setSelected] = useState<Operation | null>(null);
  const [busy, setBusy] = useState(false);
  const [problem, setProblem] = useState<string | null>(null);
  useEffect(() => { let current = true; void operationHistory(kind).then((v) => { if (current) { setRecords(v); setProblem(null); } }).catch((e: unknown) => { if (current) setProblem(String(e)); }); return () => { current = false; }; }, [kind, refresh]);
  return <details><summary>Saved {kind} setup outcomes</summary>{problem && <p role="alert">{problem}</p>}{records.map((r) => <div key={r.id}><p>#{r.id} · {r.state}</p>{r.state === "applying" && <p className="error">Applying or interrupted. Further setup is blocked. Inspect this scope before further changes; no automatic retry.</p>}<p>{r.outcome?.problem}</p><button className="button" disabled={busy} onClick={() => { setBusy(true); void getOperation(kind, r.id).then(setSelected).catch((e: unknown) => setProblem(String(e))).finally(() => setBusy(false)); }}>Inspect scoped approval #{r.id}</button></div>)}{selected && <OperationPreview preview={selected} onUpdate={(next) => { setSelected(next); setRecords((rows) => rows.map((r) => r.id === next.id ? next : r)); }} busy={busy} setBusy={setBusy} />}</details>;
}
