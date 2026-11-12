//! The Files tab: every file the project holds outside the code, each with a preview, its kind,
//! who added it and its size. A file is removed with a confirm, and the removal is logged.

use gpui::{
    Context, Entity, FontWeight, ObjectFit, Subscription, Window, div, img, prelude::*, px,
};
use x100_client::Change;
use x100_core::fold::{ProjectPart, ProjectState};
use x100_core::id::{FileId, ProjectId};
use x100_core::party::Party;

use crate::notice::{self, Outcome};
use crate::session::{Session, SessionEvent};
use crate::{theme, words};

const CARD_WIDTH: f32 = 168.;
const PREVIEW_HEIGHT: f32 = 84.;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileModel {
    pub id: FileId,
    pub name: String,
    pub media_type: String,
    pub kind: String,
    pub by: String,
    pub size: u64,
}

impl FileModel {
    /// Every file the project holds, in the order they were added.
    pub fn all(p: &ProjectState) -> Vec<FileModel> {
        let mut files: Vec<_> = p.files().collect();
        files.sort_by_key(|f| (f.added, f.file.id));
        files
            .into_iter()
            .map(|f| FileModel {
                id: f.file.id,
                name: f.file.name.clone(),
                media_type: f.file.media_type.clone(),
                kind: words::kind(&f.file.media_type),
                by: match f.file.added_by {
                    Party::Builder => "You".into(),
                    Party::Agent(a) => p
                        .agent(a)
                        .map_or_else(|| "an agent".into(), |a| a.role.clone()),
                },
                size: f.file.size,
            })
            .collect()
    }
}

pub struct FilesView {
    session: Entity<Session>,
    project: ProjectId,
    files: Vec<FileModel>,
    /// The file the builder is being asked to confirm removing.
    confirming: Option<FileId>,
    outcome: Option<Outcome>,
    _subscription: Subscription,
}

impl FilesView {
    pub fn new(session: Entity<Session>, project: ProjectId, cx: &mut Context<Self>) -> Self {
        let subscription = cx.subscribe(&session, |this, _, event, cx| {
            let ours = match event {
                SessionEvent::Changed(Change::Project { project, changes }) => {
                    *project == this.project && changes.parts.contains(&ProjectPart::Files)
                }
                SessionEvent::Changed(Change::Reset) => true,
                // A picture has arrived.
                SessionEvent::Preview => {
                    cx.notify();
                    false
                }
                _ => false,
            };
            if ours {
                this.refresh(cx);
            }
        });
        let mut view = Self {
            session,
            project,
            files: Vec::new(),
            confirming: None,
            outcome: None,
            _subscription: subscription,
        };
        view.refresh(cx);
        view
    }

    /// The files shown, in order.
    pub fn shown(&self) -> &[FileModel] {
        &self.files
    }

    pub fn outcome(&self) -> Option<&Outcome> {
        self.outcome.as_ref()
    }

    fn refresh(&mut self, cx: &mut Context<Self>) {
        let now = self
            .session
            .read(cx)
            .project(self.project, |p, _| FileModel::all(p))
            .unwrap_or_default();
        if now != self.files {
            if self
                .confirming
                .is_some_and(|c| !now.iter().any(|f| f.id == c))
            {
                self.confirming = None;
            }
            self.files = now;
            cx.notify();
        }
    }

    fn remove(&mut self, file: FileId, cx: &mut Context<Self>) {
        let client = self.session.read(cx).client().clone();
        let project = self.project;
        self.outcome = Some(Outcome::Working);
        cx.notify();
        notice::run(
            cx,
            async move { client.remove_file(project, file).await },
            |this, result, cx| {
                this.outcome = result.err();
                this.confirming = None;
                cx.notify();
            },
        );
    }

    fn card(&mut self, f: &FileModel, cx: &mut Context<Self>) -> gpui::AnyElement {
        let id = f.id;
        let image = self
            .session
            .update(cx, |s, cx| s.image(self.project, f.id, &f.media_type, cx));
        let preview = div()
            .w_full()
            .h(px(PREVIEW_HEIGHT))
            .rounded(px(8.))
            .overflow_hidden()
            .bg(gpui::rgb(0x0a0a0b))
            .flex()
            .items_center()
            .justify_center();
        let preview = match image {
            Some(image) => preview.child(img(image).size_full().object_fit(ObjectFit::Contain)),
            None => preview.child(
                div()
                    .text_size(px(15.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(theme::dim())
                    .child(f.kind.to_uppercase()),
            ),
        };
        let name = f.name.clone();
        let confirming = self.confirming == Some(id);
        div()
            .relative()
            .flex_none()
            .w(px(CARD_WIDTH))
            .p(px(9.))
            .flex()
            .flex_col()
            .gap(px(4.))
            .rounded(px(12.))
            .border_1()
            .border_color(theme::line())
            .child(preview)
            .child(
                div()
                    .text_size(px(theme::TEXT))
                    .font_weight(FontWeight::MEDIUM)
                    .whitespace_nowrap()
                    .overflow_hidden()
                    .text_ellipsis()
                    .child(f.name.clone()),
            )
            .child(
                div()
                    .text_size(px(theme::TINY))
                    .text_color(theme::muted())
                    .child(format!(
                        "{} \u{b7} {} \u{b7} {}",
                        f.kind,
                        f.by,
                        words::size(f.size)
                    )),
            )
            .child(if confirming {
                div()
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .text_size(px(theme::TINY))
                    .child(
                        div()
                            .id(("remove-yes", id.ulid().0 as u64))
                            .debug_selector({
                                let name = name.clone();
                                move || format!("file-remove-{name}")
                            })
                            .px(px(9.))
                            .py(px(3.))
                            .rounded_full()
                            .border_1()
                            .border_color(theme::error())
                            .text_color(theme::error())
                            .cursor_pointer()
                            .on_click(cx.listener(move |this, _, _, cx| this.remove(id, cx)))
                            .child("Remove"),
                    )
                    .child(
                        div()
                            .id(("remove-no", id.ulid().0 as u64))
                            .px(px(9.))
                            .py(px(3.))
                            .rounded_full()
                            .border_1()
                            .border_color(theme::line())
                            .text_color(theme::muted())
                            .cursor_pointer()
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.confirming = None;
                                cx.notify();
                            }))
                            .child("Keep"),
                    )
                    .into_any_element()
            } else {
                div().h(px(0.)).into_any_element()
            })
            .child(
                div()
                    .id(("file-x", id.ulid().0 as u64))
                    .debug_selector(move || format!("file-x-{name}"))
                    .absolute()
                    .top(px(14.))
                    .right(px(14.))
                    .size(px(20.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded_full()
                    .bg(gpui::rgba(0x000000b3))
                    .border_1()
                    .border_color(theme::line())
                    .text_size(px(theme::TINY))
                    .cursor_pointer()
                    .hover(|s| s.bg(theme::selected()))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.confirming = Some(id);
                        this.outcome = None;
                        cx.notify();
                    }))
                    .child("\u{d7}"),
            )
            .into_any_element()
    }
}

impl Render for FilesView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let files = self.files.clone();
        let total: u64 = files.iter().map(|f| f.size).sum();
        let cards: Vec<_> = files.iter().map(|f| self.card(f, cx)).collect();
        div()
            .id("files")
            .size_full()
            .overflow_y_scroll()
            .flex()
            .flex_col()
            .gap(px(12.))
            .p(px(16.))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(12.))
                    .text_size(px(theme::SMALL))
                    .text_color(theme::muted())
                    .child(format!(
                        "{} \u{b7} {}",
                        if files.len() == 1 {
                            "1 file".to_owned()
                        } else {
                            format!("{} files", files.len())
                        },
                        words::size(total)
                    ))
                    .children(self.outcome.as_ref().map(notice::line)),
            )
            .child(div().flex().flex_wrap().gap(px(12.)).children(cards))
            .when(files.is_empty(), |d| {
                d.child(
                    div()
                        .text_size(px(theme::TEXT))
                        .text_color(theme::dim())
                        .child("No files yet. Reference images, bug screenshots and anything you attach are kept here."),
                )
            })
    }
}
