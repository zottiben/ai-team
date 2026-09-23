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
