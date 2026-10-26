import { expect, it } from "vitest";
import { FlowTrace } from "./flow";

it("moves only the display, holding the reported value without collecting observations", () => {
  const trace = new FlowTrace(), source = [10, 20, 5];
  trace.update(source);const before = trace.path(30);
  trace.advance(1000);expect(trace.path(30)).not.toBe(before);
  expect(trace.samples()).toEqual(source);expect(source).toEqual([10,20,5]);
  const held = trace.path(30);trace.advance(0);expect(trace.path(30)).toBe(held);
  trace.update([10,20,5,8]);expect(trace.samples()).toEqual([10,20,5,8]);
});
it("does not bridge unknown history, replay old samples or grow with animation frames", () => {
  const trace = new FlowTrace();trace.update([10,null,20]);
  expect(trace.path(30).match(/M/g)).toHaveLength(2);
  for(let n=0;n<1000;n++) trace.advance(100);
  expect(trace.samples()).toEqual([20]);
  const before=trace.path(30);trace.update([10,null,20]);expect(trace.path(30)).toBe(before);
  trace.update([10,null,20,null]);expect(trace.path(30)).not.toMatch(/L300\.00/);
  trace.reset([0]);expect(trace.samples()).toEqual([0]);expect(trace.path(30)).toContain('78.00');
  trace.reset([null]);expect(trace.path(30)).toBe('');
});
it("keeps an updating live bucket continuous instead of inventing gaps", () => {
  const trace = new FlowTrace(); trace.update([10,20]); trace.advance(100);
  trace.update([10,25]);
  expect(trace.samples()).toEqual([10,20,25]);
  expect(trace.path(30).match(/M/g)).toHaveLength(1);
  trace.update([30,40]);
  expect(trace.samples().at(-1)).toBe(40);
  expect(trace.samples()).not.toContain(null);
  trace.update([30,null,50]);
  expect(trace.samples()).toContain(null);
  expect(trace.path(60).match(/M/g)).toHaveLength(2);
});

it("bounds new receipts and resets the previous node rather than carrying its trail", () => {
  const trace=new FlowTrace();trace.update(Array.from({length:1000},(_,i)=>i));expect(trace.samples().length).toBeLessThanOrEqual(120);
  trace.reset([5]);expect(trace.samples()).toEqual([5]);
});
