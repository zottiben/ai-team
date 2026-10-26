import { useSyncExternalStore } from "react";
import type { ChatDetail } from "../../chat-api";
import type { RunEvent } from "../../api";

export type ChatDisplay = { detail: ChatDetail; events: RunEvent[] };
let current: { owner: symbol; value: ChatDisplay } | null = null;
const listeners = new Set<() => void>();
const notify = () => listeners.forEach(listener => listener());
/** Ephemeral references from the existing chat controller; no polling, copying or persistence. */
export function publishChatDisplay(owner: symbol, value: ChatDisplay) { current = { owner, value }; notify(); }
export function releaseChatDisplay(owner: symbol) { if (current?.owner === owner) { current = null; notify(); } }
const subscribe = (listener: () => void) => { listeners.add(listener); return () => { listeners.delete(listener); }; };
const snapshot = () => current?.value ?? null;
export function useChatDisplay(chat: number | null, project: number | null) {
  const value = useSyncExternalStore(subscribe, snapshot);
  return value?.detail.id === chat && value.detail.project_id === project ? value : null;
}
