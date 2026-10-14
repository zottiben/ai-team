// Vite-only visual fixture: no app bootstrap, auth, API requests or operator state.
import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import "../styles.css";
import SynthPreview from "./SynthPreview";

const root = document.getElementById("root");
if (!root) throw new Error("synth-preview.html has no #root");
createRoot(root).render(<StrictMode><main><SynthPreview /></main></StrictMode>);
