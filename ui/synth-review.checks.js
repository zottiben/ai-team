import { shellFixture } from './synth-fixture.checks.js';
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
  await until(() => visible(diff) && diff.querySelectorAll(".diff__line").length > 0, "Visible diff measurement did not settle");
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
  const { moveReviewFile } = await shellFixture();
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

export async function exerciseApproval() {
  must(document.visibilityState === "visible", "Visible viewport required for confirmation checks");
  const tab = await until(() => [...document.querySelectorAll('[role="tab"]')].find(node => node.textContent.includes("Review")), "Review tab unavailable");
  tab.click();
  const path = "src/feature-1/a-long-but-real-fixture-path/module-1.ts";
  const stage = await until(() => button(document, `Stage ${path}`), "Stage preview source unavailable");
  const card = stage.parentElement.querySelector(".diff");
  card.querySelector('[aria-label="comment on new line 1"]').click();
  const draft = await until(() => card.querySelector("textarea"), "Source comment did not open");
  fill(draft, "Keep this unsent source comment across the captured preview.");
  stage.scrollIntoView({ block: "center" }); stage.click();
  const dialog = await until(() => document.querySelector('[role="dialog"][aria-label="Checkout action approval"]'), "Exact confirmation did not open");
  await until(() => document.activeElement === dialog.querySelector("[data-approval-heading]"), "Confirmation did not focus its non-executing heading");
  must(!dialog.querySelector(".diff__add"), "Captured evidence offered a new repair action");
  must(dialog.textContent.includes("const next1 = true;"), "Captured hunks are absent");
  const identity = [...dialog.querySelectorAll("p")].find(node => node.textContent.includes("fingerprint"))?.textContent;
  const { moveReviewFile, reviewRequests } = await shellFixture();
  moveReviewFile(path);
  await until(() => dialog.isConnected && dialog.textContent.includes("Changed since preview"), "A refresh lost the confirmation or failed to invalidate approval");
  const approve = must(button(dialog, "Approve: Stage file"), "Confirmation button absent");
  must(approve.disabled, "Changed checkout retained approval");
  approve.click();
  must(dialog.textContent.includes(identity), "Current state replaced the captured fingerprint");
  must(dialog.textContent.includes("const next1 = true;"), "Refreshing discarded captured code");
  const retained = document.querySelector(`textarea[aria-label="comment on ${path} new line 1"]`);
  must(retained?.value.includes("unsent source comment"), "Preview/refresh lost the original comment body");
  must(button(retained.closest("form"), "Send to agent").disabled, "Moved source comment acquired a new sender");
  await delay(200); // Let the finite entry and resize placement finish; not a frame-time claim.
  must(dialog.isConnected, "Moving the trigger offscreen dismissed a still-visible confirmation");
  const rect = dialog.getBoundingClientRect();
  must(rect.left >= 7 && rect.right <= document.documentElement.clientWidth - 7 && rect.top >= 7 && rect.bottom <= innerHeight - 7,
    `Confirmation escaped the viewport: ${JSON.stringify({left:rect.left,right:rect.right,top:rect.top,bottom:rect.bottom,width:document.documentElement.clientWidth,height:innerHeight})}`);
  document.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true, cancelable: true }));
  await until(() => !dialog.isConnected, "Escape did not dismiss the confirmation");
  must(document.activeElement === button(document, "Inspect captured checkout approval"), "Escape lost the confirmation trigger");
  must(reviewRequests.length === 1 && reviewRequests[0].path.endsWith("/checkout/preview"), `Unexpected fixture command: ${JSON.stringify(reviewRequests)}`);
  const apiRequests = performance.getEntriesByType("resource").filter(entry => new URL(entry.name).pathname.startsWith("/api/")).length;
  must(apiRequests === 0, "Confirmation reached a real backend");
  must(document.documentElement.scrollWidth <= document.documentElement.clientWidth + 1, "Confirmation caused page overflow");
  return { width: document.documentElement.clientWidth, theme: document.documentElement.dataset.theme, capturedHunks: true, staleDisabled: true, commentRetained: true, syntheticPreviews: reviewRequests.length, apiRequests };
}

export async function exerciseSyntax() {
  must(document.visibilityState === "visible", "Visible viewport required for syntax checks");
  const messages = [], original = Worker.prototype.postMessage, originalTerminate = Worker.prototype.terminate;
  let retired = 0;
  Worker.prototype.terminate = function() { retired++; return originalTerminate.call(this); };
  Worker.prototype.postMessage = function(message, ...rest) {
    if (message?.input?.lines) messages.push({ rows: message.input.lines.length, chars: message.input.lines.reduce((n, row) => n + row.text.length + 1, 0) });
    return original.call(this, message, ...rest);
  };
  try {
    const tab = await until(() => [...document.querySelectorAll('[role="tab"]')].find(node => node.textContent.includes("Review")), "Review tab unavailable");
    tab.click();
    const diff = await until(() => document.querySelector('[aria-label="Diff for src/large.ts"]'), "Large diff unavailable");
    diff.scrollIntoView({ block: "center" });
    await until(() => diff.querySelector(".diff-syntax-keyword"), "Visible chunk did not receive syntax hints");
    const target = document.querySelector('[aria-label="Go to line in src/large.ts"]');
    fill(target, "8000"); button(target.closest("form"), "Go").click();
    const add = await until(() => diff.querySelector('[aria-label="comment on new line 8000"]'), "Distant syntax row absent");
    const code = add.closest(".diff__line").querySelector(".diff__text");
    await until(() => code.querySelector(".diff-syntax-keyword"), "Distant visible chunk stayed uncoloured");
    must(code.textContent === "export const sample8000 = 8000; // synthetic line, never executed", "Syntax replaced source text");
    must(document.activeElement === add, "Syntax arrival stole line focus");
    const built = new Worker("/dist/diff-syntax-worker.js", { type: "module" });
    try {
      await new Promise((resolve, reject) => {
        const timer = setTimeout(() => reject(Error("Built syntax worker timed out")), 5000);
        built.onerror = event => { clearTimeout(timer); reject(Error(`Built syntax worker failed: ${event.message}`)); };
        built.onmessage = event => { clearTimeout(timer); if (event.data.result.state === "ready" && event.data.result.lines.h0l0?.some(span => span.role === "keyword")) resolve(); else reject(Error(`Built worker returned no grammar: ${JSON.stringify(event.data)}`)); };
        built.postMessage({ id: 991, input: { path: "example.ts", lines: [{ key: "h0l0", kind: "added", text: 'const fixture = "safe";', display: true }] } });
      });
    } finally { built.terminate(); }
    const beforeHide = retired;
    [...document.querySelectorAll('[role="tab"]')].find(node => node.textContent.includes("Overview")).click();
    await until(() => retired > beforeHide, "Hidden Review kept its syntax worker alive");
    tab.click();
    await until(() => visible(diff) && diff.querySelectorAll(".diff__line").length > 0, "Restored Review measurement did not settle");
    must(messages.every(message => message.rows <= 128 && message.chars <= 65536), "Syntax shipped an unbounded diff to the worker");
    must(diff.querySelectorAll(".diff__line").length < 80, "Syntax defeated row windowing");
    must(document.documentElement.scrollWidth <= document.documentElement.clientWidth + 1, "Syntax caused horizontal overflow");
    const apiRequests = performance.getEntriesByType("resource").filter(entry => new URL(entry.name).pathname.startsWith("/api/")).length;
    must(apiRequests === 0, "Syntax reached a real backend");
    return { width: document.documentElement.clientWidth, theme: document.documentElement.dataset.theme, chunks: messages.length, maximumRows: Math.max(...messages.map(message => message.rows)), builtWorker: true, exactText: true, hiddenWorkerRetired: true, apiRequests };
  } finally { Worker.prototype.postMessage = original; Worker.prototype.terminate = originalTerminate; }
}

export async function exerciseTeamReview() {
  must(document.visibilityState === "visible", "Visible viewport required for team review checks");
  const tab = await until(() => [...document.querySelectorAll('[role="tab"]')].find(node => node.textContent.includes("Review")), "Review tab unavailable");
  tab.click();
  (await until(() => button(document, "Review S1"), "Team review fixture unavailable")).click();
  const active = () => [...document.querySelectorAll('[aria-label="Draft review"]')].find(visible);
  let panel = await until(() => { const p=active(); return p?.textContent.includes("Review S1") && p; }, "First draft did not open");
  panel.scrollIntoView({ block: "start" });
  const finding = panel.querySelector('textarea'), add = panel.querySelector('[aria-label="comment on new line 1"]');
  fill(finding, "Keep the original S1 general finding."); add.click();
  const inline = await until(() => panel.querySelector('textarea[aria-label="comment on src/S1.ts new line 1"]'), "Team inline draft absent");
  fill(inline, "Keep the original S1 exact line.");
  must(!document.querySelector('[aria-label="Working checkout actions"] .diff__add'), "Team mode offered a solo checkout repair");
  button(document, "Review S2").click();
  panel = await until(() => { const p=active(); return p?.textContent.includes("Review S2") && p; }, "Second draft did not open");
  must(panel.querySelector("textarea").value === "", "A general finding followed a different slice");
  button(document, "Return to retained review S1 · run 14 · revision 1 · generation 3").click();
  panel = await until(() => { const p=active(); return p?.textContent.includes("Review S1") && p; }, "Retained draft did not reopen");
  must(panel.contains(finding) && panel.contains(inline) && finding.value.includes("original S1") && inline.value.includes("original S1"), "Switching drafts lost the original input owners");
  const { replaceTeamReview, reviewRequests } = await shellFixture();
  replaceTeamReview();
  await until(() => button(panel, "Send finding for repair")?.disabled, "A newer generation was borrowed for an old finding");
  button(document, "Review S1").click();
  panel = await until(() => { const p=active(); return p?.textContent.includes("draft revision 2") && p; }, "Replacement draft did not open");
  must(panel.querySelector("textarea").value === "", "A general finding followed the replacement revision");
  must(panel.textContent.includes("export const revision = 2;"), "Replacement review has the wrong committed text");
  button(document, "Return to retained review S1 · run 14 · revision 1 · generation 3").click();
  panel = await until(() => { const p=active(); return p?.textContent.includes("draft revision 1") && p; }, "Old revision was discarded");
  panel.scrollIntoView({ block: "start" });
  must(panel.contains(finding) && panel.contains(inline), "An immutable revision switch remounted the original drafts");
  must(button(panel, "Send finding for repair").disabled && button(inline.closest("form"), "Send to agent").disabled, "Old unsubmitted drafts acquired replacement authority");
  const { setSynth } = await import("/src/ui-preferences.ts");
  setSynth(false); await delay(100); setSynth(true); await delay(150);
  must(panel.contains(finding) && panel.contains(inline) && finding.value.includes("original S1") && inline.value.includes("original S1"), "Skins lost a team draft");
  must(reviewRequests.length === 3 && reviewRequests.every(request => request.path === "/api/chats/1/draft/review"), `Unexpected team action: ${JSON.stringify(reviewRequests)}`);
  must(document.documentElement.scrollWidth <= document.documentElement.clientWidth + 1, "Team review overflowed the page");
  const apiRequests = performance.getEntriesByType("resource").filter(entry => new URL(entry.name).pathname.startsWith("/api/")).length;
  must(apiRequests === 0, "Team review reached a real backend");
  return { width: document.documentElement.clientWidth, theme: document.documentElement.dataset.theme, originalInputsRetained: true, oldScopeDisabled: true, syntheticReads: reviewRequests.length, apiRequests };
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
