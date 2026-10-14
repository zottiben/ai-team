import { createContext, useCallback, useContext, useEffect, useId, useMemo, useState, type ReactNode } from "react";

interface ViewSignals {
  attention: string | undefined;
  register: (id: string) => () => void;
  announce: (message: string) => void;
}
const Signals = createContext<ViewSignals | null>(null);

/** One host per visible view: one attention pulse and one polite announcement stream. */
export function SynthProvider({ children }: { children: ReactNode }) {
  const [requests, setRequests] = useState<string[]>([]);
  const [notice, setNotice] = useState({ text: "", sequence: 0 });
  const register = useCallback((id: string) => {
    setRequests(old => old.includes(id) ? old : [...old, id]);
    return () => setRequests(old => old.filter(entry => entry !== id));
  }, []);
  const announce = useCallback((text: string) => setNotice(old => ({ text, sequence: old.sequence + 1 })), []);
  const value = useMemo(() => ({ attention: requests[0], register, announce }), [requests, register, announce]);
  return <Signals.Provider value={value}>
    {children}
    <span className="synth-sr-only" aria-live="polite" aria-atomic="true"><span key={notice.sequence}>{notice.text}</span></span>
  </Signals.Provider>;
}

export function useAttention(requested: boolean): boolean {
  const signals = useContext(Signals);
  const id = useId();
  const register = signals?.register;
  useEffect(() => requested ? register?.(id) : undefined, [id, register, requested]);
  // Without a view owner attention stays solid; never create uncoordinated loops.
  return requested && signals?.attention === id;
}

export function useAnnounce() {
  const signals = useContext(Signals);
  if (!signals) throw new Error("Synth selectors need a SynthProvider for announcements");
  return signals.announce;
}
