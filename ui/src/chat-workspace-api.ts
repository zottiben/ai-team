import { api, post } from "./api";
import type { Chat } from "./chat-api";

export type WorkspaceChoice = { path: string; name: string; branch: string | null; unavailable: string | null; processes?: { pid: number; name: string }[]; lease_holder?: string | null };
export type WorkspaceSetupReceipt = {
  id: number; project_id: number; request_id: string; repo_path: string; branch: string; workspace_path: string | null;
  state: "pending" | "running" | "ready" | "failed" | "inspection"; detail: string; rev: number;
  steps: { command: string; passed: boolean; output: string; at: string }[];
};
export const workspaceSetups = (project: string) => api<WorkspaceSetupReceipt[]>(`/chat-workspace-setups?project=${encodeURIComponent(project)}`);
export const startWorkspaceSetup = (project: string, requestId: string, branch: string) => post<WorkspaceSetupReceipt>("/chat-workspace-setups", { project, request_id: requestId, branch, approved: true });
export const commandWorkspaceSetup = (project: string, receipt: WorkspaceSetupReceipt, action: "inspect" | "retry_dependencies") => post<WorkspaceSetupReceipt>(`/chat-workspace-setups/${receipt.id}`, { project, revision: receipt.rev, action });
export type WorkspaceRequest = { id: number; chat_id: number; after_node_id: number | null; from_path: string; to_path: string; state: "pending" | "applied" | "cancelled" };
export const chatWorkspaces = (project: string) => api<WorkspaceChoice[]>(`/chat-workspaces?project=${encodeURIComponent(project)}`);
export const workspaceRequests = (chat: number) => api<{ requests: WorkspaceRequest[] }>(`/chats/${chat}/workspace`);
export const requestWorkspace = (chat: number, path: string) => post<WorkspaceRequest>(`/chats/${chat}/workspace`, { path });
export const approveWorkspace = (chat: number, request: number, revision: number) => post<Chat>(`/chats/${chat}/workspace/approve`, { request, revision });
export const cancelWorkspace = (chat: number, request: number) => post<unknown>(`/chats/${chat}/workspace/cancel`, { request });
