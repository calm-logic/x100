//! The main panel: the tab bar and the open tab. A tab shows a dot when something in it changed
//! while it was not open, from the parts core's `Changes` names.
//!
//! Each tab's view is made in [`MainPanel::make`], the first time the tab is opened, and kept.

use std::collections::{BTreeSet, HashMap};

use gpui::{AnyView, Context, Entity, SharedString, Subscription, Window, div, prelude::*, px};
use x100_client::Change;
use x100_core::fold::ProjectPart;
use x100_core::id::ProjectId;

use crate::budget::BudgetTab;
use crate::files::FilesView;
use crate::infra::InfraView;
use crate::log::LogView;
use crate::preview::PreviewView;
use crate::session::{Session, SessionEvent};
use crate::settings::SettingsView;
use crate::specs::SpecsView;
use crate::tasks::TasksView;
use crate::team::TeamView;
use crate::{theme, widgets};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Tab {
    Team,
    Specs,
    Tasks,
    Files,
    Budget,
    Log,
    Preview,
    Infra,
    Settings,
}

impl Tab {
    /// In the order `PRODUCT.md` gives.
    pub const ALL: [Tab; 9] = [
        Tab::Team,
        Tab::Specs,
        Tab::Tasks,
        Tab::Files,
        Tab::Budget,
        Tab::Log,
        Tab::Preview,
        Tab::Infra,
        Tab::Settings,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Tab::Team => "Team",
            Tab::Specs => "Specs",
            Tab::Tasks => "Tasks",
            Tab::Files => "Files",
            Tab::Budget => "Budget",
            Tab::Log => "Log",
            Tab::Preview => "Preview",
            Tab::Infra => "Infra",
            Tab::Settings => "Settings",
        }
    }

    /// The tab a part of the project's state is shown in. The chat, held actions and the sidebar
    /// are not tabs, and nothing in v1 changes Infra.
    pub fn showing(part: ProjectPart) -> Option<Tab> {
        Some(match part {
            ProjectPart::Team => Tab::Team,
            ProjectPart::Specs => Tab::Specs,
            ProjectPart::Tasks => Tab::Tasks,
            ProjectPart::Files => Tab::Files,
            ProjectPart::Budget => Tab::Budget,
            ProjectPart::Log => Tab::Log,
            ProjectPart::Preview => Tab::Preview,
            ProjectPart::Settings => Tab::Settings,
            ProjectPart::Chat | ProjectPart::HeldActions | ProjectPart::Sidebar => return None,
        })
    }
}

pub struct MainPanel {
    session: Entity<Session>,
    project: ProjectId,
    open: Tab,
    unseen: BTreeSet<Tab>,
    team: Entity<TeamView>,
    others: HashMap<Tab, AnyView>,
    _subscription: Subscription,
}

impl MainPanel {
    pub fn new(session: Entity<Session>, project: ProjectId, cx: &mut Context<Self>) -> Self {
        let subscription = cx.subscribe(&session, |this, _, event, cx| {
            if let SessionEvent::Changed(change) = event {
                this.changed(change, cx);
            }
        });
        let team = cx.new(|cx| TeamView::new(session.clone(), project, cx));
        Self {
            session,
            project,
            open: Tab::Team,
            unseen: BTreeSet::new(),
            team,
            others: HashMap::new(),
            _subscription: subscription,
        }
    }

    pub fn team(&self) -> &Entity<TeamView> {
        &self.team
    }

    pub fn open_tab(&self) -> Tab {
        self.open
    }

    /// The tabs showing a dot.
    pub fn unseen(&self) -> &BTreeSet<Tab> {
        &self.unseen
    }

    pub fn open(&mut self, tab: Tab, cx: &mut Context<Self>) {
        self.open = tab;
        self.unseen.remove(&tab);
        cx.notify();
    }

    /// The view of a tab that has been opened, as its own type.
    pub fn tab_view<V: 'static>(&self, tab: Tab) -> Option<Entity<V>> {
        self.others.get(&tab)?.clone().downcast().ok()
    }

    fn make(&self, tab: Tab, cx: &mut Context<Self>) -> AnyView {
        let (session, project) = (self.session.clone(), self.project);
        match tab {
            Tab::Team => self.team.clone().into(),
            Tab::Specs => cx.new(|cx| SpecsView::new(session, project, cx)).into(),
            Tab::Tasks => cx.new(|cx| TasksView::new(session, project, cx)).into(),
            Tab::Files => cx.new(|cx| FilesView::new(session, project, cx)).into(),
            Tab::Budget => cx.new(|cx| BudgetTab::new(session, project, cx)).into(),
            Tab::Log => cx.new(|cx| LogView::new(session, project, cx)).into(),
            Tab::Preview => cx.new(|cx| PreviewView::new(session, project, cx)).into(),
            Tab::Infra => cx.new(|_| InfraView).into(),
            Tab::Settings => cx.new(|cx| SettingsView::new(session, project, cx)).into(),
        }
    }

    fn view(&mut self, tab: Tab, cx: &mut Context<Self>) -> AnyView {
        if let Some(view) = self.others.get(&tab) {
            return view.clone();
        }
        let view = self.make(tab, cx);
        self.others.insert(tab, view.clone());
        view
    }

    fn changed(&mut self, change: &Change, cx: &mut Context<Self>) {
        let Change::Project { project, changes } = change else {
            return;
        };
        if *project != self.project {
            return;
        }
        let before = self.unseen.len();
        let open = self.open;
        self.unseen.extend(
            changes
                .parts
                .iter()
                .filter_map(|p| Tab::showing(*p))
                .filter(|t| *t != open),
        );
        if self.unseen.len() != before {
            cx.notify();
        }
    }
}

impl Render for MainPanel {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let open = self.open;
        let view = self.view(open, cx);
        let tabs = Tab::ALL.map(|tab| {
            let name: SharedString = tab.name().into();
            div()
                .id(tab.name())
                .debug_selector(move || format!("tab-{}", tab.name()))
                .relative()
                .flex_none()
                .px(px(10.))
                .py(px(5.))
                .rounded(px(7.))
                .text_size(px(theme::TEXT))
                .cursor_pointer()
                .text_color(if tab == open {
                    theme::text()
                } else {
                    theme::muted()
                })
                .when(tab == open, |d| d.bg(theme::selected()))
                .hover(|s| s.text_color(theme::text()))
                .on_click(cx.listener(move |this, _, _, cx| this.open(tab, cx)))
                .child(name)
                .when(self.unseen.contains(&tab), |d| {
                    d.child(
                        widgets::dot(5., theme::live())
                            .absolute()
                            .right(px(3.))
                            .top(px(4.)),
                    )
                })
        });
        div()
            .size_full()
            .flex()
            .flex_col()
            .min_w_0()
            .child(
                div()
                    .flex_none()
                    .flex()
                    .gap(px(2.))
                    .px(px(10.))
                    .py(px(8.))
                    .border_b_1()
                    .border_color(theme::line())
                    .children(tabs),
            )
            .child(div().flex_1().min_h_0().child(view))
    }
}
