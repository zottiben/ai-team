import { FitAddon } from "@xterm/addon-fit";
import { Terminal as XTerm } from "@xterm/xterm";
import { useCallback, useEffect, useRef, useState } from "react";

import {
  closeTerminal,
  openTerminal,
  readTerminal,
  resizeTerminal,
  terminals as listTerminals,
  writeTerminal,
  type Where,
} from "./api";

import "@xterm/xterm/css/xterm.css";

/**
 * A real terminal in the worktree.
 *
 * Not a shell reimplementation - enough to run a gate by hand and read what it says. The
 * session lives in the server, so closing the window does not kill a `cargo test` four
 * minutes in; reopening finds it by asking which sessions this worktree already has.
 */
export function TerminalPane({
  project,
  workspace = null,
  node,
}: {
  project: string | null;
  workspace?: string | null;
  node: number | null;
}) {
  const [id, setId] = useState<number | null>(null);
  const [problem, setProblem] = useState<string | null>(null);

  const where: Where | null = project === null ? null : { project, workspace, node };

  // Attach to an existing session before opening one, so a reconnecting window rejoins
  // the terminal it had rather than leaving it orphaned and starting another.
  const attach = useCallback(async () => {
    if (where === null) return;
    try {
      const open = (await listTerminals(where)).filter((session) => !session.done);
      setId(open[0]?.id ?? (await openTerminal(where)).id);
      setProblem(null);
    } catch (error: unknown) {
      setProblem(error instanceof Error ? error.message : String(error));
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [project, workspace, node]);

  useEffect(() => {
    void attach();
  }, [attach]);

  if (project === null) {
    return <p className="empty">Pick a project to open a terminal in it.</p>;
  }

  return (
    <div className="terminal">
      <div className="main__header">
        <div>
          <h2>Terminal</h2>
          {id !== null && <span className="faint mono">session {id}</span>}
        </div>
        <button
          type="button"
          className="button"
          onClick={() => {
            if (id !== null) void closeTerminal(id).then(() => setId(null)).then(attach);
          }}
        >
          Restart
        </button>
      </div>
      {problem !== null && <p className="error">{problem}</p>}
      {id !== null && <Session id={id} />}
    </div>
  );
}

/** xterm needs real colours, so the semantic tokens are resolved once at mount. */
function readTheme(): Record<string, string> {
  const styles = getComputedStyle(document.documentElement);
  const token = (name: string, fallback: string) =>
    styles.getPropertyValue(name).trim() || fallback;
  return {
    background: token("--surface-canvas-default", "#101418"),
    foreground: token("--text-primary-default", "#e6e9ee"),
    cursor: token("--accent-action-default", "#6ea8fe"),
    selectionBackground: token("--accent-action-muted", "#6ea8fe40"),
  };
}

/**
 * One server-side session, attached to an xterm.
 *
 * Split out from the pane because a session is not always a worktree's: signing a
 * provider in opens one in the home directory (D25), and it needs exactly this - a
 * terminal to watch - without a project to be a terminal *of*.
 */
export function Session({ id }: { id: number }) {
  const host = useRef<HTMLDivElement | null>(null);

  useEffect(() => {
    if (host.current === null) return undefined;

    const term = new XTerm({
      convertEol: true,
      fontFamily: "var(--font-mono)",
      fontSize: 13,
      // Read from the token layer so the pane repaints with the window rather than
      // staying dark when it goes light.
      theme: readTheme(),
    });
    const fit = new FitAddon();
    term.loadAddon(fit);
    term.open(host.current);
    fit.fit();

    // The size has to reach the process: a shell that thinks it has eighty columns wraps
    // at eighty, and the output arrives already broken.
    const tell = () => {
      fit.fit();
      void resizeTerminal(id, term.rows, term.cols).catch(() => {});
    };
    tell();
    const observer = new ResizeObserver(tell);
    observer.observe(host.current);

    term.onData((text) => void writeTerminal(id, text).catch(() => {}));

    // Polled from an absolute cursor, the same way Pi's stream is ingested: a reconnect
    // asks from a number it already has rather than replaying everything.
    let cursor = 0;
    let stopped = false;
    const poll = async () => {
      while (!stopped) {
        try {
          const chunk = await readTerminal(id, cursor);
          if (chunk.text !== "") term.write(chunk.text);
          cursor = chunk.cursor;
          if (chunk.done) {
            term.write(`\r\n[exited ${chunk.status ?? "?"}]\r\n`);
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
      observer.disconnect();
      term.dispose();
    };
  }, [id]);

  return <div className="terminal__host" ref={host} />;
}
