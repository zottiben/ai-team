/**
 * The chevron every `chat-select` wears.
 *
 * Its own module because the lazily-loaded thinking picker needs it too, and reaching
 * back into `Chat` for it would pull the whole view into that chunk.
 */
export function SelectChevron() {
  return <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true"><path d="m7 10 5 5 5-5" /></svg>;
}
