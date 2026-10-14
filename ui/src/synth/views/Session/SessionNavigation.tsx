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
const VIEWS = ["overview", "review", "work"] as const;
export function PanelSelector({ panel, onChange }: { panel: ChatPanel; onChange: (panel: ChatPanel) => void }) {
  return <SynthProvider><PresetSelector label="Panel view" value={panel} options={VIEWS.map(id => ({ id, label: CHAT_PANELS[id] }))}
    onChange={id => { const next = VIEWS.find(name => name === id); if (next) onChange(next); }} /></SynthProvider>;
}
