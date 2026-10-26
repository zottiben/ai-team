import type { PatchModule } from "./evidence";
import { Led } from "../../primitives/Led";
import { Jack } from "../../primitives/Jack";

export const moduleLabel = (module: PatchModule) => module.kind === "draft" ? `${module.slice.slice_key} draft` : `${module.node.role} · node #${module.node.id}`;
export function SeatModule({ module, selected, onSelect }: { module: PatchModule; selected: boolean; onSelect: () => void }) {
  const draft = module.kind === "draft" ? module.slice : null;
  const agent = module.kind !== "draft" ? module : null;
  const working = agent?.seat?.live === true && agent.seat.supervised === true;
  return <article className="synth-plate synth-patch-module" data-tone={draft ? "night" : module.kind === "controller" ? "ice" : "lavender"} data-patch-id={module.id} data-selected={selected || undefined}>
    <div className="synth-patch-module__head"><span className="synth-silk">{module.kind}</span><Led state={working ? "pulse" : "on"} signal={working ? "working" : draft?.build_status === "verified" ? "done" : "info"}/></div>
    <button className="synth-patch-module__select" type="button" aria-pressed={selected} onClick={onSelect}>{moduleLabel(module)}</button>
    <div className="synth-lcd synth-patch-module__readout">
      <span className="synth-silk">Stored status</span>
      <p>{draft ? draft.build_status : agent?.node.status}</p>
      <p className="synth-muted">{draft ? `Lease: ${draft.lease_state}` : `${agent?.node.provider}/${agent?.node.model}`}</p>
    </div>
    <Jack label="Inspect evidence" aria-label={`Inspect ${moduleLabel(module)}`} signal={draft ? "diff" : "plan"} armed={selected} onClick={onSelect}/>
  </article>;
}
