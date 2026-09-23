import { useEffect, useMemo, useState, type ReactNode } from "react";

export type PagePanel = {
  id: string;
  label: string;
  span: 1 | 2 | 3;
  content: ReactNode;
};

type SavedPanel = { id: string; visible: boolean; span: 1 | 2 | 3 };
type SavedLayout = { density: "compact" | "comfortable"; panels: SavedPanel[] };

const PREFIX = "ai-team.page-layout.v1";

function defaults(panels: PagePanel[]): SavedLayout {
  return {
    density: "compact",
    panels: panels.map((panel) => ({ id: panel.id, visible: true, span: panel.span })),
  };
}

function read(view: string, panels: PagePanel[]): SavedLayout {
  const fallback = defaults(panels);
  try {
    const parsed = JSON.parse(localStorage.getItem(`${PREFIX}.${view}`) ?? "null") as Partial<SavedLayout> | null;
    if (parsed === null || !Array.isArray(parsed.panels)) return fallback;
    const known = new Map(panels.map((panel) => [panel.id, panel]));
    const saved: SavedPanel[] = [];
    for (const candidate of parsed.panels) {
      if (
        typeof candidate?.id !== "string" ||
        !known.has(candidate.id) ||
        typeof candidate.visible !== "boolean" ||
        ![1, 2, 3].includes(candidate.span)
      ) {
        continue;
      }
      saved.push({ id: candidate.id, visible: candidate.visible, span: candidate.span });
      known.delete(candidate.id);
    }
    for (const panel of panels) {
      if (known.has(panel.id)) saved.push({ id: panel.id, visible: true, span: panel.span });
    }
    return {
      density: parsed.density === "comfortable" ? "comfortable" : "compact",
      panels: saved,
    };
  } catch {
    return fallback;
  }
}

/**
 * A local, presentational dashboard layout.
 *
 * Runs and repositories never depend on it: order, width, visibility and density stay in
 * this browser. The explicit controls are also keyboard-usable, unlike drag-only layouts.
 */
export function PageLayout({ view, panels }: { view: string; panels: PagePanel[] }) {
  const signature = panels.map((panel) => `${panel.id}:${panel.span}`).join("|");
  const [layout, setLayout] = useState<SavedLayout>(() => read(view, panels));

  useEffect(() => {
    setLayout(read(view, panels));
    // `signature` deliberately resets only when the available panel set changes.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [view, signature]);

  useEffect(() => {
    localStorage.setItem(`${PREFIX}.${view}`, JSON.stringify(layout));
  }, [layout, view]);

  const byId = useMemo(() => new Map(panels.map((panel) => [panel.id, panel])), [panels]);
  const update = (id: string, change: Partial<SavedPanel>) => {
    setLayout((current) => ({
      ...current,
      panels: current.panels.map((panel) => (panel.id === id ? { ...panel, ...change } : panel)),
    }));
  };
  const move = (id: string, direction: -1 | 1) => {
    setLayout((current) => {
      const at = current.panels.findIndex((panel) => panel.id === id);
      const to = at + direction;
      if (at < 0 || to < 0 || to >= current.panels.length) return current;
      const next = [...current.panels];
      [next[at], next[to]] = [next[to]!, next[at]!];
      return { ...current, panels: next };
    });
  };

  return (
    <div className="page-layout" data-density={layout.density}>
      <div className="page-layout__toolbar">
        <details>
          <summary className="button">Layout</summary>
          <div className="page-layout__menu">
            <label className="settings__toggle">
              <input
                type="checkbox"
                checked={layout.density === "compact"}
                onChange={(event) =>
                  setLayout((current) => ({
                    ...current,
                    density: event.target.checked ? "compact" : "comfortable",
                  }))
                }
              />
              Compact panels
            </label>
            <div className="page-layout__rows">
              {layout.panels.map((saved, index) => {
                const panel = byId.get(saved.id);
                if (panel === undefined) return null;
                return (
                  <div key={saved.id} className="page-layout__row">
                    <label>
                      <input
                        type="checkbox"
                        checked={saved.visible}
                        onChange={(event) => update(saved.id, { visible: event.target.checked })}
                      />
                      {panel.label}
                    </label>
                    <select
                      aria-label={`width for ${panel.label}`}
                      value={saved.span}
                      onChange={(event) =>
                        update(saved.id, { span: Number(event.target.value) as 1 | 2 | 3 })
                      }
                    >
                      <option value={1}>1 column</option>
                      <option value={2}>2 columns</option>
                      <option value={3}>full width</option>
                    </select>
                    <button
                      type="button"
                      className="button"
                      aria-label={`Move ${panel.label} earlier`}
                      disabled={index === 0}
                      onClick={() => move(saved.id, -1)}
                    >
                      ↑
                    </button>
                    <button
                      type="button"
                      className="button"
                      aria-label={`Move ${panel.label} later`}
                      disabled={index === layout.panels.length - 1}
                      onClick={() => move(saved.id, 1)}
                    >
                      ↓
                    </button>
                  </div>
                );
              })}
            </div>
            <button type="button" className="button" onClick={() => setLayout(defaults(panels))}>
              Reset layout
            </button>
          </div>
        </details>
      </div>
      <div className="page-layout__grid">
        {layout.panels.map((saved) => {
          const panel = byId.get(saved.id);
          if (panel === undefined || !saved.visible) return null;
          return (
            <div key={saved.id} className="page-layout__panel" data-span={saved.span}>
              {panel.content}
            </div>
          );
        })}
      </div>
    </div>
  );
}
