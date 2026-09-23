import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, expect, it } from "vitest";

import { PageLayout, type PagePanel } from "./PageLayout";

const panels = (): PagePanel[] => [
  { id: "activity", label: "Activity", span: 2, content: <section>activity panel</section> },
  { id: "map", label: "Map", span: 1, content: <section>map panel</section> },
  { id: "crew", label: "Crew", span: 1, content: <section>crew panel</section> },
];

beforeEach(() => localStorage.clear());

it("persists panel visibility, width, order, and compact density per page", async () => {
  const user = userEvent.setup();
  const first = render(<PageLayout view="overview" panels={panels()} />);

  await user.click(screen.getByText("Layout"));
  await user.click(screen.getByLabelText("Map"));
  await user.selectOptions(screen.getByLabelText("width for Activity"), "3");
  await user.click(screen.getByLabelText("Move Crew earlier"));
  await user.click(screen.getByText("Compact panels"));

  expect(screen.queryByText("map panel")).toBeNull();
  expect(screen.getByText("activity panel").parentElement?.getAttribute("data-span")).toBe("3");
  first.unmount();

  const second = render(<PageLayout view="overview" panels={panels()} />);
  expect(screen.queryByText("map panel")).toBeNull();
  expect(screen.getByText("activity panel").parentElement?.getAttribute("data-span")).toBe("3");
  expect(second.container.querySelector(".page-layout")?.getAttribute("data-density")).toBe(
    "comfortable",
  );
  await user.click(screen.getByText("Layout"));
  await user.click(screen.getByLabelText("Map"));
  const text = second.container.querySelector(".page-layout__grid")?.textContent ?? "";
  expect(text.indexOf("crew panel")).toBeLessThan(text.indexOf("map panel"));
});

it("reset restores every panel and its default span", async () => {
  const user = userEvent.setup();
  render(<PageLayout view="work" panels={panels()} />);
  await user.click(screen.getByText("Layout"));
  await user.click(screen.getByLabelText("Map"));
  await user.click(screen.getByText("Reset layout"));

  expect(screen.getByText("map panel")).toBeDefined();
  expect(screen.getByText("activity panel").parentElement?.getAttribute("data-span")).toBe("2");
});
