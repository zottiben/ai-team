import { readFileSync } from "node:fs";
import { join } from "node:path";
import { act, fireEvent, render, screen, within } from "@testing-library/react";
import { afterEach, expect, it, vi } from "vitest";
import WorkPanel from "./WorkPanel";
import { ChatContext } from "../../../ChatContext";
import type { RunEvent } from "../../../api";
import type { ChatDetail, ChatTurn } from "../../../chat-api";
import type { BuildReview, TeamBuild } from "../../../team-api";
import { useWorkView } from "../Work/view";

const service = vi.hoisted(() => ({
  reviewTeam: vi.fn(), approveTeam: vi.fn(), teamCommand: vi.fn(), continueTeam: vi.fn(), closeTeam: vi.fn(),
  chatPlan: vi.fn(), inspectRetained: vi.fn(), keepRetained: vi.fn(), retainedFile: vi.fn(),
}));
vi.mock("../../../team-api", async original => ({ ...(await original<typeof import("../../../team-api")>()), ...service }));
vi.mock("../../../plan-api", () => ({ chatPlan: service.chatPlan }));
vi.mock("../../../changes-api", () => ({ inspectRetained: service.inspectRetained, keepRetained: service.keepRetained, retainedFile: service.retainedFile }));

const NOW = Date.UTC(2026, 9, 15, 10, 0, 0);
const node = (over: Partial<ChatTurn["node"]> = {}): ChatTurn["node"] => ({
  id: 1, role: "assistant", provider: "openai", model: "gpt-5", status: "done", attempt: 1,
  slice_key: null, worktree_path: null, branch: null, blocked_reason: null,
  started_at: null, ended_at: null, usage: { tokens_in: 0, tokens_out: 0, cache_read: 0, cache_write: 0 }, ...over,
});
const run = (over: Partial<ChatTurn["run"]> = {}): ChatTurn["run"] => ({
  id: 1, project_id: 1, prompt: "Do the work", status: "done", trigger: "manual", workspace_path: "/repo",
  created_at: "2026-10-15T09:00:00Z", started_at: "2026-10-15T09:00:00Z", ended_at: "2026-10-15T09:30:00Z", ...over,
});
const turn = (over: Partial<ChatTurn> = {}): ChatTurn => ({ run: run(), node: node(), team: null, members: [], ...over });
const detail = (turns: ChatTurn[], over: Partial<ChatDetail> = {}): ChatDetail => ({
  id: 4, project_id: 1, title: "Work", workspace_path: "/repo", provider: "openai", model: "gpt-5", reasoning: "high",
  mode: "single", active_node_id: null, live_text: "", stop_requested: false, archived: false, rev: 1,
  created_at: "", updated_at: "", turns, followups: [], team_builds: [], state: "idle", can_resume: false,
  orphan_running: false, ...over,
});
const event = (id: number, summary: string, nodeRun: number, at = "2026-10-15T09:10:00Z"): RunEvent =>
  ({ id, node_run_id: nodeRun, kind: "note", actor: "pi", summary, message: null, thinking: [], at });

function Harness({ chat, events = [], now = NOW }: { chat: ChatDetail; events?: RunEvent[]; now?: number }) {
  const view = useWorkView();
  return <WorkPanel detail={chat} events={events} now={now} view={view} />;
}
const clip = (runId: number) => screen.getByRole("button", { name: new RegExp(`^Run #${runId} ·`) });
const row = (runId: number) => document.querySelector<HTMLElement>(`.synth-turn[data-run-id="${runId}"]`);

afterEach(() => { vi.useRealTimers(); vi.clearAllMocks(); });

it("counts and names every recorded state instead of folding it into done or cancelled", () => {
  render(<Harness chat={detail([
    turn({ run: run({ id: 1 }), node: node({ id: 1 }) }),
    turn({ run: run({ id: 2, status: "cancelled" }), node: node({ id: 2, status: "cancelled" }) }),
    turn({ run: run({ id: 3, status: "failed" }), node: node({ id: 3, status: "failed", blocked_reason: "The gate rejected the diff" }) }),
    turn({ run: run({ id: 4, status: "blocked" }), node: node({ id: 4, status: "blocked" }) }),
    turn({ run: run({ id: 5, status: "queued", started_at: null, ended_at: null }), node: node({ id: 5, status: "queued" }) }),
    turn({ run: run({ id: 6, status: "running", ended_at: null }), node: node({ id: 6, status: "running" }) }),
  ], { active_node_id: 6, state: "running" })} />);
  expect(within(screen.getByRole("group", { name: "Runs" })).getByText("6")).toBeTruthy();
  expect(within(screen.getByRole("group", { name: "Done" })).getByText("1")).toBeTruthy();
  expect(within(screen.getByRole("group", { name: "Cancelled" })).getByText("1")).toBeTruthy();
  expect(within(screen.getByRole("group", { name: "Live" })).getByText("1")).toBeTruthy();
  const others = screen.getByRole("group", { name: "Other recorded states" });
  expect(within(others).getAllByRole("listitem").map(item => item.textContent)).toEqual(["Failed1", "Blocked1", "Queued1"]);
  expect(row(3)?.textContent).toContain("The gate rejected the diff");
  expect([...document.querySelectorAll(".synth-turn")].map(turn => turn.getAttribute("data-status")))
    .toEqual(["done", "cancelled", "failed", "blocked", "queued", "running"]);
  expect(document.querySelectorAll(".synth-clip[data-live]")).toHaveLength(1);
});

it("numbers the pads oldest first and names the range they cover", () => {
  render(<Harness chat={detail([
    turn({ run: run({ id: 22, status: "cancelled", prompt: "Ground yourself on the ticket\nthen plan" }), node: node({ id: 22, status: "cancelled" }) }),
    turn({ run: run({ id: 23 }), node: node({ id: 23 }) }),
    turn({ run: run({ id: 35, status: "failed" }), node: node({ id: 35, status: "failed" }) }),
  ])} />);
  const pattern = screen.getByRole("group", { name: "Run pattern" });
  expect(within(pattern).getByText("#22 → #35")).toBeTruthy();
  expect(within(pattern).getAllByRole("button").map(pad => pad.getAttribute("aria-label")))
    .toEqual(["Run #22: Cancelled, 30m", "Run #23: Done, 30m", "Run #35: Failed, 30m"]);
  expect([...document.querySelectorAll(".synth-turn")].map(row => row.getAttribute("data-run-id"))).toEqual(["22", "23", "35"]);
  expect(clip(22).getAttribute("title")).toBe("Run #22 · Cancelled · 30m · Ground yourself on the ticket");
  expect(document.querySelectorAll(".synth-clip__playhead")).toHaveLength(0);
});

it("transitions the live clip over one second and stops it under reduced motion", () => {
  const css = readFileSync(join(import.meta.dirname, "../Work/work.css"), "utf8");
  expect(css).toContain('.synth-arrangement__lane[data-motion="true"] .synth-clip[data-live] { transition: flex-grow 1s linear; }');
  expect(css).toMatch(/@media \(prefers-reduced-motion: reduce\) \{[\s\S]*\.synth-clip\[data-live\] \{ transition: none; \}/);
  expect(css).toContain("font-size: 14px; line-height: 1.5;");
});

it("sizes clips by recorded wall duration and rules the lane", () => {
  render(<Harness chat={detail([
    turn({ run: run({ id: 1, ended_at: "2026-10-15T09:30:00Z" }), node: node({ id: 1 }) }),
    turn({ run: run({ id: 2, status: "cancelled", started_at: "2026-10-15T09:30:00Z", ended_at: "2026-10-15T09:40:00Z" }), node: node({ id: 2, status: "cancelled" }) }),
    turn({ run: run({ id: 3, started_at: "2026-10-15T09:40:00Z", ended_at: "2026-10-15T09:40:00Z" }), node: node({ id: 3 }) }),
  ])} />);
  expect([...document.querySelectorAll<HTMLElement>(".synth-clip")].map(element => element.style.flexGrow)).toEqual(["30", "10", "0"]);
  expect([...document.querySelectorAll(".synth-arrangement__ruler span")].map(tick => tick.textContent)).toEqual(["0m", "10m", "20m", "30m", "40m"]);
  expect(clip(3).getAttribute("aria-label")).toContain("0s");
  expect(screen.queryByText(/No ruler/)).toBeNull();
});

it("keeps widths equal and hides the ruler when a run has no recorded end", () => {
  render(<Harness chat={detail([
    turn({ run: run({ id: 1 }), node: node({ id: 1 }) }),
    turn({ run: run({ id: 2, status: "running", started_at: "2026-10-15T09:30:00Z", ended_at: null }), node: node({ id: 2, status: "running" }) }),
  ])} />);
  expect([...document.querySelectorAll<HTMLElement>(".synth-clip")].map(element => element.style.flexGrow)).toEqual(["1", "1"]);
  expect(document.querySelectorAll(".synth-arrangement__ruler")).toHaveLength(0);
  expect(screen.getByText(/No ruler: at least one run has no valid recorded start and end/)).toBeTruthy();
  expect(row(2)?.textContent).toContain("duration unknown");
  expect(document.querySelectorAll(".synth-clip[data-live]")).toHaveLength(0);
});

it("grows only an actually live run on the shared one-second readout", () => {
  vi.useFakeTimers({ shouldAdvanceTime: false });
  vi.setSystemTime(new Date(NOW));
  const live = detail([
    turn({ run: run({ id: 1, ended_at: "2026-10-15T09:10:00Z" }), node: node({ id: 1 }) }),
    turn({ run: run({ id: 2, status: "running", started_at: "2026-10-15T09:50:00Z", ended_at: null }), node: node({ id: 2, status: "running" }) }),
  ], { active_node_id: 2, state: "running" });
  render(<Harness chat={live} now={NOW} />);
  expect([...document.querySelectorAll<HTMLElement>(".synth-clip")].map(element => element.style.flexGrow)).toEqual(["10", "10"]);
  act(() => { vi.advanceTimersByTime(60_000); });
  expect([...document.querySelectorAll<HTMLElement>(".synth-clip")].map(element => element.style.flexGrow)).toEqual(["10", "11"]);
  expect(row(2)?.textContent).toContain("11m");
});

it("leaves an interrupted turn still, and still visible", () => {
  vi.useFakeTimers({ shouldAdvanceTime: false });
  vi.setSystemTime(new Date(NOW));
  render(<Harness chat={detail([
    turn({ run: run({ id: 1, status: "running", started_at: "2026-10-15T09:50:00Z", ended_at: null }), node: node({ id: 1, status: "running" }) }),
  ], { active_node_id: 1, state: "interrupted", orphan_running: false })} />);
  expect(row(1)?.textContent).toContain("Interrupted");
  expect(row(1)?.textContent).toContain("duration unknown");
  act(() => { vi.advanceTimersByTime(60_000); });
  expect(row(1)?.textContent).toContain("duration unknown");
  expect(document.querySelectorAll(".synth-clip[data-live]")).toHaveLength(0);
  expect(within(screen.getByRole("group", { name: "Live" })).getByText("0")).toBeTruthy();
});

it("expands a clamped prompt without losing the model and duration", () => {
  const prompt = `Ground yourself on the ticket.\n\n- ${"detail ".repeat(60)}`;
  render(<Harness chat={detail([turn({ run: run({ id: 9, prompt }), node: node({ id: 9 }) })])} />);
  const clamped = document.querySelector(".synth-turn__prompt");
  expect(clamped?.getAttribute("data-clamp")).toBe("4");
  fireEvent.click(screen.getByRole("button", { name: "Show full prompt" }));
  expect(document.querySelector(".synth-turn__prompt")).toBeNull();
  expect(row(9)?.textContent).toContain("Ground yourself on the ticket.");
  expect(row(9)?.textContent).toContain("openai/gpt-5 · 30m");
  fireEvent.click(screen.getByRole("button", { name: "Show less" }));
  expect(document.querySelector(".synth-turn__prompt")?.getAttribute("data-clamp")).toBe("4");
  expect(screen.getByRole("button", { name: "Show full prompt" })).toBeTruthy();
});

it("renders a continuation in its own voice and offers no expansion for a short prompt", () => {
  render(<Harness chat={detail([turn({ run: run({ id: 3, prompt: "continue" }), node: node({ id: 3 }) })])} />);
  expect(document.querySelector(".synth-turn__prompt")?.getAttribute("data-continuation")).toBe("true");
  expect(screen.queryByRole("button", { name: "Show full prompt" })).toBeNull();
});

it("jumps from a pad and from a clip to the exact run row, and pulses it once", () => {
  vi.useFakeTimers({ shouldAdvanceTime: false });
  render(<Harness chat={detail([
    turn({ run: run({ id: 11 }), node: node({ id: 11 }) }),
    turn({ run: run({ id: 12, status: "cancelled" }), node: node({ id: 12, status: "cancelled" }) }),
  ])} />);
  fireEvent.click(screen.getByRole("button", { name: /^Run #12: Cancelled/ }));
  expect(document.activeElement?.getAttribute("data-run-id")).toBe("12");
  expect(row(12)?.getAttribute("data-pulse")).toBe("true");
  act(() => { vi.advanceTimersByTime(600); });
  expect(row(12)?.getAttribute("data-pulse")).toBeNull();
  expect(screen.getByRole("button", { name: /^Run #12: Cancelled/ }).getAttribute("aria-current")).toBe("true");
  fireEvent.click(clip(11));
  expect(document.activeElement?.getAttribute("data-run-id")).toBe("11");
  expect(row(11)?.getAttribute("data-pulse")).toBe("true");
});

it("filters without losing a state, and says what is hidden", () => {
  render(<Harness chat={detail([
    turn({ run: run({ id: 1 }), node: node({ id: 1 }) }),
    turn({ run: run({ id: 2, status: "cancelled" }), node: node({ id: 2, status: "cancelled" }) }),
    turn({ run: run({ id: 3, status: "failed" }), node: node({ id: 3, status: "failed" }) }),
  ])} />);
  const filters = screen.getByRole("group", { name: "Filter turns" });
  expect(within(filters).getAllByRole("button").map(button => button.textContent)).toEqual(["All 3", "Done 1", "Cancelled 1", "Other 1"]);
  fireEvent.click(within(filters).getByRole("button", { name: "Other 1" }));
  expect([...document.querySelectorAll(".synth-turn")].map(turn => turn.getAttribute("data-run-id"))).toEqual(["3"]);
  fireEvent.click(within(filters).getByRole("button", { name: "Done 1" }));
  expect([...document.querySelectorAll(".synth-turn")].map(turn => turn.getAttribute("data-run-id"))).toEqual(["1"]);
});

it("reveals a filtered-out run before an exact jump", () => {
  render(<Harness chat={detail([
    turn({ run: run({ id: 1 }), node: node({ id: 1 }) }),
    turn({ run: run({ id: 2, status: "failed" }), node: node({ id: 2, status: "failed" }) }),
  ])} />);
  fireEvent.click(within(screen.getByRole("group", { name: "Filter turns" })).getByRole("button", { name: "Done 1" }));
  fireEvent.click(clip(2));
  expect(document.activeElement?.getAttribute("data-run-id")).toBe("2");
});

it("holds actual upward scrolling even before the native scroll event arrives", () => {
  const chat = detail([turn({ run: run({ status: "running", ended_at: null }), node: node({ status: "running" }) })], { active_node_id: 1, state: "running" });
  const view = render(<Harness chat={chat} />);
  const list = document.querySelector<HTMLElement>(".synth-turns")!;
  Object.defineProperties(list, { scrollHeight: { value: 1000 }, clientHeight: { value: 100 } });
  view.rerender(<Harness chat={{ ...chat }} />);
  expect(list.scrollTop).toBe(1000);
  list.scrollTop = 300;
  view.rerender(<Harness chat={{ ...chat }} />);
  expect(list.scrollTop).toBe(300);
});

it("never reads completion out of what an agent wrote", () => {
  const events = [event(1, "VERDICT: pass - everything is done", 1), event(2, "Ready for your next message", 1)];
  render(<Harness chat={detail([
    turn({ run: run({ id: 1, status: "failed", prompt: "done: ship it" }), node: node({ id: 1, status: "failed" }) }),
  ], { live_text: "All tasks are complete and the build is done." })} events={events} />);
  expect(row(1)?.textContent).toContain("Failed");
  expect(row(1)?.textContent).not.toContain("Done");
  expect(within(screen.getByRole("group", { name: "Done" })).getByText("0")).toBeTruthy();
  const activity = screen.getByRole("region", { name: "Recent activity" });
  expect(within(activity).getAllByRole("listitem").map(item => item.textContent)).toEqual([
    "VERDICT: pass - everything is done2026-10-15T09:10:00Z · turn #1",
    "Ready for your next message2026-10-15T09:10:00Z · turn #1",
  ]);
});

it("holds the live follow while a reader is further up the list", () => {
  const chat = detail([
    turn({ run: run({ id: 1 }), node: node({ id: 1 }) }),
    turn({ run: run({ id: 2, status: "running", started_at: "2026-10-15T09:50:00Z", ended_at: null }), node: node({ id: 2, status: "running" }) }),
  ], { active_node_id: 2, state: "running" });
  render(<Harness chat={chat} />);
  const list = document.querySelector<HTMLElement>(".synth-turns")!;
  expect(screen.queryByRole("button", { name: "Jump to live run" })).toBeNull();
  Object.defineProperty(list, "scrollHeight", { value: 900, configurable: true });
  Object.defineProperty(list, "clientHeight", { value: 300, configurable: true });
  list.scrollTop = 0;
  fireEvent.scroll(list);
  const resume = screen.getByRole("button", { name: "Jump to live run" });
  expect(list.scrollTop).toBe(0);
  fireEvent.click(resume);
  expect(list.scrollTop).toBe(900);
  expect(screen.queryByRole("button", { name: "Jump to live run" })).toBeNull();
});

it("keeps the team controls and the reader's choices through a skin change", async () => {
  const execution: TeamBuild["execution"] = {
    chat_id: 4, run_id: 12, control_node_id: 23, phase: "awaiting_approval", rev: 8,
    quiescent: true, approved_revision: null, base_sha: null, reason: null,
  };
  const review: BuildReview = {
    execution, plan: { chat_id: 4, project_id: 1, revision: 44, bundle: null },
    roster: [], roster_revision: "roster-A", head: "base-A", dirty: "",
  };
  service.reviewTeam.mockResolvedValue(review);
  const chat = detail([
    turn({ run: run({ id: 11 }), node: node({ id: 11 }) }),
    turn({
      run: run({ id: 12, status: "running", ended_at: null }), node: node({ id: 23, status: "running" }),
      team: execution, members: [],
    }),
  ], { mode: "team", active_node_id: 23, state: "awaiting_approval", team_builds: [{ execution, slices: [], closure: null }] });
  const props = (synth: boolean) => ({
    detail: chat, panel: "work" as const, opened: new Set(["work" as const]), hidden: false, status: "Awaiting approval",
    tick: 0, busy: false, command: async (act: () => Promise<unknown>) => { await act(); }, events: [], elapsed: 0,
    now: NOW, onChanged: () => {}, onFeedback: () => {}, onClose: () => {}, synth,
  });
  const view = render(<ChatContext {...props(false)} />);
  fireEvent.click(screen.getByRole("button", { name: "Review build" }));
  const approve = await screen.findByRole("button", { name: "Approve and build" });
  expect(screen.getByText("Base commit: base-A")).toBeTruthy();
  expect(document.querySelector(".chat-turn-list")).toBeTruthy();

  view.rerender(<ChatContext {...props(true)} />);
  await screen.findByRole("group", { name: "Run pattern" });
  expect(screen.getByRole("button", { name: "Approve and build" })).toBe(approve);
  expect(screen.getByText("Base commit: base-A")).toBeTruthy();
  expect(document.querySelector(".chat-turn-list")?.closest("[hidden]")).toBeTruthy();
  const retainedList = document.querySelector<HTMLElement>(".synth-turns")!;
  retainedList.scrollTop = 128;
  expect(row(12)?.textContent).toContain("Awaiting your approval");
  fireEvent.click(within(screen.getByRole("group", { name: "Filter turns" })).getByRole("button", { name: "Done 1" }));
  expect([...document.querySelectorAll(".synth-turn")].map(turn => turn.getAttribute("data-run-id"))).toEqual(["11"]);

  view.rerender(<ChatContext {...props(false)} />);
  expect(screen.getByRole("button", { name: "Approve and build" })).toBe(approve);
  expect(document.querySelectorAll(".chat-turn-list li")).toHaveLength(2);
  view.rerender(<ChatContext {...props(true)} />);
  await screen.findByRole("group", { name: "Filter turns" });
  expect(within(screen.getByRole("group", { name: "Filter turns" })).getByRole("button", { name: "Done 1" }).getAttribute("aria-pressed")).toBe("true");
  expect([...document.querySelectorAll(".synth-turn")].map(turn => turn.getAttribute("data-run-id"))).toEqual(["11"]);
  expect(document.querySelector(".synth-turns")).toBe(retainedList);
  expect(retainedList.scrollTop).toBe(128);
  expect(service.approveTeam).not.toHaveBeenCalled();
  expect(service.teamCommand).not.toHaveBeenCalled();
});
