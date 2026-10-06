import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, expect, it, vi } from "vitest";
import { ChatWorkspace } from "./ChatWorkspace";
import type { ChatDetail } from "./chat-api";

const api=vi.hoisted(()=>({chatWorkspaces:vi.fn(),workspaceRequests:vi.fn(),requestWorkspace:vi.fn(),approveWorkspace:vi.fn(),cancelWorkspace:vi.fn()}));
vi.mock("./chat-workspace-api",()=>api);
const detail:ChatDetail={id:1,project_id:2,title:"Chat",workspace_path:"/repo",provider:"local",model:"fixture",reasoning:"high",mode:"single",archived:false,active_node_id:null,stop_requested:false,rev:7,created_at:"",updated_at:"",live_text:"",turns:[],team_builds:[],followups:[],state:"idle",can_resume:false,orphan_running:false};
const pending={id:9,chat_id:1,after_node_id:3,from_path:"/repo",to_path:"/linked",state:"pending"};
const props=()=>({project:"demo",detail:null as ChatDetail|null,selected:null,tick:0,disabled:false,onSelect:vi.fn(),onChanged:vi.fn(),onPending:vi.fn()});
beforeEach(()=>{
 vi.clearAllMocks();
 api.chatWorkspaces.mockResolvedValue([{path:"/repo",name:"main",branch:"main",unavailable:null},{path:"/linked",name:"3",branch:"feature",unavailable:null},{path:"/busy",name:"4",branch:null,unavailable:"occupied"}]);
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
