import { useEffect, useRef, useState } from "react";
import { Popover } from "../../Popover";
import { useAnnounce } from "../SynthProvider";
import { HwButton } from "./HwButton";

export function PresetSelector({ label, options, value, onChange, disabled = false, announceSelection = true }: {
  label: string; options: readonly { id: string; label: string }[]; value: string;
  onChange: (id: string) => void; disabled?: boolean; announceSelection?: boolean;
}) {
  const anchor = useRef<HTMLButtonElement>(null), list = useRef<HTMLDivElement>(null);
  const [open, setOpen] = useState(false);
  const announce = useAnnounce();
  const index = options.findIndex(option => option.id === value);
  const select = (next: number) => {
    const option = options[next];
    if (!option || disabled) return;
    onChange(option.id); if (announceSelection) announce(`${label}: ${option.label}`); setOpen(false); anchor.current?.focus();
  };
  useEffect(() => { if (disabled) setOpen(false); }, [disabled]);
  return <div className="synth-preset">
    <HwButton aria-label={`Previous ${label}`} disabled={disabled || !options.length}
      onClick={() => select((Math.max(0, index) - 1 + options.length) % options.length)}>‹</HwButton>
    <button ref={anchor} type="button" className="synth-preset__value" aria-label={label}
      aria-haspopup="listbox" aria-expanded={open} disabled={disabled || !options.length}
      onClick={() => setOpen(!open)}>{options[index]?.label ?? value}</button>
    <HwButton aria-label={`Next ${label}`} disabled={disabled || !options.length}
      onClick={() => select((index + 1) % options.length)}>›</HwButton>
    {open && !disabled && <Popover anchor={anchor} label={`${label} choices`} onClose={() => setOpen(false)} width={260} className="synth-preset-popover" initialFocus={index >= 0 ? '[role="option"][aria-selected="true"]' : '[role="option"]'}>
      <div ref={list} role="listbox" aria-label={label} onKeyDown={event => {
        const buttons = [...list.current?.querySelectorAll<HTMLButtonElement>("button") ?? []];
        const focused = buttons.indexOf(document.activeElement as HTMLButtonElement);
        const to = event.key === "ArrowDown" ? (focused + 1) % buttons.length : event.key === "ArrowUp" ? (focused - 1 + buttons.length) % buttons.length
          : event.key === "Home" ? 0 : event.key === "End" ? buttons.length - 1 : -1;
        if (to >= 0) { event.preventDefault(); buttons[to]?.focus(); }
      }}>
        {options.map((option, i) => <button key={option.id} type="button" role="option" aria-selected={option.id === value}
          className="synth-key" onClick={() => select(i)}>{option.label}</button>)}
      </div>
    </Popover>}
  </div>;
}
