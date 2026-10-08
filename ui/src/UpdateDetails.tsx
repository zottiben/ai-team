import type { Updates } from "./Update";

export default function UpdateDetails({ updates }: { updates: Updates }) {
  const { available, problem, checking, working, installed, check, install, inspect } = updates;
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
      <p className="faint">Save unsaved edits, finish active work and close app terminals or sign-in sessions first. The update replaces the listed programs, keeps recovery backups and preserves your chats and settings. It does not stop agents or restart the app for you.</p>
      <button type="button" className="button button--primary" disabled={!available?.can_update || !available.approval || working || checking} onClick={() => void install()}>
        {working ? "Updating…" : "Update listed programs"}
      </button>
    </>}
    <button type="button" className="button" disabled={checking || working} onClick={() => void check(true)}>{checking ? "Checking…" : "Check for updates"}</button>
    {available?.release_url && <a href={available.release_url} target="_blank" rel="noreferrer">Release notes and downloads ↗</a>}
  </section>;
}
