import { useEffect, useLayoutEffect, useRef, useState, type RefObject } from "react";
import type { SavePlan } from "../../../PlanForms";
import { statusLabel } from "../../../PlanForms";
import { currentDrop, statusPosition, type DropView, type StatusDrop } from "./drop";
import { ownedFocus } from "./focus";
import { displayed } from "./owner";

type Proposal = { id: number; drop: StatusDrop; focus: ReturnType<typeof ownedFocus>; announce: (message: string) => void };
export function useStatusDrops(view: DropView, active: boolean, save: SavePlan, host: RefObject<HTMLDivElement | null>, ready: boolean) {
  const latest = useRef({ view, active, save }); latest.current = { view, active, save };
  const [proposal, setProposal] = useState<Proposal | null>(null), [writing, setWriting] = useState(false);
  const pending = useRef<Proposal | null>(null), sending = useRef(false), sequence = useRef(0), alive = useRef(true);
  const form = useRef<HTMLDivElement>(null), focus = useRef<ReturnType<typeof ownedFocus> | null>(null);
  const restoration = useRef<Proposal | null>(null);
  const settlement = useRef<{ drop: StatusDrop; ok: boolean; focus: ReturnType<typeof ownedFocus>; announce: (message: string) => void; error?: string } | null>(null);
  useEffect(() => {
    alive.current = true;
    return () => { alive.current = false; focus.current?.cancel(); settlement.current = null; restoration.current = null; };
  }, []);
  useLayoutEffect(() => { if (!active) { focus.current?.cancel(); settlement.current = null; restoration.current = null; } }, [active]);
  useLayoutEffect(() => {
    if (proposal && latest.current.active) proposal.focus.take(form.current?.querySelector("input") ?? null);
  }, [proposal]);
  const handle = (key: string) => {
    const nodes = [...(host.current?.querySelectorAll<HTMLButtonElement>("[data-slice-drag],[data-slice-open]") ?? [])];
    return nodes.find(node => node.dataset.sliceDrag === key && !node.matches(":disabled"))
      ?? nodes.find(node => node.dataset.sliceOpen === key) ?? null;
  };
  const captureFocus = (drop: StatusDrop, event: Event | null) => {
    focus.current?.cancel();
    const inForm = (node: Node) => form.current?.contains(node) ?? false;
    const ticket = ownedFocus(node => inForm(node) || (node instanceof HTMLElement && (node.dataset.sliceDrag === drop.key || node.dataset.sliceOpen === drop.key) && !!host.current?.contains(node)), inForm, event);
    focus.current = ticket; return ticket;
  };
  // The accepted response and re-enabled handle must both have committed. A frame
  // queued directly from the promise can still see the previous disabled DOM.
  useLayoutEffect(() => {
    if (restoration.current && !writing && active && (ready || view.disabled)) {
      const previous = restoration.current; restoration.current = null;
      previous.focus.restore(() => handle(previous.drop.key));
    }
    const result = settlement.current;
    if (!result || writing || view.disabled || !ready || (result.ok && view.revision <= result.drop.revision)) return;
    settlement.current = null;
    if (!active || document.hidden) { result.focus.cancel(); return; }
    const source = view.slices.find(slice => slice.key === result.drop.key);
    result.announce(!result.ok ? `The status request was not confirmed. ${result.error ?? "Review the current plan."}`
      : source?.status === result.drop.to ? `${result.drop.key} recorded as ${statusLabel(result.drop.to)}, plan position ${statusPosition(view.slices, result.drop.key, result.drop.to)}. Status only.`
        : "Status request recorded; the plan has since changed. Review its current status.");
    result.focus.restore(() => handle(result.drop.key));
  });
  const close = (saved: boolean, event?: Event) => {
    const previous = pending.current; pending.current = null; setProposal(null);
    if (previous) {
      const returned = !saved && latest.current.active && displayed(host.current) ? { ...previous, focus: captureFocus(previous.drop, event ?? null) }
        : { ...previous, focus: focus.current ?? previous.focus };
      returned.focus.reveal(handle(returned.drop.key));
      if (settlement.current?.focus !== returned.focus) restoration.current = returned;
      if (!saved) previous.announce("Proposal closed. No additional status request was sent.");
    }
  };
  const propose = (drop: StatusDrop, _source: HTMLButtonElement, _keyboard: boolean, announce: (message: string) => void, event: Event | null) => {
    const current = latest.current;
    if (pending.current || sending.current || !current.active || !currentDrop(current.view, drop)) return;
    settlement.current = null; restoration.current = null;
    const ticket = captureFocus(drop, event);
    if (drop.to === "blocked") {
      const next = { id: ++sequence.current, drop, focus: ticket, announce }; pending.current = next; setProposal(next);
      announce(`${drop.key} needs a blocking reason. No status request has been sent.`);
      return;
    }
    sending.current = true; setWriting(true);
    void Promise.resolve().then(() => current.save({ action: "set_slice_status", key: drop.key, status: drop.to }, drop.revision)).then(ok => {
      if (alive.current && latest.current.active && !document.hidden) settlement.current = { drop, ok, focus: ticket, announce };
      else ticket.cancel();
    }).catch(error => {
      if (alive.current && latest.current.active && !document.hidden) settlement.current = { drop, ok: false, focus: ticket, announce, error: String(error) };
      else ticket.cancel();
    }).finally(() => { sending.current = false; if (alive.current) setWriting(false); });
  };
  const saveBlocked: SavePlan = (action, revision) => {
    const captured = pending.current, current = latest.current;
    if (!captured || !current.active || !displayed(host.current) || !currentDrop(current.view, captured.drop)
      || action.action !== "set_slice_status" || action.key !== captured.drop.key || action.status !== "blocked" || revision !== captured.drop.revision) return Promise.resolve(false);
    const ticket = captureFocus(captured.drop, null);
    return current.save(action, revision).then(ok => {
      if (ok && alive.current && pending.current === captured && latest.current.active && !document.hidden)
        settlement.current = { drop: captured.drop, ok, focus: ticket, announce: captured.announce };
      return ok;
    });
  };
  return { proposal, form, writing, propose, close, saveBlocked };
}
