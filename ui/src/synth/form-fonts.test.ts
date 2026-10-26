import { readFileSync } from "node:fs";
import { join } from "node:path";
import { expect, it } from "vitest";

// jsdom does not resolve :where() cascade specificity faithfully. The browser's
// synth-visual.checks.js checks the resulting font; this checks legacy isolation.
it("keeps the legacy form-inheritance rule off Synth and restores it on toggle", () => {
  const sheet = document.createElement("style");
  sheet.textContent = readFileSync(join(import.meta.dirname, "../chat.css"), "utf8");
  const host = document.createElement("div"); host.className = "chat-shell";
  const button = document.createElement("button"); host.append(button);
  const original = document.documentElement.dataset.synth;
  document.head.append(sheet); document.body.append(host);
  try {
    const rules = [...sheet.sheet!.cssRules].filter((rule): rule is CSSStyleRule => rule instanceof CSSStyleRule);
    const legacy = rules.find(rule => rule.selectorText.includes(".chat-shell") && rule.style.font === "inherit"
      && !rule.selectorText.startsWith(':where(:root[data-synth="true"]'));
    expect(legacy).toBeDefined();
    document.documentElement.dataset.synth = "false";
    expect(button.matches(legacy!.selectorText)).toBe(true);
    document.documentElement.dataset.synth = "true";
    expect(button.matches(legacy!.selectorText)).toBe(false);
    document.documentElement.dataset.synth = "false";
    expect(button.matches(legacy!.selectorText)).toBe(true);
  } finally {
    if (original === undefined) delete document.documentElement.dataset.synth; else document.documentElement.dataset.synth = original;
    sheet.remove(); host.remove();
  }
});
