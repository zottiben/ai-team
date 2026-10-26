import { useState } from "react";
import { setSynth, useSynth } from "./ui-preferences";

export function SynthToggle() {
  const enabled = useSynth();
  const [temporary, setTemporary] = useState(false);
  return <div>
    <label className="settings__toggle">
      <input type="checkbox" role="switch" checked={enabled} aria-checked={enabled}
        onChange={event => setTemporary(!setSynth(event.target.checked))} />
      Synth UI preview
    </label>
    {temporary && <p role="status" className="faint">Storage is unavailable. This choice lasts only in this window.</p>}
  </div>;
}
