import { Fragment, useCallback, useEffect, useRef, useState } from "react";
import { FileView } from "./ReviewFile";
import { useSynth } from "./ui-preferences";
import { DeliveryChain } from "./synth/views/Review/DeliveryChain";
import { ChangeScope } from "./synth/views/Review/ChangeScope";
import { ExactApproval } from "./synth/views/Review/ExactApproval";
import { useCheckoutFiles, type CheckoutFile } from "./synth/views/Review/checkout-files";
import { useReviewFix } from "./useReviewFix";
import { checkoutState, checkoutPreview, checkoutApprove, checkoutInspect, checkoutAcknowledge, type CheckoutAction, type CheckoutState, type CheckoutOperation, type CheckoutInspection } from "./checkout-api";
const labels={stage:"Stage file",unstage:"Unstage file",commit:"Manual commit",push:"Push checkout commit",pull_request:"Open checkout draft PR",push_branch:"Chat-approved branch push"};
type Props = {chatId:number;mode:"single"|"team";tick:number;disabled:boolean;working?:boolean;otherBusy?:boolean;disabledReason?:string;onChanged:()=>void;onFeedback?:(text:string)=>void};
export function ChatCheckout(props: Props) { return <CheckoutController key={props.chatId} {...props}/>; }
function CheckoutController({chatId,mode,tick,disabled,working=false,otherBusy=false,disabledReason,onChanged,onFeedback}: Props) {
  const synth=useSynth(), sequence=useRef(0), host=useRef<HTMLElement>(null);
  const [opened,setOpened]=useState(synth);
  const [untrackedOpen,setUntrackedOpen]=useState(!synth), [untrackedMounted,setUntrackedMounted]=useState(!synth);
  const [showAll,setShowAll]=useState(!synth), [expanded,setExpanded]=useState<number>();
  const [revealed,setRevealed]=useState(new Set<string>()), [reveal,setReveal]=useState<{path:string;sequence:number}|null>(null);
  useEffect(()=>{if(synth)setOpened(true);},[synth]);
  const submitFix=useReviewFix(chatId);
  const [submitted,setSubmitted]=useState(false);
  const [data,setData]=useState<CheckoutState|null>(null);
  const [preview,setPreview]=useState<CheckoutOperation|null>(null);
  const [inspection,setInspection]=useState<CheckoutInspection|null>(null);
  const [message,setMessage]=useState(""); const [reason,setReason]=useState("");
  const [problem,setProblem]=useState<string|null>(null); const [busy,setBusy]=useState(false);const [refresh,setRefresh]=useState(0);
  const files=useCheckoutFiles(data,chatId);
  const untrackedDraft=files.rows.some(({entry})=>entry.area==="untracked"&&files.state(entry)?.draft);
  const showUntracked=!synth||untrackedOpen||untrackedDraft;
  useEffect(()=>{if(showUntracked)setUntrackedMounted(true);},[showUntracked]);
  const inspectionTick = disabled ? 0 : tick;
  useEffect(()=>{let current=true;void checkoutState(chatId).then(d=>{if(current)setData(d);}).catch(e=>{if(current)setProblem(String(e));});return()=>{current=false;};},[chatId,inspectionTick,disabled,refresh]);
  const command=useCallback(async(work:()=>Promise<void>)=>{
    setBusy(true);setProblem(null);try{await work();return true;}catch(e){setProblem(e instanceof Error?e.message:String(e));return false;}finally{setBusy(false);setRefresh(n=>n+1);onChanged();}
  },[onChanged]);
  const lockReason=disabled ? disabledReason??"Finish the current execution before changing the index or publishing."
    : busy||otherBusy ? "An existing request is pending. No additional checkout action is available."
      : data?.operations.some(o=>o.state==="running"||o.state==="inspection") ? "A checkout operation is unresolved. Drain and inspect it before another action." : null;
  const blocked=lockReason!==null;
  const approvalCurrent = !!data && !!preview && preview.chat_id === chatId && preview.state === "preview"
    && preview.snapshot.workspace_epoch === data.workspace_epoch && preview.snapshot.workspace === data.workspace
    && preview.snapshot.head === data.head && preview.snapshot.branch === data.branch && preview.snapshot.fingerprint === data.fingerprint;
  const propose=(action:CheckoutAction)=>{if(data)void command(async()=>{setPreview(await checkoutPreview(chatId,data.fingerprint,action));setInspection(null);});};
  const reviewFile=(entry:CheckoutFile,present:boolean,index:number)=>{
    if(!data)return null;
    const {file,area}=entry, state=files.state(entry);
    const extra=area==="unstaged"&&index>=3;
    const hidden=synth&&present&&extra&&!showAll&&!revealed.has(entry.key)&&!state?.draft&&!state?.revealed;
    return <Fragment key={entry.key}>
      {synth&&area==="unstaged"&&index===3&&<div className="chat-controls"><span>{showAll?`All ${data.unstaged.length} modified files shown`:`${data.unstaged.length-3} more modified files`}</span>
        <button type="button" className="button" onClick={()=>{setShowAll(true);setExpanded(++sequence.current);}}>Expand all readable files</button></div>}
      <div hidden={hidden}>
        {!present&&<p className="notice">Retained unsent comment · {entry.workspace} · generation {entry.epoch}. This file is no longer in the current {area} diff; no new action is available for this captured entry.</p>}
        <FileView file={file} retryCapturedRequest commentTarget="agent" reason={entry.reason} revision={entry.revision} reviewKey={entry.reviewKey}
          commentScope={present?entry.commentScope:JSON.stringify(["not-current",data.fingerprint])} untracked={area==="untracked"}
          initiallyCollapsed={synth&&extra&&!showAll} expand={area==="unstaged"?expanded:undefined}
          reveal={area==="unstaged"&&reveal?.path===file.path?reveal.sequence:undefined}
          onState={state=>files.report(entry,state)} comments={entry.comments}
          onComment={blocked||!present||mode!=="single"?undefined:async(body,anchor)=>command(async()=>{await submitFix({kind:"checkout",finding:{id:0,fingerprint:entry.fingerprint,head:entry.head,area,path:anchor.file_path,side:anchor.side,line:anchor.line_start,body,created_at:""}},entry.epoch);setSubmitted(true);})}/>
        <button className="button" disabled={blocked||!present} onClick={()=>propose({kind:area==="staged"?"unstage":"stage",path:file.path})}>{area==="staged"?"Unstage":"Stage"} {file.path}</button>
      </div>
    </Fragment>;
  };
  return <section ref={host} tabIndex={-1} aria-label="Working checkout actions">
    {problem&&<p className="error" role="alert">{problem}</p>}
    {!data?<p>Reading working checkout…</p>:<>
      <p className="mono" hidden={synth}>{data.workspace} · {data.branch??"detached"} · {data.head??"unborn HEAD"}</p>
      <div hidden={!synth}>{(synth||opened)&&<>
        <DeliveryChain data={data} locked={lockReason} working={working} commitMessage={message}/>
        <ChangeScope files={data.unstaged} viewed={files.viewed} onSelect={path=>{
          setRevealed(current=>new Set([...current,JSON.stringify(["unstaged",path])]));setReveal({path,sequence:++sequence.current});
        }}/>
      </>}</div>
      <p className="faint">These are the checkout's actual files, not attributed to this conversation and not a verified team draft. Each action previews exact state and refuses stale approval. Editor and terminal remain explicit operator tools, not an OS sandbox.</p>
      {lockReason&&<p className="notice" hidden={synth}>{lockReason}</p>}
      <p className="faint">{mode==="single"?"Submitting an inline comment sends it to the solo agent to address in this checkout. No commit or publication is approved.":"Working-checkout repair comments require solo mode. In team mode, review the exact verified draft to send a repair to its maker. No mode change or publication is automatic."}</p>
      {submitted&&<p role="status">Review submitted to the agent. Follow its recorded turn in this chat.</p>}
      {(["staged","unstaged"] as const).map(area=><section key={area} className="synth-review-files" aria-label={`${area} checkout files`}><h4>{area==="staged"?"Staged — the next commit's contents":"Unstaged"}{synth?` · ${data[area].length}`:null}</h4>
        {!data[area].length&&<p className="faint synth-review-empty">{synth?"None in the current snapshot.":"None."}</p>}
        {files.rows.filter(row=>row.entry.area===area).map(({entry,present},index)=>reviewFile(entry,present,index))}
      </section>)}
      <section className="synth-review-files" aria-label="untracked checkout files"><h4>Untracked{synth?` · ${data.untracked.length}`:null}</h4>
        {synth&&<button type="button" className="button" aria-expanded={showUntracked} disabled={showUntracked&&untrackedDraft}
          onClick={()=>setUntrackedOpen(!showUntracked)}>{showUntracked?"Hide":"Show"} untracked files</button>}
        {synth&&untrackedDraft&&<p className="faint">An open comment keeps this group visible.</p>}
        <div hidden={!showUntracked}><p className="faint">New text files can be reviewed and commented on without staging. Reading or marking viewed changes no file or index.</p>
          {!data.untracked.length&&<p className="faint synth-review-empty">None.</p>}
          {(untrackedMounted||showUntracked)&&files.rows.filter(row=>row.entry.area==="untracked").map(({entry,present},index)=>reviewFile(entry,present,index))}
        </div>
      </section>
      <form onSubmit={e=>{e.preventDefault();propose({kind:"commit",message});}}><label>Manual commit message<textarea value={message} onChange={e=>setMessage(e.target.value)} maxLength={16000}/></label><p className="faint">Commit the reviewed index only; unstaged/untracked files are kept. Hooks and signing are disabled for this bounded operator action. No verification, push or PR is implied.</p><button className="button" disabled={blocked||!data.staged.length||!message.trim()}>Preview manual commit</button></form>
      <div className="chat-controls"><button className="button" disabled={blocked||!data.head||!data.branch} onClick={()=>propose({kind:"push"})}>Preview checkout push</button><button className="button" disabled={blocked||!data.head||!data.branch} onClick={()=>propose({kind:"pull_request"})}>Preview checkout draft PR</button></div>
      {data.findings.length>0&&<details><summary>Recorded checkout feedback</summary><p className="faint">Historical lines may have changed. New inline submissions start an agent turn; older recorded-only feedback can still be copied to your draft. No publication is approved.</p>{data.findings.map(f=><div className="notice" key={f.id}><p className="mono">{f.path} · {f.area} · {f.side} line {f.line} · {f.head??"unborn"}</p><p>{f.body}</p>{onFeedback&&<button className="button" onClick={()=>onFeedback(`Checkout review at ${f.head??"unborn HEAD"} (${f.fingerprint}): ${f.path}, ${f.area}, ${f.side} line ${f.line}:\n${f.body}\n\nInspect the current file before acting; this feedback is not publication approval.`)}>Use feedback in next message</button>}</div>)}</details>}
      {preview&&<ExactApproval family="checkout" receipt={preview} label="Checkout action approval" returnFocus={host} summary={<>
        <h4>{labels[preview.snapshot.action.kind]} — exact approval</h4><p className="mono">{preview.snapshot.workspace} · {preview.snapshot.branch??"detached"} · {preview.snapshot.head??"unborn HEAD"}</p>
        <p className="mono">Checkout generation {preview.snapshot.workspace_epoch??"unrecorded"} · fingerprint {preview.snapshot.fingerprint}</p>
        {"path" in preview.snapshot.action&&<p className="mono">{preview.snapshot.action.path}</p>}{"message" in preview.snapshot.action&&<pre>{preview.snapshot.action.message}</pre>}
        {"commit" in preview.snapshot.action&&<p className="mono">Branch target: {preview.snapshot.action.branch} · {preview.snapshot.action.commit}</p>}
        {preview.snapshot.remote&&<><p className="mono">{preview.snapshot.remote.url} · {preview.snapshot.remote.branch}</p>{preview.snapshot.remote.base&&<p className="mono">PR base: {preview.snapshot.remote.repository} · {preview.snapshot.remote.base} · {preview.snapshot.remote.base_sha}</p>}<p>Only this pinned commit goes to the named remote ref. Dirty working files are excluded. No merge. Push and draft PR need separate approvals.</p></>}
      </>}>{ready=><>
        {lockReason&&<p className="notice">{lockReason}</p>}
        <button className="button button--primary" disabled={blocked||!approvalCurrent||!ready} onClick={()=>void command(async()=>{const r=await checkoutApprove(chatId,preview);setPreview(null);if(r.state!=="done")throw Error(r.result??"Inspect checkout outcome");if(preview.snapshot.action.kind==="commit"){const committed=preview.snapshot.action.message;setMessage(current=>current===committed?"":current);}})}>Approve: {labels[preview.snapshot.action.kind]}</button><button className="button" disabled={busy} onClick={()=>setPreview(null)}>Dismiss checkout approval</button>
        {!approvalCurrent&&<p>{preview.snapshot.workspace_epoch == null ? "Checkout generation was not recorded; request a fresh preview." : "This preview is stale. Changed since preview — review again."}</p>}
      </>}</ExactApproval>}
      {!!data.operations.length&&<details open={data.operations.some(o=>o.state==="running"||o.state==="inspection")}><summary>Checkout action history</summary>{data.operations.map(o=><div className="notice" key={o.id}><strong>{labels[o.snapshot.action.kind]} · {o.state}</strong><p>{o.result??"Command is running or interrupted. No automatic retry."}</p>{o.state==="done"&&o.snapshot.action.kind==="pull_request"&&o.result?.startsWith("https://")&&<a href={o.result} target="_blank" rel="noreferrer">Open draft PR</a>}{(o.state==="running"||o.state==="inspection")&&<button className="button" disabled={busy} onClick={()=>void command(async()=>{setInspection(await checkoutInspect(chatId,o));setReason("");})}>Drain and inspect checkout action #{o.id}</button>}</div>)}</details>}
      {inspection&&<form className="notice" onSubmit={e=>{e.preventDefault();void command(async()=>{await checkoutAcknowledge(chatId,inspection,reason);setInspection(null);});}}><h4>Keep inspected checkout state</h4><p>Local command groups drained. Inspect files, index, history and remote effects before acknowledging. A remote effect may still be delayed. This releases the reservation without certifying success, changing files or retrying the action.</p><p className="mono">{inspection.checkout.head} · {inspection.checkout.staged.length} staged · {inspection.checkout.unstaged.length} unstaged · {inspection.checkout.untracked.length} untracked</p><label>Reason for keeping the state<input value={reason} onChange={e=>setReason(e.target.value)} maxLength={4000}/></label><button className="button" disabled={busy||!reason.trim()}>Acknowledge checkout state</button></form>}
    </>}
  </section>;
}
