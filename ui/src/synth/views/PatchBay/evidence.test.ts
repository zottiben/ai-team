import { expect, it } from "vitest";
import type { TeamBuild, TeamRun } from "../../../team-api";
import { fixtureOverview } from "../../overview-fixture";
import { DRAFT_LIMIT, MEMBER_LIMIT, patchEvidence, recordedExecutions } from "./evidence";

function fixture() {
  const { turns, live } = fixtureOverview(), turn = turns[0]!;
  const execution: TeamRun = { run_id: 1, chat_id: 1, control_node_id: 1, phase: "finished", base_sha: null, approved_revision: 4, reason: null, rev: 2, quiescent: true };
  turn.team = execution;
  turn.members = [2, 3].map(id => ({ node: { ...turn.node, id, role: "builder", slice_key: id === 2 ? "S1" : null }, live_text: "" }));
  const build: TeamBuild = { execution, closure: null, slices: [{ run_id: 1, slice_key: "S1", worktree_path: "/fixture/lease", branch: "fixture/draft", lease_state: "retained", build_status: "failed", candidate_sha: "a".repeat(40), commit_sha: null, release_started: false, reason: "Synthetic rejected check", rev: 1 }] };
  const detail = { id: 1, mode: "team" as const, workspace_path: "/fixture/demo", active_node_id: 1, turns, team_builds: [build] };
  live.mode = "team";
  return { detail, live, turn, build };
}
it("connects actual membership and exact slice assignments, never roles or output success", () => {
  const { detail, live } = fixture(), before = JSON.stringify(detail);
  const graph = patchEvidence(detail, live);
  expect(graph.modules.map(node => node.kind)).toEqual(["controller", "member", "member", "draft"]);
  expect(graph.modules[1]!.id).not.toBe(graph.modules[2]!.id);
  expect(graph.links.map(link => link.kind)).toEqual(["member", "member", "draft_record", "slice_assignment"]);
  const draft = graph.modules[3]!;
  expect(draft.kind === "draft" && draft.slice.build_status).toBe("failed");
  expect(draft.kind === "draft" && draft.slice.commit_sha).toBeNull();
  expect(JSON.stringify(detail)).toBe(before);
});
it("will not substitute another run for an explicit missing selection", () => {
  const { detail, live } = fixture();
  expect(patchEvidence(detail, live, 9).modules).toEqual([]);
  expect(patchEvidence(detail, live, 9).issues[0]).toContain("no other run");
});
it("rejects foreign or ambiguous execution ownership before drawing anything", () => {
  const { detail, live, turn } = fixture(); turn.team = { ...turn.team!, chat_id: 2 };
  expect(recordedExecutions(detail)).toEqual([]);
  expect(patchEvidence(detail, live).modules).toEqual([]);
  turn.team.chat_id = 1; detail.turns.push(structuredClone(turn));
  expect(patchEvidence(detail, live).modules).toEqual([]);
});
it("does not borrow telemetry from another chat, checkout, mode, model or node's run", () => {
  for (const change of ["chat", "checkout", "mode", "model", "run"] as const) {
    const { detail, live } = fixture();
    if (change === "chat") live.chat_id = 2;
    if (change === "checkout") live.workspace = "/other";
    if (change === "mode") live.mode = "single";
    if (change === "model") live.seats[0]!.model = "other";
    if (change === "run") live.seats[0]!.run_id = 2;
    const controller = patchEvidence(detail, live).modules[0]!;
    expect(controller.kind === "controller" && controller.seat).toBeNull();
  }
});
it("omits ambiguous members and drafts instead of minting duplicate cable identities", () => {
  const { detail, live, turn, build } = fixture();
  turn.members.push(structuredClone(turn.members[0]!)); build.slices.push(structuredClone(build.slices[0]!));
  const graph = patchEvidence(detail, live);
  expect(graph.modules).toHaveLength(2);
  expect(graph.links.map(link => link.kind)).toEqual(["member"]);
  expect(graph.omitted).toBe(4);
  expect(graph.issues).toHaveLength(2);
});
it("does not attach foreign drafts or a conflicting controller's build", () => {
  const { detail, live, build } = fixture(); build.slices[0]!.run_id = 2;
  expect(patchEvidence(detail, live).links.every(link => link.kind === "member")).toBe(true);
  build.execution = { ...build.execution, control_node_id: 999 };
  expect(patchEvidence(detail, live).modules).toHaveLength(3);
  expect(patchEvidence(detail, live).issues[0]).toContain("conflicting execution ownership");
});
it("bounds drawn modules, reports omissions and never leaves dangling cables", () => {
  const { detail, live, turn, build } = fixture(), member = turn.members[0]!, slice = build.slices[0]!;
  turn.members = Array.from({ length: 100 }, (_, i) => ({ ...member, node: { ...member.node, id: i + 2, slice_key: `S${i}` } }));
  build.slices = Array.from({ length: 100 }, (_, i) => ({ ...slice, slice_key: `S${i}` }));
  const graph = patchEvidence(detail, live), ids = new Set(graph.modules.map(node => node.id));
  expect(graph.modules).toHaveLength(1 + MEMBER_LIMIT + DRAFT_LIMIT);
  expect(graph.omitted).toBe(200 - MEMBER_LIMIT - DRAFT_LIMIT);
  expect(graph.links.every(link => ids.has(link.from) && ids.has(link.to))).toBe(true);
});
it("does not turn a node reported in two runs into two different execution identities", () => {
  const { detail, live, turn } = fixture(), other = structuredClone(turn);
  other.run.id = 2; other.node.id = 10; other.team = { ...other.team!, run_id: 2, control_node_id: 10 };
  detail.turns.push(other);
  const graph = patchEvidence(detail, live);
  expect(graph.modules.map(node => node.kind)).toEqual(["controller", "draft"]);
  expect(graph.omitted).toBe(2);
  expect(graph.issues[0]).toContain("conflicting execution ownership");
});
it("selects the active recorded controller, retaining recorded order and approval revision", () => {
  const { detail, live, turn } = fixture(), next = structuredClone(turn);
  next.run.id = 2; next.node.id = 10; next.team = { ...next.team!, run_id: 2, control_node_id: 10, approved_revision: null };
  next.members = next.members.map((member, i) => ({ ...member, node: { ...member.node, id: i + 20 } }));
  detail.turns.push(next);
  expect(patchEvidence(detail, live).execution?.run_id).toBe(1);
  detail.active_node_id = 10;
  expect(patchEvidence(detail, live).execution?.run_id).toBe(2);
  expect(patchEvidence(detail, live).execution?.approved_revision).toBeNull();
  expect(patchEvidence(detail, live, 1).execution?.approved_revision).toBe(4);
});
