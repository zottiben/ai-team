import { render, screen } from "@testing-library/react";
import { expect, it } from "vitest";

import { BoardMarkdown } from "./BoardMarkdown";

it("renders the markdown agents put in plan scopes and progress", () => {
  render(
    <BoardMarkdown
      source={"## Result\n\n**Passed** with `cargo test`.\n\n- first\n- second"}
    />,
  );

  expect(screen.getByRole("heading", { name: "Result" })).toBeDefined();
  expect(screen.getByText("Passed").tagName).toBe("STRONG");
  expect(screen.getByText("cargo test").tagName).toBe("CODE");
  expect(screen.getAllByRole("listitem")).toHaveLength(2);
});

it("never turns an executable markdown URL into a link", () => {
  render(
    <BoardMarkdown
      source={"[do not run](javascript:alert(1)) [docs](https://example.com/docs)"}
    />,
  );

  expect(screen.getByText("do not run").closest("a")).toBeNull();
  expect(screen.getByRole("link", { name: "docs" }).getAttribute("href")).toBe(
    "https://example.com/docs",
  );
});

it("keeps a single newline as a line break when asked to, the way chat is written", () => {
  // A plan is hard-wrapped prose, and its newlines fold into the paragraph. An agent's
  // message puts `**PR2**` on one line and `Owner: frontend.` on the next, and means it.
  const source = "**PR2**\nOwner: frontend.";

  const { container, rerender } = render(<BoardMarkdown source={source} />);
  expect(container.querySelector("p")?.innerHTML).toBe("<strong>PR2</strong> Owner: frontend.");

  rerender(<BoardMarkdown source={source} breaks />);
  expect(container.querySelector("p")?.innerHTML).toBe(
    "<strong>PR2</strong><br>Owner: frontend.",
  );
});

it("reads the formatting inside a bold title, and carries on after it", () => {
  // A slice title is commonly `**PR1 - Add `--shout` support**`: code inside bold, shown
  // with its backticks when only the outer span was read.
  render(<BoardMarkdown source={"**PR1 - Add `--shout` support** then *soon* `done`"} />);

  const option = screen.getByText("--shout");
  expect(option.tagName).toBe("CODE");
  expect(option.closest("strong")).not.toBeNull();
  expect(screen.getByText("soon").tagName).toBe("EM");
  expect(screen.getByText("done").tagName).toBe("CODE");
});
