import { useEffect, useLayoutEffect, useRef, useState, type ReactNode, type RefObject } from "react";
import { Popover } from "../../../Popover";
import { FileView } from "../../../ReviewFile";
import { previewEvidence, type PreviewEvidence, type PreviewFamily } from "../../../preview-api";
import { useSynth } from "../../../ui-preferences";
import "./review.css";

type Receipt = { id: number; chat_id: number; rev: number; evidence?: PreviewEvidence | null };
type Props = {
  family: PreviewFamily; receipt: Receipt; label: string;
  summary: ReactNode; children: (evidenceReady: boolean) => ReactNode; returnFocus: RefObject<HTMLElement | null>;
};
export function ExactApproval(props: Props) {
  const synth = useSynth();
  return <Approval key={`${props.family}:${props.receipt.chat_id}:${props.receipt.id}`} {...props} synth={synth}/>;
}
function displayed(element: HTMLElement | null): boolean {
  if (!element?.isConnected || document.hidden) return false;
  for (let node: HTMLElement | null = element; node; node = node.parentElement) {
    const style = getComputedStyle(node);
    if (node.hidden || style.display === "none" || style.visibility === "hidden" || style.visibility === "collapse") return false;
    if (node instanceof HTMLDetailsElement && !node.open && !node.querySelector("summary")?.contains(element)) return false;
  }
  return true;
}
function Approval({ family, receipt, label, summary, children, returnFocus, synth }: Props & { synth: boolean }) {
  const host = useRef<HTMLElement>(null), anchor = useRef<HTMLButtonElement>(null), content = useRef<HTMLDivElement>(null);
  const automatic = useRef(synth);
  const [open, setOpen] = useState(false);
  const [captured, setCaptured] = useState(receipt.evidence);
  const [problem, setProblem] = useState<string | null>(null), [attempt, setAttempt] = useState(0);
  const request = useRef<Promise<PreviewEvidence | null> | null>(null);
  useEffect(() => {
    if (!automatic.current) return;
    automatic.current = false;
    if (displayed(host.current)) {
      anchor.current?.scrollIntoView?.({ block: "nearest" });
      setOpen(true);
    }
  }, []);
  useEffect(() => {
    if (!synth) { setOpen(false); return; }
    if (!open) return;
    // A refresh can move the trigger offscreen without hiding the confirmation.
    // Close only when its owning view/document is actually hidden, not on intersection.
    const changed = () => { if (!displayed(host.current)) setOpen(false); };
    changed();
    const observer = new MutationObserver(changed);
    for (let node: HTMLElement | null = host.current; node; node = node.parentElement) {
      observer.observe(node, { attributes: true, attributeFilter: ["hidden", "style", "class", "open"] });
    }
    document.addEventListener("visibilitychange", changed);
    window.addEventListener("resize", changed);
    return () => { observer.disconnect(); document.removeEventListener("visibilitychange", changed); window.removeEventListener("resize", changed); };
  }, [synth, open]);
  useEffect(() => {
    if (!synth || !open || !displayed(host.current) || captured !== undefined) return;
    let current = true;
    request.current ??= previewEvidence(receipt.chat_id, family, receipt.id);
    void request.current.then(value => { if (current) setCaptured(value); })
      .catch((error: unknown) => { if (current) setProblem(error instanceof Error ? error.message : String(error)); });
    return () => { current = false; };
  }, [synth, open, captured, family, receipt.chat_id, receipt.id, attempt]);
  useLayoutEffect(() => () => {
    const active = document.activeElement;
    if (active instanceof Node && (host.current?.contains(active) || content.current?.contains(active))) {
      returnFocus.current?.focus({ preventScroll: true });
    }
  }, [returnFocus]);
  const close = () => {
    if (content.current?.contains(document.activeElement)) anchor.current?.focus({ preventScroll: true });
    setOpen(false);
  };
  if (!synth) return <section ref={host} className="notice" aria-label={label}>{summary}{children(true)}</section>;
  const scope = `${family}:${receipt.chat_id}:${receipt.id}`;
  return <section ref={host} className="notice synth-approval-trigger" aria-label={label}>
    <p>Captured {family} approval #{receipt.id} · revision {receipt.rev}. Nothing is approved by inspecting it.</p>
    <button ref={anchor} type="button" className="button" aria-haspopup="dialog" aria-expanded={open}
      onClick={() => { if (open) close(); else setOpen(true); }}>Inspect captured {family} approval</button>
    {open && <Popover anchor={anchor} label={label} role="dialog" closeOnBlur width={660}
      initialFocus="[data-approval-heading]" className="synth-exact-approval" onClose={close}>
      <div ref={content}>
        <h3 tabIndex={-1} data-approval-heading>{label}</h3>
        {summary}
        <p className="faint">Receipt #{receipt.id} · revision {receipt.rev}. Captured presentation, not proof of reading, verification or permission. The existing command rechecks the snapshot when approved.</p>
        {captured === undefined ? problem ? <div role="alert" className="notice"><p>Captured evidence could not be read: {problem}</p>
          <button type="button" className="button" onClick={() => { request.current = null; setProblem(null); setAttempt(n => n + 1); }}>Retry captured evidence</button></div>
          : <p role="status">Reading captured evidence…</p>
          : captured === null ? <p className="notice">No pinned code evidence was recorded. No current diff has been substituted; this is not proof of no changes.</p>
            : <section aria-label="Captured code evidence" className="synth-approval-evidence">
              <p>{captured.basis}</p>
              <p>{captured.files.length} captured file entries · {captured.files_total === null ? "total unavailable" : `${captured.files_total} in the captured comparison`}{captured.omitted_files > 0 ? ` · ${captured.omitted_files} omitted` : ""}</p>
              {captured.note && <p className="notice">{captured.note}</p>}
              {captured.files.map((entry, index) => <div key={`${entry.area}:${entry.file.path}:${index}`}>
                <p className="faint">{entry.area}{entry.hunks_omitted ? " · text hunks omitted" : ""}</p>
                <FileView file={entry.file} reason={entry.reason} revision={entry.revision ?? undefined}
                  reviewKey={`approval:${scope}:${index}`} initiallyCollapsed={index >= 3}/>
              </div>)}
            </section>}
        <div className="synth-approval-controls">{children(captured !== undefined)}</div>
        <button type="button" className="button" onClick={close}>Close captured preview</button>
      </div>
    </Popover>}
  </section>;
}
