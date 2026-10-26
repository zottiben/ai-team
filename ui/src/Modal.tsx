import { useLayoutEffect, useRef, type ReactNode, type RefObject } from "react";
import { createPortal } from "react-dom";

/** Native modal ownership makes the background inert; Tab stays in this dialog. */
export function Modal({ anchor, label, children, onClose, className = "" }: {
  anchor: RefObject<HTMLButtonElement | null>; label: string; children: ReactNode;
  onClose: () => void; className?: string;
}) {
  const dialog = useRef<HTMLDialogElement>(null);
  useLayoutEffect(() => {
    const element = dialog.current;
    element?.showModal();
    return () => { element?.close(); anchor.current?.focus({ preventScroll: true }); };
  }, [anchor]);
  return createPortal(<dialog ref={dialog} tabIndex={-1} role="dialog" aria-modal="true" aria-label={label} className={className}
    onCancel={event => { event.preventDefault(); onClose(); }}
    onKeyDown={event => {
      if (event.key === "Escape") event.stopPropagation();
      if (event.key !== "Tab" || event.defaultPrevented) return;
      const owner = event.currentTarget;
      const controls = [...owner.querySelectorAll<HTMLElement>('button, a[href], input, select, textarea, summary, [tabindex]')]
        .filter(element => element.tabIndex >= 0 && !element.matches(':disabled') && !element.closest('[inert]') && element.getClientRects().length > 0 && getComputedStyle(element).visibility !== 'hidden');
      const edge = event.shiftKey ? controls[0] : controls.at(-1);
      if (!controls.length || document.activeElement === owner || document.activeElement === edge) {
        event.preventDefault();
        (event.shiftKey ? controls.at(-1) ?? owner : controls[0] ?? owner).focus({ preventScroll: true });
      }
    }}
    onClick={event => {
      if (event.target !== event.currentTarget) return;
      const box = event.currentTarget.getBoundingClientRect();
      if (event.clientX < box.left || event.clientX > box.right || event.clientY < box.top || event.clientY > box.bottom) onClose();
    }}>{children}</dialog>, document.body);
}
