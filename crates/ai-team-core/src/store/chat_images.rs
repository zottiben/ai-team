//! Image input stays on its immutable turn/queue receipt, not on a mutable checkout.
use crate::{
    chat_images::{ImageInfo, ImageInput},
    Error, Result, Store,
};
use rusqlite::{params, Connection, OptionalExtension};

pub(super) fn metadata(conn: &Connection, node: i64, queued: bool) -> Result<Vec<ImageInfo>> {
    let (table, key) = if queued {
        ("chat_followup", "id")
    } else {
        ("chat_turn", "node_id")
    };
    let mut query = conn.prepare(&format!("SELECT CAST(j.key AS INTEGER),json_extract(j.value,'$.name'),json_extract(j.value,'$.media_type'),json_extract(j.value,'$.bytes') FROM {table} t,json_each(t.images_meta) j WHERE t.{key}=?1 ORDER BY CAST(j.key AS INTEGER)"))?;
    let images = query
        .query_map([node], |row| {
            Ok(ImageInfo {
                index: row.get(0)?,
                name: row.get(1)?,
                media_type: row.get(2)?,
                bytes: row.get(3)?,
            })
        })?
        .collect::<rusqlite::Result<_>>()?;
    Ok(images)
}

pub(super) fn describe(images: &str) -> Result<String> {
    let images: Vec<ImageInput> = serde_json::from_str(images)?;
    let metadata = images
        .iter()
        .enumerate()
        .map(|(index, image)| ImageInfo {
            index,
            name: image.name.clone(),
            media_type: image.media_type.clone(),
            bytes: image.data.len() / 4 * 3
                - image
                    .data
                    .bytes()
                    .rev()
                    .take_while(|byte| *byte == b'=')
                    .count(),
        })
        .collect::<Vec<_>>();
    Ok(serde_json::to_string(&metadata)?)
}

pub(super) fn queued_json(conn: &Connection, id: i64) -> Result<String> {
    Ok(conn.query_row(
        "SELECT images_json FROM chat_followup WHERE id=?1",
        [id],
        |r| r.get(0),
    )?)
}

pub(super) fn check_replay(conn: &Connection, node: i64, images: &str) -> Result<()> {
    let saved: String = conn.query_row(
        "SELECT images_json FROM chat_turn WHERE node_id=?1",
        [node],
        |r| r.get(0),
    )?;
    if saved != images {
        return Err(Error::invalid(
            "that request id belongs to different image attachments",
        ));
    }
    Ok(())
}

impl Store {
    pub fn check_chat_image_replay(
        &self,
        chat: i64,
        node: i64,
        images: &crate::chat_images::Images,
    ) -> Result<()> {
        let saved: String = self.db().conn().query_row(
            "SELECT images_json FROM chat_turn WHERE chat_id=?1 AND node_id=?2",
            params![chat, node],
            |r| r.get(0),
        )?;
        if saved != images.json()? {
            return Err(Error::invalid(
                "that request id belongs to different image attachments",
            ));
        }
        Ok(())
    }

    pub fn chat_image(&self, chat: i64, node: i64, index: usize) -> Result<ImageInput> {
        if index >= crate::chat_images::MAX_IMAGES {
            return Err(Error::invalid("no such image in this message"));
        }
        let value: Option<String> = self.db().conn().query_row("SELECT json_extract(images_json,?3) FROM chat_turn WHERE chat_id=?1 AND node_id=?2", params![chat,node,format!("$[{index}]")], |r| r.get::<_,Option<String>>(0)).optional()?.flatten();
        Ok(serde_json::from_str(&value.ok_or_else(|| {
            Error::invalid("no such image in this chat message")
        })?)?)
    }

    pub fn chat_run_images(&self, chat: i64, run: i64) -> Result<Vec<ImageInput>> {
        let value: Option<String> = self
            .db()
            .conn()
            .query_row(
                "SELECT images_json FROM chat_turn WHERE chat_id=?1 AND run_id=?2",
                params![chat, run],
                |r| r.get(0),
            )
            .optional()?;
        Ok(serde_json::from_str(value.as_deref().unwrap_or("[]"))?)
    }
}
