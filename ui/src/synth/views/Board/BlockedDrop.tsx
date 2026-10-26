import { useEffect, useRef, useState } from "react";
import type { SavePlan } from "../../../PlanForms";
import type { StatusDrop } from "./drop";

/** A retained, fixed-target proposal. The board does not move before the service answers. */
export function BlockedDrop({ proposal: supplied, revision, disabled, save, close }: {
  proposal: StatusDrop; revision: number; disabled: boolean; save: SavePlan; close: (saved: boolean, event?: Event) => void;
}) {
  const [proposal] = useState(supplied), [reason, setReason] = useState("");
  const [busy, setBusy] = useState(false), [attempted, setAttempted] = useState(false), [error, setError] = useState<string | null>(null);
  const pending = useRef(false), alive = useRef(true);
  useEffect(() => { alive.current = true; return () => { alive.current = false; }; }, []);
  const stale = revision !== proposal.revision;
  return <form className="plan-form synth-blocked-drop" aria-label={`Blocking reason for ${proposal.key}`} onKeyDown={event => {
    if (event.key === "Escape" && !pending.current) { event.preventDefault(); close(false, event.nativeEvent); }
  }} onSubmit={async event => {
    event.preventDefault();
    if (pending.current || disabled || stale || !reason.trim() || proposal.to !== "blocked") return;
    pending.current = true; setBusy(true); setAttempted(true); setError(null);
    try {
      const ok = await save({ action: "set_slice_status", key: proposal.key, status: "blocked", reason }, proposal.revision);
      if (ok && alive.current) close(true);
    } catch (error) { if (alive.current) setError(String(error)); }
    finally { pending.current = false; if (alive.current) setBusy(false); }
  }}>
    <strong>Proposed blocked status · {proposal.key}</strong>
    <p className="faint">{busy ? "Sending this status change…" : attempted ? "The status change was not confirmed. Review the current plan before retrying; your reason is retained." : "No status change has been sent. Supply a reason or cancel."} Status only, not execution approval.</p>
    <label>Blocking reason<input value={reason} required maxLength={16000} readOnly={disabled || busy}
      onChange={event => { if (!pending.current && !disabled) setReason(event.target.value); }}/></label>
    {stale && <p className="notice">The plan changed after this drop. This draft remains bound to revision {proposal.revision}; cancel and choose again against the current plan.</p>}
    {disabled && <p className="notice">This plan cannot currently be edited. The captured reason remains here.</p>}
    {error && <p role="alert" className="error">{error}</p>}
    <div className="plan-actions">
      <button className="button" disabled={disabled || busy || stale || !reason.trim()}>Update status</button>
      <button className="button" type="button" disabled={busy} onClick={event => { if (!pending.current) close(false, event.nativeEvent); }}>Cancel status change</button>
    </div>
  </form>;
}
