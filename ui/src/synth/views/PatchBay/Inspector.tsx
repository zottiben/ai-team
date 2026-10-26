import { useMemo } from "react";
import type { RunEvent } from "../../../api";
import { measurements } from "../../data/telemetry";
import AgentTelemetry from "../panels/AgentTelemetry";
import { Plate } from "../../primitives/Plate";
import { moduleLabel } from "./SeatModule";
import type { PatchModule } from "./evidence";

export function Inspector({ module, events }: { module: PatchModule | null; events: RunEvent[] }) {
  const node = module && module.kind !== "draft" ? module.node : null;
  const history = useMemo(() => node ? measurements(events, node.id) : [], [events, node?.id]);
  if (!module) return <Plate heading="Inspector" tone="lavender"><p className="synth-muted">Select a recorded module to inspect its evidence. A missing selection is not replaced by another node.</p></Plate>;
  return <Plate heading={moduleLabel(module)} tone="lavender" aria-label="Patch inspector">
    {module.kind === "draft" ? <dl>
      <dt>Build status</dt><dd>{module.slice.build_status}</dd><dt>Lease</dt><dd>{module.slice.lease_state}</dd>
      <dt>Checkout</dt><dd className="mono">{module.slice.worktree_path ?? "Unrecorded"}</dd>
      <dt>Branch</dt><dd className="mono">{module.slice.branch ?? "Unrecorded"}</dd>
      <dt>Candidate commit</dt><dd className="mono">{module.slice.candidate_sha ?? "Unrecorded"}</dd>
      <dt>Recorded commit</dt><dd className="mono">{module.slice.commit_sha ?? "Unrecorded"}</dd>
      <dt>Reason</dt><dd>{module.slice.reason ?? "None recorded"}</dd>
      <dt>Release started</dt><dd>{module.slice.release_started ? "Yes" : "No"}</dd>
    </dl> : <>
      <dl><dt>Stored node</dt><dd>#{module.node.id} · {module.node.status} · attempt {module.node.attempt}</dd>
        <dt>Recorded model</dt><dd>{module.node.provider}/{module.node.model}</dd>
        <dt>Slice</dt><dd>{module.node.slice_key ?? "Unrecorded"}</dd>
        <dt>Checkout</dt><dd className="mono">{module.node.worktree_path ?? "Unrecorded"}</dd>
        <dt>Branch</dt><dd className="mono">{module.node.branch ?? "Unrecorded"}</dd>
        <dt>Supervision</dt><dd>{module.seat?.supervised == null ? "Unknown" : module.seat.supervised ? "Process observed" : "No live process"}</dd>
        <dt>Blocked reason</dt><dd>{module.node.blocked_reason ?? "None recorded"}</dd>
      </dl>
      {module.seat && <><p>{module.seat.activity?.summary ?? "No recorded activity"}</p><p className="mono">{module.seat.activity?.detail}</p><p>{module.seat.said}</p></>}
      <AgentTelemetry history={history} node={module.node.id}/>
    </>}
    <p className="synth-muted">Recorded evidence, not a mutable workflow. A candidate or recorded commit does not establish publication or approval of the current checkout.</p>
  </Plate>;
}
