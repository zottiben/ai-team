import { useState } from "react";
import type { CatalogueItem, ToolboxCatalogue } from "./toolbox-api";

export function CatalogueBrowser({ catalogue }: { catalogue: ToolboxCatalogue }) {
  const [query, setQuery] = useState("");
  const [copied, setCopied] = useState<string | null>(null);
  const [problem, setProblem] = useState<string | null>(null);
  const groups: [string, CatalogueItem[]][] = [
    ["Hooks", catalogue.hooks], ["MCP presets", catalogue.mcp], ["Skills", catalogue.skills],
    ["Rule snippets", catalogue.rules], ["Templates", catalogue.templates],
    ["Environment helpers", catalogue.helpers], ["Base charter", [catalogue.charter]],
  ];
  const copy = async (item: CatalogueItem) => {
    setProblem(null); setCopied(null);
    try { await navigator.clipboard.writeText(item.contents); setCopied(item.key); }
    catch { setProblem("Clipboard unavailable. Select and copy the displayed text instead."); }
  };
  return <details className="toolbox__catalogue">
    <summary>Browse bundled catalogue, rules and templates</summary>
    <p className="faint">Revision {catalogue.revision.slice(0, 12)}. These are bundled reference texts, not your installed configuration. Browsing or copying makes no project changes. Choose installation items below; rules and templates are copied for manual editing, never appended silently.</p>
    <label>Search catalogue <input type="search" value={query} onChange={(e) => setQuery(e.target.value)} /></label>
    {groups.map(([label, items]) => {
      const found = items.filter((item) => `${item.key} ${item.description ?? ""}`.toLowerCase().includes(query.toLowerCase()));
      return found.length > 0 && <section key={label} aria-label={`Catalogue ${label}`}><h4>{label} ({found.length})</h4>{found.map((item) => <details key={item.key}>
        <summary>{item.key}{item.description && ` — ${item.description}`}</summary>
        <button className="button" onClick={() => void copy(item)}>Copy {item.key}</button>
        <pre tabIndex={0}>{item.contents}</pre>
      </details>)}</section>;
    })}
    {copied && <p role="status">Copied {copied}. No files changed.</p>}
    {problem && <p role="alert" className="error">{problem}</p>}
    <details><summary>Licences and catalogue updates</summary><p>Catalogue content ships with AI Team. Updating AI Team updates this pinned bundle; this view does not update or discover a standalone ai-toolbox clone.</p><pre tabIndex={0}>{catalogue.notice}</pre></details>
  </details>;
}
