import type { NodeRun } from "../../../api";
import type { ChatDetail, ChatTurn } from "../../../chat-api";
import type { ChatOverview, ChatSeat } from "../../../chat-overview-api";
import type { BuildSlice, TeamRun } from "../../../team-api";

type Source = Pick<ChatDetail, "id" | "mode" | "workspace_path" | "active_node_id" | "turns" | "team_builds">;
export type AgentModule = { id: string; kind: "controller" | "member"; node: NodeRun; seat: ChatSeat | null };
export type DraftModule = { id: string; kind: "draft"; slice: BuildSlice };
export type PatchModule = AgentModule | DraftModule;
export type PatchLink = { from: string; to: string; kind: "member" | "draft_record" | "slice_assignment" };
export type PatchEvidence = { execution: TeamRun | null; modules: PatchModule[]; links: PatchLink[]; omitted: number; issues: string[] };
export const MEMBER_LIMIT = 32, DRAFT_LIMIT = 16;
const validId = (id: number) => Number.isSafeInteger(id) && id > 0;
const agentId = (run: number, node: number) => JSON.stringify(["agent", run, node]);
const draftId = (run: number, key: string) => JSON.stringify(["draft", run, key]);
const sameExecution = (a: TeamRun, b: TeamRun) => a.chat_id === b.chat_id && a.run_id === b.run_id && a.control_node_id === b.control_node_id;
function unique<T>(rows: readonly T[], key: (row: T) => string | number): T[] {
  const counts = new Map<string | number, number>();
  for (const row of rows) counts.set(key(row), (counts.get(key(row)) ?? 0) + 1);
  return rows.filter(row => counts.get(key(row)) === 1);
}
export function recordedExecutions(detail: Source): ChatTurn[] {
  return unique(detail.turns.filter(turn => turn.team !== null && turn.team.chat_id === detail.id
    && validId(turn.run.id) && turn.team.run_id === turn.run.id && turn.team.control_node_id === turn.node.id && validId(turn.node.id)), turn => turn.run.id);
}

/** Evidence links, never dependency order, output provenance or editable execution wiring.
 * A selected missing run remains missing; neither role names nor array positions replace it. */
export function patchEvidence(detail: Source, live: ChatOverview | null, selectedRun?: number): PatchEvidence {
  const empty: PatchEvidence = { execution: null, modules: [], links: [], omitted: 0, issues: [] };
  if (detail.mode !== "team") return { ...empty, issues: ["This chat is not in team mode."] };
  const turns = recordedExecutions(detail);
  const turn = selectedRun === undefined
    ? turns.find(turn => turn.team?.control_node_id === detail.active_node_id) ?? turns.at(-1)
    : turns.find(turn => turn.run.id === selectedRun);
  if (!turn?.team) return { ...empty, issues: [selectedRun === undefined ? "No unambiguous team execution is recorded." : "The selected execution is unavailable; no other run was substituted."] };
  const issues: string[] = [], run = turn.run.id;
  const owners = new Map<number, Set<number>>();
  for (const candidate of turns) for (const node of [candidate.node, ...candidate.members.map(member => member.node)]) {
    if (!validId(node.id)) continue;
    const runs = owners.get(node.id) ?? new Set<number>(); runs.add(candidate.run.id); owners.set(node.id, runs);
  }
  if (owners.get(turn.node.id)?.size !== 1) return { ...empty, issues: ["The controller has conflicting execution ownership."] };
  const builds = detail.team_builds.filter(build => build.execution.run_id === run);
  const build = builds.length === 1 && sameExecution(builds[0]!.execution, turn.team) ? builds[0]! : null;
  if (builds.length && !build) issues.push("Draft evidence has conflicting execution ownership and is not connected.");
  const execution = build && build.execution.rev > turn.team.rev ? build.execution : turn.team;
  const scoped = live?.chat_id === detail.id && live.mode === "team" && live.workspace === detail.workspace_path;
  if (live && !scoped) issues.push("Seat telemetry does not match this chat, mode and checkout.");
  const seat = (node: NodeRun): ChatSeat | null => {
    const found = scoped ? live.seats.filter(seat => seat.node_id === node.id && seat.run_id === run) : [];
    const value = found.length === 1 ? found[0]! : null;
    return value && value.role === node.role && value.provider === node.provider && value.model === node.model ? value : null;
  };
  const recordedMembers = turn.members.filter(member => member.node.id !== turn.node.id);
  const members = recordedMembers.filter(member => validId(member.node.id) && owners.get(member.node.id)?.size === 1);
  if (members.length !== recordedMembers.length) issues.push("Member identities with missing or conflicting execution ownership are omitted.");
  const uniqueMembers = unique(members, member => member.node.id);
  if (members.length !== uniqueMembers.length) issues.push("Ambiguous member identities are omitted, not merged by role.");
  const slices = (build?.slices ?? []).filter(slice => slice.run_id === run && slice.slice_key.length > 0);
  if (build && slices.length !== build.slices.length) issues.push("Draft records with missing or foreign run identity are omitted.");
  const uniqueSlices = unique(slices, slice => slice.slice_key);
  if (slices.length !== uniqueSlices.length) issues.push("Ambiguous draft identities are omitted, not merged by slice name.");
  const controller: AgentModule = { id: agentId(run, turn.node.id), kind: "controller", node: turn.node, seat: seat(turn.node) };
  const agents: AgentModule[] = uniqueMembers.slice(0, MEMBER_LIMIT).map(member => ({ id: agentId(run, member.node.id), kind: "member", node: member.node, seat: seat(member.node) }));
  const drafts: DraftModule[] = uniqueSlices.slice(0, DRAFT_LIMIT).map(slice => ({ id: draftId(run, slice.slice_key), kind: "draft", slice }));
  const links: PatchLink[] = [
    ...agents.map(agent => ({ from: controller.id, to: agent.id, kind: "member" as const })),
    ...drafts.map(draft => ({ from: controller.id, to: draft.id, kind: "draft_record" as const })),
  ];
  for (const agent of agents) {
    const draft = drafts.find(draft => draft.slice.slice_key === agent.node.slice_key);
    if (draft) links.push({ from: agent.id, to: draft.id, kind: "slice_assignment" });
  }
  return { execution, modules: [controller, ...agents, ...drafts], links,
    omitted: recordedMembers.length - agents.length + (build?.slices.length ?? 0) - drafts.length, issues };
}
