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

// CodeMirror measures the document to decide what to draw, and measuring means asking a
// Range for its rectangles. jsdom implements Range but not its geometry, so the first
// paint of any editor throws - and an unhandled error fails the whole run even when every
// assertion passed.
//
// Zeroes are the honest answer here: there is no layout engine, so nothing has a size.
// CodeMirror copes, because the same is true of a document that has not been laid out yet.
if (typeof Range !== "undefined" && Range.prototype.getClientRects === undefined) {
  const empty = () => ({ top: 0, left: 0, bottom: 0, right: 0, width: 0, height: 0, x: 0, y: 0 });
  const noRects = () => Object.assign([], { item: () => null });
  Range.prototype.getClientRects = noRects as unknown as typeof Range.prototype.getClientRects;
  Range.prototype.getBoundingClientRect =
    empty as unknown as typeof Range.prototype.getBoundingClientRect;
}
