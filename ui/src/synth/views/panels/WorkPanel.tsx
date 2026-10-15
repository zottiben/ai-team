import { useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
import type { RunEvent } from "../../../api";
import type { ChatDetail } from "../../../chat-api";
import { SynthProvider } from "../../SynthProvider";
import { useReadout } from "../../motion/readouts";
import { HwButton } from "../../primitives/HwButton";
import { Lcd } from "../../primitives/Lcd";
import { Plate } from "../../primitives/Plate";
import { Arrangement } from "../Work/Arrangement";
import { RunCounters } from "../Work/RunCounters";
import { RunPattern } from "../Work/RunPattern";
import { TurnsList } from "../Work/TurnsList";
import { runRecords } from "../Work/runs";
import { useWorkView, type WorkView } from "../Work/view";
import "../../tokens.css";
import "../../primitives.css";
import "../Work/work.css";

/** The distance from the bottom that still counts as following the live run. */
const FOLLOW = 80;

/**
 * The Work panel (§9): counters, run pattern, arrangement and turns.
 *
 * Decoration only. Every value comes from the chat detail the one chat controller
 * already loaded, no request is made here, and nothing in this view can start, stop or
 * approve work. Team execution keeps its own controls, rendered by `ChatContext` above
 * this panel so that switching skin never unmounts an open approval or inspection.
 */
export default function WorkPanel({ detail, events, now, view: suppliedView, visible = true }: {
  detail: ChatDetail; events: RunEvent[]; now: number; view?: WorkView; visible?: boolean;
}) {
  const ownedView = useWorkView();
  const view = suppliedView ?? ownedView;
  const host = useRef<HTMLDivElement>(null);
  const list = useRef<HTMLOListElement>(null);
  const records = useMemo(() => runRecords(detail), [detail]);
  const live = records.some(record => record.live);
  const [ticked, setTicked] = useState(now);
  // One shared 1Hz readout, and only while a run is genuinely live. An interrupted turn
  // has no process behind it, so nothing about it may keep moving.
  useReadout(host, () => setTicked(Date.now()), live);
  const clock = Math.max(ticked, now);

  // `selected` stays on the run a reader jumped to; `pulse` is the one 600ms ring.
  const [selected, setSelected] = useState<number | null>(null);
  const [pulse, setPulse] = useState<number | null>(null);
  const [jump, setJump] = useState(0);
  const [following, setFollowing] = useState(true);
  const pending = useRef<number | null>(null);
  const holding = useRef(false);
  const followedAt = useRef<number | null>(null);

  const reveal = useCallback((runId: number) => {
    holding.current = true;
    view.setFilter("all");
    pending.current = runId;
    setFollowing(false);
    setSelected(runId);
    setPulse(runId);
    setJump(count => count + 1);
  }, [view.setFilter]);
  const onReadIntent = useCallback(() => { holding.current = false; }, []);
  const onScroll = useCallback(() => {
    const rows = list.current;
    if (!rows || holding.current) return;
    setFollowing(rows.scrollHeight - rows.scrollTop - rows.clientHeight <= FOLLOW);
  }, []);

  useEffect(() => {
    if (pulse === null) return;
    const timer = setTimeout(() => setPulse(null), 600);
    return () => clearTimeout(timer);
  }, [pulse, jump]);

  useLayoutEffect(() => {
    const runId = pending.current, rows = list.current;
    if (runId === null || !rows || !visible) return;
    const target = rows.querySelector<HTMLElement>(`[data-run-id="${runId}"]`);
    if (!target) return;
    rows.scrollTop += target.getBoundingClientRect().top - rows.getBoundingClientRect().top;
    target.focus({ preventScroll: true });
    pending.current = null;
  }, [jump, records, view.filter, visible]);

  useLayoutEffect(() => {
    const rows = list.current;
    if (!rows || !visible || !following || !live) { followedAt.current = null; return; }
    if (followedAt.current !== null && rows.scrollTop < followedAt.current
      && rows.scrollHeight - rows.scrollTop - rows.clientHeight > FOLLOW) {
      setFollowing(false);
      followedAt.current = null;
      return;
    }
    rows.scrollTop = rows.scrollHeight;
    followedAt.current = rows.scrollTop;
  }, [following, live, records, view.filter, view.expanded, visible]);

  const recent = events.slice(-20);
  return <SynthProvider><div ref={host} className="synth-work">
    <Plate tone="night" className="synth-work__runs" aria-label="Runs"
      heading="Runs" actions={<span className="synth-mono">this conversation</span>}>
      <RunCounters records={records} />
      <RunPattern records={records} now={clock} selected={selected} onSelect={reveal} />
      <Arrangement records={records} events={events} now={clock} selected={selected} onSelect={reveal} />
    </Plate>
    <Plate className="synth-work__turns" aria-label="Turns" heading="Turns"
      actions={!following && live
        ? <HwButton onClick={() => { holding.current = false; setFollowing(true); }}>Jump to live run</HwButton>
        : <span className="synth-mono">{records.length} recorded</span>}>
      <TurnsList records={records} now={clock} view={view} pulse={pulse} listRef={list} onScroll={onScroll} onReadIntent={onReadIntent} />
    </Plate>
    <Plate className="synth-work__activity" aria-label="Recent activity" heading="Recent activity">
      {!recent.length ? <p className="synth-muted">No activity yet.</p> : <Lcd className="synth-work__events">
        <ol>{recent.map(event => <li key={event.id}>
          <span>{event.summary}</span>
          <span className="synth-mono">{event.at} · turn #{event.node_run_id}</span>
        </li>)}</ol>
      </Lcd>}
    </Plate>
  </div></SynthProvider>;
}
