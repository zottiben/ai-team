import { api } from "./api";
import type { ChatPanel } from "./ChatContext";
export type ChatTodayEntry = {
  chat_id: number;
  project_slug: string;
  project_name: string;
  title: string;
  workspace_path: string;
  updated_at: string;
  state: string;
  detail: string | null;
  panel: ChatPanel;
  needs_attention: boolean;
  working: boolean;
  drafts: number;
  questions: number;
};
export type ChatToday = {
  entries: ChatTodayEntry[];
  chats: number;
  needs_attention: number;
  working: number;
  drafts: number;
};
export const chatToday = () => api<ChatToday>("/chat-today");
