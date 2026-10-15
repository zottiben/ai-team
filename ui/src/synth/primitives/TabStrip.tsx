import { useRef } from "react";
import { Led } from "./Led";

export interface TabOption { id: string; label: string; panel: string; disabled?: boolean; badge?: number }
export function TabStrip({ label, tabs, value, onChange }: {
  label: string; tabs: readonly TabOption[]; value: string | null; onChange: (id: string) => void;
}) {
  const host = useRef<HTMLDivElement>(null);
  const enabled = tabs.filter(t => !t.disabled);
  const focusable = enabled.some(tab => tab.id === value) ? value : enabled[0]?.id;
  return <div ref={host} className="synth-tabs" role="tablist" aria-label={label} onKeyDown={event => {
    const buttons = [...host.current?.querySelectorAll<HTMLButtonElement>('button:not(:disabled)') ?? []];
    const index = buttons.indexOf(document.activeElement as HTMLButtonElement);
    if (index < 0 || !buttons.length) return;
    const target = event.key === "ArrowRight" ? (index + 1) % buttons.length
      : event.key === "ArrowLeft" ? (index + buttons.length - 1) % buttons.length
      : event.key === "Home" ? 0 : event.key === "End" ? buttons.length - 1 : -1;
    if (target >= 0) {
      event.preventDefault();
      for (const [i, button] of buttons.entries()) button.tabIndex = i === target ? 0 : -1;
      buttons[target]?.focus();
    }
  }}>
    {tabs.map(tab => <button key={tab.id} id={tab.id} type="button" role="tab" className="synth-key"
      aria-selected={value === tab.id} aria-controls={tab.panel} disabled={tab.disabled}
      tabIndex={focusable === tab.id ? 0 : -1}
      onClick={() => onChange(tab.id)}><Led state={value === tab.id ? "on" : "off"} size={6} />{tab.label}{tab.badge !== undefined && <span className="synth-tab-badge">{tab.badge}</span>}</button>)}
  </div>;
}
