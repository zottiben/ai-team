import { expect, it } from "vitest";

import { lineChanges } from "./changes";

it("an unchanged buffer claims nothing", () => {
  const changes = lineChanges("a\nb\n", "a\nb\n");
  expect([...changes.added]).toEqual([]);
  expect(changes.removedCount).toBe(0);
  expect(changes.untracked).toBe(false);
});

it("marks the line that was added, by its number in this buffer", () => {
  const changes = lineChanges("a\nb\nc", "a\nnew\nb\nc");
  expect([...changes.added]).toEqual([2]);
  expect(changes.removedCount).toBe(0);
});

it("hangs a removal under the line above it, and still counts the lines", () => {
  const changes = lineChanges("a\nb\nc\nd", "a\nd");
  expect(changes.removedCount).toBe(2);
  // b and c went from after line 1, so the wedge sits under line 1.
  expect([...changes.removedAfter]).toEqual([1]);
  expect([...changes.added]).toEqual([]);
});

it("an edited line is one added line and one removed line", () => {
  const changes = lineChanges("a\nb\nc", "a\nB\nc");
  expect([...changes.added]).toEqual([2]);
  expect(changes.removedCount).toBe(1);
});

it("a file that is not in HEAD is untracked, not a buffer of additions", () => {
  // Drawing every line of a new file as added hides the markers that mean something
  // the first time it is committed.
  const changes = lineChanges(null, "a\nb\n");
  expect(changes.untracked).toBe(true);
  expect([...changes.added]).toEqual([]);
});

it("an unchanged head and tail are not compared, so a big file stays cheap", () => {
  const before = Array.from({ length: 5000 }, (_, index) => `line ${index}`);
  const after = [...before];
  after[2500] = "line 2500 edited";
  const started = Date.now();
  const changes = lineChanges(before.join("\n"), after.join("\n"));
  expect([...changes.added]).toEqual([2501]);
  expect(changes.coarse).toBe(false);
  expect(Date.now() - started).toBeLessThan(500);
});

it("two files too far apart to compare say so rather than guessing", () => {
  const before = Array.from({ length: 2500 }, (_, index) => `old ${index}`).join("\n");
  const after = Array.from({ length: 2500 }, (_, index) => `new ${index}`).join("\n");
  const changes = lineChanges(before, after);
  expect(changes.coarse).toBe(true);
  expect([...changes.added]).toEqual([]);
  expect([...changes.removedAfter]).toEqual([]);
});

it("an empty buffer against a committed file is all removal", () => {
  const changes = lineChanges("a\nb\n", "");
  expect(changes.removedCount).toBeGreaterThan(0);
  expect([...changes.added]).toEqual([]);
});
