import { useState } from "react";
import type { Updates } from "./Update";

export default function UpdateDetails({ updates }: { updates: Updates }) {
  const { available, problem, checking, working, installed, check, install, inspect } = updates;
  const [approvedSessions, setApprovedSessions] = useState<string | null>(null);
  const sessions = available?.terminals;
  const needsClosure = Boolean(sessions?.sessions.length);
  const approved = needsClosure && approvedSessions === sessions?.approval;
  return <section className="settings__group updates" aria-label="Application updates">
    <h3>Application updates</h3>
    <p className="faint">Checks on opening, every 15 minutes and when returning after a pause. Nothing is installed automatically.</p>
    {available && <p>Running {available.current}{available.latest ? ` · Latest ${available.latest}` : " · Latest release could not be checked"}</p>}
    <ul className="updates__targets">{available?.targets?.map(target => <li key={target.path}>
      <strong>{target.name}</strong> · {target.version ?? "version unavailable"}<span className="mono">{target.path}</span>
    </li>)}</ul>
    {available?.method === "source" && <p>This replaces the local build with the reviewed published release, not another source build.</p>}
    {available?.blocked && <p role="status">{available.blocked}</p>}
    {problem && <p className="error" role="alert">{problem}</p>}
    {updates.notice && <p>{updates.notice}</p>}
    {available?.state === "inspection" && <button type="button" className="button" disabled={working} onClick={() => void inspect()}>Inspect interrupted update</button>}
    {(!working && available?.state !== "inspection" && (installed || available?.state === "restart")) ? <p role="status">{installed ? `Installed ${installed}.` : "The installed files changed."} <strong>Restart AI Team</strong> to use it. Your running window stays on its current version until you quit.</p> : <>
      <p className="faint">The update can close the app sessions listed below and wait for them to finish. It replaces the listed programs, keeps recovery backups and preserves your chats, settings and unsent drafts. It does not stop chat agents or team builds, or restart the app for you.</p>
      {needsClosure && <>
        <ul className="updates__targets">{sessions?.sessions.map(session => <li key={session.id}>
          <strong>{session.label} · Session {session.id}</strong><span className="mono">{session.worktree}</span>
        </li>)}</ul>
        <label className="settings__toggle"><input type="checkbox" checked={approved} disabled={working} onChange={event => setApprovedSessions(event.target.checked ? sessions?.approval ?? null : null)} /><span>Close these app terminal/sign-in sessions, including any commands in them. Save terminal input first.</span></label>
      </>}
      <button type="button" className="button button--primary" disabled={!available?.can_update || !available.approval || working || checking || (needsClosure && !approved)} onClick={() => void install(approved ? sessions?.approval : undefined)}>
        {working ? "Updating…" : needsClosure ? "Close sessions and update" : "Update listed programs"}
      </button>
    </>}
    <button type="button" className="button" disabled={checking || working} onClick={() => void check(true)}>{checking ? "Checking…" : "Check for updates"}</button>
    {available?.release_url && <a href={available.release_url} target="_blank" rel="noreferrer">Release notes and downloads ↗</a>}
  </section>;
}
