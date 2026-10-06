import { fireEvent, render, screen, waitFor } from "@testing-library/react";
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
