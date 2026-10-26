import { useLayoutEffect, useRef, useState, type RefObject } from "react";
import { useReadout } from "./readouts";
import { useVisible } from "./useVisible";

/** Coalesce presentation only. Controllers retain every receipt. A new scope or newly
 * revealed view paints current data immediately; steady updates share the 1Hz clock. */
export function useReadoutSnapshot<T>(host: RefObject<Element | null>, value: T, scope: string, active: boolean): T {
  const visible = useVisible(host), latest = useRef(value);
  latest.current = value;
  const [sample, setSample] = useState({ scope, value });
  const prior = useRef({ scope, active, visible });
  useLayoutEffect(() => {
    const old = prior.current;
    if (old.scope !== scope || old.active !== active || visible && !old.visible) setSample({ scope, value: latest.current });
    prior.current = { scope, active, visible };
  }, [scope, active, visible]);
  useReadout(host, () => setSample({ scope, value: latest.current }), active && value !== sample.value);
  return !active || sample.scope !== scope ? value : sample.value;
}
