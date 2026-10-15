import { useSyncExternalStore } from "react";

const SYNTH = "ui.synth";
const CHANGED = "ai-team:ui-preferences";

export function storedSynth(): boolean {
  try { return localStorage.getItem(SYNTH) === "true"; }
  catch { return false; }
}

/** Applied before React; scoped tokens also reach portals under document.body. */
export function initializeSynth(): void {
  document.documentElement.dataset.synth = String(storedSynth());
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
    if (event.key === SYNTH || event.key === EFFECTS || event.key === null) {
      initializeSynth();
      changed();
    }
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
