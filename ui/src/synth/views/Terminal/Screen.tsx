import { Terminal as XTerm } from "@xterm/xterm";
import { FitAddon } from "@xterm/addon-fit";
import { useEffect, useRef, useState } from "react";

import { resizeTerminal } from "../../../api";
import { serialInput, streamOutput } from "../../../terminal-stream";
import { onThemeChange, terminalTheme } from "../../terminal-theme";
import { useReducedMotion } from "../../motion/useReducedMotion";
import { useVisible } from "../../motion/useVisible";
import { SelectionToolbar } from "./SelectionToolbar";

import "@xterm/xterm/css/xterm.css";

export type Renderer = { kind: "webgl" | "standard"; reason: string | null };
export type ScreenHandlers = {
  onOutput?(text: string): void;
  onDropped?(bytes: number): void;
  onRenderer?(renderer: Renderer): void;
  onEnded?(status: number | null): void;
  onSend?(text: string): void;
};

/** One mounted input owner and output cursor per PTY, independent of skin and tab. */
export function Screen({ session, effects, synth = true, label, handlers = {} }: {
  session: number;
  effects: boolean;
  synth?: boolean;
  label: string;
  handlers?: ScreenHandlers;
}) {
  const host = useRef<HTMLDivElement | null>(null);
  const instance = useRef<XTerm | null>(null);
  const fitScreen = useRef<(() => void) | null>(null);
  const callbacks = useRef(handlers);
  callbacks.current = handlers;
  const reduced = useReducedMotion();
  const visible = useVisible(host);
  const [inputProblem, setInputProblem] = useState<string | null>(null);
  const [inputAttempt, setInputAttempt] = useState(0);
  const [selection, setSelection] = useState<{ text: string; top: number; left: number } | null>(null);

  useEffect(() => {
    if (host.current === null) return;
    setInputProblem(null);
    setSelection(null);
    const term = new XTerm({ convertEol: true, fontSize: 13, cursorBlink: false, theme: terminalTheme() });
    instance.current = term;
    const fit = new FitAddon();
    term.loadAddon(fit);
    term.open(host.current);
    const tell = () => {
      if (!host.current || host.current.clientWidth === 0 || host.current.clientHeight === 0) return;
      fit.fit();
      void resizeTerminal(session, term.rows, term.cols).catch(() => {});
    };
    fitScreen.current = tell;
    tell();
    const observer = new ResizeObserver(tell);
    observer.observe(host.current);
    const untheme = onThemeChange((theme) => { term.options.theme = theme; });
    const input = serialInput(session, setInputProblem);
    term.onData((text) => input.push(text));
    const selected = term.onSelectionChange(() => {
      const text = term.getSelection();
      const at = term.getSelectionPosition();
      if (text.trim() === "" || !at || host.current === null) { setSelection(null); return; }
      const grid = term.element?.querySelector(".xterm-screen")?.getBoundingClientRect() ?? host.current.getBoundingClientRect();
      const frame = host.current.closest(".synth-screen")?.getBoundingClientRect() ?? grid;
      const rowHeight = grid.height / Math.max(1, term.rows);
      const columnWidth = grid.width / Math.max(1, term.cols);
      const viewport = at.start.y - (term.buffer.active.viewportY ?? 0);
      setSelection({ text,
        top: grid.top - frame.top + Math.max(0, Math.min(grid.height, viewport * rowHeight)),
        left: grid.left - frame.left + Math.max(0, Math.min(grid.width, at.start.x * columnWidth)),
      });
    });
    const stop = streamOutput(session, {
      text(text) { term.write(text); },
      observe(text, replay) { if (!replay) callbacks.current.onOutput?.(text); },
      dropped(bytes) { callbacks.current.onDropped?.(bytes); },
      done(status) {
        term.write(`\r\n[exited ${status ?? "?"}]\r\n`);
        callbacks.current.onEnded?.(status);
      },
    });
    return () => {
      stop(); input.stop(); selected.dispose(); untheme(); observer.disconnect();
      fitScreen.current = null;
      instance.current = null;
      term.dispose();
    };
  }, [session, inputAttempt]);

  // Preferences change options, never dispose a terminal with pending input or selection.
  useEffect(() => {
    const term = instance.current;
    if (!term) return;
    const styles = getComputedStyle(document.documentElement);
    term.options.fontFamily = styles.getPropertyValue(synth ? "--synth-font-mono" : "--font-mono").trim() || "monospace";
    term.options.fontSize = synth ? 13.5 : 13;
    term.options.lineHeight = synth ? 1.78 : 1;
    term.options.cursorBlink = synth && visible && !reduced;
    term.options.theme = terminalTheme();
    fitScreen.current?.();
  }, [session, inputAttempt, synth, visible, reduced]);

  useEffect(() => {
    const term = instance.current;
    if (!term || !synth) return;
    let disposed = false;
    let webgl: { dispose(): void } | null = null;
    const standard = (reason: string) => {
      webgl?.dispose(); webgl = null;
      callbacks.current.onRenderer?.({ kind: "standard", reason });
    };
    void (async () => {
      try {
        const { WebglAddon } = await import("@xterm/addon-webgl");
        if (disposed) return;
        const addon = new WebglAddon();
        addon.onContextLoss(() => standard("The GPU dropped this terminal's context; the standard renderer took over."));
        webgl = addon;
        term.loadAddon(addon);
        callbacks.current.onRenderer?.({ kind: "webgl", reason: null });
      } catch (error: unknown) {
        if (!disposed) standard(`WebGL is unavailable here (${error instanceof Error ? error.message : String(error)}); the standard renderer is painting.`);
      }
    })();
    return () => { disposed = true; webgl?.dispose(); };
  }, [session, inputAttempt, synth]);

  return <div className="synth-screen" data-effects={effects || undefined}>
    {inputProblem !== null && <div role="alert" className="synth-screen__alert">
      <p>{inputProblem}</p>
      <button type="button" className="button synth-key" onClick={() => setInputAttempt((attempt) => attempt + 1)}>Enable input after inspection</button>
    </div>}
    <div className="synth-screen__well terminal__host" aria-label={label}>
      <div className="synth-screen__mount" ref={host} />
    </div>
    {selection && <SelectionToolbar text={selection.text} top={selection.top} left={selection.left} onSend={callbacks.current.onSend} />}
  </div>;
}
