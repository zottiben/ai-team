const delay = ms => new Promise(resolve => setTimeout(resolve, ms));
const must = (value, message) => { if (!value) throw Error(message); return value; };
async function until(check, message) {
  for (let n = 0; n < 100; n++) { const value = check(); if (value) return value; await delay(50); }
  throw Error(message);
}
const visible = node => !!node && node.getClientRects().length > 0;
const button = (root, text) => [...root.querySelectorAll("button")].find(node => node.textContent.trim() === text && visible(node));
function fill(input, text) {
  const prototype = input instanceof HTMLTextAreaElement ? HTMLTextAreaElement.prototype : HTMLInputElement.prototype;
  Object.getOwnPropertyDescriptor(prototype, "value").set.call(input, text);
  input.dispatchEvent(new Event("input", { bubbles: true }));
}
export async function exerciseReview() {
  const review = await until(() => [...document.querySelectorAll('[role="tab"]')].find(node => node.textContent.includes("Review")), "Review tab unavailable");
  review.click();
  const panel = await until(() => document.querySelector('[aria-label="Changes and delivery"] .synth-review-delivery'), "Review fixture did not settle");
  must(panel.textContent.includes("grant unknown"), "Unknown push authority was invented");
  const current = document.querySelector('[aria-label="unstaged checkout files"]');
  must(current && [...current.querySelectorAll(".diff")].filter(visible).length === 3, "First-three default was not applied");
  const diff = must(document.querySelector('[aria-label="Diff for src/large.ts"]'), "Large diff absent");
  must(diff.dataset.windowed === "true", "Ten-thousand-line diff was not windowed");
  const initialRows = diff.querySelectorAll(".diff__line").length;
  must(initialRows > 0 && initialRows < 80, `Unbounded or empty initial diff: ${initialRows}`);
  const target = document.querySelector('[aria-label="Go to line in src/large.ts"]');
  target.scrollIntoView({ block: "center" }); fill(target, "8000");
  button(target.closest("form"), "Go").click();
  const add = await until(() => diff.querySelector('[aria-label="comment on new line 8000"]'), "Line jump never rendered its target");
  await delay(300);
  const rect = add.getBoundingClientRect(), frame = diff.getBoundingClientRect();
  must(document.activeElement === add && rect.top >= Math.max(frame.top, 0) - 2 && rect.bottom <= Math.min(frame.bottom, innerHeight) + 2,
    `Line jump focused outside the visible viewport: ${JSON.stringify({top:rect.top,bottom:rect.bottom,frameTop:frame.top,frameBottom:frame.bottom,height:innerHeight})}`);
  add.click();
  const draft = await until(() => diff.querySelector('textarea[aria-label="comment on src/large.ts new line 8000"]'), "Comment did not open");
  must(document.activeElement === draft, "Opening a comment did not focus its input");
  fill(draft, "Keep this exact line 8000 draft; no execution requested.");
  diff.scrollTop = 0; diff.dispatchEvent(new Event("scroll")); await delay(150);
  must(diff.contains(draft), "Scrolling recycled the active comment DOM");
  const tabs = [...document.querySelectorAll('[role="tab"]')], overview = tabs.find(node => node.textContent.includes("Overview"));
  overview.click(); await delay(100); review.click(); await delay(150);
  must(diff.contains(draft) && draft.value.includes("exact line 8000"), "Panel changes lost the comment");
  button(diff.closest(".diff"), "Show comment").click(); await delay(200);
  const { moveReviewFile } = await import("/src/synth/shell-preview.tsx");
  moveReviewFile("src/large.ts");
  await until(() => current.textContent.includes("Retained unsent comment"), "A moved file lost its retained comment");
  const retained = current.querySelector('textarea[aria-label="comment on src/large.ts new line 8000"]');
  must(retained?.value.includes("exact line 8000"), "Moving to the index discarded the draft");
  const send = button(retained.closest("form"), "Send to agent");
  must(send.disabled, "A missing original diff offered a new retargeted submission");
  const untracked = document.querySelector('[aria-label="untracked checkout files"]');
  button(untracked, "Show untracked files").click(); await delay(100);
  const newFile = must(untracked.querySelector(".diff"), "Untracked group did not open");
  newFile.querySelector('[aria-label="comment on new line 1"]').click(); await delay(50);
  must(button(untracked, "Hide untracked files").disabled, "Untracked grouping can hide an open anchor");
  must(document.documentElement.scrollWidth <= document.documentElement.clientWidth + 1,
    `Review overflow: ${document.documentElement.scrollWidth} / ${document.documentElement.clientWidth}`);
  const apiRequests = performance.getEntriesByType("resource").filter(entry => new URL(entry.name).pathname.startsWith("/api/")).length;
  must(apiRequests === 0, "Fixture reached a real backend");
  return { width: document.documentElement.clientWidth, theme: document.documentElement.dataset.theme, initialRows,
    retainedRows: current.querySelectorAll(".diff__line").length, draftRetained: true, missingAnchorDisabled: true, apiRequests };
}

// A synthetic visible scroll probe, not native input, INP or whole-app CPU certification.
export async function measureReviewScroll() {
  must(document.visibilityState === "visible", "Visible viewport required for the scroll probe");
  const tab = await until(() => [...document.querySelectorAll('[role="tab"]')].find(node => node.textContent.includes("Review")), "Review tab unavailable");
  tab.click();
  const scroller = await until(() => document.querySelector('[aria-label="Diff for src/large.ts"]'), "Review not ready");
  scroller.scrollIntoView({ block: "center" }); await delay(250);
  const intervals = []; let previous = null, rows = 0;
  for (let frame = 0; frame < 120; frame++) {
    const at = await new Promise(requestAnimationFrame);
    if (previous !== null) intervals.push(at - previous);
    previous = at;
    scroller.scrollTop = (scroller.scrollHeight - scroller.clientHeight) * frame / 119;
    rows = Math.max(rows, scroller.querySelectorAll(".diff__line").length);
  }
  intervals.sort((a, b) => a - b);
  return { frames: 120, medianMs: intervals[Math.floor(intervals.length / 2)], p95Ms: intervals[Math.floor(intervals.length * .95)], maximumRows: rows,
    visibility: document.visibilityState, width: document.documentElement.clientWidth };
}
