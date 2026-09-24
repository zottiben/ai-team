// The lifeline: a turn ends when the ai-team process supervising it does.
//
// ai-team starts every turn as `pi` in a process group of its own and reads it through a
// pipe. When that process dies without stopping the turn - a crash, a full disk, an app
// quit before it could unwind - nothing tells Pi. Its stdin is /dev/null, and Node ignores
// SIGPIPE, so a closed stdout does not end it either. The turn carries on in its worktree
// with nobody recording what it does, and Resume is refused for as long as it runs,
// because something is still running there. Two such turns sat for hours before this.
//
// So a turn watches the process that started it. Once that process has gone - the turn
// has been handed to another parent - it stops its own process group: itself, the model
// CLI it drives, and the MCP servers and tools it started. What it wrote stays in the
// worktree and its conversation in its Pi session, which is what Resume carries on from.

/** How often a turn looks for its supervisor. */
export const EVERY_MS = 1000;

/** How long a turn told to stop has before it is made to. */
export const GRACE_MS = 5000;

export interface Lifeline {
  /** What happens once the supervisor has gone. */
  stop?: () => void;
  /** Who the turn's parent is now. A parameter so a test can say. */
  parent?: () => number;
  every?: number;
}

/** Watch for the supervisor going. Returns what lets go of the watch. */
export function holdOn({
  stop = stopTurn,
  parent = () => process.ppid,
  every = EVERY_MS,
}: Lifeline = {}): () => void {
  const supervisor = parent();
  const timer = setInterval(() => {
    if (parent() !== supervisor) {
      clearInterval(timer);
      stop();
    }
  }, every);
  // Never the reason a turn that has finished stays alive.
  timer.unref();
  return () => clearInterval(timer);
}

/**
 * Stop this turn and everything it started.
 *
 * Its process group, which ai-team made it lead. Asked first, so Pi can close its session
 * as it would at any other end, and then made to, in case it does not.
 */
export function stopTurn(): void {
  signal("SIGTERM");
  setTimeout(() => signal("SIGKILL"), GRACE_MS).unref();
}

function signal(name: "SIGTERM" | "SIGKILL"): void {
  try {
    process.kill(-process.pid, name);
  } catch {
    // Not a group leader - started some other way than ai-team starts a turn - so there
    // is no group of its own to stop, and stopping itself is all it may do.
    process.kill(process.pid, name);
  }
}
