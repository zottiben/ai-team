import { createRef } from "react";
import { fireEvent, render } from "@testing-library/react";
import { afterEach, expect, it, vi } from "vitest";
import Shortcuts, { shortcut } from "./Shortcuts";

afterEach(() => vi.restoreAllMocks());
const key = (key: string, extra: KeyboardEventInit = {}) => new KeyboardEvent("keydown", { key, metaKey: true, ...extra });
it("maps only explicit, non-repeating, non-composing chords", () => {
  expect(shortcut(key("1"))).toBe("overview"); expect(shortcut(key("4"))).toBe("work");
  expect(shortcut(key("e"))).toBe("editor"); expect(shortcut(key("`"))).toBe("terminal");
  expect(shortcut(key("Enter", { altKey: true }))).toBe("steer");
  expect(shortcut(key("Tab", { metaKey: false, ctrlKey: true, shiftKey: true }))).toBe("previous");
  for (const extra of [{ repeat: true }, { isComposing: true }, { altKey: true }, { shiftKey: true }, { metaKey: false }]) expect(shortcut(key("1", extra))).toBeNull();
  expect(shortcut(key("s"))).toBeNull(); expect(shortcut(key("p"))).toBeNull();
});
it("preserves existing tools, modal ownership and a separate explicit steering confirmation", () => {
  const rect = new DOMRect(0, 0, 400, 600);
  vi.spyOn(HTMLElement.prototype, "getClientRects").mockReturnValue(Object.assign([rect], { item: (index: number) => index === 0 ? rect : null }));
  const host = createRef<HTMLDivElement>(), panel = vi.fn(), steer = vi.fn(), tool = vi.fn(), submit = vi.fn();
  const view = render(<div ref={host}><div className="chat-header-actions"><button data-chat-tool="editor" onClick={tool}>Editor</button></div><textarea aria-label="Message" onKeyDown={submit}/><div role="dialog"><input aria-label="Modal"/></div><Shortcuts host={host} panel="work" onPanel={panel} onSteer={steer}/></div>);
  const message = view.getByRole("textbox", { name: "Message" });
  fireEvent.keyDown(message, { key: "1", metaKey: true }); expect(panel).toHaveBeenLastCalledWith("overview");
  fireEvent.keyDown(message, { key: "Tab", ctrlKey: true }); expect(panel).toHaveBeenLastCalledWith("overview");
  fireEvent.keyDown(message, { key: "e", ctrlKey: true }); expect(tool).toHaveBeenCalledOnce();
  fireEvent.keyDown(message, { key: "Enter", metaKey: true, altKey: true }); expect(steer).toHaveBeenCalledOnce(); expect(submit).not.toHaveBeenCalled();
  fireEvent.keyDown(view.getByLabelText("Modal"), { key: "1", metaKey: true }); expect(panel).toHaveBeenCalledTimes(2);
  view.unmount(); fireEvent.keyDown(document, { key: "1", metaKey: true }); expect(panel).toHaveBeenCalledTimes(2);
});
