import { useEffect, useLayoutEffect, useRef, type RefObject } from "react";
import type { ChatPanel } from "../../../ChatContext";

export function shortcut(event: Pick<KeyboardEvent, "key" | "code" | "metaKey" | "ctrlKey" | "shiftKey" | "altKey" | "repeat" | "isComposing">): ChatPanel | "next" | "previous" | "editor" | "terminal" | "steer" | null {
  if (event.repeat || event.isComposing) return null;
  if (event.ctrlKey && !event.metaKey && !event.altKey && event.key === "Tab") return event.shiftKey ? "previous" : "next";
  if (!(event.metaKey || event.ctrlKey) || event.shiftKey) return null;
  if (event.key === "Enter" && event.altKey) return "steer";
  if (event.altKey) return null;
  const panels: Record<string, ChatPanel> = { "1": "overview", "2": "review", "3": "board", "4": "work" };
  return panels[event.key] ?? (event.key.toLowerCase() === "e" ? "editor" : event.key === "`" || event.code === "Backquote" ? "terminal" : null);
}
export default function Shortcuts({ host, panel, onPanel, onSteer }: { host: RefObject<HTMLElement | null>; panel: ChatPanel | null; onPanel: (panel: ChatPanel) => void; onSteer: () => void }) {
  const callbacks = useRef({ panel, onPanel, onSteer });
  useLayoutEffect(() => { callbacks.current = { panel, onPanel, onSteer }; }, [panel, onPanel, onSteer]);
  useEffect(() => {
    const key = (event: KeyboardEvent) => {
      const root = host.current, action = shortcut(event), target = event.target;
      if (!action || event.defaultPrevented || document.hidden || !root?.getClientRects().length || target instanceof Element && target.closest('[role="dialog"], [role="alertdialog"], [role="listbox"], [role="menu"]')) return;
      event.preventDefault(); event.stopPropagation();
      const { panel, onPanel, onSteer } = callbacks.current;
      if (action === "steer") { onSteer(); return; } // Focus the existing confirmation action; never execute from the shortcut.
      if (action === "editor" || action === "terminal") { root.querySelector<HTMLButtonElement>(`.chat-header-actions button[data-chat-tool="${action}"]`)?.click(); return; }
      if (action === "next" || action === "previous") {
        const order: ChatPanel[] = ["overview", "review", "board", "work"], index = panel === null ? action === "next" ? -1 : 0 : order.indexOf(panel);
        onPanel(order[(index + (action === "next" ? 1 : -1) + order.length) % order.length]!);
      } else onPanel(action);
    };
    document.addEventListener("keydown", key, true);
    return () => document.removeEventListener("keydown", key, true);
  }, [host]);
  return null;
}
