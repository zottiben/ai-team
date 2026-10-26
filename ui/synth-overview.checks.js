// Run only with /synth-shell-preview.html?overview=true. No API mutation or real process.
const assert = (condition, message) => { if (!condition) throw new Error(message); };
const wait = async (predicate, label = "Overview fixture") => { const until = performance.now() + 6000; while (!predicate()) { if (performance.now() > until) throw new Error(`${label} did not settle`); await new Promise(resolve => setTimeout(resolve, 30)); } };
const frame = () => new Promise(resolve => requestAnimationFrame(() => requestAnimationFrame(resolve)));
export async function exerciseOverview() {
  assert(new URLSearchParams(location.search).get("overview") === "true", "Select the isolated Overview fixture");
  const overview = () => [...document.querySelectorAll('[role="tab"]')].find(tab => tab.textContent === "Overview");
  await wait(overview);
  if (overview().getAttribute("aria-selected") !== "true") overview().click();
  if (innerWidth < 900) document.querySelector('[aria-label="Collapse sidebar"]')?.click();
  await wait(() => document.querySelector('.synth-wavetable'));
  document.querySelector('.synth-wavetable').scrollIntoView({ block: "center" }); await frame();
  await wait(() => document.querySelector('.synth-overview-rack [aria-label="Covered"]')?.getAttribute("aria-valuetext") === "67%" && document.querySelector(".synth-wavetable")?.textContent.includes("4 hits"));
  const rack = document.querySelector(".synth-overview-rack"), context = document.querySelector(".synth-agent-telemetry");
  assert(rack.textContent.includes("Cache read: 1500") && rack.textContent.includes("Output: 45"), "Meters did not preserve distinct counters");
  assert(context.textContent.includes("700 prompt tokens") && context.textContent.includes("Latest reported prompt/context: 700"), "Context substituted an estimate or a cumulative total");
  assert(context.textContent.includes("Recorded compaction at plot position"), "Explicit compaction was not placed");
  const rate = document.querySelector('[aria-label="Output scope source"]');
  [...rate.querySelectorAll("button")].find(button => button.textContent === "Receive intervals").click();
  await frame();
  const observed = document.querySelector('.synth-execution-plot:not([hidden])');
  assert(observed.textContent.includes("Observed output change") && observed.textContent.includes("15 tok/s"), "Receive-rate view is not using exact same-message intervals");
  const compaction = context.querySelector("details"); compaction.open = true;
  assert(compaction.textContent.includes("estimated after: 500"), "Compaction estimate was not labelled");
  const modes = document.querySelector('[aria-label="Repository display"]');
  const choose = name => [...modes.querySelectorAll("button")].find(button => button.textContent === name).click();
  choose("XY"); await frame();
  const xy = document.querySelector(".synth-repository-xy");
  assert(xy.getBoundingClientRect().height > 0 && xy.querySelectorAll("circle").length === 3, "XY did not plot the bounded file records");
  choose("TREE"); await frame();
  const tree = document.querySelector(".synth-repository-tree");
  assert(tree.textContent.includes("5 displayed nodes · 3 walked files") && tree.textContent.includes("notes.txt"), "Tree lost the unowned path or map bounds");
  const preferences = await import("/src/ui-preferences.ts");
  preferences.setSynth(false); await frame(); preferences.setSynth(true); await frame();
  assert(document.querySelector(".synth-repository-tree") === tree && tree.getBoundingClientRect().height > 0, "Repository mode remounted or reset across skins");
  assert([...rate.querySelectorAll("button")].find(button => button.textContent === "Receive intervals").getAttribute("aria-pressed") === "true", "Rate source choice was reset");
  assert(document.querySelector(".synth-agent-telemetry details") === compaction && compaction.open, "Compaction evidence disclosure was lost");
  choose("WAVE"); await frame();
  const tokens = document.querySelector('[aria-label="Live token scope"]');
  assert(tokens?.querySelectorAll('svg').length === 5 && tokens.querySelector('[aria-label="Cache read tokens"]') && tokens.querySelector('[aria-label="Cache write tokens"]'), "Input/output/context/cache scopes are missing");
  tokens.scrollIntoView({ block: "start" }); await frame();
  const entry = [...document.scripts].find(script => new URL(script.src, location.href).pathname === "/src/synth/shell-preview.tsx");
  assert(entry, "Exact fixture entry is missing");
  (await import(entry.src)).advanceOverviewTelemetry();
  await wait(() => tokens.textContent.replaceAll(",", "").includes("1300 prompt tokens"), "Visible live token scope");
  assert(tokens.textContent.includes("350 input tokens") && tokens.textContent.includes("75 output tokens"), "Live counters did not update together");
  assert(tokens.textContent.includes("Model capacity is not reported"), "Context usage implies an invented capacity");
  assert(rack.textContent.includes("Uncached input: 200") && rack.textContent.includes("Output: 45"), "Live samples changed canonical totals");
  tokens.scrollIntoView({ block: "start" }); await frame();
  for (const graph of tokens.querySelectorAll('.synth-graph')) {
    const bounds = graph.getBoundingClientRect(), panel = tokens.getBoundingClientRect();
    assert(bounds.left >= panel.left && bounds.right <= panel.right + 1, "Token graph escapes its plate");
    assert(graph.querySelector('.synth-graph__line').getAttribute('d'), "Observed tokens were not drawn");
  }
  assert(document.documentElement.scrollWidth <= document.documentElement.clientWidth, "Overview introduced page overflow");
  const requests = performance.getEntriesByType("resource").filter(entry => new URL(entry.name).pathname.startsWith("/api/"));
  assert(requests.length === 0, "Overview fixture reached a real API");
  return { width: document.documentElement.clientWidth, theme: document.documentElement.dataset.theme, telemetry: "live input/output/context scope, completed steps, receive rates, explicit estimated compaction", repository: "WAVE / XY / TREE retained", apiRequests: requests.length };
}
