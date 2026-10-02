import type { SetupNode } from "./toolbox-api";

export function Contents({ node }: { node: SetupNode }) {
  switch (node.kind) {
    case "missing": return <p className="faint">Absent</p>;
    case "symlink": return <p className="mono">Link → {node.target}</p>;
    case "file": return <><p className="faint mono">Mode {node.mode.toString(8)} · {node.contents.length} bytes</p><pre>{new TextDecoder().decode(new Uint8Array(node.contents))}</pre></>;
    case "directory": return <><p className="faint mono">Directory · mode {node.mode.toString(8)}</p>{Object.entries(node.entries).map(([name, child]) => <details key={name}><summary>{name}</summary><Contents node={child} /></details>)}</>;
  }
}
