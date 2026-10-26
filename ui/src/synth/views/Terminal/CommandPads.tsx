import { useRef, useState } from "react";

import type { ConfirmedPress, TerminalCommand, TerminalDeck, TerminalPad } from "../../../api";
import { CommandPad } from "../../primitives/CommandPad";
import { Silk } from "../../primitives/Silk";

/**
 * Six buttons that run a command, and never type one (§4.11).
 *
 * Three things a pad deliberately does not do. It does not write into a shell: pressing
 * one starts a session of its own, so a build running in your terminal, or half a line
 * you have typed into it, is untouched - and there is no "is the shell ready?" guess to
 * get wrong. It does not run on a glance: the press opens a confirmation naming the exact
 * command and the exact checkout, and carries both, so a pad edited in another window - or
 * a deck that moved while the dialog was open - is refused rather than run. And it does
 * not run twice: what is confirmed here is all a press is, and the caller gives those
 * bytes one id, so a lost answer is the same press arriving again rather than a second
 * run.
 *
 * A pad cannot be pressed until the deck has said which checkout it is on. "The current
 * one" is not a confirmation, and the project's name is not a directory.
 *
 * The result LED comes from a real exit receipt (§4.11) - never from a command merely
 * having stopped printing.
 */
export function CommandPads({
  pads,
  deck,
  worktree,
  disabled,
  problem,
  onRun,
  onAdd,
  onEdit,
  onRemove,
}: {
  pads: readonly TerminalPad[];
  deck: TerminalDeck | null;
  /** The checkout this deck resolved to, or null while that is still unknown. */
  worktree: string | null;
  disabled: string | null;
  problem: string | null;
  /** Given exactly what was confirmed, which is all a press is. */
  onRun: (press: ConfirmedPress) => void;
  onAdd: (label: string, command: string) => void;
  onEdit: (pad: TerminalPad, label: string, command: string) => void;
  onRemove: (pad: TerminalPad) => void;
}) {
  const [confirming, setConfirming] = useState<(TerminalPad & { checkout: string }) | null>(null);
  const [editing, setEditing] = useState<TerminalPad | "new" | null>(null);
  const trigger = useRef<HTMLElement | null>(null);
  const restore = () => trigger.current?.focus();
  const blocked = disabled ?? (worktree ? null : "Waiting for this deck's checkout.");

  const belongs = (row: TerminalCommand, pad: TerminalPad) => row.pad_id === pad.id && row.revision === pad.revision && row.command === pad.command;
  const running = (pad: TerminalPad) => (deck?.running ?? []).some(run =>
    deck?.history.some(row => row.token === run.token && belongs(row, pad)));
  const latest = (pad: TerminalPad): TerminalCommand | undefined =>
    (deck?.history ?? []).find(row => belongs(row, pad));

  return (
    <section className="synth-pads" aria-label="Command pads">
      <header className="synth-pads__header">
        <Silk>Pads</Silk>
        <span className="synth-muted">
          Each press runs in a session of its own. Nothing is typed into a shell you are using.
        </span>
      </header>
      {problem && (
        <p role="alert" className="synth-pads__problem">
          {problem}
        </p>
      )}
      <div className="synth-pads__grid">
        {pads.map((pad) => {
          const receipt = latest(pad);
          return (
            <div key={pad.id} className="synth-pads__cell">
              <CommandPad
                label={pad.label}
                command={pad.command}
                running={running(pad)}
                disabled={!!blocked}
                blockedReason={blocked ?? undefined}
                idleLabel={receipt?.state === "refused" ? "Refused — nothing ran" : receipt && receipt.state !== "finished" ? "Outcome not recorded" : undefined}
                result={
                  receipt?.state === "finished" && receipt.finished_at && receipt.exit_code !== null
                    ? {
                        id: receipt.token,
                        exitCode: receipt.exit_code,
                        finishedAt: Date.parse(receipt.finished_at),
                      }
                    : undefined
                }
                onActivate={() => {
                  trigger.current = document.activeElement as HTMLElement;
                  if (worktree) setConfirming({ ...pad, checkout: worktree });
                }}
              />
              <button
                type="button"
                className="synth-key synth-pads__edit"
                onClick={() => {
                  trigger.current = document.activeElement as HTMLElement;
                  setEditing(pad);
                }}
              >
                Edit {pad.label}
              </button>
            </div>
          );
        })}
        <button
          type="button"
          className="synth-pads__add"
          onClick={() => {
            trigger.current = document.activeElement as HTMLElement;
            setEditing("new");
          }}
        >
          Add pad
        </button>
      </div>
      {pads.length === 0 && (
        <p className="synth-muted">
          No pads yet. Add the commands you run in this project - nothing is suggested for you.
        </p>
      )}

      {confirming && worktree && (
        <div role="dialog" aria-modal="false" aria-label="Run this command" className="synth-pads__confirm">
          <p>
            Run <strong>{confirming.label}</strong>?
          </p>
          <code>{confirming.command}</code>
          <p className="synth-muted">
            In <span className="mono">{confirming.checkout}</span>, in a new managed session. Your shells, and anything typed in
            them, are left alone.
          </p>
          {confirming.checkout !== worktree && <p role="alert">The checkout changed. Cancel and review the command again.</p>}
          <div className="synth-pads__confirm-actions">
            <button
              type="button"
              className="synth-key synth-key--primary"
              autoFocus
              disabled={!!blocked || confirming.checkout !== worktree}
              onClick={() => {
                if (confirming.checkout !== worktree) return;
                const pad = confirming;
                setConfirming(null);
                restore();
                onRun({
                  pad: pad.id,
                  revision: pad.revision,
                  confirmed: pad.command,
                  checkout: pad.checkout,
                });
              }}
            >
              Run it
            </button>
            <button
              type="button"
              className="synth-key"
              onClick={() => {
                setConfirming(null);
                restore();
              }}
            >
              Cancel
            </button>
          </div>
        </div>
      )}

      {editing && (
        <PadForm
          pad={editing === "new" ? null : editing}
          onCancel={() => {
            setEditing(null);
            restore();
          }}
          onSave={(label, command) => {
            if (editing === "new") onAdd(label, command);
            else onEdit(editing, label, command);
            setEditing(null);
            restore();
          }}
          onRemove={
            editing === "new"
              ? undefined
              : () => {
                  onRemove(editing);
                  setEditing(null);
                  restore();
                }
          }
        />
      )}
    </section>
  );
}

function PadForm({
  pad,
  onSave,
  onCancel,
  onRemove,
}: {
  pad: TerminalPad | null;
  onSave: (label: string, command: string) => void;
  onCancel: () => void;
  onRemove?: () => void;
}) {
  const [label, setLabel] = useState(pad?.label ?? "");
  const [command, setCommand] = useState(pad?.command ?? "");
  return (
    <form
      className="synth-pads__form"
      aria-label={pad ? `Edit ${pad.label}` : "Add a pad"}
      onSubmit={(event) => {
        event.preventDefault();
        onSave(label, command);
      }}
    >
      <label>
        Label
        <input value={label} autoFocus maxLength={40} onChange={(event) => setLabel(event.target.value)} />
      </label>
      <label>
        Command
        <input
          className="mono"
          value={command}
          maxLength={2000}
          onChange={(event) => setCommand(event.target.value)}
        />
      </label>
      <div className="synth-pads__confirm-actions">
        <button type="submit" className="synth-key synth-key--primary" disabled={!label.trim() || !command.trim()}>
          Save pad
        </button>
        <button type="button" className="synth-key" onClick={onCancel}>
          Cancel
        </button>
        {onRemove && (
          <button type="button" className="synth-key synth-key--danger" onClick={onRemove}>
            Remove pad
          </button>
        )}
      </div>
    </form>
  );
}
