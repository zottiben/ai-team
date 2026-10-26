import type { RefObject } from "react";
import type { RunEvent } from "../../../../api";
import { useReceiptMotion } from "../useReceiptMotion";
import type { ActiveScope } from "../activity-motion";
import { HwButton } from "../../../primitives/HwButton";
import { SessionPattern } from "../SessionPattern";
import { LANES, LANE_NAMES } from "./ToolRow";
import type { ActivityLane, TrackerRow } from "./rows";
import "../../../tokens.css";
import "../../../primitives.css";
import "./tracker.css";

export default function TrackerSkin({ rows, pattern, active, list, viewport, visible, eventKey, ready, lanes, onLane, onSelect }: {
  rows: TrackerRow[]; pattern: boolean; active: ActiveScope | null; list: RefObject<HTMLDivElement | null>; viewport: RefObject<HTMLDivElement | null>; visible: boolean; ready: boolean; eventKey: (event: RunEvent) => string | number;
  lanes: ReadonlySet<ActivityLane>; onLane: (lane: ActivityLane) => void; onSelect: (row: number, event: number) => void;
}) {
  const motion = useReceiptMotion(rows, list, viewport, visible, eventKey, ready);
  return <div className="synth-tracker-heading" data-motion={motion}>
    {pattern && <SessionPattern rows={rows} active={active} ready={ready} onSelect={onSelect}/>}
    <div className="synth-tracker-filters" role="group" aria-label="Activity lanes">
      <span className="synth-silk">Tracker</span>
      {LANES.map(lane => <HwButton key={lane} latched={lanes.has(lane)} onClick={() => onLane(lane)}>{LANE_NAMES[lane]}</HwButton>)}
    </div>
  </div>;
}
