import type { PatchModule } from "./evidence";
import { Led } from "../../primitives/Led";
import { Jack } from "../../primitives/Jack";

export const moduleLabel = (module: PatchModule) => module.kind === "draft" ? `${module.slice.slice_key} draft` : `${module.node.role} · node #${module.node.id}`;
export function SeatModule({ module, selected, onSelect }: { module: PatchModule; selected: boolean; onSelect: () => void }) {
  const draft = module.kind === "draft" ? module.slice : null;
  const agent = module.kind !== "draft" ? module : null;
  const working = agent?.seat?.live === true && agent.seat.supervised === true;
  return <article className="synth-patch-module" data-patch-id={module.id} data-selected={selected || undefined}>
    <div className="synth-patch-module__head"><span className="synth-silk">{module.kind}</span><Led state={working ? "pulse" : "on"} signal={working ? "working" : draft?.build_status === "verified" ? "done" : "info"}/></div>
    <button className="synth-patch-module__select" type="button" aria-pressed={selected} onClick={onSelect}>{moduleLabel(module)}</button>
    <p className="mono">{draft ? draft.build_status : agent?.node.status}</p>
    <p className="faint">{draft ? `Lease: ${draft.lease_state}` : `${agent?.node.provider}/${agent?.node.model}`}</p>
    <Jack label="Inspect evidence" aria-label={`Inspect ${moduleLabel(module)}`} signal={draft ? "diff" : "plan"} armed={selected} onClick={onSelect}/>
  </article>;
}
