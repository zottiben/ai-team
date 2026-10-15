// Vite-only input prototype. No app bootstrap, storage, network API or execution.
import { Accessibility, DragDropManager, Draggable, Droppable, Feedback, KeyboardSensor, PointerActivationConstraints, PointerSensor } from "@dnd-kit/dom";
import { PLAN_STATUSES, type PlanStatus } from "../plan-api";
import "./tokens.css";
import "./fonts.css";
import "./board-prototype.css";

const root = document.getElementById("root");
if (!root) throw Error("Board prototype root missing");
document.documentElement.dataset.synth = "true";
document.documentElement.dataset.theme = new URLSearchParams(location.search).get("theme") === "light" ? "light" : "dark";
function element<K extends keyof HTMLElementTagNameMap>(tag: K, text = "", className = "") {
  const node = document.createElement(tag); node.textContent = text; node.className = className; return node;
}
root.append(element("h1", "100-card input prototype"), element("p", "Synthetic status changes only. No plan, API, model, persistence or execution. This is not the production Board."));
const instructions = element("p", "Drag the handle at least 4px, or use Space / left / right / Space. Escape or Tab cancels. Plan order stays fixed.");
instructions.id = "prototype-help"; root.append(instructions);
const output = element("p", "Ready", "board-prototype-output"); output.role = "status"; output.setAttribute("aria-live", "polite"); root.append(output);
const board = element("div", "", "board-prototype-lanes"); root.append(board);
const lanes = new Map<PlanStatus, HTMLElement>(), cards = new Map<string, HTMLElement>(), handles = new Map<string, HTMLButtonElement>();
const records = Array.from({ length: 100 }, (_, index) => ({ key: `S${index + 1}`, index, status: PLAN_STATUSES[index % PLAN_STATUSES.length]! }));
const signals: Record<PlanStatus, string> = { draft: "var(--slice-draft)", ready: "var(--slice-ready)", active: "var(--slice-active)", in_review: "var(--slice-review)", blocked: "var(--slice-blocked)", done: "var(--slice-done)", deferred: "var(--slice-deferred)" };
for (const status of PLAN_STATUSES) {
  const lane = element("section", "", "board-prototype-lane"); lane.dataset.lane = status;
  lane.style.setProperty("--synth-signal", signals[status]);
  lane.append(element("h2", status.replaceAll("_", " ")));
  const well = element("div", "", "board-prototype-well"); lane.append(well); board.append(lane); lanes.set(status, lane);
}
for (const record of records) {
  const card = element("article", "", "board-prototype-card"); card.dataset.card = record.key;
  const handle = element("button", `Move ${record.key}`); handle.type = "button"; handle.setAttribute("aria-describedby", instructions.id);
  const content = element("div", "", "board-prototype-content");
  content.append(handle, element("strong", `${record.key} · Synthetic retained card`), element("span", `Original plan position ${record.index + 1}`));
  card.append(content); lanes.get(record.status)!.lastElementChild!.append(card); cards.set(record.key, card); handles.set(record.key, handle);
}
let revision = 1, disabled = false, starts = 0, changes = 0, disposed = false;
function connect() {
  let closed = false;
  const ghost = element("div", "", "board-prototype-card board-prototype-ghost");
  ghost.setAttribute("aria-hidden", "true"); ghost.inert = true; ghost.style.visibility = "hidden"; document.body.append(ghost);
  const manager = new DragDropManager({
    plugins: defaults => [...defaults.filter(plugin => plugin !== Accessibility && plugin !== Feedback), Feedback.configure({ dropAnimation: null, keyboardTransition: null })],
    sensors: [PointerSensor.configure({ activationConstraints: [new PointerActivationConstraints.Distance({ value: 4 })] }), KeyboardSensor.configure({
      keyboardCodes: { start: ["Space"], end: ["Space"], cancel: ["Escape", "Tab"], left: ["ArrowLeft"], right: ["ArrowRight"], up: ["ArrowUp"], down: ["ArrowDown"] },
    })],
  });
  const feedback = manager.registry.plugins.get(Feedback);
  if (!feedback) throw Error("Feedback plugin missing");
  feedback.overlay = ghost;
  const droppables = [...lanes].map(([status, lane]) => new Droppable({ id: status, element: lane }, manager));
  const entities = [...records.map(record => new Draggable({ id: record.key, element: cards.get(record.key), handle: handles.get(record.key) }, manager)), ...droppables];
  let captured: { key: string; revision: number; status: PlanStatus; keyboard: boolean } | null = null;
  let keyboardLane: PlanStatus | null = null;
  const statusOf = (id: unknown) => PLAN_STATUSES.find(status => status === id);
  const within = (point: { x: number; y: number }, box: DOMRect) => point.x >= box.left && point.x < box.right && point.y >= box.top && point.y < box.bottom;
  const announce = (text: string) => { output.textContent = text; };
  const clearVisuals = () => { ghost.style.visibility = "hidden"; cards.forEach(card => card.removeAttribute("data-source")); lanes.forEach(lane => lane.removeAttribute("data-over")); };
  manager.monitor.addEventListener("beforedragstart", event => {
    const record = records.find(record => record.key === event.operation.source?.id), card = record && cards.get(record.key);
    if (disabled || disposed || closed || document.hidden || !board.getClientRects().length || !record || !card) { event.preventDefault(); return; }
    captured = { key: record.key, revision, status: record.status, keyboard: event.operation.activatorEvent?.type === "keydown" };
    keyboardLane = record.status;
    const copy = card.cloneNode(true);
    if (!(copy instanceof HTMLElement)) throw Error("Card clone is not an element");
    copy.querySelectorAll("[id]").forEach(node => node.removeAttribute("id"));
    copy.querySelectorAll<HTMLElement>("button,a,input,select,textarea,[tabindex]").forEach(node => { node.tabIndex = -1; node.removeAttribute("href"); });
    ghost.replaceChildren(...copy.childNodes); ghost.style.visibility = "visible";
  });
  manager.monitor.addEventListener("dragstart", () => {
    starts++; if (!captured) return;
    cards.get(captured.key)?.setAttribute("data-source", "true");
    announce(`${captured.key} picked up. Status only; saved order does not change.`);
  });
  manager.monitor.addEventListener("dragmove", event => {
    if (!captured?.keyboard || !event.by || !event.nativeEvent) return;
    event.preventDefault();
    const key = "key" in event.nativeEvent ? event.nativeEvent.key : null;
    const index = PLAN_STATUSES.indexOf(keyboardLane ?? captured.status);
    if (key !== "ArrowLeft" && key !== "ArrowRight") { announce("Within-column plan order is fixed; left and right choose status."); return; }
    keyboardLane = PLAN_STATUSES[Math.max(0, Math.min(PLAN_STATUSES.length - 1, index + (key === "ArrowLeft" ? -1 : 1)))]!;
    const lane = lanes.get(keyboardLane)!; lane.scrollIntoView({ block: "nearest", inline: "nearest" });
    droppables.forEach(droppable => droppable.refreshShape());
    const box = lane.getBoundingClientRect(), viewport = board.getBoundingClientRect();
    manager.actions.move({
      to: {
        x: Math.max(viewport.left + 16, Math.min(viewport.right - 16, box.left + box.width / 2)),
        y: Math.max(viewport.top + 60, Math.min(viewport.bottom - 20, event.operation.position.current.y))
      }, propagate: false
    });
  });
  manager.monitor.addEventListener("dragover", event => {
    lanes.forEach((lane, status) => { if (status === event.operation.target?.id) lane.dataset.over = "true"; else lane.removeAttribute("data-over"); });
    if (captured && event.operation.target) announce(`${captured.key} over ${event.operation.target.id}. Status only.`);
  });
  manager.monitor.addEventListener("dragend", event => {
    const intent = captured; captured = null; clearVisuals();
    const status = statusOf(event.operation.target?.id), point = event.operation.position.current;
    const explicitDrop = event.nativeEvent?.type === "pointerup" || (event.nativeEvent?.type === "keydown" && "code" in event.nativeEvent && event.nativeEvent.code === "Space");
    const accepted = intent && status && !event.canceled && explicitDrop && !disabled && !disposed && !closed && !document.hidden && revision === intent.revision
      && within(point, board.getBoundingClientRect()) && within(point, lanes.get(status)!.getBoundingClientRect());
    if (accepted && status !== intent.status) {
      const record = records.find(record => record.key === intent.key)!; record.status = status; revision++; changes++;
      const next = records.find(candidate => candidate.index > record.index && candidate.status === status);
      lanes.get(status)!.lastElementChild!.insertBefore(cards.get(record.key)!, next ? cards.get(next.key)! : null);
      announce(`${record.key} recorded as ${status}; original plan order retained. Fixture only.`);
    } else announce("Cancelled or unchanged. No fixture status changed.");
    if (intent?.keyboard && !disposed && !closed) handles.get(intent.key)?.focus({ preventScroll: true });
  });
  const cancelFocus = (event: FocusEvent) => { if (captured && event.target !== document.body && event.target !== handles.get(captured.key)) reset(); };
  document.addEventListener("focusin", cancelFocus);
  return {
    manager, get closed() { return closed; }, destroy() {
      if (closed) return; closed = true;
      manager.actions.stop({ canceled: true }); clearVisuals();
      entities.forEach(entity => entity.destroy()); manager.destroy(); ghost.remove();
      document.removeEventListener("focusin", cancelFocus);
    }
  };
}
let engine = connect();
// A scope cancellation owns the whole sensor lifetime. Stopping only the operation
// leaves KeyboardSensor's old document listener alive until another key is consumed.
function reset() { engine.destroy(); if (!disabled && !disposed && !document.hidden) engine = connect(); }
const visibility = () => reset();
const resized = () => { if (!engine.closed && !engine.manager.dragOperation.status.idle) reset(); };
document.addEventListener("visibilitychange", visibility); window.addEventListener("resize", resized);
export const snapshot = () => ({
  revision, starts, changes, disabled, disposed, dragging: !engine.closed && !engine.manager.dragOperation.status.idle,
  point: { ...engine.manager.dragOperation.position.current }, target: engine.manager.dragOperation.target?.id, cards: records.map(record => ({ ...record }))
});
export function freeze() { disabled = true; reset(); output.textContent = "Frozen fixture; no new status changes."; }
export function advanceRevision() { revision++; reset(); output.textContent = "Fixture revision changed; captured movement cancelled."; }
export function dispose() {
  if (disposed) return; disposed = true; engine.destroy();
  document.removeEventListener("visibilitychange", visibility); window.removeEventListener("resize", resized);
}
window.addEventListener("pagehide", dispose, { once: true });
