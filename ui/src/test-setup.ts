// What jsdom does not provide but the shell needs.
//
// `matchMedia` is the gap that matters: the theme layer asks the OS what it prefers on
// first paint, and jsdom has no window manager to ask. Stubbed here rather than in each
// test file so a new test does not have to know.

import { vi } from "vitest";

if (typeof window !== "undefined" && window.matchMedia === undefined) {
  window.matchMedia = ((query: string) => ({
    matches: false,
    media: query,
    onchange: null,
    addEventListener: vi.fn(),
    removeEventListener: vi.fn(),
    addListener: vi.fn(),
    removeListener: vi.fn(),
    dispatchEvent: vi.fn(),
  })) as unknown as typeof window.matchMedia;
}
