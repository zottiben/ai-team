import { useEffect } from "react";
import { act, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, expect, it, vi } from "vitest";
import { initializeSynth, setSynth, storedSynth, synthEnabled, synthPalette, setSynthPalette, useSynthPalette } from "./ui-preferences";
import { SynthToggle } from "./SynthToggle";

afterEach(() => { vi.restoreAllMocks(); localStorage.clear(); delete document.documentElement.dataset.synth; delete document.documentElement.dataset.synthPalette; });

it("defaults off, accepts only true and keeps the old theme", () => {
  expect(storedSynth()).toBe(false);
  localStorage.setItem("ui.synth", "yes"); expect(storedSynth()).toBe(false);
  document.documentElement.dataset.theme = "light";
  localStorage.setItem("ai-team.theme", "system");
  setSynth(true); initializeSynth();
  expect(synthEnabled()).toBe(true);
  expect(document.documentElement.dataset.theme).toBe("light");
  expect(localStorage.getItem("ai-team.theme")).toBe("system");
  setSynth(false); expect(synthEnabled()).toBe(false);
});

it("switches without remounting stateful siblings and updates both controls", () => {
  const mounted = vi.fn(), unmounted = vi.fn();
  function Draft() { useEffect(() => { mounted(); return unmounted; }, []); return <textarea aria-label="draft" />; }
  initializeSynth();
  render(<><Draft /><SynthToggle /><SynthToggle /></>);
  fireEvent.change(screen.getByLabelText("draft"), { target: { value: "unsent" } });
  fireEvent.click(screen.getAllByRole("switch")[0]!);
  expect(screen.getAllByRole("switch").every(el => el.getAttribute("aria-checked") === "true")).toBe(true);
  expect((screen.getByLabelText("draft") as HTMLTextAreaElement).value).toBe("unsent");
  expect(mounted).toHaveBeenCalledTimes(1); expect(unmounted).not.toHaveBeenCalled();
});

it("survives unavailable storage and explains the in-memory fallback", () => {
  vi.spyOn(Storage.prototype, "getItem").mockImplementation(() => { throw new Error("denied"); });
  expect(storedSynth()).toBe(false); initializeSynth();
  vi.spyOn(Storage.prototype, "setItem").mockImplementation(() => { throw new Error("full"); });
  render(<SynthToggle />); fireEvent.click(screen.getByRole("switch"));
  expect(synthEnabled()).toBe(true);
  expect(screen.getByRole("status").textContent).toContain("only in this window");
});

it("initializes a validated palette before React without changing mode or opting in", () => {
  initializeSynth();
  expect(synthPalette()).toBe("vice-city");
  localStorage.setItem("ui.synth.palette", "unknown"); initializeSynth();
  expect(document.documentElement.dataset.synthPalette).toBe("vice-city");
  localStorage.setItem("ui.synth.palette", "everforest");
  localStorage.setItem("ai-team.theme", "system");
  document.documentElement.dataset.theme = "light";
  initializeSynth();
  expect(synthPalette()).toBe("everforest");
  setSynthPalette("gruvbox");
  expect(localStorage.getItem("ui.synth.palette")).toBe("gruvbox");
  expect(synthEnabled()).toBe(false);
  expect(document.documentElement.dataset.theme).toBe("light");
  expect(localStorage.getItem("ai-team.theme")).toBe("system");
  setSynth(true); setSynth(false); setSynth(true);
  expect(synthPalette()).toBe("gruvbox");
});

it("updates palette subscribers and follows cross-window storage without remounting", () => {
  function Selection() { return <output>{useSynthPalette()}</output>; }
  initializeSynth(); render(<><Selection /><textarea aria-label="draft" /></>);
  const draft = screen.getByLabelText("draft");
  fireEvent.change(draft, { target: { value: "retained" } });
  act(() => { setSynthPalette("catppuccin"); });
  expect(screen.getByText("catppuccin")).toBeDefined();
  act(() => {
    localStorage.setItem("ui.synth.palette", "kanagawa");
    window.dispatchEvent(new StorageEvent("storage", { key: "ui.synth.palette" }));
  });
  expect(screen.getByText("kanagawa")).toBeDefined();
  expect(screen.getByLabelText("draft")).toBe(draft);
  expect((draft as HTMLTextAreaElement).value).toBe("retained");
  act(() => { localStorage.clear(); window.dispatchEvent(new StorageEvent("storage", { key: null })); });
  expect(screen.getByText("vice-city")).toBeDefined();
});

it("keeps an in-memory palette through unrelated storage events when persistence fails", () => {
  vi.spyOn(Storage.prototype, "getItem").mockImplementation(() => { throw Error("denied"); });
  vi.spyOn(Storage.prototype, "setItem").mockImplementation(() => { throw Error("full"); });
  initializeSynth(); render(<SynthToggle />);
  act(() => { expect(setSynthPalette("tokyo-night")).toBe(false); });
  window.dispatchEvent(new StorageEvent("storage", { key: "ui.synth.screen-effects" }));
  expect(synthPalette()).toBe("tokyo-night");
});

it("follows external storage changes without resetting the app", () => {
  initializeSynth(); render(<SynthToggle />);
  act(() => {
    localStorage.setItem("ui.synth", "true");
    window.dispatchEvent(new StorageEvent("storage", { key: "ui.synth" }));
  });
  expect(screen.getByRole("switch").getAttribute("aria-checked")).toBe("true");
  act(() => { localStorage.clear(); window.dispatchEvent(new StorageEvent("storage", { key: null })); });
  expect(screen.getByRole("switch").getAttribute("aria-checked")).toBe("false");
});
