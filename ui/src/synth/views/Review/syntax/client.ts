import type { SyntaxInput, SyntaxRequest, SyntaxResponse, SyntaxResult } from "./types";

type Port = {
  postMessage: (message: SyntaxRequest) => void; terminate: () => void;
  onmessage: ((event: MessageEvent<SyntaxResponse>) => void) | null;
  onerror: ((event: ErrorEvent) => void) | null;
};
type Job = { id: number; input: SyntaxInput; done: (result: SyntaxResult) => void };
const unavailable = (): SyntaxResult => ({ state: "unavailable", lines: {} });

/** One bounded queue, no idle polling. The last visible owner retires the worker. */
export function createSyntaxClient(factory: () => Port, timeout = 5000) {
  let worker: Port | null = null, active: Job | null = null, serial = 0, owners = 0;
  let timer: ReturnType<typeof setTimeout> | undefined;
  const queue = new Map<number, Job>();
  const stop = () => { clearTimeout(timer); timer = undefined; if (worker) { worker.onmessage = null; worker.onerror = null; worker.terminate(); worker = null; } };
  const failed = () => {
    const jobs = [...active ? [active] : [], ...queue.values()];
    active = null; queue.clear(); stop();
    jobs.forEach(job => job.done(unavailable()));
  };
  const pump = () => {
    if (active || !queue.size || !owners) return;
    try {
      if (!worker) {
        worker = factory();
        worker.onerror = failed;
        worker.onmessage = event => {
          if (!active || event.data.id !== active.id) return;
          const job = active; active = null; clearTimeout(timer); timer = undefined;
          // A failed dynamic import is cached by the worker's module loader. Retire
          // that context so a deliberate later retry can actually load it again.
          if (event.data.result.state === "unavailable") stop();
          job.done(event.data.result); pump();
        };
      }
      const job = queue.values().next().value;
      if (!job) return;
      queue.delete(job.id); active = job;
      timer = setTimeout(failed, timeout);
      worker.postMessage({ id: job.id, input: job.input });
    } catch { failed(); }
  };
  return {
    retain() {
      owners++; let released = false;
      return () => { if (released) return; released = true; owners--; if (!owners) { active = null; queue.clear(); stop(); } };
    },
    request(input: SyntaxInput, done: Job["done"]) {
      if (!owners || queue.size >= 32) { done(unavailable()); return () => {}; }
      const id = ++serial; queue.set(id, { id, input, done }); pump();
      return () => { queue.delete(id); if (active?.id === id) { active = null; stop(); pump(); } };
    },
  };
}
export const syntaxClient = createSyntaxClient(() => new Worker(new URL("./worker.ts", import.meta.url), { type: "module", name: "synth-diff-syntax" }));
