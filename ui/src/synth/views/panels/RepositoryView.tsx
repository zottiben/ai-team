import { memo, useMemo, useRef, useState } from "react";
import type { RepoMap } from "../../../api";
import type { ChatSeat } from "../../../chat-overview-api";
import { repositoryActivity } from "../../data/wavetable";
import { Lcd } from "../../primitives/Lcd";
import { Wavetable } from "../../primitives/Wavetable";
import { HwButton } from "../../primitives/HwButton";

function RepositoryView({ map, seats }: { map: RepoMap; seats: ChatSeat[] | null }) {
  const [mode, setMode] = useState<"wave" | "xy" | "tree">("wave");
  const opened = useRef(new Set(["wave"]));
  opened.current.add(mode);
  const data = useMemo(() => seats === null ? null : repositoryActivity(map, seats), [map, seats]);
  const maximum = Math.max(1, ...(data?.points.map(point => Number.isFinite(point.hits) ? point.hits : 0) ?? []));
  return <div className="synth-repository-view">
    <div role="group" aria-label="Repository display" className="synth-repository-modes">{(["wave", "xy", "tree"] as const).map(value => <HwButton key={value} latched={mode === value} onClick={() => setMode(value)}>{value.toUpperCase()}</HwButton>)}</div>
    <div hidden={mode !== "wave"}><Wavetable directories={data?.directories ?? null} /></div>
    {opened.current.has("xy") && <Lcd hidden={mode !== "xy"} className="synth-repository-xy">
      <strong>Recorded file touches</strong>
      <svg viewBox="0 0 300 100" preserveAspectRatio="none" aria-hidden="true">
        <path className="synth-graph__baseline" d="M5 5V95H295" />
        {data?.points.map((point, index) => Number.isFinite(point.hits) ? <circle key={point.path}
          cx={5 + index / Math.max(1, data.points.length - 1) * 290} cy={95 - point.hits / maximum * 85} r={2} /> : null)}
      </svg>
      <span className="synth-sr-only">X: displayed file order. Y: recorded touches, 0–{maximum}. Not timing or throughput.</span>
      {data === null && <p>Activity unavailable</p>}
      <details><summary>Exact plotted values and ownership</summary><ul>{data?.points.map(point => <li key={point.path}>{point.path} · {Number.isFinite(point.hits) ? point.hits : "unknown"} touches · {point.owner ?? "unowned"}</li>)}</ul></details>
    </Lcd>}
    {opened.current.has("tree") && <Lcd hidden={mode !== "tree"} className="synth-repository-tree">
      <p>{map.nodes.length} displayed nodes · {map.files} walked files</p>
      <ul>{map.nodes.map(node => <li key={node.path} data-directory={node.dir || undefined} style={{ paddingInlineStart: `${Math.min(node.depth, 12) * 8}px` }}>
        <span>{node.dir ? "Directory" : "File"} · {node.path || "(root)"}</span><span>{node.owner ?? "unowned"} · weight {node.weight}</span>
      </li>)}</ul>
    </Lcd>}
    <span className="synth-muted" title="The map is bounded to 700 nodes and 50,000 walked files. Omitted touches are outside the displayed map.">{map.truncated ? "Truncated map" : ""}{data?.omitted ? ` · ${data.omitted} paths omitted` : ""}</span>
  </div>;
}

const touches = (seats: ChatSeat[] | null) => JSON.stringify(seats?.map(seat => seat.touches.map(touch => [touch.path, touch.reads, touch.writes])) ?? null);
// Counter-only SSE ticks must not reset the repository's tool-event debounce.
export default memo(RepositoryView, (before, after) => before.map === after.map && touches(before.seats) === touches(after.seats));
