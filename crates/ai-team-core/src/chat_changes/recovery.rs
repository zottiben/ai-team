//! Human inspection releases ownership only after journalled children are drained.
//! Acknowledgement keeps files and remote state; it is not a successful delivery verdict.
use std::path::Path;

use serde::{Deserialize, Serialize};

use super::{delivery::observe, git, Delivery};
use crate::{chat::team::children, Error, Result, Store};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct CheckoutEvidence {
    pub head: String,
    pub branch: Option<String>,
    pub status: String,
}
#[derive(Debug, Serialize)]
pub struct DeliveryInspection {
    pub delivery: Delivery,
    /// Present only after every old child and these metadata reads are drained.
    pub checkout: Option<CheckoutEvidence>,
}
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeliveryAcknowledgement {
    pub delivery_id: i64,
    pub expect_revision: i64,
    pub checkout: CheckoutEvidence,
    pub reason: String,
}

pub async fn inspect(
    store: &mut Store,
    chat: i64,
    id: i64,
    revision: i64,
) -> Result<DeliveryInspection> {
    recover(store, chat, id, revision, None).await
}
pub async fn acknowledge(
    store: &mut Store,
    chat: i64,
    approval: &DeliveryAcknowledgement,
) -> Result<DeliveryInspection> {
    if approval.reason.trim().is_empty() || approval.reason.len() > 4000 {
        return Err(Error::invalid(
            "acknowledgement needs a reason of 1–4000 bytes",
        ));
    }
    recover(
        store,
        chat,
        approval.delivery_id,
        approval.expect_revision,
        Some(approval),
    )
    .await
}
async fn recover(
    store: &mut Store,
    chat: i64,
    id: i64,
    revision: i64,
    approval: Option<&DeliveryAcknowledgement>,
) -> Result<DeliveryInspection> {
    let delivery = store.chat_delivery(chat, id)?;
    let (owner, attempted) = store.reclaim_chat_delivery(chat, id, revision)?;
    owner.track(async {
        let result = async {
            for child in store.chat_children(delivery.snapshot.target.run_id)? {
                if child.state != "drained" {
                    children::drain(&child).await?;
                    store.finish_chat_child(&owner, child.id)?;
                }
            }
            let repo = Path::new(&delivery.snapshot.workspace_path);
            let checkout = CheckoutEvidence {
                head: git(repo, &["rev-parse", "--verify", "HEAD"]).await?.trim().into(),
                branch: crate::current_branch(repo).await,
                status: git(repo, &["status", "--porcelain=v1", "--untracked-files=all"]).await?,
            };
            if !owner.quiescent() { return Err(Error::invalid("metadata children have not drained")); }
            if let Some(approval) = approval {
                if approval.checkout != checkout { return Err(Error::invalid("the inspected checkout changed; inspect again before acknowledging")); }
                return Ok(("acknowledged", format!("Kept after inspection, without a delivery verdict: {}. This acknowledgement changed no files or refs and returned no leases; the earlier delivery outcome remains uncertified.", approval.reason.trim()), checkout));
            }
            if !attempted { return Ok(("refused", "The previous command drained before any external action was attempted. Approval withdrawn.".into(), checkout)); }
            let (state, message) = observe(&delivery.snapshot).await?.unwrap_or_else(|| ("inspection", "All recorded command groups have drained, but the delivery outcome is not the reviewed state. Inspect the checkout and remote; you may explicitly keep the current result without certifying delivery.".into()));
            if !owner.quiescent() { return Err(Error::invalid("inspection children have not drained")); }
            Ok((state, message, checkout))
        }.await;
        match result {
            Ok((state, message, checkout)) => Ok(DeliveryInspection { delivery: store.settle_chat_delivery(chat, id, state, &message)?, checkout: Some(checkout) }),
            Err(error) => Ok(DeliveryInspection { delivery: store.settle_chat_delivery(chat, id, "inspection", &error.to_string())?, checkout: None }),
        }
    }).await
}
