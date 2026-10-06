import { api, post } from "./api";
import type { Chat } from "./chat-api";

export type WorkspaceChoice = { path: string; name: string; branch: string | null; unavailable: string | null };
export type WorkspaceRequest = { id: number; chat_id: number; after_node_id: number | null; from_path: string; to_path: string; state: "pending" | "applied" | "cancelled" };
export const chatWorkspaces = (project: string) => api<WorkspaceChoice[]>(`/chat-workspaces?project=${encodeURIComponent(project)}`);
export const workspaceRequests = (chat: number) => api<{ requests: WorkspaceRequest[] }>(`/chats/${chat}/workspace`);
export const requestWorkspace = (chat: number, path: string) => post<WorkspaceRequest>(`/chats/${chat}/workspace`, { path });
export const approveWorkspace = (chat: number, request: number, revision: number) => post<Chat>(`/chats/${chat}/workspace/approve`, { request, revision });
export const cancelWorkspace = (chat: number, request: number) => post<unknown>(`/chats/${chat}/workspace/cancel`, { request });
