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

function subscribe(changed: () => void): () => void {
  const storage = (event: StorageEvent) => {
    if (event.key === SYNTH || event.key === null) {
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
