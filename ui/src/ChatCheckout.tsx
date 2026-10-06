import { useCallback, useEffect, useState } from "react";
import { FileView } from "./Review";
import { checkoutState, checkoutPreview, checkoutApprove, checkoutFinding, checkoutInspect, checkoutAcknowledge, type CheckoutAction, type CheckoutState, type CheckoutOperation, type CheckoutInspection } from "./checkout-api";
const labels={stage:"Stage file",unstage:"Unstage file",commit:"Manual commit",push:"Push checkout commit",pull_request:"Open checkout draft PR"};
export function ChatCheckout({chatId,tick,disabled,onChanged,onFeedback}:{chatId:number;tick:number;disabled:boolean;onChanged:()=>void;onFeedback?:(text:string)=>void}) {
  const [data,setData]=useState<CheckoutState|null>(null);
  const [preview,setPreview]=useState<CheckoutOperation|null>(null);
  const [inspection,setInspection]=useState<CheckoutInspection|null>(null);
  const [message,setMessage]=useState(""); const [reason,setReason]=useState("");
  const [problem,setProblem]=useState<string|null>(null); const [busy,setBusy]=useState(false);const [refresh,setRefresh]=useState(0);
  const inspectionTick = disabled ? 0 : tick;
  useEffect(()=>{let current=true;void checkoutState(chatId).then(d=>{if(current)setData(d);}).catch(e=>{if(current)setProblem(String(e));});return()=>{current=false;};},[chatId,inspectionTick,disabled,refresh]);
  const command=useCallback(async(work:()=>Promise<void>)=>{
    setBusy(true);setProblem(null);try{await work();return true;}catch(e){setProblem(e instanceof Error?e.message:String(e));return false;}finally{setBusy(false);setRefresh(n=>n+1);onChanged();}
  },[onChanged]);
  const blocked=disabled||busy||data?.operations.some(o=>o.state==="running"||o.state==="inspection");
  const propose=(action:CheckoutAction)=>{if(data)void command(async()=>{setPreview(await checkoutPreview(chatId,data.fingerprint,action));setInspection(null);});};
  return <section aria-label="Working checkout actions">
    {problem&&<p className="error" role="alert">{problem}</p>}
    {!data?<p>Reading working checkout…</p>:<>
      <p className="mono">{data.workspace} · {data.branch??"detached"} · {data.head??"unborn HEAD"}</p>
      <p className="faint">These are the checkout's actual files, not attributed to this conversation and not a verified team draft. Each action previews exact state and refuses stale approval. Editor and terminal remain explicit operator tools, not an OS sandbox.</p>
      {disabled&&<p className="notice">Finish the current execution before changing the index or publishing.</p>}
      {(["staged","unstaged"] as const).map(area=><section key={area} aria-label={`${area} checkout files`}><h4>{area==="staged"?"Staged — the next commit's contents":"Unstaged"}</h4>
        {!data[area].length&&<p className="faint">None.</p>}
        {data[area].map(file=><div key={file.path}>
          <FileView file={file} comments={data.findings.filter(f=>f.fingerprint===data.fingerprint&&f.area===area).map(f=>({id:f.id,review_id:0,parent_id:null,file_path:f.path,side:f.side,line_start:f.line,line_end:f.line,author:"you",body:f.body,status:"open",created_at:f.created_at}))}
            onComment={blocked?undefined:async(body,anchor)=>command(async()=>{await checkoutFinding(chatId,{id:0,fingerprint:data.fingerprint,head:data.head,area,path:anchor.file_path,side:anchor.side,line:anchor.line_start,body,created_at:""});})}/>
          <button className="button" disabled={blocked} onClick={()=>propose({kind:area==="staged"?"unstage":"stage",path:file.path})}>{area==="staged"?"Unstage":"Stage"} {file.path}</button>
        </div>)}
      </section>)}
      <h4>Untracked</h4><p className="faint">Inspect new files in Editor before staging. Staging does not commit or publish them.</p>
      {data.untracked.map(path=><div className="card__row" key={path}><span className="mono">{path}</span><button className="button" disabled={blocked} onClick={()=>propose({kind:"stage",path})}>Stage {path}</button></div>)}
      <form onSubmit={e=>{e.preventDefault();propose({kind:"commit",message});}}><label>Manual commit message<textarea value={message} onChange={e=>setMessage(e.target.value)} maxLength={16000}/></label><p className="faint">Commit the reviewed index only; unstaged/untracked files are kept. Hooks and signing are disabled for this bounded operator action. No verification, push or PR is implied.</p><button className="button" disabled={blocked||!data.staged.length||!message.trim()}>Preview manual commit</button></form>
      <div className="chat-controls"><button className="button" disabled={blocked||!data.head||!data.branch} onClick={()=>propose({kind:"push"})}>Preview checkout push</button><button className="button" disabled={blocked||!data.head||!data.branch} onClick={()=>propose({kind:"pull_request"})}>Preview checkout draft PR</button></div>
      {data.findings.length>0&&<details><summary>Recorded checkout feedback</summary><p className="faint">Historical lines may have changed. Recording feedback does not send it to an agent or approve work.</p>{data.findings.map(f=><div className="notice" key={f.id}><p className="mono">{f.path} · {f.area} · {f.side} line {f.line} · {f.head??"unborn"}</p><p>{f.body}</p>{onFeedback&&<button className="button" onClick={()=>onFeedback(`Checkout review at ${f.head??"unborn HEAD"} (${f.fingerprint}): ${f.path}, ${f.area}, ${f.side} line ${f.line}:\n${f.body}\n\nInspect the current file before acting; this feedback is not publication approval.`)}>Use feedback in next message</button>}</div>)}</details>}
      {preview&&<section className="notice" aria-label="Checkout action approval"><h4>{labels[preview.snapshot.action.kind]} — exact approval</h4><p className="mono">{preview.snapshot.workspace} · {preview.snapshot.branch??"detached"} · {preview.snapshot.head??"unborn HEAD"}</p>
        {"path" in preview.snapshot.action&&<p className="mono">{preview.snapshot.action.path}</p>}{"message" in preview.snapshot.action&&<pre>{preview.snapshot.action.message}</pre>}
        {preview.snapshot.remote&&<><p className="mono">{preview.snapshot.remote.url} · {preview.snapshot.remote.branch}</p>{preview.snapshot.remote.base&&<p className="mono">PR base: {preview.snapshot.remote.repository} · {preview.snapshot.remote.base} · {preview.snapshot.remote.base_sha}</p>}<p>Only this committed snapshot is published, never dirty files or your current branch ref. No merge. Push and draft PR need separate approvals.</p></>}
        <button className="button button--primary" disabled={blocked||data.fingerprint!==preview.snapshot.fingerprint} onClick={()=>void command(async()=>{const r=await checkoutApprove(chatId,preview);setPreview(null);if(r.state!=="done")throw Error(r.result??"Inspect checkout outcome");if(preview.snapshot.action.kind==="commit")setMessage("");})}>Approve: {labels[preview.snapshot.action.kind]}</button><button className="button" disabled={busy} onClick={()=>setPreview(null)}>Dismiss checkout approval</button>
        {data.fingerprint!==preview.snapshot.fingerprint&&<p>This preview is stale. Refresh and preview again.</p>}
      </section>}
      {!!data.operations.length&&<details open={data.operations.some(o=>o.state==="running"||o.state==="inspection")}><summary>Checkout action history</summary>{data.operations.map(o=><div className="notice" key={o.id}><strong>{labels[o.snapshot.action.kind]} · {o.state}</strong><p>{o.result??"Command is running or interrupted. No automatic retry."}</p>{o.state==="done"&&o.snapshot.action.kind==="pull_request"&&o.result?.startsWith("https://")&&<a href={o.result} target="_blank" rel="noreferrer">Open draft PR</a>}{(o.state==="running"||o.state==="inspection")&&<button className="button" disabled={busy} onClick={()=>void command(async()=>{setInspection(await checkoutInspect(chatId,o));setReason("");})}>Drain and inspect checkout action #{o.id}</button>}</div>)}</details>}
      {inspection&&<form className="notice" onSubmit={e=>{e.preventDefault();void command(async()=>{await checkoutAcknowledge(chatId,inspection,reason);setInspection(null);});}}><h4>Keep inspected checkout state</h4><p>Local command groups drained. Inspect files, index, history and remote effects before acknowledging. A remote effect may still be delayed. This releases the reservation without certifying success, changing files or retrying the action.</p><p className="mono">{inspection.checkout.head} · {inspection.checkout.staged.length} staged · {inspection.checkout.unstaged.length} unstaged · {inspection.checkout.untracked.length} untracked</p><label>Reason for keeping the state<input value={reason} onChange={e=>setReason(e.target.value)} maxLength={4000}/></label><button className="button" disabled={busy||!reason.trim()}>Acknowledge checkout state</button></form>}
    </>}
  </section>;
}
