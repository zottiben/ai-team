import { useEffect, useState } from "react";

import { approveSetup, previewSetup, scanToolbox, savedSetup, setupHistory, type SetupRecord, type SetupNode, type SetupPreview, type SetupSelection, type ToolboxScan } from "./toolbox-api";
import "./toolbox.css";

export function Toolbox({ project, initial }: { project: number; initial?: ToolboxScan }) {
  const [scans, setScans] = useState<ToolboxScan[]>(initial ? [initial] : []);
  const [selected, setSelected] = useState(initial?.root ?? "");
  const [problem, setProblem] = useState<string | null>(null);
  const [generation, setGeneration] = useState(0);
  const [busy, setBusy] = useState(false);
  const [history, setHistory] = useState<SetupRecord[]>([]);
  useEffect(() => {
    if (busy) return;
    let current = true;
    void setupHistory(project).then((records) => { if (current) setHistory(records); }).catch((e: unknown) => { if (current) setProblem(String(e)); });
    return () => { current = false; };
  }, [project, busy, generation]);
  useEffect(() => {
    if (initial && generation === 0) return;
    let current = true;
    void scanToolbox(project).then((found) => {
      if (!current) return;
      setScans(found);
      setSelected((root) => found.some((s) => s.root === root) ? root : found[0]?.root ?? "");
      setProblem(null);
    }).catch((e: unknown) => { if (current) setProblem(String(e)); });
    return () => { current = false; };
  }, [project, initial, generation]);
  const scan = scans.find((s) => s.root === selected);
  return <section className="toolbox" aria-label="Project setup">
    <div className="card__row"><h3>Project setup</h3><button className="button" disabled={busy} onClick={() => { setScans([]); setGeneration((n) => n + 1); }}>Scan again</button></div>
    <p className="faint">Scanning changes nothing. Review exact files before approving setup. No software, credentials or global configuration are installed.</p>
    {problem && <p role="alert" className="error">{problem}</p>}
    {scans.length > 1 && <label>Checkout <select aria-label="Setup checkout" disabled={busy} value={selected} onChange={(e) => setSelected(e.target.value)}>{scans.map((s) => <option key={s.root}>{s.root}</option>)}</select></label>}
    {!scan && <p className="faint">No setup scan available. Register or attach a local directory, then scan again.</p>}
    {scan && <Setup key={`${scan.root}:${generation}`} project={project} scan={scan} busy={busy} setBusy={setBusy} />}
    {history.length > 0 && <details><summary>Saved setup outcomes</summary>{history.map((record) => <div key={record.id}>
      <p>Setup #{record.id} · {record.state} · {record.root}</p>
      {record.state === "applying" && <p className="error">Applying or interrupted: some files may already have changed. Further setup is blocked here; inspect the files. This is not certified success and will not be retried automatically.</p>}
      {record.outcome && <p>{record.outcome.applied.length} recorded changes. {record.outcome.problem}</p>}
    </div>)}</details>}
  </section>;
}

function Setup({ project, scan, busy, setBusy }: { project: number; scan: ToolboxScan; busy: boolean; setBusy: (busy: boolean) => void }) {
  const rec = scan.survey?.recommendation;
  const [harnesses, setHarnesses] = useState(scan.survey?.harnesses.length ? scan.survey.harnesses : ["pi"]);
  const [hooks, setHooks] = useState(rec?.hooks ?? []);
  const [mcp, setMcp] = useState(rec?.mcp ?? []);
  const [skills, setSkills] = useState(rec?.skills ?? []);
  const [scaffold, setScaffold] = useState(true);
  const [preview, setPreview] = useState<SetupPreview | null>(null);
  const [problem, setProblem] = useState<string | null>(null);
  const [approved, setApproved] = useState(false);

  const run = async (selection: SetupSelection) => {
    setBusy(true); setProblem(null); setPreview(null); setApproved(false);
    try { setPreview(await previewSetup(project, scan.root, selection)); }
    catch (e: unknown) { setProblem(String(e)); }
    finally { setBusy(false); }
  };
  const apply = async () => {
    if (!preview || !approved) return;
    setBusy(true); setProblem(null); setApproved(false);
    try { setPreview(await approveSetup(project, preview.id)); }
    catch (e: unknown) {
      // The request might have reached the server. Never offer a blind retry.
      setPreview({ ...preview, state: "applying" }); setProblem(`${String(e)}. The request may have started. Inspect the files and saved setup status before further changes.`);
    } finally { setBusy(false); }
  };
  return <>
    <p className="mono">{scan.root}</p>
    <p className="faint">Bundled catalogue {scan.catalogue_revision.slice(0, 12)}</p>
    {scan.problem && <p role="alert" className="error">{scan.problem}</p>}
    {scan.worktree_problem && <p className="notice">Worktree inspection: {scan.worktree_problem}</p>}
    {scan.worktrees.length > 0 && <details><summary>Worktree setup consistency</summary><p className="faint">Compared with {scan.root}. Inspection only; no worktree is changed.</p>{scan.worktrees.map((tree) => <p key={tree.path}>{tree.path} · {tree.branch ?? "detached"} · {tree.problem ?? (tree.different.length ? `Differs: ${tree.different.join(", ")}` : "In step")}</p>)}</details>}
    {scan.survey && <>
      <p>Detected: {rec?.detected.join(", ") || "no recognised stack"}. Existing setup: {scan.survey.state}.</p>
      {scan.survey.items.length > 0 && <details><summary>Installed configuration ({scan.survey.items.length})</summary><ul>{scan.survey.items.map((item, i) => <li key={i}>{item.kind}: {item.name} · {item.origin.state}</li>)}</ul></details>}
      {scan.survey.findings.map((finding, i) => <p className="notice" key={i}>{finding.what} {finding.advice}</p>)}
      {rec?.notes.map((note, i) => <p className="faint" key={i}>{note}</p>)}
      <fieldset disabled={busy} className="toolbox__choices" onChange={() => { setPreview(null); setApproved(false); }}>
        <legend>Choose recommended setup</legend>
        <Choices label="Harnesses" options={["pi", "claude", "codex"]} selected={harnesses} onChange={setHarnesses} />
        <Choices label="Hooks" options={rec?.hooks ?? []} selected={hooks} onChange={setHooks} />
        <Choices label="MCP presets" options={rec?.mcp ?? []} selected={mcp} onChange={setMcp} />
        <Choices label="Skills" options={rec?.skills ?? []} selected={skills} onChange={setSkills} />
        <label><input type="checkbox" checked={scaffold} onChange={(e) => setScaffold(e.target.checked)} /> Scaffold missing knowledge files (never replace existing knowledge)</label>
      </fieldset>
      <div className="card__row">
        <button className="button" disabled={busy || !harnesses.length} onClick={() => void run({ operation: "install", harnesses, hooks, mcp, skills, scaffold })}>Preview selected setup</button>
        <button className="button" disabled={busy || !scan.survey.findings.some((f) => f.repairable)} onClick={() => void run({ operation: "repair" })}>Preview repairs</button>
      </div>
    </>}
    {busy && <p role="status">Working…</p>}
    {problem && <p className="error" role="alert">{problem}</p>}
    {preview && <section aria-label="Exact setup preview">
      <h4>{preview.effects.length} exact file/directory changes · {preview.state}</h4>
      <p className="faint">Before and after include removals, links and permissions. Content may contain your existing configuration secrets. Approval applies only this saved preview, never a recalculated recommendation.</p>
      {preview.warnings.map((warning, i) => <p className="notice" key={i}>{warning}</p>)}
      {preview.effects.map((effect) => <details key={effect.path}>
        <summary>{effect.path} — {effect.summary}</summary>
        <div className="toolbox__diff"><div><h5>Before</h5><Contents node={effect.before} /></div><div><h5>After</h5><Contents node={effect.after} /></div></div>
      </details>)}
      {preview.effects.length === 0 && <p>Already in place. No writes needed.</p>}
      {preview.state === "preview" && preview.effects.length > 0 && <>
        <label><input type="checkbox" checked={approved} disabled={busy} onChange={(e) => setApproved(e.target.checked)} /> I reviewed these exact changes to {preview.root}</label>
        <button className="button button--primary" disabled={busy || !approved} onClick={() => void apply()}>Approve and apply</button>
      </>}
      {preview.state === "applying" && <button className="button" disabled={busy} onClick={() => {
        setBusy(true);
        void savedSetup(project, preview.id).then(setPreview).catch((e: unknown) => setProblem(String(e))).finally(() => setBusy(false));
      }}>Inspect saved setup status</button>}
      {preview.outcome && <div role="status">
        <p>{preview.outcome.applied.length} changes applied.</p>
        {preview.outcome.problem && <p className="error">{preview.outcome.problem}</p>}
        <p>{preview.state === "applied" ? "Setup applied. Scan again to check the result and any manual follow-up." : "No automatic retry. Preserve the files, inspect the outcome, then make a fresh preview."}</p>
      </div>}
    </section>}
  </>;
}

function Choices({ label, options, selected, onChange }: { label: string; options: string[]; selected: string[]; onChange: (value: string[]) => void }) {
  return <div><strong>{label}</strong><div className="toolbox__options">{options.map((item) => <label key={item}><input type="checkbox" checked={selected.includes(item)} onChange={(e) => onChange(e.target.checked ? [...selected, item] : selected.filter((s) => s !== item))} />{item}</label>)}</div></div>;
}

function Contents({ node }: { node: SetupNode }) {
  switch (node.kind) {
    case "missing": return <p className="faint">Absent</p>;
    case "symlink": return <p className="mono">Link → {node.target}</p>;
    case "file": return <><p className="faint mono">Mode {node.mode.toString(8)} · {node.contents.length} bytes</p><pre>{new TextDecoder().decode(new Uint8Array(node.contents))}</pre></>;
    case "directory": return <><p className="faint mono">Directory · mode {node.mode.toString(8)}</p>{Object.entries(node.entries).map(([name, child]) => <details key={name}><summary>{name}</summary><Contents node={child} /></details>)}</>;
  }
}
