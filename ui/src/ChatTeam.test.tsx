import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, expect, it, vi } from "vitest";
import { ChatTeam } from "./ChatTeam";
import type { ChatDetail } from "./chat-api";
import type { BuildReview, TeamBuild } from "./team-api";

const service = vi.hoisted(() => ({ reviewTeam: vi.fn(), approveTeam: vi.fn(), teamCommand: vi.fn(), continueTeam: vi.fn(), closeTeam: vi.fn(), chatPlan: vi.fn() }));
vi.mock("./team-api", () => service);
vi.mock("./plan-api", () => ({ chatPlan: service.chatPlan }));
const build = (): TeamBuild => ({
  execution: { chat_id: 4, run_id: 12, control_node_id: 23, phase: "awaiting_approval", rev: 8, quiescent: true, approved_revision: null, base_sha: null, reason: null },
  slices: [], closure: null,
});
const detail = (b: TeamBuild): ChatDetail => ({
  id: 4, project_id: 1, title: "Work", workspace_path: "/repo", provider: "local", model: "fixture", reasoning: "high", mode: "team",
  active_node_id: 23, live_text: "", stop_requested: false, archived: false, rev: 5, created_at: "", updated_at: "", turns: [], state: "awaiting_approval", can_resume: false, orphan_running: false,
  team_builds: [b], team_recovery: { target: { chat_id: 4, run_id: 12, node_id: 23, expect_revision: 8 }, state: "quiescent", reason: null },
});
const review = (b: TeamBuild): BuildReview => ({ execution: b.execution, plan: { chat_id: 4, project_id: 1, revision: 44, bundle: null }, roster: [], roster_revision: "roster-A", head: "base-A", dirty: "" });
const props = (d: ChatDetail) => ({ detail: d, busy: false, command: async (act: () => Promise<unknown>) => { await act(); }, events: [], now: 0 });
beforeEach(() => { vi.clearAllMocks(); service.chatPlan.mockResolvedValue({ revision: 45 }); });

it("requires an explicit frozen review before approval, not a question or refresh", async () => {
  const b = build();
  const snapshot = review(b);
  service.reviewTeam.mockResolvedValue(snapshot);
  const view = render(<ChatTeam {...props(detail(b))} />);
  expect(screen.queryByRole("button", { name: "Approve and build" })).toBeNull();
  expect(service.approveTeam).not.toHaveBeenCalled();
  fireEvent.click(screen.getByRole("button", { name: "Review build" }));
  await screen.findByText("Base commit: base-A");
  expect(service.reviewTeam).toHaveBeenCalledWith(b.execution);
  view.rerender(<ChatTeam {...props(detail(b))} />);
  fireEvent.click(screen.getByRole("button", { name: "Approve and build" }));
  await waitFor(() => expect(service.approveTeam).toHaveBeenCalledWith(snapshot));
});

it("keeps a dirty or stale review from enabling a build", async () => {
  const b = build();
  service.reviewTeam.mockResolvedValue({ ...review(b), dirty: " M kept.txt" });
  render(<ChatTeam {...props(detail(b))} />);
  fireEvent.click(screen.getByRole("button", { name: "Review build" }));
  const approve = await screen.findByRole("button", { name: "Approve and build" });
  expect((approve as HTMLButtonElement).disabled).toBe(true);
  fireEvent.click(approve);
  expect(service.approveTeam).not.toHaveBeenCalled();
});

it("separates continuation, reconciliation and irreversible close with retained evidence", async () => {
  const b = build();
  b.execution = { ...b.execution, phase: "blocked", approved_revision: 44 };
  b.slices = [{ run_id: 12, slice_key: "S1", worktree_path: "/kept/work", branch: "draft", lease_state: "retained", build_status: "failed", commit_sha: null, candidate_sha: null, release_started: false, reason: "Stopped", rev: 19 }];
  render(<ChatTeam {...props(detail(b))} />);
  fireEvent.click(screen.getByRole("button", { name: "Continue S1" }));
  await waitFor(() => expect(service.continueTeam).toHaveBeenCalledWith(b.execution, b.slices[0]));
  expect(service.closeTeam).not.toHaveBeenCalled();
  fireEvent.click(screen.getByRole("button", { name: "Close and keep work…" }));
  await screen.findByRole("form", { name: "Close build" });
  fireEvent.change(screen.getByRole("textbox", { name: "Reason" }), { target: { value: "Keep the draft for inspection" } });
  fireEvent.click(screen.getByRole("button", { name: "Confirm close and keep" }));
  await waitFor(() => expect(service.closeTeam).toHaveBeenCalledWith(b, 45, "Keep the draft for inspection"));
  expect(screen.getByText("/kept/work")).toBeTruthy();
  expect(service.teamCommand).not.toHaveBeenCalled();
});

it("replays an interrupted close with its recorded immutable reason", async () => {
  const b = build();
  b.execution = { ...b.execution, phase: "blocked", approved_revision: 44 };
  b.closure = { reason: "Keep the exact recorded intent", finished_at: null, issues: ["board write interrupted"] };
  render(<ChatTeam {...props(detail(b))} />);
  fireEvent.click(screen.getByRole("button", { name: "Finish closing build" }));
  const input = await screen.findByRole("textbox", { name: "Reason" }) as HTMLInputElement;
  expect(input.value).toBe("Keep the exact recorded intent");
  expect(input.readOnly).toBe(true);
  fireEvent.click(screen.getByRole("button", { name: "Confirm close and keep" }));
  await waitFor(() => expect(service.closeTeam).toHaveBeenCalledWith(b, 45, "Keep the exact recorded intent"));
});

it("offers reconciliation, not model continuation, for a verified candidate", () => {
  const b = build();
  b.execution = { ...b.execution, phase: "blocked", approved_revision: 44 };
  b.slices = [{ run_id: 12, slice_key: "S1", worktree_path: "/kept/work", branch: "draft", lease_state: "retained", build_status: "verified", commit_sha: null, candidate_sha: "candidate", release_started: false, reason: "publication interrupted", rev: 19 }];
  render(<ChatTeam {...props(detail(b))} />);
  expect(screen.queryByRole("button", { name: "Continue S1" })).toBeNull();
  expect(screen.getByRole("button", { name: "Reconcile recorded work" })).toBeTruthy();
});

it("shows historical retained work after solo return without giving it live commands", () => {
  const b = build();
  b.slices = [{ run_id: 12, slice_key: "S1", worktree_path: "/kept/work", branch: "draft", lease_state: "retained", build_status: "failed", commit_sha: null, candidate_sha: null, release_started: true, reason: "uncertain return", rev: 19 }];
  render(<ChatTeam {...props({ ...detail(b), mode: "single", active_node_id: null })} />);
  expect(screen.getByText("/kept/work")).toBeTruthy();
  expect(screen.queryByRole("button", { name: "Continue S1" })).toBeNull();
  expect(screen.queryByRole("button", { name: "Stop team" })).toBeNull();
  expect(screen.getByText(/Return was attempted/)).toBeTruthy();
});
