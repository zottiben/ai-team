import { readTerminal, writeTerminal } from "./api";

/**
 * The two rules a terminal surface must not get wrong, in one place.
 *
 * Both skins render a session, so both would otherwise own a copy of this - and a copy
 * of a safety rule is a copy that rots. Keystroke ordering and the refusal to resend
 * after an uncertain write are the same rules whichever pane is on screen.
 */

/** What is said after a write whose fate is unknown. The surface shows it verbatim. */
export const UNCERTAIN_INPUT =
  "Queued input was stopped and not retried. Inspect the terminal before enabling input again; no previous text will be resent.";

export type InputQueue = {
  /** Queue one keystroke batch behind everything already queued. */
  push(text: string): void;
  /** Stop sending. Used when the surface goes away mid-flight. */
  stop(): void;
};

/**
 * Deliver keystrokes in order, and stop for good on an uncertain one.
 *
 * Independent HTTP requests can arrive out of order, including Enter before the command
 * it belongs to. A failed acknowledgement is uncertain delivery, not permission to retry
 * or to send the remaining suffix of what may now be a different command.
 */
export function serialInput(
  id: number,
  onUncertain: (message: string) => void,
  send: (id: number, text: string) => Promise<unknown> = writeTerminal,
): InputQueue {
  let stopped = false;
  let failed = false;
  let writes = Promise.resolve();
  return {
    push(text: string) {
      writes = writes.then(async () => {
        if (stopped || failed) return;
        try {
          await send(id, text);
        } catch (error: unknown) {
          failed = true;
          if (!stopped)
            onUncertain(
              `Input delivery failed: ${error instanceof Error ? error.message : String(error)}. ${UNCERTAIN_INPUT}`,
            );
        }
      });
    },
    stop() {
      stopped = true;
    },
  };
}

export type OutputHandlers = {
  text(text: string, replay: boolean): void;
  /** Every successful read, including empty polls; the initial read is scrollback. */
  observe?(text: string, replay: boolean): void;
  /** Output the scrollback discarded before this reader reached it. Said, not hidden. */
  dropped?(bytes: number): void;
  done(status: number | null): void;
};

/**
 * Read a session forward from an absolute cursor, the way Pi's stream is ingested: a
 * reconnect asks from a number it already has rather than replaying everything.
 */
export function streamOutput(
  id: number,
  handlers: OutputHandlers,
  read: (id: number, cursor: number) => Promise<{ text: string; cursor: number; done: boolean; status: number | null; dropped?: number }> = readTerminal,
): () => void {
  let stopped = false;
  let cursor = 0;
  let initial = true;
  const poll = async () => {
    while (!stopped) {
      try {
        const chunk = await read(id, cursor);
        if (stopped) return;
        if (chunk.dropped) handlers.dropped?.(chunk.dropped);
        if (chunk.text !== "") handlers.text(chunk.text, initial);
        handlers.observe?.(chunk.text, initial);
        initial = false;
        cursor = chunk.cursor;
        if (chunk.done) {
          handlers.done(chunk.status);
          return;
        }
      } catch {
        // A closed session is not worth an error banner over a pane that has already
        // printed everything it is going to.
        return;
      }
      await new Promise((resolve) => setTimeout(resolve, 120));
    }
  };
  void poll();
  return () => {
    stopped = true;
  };
}
