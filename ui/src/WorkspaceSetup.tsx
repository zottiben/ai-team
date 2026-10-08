import { lazy, Suspense, useEffect, useRef, useState } from "react";
import {
  commandWorkspaceSetup,
  startWorkspaceSetup,
  workspaceSetups,
  type WorkspaceSetupReceipt,
} from "./chat-workspace-api";

const Form = lazy(() => import("./WorkspaceSetupForm"));

/** Mounted with the composer, not the popover: closing a menu cannot lose a pending
 * request's identity or silently unblock Send during checkout setup. */
export function useWorkspaceSetup(
  project: string,
  enabled: boolean,
  tick: number,
) {
  const [items, setItems] = useState<WorkspaceSetupReceipt[]>([]);
  const [loaded, setLoaded] = useState(false);
  const [problem, setProblem] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [uncertain, setUncertain] = useState(false);
  const [refresh, setRefresh] = useState(0);
  const working = useRef(false);
  const serial = useRef(0);
  const request = useRef<{ id: string; branch: string } | null>(null);
  useEffect(() => {
    if (!enabled) return;
    const generation = ++serial.current;
    let current = true;
    void workspaceSetups(project)
      .then((found) => {
        if (!current || serial.current !== generation) return;
        setItems((previous) =>
          found.map((item) => {
            const newer = previous.find(
              (old) => old.id === item.id && old.rev > item.rev,
            );
            return newer ?? item;
          }),
        );
        setLoaded(true);
        if (
          request.current &&
          found.some(
            (item) =>
              item.request_id === request.current?.id &&
              item.branch === request.current.branch,
          )
        ) {
          request.current = null;
          setUncertain(false);
          setProblem(null);
        }
      })
      .catch((error: unknown) => {
        if (current && serial.current === generation)
          setProblem(error instanceof Error ? error.message : String(error));
      });
    return () => {
      current = false;
    };
  }, [project, enabled, tick, refresh]);
  const act = async (work: () => Promise<WorkspaceSetupReceipt>) => {
    if (working.current) return;
    working.current = true;
    ++serial.current;
    setBusy(true);
    setProblem(null);
    try {
      const receipt = await work();
      setItems((previous) => [
        receipt,
        ...previous.filter((item) => item.id !== receipt.id),
      ]);
    } catch (error: unknown) {
      setProblem(error instanceof Error ? error.message : String(error));
    } finally {
      working.current = false;
      setBusy(false);
      setRefresh((value) => value + 1);
    }
  };
  const pending =
    enabled &&
    (busy ||
      uncertain ||
      items.some((item) =>
        ["pending", "running", "inspection"].includes(item.state),
      ));
  return {
    items,
    loaded,
    busy,
    uncertain,
    pending,
    problem,
    requestBranch: request.current?.branch ?? "",
    refresh: () => setRefresh((value) => value + 1),
    start: (branch: string) =>
      act(async () => {
        request.current ??= { id: crypto.randomUUID(), branch: branch.trim() };
        setUncertain(true);
        const receipt = await startWorkspaceSetup(
          project,
          request.current.id,
          request.current.branch,
        );
        request.current = null;
        setUncertain(false);
        return receipt;
      }),
    command: (
      receipt: WorkspaceSetupReceipt,
      action: "inspect" | "retry_dependencies",
    ) => act(() => commandWorkspaceSetup(project, receipt, action)),
  };
}

export function WorkspaceSetup(props: {
  state: ReturnType<typeof useWorkspaceSetup>;
  disabled: boolean;
  onSelect: (path: string) => void;
}) {
  return <Suspense fallback={<p className="faint">Loading setup controls…</p>}><Form {...props} /></Suspense>;
}
