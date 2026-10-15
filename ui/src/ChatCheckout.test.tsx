import { act, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { setSynth } from "./ui-preferences";
import { ChatCheckout } from "./ChatCheckout";
import type { CheckoutState, CheckoutOperation } from "./checkout-api";
const api=vi.hoisted(()=>({checkoutState:vi.fn(),checkoutPreview:vi.fn(),checkoutApprove:vi.fn(),submitReviewFix:vi.fn(),checkoutInspect:vi.fn(),checkoutAcknowledge:vi.fn()}));
vi.mock("./checkout-api",()=>api);
vi.mock("./review-api",()=>api);
const state=():CheckoutState=>({workspace:"/linked",workspace_epoch:7,head:"abc",branch:"feature",fingerprint:"bytes1",untracked:["new.txt"],untracked_files:[{file:{path:"new.txt",old_path:null,status:"added",binary:false,additions:0,deletions:0,hunks:[]},reason:"Empty file.",fingerprint:"new-bytes"}],staged:[],unstaged:[{path:"kept.txt",old_path:null,status:"modified",binary:false,additions:1,deletions:1,hunks:[{header:"@@ -1 +1 @@",old_start:1,new_start:1,lines:[{kind:"removed",old:1,new:null,text:"old"},{kind:"added",old:null,new:1,text:"new"}]}]}],findings:[],operations:[]});
const op=():CheckoutOperation=>({id:9,chat_id:4,rev:1,state:"preview",result:null,snapshot:{workspace:"/linked",workspace_epoch:7,head:"abc",branch:"feature",fingerprint:"bytes1",action:{kind:"stage",path:"new.txt"},remote:null}});
const props={chatId:4,tick:0,disabled:false,onChanged:vi.fn(),onFeedback:vi.fn()};
afterEach(()=>{setSynth(false);});
beforeEach(()=>{localStorage.clear();setSynth(false);vi.clearAllMocks();api.checkoutState.mockResolvedValue(state());api.checkoutPreview.mockResolvedValue(op());api.checkoutApprove.mockResolvedValue({...op(),state:"done"});api.submitReviewFix.mockResolvedValue({turn:{started:true}});});
it("previews exact checkout state before any staging and approves only that receipt",async()=>{
 render(<ChatCheckout {...props}/>);fireEvent.click(await screen.findByRole("button",{name:"Stage new.txt"}));
 await screen.findByRole("region",{name:"Checkout action approval"});expect(api.checkoutPreview).toHaveBeenCalledWith(4,"bytes1",{kind:"stage",path:"new.txt"});expect(api.checkoutApprove).not.toHaveBeenCalled();
 fireEvent.click(screen.getByRole("button",{name:"Approve: Stage file"}));await waitFor(()=>expect(api.checkoutApprove).toHaveBeenCalledWith(4,op()));
});
it("invalidates a stale preview on refresh without sending the action",async()=>{
 const view=render(<ChatCheckout {...props}/>);fireEvent.click(await screen.findByRole("button",{name:"Stage new.txt"}));await screen.findByRole("region",{name:"Checkout action approval"});
 api.checkoutState.mockResolvedValue({...state(),fingerprint:"bytes2"});view.rerender(<ChatCheckout {...props} tick={1}/>);await screen.findByText(/This preview is stale/);expect((screen.getByRole("button",{name:"Approve: Stage file"}) as HTMLButtonElement).disabled).toBe(true);expect(api.checkoutApprove).not.toHaveBeenCalled();
});
it("does not expose a previous chat's checkout while the next request is pending", async () => {
 const view=render(<ChatCheckout {...props}/>); await screen.findByRole("button",{name:"Stage new.txt"});
 api.checkoutState.mockImplementation(()=>new Promise(()=>{}));
 view.rerender(<ChatCheckout {...props} chatId={5}/>);
 expect(screen.queryByRole("button",{name:"Stage new.txt"})).toBeNull();
 expect(screen.queryByText("/linked · feature · abc")).toBeNull();
});
it("refuses approval from another checkout generation even if a supplied fingerprint repeats", async () => {
 const view=render(<ChatCheckout {...props}/>); fireEvent.click(await screen.findByRole("button",{name:"Stage new.txt"}));
 await screen.findByRole("region",{name:"Checkout action approval"});
 api.checkoutState.mockResolvedValue({...state(),workspace_epoch:8}); view.rerender(<ChatCheckout {...props} tick={1}/>);
 await waitFor(()=>expect(api.checkoutState).toHaveBeenCalledTimes(3));
 expect((screen.getByRole("button",{name:"Approve: Stage file"}) as HTMLButtonElement).disabled).toBe(true);
 expect(api.checkoutApprove).not.toHaveBeenCalled();
});
it("keeps a historical-format preview readable without treating an unrecorded epoch as current",async()=>{
 const legacy=op(); delete legacy.snapshot.workspace_epoch; api.checkoutPreview.mockResolvedValue(legacy);
 render(<ChatCheckout {...props}/>); fireEvent.click(await screen.findByRole("button",{name:"Stage new.txt"}));
 await screen.findByText(/Checkout generation was not recorded/);
 expect((screen.getByRole("button",{name:"Approve: Stage file"}) as HTMLButtonElement).disabled).toBe(true);
 expect(api.checkoutApprove).not.toHaveBeenCalled();
});
it("invalidates Viewed on non-rendered file-byte changes without clearing an unrelated file", async()=>{
 const first={...state(),unstaged_revisions:{"kept.txt":"file-bytes-one"}};
 api.checkoutState.mockResolvedValue(first); const view=render(<ChatCheckout {...props}/>);
 fireEvent.click(await screen.findByRole("checkbox",{name:"Viewed kept.txt"}));
 api.checkoutState.mockResolvedValue({...first,fingerprint:"unrelated-change"}); view.rerender(<ChatCheckout {...props} tick={1}/>);
 await waitFor(()=>expect(api.checkoutState).toHaveBeenCalledTimes(2));
 expect((screen.getByRole("checkbox",{name:"Viewed kept.txt"}) as HTMLInputElement).checked).toBe(true);
 api.checkoutState.mockResolvedValue({...first,fingerprint:"newline-change",unstaged_revisions:{"kept.txt":"file-bytes-two"}});
 view.rerender(<ChatCheckout {...props} tick={2}/>);
 await waitFor(()=>expect((screen.getByRole("checkbox",{name:"Viewed kept.txt"}) as HTMLInputElement).checked).toBe(false));
});
it("keeps feedback drafts after rejected line anchors without approving publication",async()=>{
 api.submitReviewFix.mockRejectedValue(Error("diff changed"));render(<ChatCheckout {...props}/>);fireEvent.click(await screen.findByRole("button",{name:"comment on new line 1"}));fireEvent.change(screen.getByLabelText("comment on kept.txt new line 1"),{target:{value:"Check error handling"}});fireEvent.click(screen.getByRole("button",{name:"Comment"}));await screen.findByText("diff changed");expect((screen.getByLabelText("comment on kept.txt new line 1") as HTMLTextAreaElement).value).toBe("Check error handling");expect(api.submitReviewFix).toHaveBeenCalledWith(4,expect.objectContaining({request_id:expect.any(String),workspace_epoch:7,review:{kind:"checkout",finding:expect.objectContaining({fingerprint:"bytes1",path:"kept.txt",side:"new",line:1,area:"unstaged"})}}));expect(props.onFeedback).not.toHaveBeenCalled();expect(api.checkoutApprove).not.toHaveBeenCalled();
});
it("retries a lost submission with its original request, anchor and checkout generation",async()=>{
 api.submitReviewFix.mockRejectedValueOnce(Error("response lost"));
 const view=render(<ChatCheckout {...props}/>);
 fireEvent.click(await screen.findByRole("button",{name:"comment on new line 1"}));
 fireEvent.change(screen.getByLabelText("comment on kept.txt new line 1"),{target:{value:"Fix this line"}});
 fireEvent.click(screen.getByRole("button",{name:"Comment"}));
 await screen.findByText("response lost");
 const original=api.submitReviewFix.mock.calls[0];
 if(!original)throw Error("the first submission was not recorded");
 api.checkoutState.mockResolvedValue({...state(),fingerprint:"new-bytes",workspace_epoch:9});
 view.rerender(<ChatCheckout {...props} tick={3}/>);
 await waitFor(()=>expect(api.checkoutState.mock.calls.length).toBeGreaterThan(1));
 fireEvent.click(screen.getByRole("button",{name:"Comment"}));
 await screen.findByText(/Review submitted to the agent/);
 expect(api.submitReviewFix.mock.calls[1]).toEqual(original);
 expect(original[1].workspace_epoch).toBe(7);
 expect(original[1].review.finding.fingerprint).toBe("bytes1");
 expect(props.onFeedback).not.toHaveBeenCalled();
 expect(api.checkoutApprove).not.toHaveBeenCalled();
});
it("offers recorded feedback as composer text, not approval or delivery",async()=>{
 api.checkoutState.mockResolvedValue({...state(),findings:[{id:1,head:"abc",fingerprint:"previous",area:"unstaged",path:"kept.txt",side:"new",line:1,body:"check this",created_at:"now"}]});render(<ChatCheckout {...props}/>);fireEvent.click(await screen.findByRole("button",{name:"Use feedback in next message"}));expect(props.onFeedback).toHaveBeenCalledWith(expect.stringContaining("not publication approval"));expect(api.checkoutApprove).not.toHaveBeenCalled();
});
it("shows uncertain commands after reload and requires inspection plus a reason",async()=>{
 const uncertain={...op(),state:"inspection",rev:3,result:"may have happened"};api.checkoutState.mockResolvedValue({...state(),operations:[uncertain]});const inspected={operation:{...uncertain,rev:5},checkout:state()};api.checkoutInspect.mockResolvedValue(inspected);api.checkoutAcknowledge.mockResolvedValue({...uncertain,state:"acknowledged"});render(<ChatCheckout {...props}/>);expect((await screen.findByRole("button",{name:"Stage new.txt"}) as HTMLButtonElement).disabled).toBe(true);fireEvent.click(screen.getByRole("button",{name:"Drain and inspect checkout action #9"}));await screen.findByText("Keep inspected checkout state");expect(api.checkoutAcknowledge).not.toHaveBeenCalled();fireEvent.change(screen.getByLabelText("Reason for keeping the state"),{target:{value:"Inspected files and remote"}});fireEvent.click(screen.getByRole("button",{name:"Acknowledge checkout state"}));await waitFor(()=>expect(api.checkoutAcknowledge).toHaveBeenCalledWith(4,inspected,"Inspected files and remote"));expect(api.checkoutApprove).not.toHaveBeenCalled();
});
it("reviews untracked text and submits its exact new-side anchor without staging",async()=>{
 const data=state();
 const source=data.unstaged[0];
 if(!source)throw Error("missing fixture diff");
 data.unstaged=[];
 data.untracked_files=[{file:{...source,path:"new.txt",status:"added",deletions:0,hunks:[{header:"@@ -0,0 +1 @@",old_start:0,new_start:1,lines:[{kind:"added",old:null,new:1,text:"untracked source"}]}]},reason:null,fingerprint:"untracked-bytes"}];
 api.checkoutState.mockResolvedValue(data);
 render(<ChatCheckout {...props}/>);
 await screen.findByText("untracked source");
 fireEvent.click(screen.getByRole("button",{name:"comment on new line 1"}));
 fireEvent.change(screen.getByLabelText("comment on new.txt new line 1"),{target:{value:"Check this new file"}});
 fireEvent.click(screen.getByRole("button",{name:"Comment"}));
 await waitFor(()=>expect(api.submitReviewFix).toHaveBeenCalledWith(4,expect.objectContaining({workspace_epoch:7,review:{kind:"checkout",finding:expect.objectContaining({path:"new.txt",area:"untracked",side:"new",line:1,fingerprint:"bytes1"})}})));
 expect(api.checkoutPreview).not.toHaveBeenCalled();
 expect(api.checkoutApprove).not.toHaveBeenCalled();
});
it("retains an unsent comment when its file moves out of the current diff",async()=>{
 const first=state(), view=render(<ChatCheckout {...props}/>);
 fireEvent.click(await screen.findByRole("button",{name:"comment on new line 1"}));
 fireEvent.change(screen.getByLabelText("comment on kept.txt new line 1"),{target:{value:"Keep this original review"}});
 api.checkoutState.mockResolvedValue({...first,fingerprint:"staged",staged:first.unstaged,unstaged:[]});
 view.rerender(<ChatCheckout {...props} tick={1}/>);
 await screen.findByRole("button",{name:"Unstage kept.txt"});
 expect((screen.getByLabelText("comment on kept.txt new line 1") as HTMLTextAreaElement).value).toBe("Keep this original review");
 expect((screen.getByRole("button",{name:"Comment"}) as HTMLButtonElement).disabled).toBe(true);
 expect(api.submitReviewFix).not.toHaveBeenCalled();
 fireEvent.click(screen.getByRole("button",{name:"Cancel comment"}));
 expect(screen.queryByLabelText("comment on kept.txt new line 1")).toBeNull();
 expect(screen.queryByRole("button",{name:"Stage kept.txt"})).toBeNull();
});
it("preserves a newer commit message after approval of the captured message",async()=>{
 const initial=state();api.checkoutState.mockResolvedValue({...initial,staged:initial.unstaged,unstaged:[]});
 api.checkoutPreview.mockResolvedValue({...op(),snapshot:{...op().snapshot,action:{kind:"commit",message:"First message"}}});
 render(<ChatCheckout {...props}/>);const input=await screen.findByLabelText("Manual commit message");
 fireEvent.change(input,{target:{value:"First message"}});fireEvent.click(screen.getByRole("button",{name:"Preview manual commit"}));
 await screen.findByRole("button",{name:"Approve: Manual commit"});fireEvent.change(input,{target:{value:"Next message, not submitted"}});
 fireEvent.click(screen.getByRole("button",{name:"Approve: Manual commit"}));
 await waitFor(()=>expect(api.checkoutApprove).toHaveBeenCalledTimes(1));
 await waitFor(()=>expect((input as HTMLTextAreaElement).value).toBe("Next message, not submitted"));
});
it("shows first-three Synth files and retains visible comments and Viewed across skins",async()=>{
 setSynth(true);const initial=state(), source=initial.unstaged[0];if(!source)throw Error("fixture");
 initial.unstaged=Array.from({length:5},(_,n)=>({...source,path:`file${n}.ts`}));api.checkoutState.mockResolvedValue(initial);
 const scroll=vi.fn();Object.defineProperty(HTMLElement.prototype,"scrollIntoView",{value:scroll,configurable:true});
 render(<ChatCheckout {...props}/>);
 await screen.findByRole("button",{name:"Stage file0.ts"});
 expect(screen.queryByRole("button",{name:"Stage file3.ts"})).toBeNull();
 expect(screen.queryByRole("button",{name:"Stage new.txt"})).toBeNull();
 fireEvent.click(screen.getByRole("button",{name:"Show file3.ts · not viewed"}));
 const card=(await screen.findByRole("button",{name:"Collapse file3.ts"})).closest(".diff");if(!card)throw Error("file missing");
 fireEvent.click(within(card as HTMLElement).getByRole("button",{name:"comment on new line 1"}));
 const input=screen.getByLabelText("comment on file3.ts new line 1");fireEvent.change(input,{target:{value:"Retained through skin changes"}});
 act(()=>setSynth(false));act(()=>setSynth(true));
 expect(screen.getByLabelText("comment on file3.ts new line 1")).toBe(input);
 fireEvent.click(screen.getByRole("checkbox",{name:"Viewed file3.ts"}));
 expect(screen.getByLabelText("comment on file3.ts new line 1")).toBe(input);
 expect(screen.getByRole("button",{name:"Show file3.ts · viewed"}).getAttribute("data-viewed")).toBe("true");
 expect(api.checkoutPreview).not.toHaveBeenCalled();expect(api.checkoutApprove).not.toHaveBeenCalled();
 Reflect.deleteProperty(HTMLElement.prototype,"scrollIntoView");
});
it("opens a Synth confirmation with captured hunks that do not follow refreshed files",async()=>{
 setSynth(true); const initial=state(), file=initial.unstaged[0]; if(!file)throw Error("fixture");
 const captured:CheckoutOperation={...op(),snapshot:{...op().snapshot,action:{kind:"stage",path:"kept.txt"}},evidence:{basis:"Captured index/worktree",files_total:1,omitted_files:0,note:null,files:[{area:"unstaged",file,revision:"captured-file",reason:null,hunks_omitted:false}]}};
 api.checkoutPreview.mockResolvedValue(captured);
 const view=render(<ChatCheckout {...props}/>);
 fireEvent.click(await screen.findByRole("button",{name:"Stage kept.txt"}));
 const dialog=await screen.findByRole("dialog",{name:"Checkout action approval"});
 expect(within(dialog).getByText("new")).toBeTruthy();
 expect(within(dialog).queryByRole("button",{name:/comment on/})).toBeNull();
 expect(api.checkoutApprove).not.toHaveBeenCalled();
 api.checkoutState.mockResolvedValue({...initial,fingerprint:"changed-bytes",unstaged:[{...file,hunks:file.hunks.map(hunk=>({...hunk,lines:hunk.lines.map(line=>({...line,text:"refreshed replacement"}))}))}]});
 view.rerender(<ChatCheckout {...props} tick={1}/>);
 await within(dialog).findByText(/Changed since preview/);
 expect(within(dialog).getByText("new")).toBeTruthy();
 expect(within(dialog).queryByText("refreshed replacement")).toBeNull();
 expect((within(dialog).getByRole("button",{name:"Approve: Stage file"}) as HTMLButtonElement).disabled).toBe(true);
 fireEvent.keyDown(document,{key:"Escape"});
 expect(screen.queryByRole("dialog",{name:"Checkout action approval"})).toBeNull();
 expect(document.activeElement).toBe(screen.getByRole("button",{name:"Inspect captured checkout approval"}));
 expect(api.checkoutApprove).not.toHaveBeenCalled();
});
it("does not inspect busy ticks repeatedly or offer checkout writes",async()=>{
 const view=render(<ChatCheckout {...props} disabled/>);expect((await screen.findByRole("button",{name:"Stage new.txt"}) as HTMLButtonElement).disabled).toBe(true);view.rerender(<ChatCheckout {...props} disabled tick={1}/>);view.rerender(<ChatCheckout {...props} disabled tick={2}/>);expect(api.checkoutState).toHaveBeenCalledTimes(1);expect(api.checkoutApprove).not.toHaveBeenCalled();
});
