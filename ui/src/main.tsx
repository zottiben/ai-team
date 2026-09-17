import { StrictMode } from "react";
import { createRoot } from "react-dom/client";

import App from "./App";
import { captureToken } from "./api";
import "./styles.css";

// Before the first render, and before anything can fetch: the token has to be out of
// the address bar and into this tab's storage.
captureToken(window.location, window.history);

const root = document.getElementById("root");
if (!root) throw new Error("index.html has no #root");

createRoot(root).render(
  <StrictMode>
    <App />
  </StrictMode>,
);
