import { CHAT_PANELS, type ChatPanel } from "../../../ChatContext";
import { SynthProvider } from "../../SynthProvider";
import { PresetSelector } from "../../primitives/PresetSelector";
import { TabStrip } from "../../primitives/TabStrip";
import "../Shell/shell.css";

export function SessionTabs({ panel, disabled, onChange }: { panel: ChatPanel | null; disabled: boolean; onChange: (panel: ChatPanel) => void }) {
  const tabs = (Object.keys(CHAT_PANELS) as ChatPanel[]).map(name => ({ id: `chat-context-${name}`, panel: `chat-pane-${name}`, label: CHAT_PANELS[name], disabled }));
  return <TabStrip label="Session views" tabs={tabs} value={panel ? `chat-context-${panel}` : null} onChange={id => {
    const name = (Object.keys(CHAT_PANELS) as ChatPanel[]).find(name => id === `chat-context-${name}`);
    if (name) onChange(name);
  }} />;
}
export function HeaderIcon({ name }: { name: "editor" | "terminal" | "rename" | "archive" }) {
  const paths = {
    editor: "m8.5 7.5-5 4.5 5 4.5M15.5 7.5l5 4.5-5 4.5",
    terminal: "M5 5h14a2 2 0 0 1 2 2v10a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2V7a2 2 0 0 1 2-2Zm2.5 5L10 12l-2.5 2M12.5 15h4",
    rename: "m15.5 4.5 4 4L9 19l-5 1 1-5Z",
    archive: "M4 5h16v4H4ZM5.5 9v10h13V9M10 13h4",
  };
  return <svg width="15" height="15" viewBox="0 0 24 24" aria-hidden="true" fill="none" stroke="currentColor" strokeWidth="1.7" strokeLinecap="round" strokeLinejoin="round"><path d={paths[name]}/></svg>;
}
const VIEWS = ["overview", "review", "work"] as const;
export function PanelSelector({ panel, onChange }: { panel: ChatPanel; onChange: (panel: ChatPanel) => void }) {
  return <SynthProvider><PresetSelector label="Panel view" value={panel} options={VIEWS.map(id => ({ id, label: CHAT_PANELS[id] }))}
    onChange={id => { const next = VIEWS.find(name => name === id); if (next) onChange(next); }} /></SynthProvider>;
}
