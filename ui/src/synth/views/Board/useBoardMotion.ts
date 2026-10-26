import { useLayoutEffect, useRef, useState, type RefObject } from "react";
import type { StatusDrop } from "./drop";
import { settleCard, type CardPose } from "./settle";

export function useBoardMotion(host: RefObject<HTMLDivElement | null>, enabled: boolean, revision: number) {
  const latest = useRef({ enabled, revision }); latest.current = { enabled, revision };
  const active = useRef<ReturnType<typeof settleCard>>(null), sequence = useRef(0);
  const [event, setEvent] = useState<{ status: StatusDrop["to"]; revision: number; id: number } | null>(null);
  const cancel = () => { active.current?.cancel(); active.current = null; };
  useLayoutEffect(() => cancel, [host, enabled, revision]);
  useLayoutEffect(() => { if (active.current && !active.current.current()) cancel(); });
  const settle = (drop: StatusDrop, pose: CardPose | null) => {
    cancel();
    if (!latest.current.enabled || !host.current) return;
    let result: ReturnType<typeof settleCard> = null;
    result = settleCard(host.current, drop.key, drop.to, pose, () => { if (active.current === result) active.current = null; });
    active.current = result;
  };
  const confirmed = (drop: StatusDrop) => {
    if (latest.current.enabled) setEvent({ status: drop.to, revision: latest.current.revision, id: ++sequence.current });
  };
  return { settle, confirmed, event: event?.revision === revision ? event : null };
}
