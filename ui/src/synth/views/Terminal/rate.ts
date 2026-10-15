import { SampleBuffer, type Series } from "../../primitives/graph";

/**
 * Lines per second, as this window received them.
 *
 * Collected by the thing that reads the stream, never by an animation frame (§13.1), and
 * rolled forward lazily when somebody reads it - so an idle pane keeps no timer running
 * and a scope that nobody is looking at costs nothing.
 *
 * What it measures is deliberately narrow: output this window polled, not output the
 * process produced. Unobserved intervals stay missing. An observed empty poll can
 * establish zero; absence of a poll cannot.
 */
export function createRateSampler(now: () => number = () => performance.now(), capacity = 120) {
  const window = Math.max(1, Math.min(600, Number.isFinite(capacity) ? Math.floor(capacity) : 120));
  const buffer = new SampleBuffer(window);
  let second = Math.floor(now() / 1000);
  let lines = 0;
  let observed = false;
  const roll = (to: number) => {
    if (to <= second) return;
    buffer.push(observed ? lines : null);
    lines = 0;
    observed = false;
    for (let missed = second + 1; missed < to && missed <= second + window; missed += 1) buffer.push(null);
    second = to;
  };
  return {
    /** Count text as it arrives. Newlines, because that is what a line is on a pty. */
    add(text: string) {
      roll(Math.floor(now() / 1000));
      observed = true;
      for (const ch of text) if (ch === "\n") lines += 1;
    },
    series(): Series {
      roll(Math.floor(now() / 1000));
      return buffer.snapshot();
    },
  };
}

export type RateSampler = ReturnType<typeof createRateSampler>;
