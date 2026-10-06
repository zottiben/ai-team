import { api, type RepoMap } from "./api";

/**
 * The chat's own Overview, in two halves.
 *
 * They are fetched separately because they move at different speeds: the walk of a
 * checkout is read once per chat, the live half on every tick. Everything here is
 * reached from one chat id - there is no project, run or crew parameter to get wrong.
 */
export type ChatMapView = {
  workspace: string;
  /** Zones are *this chat's* seats, so a solo chat is one zone and not the roster's. */
  map: RepoMap;
};

export type SeatActivity = {
  kind: string;
  /** The tool, or the summary of whatever else the seat last did. */
  summary: string;
  /** Where it is reading or writing, or the command it is running. Bounded by the server. */
  detail: string | null;
  /** That path relative to the checkout, when it lands inside one. What the map lights. */
  file: string | null;
  at: string;
};

export type SeatTouch = {
  /** Relative to the checkout, which is what the map's nodes are keyed on. */
  path: string;
  tool: string;
  reads: number;
  writes: number;
  at: string;
};

export type ChatSeat = {
  role: string;
  node_id: number;
  run_id: number;
  provider: string;
  model: string;
  status: string;
  attempt: number;
  slice_key: string | null;
  worktree: string | null;
  started_at: string | null;
  ended_at: string | null;
  blocked_reason: string | null;
  /** Holds an unfinished turn. Not, on its own, that anything is behind it. */
  live: boolean;
  /** Whether a process is actually behind that turn; null when the seat is not live. */
  supervised: boolean | null;
  activity: SeatActivity | null;
  said: string | null;
  runs: number;
  steps: number;
  usage: { tokens_in: number; tokens_out: number; cache_read: number; cache_write: number };
  context_tokens: number | null;
  touches: SeatTouch[];
  live_touches?: SeatTouch[];
  /** Paths it touched that resolve outside this checkout, counted rather than dropped. */
  outside: number;
};

export type ChatOverview = {
  chat_id: number;
  project: string;
  workspace: string;
  mode: "single" | "team";
  seats: ChatSeat[];
  totals: {
    turns: number;
    steps: number;
    usage: { tokens_in: number; tokens_out: number; cache_read: number; cache_write: number };
    files_touched: number;
    files_written: number;
  };
  /** The evidence is read from the newest turns. These say how many, out of how many. */
  nodes_total: number;
  nodes_read: number;
};

export function chatOverview(id: number): Promise<ChatOverview> {
  return api(`/chats/${id}/overview`);
}

export function chatOverviewMap(id: number): Promise<ChatMapView> {
  return api(`/chats/${id}/overview/map`);
}
