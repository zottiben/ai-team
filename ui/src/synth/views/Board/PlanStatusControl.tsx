import { useEffect, useLayoutEffect, useRef, useState } from "react";
import { PLAN_STATUSES, type PlanStatus } from "../../../plan-api";
import { statusLabel, type SavePlan } from "../../../PlanForms";
import { SynthProvider, useAnnounce } from "../../SynthProvider";
import { PresetSelector } from "../../primitives/PresetSelector";
import { displayed, useDisplayed } from "./owner";
import { ownedFocus } from "./focus";

const options = PLAN_STATUSES.map(id => ({ id, label: statusLabel(id) }));
type Props = { status: PlanStatus; revision: number; disabled: boolean; save: SavePlan };
export function PlanStatusControl(props: Props) { return <SynthProvider><StatusControl {...props}/></SynthProvider>; }
function StatusControl({ status, revision, disabled, save }: Props) {
  const host = useRef<HTMLDivElement>(null), active = useDisplayed(host), announce = useAnnounce();
  const sending = useRef(false), alive = useRef(true), [busy, setBusy] = useState(false);
  const focus = useRef<ReturnType<typeof ownedFocus> | null>(null), settled = useRef(false);
  useEffect(() => { alive.current = true; return () => { alive.current = false; focus.current?.cancel(); }; }, []);
  useLayoutEffect(() => {
    if (!active) { focus.current?.cancel(); settled.current = false; }
    else if (settled.current && !busy && !disabled) {
      settled.current = false; focus.current?.restore(() => host.current?.querySelector('button[aria-label="Plan status"]') ?? null);
    }
  }, [active, busy, disabled]);
  const change = async (value: string) => {
    const next = PLAN_STATUSES.find(status => status === value);
    if (!next || next === status || disabled || sending.current || !displayed(host.current)) return;
    focus.current?.cancel(); settled.current = false;
    const source = document.activeElement;
    focus.current = ownedFocus(node => node === source || (host.current?.contains(node) ?? false));
    sending.current = true; setBusy(true);
    let accepted = false;
    try { accepted = await save({ action: "set_plan_status", status: next }, revision); }
    catch (error) { console.warn("Plan status request was not confirmed.", error); }
    finally {
      sending.current = false;
      if (alive.current) {
        settled.current = displayed(host.current); setBusy(false);
        if (settled.current) announce(accepted ? "Plan status request recorded. Status only; not verification or execution approval." : "Plan status request was not confirmed. Review the current plan.");
        else focus.current?.cancel();
      }
    }
  };
  return <div ref={host} className="synth-plan-status">
    <span className="synth-silk" aria-hidden="true">Plan status</span>
    <PresetSelector label="Plan status" options={options} value={status} disabled={disabled || busy || !active} announceSelection={false} onChange={value => { void change(value); }}/>
  </div>;
}
