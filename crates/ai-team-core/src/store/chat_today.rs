use crate::{
    chat_today::{ChatToday, Entry},
    plan_library::PlanLibraryFilter,
    ChatTeamPhase, NodeStatus, Result, Store,
};
use rusqlite::OptionalExtension;

impl Store {
    pub fn chat_today(&self) -> Result<ChatToday> {
        let mut query=self.db().conn().prepare("SELECT c.id FROM chat c JOIN project p ON p.id=c.project_id WHERE c.archived=0 AND p.status!='archived' ORDER BY c.updated_at DESC,c.id DESC")?;
        let ids = query
            .query_map([], |r| r.get::<_, i64>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        let plans = self.plan_library(&PlanLibraryFilter::default())?;
        let mut entries = Vec::new();
        for id in ids {
            let mut entry = self.today_chat(id)?;
            entry.questions = plans
                .entries
                .iter()
                .find(|plan| plan.chat_id == id)
                .map_or(0, |plan| plan.open_questions);
            if entry.questions > 0 {
                entry.needs_attention = true;
                if matches!(entry.state.as_str(), "idle" | "empty") {
                    entry.state = "questions".into();
                    entry.panel = "board".into();
                }
            }
            entries.push(entry);
        }
        let result = ChatToday {
            chats: entries.len(),
            needs_attention: entries.iter().filter(|entry| entry.needs_attention).count(),
            working: entries.iter().filter(|entry| entry.working).count(),
            drafts: entries.iter().map(|entry| entry.drafts).sum(),
            entries: Vec::new(),
        };
        // Stable sort retains recency within each urgency tier. Do not resurface every
        // failed historical attempt when its chat has already moved on.
        entries.sort_by_key(|entry| (!entry.needs_attention, !entry.working));
        entries.truncate(200);
        Ok(ChatToday { entries, ..result })
    }
    fn today_chat(&self, id: i64) -> Result<Entry> {
        let chat = self.chat(id)?;
        let project = self.project(chat.project_id)?;
        let latest: Option<i64> = self
            .db()
            .conn()
            .query_row(
                "SELECT node_id FROM chat_turn WHERE chat_id=?1 ORDER BY run_id DESC LIMIT 1",
                [id],
                |r| r.get(0),
            )
            .optional()?;
        let mut entry = Entry {
            chat_id: id,
            project_slug: project.slug,
            project_name: project.name,
            title: chat.title.clone(),
            workspace_path: chat.workspace_path.clone(),
            updated_at: chat.updated_at.clone(),
            state: "empty".into(),
            detail: None,
            panel: "overview".into(),
            needs_attention: false,
            working: false,
            drafts: 0,
            questions: 0,
        };
        if let Some(node) = chat.active_node_id.or(latest) {
            let node = self.node_run(node)?;
            let team = self.chat_team_run(node.run_id)?;
            let active = chat.active_node_id == Some(node.id);
            entry.working = active
                && (chat.supervisor_alive(&node)
                    || chat.pi_alive(&node)
                    || team
                        .as_ref()
                        .is_some_and(crate::ChatTeamRun::supervisor_alive));
            entry.state = if active {
                entry.panel = "work".into();
                match team.as_ref().map(|team| team.phase) {
                    Some(ChatTeamPhase::AwaitingApproval) => "awaiting_approval",
                    Some(ChatTeamPhase::Blocked) => "blocked",
                    _ if entry.working && chat.stop_requested => "stopping",
                    _ if entry.working => "running",
                    _ if node.supervisor_pid.is_none() && node.pi_pid.is_none() => "starting",
                    _ => "interrupted",
                }
            } else {
                match node.status {
                    NodeStatus::Failed => "failed",
                    NodeStatus::Blocked | NodeStatus::Parked => "blocked",
                    NodeStatus::Cancelled => "stopped",
                    _ => "idle",
                }
            }
            .into();
            entry.needs_attention = matches!(
                entry.state.as_str(),
                "awaiting_approval" | "blocked" | "interrupted" | "failed"
            );
            entry.detail = team
                .and_then(|team| team.reason)
                .or(self.run(node.run_id)?.blocked_reason)
                .map(|text| text.chars().take(600).collect());
        }
        let (drafts,inspection,handoff,queued):(i64,bool,bool,bool)=self.db().conn().query_row(
            "SELECT (SELECT COUNT(*) FROM chat_build_slice s JOIN chat_team_run t ON t.run_id=s.run_id WHERE t.chat_id=?1 AND s.build_status='verified' AND s.commit_sha IS NOT NULL AND s.lease_state!='released'),
             EXISTS(SELECT 1 FROM chat_delivery WHERE chat_id=?1 AND state='inspection') OR EXISTS(SELECT 1 FROM chat_checkout_operation WHERE chat_id=?1 AND state='inspection'),
             EXISTS(SELECT 1 FROM chat_workspace_request WHERE chat_id=?1 AND state='pending'),
             EXISTS(SELECT 1 FROM chat_followup WHERE chat_id=?1 AND state IN ('queued','starting'))",[id],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?)))?;
        entry.drafts = drafts;
        entry.needs_attention |=
            inspection || handoff || drafts > 0 || (queued && chat.active_node_id.is_none());
        if inspection {
            entry.state = "inspection".into();
            entry.panel = "review".into();
        } else if handoff {
            entry.state = "checkout_change".into();
        } else if !entry.working
            && !matches!(
                entry.state.as_str(),
                "failed" | "blocked" | "interrupted" | "awaiting_approval"
            )
        {
            if drafts > 0 {
                entry.state = "review".into();
                entry.panel = "review".into();
            } else if queued {
                entry.state = "queued_followup".into();
                entry.panel = "work".into();
            }
        }
        Ok(entry)
    }
}
