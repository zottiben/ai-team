/** Run only in the isolated shell fixture after each viewport / tab transition. */
export async function checkShell() {
  if (location.pathname !== '/synth-shell-preview.html') throw new Error('Use the network-disabled shell fixture');
  await document.fonts.ready;
  await new Promise(resolve => setTimeout(resolve, 100));
  const root = document.documentElement, body = document.querySelector('.chat-body');
  if (root.scrollWidth > root.clientWidth) throw new Error(`Page overflow: ${root.scrollWidth} > ${root.clientWidth}`);
  const api = performance.getEntriesByType('resource').filter(entry => new URL(entry.name).pathname.startsWith('/api/'));
  if (api.length) throw new Error('Fixture escaped its API stub');
  const project = document.querySelector('.chat-project-name'), heading = document.querySelector('.chat-heading > .faint');
  if (root.dataset.synth === 'true' && project && heading && getComputedStyle(project, '::before').backgroundColor !== getComputedStyle(heading, '::before').backgroundColor)
    throw new Error('Project marker differs between sidebar and chat header');
  const sidebar = document.querySelector('.chat-sidebar'), main = document.querySelector('.chat-main');
  if (sidebar && main && sidebar.getBoundingClientRect().width > 0) {
    const a = sidebar.getBoundingClientRect(), b = main.getBoundingClientRect();
    if (Math.min(a.right, b.right) - Math.max(a.left, b.left) > 1 && Math.min(a.bottom, b.bottom) - Math.max(a.top, b.top) > 1)
      throw new Error('Expanded sidebar obscures the workspace');
  }
  if (root.dataset.synth === 'true' && body?.dataset.context === 'true' && body.dataset.synthStacked === 'false') {
    const panel = document.querySelector('#chat-context');
    if (Math.abs(panel.getBoundingClientRect().height - body.getBoundingClientRect().height) > 1) throw new Error('Side-by-side panel lost workspace height');
  }
  if (body?.dataset.synthBoard === 'true') {
    if (!document.querySelector('.chat-conversation')?.hidden) throw new Error('Board left the conversation visible');
    const panel = document.querySelector('#chat-context');
    if (Math.abs(panel.getBoundingClientRect().width - body.getBoundingClientRect().width) > 1) throw new Error('Board is not full width');
  }
  return { width: root.clientWidth, content: root.scrollWidth, apiRequests: api.length, stacked: body?.dataset.synthStacked ?? null };
}
