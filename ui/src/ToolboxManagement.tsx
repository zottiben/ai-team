import { useEffect, useState } from "react";
import { registerProject, type Registered as RegisteredProject } from "./api";
import { toolboxCatalogue, type ToolboxCatalogue } from "./toolbox-api";
import { discoverProjects, previewRegistry, previewUser, registrations, userAuthority, type Authority, type Discovery, type Operation, type Registration, type RegistrySelection } from "./toolbox-operations-api";
import { OperationHistory, OperationPreview } from "./ToolboxOperation";
import "./toolbox.css";

export function ToolboxManagement({ mode, onChanged, onRegistered }: { mode: "user" | "registry"; onChanged: () => void; onRegistered: (value: RegisteredProject) => void }) {
  return <section className="toolbox" aria-label={mode === "user" ? "User-level toolbox setup" : "Project discovery and registrations"}>{mode === "user" ? <UserSetup /> : <Registry onChanged={onChanged} onRegistered={onRegistered} />}</section>;
}
function UserSetup() {
  const [authority, setAuthority] = useState<Authority | null>(null);
  const [catalogue, setCatalogue] = useState<ToolboxCatalogue | null>(null);
  const [harnesses, setHarnesses] = useState(["pi"]);
  const [skills, setSkills] = useState<string[]>([]);
  const [charter, setCharter] = useState(false);
  const [path, setPath] = useState("");
  const [copies, setCopies] = useState(false);
  const [preview, setPreview] = useState<Operation | null>(null);
  const [busy, setBusy] = useState(false);
  const [problem, setProblem] = useState<string | null>(null);
  useEffect(() => { let current = true; void Promise.all([userAuthority(), toolboxCatalogue()]).then(([a, c]) => { if (current) { setAuthority(a); setCatalogue(c); } }).catch((e: unknown) => { if (current) setProblem(String(e)); }); return () => { current = false; }; }, []);
  return <><h3>User-level setup</h3><p className="notice">Separate approval: changes apply across projects, not just the selected project. Nothing installs software or rewrites global MCP registrations.</p>
    {authority?.kind === "user" && <p>Home: {authority.home} · Pi: {authority.pi_agent}</p>}
    <fieldset className="toolbox__choices" disabled={busy || !catalogue || !authority} onChange={() => setPreview(null)}><legend>Choose user configuration</legend>
      <Checks label="User harnesses" options={["pi", "claude", "codex"]} selected={harnesses} onChange={setHarnesses} />
      <Checks label="Global skills" options={catalogue?.skills.map((i) => i.key) ?? []} selected={skills} onChange={setSkills} />
      <label><input type="checkbox" checked={copies} onChange={(e) => setCopies(e.target.checked)} /> Independent global Claude skill copies</label>
      <label><input type="checkbox" checked={charter} onChange={(e) => setCharter(e.target.checked)} /> Append base charter to global rules</label>
      <label className="toolbox__path">Custom charter file (optional absolute path)<input value={path} onChange={(e) => setPath(e.target.value)} /></label>
      <p className="faint">Without a custom path: Claude ~/.claude/CLAUDE.md; Codex ~/.codex/AGENTS.md; Pi's configured agent directory/AGENTS.md. Existing knowledge and existing charter blocks are preserved.</p>
      <button className="button" disabled={!harnesses.length || (!skills.length && !charter)} onClick={() => { setBusy(true); setProblem(null); setPreview(null); void previewUser({ harnesses, skills, no_symlink: copies, charter, charter_path: path.trim() || null }).then(setPreview).catch((e: unknown) => setProblem(String(e))).finally(() => setBusy(false)); }}>Preview user setup</button>
    </fieldset>
    {problem && <p role="alert" className="error">{problem}</p>}
    {preview && <OperationPreview preview={preview} onUpdate={setPreview} busy={busy} setBusy={setBusy} />}
    <OperationHistory kind="user" refresh={preview?.state === "preview" ? 0 : preview?.id ?? 0} />
  </>;
}
function Registry({ onChanged, onRegistered }: { onChanged: () => void; onRegistered: (value: RegisteredProject) => void }) {
  const [projects, setProjects] = useState<Registration[]>([]);
  const [roots, setRoots] = useState("");
  const [found, setFound] = useState<Discovery | null>(null);
  const [preview, setPreview] = useState<Operation | null>(null);
  const [busy, setBusy] = useState(false);
  const [problem, setProblem] = useState<string | null>(null);
  const [generation, setGeneration] = useState(0);
  useEffect(() => { let current = true; void registrations().then((r) => { if (current) { setProjects(r.projects); if (generation === 0) setRoots(r.roots.join("\n")); } }).catch((e: unknown) => { if (current) setProblem(String(e)); }); return () => { current = false; }; }, [generation]);
  const paths = () => roots.split("\n").map((p) => p.trim()).filter(Boolean);
  const propose = async (selection: RegistrySelection) => { setBusy(true); setProblem(null); setPreview(null); try { setPreview(await previewRegistry(selection)); } catch (e: unknown) { setProblem(String(e)); } finally { setBusy(false); } };
  return <><h3>Discover and manage projects</h3><p className="faint">Discovery reads selected directories only: no symlink traversal, no agent or hooks, no automatic registration. Forget/prune archives registrations; all files and history remain recoverable with Restore.</p>
    <fieldset className="toolbox__choices" disabled={busy}><legend>Discovery roots</legend><label className="toolbox__path">One absolute directory per line<textarea value={roots} onChange={(e) => { setRoots(e.target.value); setPreview(null); setFound(null); }} /></label>
      <button className="button" disabled={!paths().length} onClick={() => { setBusy(true); setProblem(null); void discoverProjects(paths()).then(setFound).catch((e: unknown) => setProblem(String(e))).finally(() => setBusy(false)); }}>Scan selected roots</button>
      <button className="button" onClick={() => void propose({ operation: "scan_roots", roots: paths() })}>Preview remembered roots</button>
    </fieldset>
    {found && <div><h4>Discovered repositories</h4>{found.warnings.map((w, i) => <p className="notice" key={i}>{w}</p>)}{found.repositories.map((r) => <p key={r.path}>{r.path} {r.project !== null ? "Already registered (restore below if forgotten)" : <button className="button" disabled={busy} onClick={() => { setBusy(true); setProblem(null); void registerProject({ path: r.path }).then((result) => { onRegistered(result); onChanged(); setGeneration((n) => n + 1); setFound({ ...found, repositories: found.repositories.map((row) => row.path === r.path ? { ...row, project: result.project.id } : row) }); }).catch((e: unknown) => setProblem(String(e))).finally(() => setBusy(false)); }}>Add discovered repository</button>}</p>)}</div>}
    <button className="button" disabled={busy} onClick={() => void propose({ operation: "prune" })}>Preview pruning missing projects</button>
    {projects.map((p) => <div className="card" key={p.project}><p>{p.name} · {p.status}</p><p>{p.roots.map((r) => `${r.path} (${r.exists ? "present" : "missing"})`).join(", ")}</p>{p.reason && <p className="notice">{p.reason}</p>}<button className="button" disabled={busy || !!p.reason} onClick={() => void propose({ operation: p.status === "archived" ? "restore" : "forget", projects: [p.project] })}>{p.status === "archived" ? "Preview restoring" : "Preview forgetting"} {p.name}</button></div>)}
    {problem && <p role="alert" className="error">{problem}</p>}
    {preview && <OperationPreview preview={preview} onUpdate={setPreview} busy={busy} setBusy={setBusy} onApplied={() => { setGeneration((n) => n + 1); onChanged(); }} />}
    <OperationHistory kind="registry" refresh={generation} />
  </>;
}
function Checks({ label, options, selected, onChange }: { label: string; options: string[]; selected: string[]; onChange: (v: string[]) => void }) {
  return <div><strong>{label}</strong><div className="toolbox__options">{options.map((key) => <label key={key}><input type="checkbox" checked={selected.includes(key)} onChange={(e) => onChange(e.target.checked ? [...selected, key] : selected.filter((k) => k !== key))} />{key}</label>)}</div></div>;
}
