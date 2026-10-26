import { render, screen } from "@testing-library/react";
import { expect, it } from "vitest";
import { PanelLoading } from "./PanelLoading";

it("keeps one truthful loading announcement and no fabricated progress", () => {
  const view = render(<PanelLoading>Loading overview…</PanelLoading>);
  expect(screen.getByRole("status").textContent).toBe("Loading overview…");
  expect(screen.queryByRole("progressbar")).toBeNull();
  expect(view.container.querySelector('[aria-hidden="true"]')?.childElementCount).toBe(3);
  expect(view.container.querySelectorAll("button, input, [tabindex]")).toHaveLength(0);
});
