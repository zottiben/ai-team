import "./shell.css";
import { ChatMeters } from "./ChatMeters";

/** Existing app context and the selected chat's single feed; no global polling. */
export default function TitleBar({ context, version, chat = null, project = null }: { context: string; version?: string; chat?: number | null; project?: number | null }) {
  return <header className="synth-titlebar" aria-label="AI Team workspace">
    <span className="synth-logo" aria-hidden="true"><svg viewBox="0 0 32 32" fill="none" stroke="currentColor" strokeWidth="2.5">
      <circle cx="8" cy="9" r="3" /><circle cx="24" cy="23" r="3" /><path d="M8 12v5c0 11 16-13 16-2v5" />
    </svg></span>
    <strong>AI Team</strong><span className="synth-titlebar__context" title={context}>{context}</span>
    <ChatMeters chat={chat} project={project}/>
    <span className="synth-titlebar__version">{version ? `v${version}` : "Version unavailable"}</span>
  </header>;
}
