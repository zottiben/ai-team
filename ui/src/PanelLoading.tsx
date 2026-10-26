/** A truthful loading label; the Synth wells are inert decoration, not progress. */
export function PanelLoading({ children }: { children: string }) {
  return <div className="chat-panel-loading faint" role="status">
    <p>{children}</p>
    <div className="chat-panel-loading__wells" aria-hidden="true"><i/><i/><i/></div>
  </div>;
}
