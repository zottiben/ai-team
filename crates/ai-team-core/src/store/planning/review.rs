//! Review admission shares the chat turn's transaction: no recorded-only or orphan queue.
use super::{engine, snapshot};
use crate::{
    chat_review::{Review, ReviewRequest, ReviewSubmission},
    ChatBuildSlice, ChatBuildStart, ChatSubmission, Error, ModelRegistry, Result, Store,
};
use rusqlite::{params, Connection, OptionalExtension};

pub(in crate::store) struct Prepared {
    pub input: ReviewRequest,
    pub json: String,
    pub source_head: Option<String>,
    pub parent: Option<ChatBuildSlice>,
    pub plan_revision: Option<i64>,
    pub slice_revision: Option<i64>,
    pub planning_path: std::path::PathBuf,
}
impl Prepared {
    pub(in crate::store) fn check_replay(&self, conn: &Connection, chat: i64) -> Result<()> {
        let recorded: Option<String> = conn
            .query_row(
                "SELECT input_json FROM chat_review_request WHERE chat_id=?1 AND request_id=?2",
                params![chat, self.input.request_id],
                |r| r.get(0),
            )
            .optional()?;
        if recorded.is_some_and(|json| json != self.json) {
            return Err(Error::invalid(
                "that review request id belongs to different feedback",
            ));
        }
        Ok(())
    }
}

impl Store {
    pub(crate) fn chat_review_for_node(
        &self,
        chat: i64,
        node: i64,
    ) -> Result<Option<ReviewRequest>> {
        let json: Option<String> = self
            .db()
            .conn()
            .query_row(
                "SELECT input_json FROM chat_review_request WHERE chat_id=?1 AND node_id=?2",
                params![chat, node],
                |r| r.get(0),
            )
            .optional()?;
        Ok(json.map(|json| serde_json::from_str(&json)).transpose()?)
    }
    pub(crate) fn chat_review_replay(
        &self,
        chat: i64,
        request: &str,
        json: &str,
    ) -> Result<Option<ReviewSubmission>> {
        let found = self.db().conn().query_row("SELECT input_json,run_id,node_id FROM chat_review_request WHERE chat_id=?1 AND request_id=?2", params![chat,request], |r| Ok((r.get::<_,String>(0)?,r.get(1)?,r.get(2)?))).optional()?;
        found
            .map(|(recorded, run_id, node_id)| {
                if recorded != json {
                    return Err(Error::invalid(
                        "that review request id belongs to different feedback",
                    ));
                }
                Ok(ReviewSubmission {
                    turn: ChatSubmission {
                        run_id,
                        node_id,
                        started: false,
                    },
                    build: None,
                })
            })
            .transpose()
    }
    pub(crate) fn begin_chat_review(
        &mut self,
        chat: i64,
        input: &ReviewRequest,
        json: &str,
        registry: &ModelRegistry,
    ) -> Result<ReviewSubmission> {
        let prompt = crate::chat_review::prompt(&input.review);
        if input.request_id.is_empty() || input.request_id.len() > 128 {
            return Err(Error::invalid("review feedback needs a bounded request id"));
        }
        let current = self.chat(chat)?;
        let mut prepared = Prepared {
            input: serde_json::from_str(json)?,
            json: json.into(),
            source_head: None,
            parent: None,
            plan_revision: None,
            slice_revision: None,
            planning_path: self.planning_path()?,
        };
        if let Review::Draft { target, .. } = &input.review {
            let (_, parent) = crate::chat_changes::owned_slice(self, chat, target)?;
            let agent = super::super::chat_teams::build::approved_agent(self.db().conn(), &parent)?;
            registry.resolve(&agent)?;
            let team = self
                .project(current.project_id)?
                .team_id
                .ok_or_else(|| Error::invalid("this project no longer has the approved team"))?;
            let verifier = self
                .agents(team)?
                .into_iter()
                .find(|a| a.role == crate::VERIFIER_ROLE && a.enabled && a.read_only)
                .ok_or_else(|| Error::invalid("a repair needs an enabled independent verifier"))?;
            registry.resolve(&verifier)?;
            let (head, dirty) =
                super::build::checkout(std::path::Path::new(&current.workspace_path))?;
            if !dirty.is_empty() {
                return Err(Error::invalid("the human checkout is dirty; preserve it and choose a clean checkout before a team repair"));
            }
            let plan = self.chat_plan(chat, crate::planning::PlanActor::Human)?;
            let slice = plan
                .bundle
                .as_ref()
                .and_then(|p| p.slices.iter().find(|s| s.id == parent.planner_slice_id))
                .ok_or_else(|| Error::invalid("this draft's owned slice no longer exists"))?;
            prepared.plan_revision = Some(plan.revision);
            prepared.slice_revision = Some(slice.rev);
            prepared.source_head = Some(head);
            prepared.parent = Some(parent);
        }
        let turn = self.begin_chat_review_turn(chat, &prompt, prepared, registry)?;
        let build = if turn.started && current.mode == crate::ChatMode::Team {
            Some(ChatBuildStart {
                chat_id: chat,
                node_id: turn.node_id,
                run_id: turn.run_id,
                revision: 2,
            })
        } else {
            None
        };
        Ok(ReviewSubmission { turn, build })
    }
    pub(crate) fn chat_build_source_head(&self, run: i64, base: &str) -> Result<String> {
        source_head(self.db().conn(), run, base)
    }
    pub(crate) fn chat_build_is_review(&self, run: i64) -> Result<bool> {
        is_review(self.db().conn(), run)
    }
}
pub(in crate::store) fn source_head(conn: &Connection, run: i64, base: &str) -> Result<String> {
    Ok(conn.query_row("SELECT source_head FROM chat_review_request WHERE run_id=?1 AND parent_run_id IS NOT NULL",[run],|r|r.get::<_,String>(0)).optional()?.unwrap_or_else(||base.into()))
}
pub(in crate::store) fn is_review(conn: &Connection, run: i64) -> Result<bool> {
    Ok(conn.query_row("SELECT EXISTS(SELECT 1 FROM chat_review_request WHERE run_id=?1 AND parent_run_id IS NOT NULL)",[run],|r|r.get(0))?)
}
pub(in crate::store) fn validate(
    conn: &Connection,
    chat: &crate::Chat,
    p: &Prepared,
) -> Result<()> {
    if p.input.workspace_epoch != chat.workspace_epoch {
        return Err(Error::invalid("the reviewed checkout changed"));
    }
    let Some(parent) = &p.parent else {
        return Ok(());
    };
    let fresh = super::super::chat_teams::build::read(conn, parent.run_id, &parent.slice_key)?;
    if fresh.rev != parent.rev
        || fresh.build_status != "verified"
        || fresh.lease_state != "released"
        || fresh.commit_sha.is_none()
    {
        return Err(Error::invalid(
            "this draft changed or still holds a lease; inspect it before repairing",
        ));
    }
    let newer:bool=conn.query_row("SELECT EXISTS(SELECT 1 FROM chat_build_slice s JOIN chat_team_run t ON t.run_id=s.run_id WHERE t.chat_id=?1 AND s.planner_slice_id=?2 AND s.run_id>?3)",params![chat.id,parent.planner_slice_id,parent.run_id],|r|r.get(0))?;
    if newer {
        return Err(Error::invalid(
            "a newer attempt exists for this slice; review or resume that attempt instead",
        ));
    }
    let team: Option<i64> = conn.query_row(
        "SELECT team_id FROM run WHERE id=?1",
        [parent.run_id],
        |r| r.get(0),
    )?;
    let current: Option<i64> = conn.query_row(
        "SELECT team_id FROM project WHERE id=?1",
        [chat.project_id],
        |r| r.get(0),
    )?;
    if team != current {
        return Err(Error::invalid(
            "the original team changed; this review cannot approve a replacement team",
        ));
    }
    super::super::chat_teams::build::approved_agent(conn, parent)?;
    let store = engine::open(&p.planning_path, false)?
        .ok_or_else(|| Error::invalid("the owned plan is missing"))?;
    let plan = snapshot(&store, chat)?;
    let slice = store.slice_by_id(parent.planner_slice_id)?;
    let hash: Option<String> = conn.query_row(
        "SELECT scope_hash FROM chat_build_slice WHERE run_id=?1 AND slice_key=?2",
        params![parent.run_id, parent.slice_key],
        |r| r.get(0),
    )?;
    if Some(plan.revision) != p.plan_revision
        || Some(slice.rev) != p.slice_revision
        || hash.as_deref() != Some(crate::chat_review::scope_hash(&slice).as_str())
        || slice.branch != parent.branch
        || slice.status != ai_planner_core::Status::InReview
        || slice.claimed_by.is_some()
    {
        return Err(Error::invalid("the original slice scope/claim changed or has no scope receipt; review a new build rather than silently repairing different work"));
    }
    let (head, dirty) = super::build::checkout(std::path::Path::new(&chat.workspace_path))?;
    if Some(&head) != p.source_head.as_ref() || !dirty.is_empty() {
        return Err(Error::invalid(
            "the human checkout changed during review admission",
        ));
    }
    Ok(())
}
pub(in crate::store) fn record(
    conn: &Connection,
    chat: i64,
    run: i64,
    node: i64,
    p: &Prepared,
) -> Result<()> {
    conn.execute("INSERT INTO chat_review_request(chat_id,request_id,input_json,run_id,node_id,workspace_epoch,source_head,parent_run_id,slice_key,created_at) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)",params![chat,p.input.request_id,p.json,run,node,p.input.workspace_epoch,p.source_head,p.parent.as_ref().map(|s|s.run_id),p.parent.as_ref().map(|s|&s.slice_key),crate::now()])?;
    match &p.input.review {
        Review::Checkout { finding: f } => {
            conn.execute("INSERT INTO chat_checkout_finding(chat_id,fingerprint,head,area,path,side,line,body,created_at) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9)",params![chat,f.fingerprint,f.head,f.area,f.path,f.side,f.line,f.body,crate::now()])?;
        }
        Review::Draft {
            target,
            body,
            anchor,
        } => {
            let commit = p
                .parent
                .as_ref()
                .and_then(|parent| parent.commit_sha.as_deref());
            let payload = serde_json::json!({"chat_draft_review":commit,"slice_key":target.slice_key,"body":body,"anchor":anchor,"repair_run_id":run});
            conn.execute("INSERT INTO event(run_id,at,kind,actor,summary,payload_json) VALUES(?1,?2,'note','you',?3,?4)",params![target.run_id,crate::now(),format!("Review · {}: {body} — submitted as repair run #{run}",target.slice_key),payload.to_string()])?;
        }
    }
    if let Some(parent) = &p.parent {
        conn.execute("INSERT INTO chat_build_slice(run_id,slice_key,planner_slice_id,approved_rev,assigned_agent_id,assigned_agent_rev,agent_snapshot,branch,lease_holder,scope_hash) SELECT ?1,slice_key,planner_slice_id,?4,assigned_agent_id,assigned_agent_rev,agent_snapshot,branch,?5,scope_hash FROM chat_build_slice WHERE run_id=?2 AND slice_key=?3",params![run,parent.run_id,parent.slice_key,p.slice_revision,format!("ai-team chat-{chat} run-{run} slice-{}",parent.planner_slice_id)])?;
        conn.execute("UPDATE chat_team_run SET phase='building',base_sha=?2,approved_revision=?3,supervisor_pid=NULL,supervisor_identity=NULL,rev=rev+1 WHERE run_id=?1",params![run,parent.commit_sha,p.plan_revision])?;
        conn.execute(
            "UPDATE node_run SET status='done',supervisor_pid=NULL,ended_at=?2 WHERE id=?1",
            params![node, crate::now()],
        )?;
    }
    Ok(())
}
