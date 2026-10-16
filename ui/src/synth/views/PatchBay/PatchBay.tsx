import { useMemo, useState } from "react";
import type { ChatDetail } from "../../../chat-api";
import type { ChatOverview } from "../../../chat-overview-api";
import type { RunEvent } from "../../../api";
import { Plate } from "../../primitives/Plate";
import { HwButton } from "../../primitives/HwButton";
import { patchEvidence, recordedExecutions } from "./evidence";
import { PatchCanvas } from "./PatchCanvas";
import { Inspector } from "./Inspector";
import { moduleLabel } from "./SeatModule";
import "./patch-bay.css";

type Props = { detail: ChatDetail; live: ChatOverview | null; events: RunEvent[]; onWork?: () => void; onReview?: () => void };
export default function PatchBay(props: Props) {
  return <EvidenceBay key={JSON.stringify([props.detail.id, props.detail.workspace_path, props.detail.workspace_epoch, props.detail.mode])} {...props}/>;
}
function EvidenceBay({ detail, live, events, onWork, onReview }: Props) {
  const [run, setRun] = useState<number | undefined>(), [selected, setSelected] = useState<string | null>(null), [zoom, setZoom] = useState(1);
  const graph = useMemo(() => patchEvidence(detail, live, run), [detail, live, run]);
  const turns = useMemo(() => recordedExecutions(detail), [detail]);
  const inspected = graph.modules.find(module => module.id === selected) ?? null;
  const label = (id: string) => { const module = graph.modules.find(module => module.id === id); return module ? moduleLabel(module) : "Unavailable module"; };
  return <div className="synth-patch-bay" aria-label="Team evidence patch bay">
    <Plate tone="night" heading="Team patch bay" actions={<span className="synth-silk">Read-only evidence</span>}>
      <div className="synth-patch-toolbar">
        <label>Recorded execution<select value={run ?? "latest"} onChange={event => { setRun(event.target.value === "latest" ? undefined : Number(event.target.value)); setSelected(null); }}>
          <option value="latest">Current / latest recorded</option>{turns.map(turn => <option key={turn.run.id} value={turn.run.id}>Run #{turn.run.id} · {turn.team?.phase}</option>)}
          {run !== undefined && !turns.some(turn => turn.run.id === run) && <option value={run}>Run #{run} · unavailable</option>}
        </select></label>
        <label>Canvas width<select value={zoom} onChange={event => setZoom(Number(event.target.value))}><option value={.8}>80%</option><option value={1}>100%</option><option value={1.25}>125%</option><option value={1.5}>150%</option></select></label>
        {onWork && <HwButton onClick={onWork}>Execution controls in Work</HwButton>}{onReview && <HwButton onClick={onReview}>Drafts &amp; delivery in Review</HwButton>}
      </div>
      {graph.execution && <p>Run #{graph.execution.run_id} · {graph.execution.phase} · approved plan revision {graph.execution.approved_revision ?? "unrecorded"} · {graph.execution.quiescent ? "quiescent" : "not quiescent"}</p>}
      <p className="faint">Cables show recorded membership, draft ownership and slice assignments—not dependencies, execution order or publication. Models and run settings are recorded facts. Approve, stop, recover or continue only through Work’s existing exact controls; delivery remains separate in Review.</p>
      {graph.execution?.reason && <p className="notice">{graph.execution.reason}</p>}
      {graph.issues.map(issue => <p className="notice" key={issue}>{issue}</p>)}
      {!!graph.omitted && <p className="faint">{graph.omitted} records omitted by identity checks or the 32-member / 16-draft display limits. Full history remains in Work and Review.</p>}
      {graph.modules.length > 0 && <PatchCanvas graph={graph} selected={selected} onSelect={setSelected} zoom={zoom}/>}
      <details><summary>Recorded cable relationships ({graph.links.length})</summary><ul>{graph.links.map(link => <li key={JSON.stringify(link)}>{label(link.from)} → {label(link.to)} · {link.kind.replaceAll("_", " ")}</li>)}</ul></details>
    </Plate>
    <Inspector module={inspected} events={events}/>
  </div>;
}
