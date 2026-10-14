import { useId } from "react";

export function Switch({ label, checked, onChange, disabled = false }: {
  label: string; checked: boolean; onChange: (checked: boolean) => void; disabled?: boolean;
}) {
  const id = useId();
  return <label className="synth-switch" htmlFor={id}>
    <input id={id} type="checkbox" role="switch" checked={checked} aria-checked={checked}
      disabled={disabled} onChange={e => { if (!disabled) onChange(e.target.checked); }} />
    <span className="synth-switch__track" aria-hidden="true"><span /></span>
    <span>{label}</span>
  </label>;
}
