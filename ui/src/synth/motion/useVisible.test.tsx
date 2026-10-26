import { act, renderHook } from "@testing-library/react";
import { afterEach, expect, it, vi } from "vitest";
import { useVisible } from "./useVisible";

afterEach(() => { vi.restoreAllMocks(); vi.unstubAllGlobals(); });

it("uses the latest observation when hide and reveal arrive in the same batch", () => {
  const ref = { current: document.createElement("div") };
  let deliver: (states: boolean[]) => void = () => { throw Error("Observer not mounted"); };
  const disconnect = vi.fn();
  vi.stubGlobal("IntersectionObserver", class implements IntersectionObserver {
    root = null; rootMargin = "0px"; thresholds = [0];
    observe = vi.fn(); unobserve = vi.fn(); disconnect = disconnect; takeRecords = () => [];
    constructor(callback: IntersectionObserverCallback) {
      deliver = states => callback(states.map((visible, time) => ({ target: ref.current, time,
        isIntersecting: visible, intersectionRatio: visible ? 1 : 0, rootBounds: null,
        boundingClientRect: new DOMRect(), intersectionRect: new DOMRect() })), this);
    }
  });
  const { result, unmount } = renderHook(() => useVisible(ref));
  act(() => deliver([false, true])); expect(result.current).toBe(true);
  act(() => deliver([true, false])); expect(result.current).toBe(false);
  act(() => deliver([false, true])); expect(result.current).toBe(true);
  vi.spyOn(document, "hidden", "get").mockReturnValue(true);
  act(() => document.dispatchEvent(new Event("visibilitychange")));
  expect(result.current).toBe(false);
  act(() => deliver([false, true])); expect(result.current).toBe(false);
  unmount(); expect(disconnect).toHaveBeenCalledOnce();
});
