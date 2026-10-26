import { Accessibility, Cursor, DragDropManager, Draggable, Droppable, Feedback, KeyboardSensor, PointerActivationConstraints, PointerSensor, PreventSelection } from "@dnd-kit/dom";
import { DOMRectangle } from "@dnd-kit/dom/utilities";
import { PLAN_STATUSES, type PlanStatus } from "../../../plan-api";
import { statusLabel } from "../../../PlanForms";
import { currentDrop, statusPosition, type DropView, type StatusDrop } from "./drop";
import { statusSignals } from "./status";
import { cardPose, type CardPose } from "./settle";

import { displayed } from "./owner";
const within = (point: { x: number; y: number }, box: DOMRect) => point.x >= box.left && point.x < box.right && point.y >= box.top && point.y < box.bottom;
const statusOf = (value: unknown) => PLAN_STATUSES.find(status => status === value);
type Engine = { destroy: () => void; readonly active: boolean };

/** DOM-only feedback. React/store state changes only when a valid gesture proposes a status. */
export function statusDrag(board: HTMLElement, callbacks: {
  read: () => DropView; expand: (status: PlanStatus) => void; announce: (message: string) => void;
  propose: (drop: StatusDrop, handle: HTMLButtonElement, keyboard: boolean, event: Event | null, pose: CardPose | null) => void;
  returnCard: (drop: StatusDrop, event: Event | null, pose: CardPose | null) => void; ready: (ready: boolean) => void;
}) {
  let disposed = false, engine: Engine | null = null;
  const reset = () => {
    engine?.destroy(); engine = null;
    if (!disposed && !callbacks.read().disabled && displayed(board)) engine = connect();
    callbacks.ready(engine !== null);
  };
  function connect(): Engine {
    let closed = false, frame: number | null = null;
    const lanes = new Map<PlanStatus, HTMLElement>();
    board.querySelectorAll<HTMLElement>(".plan-lane").forEach(lane => { const status = statusOf(lane.dataset.status); if (status) lanes.set(status, lane); });
    const cards = new Map<string, { card: HTMLElement; handle: HTMLButtonElement }>();
    board.querySelectorAll<HTMLElement>("[data-slice-key]").forEach(card => {
      const key = card.dataset.sliceKey, handle = card.querySelector<HTMLButtonElement>("[data-slice-drag]");
      if (key && handle?.dataset.sliceDrag === key && !cards.has(key)) cards.set(key, { card, handle });
    });
    const overlay = document.createElement("div"), insertion = document.createElement("div");
    overlay.className = "synth-plan-drag-overlay synth-plan-portal"; overlay.inert = true; overlay.setAttribute("aria-hidden", "true"); overlay.style.visibility = "hidden";
    insertion.className = "synth-plan-insertion"; insertion.setAttribute("aria-hidden", "true");
    document.body.append(overlay);
    let manager: DragDropManager;
    try { manager = new DragDropManager({
      // The app paints its inert overlay. These optional plugins inject page-wide
      // styles at pickup/drop; retain the toolkit's sensors, collision and scrolling.
      plugins: defaults => defaults.filter(plugin => plugin !== Accessibility && plugin !== Feedback && plugin !== Cursor && plugin !== PreventSelection),
      sensors: [PointerSensor.configure({ activationConstraints: [new PointerActivationConstraints.Distance({ value: 4 })] }), KeyboardSensor.configure({
        keyboardCodes: { start: ["Space"], end: ["Space"], cancel: ["Escape", "Tab"], left: ["ArrowLeft"], right: ["ArrowRight"], up: ["ArrowUp"], down: ["ArrowDown"] },
      })],
    }); } catch (error) { overlay.remove(); throw error; }
    const droppables: Droppable[] = [], entities: Draggable[] = [];
    try {
      lanes.forEach((element, id) => droppables.push(new Droppable({ id, element }, manager)));
      cards.forEach((value, id) => entities.push(new Draggable({ id, element: value.card, handle: value.handle }, manager)));
    } catch (error) {
      entities.forEach(entity => entity.destroy()); droppables.forEach(entity => entity.destroy()); manager.destroy(); overlay.remove(); throw error;
    }
    let captured: { key: string; from: PlanStatus; revision: number; keyboard: boolean; pointer: number | null; left: number; top: number; width: number; height: number } | null = null;
    let keyboardLane: PlanStatus | null = null, lastTarget: PlanStatus | null = null;
    const clear = () => {
      if (frame !== null) cancelAnimationFrame(frame); frame = null;
      overlay.style.visibility = "hidden"; insertion.remove(); board.removeAttribute("data-dragging");
      cards.forEach(({ card }) => card.removeAttribute("data-source")); lanes.forEach(lane => lane.removeAttribute("data-over")); lastTarget = null;
    };
    const keyboardPosition = () => {
      if (!captured?.keyboard || !keyboardLane) return;
      const lane = lanes.get(keyboardLane); if (!lane) return;
      lane.scrollIntoView({ block: "nearest", inline: "nearest" });
      droppables.forEach(target => target.refreshShape());
      const box = lane.getBoundingClientRect(), viewport = board.getBoundingClientRect();
      const point = {
        x: Math.max(Math.max(0, viewport.left) + 8, Math.min(Math.min(innerWidth, viewport.right) - 8, box.left + box.width / 2)),
        y: Math.max(Math.max(0, viewport.top) + 12, Math.min(Math.min(innerHeight, viewport.bottom) - 12, manager.dragOperation.position.current.y)),
      };
      const current = manager.dragOperation.position.current;
      if (Math.abs(current.x - point.x) > .1 || Math.abs(current.y - point.y) > .1) manager.actions.move({ to: point, propagate: false });
      // Scrolling can put another lane under the exact same viewport coordinate.
      // 0.5.1 otherwise suppresses collision updates at unchanged coordinates.
      manager.collisionObserver.forceUpdate();
    };
    const paint = () => {
      if (!captured) return;
      const { x, y } = manager.dragOperation.position.delta;
      const left = captured.left + x, top = captured.top + y;
      overlay.style.transform = `translate3d(${left}px, ${top}px, 0)`;
      const shape = DOMRectangle.from(new DOMRect(left, top, captured.width, captured.height));
      if (!manager.dragOperation.shape?.current.equals(shape)) manager.dragOperation.shape = shape;
    };
    const geometry = () => {
      frame = null;
      if (closed || !captured) return;
      if (document.hidden || !board.getClientRects().length) { reset(); return; }
      if (captured.keyboard) keyboardPosition();
      else { droppables.forEach(target => target.refreshShape()); manager.collisionObserver.forceUpdate(); }
      paint();
    };
    const schedule = () => { if (!closed && !disposed && captured && frame === null) frame = requestAnimationFrame(geometry); };
    const geometryChanges = new ResizeObserver(schedule);
    lanes.forEach(lane => geometryChanges.observe(lane));
    const scrolled = (event: Event) => {
      const target = event.target;
      if (target === document || target instanceof Node && (board.contains(target) || target.contains(board))) schedule();
    };
    document.addEventListener("scroll", scrolled, { capture: true, passive: true });
    manager.monitor.addEventListener("beforedragstart", event => {
      const view = callbacks.read(), matches = view.slices.filter(slice => slice.key === event.operation.source?.id), source = matches[0];
      const node = source && cards.get(source.key), input = event.operation.activatorEvent, keyboard = input?.type === "keydown";
      const pointer = input && "pointerId" in input && typeof input.pointerId === "number" ? input.pointerId : null;
      if (closed || disposed || view.disabled || !Number.isSafeInteger(view.revision) || view.revision < 0 || matches.length !== 1 || !source || !node || node.handle.matches(":disabled") || !displayed(node.card) || (!keyboard && pointer === null)) { event.preventDefault(); return; }
      const box = node.card.getBoundingClientRect();
      captured = { key: source.key, from: source.status, revision: view.revision, keyboard, pointer, left: box.left, top: box.top, width: box.width, height: box.height }; keyboardLane = source.status;
      const copy = node.card.cloneNode(true);
      if (!(copy instanceof HTMLElement)) { event.preventDefault(); captured = null; return; }
      copy.removeAttribute("id"); copy.removeAttribute("data-revealed"); copy.removeAttribute("data-slice-key"); copy.classList.add("synth-plan-ghost");
      copy.querySelectorAll("[id]").forEach(node => node.removeAttribute("id"));
      copy.querySelectorAll<HTMLElement>("button,a,input,select,textarea,[tabindex]").forEach(node => { node.tabIndex = -1; node.removeAttribute("href"); });
      copy.querySelectorAll<HTMLElement>("[data-motion]").forEach(node => { node.dataset.motion = "false"; });
      overlay.style.setProperty("--synth-signal", `var(--${statusSignals[source.status]})`);
      overlay.style.width = `${box.width}px`; overlay.style.height = `${box.height}px`;
      overlay.replaceChildren(copy); paint(); overlay.style.visibility = "visible";
    });
    manager.monitor.addEventListener("dragstart", () => {
      if (!captured) return;
      cards.get(captured.key)?.card.setAttribute("data-source", "true"); board.dataset.dragging = "true";
      callbacks.announce(`${captured.key} picked up. Left and right choose a status; plan order remains fixed.`);
      schedule();
    });
    manager.monitor.addEventListener("dragmove", event => {
      schedule();
      if (!captured?.keyboard || !event.by || !event.nativeEvent) return;
      event.preventDefault();
      const key = "key" in event.nativeEvent ? event.nativeEvent.key : null;
      if (key !== "ArrowLeft" && key !== "ArrowRight") { callbacks.announce("Within-column order stays fixed. Left and right choose a status."); return; }
      const index = PLAN_STATUSES.indexOf(keyboardLane ?? captured.from);
      keyboardLane = PLAN_STATUSES[Math.max(0, Math.min(PLAN_STATUSES.length - 1, index + (key === "ArrowLeft" ? -1 : 1)))]!;
      callbacks.expand(keyboardLane); keyboardPosition();
    });
    manager.monitor.addEventListener("dragover", event => {
      const target = statusOf(event.operation.target?.id);
      lanes.forEach((lane, status) => { if (status === target) lane.dataset.over = "true"; else lane.removeAttribute("data-over"); });
      if (!target) { insertion.remove(); lastTarget = null; return; }
      if (!captured || lastTarget === target) return;
      lastTarget = target; insertion.remove(); callbacks.expand(target);
      const view = callbacks.read(), index = view.slices.findIndex(slice => slice.key === captured?.key);
      const next = view.slices.slice(index + 1).find(slice => slice.status === target);
      const well = lanes.get(target)?.querySelector(".plan-lane-cards");
      if (well && target !== captured.from) well.insertBefore(insertion, next ? cards.get(next.key)?.card ?? null : null);
      callbacks.announce(`${captured.key}: ${statusLabel(target)}, plan position ${statusPosition(view.slices, captured.key, target) ?? "unknown"}. Status only.`);
    });
    manager.monitor.addEventListener("dragend", event => {
      const pose = captured ? cardPose(board, overlay) : null;
      const intent = captured; captured = null; clear();
      if (!intent || closed || disposed) return;
      // Finish with a fresh sensor lifetime even when no React revision changes
      // (same-lane drops/cancellation). Old deferred cleanup cannot eat pickup.
      queueMicrotask(() => { if (!closed && !disposed) reset(); });
      const target = statusOf(event.operation.target?.id), native = event.nativeEvent;
      const explicit = intent.keyboard ? native?.type === "keydown" && "code" in native && native.code === "Space"
        : native?.type === "pointerup" && "pointerId" in native && native.pointerId === intent.pointer;
      const point = native && "clientX" in native && "clientY" in native && typeof native.clientX === "number" && typeof native.clientY === "number"
        ? { x: native.clientX, y: native.clientY } : event.operation.position.current;
      const lane = target && lanes.get(target), source = cards.get(intent.key);
      const drop = target && { key: intent.key, from: intent.from, to: target, revision: intent.revision };
      if (!event.canceled && explicit && lane && source && drop && !source.handle.matches(":disabled") && displayed(source.card) && displayed(board) && within(point, board.getBoundingClientRect())
        && within(point, lane.getBoundingClientRect()) && within(point, new DOMRect(0, 0, innerWidth, innerHeight)) && currentDrop(callbacks.read(), drop)) {
        callbacks.propose(drop, source.handle, intent.keyboard, native ?? null, pose);
      } else {
        callbacks.announce("No status request was sent by this gesture.");
        if (native && (explicit || (native.type === "keydown" && "code" in native && native.code === "Escape")))
          callbacks.returnCard({ key: intent.key, from: intent.from, to: intent.from, revision: intent.revision }, native, pose);
      }
    });
    const focus = (event: FocusEvent) => { if (captured && event.target !== document.body && event.target !== cards.get(captured.key)?.handle) reset(); };
    document.addEventListener("focusin", focus);
    board.dataset.statusDrag = "ready";
    return {
      get active() { return captured !== null; },
      destroy() {
        if (closed) return; closed = true;
        const active = captured !== null; captured = null;
        manager.actions.stop({ canceled: true }); clear();
        entities.forEach(entity => entity.destroy()); droppables.forEach(entity => entity.destroy()); manager.destroy(); overlay.remove();
        document.removeEventListener("focusin", focus); document.removeEventListener("scroll", scrolled, true); geometryChanges.disconnect(); board.removeAttribute("data-status-drag");
        if (active && displayed(board)) callbacks.announce("Gesture cancelled. No status request was sent.");
      },
    };
  }
  const visibility = () => reset(), resize = () => { if (engine?.active) reset(); };
  const pointer = () => { if (engine?.active) reset(); };
  const context = (event: Event) => { if (engine?.active || (event.target instanceof Node && board.contains(event.target))) reset(); };
  let width = board.clientWidth;
  const dimensions = new ResizeObserver(() => {
    const next = board.clientWidth;
    if (next !== width && engine?.active) reset();
    width = next;
  });
  dimensions.observe(board);
  const observer = new MutationObserver(() => {
    if (!displayed(board)) { engine?.destroy(); engine = null; callbacks.ready(false); }
    else if (!engine && !callbacks.read().disabled) reset();
  });
  for (let node: HTMLElement | null = board; node; node = node.parentElement) observer.observe(node, { attributes: true, attributeFilter: ["hidden", "style", "class", "open"] });
  document.addEventListener("visibilitychange", visibility); document.addEventListener("pointerdown", pointer); document.addEventListener("contextmenu", context);
  window.addEventListener("resize", resize); window.addEventListener("blur", resize);
  const dispose = () => {
    disposed = true; engine?.destroy(); engine = null; observer.disconnect(); dimensions.disconnect(); callbacks.ready(false);
    document.removeEventListener("visibilitychange", visibility); document.removeEventListener("pointerdown", pointer); document.removeEventListener("contextmenu", context);
    window.removeEventListener("resize", resize); window.removeEventListener("blur", resize);
  };
  try { reset(); } catch (error) { dispose(); throw error; }
  return dispose;
}
