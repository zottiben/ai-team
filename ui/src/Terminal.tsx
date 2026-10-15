import { FitAddon } from "@xterm/addon-fit";
import { Terminal as XTerm } from "@xterm/xterm";
import { lazy, Suspense, useEffect, useRef, useState } from "react";

import {
  resizeTerminal,
  type RunEvent,
} from "./api";
import { serialInput, streamOutput } from "./terminal-stream";
import { useSynth } from "./ui-preferences";

import "@xterm/xterm/css/xterm.css";

const TerminalWorkspace = lazy(() => import("./synth/views/Terminal/TerminalWorkspace"));

/**
 * A real terminal in the worktree.
 *
 * Not a shell reimplementation - enough to run a gate by hand and read what it says. The
 * session lives in the server, so closing the window does not kill a `cargo test` four
 * minutes in; reopening finds it by asking which sessions this worktree already has.
 *
 * Both skins share one controller and mounted xterm/input owner per session. A skin
 * change must not discard scroll position, selection or uncertain input state.
 */
export function TerminalPane({
  project,
  workspace = null,
  node,
  events,
  agentActive,
  onSend,
  onClose,
}: {
  project: string | null;
  workspace?: string | null;
  node: number | null;
  /** The chat's event feed, which is where the synth read-only mirror comes from. */
  events?: readonly RunEvent[];
  agentActive?: boolean;
  onSend?: (text: string, source?: "operator_terminal" | "agent_shell") => void;
  onClose?: () => void;
}) {
  const synth = useSynth();
  if (project !== null)
    return (
      <Suspense fallback={<p>Loading terminal…</p>}>
        <TerminalWorkspace
          key={JSON.stringify([project, workspace, node])}
          project={project}
          synth={synth}
          workspace={workspace}
          node={node}
          events={events}
          agentActive={agentActive}
          onSend={onSend}
          onClose={onClose}
        />
      </Suspense>
    );
  return <p className="empty">Pick a project to open a terminal in it.</p>;
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
  const [inputProblem, setInputProblem] = useState<string | null>(null);
  const [inputAttempt, setInputAttempt] = useState(0);

  useEffect(() => {
    if (host.current === null) return undefined;
    setInputProblem(null);

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

    // Ordering and the refusal to resend after an uncertain write are the same rules in
    // both skins, so they live in one module rather than in two copies of this effect.
    const input = serialInput(id, setInputProblem);
    term.onData((text) => input.push(text));
    const stop = streamOutput(id, {
      text: (text) => term.write(text),
      done: (status) => term.write(`\r\n[exited ${status ?? "?"}]\r\n`),
    });

    return () => {
      stop();
      input.stop();
      observer.disconnect();
      term.dispose();
    };
  }, [id, inputAttempt]);

  return <>{inputProblem && <div role="alert"><p className="error">{inputProblem}</p><button className="button" onClick={() => setInputAttempt((attempt) => attempt + 1)}>Enable input after inspection</button></div>}<div className="terminal__host" ref={host} /></>;
}
