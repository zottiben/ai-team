import { useRef } from "react";
import { act, cleanup, render } from "@testing-library/react";
import { afterEach, expect, it, vi } from "vitest";
import { useTicker } from "./useTicker";
import { Led } from "../primitives/Led";
import { Cable } from "../primitives/Cable";
import { SynthProvider } from "../SynthProvider";

function environment() {
  let reduced = false, id = 0;
  const listeners = new Set<() => void>(), frames = new Map<number, FrameRequestCallback>();
  const observers: { target: Element | null; change: (visible: boolean) => void; disconnect: ReturnType<typeof vi.fn> }[] = [];
  vi.stubGlobal("matchMedia", () => ({ matches: reduced,
    addEventListener: (_name: string, fn: () => void) => listeners.add(fn),
    removeEventListener: (_name: string, fn: () => void) => listeners.delete(fn),
  }));
  vi.stubGlobal("requestAnimationFrame", (callback: FrameRequestCallback) => { frames.set(++id, callback); return id; });
  vi.stubGlobal("cancelAnimationFrame", (handle: number) => frames.delete(handle));
  vi.stubGlobal("IntersectionObserver", class {
    target: Element | null = null;
    disconnect = vi.fn();
    constructor(callback: IntersectionObserverCallback) {
      this.change = visible => {
        if (!this.target) return;
        const rect = this.target.getBoundingClientRect();
        const entry: IntersectionObserverEntry = { target: this.target, time: 0, isIntersecting: visible, intersectionRatio: visible ? 1 : 0,
          boundingClientRect: rect, intersectionRect: rect, rootBounds: rect };
        callback([entry], this as unknown as IntersectionObserver);
      };
      observers.push(this);
    }
    change: (visible: boolean) => void;
    observe(target: Element) { this.target = target; }
  });
  return { frames, observers, listeners,
    visible(value: boolean) { act(() => observers.forEach(o => o.change(value))); },
    reduced(value: boolean) { act(() => { reduced = value; [...listeners].forEach(fn => fn()); }); },
    frame(now: number) { act(() => { const callbacks = [...frames.values()]; frames.clear(); callbacks.forEach(cb => cb(now)); }); },
  };
}
afterEach(() => { cleanup(); vi.restoreAllMocks(); vi.unstubAllGlobals(); });

function Visual({ tick }: { tick: () => void }) {
  const host = useRef<HTMLDivElement>(null);
  useTicker(host, tick, 30);
  return <div ref={host} />;
}

it("stops all rAF work offscreen, hidden, reduced or unmounted and releases observers", () => {
  const env = environment(), tick = vi.fn();
  const view = render(<Visual tick={tick} />);
  expect(env.frames.size).toBe(0);
  env.visible(true); expect(env.frames.size).toBe(1);
  env.frame(0); expect(tick).toHaveBeenCalledTimes(1);
  env.visible(false); expect(env.frames.size).toBe(0);
  env.visible(true);
  const hidden = vi.spyOn(document, "hidden", "get").mockReturnValue(true);
  act(() => document.dispatchEvent(new Event("visibilitychange"))); expect(env.frames.size).toBe(0);
  hidden.mockReturnValue(false);
  act(() => document.dispatchEvent(new Event("visibilitychange"))); expect(env.frames.size).toBe(1);
  env.reduced(true); expect(env.frames.size).toBe(0);
  env.reduced(false); env.visible(true); expect(env.frames.size).toBe(1);
  view.unmount(); expect(env.frames.size).toBe(0);
  expect(env.observers.every(o => o.disconnect.mock.calls.length === 1)).toBe(true);
  expect(env.listeners.size).toBe(0);
});

it("keeps CSS pulses gated too; only one visible attention LED owns the loop", () => {
  const env = environment();
  const view = render(<SynthProvider><Led state="attention" /><Led state="attention" /><Led state="pulse" /></SynthProvider>);
  env.visible(true);
  const leds = [...view.container.querySelectorAll<HTMLElement>(".synth-led")];
  expect(leds.filter(el => el.dataset.state === "attention" && el.dataset.motion === "true")).toHaveLength(1);
  act(() => env.observers[0]?.change(false));
  expect(leds[0]?.dataset.motion).toBe("false"); expect(leds[1]?.dataset.motion).toBe("true");
  env.reduced(true);
  expect(leds.every(el => el.dataset.motion === "false")).toBe(true);
});

it("restarts a flash for a distinct event even when the event state does not change", () => {
  environment();
  const view = render(<Led state="event" eventId={1} />);
  const first = view.container.querySelector(".synth-led");
  view.rerender(<Led state="event" eventId={2} />);
  expect(view.container.querySelector(".synth-led")).not.toBe(first);
});

it("settles cables under reduced motion without a frame and stops completed springs", () => {
  const env = environment();
  const view = render(<svg><Cable from={{ x: 0, y: 0 }} to={{ x: 100, y: 0 }} connected eventId={1} /></svg>);
  env.visible(true); env.frame(0); env.frame(500); env.frame(1300);
  expect(env.frames.size).toBe(0);
  expect(view.container.querySelector("path")?.getAttribute("d")).toBe("M0 0 C12 45 88 45 100 0");
  env.reduced(true);
  view.rerender(<svg><Cable from={{ x: 0, y: 0 }} to={{ x: 100, y: 0 }} connected eventId={2} /></svg>);
  expect(env.frames.size).toBe(0);
  expect(view.container.querySelector("path")?.getAttribute("d")).toBe("M0 0 C12 45 88 45 100 0");
});
