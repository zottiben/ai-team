import {
  api,
  post,
  request,
  type NodeRun,
  type Run,
  type RunEvent,
  type Usage,
} from "./api";

export type Chat = {
  id: number;
  project_id: number;
  title: string;
  workspace_path: string;
  provider: string;
  model: string;
  reasoning: "none" | "low" | "medium" | "high";
  active_node_id: number | null;
  live_text: string;
  stop_requested: boolean;
  archived: boolean;
  rev: number;
  created_at: string;
  updated_at: string;
};

export type ChatTurn = {
  run: Run;
  node: NodeRun & { usage: Usage; started_at: string | null };
};
export type ChatDetail = Chat & {
  turns: ChatTurn[];
  state:
    | "empty"
    | "idle"
    | "running"
    | "stopping"
    | "interrupted"
    | "failed"
    | "stopped";
  can_resume: boolean;
  orphan_running: boolean;
};

export function chats(project: string): Promise<Chat[]> {
  return api(`/chats?project=${encodeURIComponent(project)}`);
}

export function chat(id: number): Promise<ChatDetail> {
  return api(`/chats/${id}`);
}

export function createChat(input: {
  project: string;
  workspace?: string;
  provider: string;
  model: string;
  reasoning: Chat["reasoning"];
}): Promise<Chat> {
  return post("/chats", input);
}

export function chatEvents(id: number, after = 0): Promise<RunEvent[]> {
  return api(`/chats/${id}/events?after=${after}`);
}

export function sendChat(
  id: number,
  message: string,
  requestId: string,
): Promise<{ run_id: number; node_id: number; started: boolean }> {
  return post(`/chats/${id}/messages`, { message, request_id: requestId });
}

export function stopChat(id: number, nodeId: number): Promise<unknown> {
  return post(`/chats/${id}/stop`, { node_id: nodeId });
}

export function resumeChat(id: number, nodeId: number): Promise<unknown> {
  return post(`/chats/${id}/resume`, { node_id: nodeId });
}

export function renameChat(id: number, title: string): Promise<Chat> {
  return request(`/chats/${id}`, "PATCH", { title });
}

export function archiveChat(id: number): Promise<Chat> {
  return request(`/chats/${id}`, "PATCH", { archived: true });
}
