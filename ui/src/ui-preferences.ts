import { useSyncExternalStore } from "react";
import { validSynthPalette, type SynthPalette } from "./synth-palettes";

const SYNTH = "ui.synth";
const PALETTE = "ui.synth.palette";
const CHANGED = "ai-team:ui-preferences";

export function storedSynth(): boolean {
  try { return localStorage.getItem(SYNTH) === "true"; }
  catch { return false; }
}

/** Applied before React; scoped tokens also reach portals under document.body. */
export function initializeSynth(): void {
  document.documentElement.dataset.synth = String(storedSynth());
  document.documentElement.dataset.synthPalette = storedPalette();
}

export function synthEnabled(): boolean {
  const value = document.documentElement.dataset.synth;
  return value === undefined ? storedSynth() : value === "true";
}

/** Returns false if persistence failed. The switch still works for this window. */
export function setSynth(enabled: boolean): boolean {
  document.documentElement.dataset.synth = String(enabled);
  let persisted = true;
  try { localStorage.setItem(SYNTH, String(enabled)); }
  catch { persisted = false; }
  window.dispatchEvent(new Event(CHANGED));
  return persisted;
}

function storedPalette(): SynthPalette {
  try {
    const value = localStorage.getItem(PALETTE);
    return validSynthPalette(value) ? value : "vice-city";
  } catch { return "vice-city"; }
}

export function synthPalette(): SynthPalette {
  const value = document.documentElement.dataset.synthPalette;
  return validSynthPalette(value) ? value : storedPalette();
}

/** The palette does not opt into Synth or change Light/Dark/System. */
export function setSynthPalette(palette: SynthPalette): boolean {
  if (!validSynthPalette(palette)) return false;
  document.documentElement.dataset.synthPalette = palette;
  let persisted = true;
  try { localStorage.setItem(PALETTE, palette); }
  catch { persisted = false; }
  window.dispatchEvent(new Event(CHANGED));
  return persisted;
}

export function useSynthPalette(): SynthPalette {
  return useSyncExternalStore(subscribe, synthPalette, () => "vice-city");
}

/**
 * Whether the terminal paints its scanlines and vignette.
 *
 * An explicit switch rather than a guess: there is no browser API that reliably says a
 * machine is on battery or thermally limited, so "pause them on low-power devices" has
 * to be somebody's choice. They are pure CSS and cheap, which is why the default is on.
 */
const EFFECTS = "ui.synth.screen-effects";

export function screenEffects(): boolean {
  try { return localStorage.getItem(EFFECTS) !== "false"; }
  catch { return true; }
}

/** Returns false if persistence failed. The switch still works for this window. */
export function setScreenEffects(enabled: boolean): boolean {
  let persisted = true;
  try { localStorage.setItem(EFFECTS, String(enabled)); }
  catch { persisted = false; }
  window.dispatchEvent(new Event(CHANGED));
  return persisted;
}

export function useScreenEffects(): boolean {
  return useSyncExternalStore(subscribe, screenEffects, () => true);
}

function subscribe(changed: () => void): () => void {
  const storage = (event: StorageEvent) => {
    if (event.key === SYNTH || event.key === null) document.documentElement.dataset.synth = String(storedSynth());
    if (event.key === PALETTE || event.key === null) document.documentElement.dataset.synthPalette = storedPalette();
    if (event.key === SYNTH || event.key === PALETTE || event.key === EFFECTS || event.key === null) changed();
  };
  window.addEventListener(CHANGED, changed);
  window.addEventListener("storage", storage);
  return () => {
    window.removeEventListener(CHANGED, changed);
    window.removeEventListener("storage", storage);
  };
}

export function useSynth(): boolean {
  return useSyncExternalStore(subscribe, synthEnabled, () => false);
}
