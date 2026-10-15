import { useLayoutEffect, useRef, useState, type RefObject } from "react";
import type { PlanStatus } from "../../../plan-api";
import { useAnnounce } from "../../SynthProvider";
import { statusDrag } from "./drag-engine";
import type { DropView, StatusDrop } from "./drop";

export default function Drag(props: {
  host: RefObject<HTMLDivElement | null>; view: DropView; expand: (status: PlanStatus) => void;
  propose: (drop: StatusDrop, handle: HTMLButtonElement, keyboard: boolean, announce: (message: string) => void, event: Event | null) => void; ready: (ready: boolean) => void;
}) {
  const latest = useRef(props); latest.current = props;
  const announce = useAnnounce(), [error, setError] = useState<string | null>(null);
  useLayoutEffect(() => {
    if (!props.host.current || props.view.disabled) { latest.current.ready(false); return; }
    setError(null);
    try {
      return statusDrag(props.host.current, {
        read: () => latest.current.view, announce, expand: status => latest.current.expand(status),
        propose: (drop, handle, keyboard, event) => latest.current.propose(drop, handle, keyboard, announce, event), ready: value => latest.current.ready(value),
      });
    } catch (error) { latest.current.ready(false); setError(String(error)); }
  }, [props.host, props.view.disabled, props.view.revision, announce]);
  return error ? <p role="alert" className="error">Status gestures unavailable: {error} Use the slice’s status form instead.</p> : null;
}
