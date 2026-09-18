// Which theme the window is wearing.
//
// The token layer does the repainting; this only decides which set of values is live.
// Kept out of React state so the very first paint is already correct - a window that
// flashes light before settling on dark is a window that looks broken once a day.

export type Theme = "dark" | "light" | "system";

const STORAGE_KEY = "ai-team.theme";

/** What `system` currently resolves to. */
export function preferred(): "dark" | "light" {
  return window.matchMedia("(prefers-color-scheme: light)").matches ? "light" : "dark";
}

export function stored(): Theme {
  const value = localStorage.getItem(STORAGE_KEY);
  return value === "dark" || value === "light" || value === "system" ? value : "system";
}

/** Put it on the document, where the token layer can see it. */
export function apply(theme: Theme): void {
  const resolved = theme === "system" ? preferred() : theme;
  document.documentElement.dataset.theme = resolved;
  localStorage.setItem(STORAGE_KEY, theme);
}

/**
 * Follow the OS while the choice is `system`.
 *
 * Returns the unsubscribe, because a listener that outlives the window it was made for
 * is the kind of leak that only shows up after an hour of use.
 */
export function followSystem(current: () => Theme): () => void {
  const query = window.matchMedia("(prefers-color-scheme: light)");
  const onChange = () => {
    if (current() === "system") apply("system");
  };
  query.addEventListener("change", onChange);
  return () => query.removeEventListener("change", onChange);
}
