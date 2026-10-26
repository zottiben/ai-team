import { act, cleanup, render } from "@testing-library/react";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { PatchCanvas } from "./PatchCanvas";
import { patchEvidence } from "./evidence";
import { fixtureTeamOverview } from "../../team-overview-fixture";

vi.mock("../../motion/useVisible", () => ({ useVisible: () => true }));
const fixture = fixtureTeamOverview(), graph = patchEvidence(fixture.detail, fixture.live);
let scale = 1, shift = 0, missing: string | null = null;
const observers = new Set<Observer>();
class Observer implements ResizeObserver {
  targets = new Set<Element>();
  constructor(private callback: ResizeObserverCallback) { observers.add(this); }
  observe(target: Element) { this.targets.add(target); }
  unobserve(target: Element) { this.targets.delete(target); }
  disconnect() { observers.delete(this); }
  notify() { this.callback([], this); }
}
const point = (id: string) => { const index = graph.modules.findIndex(module => module.id === id); return { x: 80 + index * 100 + shift, y: 140 + index * 55 }; };
beforeEach(() => {
  scale = 1; shift = 0; missing = null;
  vi.stubGlobal("ResizeObserver", Observer);
  vi.spyOn(HTMLElement.prototype, "offsetWidth", "get").mockReturnValue(800);
  vi.spyOn(HTMLElement.prototype, "offsetHeight", "get").mockReturnValue(600);
  vi.spyOn(HTMLElement.prototype, "getClientRects").mockReturnValue(Object.assign([new DOMRect(100, 200, 800, 600)], { item: () => null }));
  vi.spyOn(HTMLElement.prototype, "getBoundingClientRect").mockImplementation(function(this: HTMLElement) {
    if (this.classList.contains("synth-patch-canvas")) return new DOMRect(100, 200, 800 * scale, 600 * scale);
    const id = this.closest<HTMLElement>("[data-patch-id]")?.dataset.patchId;
    if (id && this.matches('.synth-jack > span[aria-hidden="true"]')) {
      if (id === missing) return new DOMRect();
      const socket = point(id); return new DOMRect(100 + (socket.x - 11) * scale, 200 + (socket.y - 11) * scale, 22 * scale, 22 * scale);
    }
    return new DOMRect(130, 230, 180, 220); // Deliberately not the socket position.
  });
});
afterEach(() => { cleanup(); vi.restoreAllMocks(); vi.unstubAllGlobals(); observers.clear(); });
const canvas = () => render(<PatchCanvas graph={graph} selected={null} onSelect={() => {}} zoom={1}/>);
function checkPlugs(container: HTMLElement) {
  const plugs = [...container.querySelectorAll(".synth-cable__plug")];
  expect(plugs).toHaveLength(graph.links.length * 2);
  graph.links.forEach((link, index) => [link.from, link.to].forEach((id, end) => {
    const socket = point(id), plug = plugs[index * 2 + end]!;
    expect(Number(plug.getAttribute("cx")), `Socket x for ${id}`).toBeCloseTo(socket.x);
    expect(Number(plug.getAttribute("cy")), `Socket y for ${id}`).toBeCloseTo(socket.y);
  }));
}
it.each([1, 1.5])("connects cable plugs to actual sockets, not module edges, at %sx scale", value => {
  scale = value; const view = canvas(); checkPlugs(view.container);
});
it("remeasures a resized jack even when its surrounding module keeps the same bounds", () => {
  const view = canvas();
  const targets = [...observers].flatMap(observer => [...observer.targets]);
  expect([...view.container.querySelectorAll(".synth-jack")].every(jack => targets.includes(jack))).toBe(true);
  shift = 35; act(() => observers.forEach(observer => observer.notify())); checkPlugs(view.container);
});
it("does not substitute module edges for an unavailable socket", () => {
  missing = graph.modules[1]!.id; const view = canvas();
  const measured = graph.links.filter(link => link.from !== missing && link.to !== missing);
  expect(view.container.querySelectorAll(".synth-cable")).toHaveLength(measured.length);
});
