import { fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { beforeEach, expect, it, vi } from "vitest";

import { ThinkingSelect } from "./ThinkingSelect";

const service = vi.hoisted(() => ({ getModelThinking: vi.fn() }));
vi.mock("./api", async (original) => ({
  ...(await original<typeof import("./api")>()),
  getModelThinking: service.getModelThinking,
}));

const select = () =>
  screen.getByRole("combobox", { name: "Thinking effort" }) as HTMLSelectElement;
const option = (value: string) =>
  within(select()).getByRole("option", { name: new RegExp(value, "i") }) as HTMLOptionElement;

const capabilities = (levels: string[]) => ({ provider: "openai", model: "fixture", fallback_reason: null, levels });

beforeEach(() => {
  service.getModelThinking.mockReset();
  service.getModelThinking.mockResolvedValue(capabilities([
    "none",
    "minimal",
    "low",
    "medium",
    "high",
    "xhigh",
    "max",
  ]));
});

it("offers every canonical level under the name a person reads", async () => {
  render(
    <ThinkingSelect provider="openai" model="gpt-5" value="high" onChange={vi.fn()} />,
  );
  await waitFor(() => expect(service.getModelThinking).toHaveBeenCalledWith("openai", "gpt-5"));
  expect(
    [...select().options].map((o) => `${o.value}:${o.textContent}`),
  ).toEqual([
    "none:Off",
    "minimal:Minimal",
    "low:Low",
    "medium:Medium",
    "high:High",
    "xhigh:XHigh",
    "max:Max",
  ]);
  expect(select().value).toBe("high");
});

it("disables what the selected model cannot do, including off on an always-on model", async () => {
  // An always-on model is exactly the case the old four-level picker could not express:
  // its thinking cannot be turned off, so `Off` has to stop being an offer.
  service.getModelThinking.mockResolvedValue(capabilities(["low", "medium", "high"]));
  render(
    <ThinkingSelect provider="claude" model="opus" value="medium" onChange={vi.fn()} />,
  );
  await waitFor(() => expect(option("^Off").disabled).toBe(true));
  for (const level of ["minimal", "xhigh", "max"]) expect(option(`^${level}`).disabled).toBe(true);
  for (const level of ["low", "medium", "high"]) expect(option(`^${level}`).disabled).toBe(false);
  expect(select().disabled).toBe(false);
});

it("keeps a selection the model does not support visible, and never rewrites it", async () => {
  const onChange = vi.fn();
  service.getModelThinking.mockResolvedValue(capabilities(["low", "medium"]));
  render(<ThinkingSelect provider="zai" model="glm" value="max" onChange={onChange} />);
  await waitFor(() => expect(option("^Max").disabled).toBe(false));
  // Still the selected value, still choosable, and said out loud rather than silently
  // swapped for something the model will accept.
  expect(select().value).toBe("max");
  expect(screen.getByText(/Max is not supported/i)).not.toBeNull();
  expect(onChange).not.toHaveBeenCalled();
});

it("keeps a turn that never recorded a level readable instead of inventing one", async () => {
  render(<ThinkingSelect provider="openai" model="gpt-5" value="" onChange={vi.fn()} />);
  await waitFor(() => expect(select().value).toBe(""));
  expect(screen.getByRole("option", { name: "Not recorded" })).not.toBeNull();
});

it("ignores a capability answer for a model that is no longer selected", async () => {
  // Switching models faster than the network answers is how a picker ends up enforcing
  // one model's limits against another's.
  const answers = new Map<string, (levels: string[]) => void>();
  const answer = (model: string, levels: string[]) => {
    const resolve = answers.get(model);
    if (!resolve) throw new Error(`no outstanding lookup for ${model}`);
    resolve(levels);
  };
  service.getModelThinking.mockImplementation(
    (_p: string, model: string) =>
      new Promise((resolve) => {
        answers.set(model, (levels) => resolve(capabilities(levels)));
      }),
  );
  const view = render(
    <ThinkingSelect provider="openai" model="slow" value="max" onChange={vi.fn()} />,
  );
  view.rerender(
    <ThinkingSelect provider="openai" model="fast" value="max" onChange={vi.fn()} />,
  );
  answer("fast", ["max"]);
  await waitFor(() => expect(option("^Max").disabled).toBe(false));
  // The abandoned model's answer lands late and must change nothing.
  answer("slow", ["low"]);
  await waitFor(() => expect(option("^Low").disabled).toBe(true));
  expect(screen.queryByText(/Max is not supported/i)).toBeNull();
});

it("preserves the preference but does not claim unknown levels are supported", async () => {
  service.getModelThinking.mockRejectedValue(new Error("offline"));
  const onChange = vi.fn();
  render(<ThinkingSelect provider="openai" model="gpt-5" value="high" onChange={onChange} />);
  await waitFor(() => expect(screen.getByText(/Supported levels are unavailable/i)).not.toBeNull());
  expect(select().disabled).toBe(true);
  expect(select().value).toBe("high");
  expect(onChange).not.toHaveBeenCalled();
  service.getModelThinking.mockResolvedValue(capabilities(["low", "high"]));
  fireEvent.click(screen.getByRole("button", { name: "Refresh thinking levels" }));
  await waitFor(() => expect(select().disabled).toBe(false));
  expect(onChange).not.toHaveBeenCalled();
  expect(service.getModelThinking).toHaveBeenCalledTimes(2);
});

it("shows the policy fallback's capabilities without changing the requested effort", async () => {
  service.getModelThinking.mockResolvedValue({ provider: "local", model: "default", fallback_reason: "openai is denied by the machine profile", levels: ["low", "high"] });
  const onChange = vi.fn();
  render(<ThinkingSelect provider="openai" model="preferred" value="max" onChange={onChange} />);
  expect(await screen.findByText(/These levels apply to local\/default/)).not.toBeNull();
  expect(screen.getByText(/Max is not supported/)).not.toBeNull();
  expect(select().value).toBe("max");
  expect(onChange).not.toHaveBeenCalled();
  fireEvent.change(select(), { target: { value: "high" } });
  expect(onChange).toHaveBeenCalledWith("high");
});

it("asks for nothing until a model is chosen", async () => {
  render(<ThinkingSelect provider={null} model={null} value="medium" onChange={vi.fn()} />);
  await waitFor(() => expect(select().value).toBe("medium"));
  expect(service.getModelThinking).not.toHaveBeenCalled();
  expect(select().disabled).toBe(true);
});

it("does not apply the previous model's choices while the next lookup is pending", async () => {
  service.getModelThinking.mockResolvedValueOnce(capabilities(["max"]));
  const view = render(<ThinkingSelect provider="openai" model="first" value="max" onChange={vi.fn()} />);
  await waitFor(() => expect(select().disabled).toBe(false));
  service.getModelThinking.mockImplementation(() => new Promise(() => {}));
  view.rerender(<ThinkingSelect provider="openai" model="next" value="max" onChange={vi.fn()} />);
  expect(select().disabled).toBe(true);
  expect(select().value).toBe("max");
});
