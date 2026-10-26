/** Browser-only regression checks. Run on /synth-preview.html at desktop and touch widths:
 * await (await import('/synth-preview.checks.js')).checkPreview()
 * This deliberately checks real layout, not jsdom's zero-sized boxes.
 */
export async function checkPreview() {
  if (location.pathname !== '/synth-preview.html') throw new Error('Use the isolated Synth fixture, not the live application');
  await document.fonts.ready;
  await new Promise(resolve => setTimeout(resolve, 100));
  const viewport = document.documentElement.clientWidth;
  const content = document.documentElement.scrollWidth;
  // Mobile innerWidth can expand to the overflowing content and hide this bug.
  if (content > viewport) throw new Error(`Horizontal page overflow: ${content}px content in ${viewport}px viewport`);
  const apiCalls = performance.getEntriesByType('resource').filter(entry => new URL(entry.name).pathname.startsWith('/api/')).length;
  if (apiCalls) throw new Error(`Isolated preview made ${apiCalls} API calls`);
  return { viewport, content, apiCalls };
}
