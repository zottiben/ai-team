use rusqlite::params;

use crate::{Chat, Error, ModelChoice, ModelRegistry, Reasoning, Result, Store};

impl Store {
    /// Explicit future-turn preference. The active node and queued snapshots keep
    /// their model, session and reasoning; selecting never stops or starts a child.
    pub fn set_chat_model(
        &mut self,
        id: i64,
        choice: &ModelChoice,
        expect_revision: i64,
        registry: &ModelRegistry,
    ) -> Result<Chat> {
        if choice.model.trim().is_empty() || !registry.profile().allowed(choice.provider) {
            return Err(Error::invalid(
                "that model's provider is not allowed on this machine",
            ));
        }
        self.db_mut().write(|tx| {
            if tx.execute("UPDATE chat SET provider=?2,model=?3,
                reasoning=CASE WHEN ?4 THEN reasoning ELSE ?5 END,rev=rev+1,updated_at=?6
                WHERE id=?1 AND rev=?7 AND mode='single' AND archived=0
                  AND EXISTS(SELECT 1 FROM project p WHERE p.id=chat.project_id AND p.status!='archived')
                  AND NOT EXISTS(SELECT 1 FROM chat_followup WHERE chat_id=?1 AND state='queued' AND model_settings_json IS NULL)",
                params![id,choice.provider,choice.model,choice.thinking,Reasoning::None,crate::now(),expect_revision])? != 1 {
                return Err(Error::invalid("refresh before changing this solo model; archived chats and team seats are not editable here. Send or cancel historical queued instructions whose model was not recorded first"));
            }
            Ok(())
        })?;
        self.chat(id)
    }
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct QueuedModel {
    pub provider: crate::Provider,
    pub model: String,
    pub reasoning: Reasoning,
}
