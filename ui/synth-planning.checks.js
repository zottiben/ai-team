const delay = ms => new Promise(resolve => setTimeout(resolve, ms));
const must = (value, message) => { if (!value) throw Error(message); return value; };
const visible = node => !!node && node.getClientRects().length > 0;
async function until(check, message) { for (let n = 0; n < 100; n++) { const value = check(); if (value) return value; await delay(50); } throw Error(message); }
const button = (root, text) => [...root.querySelectorAll("button")].find(node => visible(node) && node.textContent.trim() === text);
const field = (root, selector) => [...root.querySelectorAll(selector)].find(visible);
function fill(input, text) {
  const prototype = input instanceof HTMLTextAreaElement ? HTMLTextAreaElement.prototype : HTMLInputElement.prototype;
  Object.getOwnPropertyDescriptor(prototype, "value").set.call(input, text);
  input.dispatchEvent(new Event("input", { bubbles: true }));
}
export async function exercisePlanningRetention() {
  must(!document.hidden, "Native viewport is hidden; retained planning verification blocked");
  const tab = await until(() => [...document.querySelectorAll('[role="tab"]')].find(node => node.textContent.includes("Board")), "Chat Board tab missing");
  tab.click();
  const panel = await until(() => document.querySelector('.chat-planning .plan-kanban')?.closest('.chat-planning'), "Planning fixture did not settle");
  const board = panel.querySelector('.plan-kanban');
  must(getComputedStyle(board).display === "flex", "Chat Board did not load its own shared layout stylesheet");
  button(panel.querySelector('.plan-workspace-tabs'), "Plan").click();
  await delay(100);
  button(panel, "Edit Scope").click(); await delay(50);
  const scope = must(field(panel, 'textarea[name="body"]'), "Section editor missing"); fill(scope, "Retained original scope");
  button(panel, "Edit Design").click(); await delay(50);
  const design = must(field(panel, 'textarea[name="body"]'), "Second section editor missing"); fill(design, "Retained separate design");
  button(panel, "Edit Scope").click(); await delay(50);
  must(field(panel, 'textarea[name="body"]') === scope && scope.value === "Retained original scope", "Switching sections replaced the first editor");
  const questions = panel.querySelector('.plan-questions');
  const answer = must(field(questions, '.plan-item textarea'), "Answer editor missing"); fill(answer, "Unsent original answer");
  button(questions, "Ask a question").click(); await delay(50);
  const ask = must(field(questions, ':scope > form textarea'), "Question composer missing"); fill(ask, "Retain this separate question");
  button(questions, "Cancel question").click(); await delay(50);
  must(ask.isConnected && !visible(ask), "Cancel question left the retained form visible or replaced it");
  button(questions, "Ask a question").click(); await delay(50);
  must(field(questions, ':scope > form textarea') === ask && ask.value === "Retain this separate question", "Question composer was reset");
  const tabs = panel.querySelector('.plan-workspace-tabs');
  button(tabs, "Board").click(); await delay(50); button(tabs, "Plan").click(); await delay(50);
  const entry = must([...document.scripts].find(script => new URL(script.src || location.href).pathname.endsWith('/src/synth/shell-preview.tsx')), "Shell fixture entry missing");
  const fixture = await import(entry.src); fixture.advanceBoard();
  await until(() => button(panel, "Edit Refreshed scope"), "Synthetic plan refresh did not arrive");
  must(scope.isConnected && scope.value === "Retained original scope" && design.isConnected && design.value === "Retained separate design", "Refresh discarded a section editor");
  must(field(panel, 'input[name="title"]').value === "Scope", "Retained fields borrowed a refreshed section title");
  must(answer.isConnected && answer.value === "Unsent original answer", "A peer answer discarded the unsent local answer");
  must(button(answer.closest("form"), "Answer question").disabled && button(ask.closest("form"), "Save question").disabled, "Stale drafts borrowed the new plan revision");
  const preferences = await import('/src/ui-preferences.ts');
  preferences.setSynth(false); await delay(100); preferences.setSynth(true); await delay(100);
  must(scope.isConnected && design.isConnected && answer.isConnected && ask.isConnected, "Changing skins replaced planning input owners");
  must(document.documentElement.scrollWidth <= document.documentElement.clientWidth + 1, "Planning overflow escaped the viewport");
  must(fixture.boardRequests.length === 0, "Retention probe attempted a mutation");
  const apiRequests = performance.getEntriesByType("resource").filter(entry => new URL(entry.name).pathname.startsWith('/api/')).length;
  must(apiRequests === 0, "Planning fixture reached a real API");
  return { width: document.documentElement.clientWidth, theme: document.documentElement.dataset.theme, retainedInputs: 4, staleAnswersDisabled: true, mutationAttempts: 0, apiRequests };
}
