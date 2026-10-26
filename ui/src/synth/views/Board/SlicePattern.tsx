import { useRef, useState } from "react";
import { PLAN_STATUSES, type PlanSlice } from "../../../plan-api";
import { statusLabel } from "../../../PlanForms";
import { signalStyle } from "../../primitives/signal";
import { statusSignals } from "./status";

export function SlicePattern({ slices, onReveal }: { slices: PlanSlice[]; onReveal: (key: string) => void }) {
  const [focused, setFocused] = useState<string | null>(null);
  const buttons = useRef(new Map<string, HTMLButtonElement>());
  const active = slices.some(slice => slice.key === focused) ? focused : slices[0]?.key;
  return <div className="synth-slice-pattern" role="group" aria-label="Slice pattern in plan order">
    <div className="synth-slice-pattern__legend">
      <strong className="synth-silk">Slice pattern</strong>
      {PLAN_STATUSES.map(status => <span key={status} style={signalStyle(statusSignals[status])}><i aria-hidden="true"/>{statusLabel(status)}</span>)}
    </div>
    <div className="synth-slice-pattern__scroll">
      {slices.map((slice, index) => <button key={slice.key} type="button" className="synth-slice-pad" data-status={slice.status}
        ref={node => { if (node) buttons.current.set(slice.key, node); else buttons.current.delete(slice.key); }}
        tabIndex={slice.key === active ? 0 : -1} onFocus={() => setFocused(slice.key)}
        style={signalStyle(statusSignals[slice.status])} title={`${slice.key} · ${slice.title} · ${statusLabel(slice.status)}`}
        aria-label={`Reveal ${slice.key} · ${statusLabel(slice.status)} · ${slice.title}`} onClick={() => onReveal(slice.key)}
        onKeyDown={event => {
          const next = event.key === "Home" ? 0 : event.key === "End" ? slices.length - 1 : event.key === "ArrowLeft" ? index - 1 : event.key === "ArrowRight" ? index + 1 : null;
          if (next === null) return;
          event.preventDefault();
          const target = slices[Math.max(0, Math.min(slices.length - 1, next))];
          if (target) buttons.current.get(target.key)?.focus();
        }}>
        <span className="synth-slice-pad__light" aria-hidden="true"/><span className="synth-slice-pad__key">{slice.key}</span>
      </button>)}
      {!slices.length && <span className="synth-muted">No slices recorded.</span>}
    </div>
  </div>;
}
