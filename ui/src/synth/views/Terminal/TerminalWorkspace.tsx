import { useCallback, useEffect, useId, useMemo, useRef, useState } from "react";

import {
  addTerminalPad,
  closeTerminal,
  editTerminalPad,
  openTerminal,
  removeTerminalPad,
  runTerminalPad,
  terminalDeck,
  terminalPads,
  terminals as listTerminals,
  type ConfirmedPress,
  type PressedCommand,
  type RunEvent,
  type TerminalDeck,
  type TerminalPad,
  type TerminalPress,
  type TerminalSession,
  type Where,
} from "../../../api";
import { setScreenEffects, useScreenEffects } from "../../../ui-preferences";
import { SynthProvider, useAnnounce } from "../../SynthProvider";
import { Silk } from "../../primitives/Silk";
import { Switch } from "../../primitives/Switch";
import { agentShell } from "./agent-shell";
import { AgentMirror } from "./AgentMirror";
import { CommandPads } from "./CommandPads";
import { ProcessRack } from "./ProcessRack";
import { createRateSampler } from "./rate";
import { Screen, type Renderer } from "./Screen";
import { AGENT_TAB, ShellTabs } from "./ShellTabs";

import "../../tokens.css";
import "../../primitives.css";
import "./terminal.css";

/** How often the command deck is re-read while the workspace is open. */
const DECK_POLL = 1_000;
const pressKey = (press: ConfirmedPress) => JSON.stringify([press.pad, press.revision, press.confirmed, press.checkout]);

/**
 * What to say about a press that answered with something other than a fresh run.
 *
 * None of these is an error - the server answered, and the row it answered with is the
 * truth about that press. But none of them is silence either: the operator pressed a
 * button, and is owed what happened to it, including the one case where nobody can say
 * whether the command ever started.
 */
function outcome(receipt: PressedCommand): string | null {
  if (receipt.state === "uncertain")
    return `${receipt.label} was already sent, and whether it started was never recorded. It is not being run again - check the session list before pressing it.`;
  if (receipt.state === "refused")
    return `${receipt.label} ran nothing: ${receipt.refusal ?? "no reason was recorded"}. Press it again once that has cleared.`;
  if (receipt.replayed)
    return `${receipt.label} was already sent; showing that run rather than starting another.`;
  return null;
}

/**
 * The terminal workspace (§12).
 *
 * Your shells and the agent's, on one screen, with the command deck beside them. Three
 * boundaries are structural rather than enforced by care:
 *
 * - The agent's tab has no session, so there is nowhere for a keystroke to go.
 * - A pad never writes to a session; it starts one. Nothing is appended to a shell that
 *   is busy, and nothing lands on top of a half-typed line.
 * - A selection becomes a draft chip and nothing else. It is quoted output - it does not
 *   send, and it cannot grant an agent any permission it did not already have.
 */
export default function TerminalWorkspace({
  project,
  synth = true,
  agentActive = false,
  workspace = null,
  node,
  events = [],
  onSend,
  onClose,
}: {
  project: string;
  synth?: boolean;
  agentActive?: boolean;
  workspace?: string | null;
  node: number | null;
  /** The chat's own event feed, which is where the read-only mirror comes from. */
  events?: readonly RunEvent[];
  onSend?: (text: string, source?: "operator_terminal" | "agent_shell") => void;
  onClose?: () => void;
}) {
  const panelId = useId();
  const where: Where = useMemo(() => ({ project, workspace, node }), [project, workspace, node]);
  const [sessions, setSessions] = useState<TerminalSession[]>([]);
  const [selected, setSelected] = useState<string>("");
  const [pads, setPads] = useState<readonly TerminalPad[]>([]);
  const [deck, setDeck] = useState<TerminalDeck | null>(null);
  const [problem, setProblem] = useState<string | null>(null);
  const [padProblem, setPadProblem] = useState<string | null>(null);
  const [renderer, setRenderer] = useState<Renderer | null>(null);
  const [dropped, setDropped] = useState<Record<number, number>>({});
  const [busy, setBusy] = useState(false);
  const effects = useScreenEffects();
  const alive = useRef(true);
  /** Which checkout this window has already opened a shell for. */
  const started = useRef<string | null>(null);
  const knownSessions = useRef(new Set<number>());
  /** The press in flight, and the id it is known by until it is answered. */
  const presses = useRef(new Map<string, string>());
  // Collection belongs to the stream, not to a frame (§13.1): the scope reads a snapshot
  // this sampler already holds.
  const rates = useRef(new Map<number, ReturnType<typeof createRateSampler>>());

  useEffect(() => {
    alive.current = true;
    return () => {
      alive.current = false;
    };
  }, []);

  const say = (error: unknown) => {
    if (alive.current) setProblem(error instanceof Error ? error.message : String(error));
  };

  /**
   * Rejoin before opening. A window that opened a session every time it mounted would
   * leave the four-minute `cargo test` behind and start another shell beside it.
   *
   * A checkout with nothing open gets one shell, once - opening the terminal is asking
   * for one. Once per checkout, so closing the last tab leaves it closed rather than
   * starting another on the way out.
   */
  const attach = useCallback(async () => {
    const mine = (found: TerminalSession[]) =>
      found.filter((session) => !session.done || session.kind === "command" || knownSessions.current.has(session.id));
    try {
      let open = mine(await listTerminals(where));
      if (!alive.current) return;
      const checkout = `${project}\u0000${workspace ?? ""}\u0000${node ?? ""}`;
      if (open.length === 0 && started.current !== checkout) {
        started.current = checkout;
        await openTerminal(where);
        open = mine(await listTerminals(where));
        if (!alive.current) return;
      }
      knownSessions.current = new Set(open.map(session => session.id));
      setSessions(open);
      setProblem(null);
      setSelected((current) => {
        if (current === AGENT_TAB || open.some((session) => String(session.id) === current)) return current;
        return String(open[0]?.id ?? AGENT_TAB);
      });
    } catch (error: unknown) {
      say(error);
    }
  }, [where, project, workspace, node]);

  useEffect(() => {
    void attach();
  }, [attach]);

  useEffect(() => {
    if (!synth) return;
    let current = true;
    void terminalPads(project)
      .then((found) => current && alive.current && setPads(found))
      .catch(say);
    return () => {
      current = false;
    };
  }, [project, synth]);

  // Polled rather than pushed, like every other view (§19.11): `ait ui` and the process
  // that runs a command are the same process here, but the window is still a view.
  useEffect(() => {
    if (!synth) return;
    let current = true;
    const read = async () => {
      try {
        const found = await terminalDeck(where);
        if (current && alive.current) setDeck(found);
      } catch (error: unknown) {
        say(error);
      }
    };
    void read();
    const timer = setInterval(() => void read(), DECK_POLL);
    return () => {
      current = false;
      clearInterval(timer);
    };
  }, [where, synth]);

  const agent = useMemo(() => agentShell(events), [events]);
  const lastOperator = useRef("");
  if (selected !== AGENT_TAB) lastOperator.current = selected;
  const displayed = !synth && selected === AGENT_TAB ? lastOperator.current : selected;

  /**
   * One id per press, kept until the server answers.
   *
   * A failed call may be a refusal or a lost answer, and the two are not told apart from
   * here - so the id stays, and pressing the same pad again is the *same* press, which
   * the server answers with the receipt it already has instead of running the command
   * twice. The key is exactly what the server fingerprints, so a press that differs in
   * any way gets an id of its own and can never collide with the one before it.
   */
  const identify = (confirmed: ConfirmedPress): TerminalPress => {
    const key = pressKey(confirmed);
    let id = presses.current.get(key);
    if (!id) { id = crypto.randomUUID(); presses.current.set(key, id); }
    return { ...confirmed, request_id: id };
  };

  const act = async (work: () => Promise<unknown>) => {
    if (busy) return;
    setBusy(true);
    setProblem(null);
    try {
      await work();
    } catch (error: unknown) {
      say(error);
    } finally {
      if (alive.current) setBusy(false);
    }
  };

  return (
    <SynthProvider>
      {synth && <DeckAnnouncer deck={deck} />}
      <section className={synth ? "synth-terminal" : "terminal terminal-workspace"} aria-label="Terminal">
        {!synth && <div className="main__header">
          <div><h2>Terminal</h2>{displayed && <span className="faint mono">session {displayed}</span>}</div>
          <button type="button" className="button" disabled={busy} onClick={() => void act(async () => {
            if (displayed) await closeTerminal(Number(displayed));
            started.current = null;
            await attach();
          })}>Restart</button>
        </div>}
        {synth && <header className="synth-terminal__toolbar">
          <Silk>Terminal</Silk>
          <span className="synth-terminal__badge">Operator</span>
          <span className="synth-muted">
            Runs in your checkout as you - an operator tool, not an OS sandbox.
          </span>
          <Switch
            label="Screen effects"
            checked={effects}
            onChange={(next) => setScreenEffects(next)}
          />
          {onClose && (
            <button type="button" className="synth-key" onClick={onClose} aria-label="Close the terminal">
              ×
            </button>
          )}
        </header>}

        {problem && (
          <p role="alert" className="synth-terminal__problem">
            {problem}
          </p>
        )}

        {synth && <ShellTabs
          panelId={panelId}
          sessions={sessions}
          selected={selected}
          agentRunning={agentActive}
          busy={busy}
          onSelect={setSelected}
          onOpen={() =>
            void act(async () => {
              const opened = await openTerminal(where);
              await attach();
              if (alive.current) setSelected(String(opened.id));
            })
          }
          onClose={(id) =>
            void act(async () => {
              await closeTerminal(id);
              await attach();
            })
          }
        />}

        <div className={synth ? "synth-terminal__deck" : "terminal-workspace__deck"}>
          <div id={panelId} role={synth ? "tabpanel" : undefined} aria-labelledby={synth ? `${panelId}-tab-${selected}` : undefined}>
            <div hidden={!synth || displayed !== AGENT_TAB}><AgentMirror commands={agent} effects={effects} onSend={onSend ? text => onSend(text, "agent_shell") : undefined} /></div>
            {sessions.map((session) => <div key={session.id} hidden={String(session.id) !== displayed}>
              <Screen
                session={session.id}
                synth={synth}
                effects={synth && effects}
                label={session.kind === "command" ? `${session.label} output` : "Shell"}
                handlers={{
                  onOutput: (text) => {
                    let rate = rates.current.get(session.id);
                    if (!rate) { rate = createRateSampler(); rates.current.set(session.id, rate); }
                    rate.add(text);
                  },
                  onDropped: (bytes) => setDropped(current => ({ ...current, [session.id]: (current[session.id] ?? 0) + bytes })),
                  onRenderer: setRenderer,
                  onEnded: () => void attach(),
                  onSend,
                }}
              />
            </div>)}
          </div>

          {synth && <ProcessRack
            deck={deck}
            rate={() => rates.current.get(Number(displayed))?.series() ?? []}
            active={agentActive && selected !== AGENT_TAB && sessions.some(session => String(session.id) === displayed && !session.done)}
            agent={agent}
            renderer={renderer}
            dropped={dropped[Number(displayed)] ?? 0}
          />}
        </div>

        {synth && <CommandPads
          pads={pads}
          deck={deck}
          worktree={deck?.worktree ?? null}
          disabled={busy ? "Another terminal action is in flight." : null}
          problem={padProblem}
          onRun={(confirmed) =>
            void act(async () => {
              setPadProblem(null);
              try {
                const started = await runTerminalPad(where, identify(confirmed));
                // Answered, so this press is over: the next one is a press of its own.
                presses.current.delete(pressKey(confirmed));
                await attach();
                if (!alive.current) return;
                if (started.session_id !== null) setSelected(String(started.session_id));
                setPadProblem(outcome(started));
              } catch (error: unknown) {
                // No answer arrived, so the press keeps its id: this may have been a lost
                // response rather than a refusal, and pressing again under a new one is
                // how a command gets run twice. Unchanged bytes retry as the same press;
                // edit the pad and it is a different press, with a new id, as it should be.
                if (alive.current)
                  setPadProblem(error instanceof Error ? error.message : String(error));
              }
            })
          }
          onAdd={(label, command) =>
            void act(async () => {
              const added = await addTerminalPad(project, label, command);
              if (alive.current) setPads((current) => [...current, added]);
            })
          }
          onEdit={(pad, label, command) =>
            void act(async () => {
              const edited = await editTerminalPad(pad.id, project, label, command);
              if (alive.current)
                setPads((current) => current.map((item) => (item.id === pad.id ? edited : item)));
            })
          }
          onRemove={(pad) =>
            void act(async () => {
              await removeTerminalPad(pad.id, project);
              if (alive.current) setPads((current) => current.filter((item) => item.id !== pad.id));
            })
          }
        />}
      </section>
    </SynthProvider>
  );
}

/**
 * "Run finished" through the one polite live region (§18).
 *
 * The history that was already there when the workspace opened is not news, so the
 * first read only records where it had got to.
 */
function DeckAnnouncer({ deck }: { deck: TerminalDeck | null }) {
  const announce = useAnnounce();
  const seen = useRef<string | null>(null);
  useEffect(() => {
    const settled = deck?.history.find((row) => row.state !== "running");
    if (!settled || settled.token === seen.current) return;
    const first = seen.current === null;
    seen.current = settled.token;
    if (first) return;
    announce(
      settled.state === "finished"
        ? `${settled.label} finished with exit ${settled.exit_code ?? "no status"}`
        : `${settled.label} has no recorded outcome`,
    );
  }, [deck, announce]);
  return null;
}
