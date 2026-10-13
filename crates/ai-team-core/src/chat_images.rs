//! Bounded, immutable image input. Names never become paths or publication authority.
use std::io::{Cursor, Write};

use base64::{engine::general_purpose::STANDARD, Engine};
use serde::{Deserialize, Serialize};

use crate::{Error, Result};

pub const MAX_IMAGES: usize = 4;
pub const MAX_IMAGE_BYTES: usize = 5 * 1024 * 1024;
pub const MAX_REQUEST_BYTES: usize = 29 * 1024 * 1024;

#[derive(Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ImageInput {
    pub name: String,
    pub media_type: String,
    pub data: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ImageInfo {
    pub index: usize,
    pub name: String,
    pub media_type: String,
    pub bytes: usize,
}

impl std::fmt::Debug for ImageInput {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ImageInput")
            .field("name", &self.name)
            .field("media_type", &self.media_type)
            .field("encoded_bytes", &self.data.len())
            .finish()
    }
}

#[derive(Debug, Default)]
pub struct Images(pub(crate) Vec<ImageInput>);

#[derive(Debug)]
pub struct Prompt {
    pub expected_reasoning: Option<crate::Reasoning>,
    pub workspace_epoch: i64,
    pub images: Images,
    pub publication: Option<crate::chat_push::Target>,
}

impl Images {
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
    pub(crate) fn json(&self) -> Result<String> {
        Ok(serde_json::to_string(&self.0)?)
    }
}

pub(crate) fn attach(
    store: &crate::Store,
    chat: i64,
    node: &crate::NodeRun,
    registry: &crate::ModelRegistry,
    turn: &mut crate::pi::PiTurn,
) -> Result<()> {
    let images = store.chat_run_images(chat, node.run_id)?;
    if images.is_empty() {
        return Ok(());
    }
    let supports_images = registry
        .models()?
        .iter()
        .any(|model| model.provider == node.provider && model.model == node.model && model.images);
    if !supports_images {
        return Err(Error::invalid("this turn's model does not advertise image input; choose an image-capable model before sending images"));
    }
    turn.attach_images(&images)
}

/// Decode and validate before any admission, stop, queue or publication side effect.
/// Full decoding catches truncated/corrupt images, with a bounded allocation budget.
pub fn validate(images: Vec<ImageInput>) -> Result<Images> {
    if images.len() > MAX_IMAGES {
        return Err(Error::invalid("attach at most 4 images per message"));
    }
    for input in &images {
        if input.name.is_empty()
            || input.name.len() > 240
            || input.name.chars().any(char::is_control)
        {
            return Err(Error::invalid(
                "an image needs a name of 1–240 bytes without control characters",
            ));
        }
        let expected =
            match input.media_type.as_str() {
                "image/png" => image::ImageFormat::Png,
                "image/jpeg" => image::ImageFormat::Jpeg,
                "image/gif" => image::ImageFormat::Gif,
                "image/webp" => image::ImageFormat::WebP,
                _ => return Err(Error::invalid(
                    "attach PNG, JPEG, GIF or WebP images; SVG and other files are not supported",
                )),
            };
        let bytes = decode(input)?;
        if image::guess_format(&bytes).ok() != Some(expected) {
            return Err(Error::invalid(
                "the image bytes do not match their declared type",
            ));
        }
        let mut reader = image::ImageReader::with_format(Cursor::new(&bytes), expected);
        let mut limits = image::Limits::default();
        limits.max_image_width = Some(8192);
        limits.max_image_height = Some(8192);
        limits.max_alloc = Some(64 * 1024 * 1024);
        reader.limits(limits);
        reader.decode().map_err(|_| {
            Error::invalid(
                "the image is corrupt or exceeds the image decoding limits (8192px / 64 MiB)",
            )
        })?;
    }
    Ok(Images(images))
}

fn decode(input: &ImageInput) -> Result<Vec<u8>> {
    if input.data.len() > MAX_IMAGE_BYTES.div_ceil(3) * 4 {
        return Err(Error::invalid("each image must be at most 5 MiB"));
    }
    let bytes = STANDARD
        .decode(&input.data)
        .map_err(|_| Error::invalid("image data is not valid base64"))?;
    if bytes.is_empty() || bytes.len() > MAX_IMAGE_BYTES {
        return Err(Error::invalid("each image must contain 1 byte to 5 MiB"));
    }
    Ok(bytes)
}

/// Private scratch files live only as long as this Pi invocation. Retained bytes stay
/// in the database; no file path is accepted from the browser or written in a checkout.
pub(crate) fn materialize(images: &[ImageInput]) -> Result<Option<tempfile::TempDir>> {
    if images.is_empty() {
        return Ok(None);
    }
    let mut builder = tempfile::Builder::new();
    builder.prefix("ai-team-images-");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        builder.permissions(std::fs::Permissions::from_mode(0o700));
    }
    let directory = builder.tempdir()?;
    for (index, input) in images.iter().enumerate() {
        let extension = match input.media_type.as_str() {
            "image/png" => "png",
            "image/jpeg" => "jpg",
            "image/gif" => "gif",
            "image/webp" => "webp",
            _ => return Err(Error::invalid("the recorded image format is not supported")),
        };
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(directory.path().join(format!("{index}.{extension}")))?;
        file.write_all(&decode(input)?)?;
    }
    Ok(Some(directory))
}

#[cfg(test)]
mod tests;
