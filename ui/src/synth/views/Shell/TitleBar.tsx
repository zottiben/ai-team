import type { ReactNode } from "react";
import { SynthProvider } from "../../SynthProvider";
import { PresetSelector } from "../../primitives/PresetSelector";
import "./shell.css";
import { ChatMeters } from "./ChatMeters";

/** Existing sidebar scope and navigation; never fetches or guesses another chat. */
export default function TitleBar({ context, version, chat = null, project = null, sessions, onSession, notifications }: {
  context: string; version?: string; chat?: number | null; project?: number | null;
  sessions: { id: string; label: string }[]; onSession: (id: string) => void; notifications: ReactNode;
}) {
  const index = sessions.findIndex(session => session.id === String(chat));
  return <header className="synth-titlebar" aria-label="AI Team workspace">
    <div className="synth-titlebar__brand">
      <span className="synth-logo" aria-hidden="true"><svg width="20" height="20" viewBox="0 0 20 20">
        <circle cx="5.5" cy="6" r="2.6" fill="currentColor"/><circle cx="14.5" cy="14" r="2.6" fill="currentColor"/>
        <path d="M5.5 8.6c0 8 9-2.6 9 2.8" fill="none" stroke="currentColor" strokeWidth="1.9" strokeLinecap="round"/>
      </svg></span>
      <div><strong>AI Team</strong><span className="synth-titlebar__credit">Powered by Pi</span></div>
    </div>
    <div className="synth-titlebar__context" title={context}>
      <SynthProvider><PresetSelector label="Session" options={sessions} value={index < 0 ? context : String(chat)} onChange={onSession}/></SynthProvider>
      <span className="synth-titlebar__position">{index < 0 ? `${chat === null ? "Browse" : "Outside filter"} · ${sessions.length} shown` : `${index + 1} / ${sessions.length}`}</span>
    </div>
    <div className="synth-titlebar__right"><ChatMeters chat={chat} project={project}/>{notifications}
      <span className="synth-titlebar__version">{version ? `v${version}` : "Version unavailable"}</span>
    </div>
  </header>;
}
