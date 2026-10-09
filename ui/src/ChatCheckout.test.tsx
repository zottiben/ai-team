import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, expect, it, vi } from "vitest";
import { ChatCheckout } from "./ChatCheckout";
import type { CheckoutState, CheckoutOperation } from "./checkout-api";
const api=vi.hoisted(()=>({checkoutState:vi.fn(),checkoutPreview:vi.fn(),checkoutApprove:vi.fn(),submitReviewFix:vi.fn(),checkoutInspect:vi.fn(),checkoutAcknowledge:vi.fn()}));
vi.mock("./checkout-api",()=>api);
vi.mock("./review-api",()=>api);
const state=():CheckoutState=>({workspace:"/linked",workspace_epoch:7,head:"abc",branch:"feature",fingerprint:"bytes1",untracked:["new.txt"],untracked_files:[{file:{path:"new.txt",old_path:null,status:"added",binary:false,additions:0,deletions:0,hunks:[]},reason:"Empty file.",fingerprint:"new-bytes"}],staged:[],unstaged:[{path:"kept.txt",old_path:null,status:"modified",binary:false,additions:1,deletions:1,hunks:[{header:"@@ -1 +1 @@",old_start:1,new_start:1,lines:[{kind:"removed",old:1,new:null,text:"old"},{kind:"added",old:null,new:1,text:"new"}]}]}],findings:[],operations:[]});
const op=():CheckoutOperation=>({id:9,chat_id:4,rev:1,state:"preview",result:null,snapshot:{workspace:"/linked",head:"abc",branch:"feature",fingerprint:"bytes1",action:{kind:"stage",path:"new.txt"},remote:null}});
const props={chatId:4,tick:0,disabled:false,onChanged:vi.fn(),onFeedback:vi.fn()};
beforeEach(()=>{vi.clearAllMocks();api.checkoutState.mockResolvedValue(state());api.checkoutPreview.mockResolvedValue(op());api.checkoutApprove.mockResolvedValue({...op(),state:"done"});api.submitReviewFix.mockResolvedValue({turn:{started:true}});});
it("previews exact checkout state before any staging and approves only that receipt",async()=>{
 render(<ChatCheckout {...props}/>);fireEvent.click(await screen.findByRole("button",{name:"Stage new.txt"}));
 await screen.findByRole("region",{name:"Checkout action approval"});expect(api.checkoutPreview).toHaveBeenCalledWith(4,"bytes1",{kind:"stage",path:"new.txt"});expect(api.checkoutApprove).not.toHaveBeenCalled();
 fireEvent.click(screen.getByRole("button",{name:"Approve: Stage file"}));await waitFor(()=>expect(api.checkoutApprove).toHaveBeenCalledWith(4,op()));
});
it("invalidates a stale preview on refresh without sending the action",async()=>{
 const view=render(<ChatCheckout {...props}/>);fireEvent.click(await screen.findByRole("button",{name:"Stage new.txt"}));await screen.findByRole("region",{name:"Checkout action approval"});
 api.checkoutState.mockResolvedValue({...state(),fingerprint:"bytes2"});view.rerender(<ChatCheckout {...props} tick={1}/>);await screen.findByText(/This preview is stale/);expect((screen.getByRole("button",{name:"Approve: Stage file"}) as HTMLButtonElement).disabled).toBe(true);expect(api.checkoutApprove).not.toHaveBeenCalled();
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
it("does not inspect busy ticks repeatedly or offer checkout writes",async()=>{
 const view=render(<ChatCheckout {...props} disabled/>);expect((await screen.findByRole("button",{name:"Stage new.txt"}) as HTMLButtonElement).disabled).toBe(true);view.rerender(<ChatCheckout {...props} disabled tick={1}/>);view.rerender(<ChatCheckout {...props} disabled tick={2}/>);expect(api.checkoutState).toHaveBeenCalledTimes(1);expect(api.checkoutApprove).not.toHaveBeenCalled();
});
