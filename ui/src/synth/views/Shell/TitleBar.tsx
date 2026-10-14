import "./shell.css";

/** Existing app context only. No inferred global metrics or second data controller. */
export default function TitleBar({ context, version }: { context: string; version?: string }) {
  return <header className="synth-titlebar" aria-label="AI Team workspace">
    <span className="synth-logo" aria-hidden="true"><svg viewBox="0 0 32 32" fill="none" stroke="currentColor" strokeWidth="2.5">
      <circle cx="8" cy="9" r="3" /><circle cx="24" cy="23" r="3" /><path d="M8 12v5c0 11 16-13 16-2v5" />
    </svg></span>
    <strong>AI Team</strong><span className="synth-titlebar__context" title={context}>{context}</span>
    <span className="synth-titlebar__version">{version ? `v${version}` : "Version unavailable"}</span>
  </header>;
}
