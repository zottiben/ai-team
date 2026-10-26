// Isolated library prototype checks, not native input or production Board acceptance.
const delay = ms => new Promise(resolve => setTimeout(resolve, ms));
const must = (value, message) => { if (!value) throw Error(message); };
async function until(check, message) { const end = performance.now() + 4000; while (performance.now() < end) { const value = check(); if (value) return value; await delay(20); } throw Error(message); }
async function fixture() {
  must(document.visibilityState === "visible", "Native viewport is hidden; input verification blocked");
  // Import the loaded entry URL, including Vite's timestamp. Importing the bare
  // path can execute a second fixture and make a supposed 100-card probe use 200.
  const entry = [...document.scripts].find(script => new URL(script.src || location.href).pathname.endsWith("/src/synth/board-prototype.ts"));
  must(entry, "Board prototype entry missing");
  const module = await import(entry.src);
  must(document.querySelectorAll("[data-card]").length === 100 && document.querySelectorAll("h1").length === 1, "Prototype was bootstrapped more than once");
  return module;
}
const key = (key, code = key) => document.activeElement.dispatchEvent(new KeyboardEvent("keydown", { key, code, bubbles: true, cancelable: true }));
const start = async (module, handle) => { handle.focus(); key(" ", "Space"); await until(() => module.snapshot().dragging, "Keyboard pickup failed"); };
const noApi = () => {
  const count = performance.getEntriesByType("resource").filter(entry => new URL(entry.name).pathname.startsWith("/api/")).length;
  must(count === 0, "Input prototype reached a real API"); return count;
};
export async function exerciseBoardKeyboard() {
  const module = await fixture(), handle = document.querySelector('[data-card="S1"] button');
  await start(module, handle); key("ArrowRight");
  await until(() => module.snapshot().target === "ready", "Keyboard did not select the next lane after scrolling");
  key(" ", "Space"); await until(() => !module.snapshot().dragging, "Drop did not settle");
  must(module.snapshot().cards[0].status === "ready" && module.snapshot().changes === 1, "Keyboard drop changed the wrong fixture status");
  must(document.querySelector('[data-card="S1"] button') === handle && document.activeElement === handle, "Drop lost its original handle or focus");
  must(document.querySelector('[data-lane="ready"] [data-card]').dataset.card === "S1", "Drop invented a new persisted order");
  await start(module, handle); key("ArrowDown"); await delay(60); key(" ", "Space");
  await until(() => !module.snapshot().dragging, "Same-lane drop did not settle");
  must(module.snapshot().changes === 1, "Within-lane movement invented an ordering mutation");
  await start(module, handle); key("ArrowLeft"); await delay(100); key("Escape");
  await until(() => !module.snapshot().dragging, "Escape did not cancel");
  must(module.snapshot().changes === 1, "Escape changed a status");
  await start(module, handle); key("Tab");
  await until(() => !module.snapshot().dragging, "Tab did not cancel");
  const other = document.querySelector('[data-card="S2"] button');
  await start(module, handle); other.focus();
  await until(() => !module.snapshot().dragging, "Outside focus did not cancel");
  must(document.activeElement === other, "Cancellation stole newer input focus");
  await start(module, handle); window.dispatchEvent(new Event("resize"));
  await until(() => !module.snapshot().dragging, "Resize did not cancel");
  must(module.snapshot().changes === 1, "A cancellation changed a status");
  await start(module, handle); module.advanceRevision();
  await until(() => !module.snapshot().dragging, "Changed revision did not cancel");
  must(module.snapshot().changes === 1, "A movement borrowed a newer revision");
  await start(module, handle); module.freeze();
  await until(() => !module.snapshot().dragging, "Frozen state did not cancel");
  const starts = module.snapshot().starts; handle.focus(); key(" ", "Space"); await delay(100);
  must(module.snapshot().starts === starts && module.snapshot().changes === 1, "Frozen board admitted a movement");
  module.dispose(); await delay(100);
  must(!document.querySelector(".board-prototype-ghost") && !document.querySelector("[data-source]"), "Disposed feedback remained attached");
  let frames = 0; const original = window.requestAnimationFrame;
  window.requestAnimationFrame = callback => { frames++; return original.call(window, callback); };
  try { await delay(500); } finally { window.requestAnimationFrame = original; }
  must(frames === 0, `Disposed prototype kept scheduling ${frames} frames`);
  return { width: document.documentElement.clientWidth, cards: 100, syntheticKeyboard: true, statusChanges: 1, cancellation: true, disposalFrames: frames, apiRequests: noApi() };
}
let pointerProbe;
export async function preparePointerProbe() {
  const module = await fixture(), frames = []; let last = null, frame = null;
  const tick = time => { if (last !== null) frames.push(time - last); last = time; frame = requestAnimationFrame(tick); };
  const observer = new MutationObserver(() => {
    if (document.querySelector("[data-source]") && frame === null) frame = requestAnimationFrame(tick);
    else if (!document.querySelector("[data-source]") && frame !== null) { cancelAnimationFrame(frame); frame = null; }
  });
  observer.observe(document.querySelector(".board-prototype-lanes"), { subtree: true, attributes: true, attributeFilter: ["data-source"] });
  pointerProbe = { module, frames, close: () => { observer.disconnect(); if (frame !== null) cancelAnimationFrame(frame); } };
  return { cards: 100, before: module.snapshot().changes };
}
export async function readPointerProbe() {
  must(pointerProbe, "Prepare the pointer probe first");
  const { module, frames, close } = pointerProbe; pointerProbe = null;
  await until(() => !module.snapshot().dragging, "Pointer operation did not settle"); close();
  must(module.snapshot().changes === 1 && module.snapshot().cards[0].status === "ready", "Pointer drag did not change only the selected fixture status");
  const sorted = frames.slice().sort((a, b) => a - b);
  return { cards: document.querySelectorAll("[data-card]").length, changes: 1, pointerFrames: frames.length,
    medianMs: sorted[Math.floor(sorted.length / 2)] ?? null, p95Ms: sorted[Math.floor(sorted.length * .95)] ?? null,
    note: "Chromium tool-driven pointer sample, not physical WKWebView/production 60fps or INP acceptance", apiRequests: noApi() };
}
