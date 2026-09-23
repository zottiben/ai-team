import { useCallback, useEffect, useState } from "react";

import { browse, type Listing } from "./api";

/**
 * Finding a repository by looking, rather than typing its path from memory.
 *
 * Served rather than native (D24). A Tauri file dialog would be one line in the desktop
 * shell and nothing at all in `ait ui`, and the page is loaded over loopback HTTP
 * precisely so that the two surfaces are one frontend with one set of tests - so the
 * picker is a route both of them already have.
 *
 * It is a disclosure rather than a modal. The text field stays, because somebody who
 * knows the path should not have to click through four directories to say so, and the
 * two are wired together: browsing fills the field, so what gets submitted is always the
 * thing that is written down.
 */
export function Browse({ onPick }: { onPick: (path: string) => void }) {
  const [listing, setListing] = useState<Listing | null>(null);
  const [problem, setProblem] = useState<string | null>(null);

  const look = useCallback(async (path: string) => {
    try {
      setListing(await browse(path));
      setProblem(null);
    } catch (error: unknown) {
      // The listing is kept on failure, so a directory that cannot be read leaves you
      // where you were rather than throwing you back to home with nothing to click.
      setProblem(error instanceof Error ? error.message : String(error));
    }
  }, []);

  useEffect(() => {
    void look("");
  }, [look]);

  if (listing === null) {
    return <p className="empty">{problem ?? "Looking…"}</p>;
  }

  return (
    <div className="browse">
      <div className="browse__where">
        <button
          type="button"
          className="button"
          aria-label="up one directory"
          disabled={listing.parent === null}
          onClick={() => void look(listing.parent ?? "")}
        >
          ↑
        </button>
        <span className="mono faint browse__path">{listing.path}</span>
        <button type="button" className="button" onClick={() => onPick(listing.path)}>
          Use this one
        </button>
      </div>

      {problem !== null && <p className="error">{problem}</p>}

      {listing.entries.length === 0 && <p className="empty">Nothing in here.</p>}

      <div className="browse__list">
        {listing.entries.map((entry) => (
          <div key={entry.path} className="browse__entry">
            {/* Going in and choosing are different things, and a repository is usually
                both - so it gets two controls rather than a single click that has to
                guess which one was meant. */}
            <button type="button" className="browse__name" onClick={() => void look(entry.path)}>
              <span>{entry.name}</span>
              {entry.repo && (
                <span className="status" data-status="done">
                  repo
                </span>
              )}
            </button>
            <button
              type="button"
              className="button"
              aria-label={`choose ${entry.name}`}
              onClick={() => onPick(entry.path)}
            >
              Choose
            </button>
          </div>
        ))}
      </div>
    </div>
  );
}
