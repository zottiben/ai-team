import { useState } from "react";

/**
 * What a selection can do: be copied, or be quoted into the draft.
 *
 * Send adds a chip to the composer and nothing else (§12.2). It does not send the
 * message, and the text it carries is quoted output - a line in a build log that reads
 * like an instruction stays a line in a build log.
 */
export function SelectionToolbar({
  text,
  top,
  left,
  onSend,
}: {
  text: string;
  top: number;
  left: number;
  onSend?: (text: string) => void;
}) {
  const [copied, setCopied] = useState(false);
  const lines = text.split("\n").length;
  return (
    <div className="synth-selection" style={{ top, left }} role="group" aria-label="Selected output">
      <span className="synth-selection__count">
        {lines} line{lines === 1 ? "" : "s"}
      </span>
      <button
        type="button"
        className="synth-key"
        onClick={() => {
          void navigator.clipboard
            ?.writeText(text)
            .then(() => setCopied(true))
            .catch(() => setCopied(false));
        }}
      >
        Copy
      </button>
      {onSend && (
        <button
          type="button"
          className="synth-key synth-key--comment"
          title="Attaches the selection to your draft as a quoted chip. It is not sent, and it authorises nothing."
          onClick={() => onSend(text)}
        >
          Send to agent
        </button>
      )}
      <span role="status" className="synth-muted">
        {copied ? "Copied" : ""}
      </span>
    </div>
  );
}
