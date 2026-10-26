export type Tick = (now: number, dt: number) => void | boolean;
interface Subscriber { tick: Tick; every: number; last: number | null; visible: boolean }

/** One clock for all synth visuals. A false callback result completes a finite effect. */
export function createTicker(request: typeof requestAnimationFrame, cancel: typeof cancelAnimationFrame) {
  const subscribers = new Set<Subscriber>();
  let frame: number | null = null;
  let paused = false;
  const eligible = () => !paused && [...subscribers].some(s => s.visible);
  const schedule = () => {
    if (eligible()) {
      if (frame === null) frame = request(loop);
    } else if (frame !== null) {
      cancel(frame);
      frame = null;
    }
  };
  function loop(now: number) {
    frame = null;
    try {
      for (const s of [...subscribers]) {
        if (paused || !s.visible || !subscribers.has(s)) continue;
        const dt = s.last === null ? 0 : now - s.last;
        if (s.last === null || dt >= s.every) {
          s.last = now;
          if (s.tick(now, dt) === false) subscribers.delete(s);
        }
      }
    } finally { schedule(); }
  }
  return {
    subscribe(tick: Tick, fps: number, visible = true) {
      if (!Number.isFinite(fps) || fps <= 0 || fps > 60) throw new Error("Ticker fps must be in (0, 60]");
      const s: Subscriber = { tick, every: 1000 / fps, last: null, visible };
      subscribers.add(s);
      schedule();
      return {
        visible(value: boolean) {
          if (s.visible !== value) { s.visible = value; s.last = null; }
          schedule();
        },
        unsubscribe() { subscribers.delete(s); schedule(); },
      };
    },
    pause(value: boolean) {
      if (paused !== value) {
        paused = value;
        for (const s of subscribers) s.last = null;
      }
      schedule();
    },
  };
}

let shared: ReturnType<typeof createTicker> | undefined;
let users = 0;
const visibility = () => shared?.pause(document.hidden);

export function subscribe(tick: Tick, fps: number, visible: boolean) {
  shared ??= createTicker(cb => requestAnimationFrame(cb), id => cancelAnimationFrame(id));
  const sub = shared.subscribe(tick, fps, visible);
  if (users++ === 0) {
    visibility();
    document.addEventListener("visibilitychange", visibility);
  }
  let removed = false;
  return {
    visible: sub.visible,
    unsubscribe() {
      if (removed) return;
      removed = true;
      sub.unsubscribe();
      if (--users === 0) document.removeEventListener("visibilitychange", visibility);
    },
  };
}
