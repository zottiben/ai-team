import { useId, useState } from "react";
import { SYNTH_PALETTES } from "../synth-palettes";
import { setSynthPalette, useSynthPalette } from "../ui-preferences";
import "./palettes.css";

/** Local paint only. Native radios retain keyboard and screen-reader selection. */
export default function PalettePicker() {
  const palette = useSynthPalette();
  const group = useId();
  const [persisted, setPersisted] = useState(true);
  return <fieldset className="synth-palettes">
    <legend>Colour palette</legend>
    <p className="synth-muted">Every surface, signal, editor and terminal. Works with Light, Dark or System.</p>
    <div className="synth-palettes__grid">
      {SYNTH_PALETTES.map(option => <label className="synth-palette" key={option.id}>
        <input type="radio" name={group} value={option.id} checked={palette === option.id}
          onChange={() => setPersisted(setSynthPalette(option.id))} />
        <span className="synth-palette__sample" data-synth="true" data-synth-palette={option.id} aria-hidden="true">
          <span className="synth-palette__mini"><i /><i /><i /><i /></span>
          <span className="synth-palette__swatches"><i /><i /><i /><i /><i /><i /></span>
        </span>
        <span className="synth-palette__caption"><strong>{option.name}</strong><small>{option.description}</small></span>
        <span className="synth-palette__selected" aria-hidden="true">{palette === option.id ? "✓" : ""}</span>
      </label>)}
    </div>
    {!persisted && <p role="status">Palette changed only in this window; local storage is unavailable.</p>}
    <p className="synth-muted">Original Synth plus adaptations inspired by <a href="https://vimcolorschemes.com/i/trending" target="_blank" rel="noreferrer">vimcolorschemes</a>.</p>
  </fieldset>;
}
