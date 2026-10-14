import { fireEvent, render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { useState } from "react";
import { expect, it, vi } from "vitest";
import { SynthProvider } from "../SynthProvider";
import { TabStrip } from "./TabStrip";
import { PresetSelector } from "./PresetSelector";
import { Switch } from "./Switch";
import { HwButton } from "./HwButton";
import { SegmentMeter } from "./SegmentMeter";

it("roves tabs without selecting until activation, skipping disabled tabs", async () => {
  const user = userEvent.setup(), change = vi.fn();
  render(<TabStrip label="View" value="overview" onChange={change} tabs={[
    { id: "overview", label: "Overview", panel: "o" }, { id: "review", label: "Review", panel: "r", disabled: true }, { id: "work", label: "Work", panel: "w" },
  ]} />);
  screen.getByRole("tab", { name: "Overview" }).focus();
  await user.keyboard("{ArrowRight}");
  expect(document.activeElement).toBe(screen.getByRole("tab", { name: "Work" }));
  expect(change).not.toHaveBeenCalled();
  await user.keyboard("{Enter}"); expect(change).toHaveBeenCalledExactlyOnceWith("work");
  await user.keyboard("{Home}"); expect(document.activeElement).toBe(screen.getByRole("tab", { name: "Overview" }));
});

it("a preset wraps, exposes its selected option, returns focus, and uses one announcement stream", async () => {
  const user = userEvent.setup();
  function Preset() {
    const [value, setValue] = useState("b");
    return <SynthProvider><PresetSelector label="View" value={value} onChange={setValue} options={[{ id: "a", label: "First" }, { id: "b", label: "Second" }]} /></SynthProvider>;
  }
  render(<Preset />);
  await user.click(screen.getByRole("button", { name: "View" }));
  expect(document.activeElement).toBe(screen.getByRole("option", { name: "Second" }));
  await user.keyboard("{Escape}");
  expect(document.activeElement).toBe(screen.getByRole("button", { name: "View" }));
  expect(screen.queryByRole("listbox")).toBeNull();
  await user.click(screen.getByRole("button", { name: "Next View" }));
  expect(screen.getByRole("button", { name: "View" }).textContent).toBe("First");
  expect(document.querySelector('[aria-live="polite"]')?.textContent).toBe("View: First");
  expect(document.querySelectorAll('[aria-live="polite"]')).toHaveLength(1);
});

it("native switches work by keyboard and locked keys state their reason", async () => {
  const change = vi.fn(), user = userEvent.setup();
  render(<><Switch label="Preview" checked={false} onChange={change} /><HwButton lockedReason="Agent is working">Commit</HwButton></>);
  screen.getByRole("switch", { name: "Preview" }).focus(); await user.keyboard(" "); expect(change).toHaveBeenCalledWith(true);
  expect(screen.getByRole("button", { name: "Commit" }).hasAttribute("disabled")).toBe(true);
  expect(screen.getByText("Agent is working")).toBeDefined();
});

it("caps meter bars without losing the exact count or unknown state", () => {
  const view = render(<SegmentMeter label="Edits" value={42} />);
  expect(within(view.container).getByText("Edits · 42")).toBeDefined();
  expect(view.container.querySelectorAll('[data-lit="true"]')).toHaveLength(6);
  view.rerender(<SegmentMeter label="Edits" value={NaN} />);
  expect(screen.getByText("Edits · unavailable")).toBeDefined();
  expect(view.container.querySelectorAll('[data-lit="true"]')).toHaveLength(0);
});

it("does not activate a disabled switch", () => {
  const change = vi.fn(); render(<Switch label="Policy" checked disabled onChange={change} />);
  fireEvent.click(screen.getByRole("switch")); expect(change).not.toHaveBeenCalled();
});
