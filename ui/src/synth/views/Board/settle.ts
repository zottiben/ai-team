import type { PlanStatus } from "../../../plan-api";
import { displayed } from "./owner";
import { statusSignals } from "./status";

type Box = { left: number; top: number; width: number; height: number };
export type CardPose = { box: Box; board: Box; width: number; height: number };
const box = (node: HTMLElement): Box => { const { left, top, width, height } = node.getBoundingClientRect(); return { left, top, width, height }; };
const valid = (value: Box) => Object.values(value).every(Number.isFinite) && value.width > 0 && value.height > 0;
const same = (a: Box, b: Box) => Math.abs(a.left - b.left) < 2 && Math.abs(a.top - b.top) < 2 && Math.abs(a.width - b.width) < 2 && Math.abs(a.height - b.height) < 2;
const reduced = () => matchMedia("(prefers-reduced-motion: reduce)").matches;
let sequence = 0;

/** Ephemeral viewport geometry, never part of a planning command or receipt. */
export function cardPose(board: HTMLElement, node: HTMLElement | null): CardPose | null {
  if (!node || board.dataset.motion !== "true" || !displayed(board) || reduced()) return null;
  const value = { box: box(node), board: box(board), width: innerWidth, height: innerHeight };
  return valid(value.box) && valid(value.board) ? value : null;
}

/** A finite portal FLIP; the real card remains the sole focus/semantic owner. */
export function settleCard(board: HTMLElement, key: string, status: PlanStatus, pose: CardPose | null, ended?: () => void) {
  if (!pose || pose.width !== innerWidth || pose.height !== innerHeight
    || board.dataset.motion !== "true" || reduced() || !displayed(board) || typeof Element.prototype.animate !== "function") return null;
  const frame = box(board);
  if (Math.abs(frame.left - pose.board.left) >= 2 || Math.abs(frame.top - pose.board.top) >= 2 || Math.abs(frame.width - pose.board.width) >= 2) return null;
  const matches = [...board.querySelectorAll<HTMLElement>("[data-slice-key]")].filter(node => node.dataset.sliceKey === key);
  const card = matches[0];
  if (matches.length !== 1 || !card || !displayed(card) || card.closest<HTMLElement>(".plan-lane")?.dataset.status !== status) return null;
  const token = String(++sequence);
  // An explicit settle supersedes a still-expanding lane; never fly toward a
  // moving/clipped target or move the semantic/focus owner into the portal.
  board.dataset.flipping = token;
  const focused = document.activeElement;
  if (focused instanceof HTMLElement && card.contains(focused)) focused.scrollIntoView({ block: "nearest", inline: "nearest" });
  const target = box(card);
  if (!valid(target) || target.left >= innerWidth || target.top >= innerHeight || target.left + target.width <= 0 || target.top + target.height <= 0) { delete board.dataset.flipping; return null; }
  const layer = document.createElement("div"), copy = card.cloneNode(true);
  if (!(copy instanceof HTMLElement)) { delete board.dataset.flipping; return null; }
  layer.className = "synth-plan-settle-layer synth-plan-portal"; layer.inert = true; layer.setAttribute("aria-hidden", "true");
  layer.style.setProperty("--synth-signal", `var(--${statusSignals[status]})`);
  copy.classList.add("synth-plan-settle-overlay"); copy.inert = true; copy.setAttribute("aria-hidden", "true");
  for (const attribute of ["id", "data-source", "data-revealed", "data-slice-key", "data-settling"]) copy.removeAttribute(attribute);
  copy.querySelectorAll("[id]").forEach(node => node.removeAttribute("id"));
  copy.querySelectorAll<HTMLElement>("button,a,input,select,textarea,[tabindex]").forEach(node => { node.tabIndex = -1; node.removeAttribute("href"); });
  copy.querySelectorAll<HTMLElement>("[data-motion]").forEach(node => { node.dataset.motion = "false"; });
  Object.assign(copy.style, { left: `${target.left}px`, top: `${target.top}px`, width: `${target.width}px`, height: `${target.height}px` });
  layer.append(copy); document.body.append(layer); card.dataset.settling = token;
  let closed = false, animation: Animation | null = null;
  const media = matchMedia("(prefers-reduced-motion: reduce)");
  const current = () => !document.hidden && !media.matches && displayed(board) && displayed(card) && card.dataset.sliceKey === key && card.closest<HTMLElement>(".plan-lane")?.dataset.status === status && same(box(card), target);
  const changed = () => { if (!current()) cancel(); };
  const cancel = () => {
    if (closed) return; closed = true;
    if (animation) { animation.onfinish = null; animation.oncancel = null; animation.cancel(); }
    layer.remove();
    if (card.dataset.settling === token) delete card.dataset.settling;
    if (board.dataset.flipping === token) delete board.dataset.flipping;
    dimensions.disconnect(); media.removeEventListener("change", cancel);
    document.removeEventListener("scroll", changed, true); document.removeEventListener("visibilitychange", changed);
    document.removeEventListener("pointerdown", cancel, true); document.removeEventListener("keydown", cancel, true);
    window.removeEventListener("resize", cancel); window.removeEventListener("blur", cancel); ended?.();
  };
  const dimensions = new ResizeObserver(changed); dimensions.observe(card);
  document.addEventListener("scroll", changed, true); document.addEventListener("visibilitychange", changed);
  document.addEventListener("pointerdown", cancel, true); document.addEventListener("keydown", cancel, true);
  window.addEventListener("resize", cancel); window.addEventListener("blur", cancel); media.addEventListener("change", cancel);
  const x = pose.box.left - target.left + (pose.box.width - target.width) / 2;
  const y = pose.box.top - target.top + (pose.box.height - target.height) / 2 - 3;
  try {
    animation = copy.animate([
      { transform: `translate(${x}px, ${y}px) scale(${pose.box.width / target.width}, ${pose.box.height / target.height}) rotate(-1.5deg)` },
      { transform: "none" },
    ], { duration: 200, easing: getComputedStyle(board).getPropertyValue("--ease-out").trim() || "ease-out" });
    animation.onfinish = cancel; animation.oncancel = cancel;
  } catch (error) { cancel(); console.warn("Card settlement is unavailable; recorded planning state is unchanged.", error); return null; }
  return { cancel, current };
}
