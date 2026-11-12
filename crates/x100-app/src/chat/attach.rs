//! Files attached to the brief and to messages: choosing them, uploading them through the
//! protocol, and how each is previewed. An image shows as itself; anything else as its name,
//! kind and size.

use std::path::PathBuf;
use std::sync::Arc;

use gpui::{AnyElement, Image, ObjectFit, div, img, prelude::*, px};
use x100_client::Client;
use x100_core::file::FileRef;
use x100_core::id::ProjectId;

use crate::{theme, words};

/// A file read from disk and recorded by the daemon, with its bytes for a preview.
pub struct Uploaded {
    pub file: FileRef,
    pub bytes: Vec<u8>,
}

/// Reads `path` and uploads it to the project.
pub async fn upload(
    client: Client,
    project: ProjectId,
    path: PathBuf,
    bytes: std::io::Result<Vec<u8>>,
) -> Result<Uploaded, String> {
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "file".into());
    let bytes = bytes.map_err(|e| format!("could not read {}: {e}", path.display()))?;
    let media_type = words::media_type(&name);
    let file = client
        .upload_file(project, name.clone(), media_type, bytes.clone())
        .await
        .map_err(|e| format!("could not upload {name}: {e}"))?;
    Ok(Uploaded { file, bytes })
}

/// A file's preview. `image` is its picture once there is one.
pub fn preview(file: &FileRef, image: Option<Arc<Image>>) -> AnyElement {
    let frame = div()
        .flex_none()
        .rounded(px(8.))
        .border_1()
        .border_color(theme::line())
        .bg(gpui::rgb(0x0a0a0b))
        .overflow_hidden();
    match image {
        Some(image) => frame
            .w(px(64.))
            .h(px(56.))
            .child(img(image).size_full().object_fit(ObjectFit::Cover))
            .into_any_element(),
        None => frame
            .max_w(px(220.))
            .px(px(10.))
            .py(px(7.))
            .flex()
            .flex_col()
            .gap(px(2.))
            .child(
                div()
                    .text_size(px(theme::SMALL))
                    .text_color(theme::text())
                    .whitespace_nowrap()
                    .overflow_hidden()
                    .text_ellipsis()
                    .child(file.name.clone()),
            )
            .child(
                div()
                    .text_size(px(theme::TINY))
                    .text_color(theme::muted())
                    .child(format!(
                        "{} · {}",
                        words::kind(&file.media_type),
                        words::size(file.size)
                    )),
            )
            .into_any_element(),
    }
}
