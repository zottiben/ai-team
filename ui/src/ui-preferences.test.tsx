import { useEffect } from "react";
import { act, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, expect, it, vi } from "vitest";
import { initializeSynth, setSynth, storedSynth, synthEnabled } from "./ui-preferences";
import { SynthToggle } from "./SynthToggle";

afterEach(() => { vi.restoreAllMocks(); localStorage.clear(); delete document.documentElement.dataset.synth; });

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
