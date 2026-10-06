//! The desktop operating picture is chat-owned. Legacy runs remain history, never
//! evidence of current activity merely because their old status is still blocked.
use serde::Serialize;

#[derive(Debug, Serialize)]
pub struct ChatToday {
    pub entries: Vec<Entry>,
    pub chats: usize,
    pub needs_attention: usize,
    pub working: usize,
    pub drafts: i64,
}
#[derive(Debug, Serialize)]
pub struct Entry {
    pub chat_id: i64,
    pub project_slug: String,
    pub project_name: String,
    pub title: String,
    pub workspace_path: String,
    pub updated_at: String,
    pub state: String,
    pub detail: Option<String>,
    pub panel: String,
    pub needs_attention: bool,
    pub working: bool,
    pub drafts: i64,
    pub questions: i64,
}
