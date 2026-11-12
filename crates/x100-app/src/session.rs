//! The app's one connection to the daemon, as a GPUI entity. It holds the client, which project
//! is selected, and images fetched for previews, and turns the client's changes into events
//! each view subscribes to. Everything else is read from the client's folded state when drawn.

use std::collections::HashMap;
use std::sync::Arc;

use gpui::{Context, EventEmitter, Image, ImageFormat, Task};
use tokio::sync::broadcast::error::RecvError;
use x100_client::{Change, Client};
use x100_core::fold::{DaemonState, ProjectState};
use x100_core::id::{FileId, ProjectId};

pub enum SessionEvent {
    /// Something in the daemon's state changed, in the client's terms.
    Changed(Change),
    /// Another project was selected.
    Selected,
    /// An image for a preview has arrived.
    Preview,
}

enum Preview {
    Loading,
    Ready(Arc<Image>),
    Failed,
}

pub struct Session {
    client: Client,
    selected: Option<ProjectId>,
    connected: bool,
    previews: HashMap<FileId, Preview>,
    _pump: Task<()>,
}

impl EventEmitter<SessionEvent> for Session {}

impl Session {
    pub fn new(client: Client, cx: &mut Context<Self>) -> Self {
        // Subscribed before anything is read, so no change falls between a read and the pump.
        let mut changes = client.changes();
        let pump = cx.spawn(async move |this, cx| {
            loop {
                let change = match changes.recv().await {
                    Ok(change) => change,
                    // Too far behind to know what changed: everything may have.
                    Err(RecvError::Lagged(_)) => Change::Reset,
                    Err(RecvError::Closed) => return,
                };
                if this.update(cx, |s, cx| s.apply(change, cx)).is_err() {
                    return;
                }
            }
        });
        let mut session = Self {
            client,
            selected: None,
            connected: true,
            previews: HashMap::new(),
            _pump: pump,
        };
        session.select_any();
        session
    }

    pub fn client(&self) -> &Client {
        &self.client
    }

    pub fn connected(&self) -> bool {
        self.connected
    }

    pub fn selected(&self) -> Option<ProjectId> {
        self.selected
    }

    pub fn select(&mut self, project: ProjectId, cx: &mut Context<Self>) {
        if self.selected != Some(project) {
            self.selected = Some(project);
            cx.emit(SessionEvent::Selected);
        }
    }

    /// Reads one project's state, with the daemon's for the names of harnesses and machines.
    pub fn project<R>(
        &self,
        project: ProjectId,
        f: impl FnOnce(&ProjectState, &DaemonState) -> R,
    ) -> Option<R> {
        self.client
            .with_state(|s| s.projects.get(&project).map(|p| f(p, &s.daemon)))
    }

    fn apply(&mut self, change: Change, cx: &mut Context<Self>) {
        if let Change::Connection { connected } = change {
            self.connected = connected;
        }
        if self.selected.is_none() && self.select_any() {
            cx.emit(SessionEvent::Selected);
        }
        cx.emit(SessionEvent::Changed(change));
    }

    /// Selects the first project, if there is one and none is selected.
    fn select_any(&mut self) -> bool {
        if self.selected.is_none() {
            self.selected = self
                .client
                .with_state(|s| s.daemon.projects().next().map(|(id, _)| id));
        }
        self.selected.is_some()
    }

    /// The image to preview a file with, fetched from the daemon the first time it is asked for.
    /// `None` while it loads, and for anything that is not an image.
    pub fn image(
        &mut self,
        project: ProjectId,
        file: FileId,
        media_type: &str,
        cx: &mut Context<Self>,
    ) -> Option<Arc<Image>> {
        let format = image_format(media_type)?;
        match self.previews.get(&file) {
            Some(Preview::Ready(image)) => return Some(image.clone()),
            Some(Preview::Loading | Preview::Failed) => return None,
            None => {}
        }
        self.previews.insert(file, Preview::Loading);
        let client = self.client.clone();
        cx.spawn(async move |this, cx| {
            let read = client.read_file(project, file).await;
            let _ = this.update(cx, |s, cx| {
                let preview = match read {
                    Ok((_, bytes)) => Preview::Ready(Arc::new(Image::from_bytes(format, bytes))),
                    Err(e) => {
                        log::warn!("could not read {file} for a preview: {e}");
                        Preview::Failed
                    }
                };
                s.previews.insert(file, preview);
                cx.emit(SessionEvent::Preview);
            });
        })
        .detach();
        None
    }

    /// Keeps the bytes of a file just uploaded, so its preview needs no second read.
    pub fn keep_image(&mut self, file: FileId, media_type: &str, bytes: &[u8]) {
        if let Some(format) = image_format(media_type) {
            let image = Arc::new(Image::from_bytes(format, bytes.to_vec()));
            self.previews.insert(file, Preview::Ready(image));
        }
    }
}

pub fn image_format(media_type: &str) -> Option<ImageFormat> {
    Some(match media_type {
        "image/png" => ImageFormat::Png,
        "image/jpeg" => ImageFormat::Jpeg,
        "image/gif" => ImageFormat::Gif,
        "image/webp" => ImageFormat::Webp,
        "image/svg+xml" => ImageFormat::Svg,
        "image/bmp" => ImageFormat::Bmp,
        _ => return None,
    })
}
