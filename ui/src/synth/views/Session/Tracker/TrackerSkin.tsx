import { HwButton } from "../../../primitives/HwButton";
import { SessionPattern } from "../SessionPattern";
import { LANES, LANE_NAMES } from "./ToolRow";
import type { ActivityLane, TrackerRow } from "./rows";
import "../../../tokens.css";
import "../../../primitives.css";
import "./tracker.css";

export default function TrackerSkin({ rows, pattern, lanes, onLane, onSelect }: {
  rows: TrackerRow[]; pattern: boolean; lanes: ReadonlySet<ActivityLane>; onLane: (lane: ActivityLane) => void; onSelect: (row: number, event: number) => void;
}) {
  return <div className="synth-tracker-heading">
    {pattern && <SessionPattern rows={rows} onSelect={onSelect}/>}
    <div className="synth-tracker-filters" role="group" aria-label="Activity lanes">
      <span className="synth-silk">Tracker</span>
      {LANES.map(lane => <HwButton key={lane} latched={lanes.has(lane)} onClick={() => onLane(lane)}>{LANE_NAMES[lane]}</HwButton>)}
      <small>Messages, notices and errors stay visible.</small>
    </div>
  </div>;
}
