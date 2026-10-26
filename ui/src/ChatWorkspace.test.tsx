import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { useState } from "react";
import { beforeEach, expect, it, vi } from "vitest";
import { ChatWorkspace } from "./ChatWorkspace";
import type { ChatDetail } from "./chat-api";

const api=vi.hoisted(()=>({chatWorkspaces:vi.fn(),workspaceRequests:vi.fn(),requestWorkspace:vi.fn(),approveWorkspace:vi.fn(),cancelWorkspace:vi.fn(),workspaceSetups:vi.fn(),startWorkspaceSetup:vi.fn(),commandWorkspaceSetup:vi.fn()}));
vi.mock("./chat-workspace-api",()=>api);
const detail:ChatDetail={id:1,project_id:2,title:"Chat",workspace_path:"/repo",provider:"local",model:"fixture",reasoning:"high",mode:"single",archived:false,active_node_id:null,stop_requested:false,rev:7,created_at:"",updated_at:"",live_text:"",turns:[],team_builds:[],followups:[],state:"idle",can_resume:false,orphan_running:false};
const pending={id:9,chat_id:1,after_node_id:3,from_path:"/repo",to_path:"/linked",state:"pending"};
const props=()=>({project:"demo",detail:null as ChatDetail|null,selected:null,tick:0,disabled:false,onSelect:vi.fn(),onChanged:vi.fn(),onPending:vi.fn()});
beforeEach(()=>{
 vi.clearAllMocks();
 api.chatWorkspaces.mockResolvedValue([{path:"/repo",name:"main",branch:"main",unavailable:null},{path:"/linked",name:"3",branch:"feature",unavailable:null},{path:"/busy",name:"4",branch:null,unavailable:"occupied"}]);
 api.workspaceSetups.mockResolvedValue([]);
 api.workspaceRequests.mockResolvedValue({requests:[]});
 api.approveWorkspace.mockResolvedValue({...detail,workspace_path:"/linked"});
});
it("separates existing checkout selection from setup and preserves its unsent branch draft",async()=>{
 const input=props();render(<ChatWorkspace {...input}/>);
 fireEvent.click(screen.getByRole("button",{name:"Choose checkout"}));
 const existing=await screen.findByRole("button",{name:"Existing checkout"});
 expect(existing.getAttribute("aria-pressed")).toBe("true");
 expect(screen.queryByRole("textbox",{name:"New branch (optional)"})).toBeNull();
 fireEvent.click(screen.getByRole("button",{name:"Set up a new AWT worktree"}));
 fireEvent.change(await screen.findByRole("textbox",{name:"New branch (optional)"}),{target:{value:"upgrade-pretty-bytes-v7"}});
 expect(screen.queryByRole("combobox",{name:"Chat worktree"})).toBeNull();
 fireEvent.click(existing);
 expect(screen.queryByRole("textbox",{name:"New branch (optional)"})).toBeNull();
 fireEvent.click(screen.getByRole("button",{name:"Set up a new AWT worktree"}));
 expect((screen.getByRole("textbox",{name:"New branch (optional)"}) as HTMLInputElement).value).toBe("upgrade-pretty-bytes-v7");
 expect(api.startWorkspaceSetup).not.toHaveBeenCalled();
 fireEvent.click(screen.getByRole("button",{name:"Close checkout"}));
 expect(screen.queryByRole("region",{name:"Chat checkout"})).toBeNull();
});

it("opens checkout as a modal dialog and returns focus through its icon close button",async()=>{
 const input=props();render(<ChatWorkspace {...input}/>);
 const trigger=screen.getByRole("button",{name:"Choose checkout"});trigger.focus();fireEvent.click(trigger);
 const dialog=await screen.findByRole("dialog",{name:"Chat checkout"});
 expect(dialog.getAttribute("aria-modal")).toBe("true");
 const close=await screen.findByRole("button",{name:"Close checkout"});expect(close.textContent).toBe("×");
 fireEvent.click(close);expect(screen.queryByRole("dialog",{name:"Chat checkout"})).toBeNull();expect(document.activeElement).toBe(trigger);
 expect(api.startWorkspaceSetup).not.toHaveBeenCalled();expect(api.approveWorkspace).not.toHaveBeenCalled();
});

it("selects a new chat checkout without dispatching or creating a request",async()=>{
 const input=props();render(<ChatWorkspace {...input}/>);
 fireEvent.click(screen.getByRole("button",{name:"Choose checkout"}));
 await screen.findByRole("option",{name:"3 · feature"});
 fireEvent.change(screen.getByRole("combobox",{name:"Chat worktree"}),{target:{value:"/linked"}});
 fireEvent.click(screen.getByRole("button",{name:"Use this checkout"}));
 expect(input.onSelect).toHaveBeenCalledWith("/linked");expect(api.requestWorkspace).not.toHaveBeenCalled();
});
it("shows ordinary processes without blocking selection and explains real lease refusals",async()=>{
 api.chatWorkspaces.mockResolvedValue([{path:"/linked",name:"3",branch:"feature",unavailable:null,processes:[{pid:42,name:"zsh"}]},{path:"/held",name:"1",branch:"protected",unavailable:"orphaned AWT lease"}]);
 const input=props();render(<ChatWorkspace {...input}/>);fireEvent.click(screen.getByRole("button",{name:"Choose checkout"}));
 const option=await screen.findByRole("option",{name:"3 · feature"});expect((option as HTMLOptionElement).disabled).toBe(false);
 expect(screen.getByText(/Processes detected: zsh/)).toBeTruthy();
 fireEvent.click(screen.getByText("Why some checkouts are unavailable"));expect(screen.getByText(/orphaned AWT lease/)).toBeTruthy();
});
it("requires explicit AWT consent, waits for setup, then selects its ready checkout without a turn",async()=>{
 const running={id:5,project_id:2,request_id:"",repo_path:"/repo",branch:"feature/new",workspace_path:null,state:"running",detail:"Preparing dependencies",rev:2,steps:[]};
 api.startWorkspaceSetup.mockImplementation(async(_project:string,key:string)=>{const receipt={...running,request_id:key};api.workspaceSetups.mockResolvedValue([receipt]);return receipt;});
 const input=props();const view=render(<ChatWorkspace {...input}/>);fireEvent.click(screen.getByRole("button",{name:"Choose checkout"}));
 fireEvent.click(screen.getByText("Set up a new AWT worktree"));
 const start=await screen.findByRole("button",{name:"Acquire and set up worktree"});expect((start as HTMLButtonElement).disabled).toBe(true);
 fireEvent.change(screen.getByLabelText("New branch (optional)"),{target:{value:"feature/new"}});
 fireEvent.click(screen.getByRole("checkbox"));await waitFor(()=>expect((start as HTMLButtonElement).disabled).toBe(false));fireEvent.click(start);
 await screen.findByText("Preparing dependencies");expect(api.startWorkspaceSetup).toHaveBeenCalledWith("demo",expect.any(String),"feature/new");expect(input.onSelect).not.toHaveBeenCalled();expect(input.onPending).toHaveBeenLastCalledWith(true);
 api.workspaceSetups.mockResolvedValue([{...running,state:"ready",workspace_path:"/created",rev:3,detail:"Ready"}]);view.rerender(<ChatWorkspace {...input} tick={1}/>);
 fireEvent.click(await screen.findByRole("button",{name:"Use ready checkout 5"}));expect(input.onSelect).toHaveBeenCalledWith("/created");expect(api.requestWorkspace).not.toHaveBeenCalled();
});
it("retains an uncertain setup request across closing the popover instead of acquiring again",async()=>{
 api.startWorkspaceSetup.mockRejectedValue(new Error("response lost"));const input=props();render(<ChatWorkspace {...input}/>);
 fireEvent.click(screen.getByRole("button",{name:"Choose checkout"}));fireEvent.click(screen.getByText("Set up a new AWT worktree"));
 fireEvent.change(screen.getByLabelText("New branch (optional)"),{target:{value:"feature/pinned"}});fireEvent.click(screen.getByRole("checkbox"));
 await waitFor(()=>expect((screen.getByRole("button",{name:"Acquire and set up worktree"}) as HTMLButtonElement).disabled).toBe(false));fireEvent.click(screen.getByRole("button",{name:"Acquire and set up worktree"}));
 await screen.findByText("response lost");const first=api.startWorkspaceSetup.mock.calls[0];
 fireEvent.click(screen.getByRole("button",{name:"Choose checkout"}));fireEvent.click(screen.getByRole("button",{name:"Choose checkout"}));fireEvent.click(screen.getByText("Set up a new AWT worktree"));
 expect((screen.getByLabelText("New branch (optional)") as HTMLInputElement).value).toBe("feature/pinned");
 fireEvent.click(screen.getByRole("button",{name:"Retry the same setup request"}));await waitFor(()=>expect(api.startWorkspaceSetup).toHaveBeenCalledTimes(2));expect(api.startWorkspaceSetup.mock.calls[1]).toEqual(first);expect(input.onPending).toHaveBeenLastCalledWith(true);
});
function DraftComposer({ input }: { input: ReturnType<typeof props> }) {
 const [blocked, setBlocked] = useState(false);
 return <><textarea aria-label="Unsent message" defaultValue="Clone the plan and pick up the next deferred task"/><ChatWorkspace {...input} onPending={setBlocked}/><button disabled={blocked}>Send fixture</button></>;
}

it("unblocks the composer after a recorded refusal and requires a new request to try again",async()=>{
 const refused={id:5,project_id:2,request_id:"",repo_path:"/repo",branch:"feature/pinned",workspace_path:null,state:"failed",attempted:false,detail:"Setup not started: wait for this project's execution",rev:1,steps:[]};
 api.startWorkspaceSetup.mockImplementation(async(_project:string,key:string)=>{const receipt={...refused,request_id:key};api.workspaceSetups.mockResolvedValue([receipt]);return receipt;});
 render(<DraftComposer input={props()}/>);
 fireEvent.click(screen.getByRole("button",{name:"Choose checkout"}));fireEvent.click(await screen.findByText("Set up a new AWT worktree"));
 fireEvent.change(screen.getByLabelText("New branch (optional)"),{target:{value:"feature/pinned"}});fireEvent.click(screen.getByRole("checkbox"));
 const start=screen.getByRole("button",{name:"Acquire and set up worktree"});await waitFor(()=>expect(start).toHaveProperty("disabled",false));fireEvent.click(start);
 await screen.findByText("Setup 5 · not started");
 await waitFor(()=>expect(screen.getByRole("button",{name:"Send fixture"})).toHaveProperty("disabled",false));
 expect(screen.getByRole("textbox",{name:"Unsent message"})).toHaveProperty("value","Clone the plan and pick up the next deferred task");
 expect(screen.queryByText("Worktree setup in progress")).toBeNull();
 expect(screen.queryByText(/The response is uncertain/)).toBeNull();
 expect(api.startWorkspaceSetup).toHaveBeenCalledTimes(1);
 expect(api.commandWorkspaceSetup).not.toHaveBeenCalled();
 fireEvent.click(start);await waitFor(()=>expect(api.startWorkspaceSetup).toHaveBeenCalledTimes(2));
 expect(api.startWorkspaceSetup.mock.calls[1]?.[1]).not.toBe(api.startWorkspaceSetup.mock.calls[0]?.[1]);
 expect(api.startWorkspaceSetup.mock.calls[1]?.[2]).toBe("feature/pinned");
});

it("reconciles a lost refusal response without replaying acquisition or discarding the message",async()=>{
 api.startWorkspaceSetup.mockRejectedValue(new Error("response lost"));
 const input=props();const view=render(<DraftComposer input={input}/>);
 fireEvent.click(screen.getByRole("button",{name:"Choose checkout"}));fireEvent.click(await screen.findByText("Set up a new AWT worktree"));
 fireEvent.change(screen.getByLabelText("New branch (optional)"),{target:{value:"feature/kept"}});fireEvent.click(screen.getByRole("checkbox"));
 const start=screen.getByRole("button",{name:"Acquire and set up worktree"});await waitFor(()=>expect(start).toHaveProperty("disabled",false));fireEvent.click(start);
 await screen.findByText("response lost");expect(screen.getByRole("button",{name:"Send fixture"})).toHaveProperty("disabled",true);
 const key=api.startWorkspaceSetup.mock.calls[0]?.[1];
 api.workspaceSetups.mockResolvedValue([{id:5,project_id:2,request_id:key,repo_path:"/repo",branch:"feature/kept",workspace_path:null,state:"failed",attempted:false,detail:"Setup not started: another turn was active",rev:1,steps:[]}]);
 view.rerender(<DraftComposer input={{...input,tick:1}}/>);
 await screen.findByText("Setup 5 · not started");
 await waitFor(()=>expect(screen.getByRole("button",{name:"Send fixture"})).toHaveProperty("disabled",false));
 expect(screen.getByRole("textbox",{name:"Unsent message"})).toHaveProperty("value","Clone the plan and pick up the next deferred task");
 expect(api.startWorkspaceSetup).toHaveBeenCalledTimes(1);
 expect(api.commandWorkspaceSetup).not.toHaveBeenCalled();
});

it("exposes project setup inspection from existing chats without offering another acquisition",async()=>{
 const blocked={id:5,project_id:2,request_id:"blocked",repo_path:"/repo",branch:"",workspace_path:null,state:"inspection",attempted:true,detail:"Protected pool slot; acquisition was refused",rev:4,steps:[]};
 api.workspaceSetups.mockResolvedValue([blocked]);
 api.commandWorkspaceSetup.mockImplementation(async()=>{const result={...blocked,state:"failed",rev:6,detail:"Inspection completed; no acquisition replayed"};api.workspaceSetups.mockResolvedValue([result]);return result;});
 const input={...props(),detail};render(<DraftComposer input={input}/>);
 await screen.findByText("Worktree setup needs inspection");
 await waitFor(()=>expect(screen.getByRole("button",{name:"Send fixture"})).toHaveProperty("disabled",true));
 fireEvent.click(screen.getByRole("button",{name:"Choose checkout"}));
 const inspect=await screen.findByRole("button",{name:"Inspect stopped setup 5"});
 expect(screen.queryByRole("button",{name:"Acquire and set up worktree"})).toBeNull();
 fireEvent.click(inspect);
 await screen.findByText("Inspection completed; no acquisition replayed");
 await waitFor(()=>expect(screen.getByRole("button",{name:"Send fixture"})).toHaveProperty("disabled",false));
 expect(api.commandWorkspaceSetup).toHaveBeenCalledWith("demo",blocked,"inspect");
 expect(api.startWorkspaceSetup).not.toHaveBeenCalled();
 expect(screen.getByRole("textbox",{name:"Unsent message"})).toHaveProperty("value","Clone the plan and pick up the next deferred task");
});

it("keeps checkout approval separate from inspection while both requests are pending",async()=>{
 const request={id:8,chat_id:1,after_node_id:null,from_path:"/repo",to_path:"/work",state:"pending"};
 const blocked={id:5,project_id:2,request_id:"blocked",repo_path:"/repo",branch:"",workspace_path:null,state:"inspection",attempted:true,detail:"Inspect setup first",rev:4,steps:[]};
 api.workspaceRequests.mockResolvedValue({requests:[request]});api.workspaceSetups.mockResolvedValue([blocked]);
 api.commandWorkspaceSetup.mockImplementation(async()=>{const result={...blocked,state:"failed",rev:6};api.workspaceSetups.mockResolvedValue([result]);return result;});
 render(<DraftComposer input={{...props(),detail}}/>);
 await screen.findByText("Worktree setup needs inspection");
 const inspect=await screen.findByRole("button",{name:"Inspect stopped setup 5"});
 expect(screen.getByRole("button",{name:"Approve checkout switch"})).toHaveProperty("disabled",true);
 fireEvent.click(inspect);
 await waitFor(()=>expect(screen.getByRole("button",{name:"Approve checkout switch"})).toHaveProperty("disabled",false));
 expect(screen.getByRole("button",{name:"Send fixture"})).toHaveProperty("disabled",true);
 expect(api.approveWorkspace).not.toHaveBeenCalled();expect(api.startWorkspaceSetup).not.toHaveBeenCalled();
});

it("proposes an existing chat switch without treating it as approval",async()=>{
 api.requestWorkspace.mockImplementation(async()=>{api.workspaceRequests.mockResolvedValue({requests:[pending]});return pending;});
 render(<ChatWorkspace {...props()} detail={detail}/>);
 fireEvent.click(screen.getByRole("button",{name:"Choose checkout"}));await screen.findByRole("option",{name:"3 · feature"});
 fireEvent.change(screen.getByRole("combobox",{name:"Chat worktree"}),{target:{value:"/linked"}});
 fireEvent.click(screen.getByRole("button",{name:"Review checkout switch"}));
 await screen.findByRole("button",{name:"Approve checkout switch"});
 expect(api.requestWorkspace).toHaveBeenCalledWith(1,"/linked");expect(api.approveWorkspace).not.toHaveBeenCalled();
});
it("waits for the original turn, then approves an exact request and revision",async()=>{
 api.workspaceRequests.mockResolvedValue({requests:[pending]});const input=props();
 const view=render(<ChatWorkspace {...input} detail={{...detail,active_node_id:3,state:"running"}}/>);
 const approve=await screen.findByRole("button",{name:"Approve checkout switch"});expect((approve as HTMLButtonElement).disabled).toBe(true);
 expect(input.onPending).toHaveBeenCalledWith(true);fireEvent.click(approve);expect(api.approveWorkspace).not.toHaveBeenCalled();
 view.rerender(<ChatWorkspace {...input} detail={detail} tick={1}/>);
 fireEvent.click(screen.getByRole("button",{name:"Approve checkout switch"}));
 await waitFor(()=>expect(api.approveWorkspace).toHaveBeenCalledWith(1,9,7));
});
it("shows refused approval without resending the request or moving context",async()=>{
 api.workspaceRequests.mockResolvedValue({requests:[pending]});api.approveWorkspace.mockRejectedValue(new Error("another turn owns this checkout"));const input=props();
 render(<ChatWorkspace {...input} detail={detail}/>);
 fireEvent.click(await screen.findByRole("button",{name:"Approve checkout switch"}));
 expect((await screen.findByRole("alert")).textContent).toContain("another turn owns");expect(input.onChanged).not.toHaveBeenCalled();expect(api.requestWorkspace).not.toHaveBeenCalled();
});
