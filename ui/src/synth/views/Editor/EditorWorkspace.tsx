import { useEffect, useMemo, useRef, useState, type ReactNode } from "react";

import { scm as fetchScm, type Scm, type Where } from "../../../api";
import { chatOverview, type ChatOverview } from "../../../chat-overview-api";
import { baseline } from "../../../editor-api";
import type { Buffer, Pending, Receipt } from "../../../Editor";
import type { Refusal } from "../../../editor-api";
import { HwButton } from "../../primitives/HwButton";
import { Lcd } from "../../primitives/Lcd";
import { Led } from "../../primitives/Led";
import { Plate } from "../../primitives/Plate";
import { lineChanges, NO_CHANGES, type Changes } from "./changes";
import AgentMonitor from "./AgentMonitor";
import CodeView, { type Cursor } from "./CodeView";
import EditorTabs from "./EditorTabs";
import FileBrowser from "./FileBrowser";
import { elapsedSeconds, presenceOf } from "./presence";
import SavePreview, { SaveRefusal } from "./SavePreview";
import StatusBar from "./StatusBar";
import WaveMinimap from "./WaveMinimap";
import "./editor.css";

/** §11.2's markers are recomputed as you type; 200ms is a pause, not a keystroke. */
const DIFF_DEBOUNCE = 200;
/** The agent moves at the speed of its tools. Polled, and only while on screen. */
const PRESENCE_POLL = 5000;
/** Git status over a whole checkout is not free, and a branch name does not race. */
const STATUS_POLL = 20_000;

export default function EditorWorkspace({
  where, chat, visible, buffers, active, current, problem, pending, refusal, reverting,
  receipt, busy, surface, onOpen, onSelect, onClose, onSave, onConfirm, onCancel, onReload,
  onDismissRefusal, onAskRevert, onRevert, onDismissReceipt, onDiscardKept, onOpenChat, onSteer,
}: {
  where: Where;
  chat: number | null;
  visible: boolean;
  buffers: Buffer[];
  active: string | null;
  current: Buffer | null;
  problem: string | null;
  pending: Pending | null;
  refusal: { path: string; refusal: Refusal } | null;
  reverting: string | null;
  receipt: Receipt | null;
  busy: boolean;
  surface: ReactNode;
  onOpen: (path: string) => void;
  onSelect: (path: string) => void;
  onClose: (path: string) => void;
  onSave: () => void;
  onConfirm: () => void;
  onCancel: () => void;
  onReload: (path: string) => void;
  onDismissRefusal: () => void;
  onAskRevert: (path: string | null) => void;
  onRevert: (path: string) => void;
  onDismissReceipt: () => void;
  /** Throw away one kept previous version. Only ever from here - never from a save. */
  onDiscardKept: (name: string) => void;
  onOpenChat?: () => void;
  onSteer?: () => void;
}) {
  const [scm, setScm] = useState<Scm | null>(null);
  const [overview, setOverview] = useState<ChatOverview | null>(null);
  const [now, setNow] = useState(() => Date.now());
  const [heads, setHeads] = useState<Record<string, string | null>>({});
  const [changes, setChangeSet] = useState<Changes>(NO_CHANGES);
  const [cursor, setCursor] = useState<Cursor | null>(null);
  const [viewport, setViewport] = useState<{ from: number; to: number } | null>(null);
  const [jump, setJump] = useState<number | null>(null);
  const asked = useRef(new Set<string>());

  const presence = useMemo(() => presenceOf(overview, now), [overview, now]);
  const elapsed = elapsedSeconds(presence, now);

  // Everything live stops when the pane is hidden: §16's "offscreen costs nothing"
  // applies to polling as much as to frames.
  useEffect(() => {
    if (!visible || chat === null) return undefined;
    let live = true;
    const read = () => {
      void chatOverview(chat).then(value => { if (live) setOverview(value); }).catch(() => undefined);
      if (live) setNow(Date.now());
    };
    read();
    const timer = setInterval(read, PRESENCE_POLL);
    return () => { live = false; clearInterval(timer); };
  }, [visible, chat]);

  // Slower, and on its own clock: every read of this is four git subprocesses over the
  // whole checkout, and a branch name does not move between keystrokes.
  useEffect(() => {
    if (!visible) return undefined;
    let live = true;
    const read = () => {
      void fetchScm(where).then(value => { if (live) setScm(value); }).catch(() => undefined);
    };
    read();
    const timer = setInterval(read, STATUS_POLL);
    return () => { live = false; clearInterval(timer); };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [visible, where.project, where.workspace, where.node, receipt]);

  // The elapsed readout, at 1Hz and only while something is actually running. §13.2
  // allows a once-a-second number; a loop with nothing to count is what it forbids.
  useEffect(() => {
    if (!visible || !presence.working) return undefined;
    const timer = setInterval(() => setNow(Date.now()), 1000);
    return () => clearInterval(timer);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [visible, presence.working]);

  // The committed text, once per file. The markers are computed against it in the
  // window, so they follow the buffer rather than the last save.
  useEffect(() => {
    if (current === null || asked.current.has(current.path)) return;
    asked.current.add(current.path);
    void baseline(where, current.path)
      .then(body => setHeads(state => ({ ...state, [body.path]: body.text })))
      .catch(() => setHeads(state => ({ ...state, [current.path]: null })));
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [current?.path, where.project, where.workspace, where.node]);

  useEffect(() => {
    if (current === null) { setChangeSet(NO_CHANGES); return undefined; }
    const head = heads[current.path];
    const timer = setTimeout(
      () => setChangeSet(lineChanges(head ?? null, current.text)),
      DIFF_DEBOUNCE,
    );
    return () => clearTimeout(timer);
  }, [current, heads]);

  const lengths = useMemo(
    () => (current === null ? [] : current.text.split("\n").map(line => line.length)),
    [current],
  );
  const dirty = current !== null && current.text !== current.saved;

  return <Plate tone="night" radius={22} className="synth-editor">
    <header className="synth-editor__toolbar">
      <span className="synth-silk">Editor</span>
      <span className="synth-editor__badge">Operator</span>
      <span className="synth-muted">
        Edits write to the live checkout - an operator tool, not an OS sandbox.
      </span>
      <span className="synth-editor__toolbar-gap" />
      <HwButton
        disabled={!dirty}
        onClick={() => current !== null && onAskRevert(current.path)}
      >Revert</HwButton>
      <HwButton
        variant="primary"
        disabled={!dirty || busy || current?.base === null}
        onClick={onSave}
      >Save ⌘S</HwButton>
      {onOpenChat !== undefined && <HwButton aria-label="Close editor" onClick={onOpenChat}>×</HwButton>}
    </header>

    {presence.working && <Lcd variant="working" className="synth-editor__notice">
      <Led state="pulse" signal="result" size={8} />
      {presence.role ?? "An agent"} is executing in this checkout
      {presence.now === null ? "." : `: ${presence.now.summary}.`} Your saves are checked
      against the file on disk, and refused if it moved.
    </Lcd>}

    {problem !== null && <p className="synth-editor__problem" role="alert">{problem}</p>}

    {receipt !== null && <Lcd className="synth-editor__notice" role="status">
      <Led state="on" signal="diff" size={8} />
      Wrote {receipt.path}, {receipt.bytes} bytes.{" "}
      {receipt.chats > 0
        ? `${receipt.chats} chat${receipt.chats === 1 ? "" : "s"} will be told at the start of the next turn.`
        : "No chat works in this checkout, so no agent was told."}{" "}
      {receipt.kept !== null && <>
        The version it replaced is kept at <span className="synth-mono">{receipt.kept.path}</span>
        {receipt.kept.in_checkout ? " - inside the checkout, so git will see it." : "."}{" "}
        <HwButton onClick={() => onDiscardKept(receipt.kept?.name ?? "")}>Discard kept version</HwButton>{" "}
      </>}
      {receipt.note !== null && <span className="synth-muted">{receipt.note} </span>}
      <HwButton onClick={onDismissReceipt}>Dismiss</HwButton>
    </Lcd>}

    {reverting !== null && <div className="synth-editor__confirm" role="dialog" aria-label="Confirm revert">
      <p>
        Throw away unsaved changes to <span className="synth-mono">{reverting}</span>? Nothing on
        disk changes - only this buffer goes back to the version that was read.
      </p>
      <HwButton variant="danger" onClick={() => onRevert(reverting)}>Discard my changes</HwButton>
      <HwButton onClick={() => onAskRevert(null)}>Keep editing</HwButton>
    </div>}

    <div className="synth-editor__grid">
      <FileBrowser
        where={where}
        checkout={where.workspace ?? where.project}
        scm={scm}
        presence={presence}
        active={active}
        visible={visible}
        onOpen={onOpen}
      />

      <div className="synth-editor__centre">
        <EditorTabs
          buffers={buffers}
          active={active}
          changes={changes}
          onSelect={onSelect}
          onClose={onClose}
        />
        {current === null
          ? <p className="synth-editor__empty">Open a file from the tree, or find one.</p>
          : current.editable
            ? <CodeView
                surface={surface}
                path={current.path}
                generation={current.generation}
                changes={changes}
                presence={presence}
                onCursor={setCursor}
                onViewport={setViewport}
                scrollTo={jump}
              />
            : <p className="synth-editor__empty">
                {current.path} is {current.reason ?? "not text"} - not opening it for editing.
              </p>}
      </div>

      <WaveMinimap
        lengths={lengths}
        added={changes.added}
        removed={changes.removedAfter}
        viewport={viewport}
        onJump={setJump}
      />

      <AgentMonitor
        presence={presence}
        elapsed={elapsed}
        open={buffers.map(buffer => buffer.path)}
        onOpenChat={onOpenChat}
        onSteer={onSteer}
      />
    </div>

    <StatusBar
      buffer={current}
      branch={scm?.branch ?? null}
      cursor={cursor}
      presence={presence}
      elapsed={elapsed}
    />

    {pending !== null && <SavePreview pending={pending} busy={busy} onConfirm={onConfirm} onCancel={onCancel} />}
    {refusal !== null && <SaveRefusal
      refusal={refusal.refusal}
      onReload={() => onReload(refusal.path)}
      onDismiss={onDismissRefusal}
    />}
  </Plate>;
}
