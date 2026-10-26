import { act, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { setSynth } from "./ui-preferences";
import { ChatChanges } from "./ChatChanges";
import type { Changes, Delivery, DraftReview, DeliveryInspection } from "./changes-api";

const service = vi.hoisted(() => ({ chatChanges: vi.fn(), reviewDraft: vi.fn(), submitReviewFix: vi.fn(), previewDelivery: vi.fn(), approveDelivery: vi.fn(), inspectDelivery: vi.fn(), acknowledgeDelivery: vi.fn(), committedTree: vi.fn(), committedFile: vi.fn() }));
vi.mock("./changes-api", () => service);
vi.mock("./review-api", () => service);
vi.mock("./checkout-api", () => ({ checkoutState: vi.fn(async () => ({ workspace: "/repo", workspace_epoch: 9, head: "base-sha", branch: "main", fingerprint: "working", staged: [], unstaged: [], untracked: ["solo.txt"], untracked_files: [{ file: { path: "solo.txt", old_path: null, status: "added", binary: false, hunks: [], additions: 0, deletions: 0 }, reason: "Empty file.", fingerprint: "new-file" }], findings: [], operations: [] })) }));
const target = { run_id: 14, slice_key: "S1", revision: 7 };
const draft = { target, base_sha: "base-sha", commit_sha: "commit-sha", branch: "draft", lease_state: "released", worktree_path: "/pool/returned" };
const changes = (): Changes => ({ workspace_path: "/repo", workspace_epoch: 9, head: "base-sha", branch: "main", issues: [], staged: [], unstaged: [], untracked: ["solo.txt"], drafts: [draft], deliveries: [] });
const review = (): DraftReview => ({ draft, findings: [], files: [{ path: "feature.rs", old_path: null, status: "added", binary: false, additions: 1, deletions: 0, hunks: [{ header: "@@ -0,0 +1 @@", old_start: 0, new_start: 1, lines: [{ kind: "added", old: null, new: 1, text: "verified code" }] }] }] });
const delivery = (): Delivery => ({ id: 9, chat_id: 4, rev: 1, state: "preview", result: null, snapshot: { target, action: "push", commit_sha: "commit-sha", workspace_path: "/repo", workspace_epoch: 9, checkout_head: "base-sha", checkout_branch: "main", remote_url: "ssh://git@example/repo", delivery_branch: "chat-4/commit-sha", github_repo: null, base_branch: null, base_sha: null } });
const props = { chatId: 4, mode: "team" as const, tick: 0, disabled: false, onChanged: vi.fn() };
afterEach(() => { act(() => setSynth(false)); });
beforeEach(() => {
  act(() => setSynth(false));
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
it.each([false, true])("does not claim to still read changes after inspection failed (Synth %s)", async synth => {
  act(() => setSynth(synth));
  service.chatChanges.mockRejectedValue(new Error("Synthetic inspection failure"));
  render(<ChatChanges {...props}/>);
  await screen.findByRole("alert");
  expect(screen.queryByText("Reading actual Git changes…")).toBeNull();
  expect(screen.getByText("Changes unavailable. Refresh to retry.")).toBeTruthy();
  service.chatChanges.mockResolvedValue(changes());
  fireEvent.click(screen.getByRole("button", { name: "Refresh changes" }));
  await screen.findByText(/Working checkout · main/);
  expect(screen.queryByText("Changes unavailable. Refresh to retry.")).toBeNull();
  expect(screen.queryByRole("alert")).toBeNull();
});
it("shows real working changes separately from a chat's returned-worktree draft", async () => {
  render(<ChatChanges {...props} />);
  await screen.findByText("solo.txt");
  expect(screen.getByText(/Working checkout ·/).closest("details")?.open).toBe(true);
  await openReview();
  expect(service.reviewDraft).toHaveBeenCalledWith(4, target);
  expect(screen.getByText(/not attributed to this conversation/)).toBeTruthy();
  expect(screen.getByRole("button", { name: /comment on new line/ })).toBeTruthy();
  expect(service.previewDelivery).not.toHaveBeenCalled();
  expect(service.approveDelivery).not.toHaveBeenCalled();
});
it("does not expose another chat's drafts during a delayed switch", async () => {
  const view=render(<ChatChanges {...props}/>); await openReview();
  service.chatChanges.mockImplementation(()=>new Promise(()=>{}));
  view.rerender(<ChatChanges {...props} chatId={5}/>);
  expect(screen.queryByText("verified code")).toBeNull();
  expect(screen.queryByRole("button",{name:"Review S1"})).toBeNull();
});
it("invalidates a delivery preview after a checkout-generation change", async () => {
  const view=render(<ChatChanges {...props}/>); await openReview();
  fireEvent.click(screen.getByRole("button",{name:"Preview: Push draft"}));
  await screen.findByRole("region",{name:"Delivery approval"});
  service.chatChanges.mockResolvedValue({...changes(),workspace_epoch:10});
  view.rerender(<ChatChanges {...props} tick={1}/>);
  await waitFor(()=>expect(service.chatChanges).toHaveBeenCalledTimes(4));
  expect((screen.getByRole("button",{name:"Approve: Push draft"}) as HTMLButtonElement).disabled).toBe(true);
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
it("retains general findings and inline drafts when another verified slice is opened", async () => {
  const other = { ...draft, target: { ...target, slice_key: "S2" }, commit_sha: "other-sha" };
  service.chatChanges.mockResolvedValue({ ...changes(), drafts: [draft, other] });
  service.reviewDraft.mockImplementation((_chat, requested) => Promise.resolve(requested.slice_key === "S1" ? review() : { ...review(), draft: other, files: [{ ...review().files[0], path: "other.rs" }] }));
  render(<ChatChanges {...props}/>); await openReview();
  fireEvent.change(screen.getByLabelText("Review finding"), { target: { value: "Original slice note" } });
  fireEvent.click(screen.getByRole("button", { name: "comment on new line 1" }));
  fireEvent.change(screen.getByLabelText("comment on feature.rs new line 1"), { target: { value: "Original inline draft" } });
  fireEvent.click(screen.getByRole("button", { name: "Review S2" }));
  await waitFor(() => expect((screen.getByRole("textbox", { name: "Review finding" }) as HTMLTextAreaElement).value).toBe(""));
  fireEvent.click(screen.getByRole("button", { name: "Review S1" }));
  await waitFor(() => expect((screen.getByRole("textbox", { name: "Review finding" }) as HTMLTextAreaElement).value).toBe("Original slice note"));
  expect((screen.getByRole("textbox", { name: "comment on feature.rs new line 1" }) as HTMLTextAreaElement).value).toBe("Original inline draft");
  expect(service.submitReviewFix).not.toHaveBeenCalled();
});
it("starts with three readable team files and preserves an expanded comment across skins", async () => {
  setSynth(true);
  service.reviewDraft.mockResolvedValue({ ...review(), files: Array.from({ length: 5 }, (_, index) => ({ ...review().files[0], path: `file-${index}.ts` })) });
  render(<ChatChanges {...props}/>);
  fireEvent.click(await screen.findByRole("button", { name: "Review S1" }));
  expect((await screen.findByRole("button", { name: "Expand file-3.ts" })).getAttribute("aria-expanded")).toBe("false");
  fireEvent.click(screen.getByRole("button", { name: "Expand all readable files" }));
  const file = screen.getByRole("button", { name: "Collapse file-3.ts" }).closest("section")!;
  fireEvent.click(within(file).getByRole("button", { name: "comment on new line 1" }));
  const input = within(file).getByLabelText("comment on file-3.ts new line 1");
  fireEvent.change(input, { target: { value: "Keep this fourth file anchor" } });
  act(() => setSynth(false)); act(() => setSynth(true));
  expect(screen.getByLabelText("comment on file-3.ts new line 1")).toBe(input);
  expect((input as HTMLTextAreaElement).value).toBe("Keep this fourth file anchor");
  expect((screen.getByRole("button", { name: "Collapse file-3.ts" }) as HTMLButtonElement).disabled).toBe(true);
  expect(service.submitReviewFix).not.toHaveBeenCalled();
});
it("does not borrow a newer checkout generation for an unsent general finding", async () => {
  const view = render(<ChatChanges {...props}/>); await openReview();
  fireEvent.change(screen.getByLabelText("Review finding"), { target: { value: "Original generation" } });
  service.chatChanges.mockResolvedValue({ ...changes(), workspace_epoch: 10 });
  view.rerender(<ChatChanges {...props} tick={1}/>);
  await waitFor(() => expect(service.chatChanges).toHaveBeenCalledTimes(3));
  expect((screen.getByRole("button", { name: "Send finding for repair" }) as HTMLButtonElement).disabled).toBe(true);
  expect(service.submitReviewFix).not.toHaveBeenCalled();
});
it("does not erase a new general finding when an older submission finishes", async () => {
  let finish: (() => void) | undefined;
  service.submitReviewFix.mockImplementation(() => new Promise<void>(resolve => { finish = resolve; }));
  render(<ChatChanges {...props}/>); await openReview();
  fireEvent.change(screen.getByLabelText("Review finding"), { target: { value: "Submitted note" } });
  fireEvent.click(screen.getByRole("button", { name: "Send finding for repair" }));
  await waitFor(() => expect(service.submitReviewFix).toHaveBeenCalledTimes(1));
  fireEvent.change(screen.getByLabelText("Review finding"), { target: { value: "New unsent note" } });
  await act(async () => finish?.());
  expect((screen.getByLabelText("Review finding") as HTMLTextAreaElement).value).toBe("New unsent note");
});
it("keeps uncertain requests separate across exact draft revisions and replays the original one", async () => {
  service.submitReviewFix.mockRejectedValueOnce(Error("response lost"));
  const view = render(<ChatChanges {...props}/>); await openReview();
  fireEvent.change(screen.getByRole("textbox", { name: "Review finding" }), { target: { value: "Same text, explicit scope" } });
  fireEvent.click(screen.getByRole("button", { name: "Send finding for repair" }));
  await screen.findByText("response lost");
  const original = service.submitReviewFix.mock.calls[0];
  const next = { ...draft, target: { ...target, revision: 8 }, commit_sha: "next-commit" };
  service.chatChanges.mockResolvedValue({ ...changes(), workspace_epoch: 10, drafts: [next] });
  service.reviewDraft.mockResolvedValue({ ...review(), draft: next });
  view.rerender(<ChatChanges {...props} tick={1}/>);
  await screen.findByText(/This draft changed/);
  fireEvent.click(screen.getByRole("button", { name: "Review S1" }));
  await screen.findByRole("heading", { name: "Review S1 · next-commit" });
  fireEvent.change(screen.getByRole("textbox", { name: "Review finding" }), { target: { value: "Same text, explicit scope" } });
  fireEvent.click(screen.getByRole("button", { name: "Send finding for repair" }));
  await waitFor(() => expect(service.submitReviewFix).toHaveBeenCalledTimes(2));
  const newer = service.submitReviewFix.mock.calls[1];
  expect(newer?.[1]).toEqual(expect.objectContaining({ request_id: expect.any(String), workspace_epoch: 10, review: expect.objectContaining({ target: next.target }) }));
  expect(newer?.[1].request_id).not.toBe(original?.[1].request_id);
  const retained = await screen.findByRole("button", { name: "Return to retained review S1 · run 14 · revision 7 · generation 9" });
  await waitFor(() => expect((retained as HTMLButtonElement).disabled).toBe(false));
  fireEvent.click(retained);
  expect((screen.getByRole("textbox", { name: "Review finding" }) as HTMLTextAreaElement).value).toBe("Same text, explicit scope");
  fireEvent.click(screen.getByRole("button", { name: "Send finding for repair" }));
  await waitFor(() => expect(service.submitReviewFix).toHaveBeenCalledTimes(3));
  expect(service.submitReviewFix.mock.calls[2]).toEqual(original);
  expect(service.approveDelivery).not.toHaveBeenCalled();
});
it("does not offer a team repair in solo mode or change mode implicitly", async () => {
  render(<ChatChanges {...props} mode="single"/>); await openReview();
  await screen.findByText(/Switch to team to repair/);
  expect(screen.queryByRole("button", { name: /comment on/ })).toBeNull();
  fireEvent.change(screen.getByLabelText("Review finding"), { target: { value: "Keep for team" } });
  expect((screen.getByRole("button", { name: "Send finding for repair" }) as HTMLButtonElement).disabled).toBe(true);
  expect(service.submitReviewFix).not.toHaveBeenCalled();
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
it("uses the captured delivery hunks and existing exact approval controller in Synth", async () => {
  act(() => setSynth(true));
  const file = review().files[0]; if (!file) throw Error("fixture");
  const captured: Delivery = { ...delivery(), evidence: { basis: "Captured verified range", files_total: 1, omitted_files: 0, note: null,
    files: [{ area: "commit", file: { ...file, path: "captured-only.rs" }, revision: "commit-sha", reason: null, hunks_omitted: false }] } };
  service.previewDelivery.mockResolvedValue(captured);
  render(<ChatChanges {...props}/>); await openReview();
  fireEvent.click(screen.getByRole("button", { name: "Preview: Push draft" }));
  const dialog = await screen.findByRole("dialog", { name: "Delivery approval" });
  expect(within(dialog).getByText("captured-only.rs")).toBeTruthy();
  expect(within(dialog).getByText(/draft revision 7/)).toBeTruthy();
  expect(within(dialog).queryByRole("button", { name: /comment on/ })).toBeNull();
  expect(service.approveDelivery).not.toHaveBeenCalled();
  fireEvent.click(within(dialog).getByRole("button", { name: "Approve: Push draft" }));
  await waitFor(() => expect(service.approveDelivery).toHaveBeenCalledWith(4, captured));
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
