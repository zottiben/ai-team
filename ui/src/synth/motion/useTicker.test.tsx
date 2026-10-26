import { useRef } from "react";
import { act, cleanup, render } from "@testing-library/react";
import { afterEach, expect, it, vi } from "vitest";
import { useTicker } from "./useTicker";
import { Led } from "../primitives/Led";
import { Cable } from "../primitives/Cable";
import { Scope } from "../primitives/Scope";
import { SynthProvider } from "../SynthProvider";
import ActivityEqualizer from "../views/Session/ActivityEqualizer";

function environment() {
  let reduced = false, id = 0;
  const listeners = new Set<() => void>(), frames = new Map<number, FrameRequestCallback>();
  const observers: { target: Element | null; change: (visible: boolean | boolean[]) => void; disconnect: ReturnType<typeof vi.fn> }[] = [];
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
        const entry: IntersectionObserverEntry = { target: this.target, time: 0, isIntersecting: false, intersectionRatio: 0,
          boundingClientRect: rect, intersectionRect: rect, rootBounds: rect };
        callback((Array.isArray(visible) ? visible : [visible]).map(value => ({ ...entry, isIntersecting: value, intersectionRatio: value ? 1 : 0 })), this as unknown as IntersectionObserver);
      };
      observers.push(this);
    }
    change: (visible: boolean | boolean[]) => void;
    observe(target: Element) { this.target = target; }
  });
  return { frames, observers, listeners,
    visible(value: boolean) { act(() => observers.forEach(o => o.change(value))); },
    reduced(value: boolean) { act(() => { reduced = value; [...listeners].forEach(fn => fn()); }); },
    frame(now: number) { act(() => { const callbacks = [...frames.values()]; frames.clear(); callbacks.forEach(cb => cb(now)); }); },
  };
}
afterEach(() => { cleanup(); vi.useRealTimers(); vi.restoreAllMocks(); vi.unstubAllGlobals(); });

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

it("uses the latest entry when a visibility batch hides and reveals the same owner", () => {
 const env=environment(),tick=vi.fn();const view=render(<Visual tick={tick}/>);
 act(()=>env.observers[0]?.change([false,true]));expect(env.frames.size).toBe(1);
 act(()=>env.observers[0]?.change([true,false]));expect(env.frames.size).toBe(0);view.unmount();
});

it("scrolls a reported line without new data, freezes hidden/reduced owners and resets across nodes", () => {
 const env=environment();const view=render(<Scope label="Tokens" unit="tokens" samples={[10,20,5]} flow active identity={1}/>);
 env.visible(true);env.visible(true);env.frame(0);const path=()=>view.container.querySelector('.synth-graph__line')?.getAttribute('d');
 const first=path();env.frame(100);expect(path()).not.toBe(first);
 env.visible(false);expect(env.frames.size).toBe(0);const frozen=path();env.frame(1000);expect(path()).toBe(frozen);
 env.visible(true);env.visible(true);env.frame(2000);expect(path()).toBe(frozen);
 env.reduced(true);expect(env.frames.size).toBe(0);
 view.rerender(<Scope label="Tokens" unit="tokens" samples={[]} flow active identity={2}/>);expect(path()).toBe('');
 view.unmount();expect(env.frames.size).toBe(0);
});

it("never runs an inactive flow, and freezes immediately when its chat stops", () => {
  const env = environment(), samples = [10, 20, 5];
  const view = render(<Scope label="Tokens" unit="tokens" samples={samples} flow active={false}/>);
  env.visible(true); env.visible(true);
  expect(env.frames.size).toBe(0);
  view.rerender(<Scope label="Tokens" unit="tokens" samples={samples} flow active/>);
  env.frame(0); env.frame(100);
  const graph = view.container.querySelector<HTMLElement>(".synth-graph")!;
  const phase = graph.style.backgroundPositionX;
  view.rerender(<Scope label="Tokens" unit="tokens" samples={samples} flow active={false}/>);
  expect(env.frames.size).toBe(0);
  env.frame(1000); expect(graph.style.backgroundPositionX).toBe(phase);
  view.rerender(<Scope label="Tokens" unit="tokens" samples={[10, 20, 8]} flow active={false}/>);
  expect(graph.textContent).toContain("8 tokens");
  expect(env.frames.size).toBe(0);
});

it("keeps reduced-motion readouts discrete and stops their clock offscreen or hidden", () => {
  vi.useFakeTimers();
  const env = environment(), read = vi.fn(() => [10, 20]);
  env.reduced(true);
  const view = render(<Scope label="Rate" unit="tok/s" samples={[]} read={read} active />);
  const initial = read.mock.calls.length;
  expect(vi.getTimerCount()).toBe(0);
  env.visible(true); act(() => { vi.advanceTimersByTime(1000); });
  expect(read).toHaveBeenCalledTimes(initial + 1); expect(env.frames.size).toBe(0);
  env.visible(false); expect(vi.getTimerCount()).toBe(0);
  act(() => { vi.advanceTimersByTime(10_000); }); expect(read).toHaveBeenCalledTimes(initial + 1);
  env.visible(true); expect(vi.getTimerCount()).toBe(1);
  vi.spyOn(document, "hidden", "get").mockReturnValue(true);
  act(() => document.dispatchEvent(new Event("visibilitychange"))); expect(vi.getTimerCount()).toBe(0);
  view.unmount();
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

it("does not replay an old LED event flash when it scrolls back into view", () => {
  vi.useFakeTimers(); const env = environment();
  const view = render(<Led state="event" eventId={1} />);
  env.visible(true);
  expect(view.container.querySelector(".synth-led")?.getAttribute("data-motion")).toBe("true");
  act(() => { vi.advanceTimersByTime(600); });
  env.visible(false); env.visible(true);
  expect(view.container.querySelector(".synth-led")?.getAttribute("data-motion")).toBe("false");
  view.rerender(<Led state="event" eventId={2} />);
  expect(view.container.querySelector(".synth-led")?.getAttribute("data-motion")).toBe("true");
  view.unmount(); expect(vi.getTimerCount()).toBe(0);
});

it("animates decorative EQ segments at20Hz only while active and clears them on stop", () => {
  const env = environment(), view = render(<ActivityEqualizer active={false}/>);
  const pattern = () => [...view.container.querySelectorAll("rect")].map(rect => `${rect.dataset.lit}/${rect.dataset.peak}`).join();
  const idle = pattern();
  expect(view.container.querySelectorAll("rect")).toHaveLength(72);
  env.visible(true); expect(env.frames.size).toBe(0);
  view.rerender(<ActivityEqualizer active/>); env.visible(true); env.frame(0);
  const initial = pattern(); expect(initial).not.toBe(idle);
  env.frame(20); expect(pattern()).toBe(initial);
  env.frame(100); env.frame(200); expect(pattern()).not.toBe(initial);
  view.rerender(<ActivityEqualizer active={false}/>);
  expect(env.frames.size).toBe(0); expect(pattern()).toBe(idle);
  env.frame(2000); expect(pattern()).toBe(idle);
  view.unmount(); expect(env.frames.size).toBe(0);
});

it("keeps a static working EQ with reduced motion and freezes offscreen or document-hidden", () => {
  const env = environment(); env.reduced(true);
  const view = render(<ActivityEqualizer active/>);
  const pattern = () => view.container.innerHTML;
  const still = pattern(); env.visible(true); env.frame(1000);
  expect(env.frames.size).toBe(0); expect(pattern()).toBe(still);
  expect(view.container.querySelector('[data-lit="true"]')).not.toBeNull();
  env.reduced(false); env.visible(true); env.frame(0); env.frame(100);
  env.visible(false); expect(env.frames.size).toBe(0);
  const hidden = pattern(); env.frame(1000); expect(pattern()).toBe(hidden);
  env.visible(true);
  vi.spyOn(document, "hidden", "get").mockReturnValue(true);
  act(() => document.dispatchEvent(new Event("visibilitychange")));
  expect(env.frames.size).toBe(0); env.frame(2000); expect(pattern()).toBe(hidden);
  view.unmount(); expect(env.frames.size).toBe(0);
  expect(env.listeners.size).toBe(0);
});

it("settles cables under reduced motion without a frame and stops completed springs", () => {
  const env = environment();
  const view = render(<svg><Cable from={{ x: 0, y: 0 }} to={{ x: 100, y: 0 }} connected eventId={1} /></svg>);
  env.visible(true); env.frame(0); env.frame(500); env.frame(1300);
  expect(env.frames.size).toBe(0);
  expect(view.container.querySelector("path")?.getAttribute("d")).toBe("M0 0 C12 45 88 45 100 0");
  view.rerender(<svg><Cable from={{ x: 10, y: 0 }} to={{ x: 110, y: 0 }} connected eventId={1} /></svg>);
  env.visible(true);
  expect(env.frames.size, "a resize is not a new connection").toBe(0);
  expect(view.container.querySelector("path")?.getAttribute("d")).toBe("M10 0 C22 45 98 45 110 0");
  env.reduced(true);
  view.rerender(<svg><Cable from={{ x: 0, y: 0 }} to={{ x: 100, y: 0 }} connected eventId={2} /></svg>);
  expect(env.frames.size).toBe(0);
  expect(view.container.querySelector("path")?.getAttribute("d")).toBe("M0 0 C12 45 88 45 100 0");
});
