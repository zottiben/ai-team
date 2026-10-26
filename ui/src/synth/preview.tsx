// Vite-only visual fixture: no app bootstrap, auth, API requests or operator state.
import { StrictMode, useLayoutEffect, useState } from "react";
import { createRoot } from "react-dom/client";
import "../styles.css";
import SynthPreview from "./SynthPreview";
import { HwButton } from "./primitives/HwButton";

function Preview() {
  const [theme, setTheme] = useState(new URLSearchParams(location.search).get("theme") === "light" ? "light" : "dark");
  useLayoutEffect(() => { document.documentElement.dataset.theme = theme; }, [theme]);
  return <main>
    <div className="synth-fixture-themes" role="group" aria-label="Fixture theme">
      <span>Isolated preview · no saved preferences</span>
      {(["dark", "light"] as const).map(value => <HwButton key={value} aria-pressed={theme === value} onClick={() => setTheme(value)}>{value === "dark" ? "Dark" : "Light"}</HwButton>)}
    </div>
    <SynthPreview />
  </main>;
}
const root = document.getElementById("root");
if (!root) throw new Error("synth-preview.html has no #root");
createRoot(root).render(<StrictMode><Preview /></StrictMode>);
