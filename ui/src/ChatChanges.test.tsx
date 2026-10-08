import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, expect, it, vi } from "vitest";
import { ChatChanges } from "./ChatChanges";
import type { Changes, Delivery, DraftReview, DeliveryInspection } from "./changes-api";

const service = vi.hoisted(() => ({ chatChanges: vi.fn(), reviewDraft: vi.fn(), submitReviewFix: vi.fn(), previewDelivery: vi.fn(), approveDelivery: vi.fn(), inspectDelivery: vi.fn(), acknowledgeDelivery: vi.fn(), committedTree: vi.fn(), committedFile: vi.fn() }));
vi.mock("./changes-api", () => service);
vi.mock("./review-api", () => service);
vi.mock("./checkout-api", () => ({ checkoutState: vi.fn(async () => ({ workspace: "/repo", head: "base-sha", branch: "main", fingerprint: "working", staged: [], unstaged: [], untracked: ["solo.txt"], findings: [], operations: [] })) }));
const target = { run_id: 14, slice_key: "S1", revision: 7 };
const draft = { target, base_sha: "base-sha", commit_sha: "commit-sha", branch: "draft", lease_state: "released", worktree_path: "/pool/returned" };
const changes = (): Changes => ({ workspace_path: "/repo", workspace_epoch: 9, head: "base-sha", branch: "main", issues: [], staged: [], unstaged: [], untracked: ["solo.txt"], drafts: [draft], deliveries: [] });
const review = (): DraftReview => ({ draft, findings: [], files: [{ path: "feature.rs", old_path: null, status: "added", binary: false, additions: 1, deletions: 0, hunks: [{ header: "@@ -0,0 +1 @@", old_start: 0, new_start: 1, lines: [{ kind: "added", old: null, new: 1, text: "verified code" }] }] }] });
const delivery = (): Delivery => ({ id: 9, chat_id: 4, rev: 1, state: "preview", result: null, snapshot: { target, action: "push", commit_sha: "commit-sha", workspace_path: "/repo", checkout_head: "base-sha", checkout_branch: "main", remote_url: "ssh://git@example/repo", delivery_branch: "chat-4/commit-sha", github_repo: null, base_branch: null, base_sha: null } });
const props = { chatId: 4, tick: 0, disabled: false, onChanged: vi.fn() };
beforeEach(() => {
  vi.clearAllMocks();
  service.chatChanges.mockResolvedValue(changes());
  service.reviewDraft.mockResolvedValue(review());
  service.previewDelivery.mockResolvedValue(delivery());
  service.approveDelivery.mockResolvedValue({ ...delivery(), state: "done" });
  service.submitReviewFix.mockResolvedValue({ turn: { started: true } });
});

async function openReview() {
  fireEvent.click(await screen.findByRole("button", { name: "Review S1" }));
  await screen.findByText("verified code");
}
it("shows real working changes separately from a chat's returned-worktree draft", async () => {
  render(<ChatChanges {...props} />);
  await screen.findByText("solo.txt");
  await openReview();
  expect(service.reviewDraft).toHaveBeenCalledWith(4, target);
  expect(screen.getByText(/not attributed to this conversation/)).toBeTruthy();
  expect(screen.getByRole("button", { name: /comment on new line/ })).toBeTruthy();
  expect(service.previewDelivery).not.toHaveBeenCalled();
  expect(service.approveDelivery).not.toHaveBeenCalled();
});
it("browses objects from the reviewed commit rather than the current checkout", async () => {
  service.committedTree.mockResolvedValue([{ path: "README.md", kind: "blob", size: 15 }]);
  service.committedFile.mockResolvedValue({ path: "README.md", commit_sha: "commit-sha", text: "committed content", reason: null });
  render(<ChatChanges {...props} />); await openReview();
  fireEvent.click(screen.getByRole("button", { name: "Browse committed tree" }));
  fireEvent.change(await screen.findByRole("combobox", { name: "File in reviewed commit" }), { target: { value: "README.md" } });
  fireEvent.click(screen.getByRole("button", { name: "Read committed file" }));
  await screen.findByText("committed content");
  expect(service.committedFile).toHaveBeenCalledWith(4, target, "README.md");
  expect(service.approveDelivery).not.toHaveBeenCalled();
});
it("submits findings for an exact repair without approving publication", async () => {
  render(<ChatChanges {...props} />); await openReview();
  fireEvent.change(screen.getByRole("textbox", { name: "Review finding" }), { target: { value: "Check cancellation" } });
  fireEvent.click(screen.getByRole("button", { name: "Send finding for repair" }));
  await waitFor(() => expect(service.submitReviewFix).toHaveBeenCalledWith(4, { request_id: expect.any(String), workspace_epoch: 9, review: { kind: "draft", target, body: "Check cancellation", anchor: null } }));
  expect(service.approveDelivery).not.toHaveBeenCalled();
});
it("sends an inline team comment with its exact commit revision and line, retaining rejected drafts",async()=>{
 service.submitReviewFix.mockRejectedValueOnce(Error("draft branch changed"));
 render(<ChatChanges {...props}/>);await openReview();
 fireEvent.click(screen.getByRole("button",{name:"comment on new line 1"}));
 fireEvent.change(screen.getByLabelText("comment on feature.rs new line 1"),{target:{value:"Handle cancellation here"}});
 fireEvent.click(screen.getByRole("button",{name:"Comment"}));
 await screen.findByText("draft branch changed");
 expect((screen.getByLabelText("comment on feature.rs new line 1") as HTMLTextAreaElement).value).toBe("Handle cancellation here");
 const first=service.submitReviewFix.mock.calls[0];
 expect(first).toEqual([4,{request_id:expect.any(String),workspace_epoch:9,review:{kind:"draft",target,body:"Handle cancellation here",anchor:{path:"feature.rs",side:"new",line:1}}}]);
 fireEvent.click(screen.getByRole("button",{name:"Comment"}));
 await screen.findByText(/Review submitted to the agent/);
 expect(service.submitReviewFix.mock.calls[1]).toEqual(first);
 expect(service.approveDelivery).not.toHaveBeenCalled();
});
it("requires separate preview and approval with the exact destination and revision", async () => {
  render(<ChatChanges {...props} />); await openReview();
  fireEvent.click(screen.getByRole("button", { name: "Preview: Push draft" }));
  await screen.findByRole("region", { name: "Delivery approval" });
  expect(service.previewDelivery).toHaveBeenCalledWith(4, target, "push");
  expect(service.approveDelivery).not.toHaveBeenCalled();
  expect(screen.getByText(/ssh:\/\/git@example\/repo/)).toBeTruthy();
  fireEvent.click(screen.getByRole("button", { name: "Approve: Push draft" }));
  await waitFor(() => expect(service.approveDelivery).toHaveBeenCalledWith(4, delivery()));
});
it("keeps refusals visible and refreshes durable results rather than claiming success", async () => {
  service.approveDelivery.mockResolvedValue({ ...delivery(), state: "refused", result: "Origin changed; review again" });
  render(<ChatChanges {...props} />); await openReview();
  fireEvent.click(screen.getByRole("button", { name: "Preview: Push draft" }));
  fireEvent.click(await screen.findByRole("button", { name: "Approve: Push draft" }));
  await waitFor(() => expect(screen.getByRole("alert").textContent).toContain("Origin changed"));
  await waitFor(() => expect(service.chatChanges.mock.calls.length).toBeGreaterThan(3));
});
it("disables delivery of a stale reviewed slice", async () => {
  const view = render(<ChatChanges {...props} />); await openReview();
  service.chatChanges.mockResolvedValue({ ...changes(), drafts: [{ ...draft, target: { ...target, revision: 8 } }] });
  view.rerender(<ChatChanges {...props} tick={1} />);
  await screen.findByText(/This draft changed/);
  expect((screen.getByRole("button", { name: "Preview: Push draft" }) as HTMLButtonElement).disabled).toBe(true);
});
it("drains and inspects before a separately reasoned keep-without-certification", async () => {
  const pending = { ...delivery(), state: "inspection" as const, rev: 3 };
  service.chatChanges.mockResolvedValue({ ...changes(), deliveries: [pending] });
  const inspected: DeliveryInspection = { delivery: { ...pending, rev: 5 }, checkout: { head: "manual-sha", branch: "main", status: " M kept.txt" } };
  service.inspectDelivery.mockResolvedValue(inspected);
  service.acknowledgeDelivery.mockResolvedValue({ ...inspected, delivery: { ...pending, state: "acknowledged" } });
  render(<ChatChanges {...props} />);
  fireEvent.click(await screen.findByRole("button", { name: "Drain and inspect delivery #9" }));
  await screen.findByRole("form", { name: "Keep inspected delivery" });
  expect(service.acknowledgeDelivery).not.toHaveBeenCalled();
  fireEvent.change(screen.getByRole("textbox", { name: "Reason for keeping this outcome" }), { target: { value: "Keep my manual result" } });
  fireEvent.click(screen.getByRole("button", { name: "Acknowledge and keep current state" }));
  await waitFor(() => expect(service.acknowledgeDelivery).toHaveBeenCalledWith(4, inspected, "Keep my manual result"));
  expect(service.approveDelivery).not.toHaveBeenCalled();
});
it("does not spawn repeated Git inspections for busy execution ticks", async () => {
  const view = render(<ChatChanges {...props} disabled />);
  await screen.findByText("solo.txt");
  view.rerender(<ChatChanges {...props} disabled tick={2} />);
  view.rerender(<ChatChanges {...props} disabled tick={3} />);
  expect(service.chatChanges).toHaveBeenCalledTimes(1);
  view.rerender(<ChatChanges {...props} tick={4} />);
  await waitFor(() => expect(service.chatChanges).toHaveBeenCalledTimes(2));
});
