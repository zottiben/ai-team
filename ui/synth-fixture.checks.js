/** Import the already-running entry, including Vite's timestamp; never bootstrap twice. */
export async function shellFixture() {
  const entries = [...document.scripts].filter(script => new URL(script.src || location.href).pathname === '/src/synth/shell-preview.tsx');
  if (entries.length !== 1 || document.querySelectorAll('#root > .chat-shell').length !== 1) throw new Error('Expected one isolated shell fixture owner');
  return import(entries[0].src);
}
