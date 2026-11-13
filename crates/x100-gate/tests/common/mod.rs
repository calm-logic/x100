//! Shared fixtures: the Fernleaf project, a project with no team yet, and ways to put an action
//! into a state the way the daemon would.
#![allow(dead_code)]

use x100_core::action::{
    Action, ActionEvent, ActionKind, AskMeAbout, Classification, ClassifiedBy, Decision, Request,
    Risk,
};
use x100_core::effort::EffortRange;
use x100_core::event::{Envelope, ProjectEvent, Seq};
use x100_core::fold::ProjectState;
use x100_core::id::{ActionId, AgentId, DriverId, OneOffCallId, TaskId};
use x100_core::money::{Money, Percent};
use x100_core::party::Actor;
use x100_core::project::{SettingChange, Settings};
use x100_core::testing::{Fernleaf, START, fernleaf, gpt, opus, sonnet};
use x100_core::time::Timestamp;
use x100_core::tool::ToolCall;
use x100_gate::{Asked, Classifier, Labels, Question, Ruling, Verdict, assess, judge};

pub const LEVELS: [AskMeAbout; 4] = [
    AskMeAbout::Nothing,
    AskMeAbout::HighRiskOnly,
    AskMeAbout::MediumAndUp,
    AskMeAbout::EveryDecision,
];

pub fn push(state: &mut ProjectState, by: Actor, event: ProjectEvent) {
    let seq = Seq(state.seq().map_or(1, |s| s.0 + 1));
    let at = Timestamp::from_millis(START.as_millis() + 100_000_000 + seq.0 * 1000);
    state
        .apply(&Envelope { seq, at, by, event })
        .expect("the event folds");
}

pub fn set_level(state: &mut ProjectState, level: AskMeAbout) {
    push(
        state,
        Actor::Builder,
        ProjectEvent::SettingChanged(SettingChange::AskMeAbout(level)),
    );
}

/// The Fernleaf project at `level`, with its ids.
pub fn fern(level: AskMeAbout) -> (ProjectState, Fernleaf) {
    let f = fernleaf();
    let mut state = ProjectState::fold(&f.log).expect("fernleaf folds");
    set_level(&mut state, level);
    (state, f)
}

/// A project that has a CEO and no team.
pub fn bare(level: AskMeAbout) -> (ProjectState, AgentId) {
    let ceo = AgentId::from_u128(1);
    let mut state = ProjectState::default();
    push(
        &mut state,
        Actor::Builder,
        ProjectEvent::Created {
            ceo,
            charter: "Test".into(),
            effort: EffortRange::FULL,
            settings: Settings {
                ask_me_about: level,
                budget: Money::from_dollars(50),
                lab_share: Percent::new(10).expect("a percentage"),
                trial_before_commit: false,
                allowed: [opus(), sonnet(), gpt()].into(),
            },
        },
    );
    (state, ceo)
}

static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1_000);

pub fn action(agent: AgentId, request: Request) -> Action {
    let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    Action {
        id: ActionId::from_u128(u128::from(n)),
        agent,
        request,
    }
}

pub fn driver() -> DriverId {
    DriverId::from_u128(77)
}

pub fn tool(agent: AgentId, call: ToolCall) -> Action {
    action(
        agent,
        Request::Tool {
            driver: driver(),
            call,
        },
    )
}

pub fn command(agent: AgentId, command: &str) -> Action {
    action(
        agent,
        Request::Command {
            driver: driver(),
            command: command.into(),
        },
    )
}

pub fn egress(agent: AgentId, host: &str, method: &str) -> Action {
    action(
        agent,
        Request::Egress {
            driver: driver(),
            host: host.into(),
            method: method.into(),
        },
    )
}

pub fn merge(agent: AgentId, task: TaskId) -> Action {
    action(agent, Request::Merge { task })
}

/// Records `action` as proposed, classified, held and then decided, as the daemon would.
pub fn decide(state: &mut ProjectState, action: &Action, decision: Decision) {
    let classification = Classification {
        kind: ActionKind::SendOutside,
        risk: Risk::High,
        by: ClassifiedBy::Rules,
        why: "test".into(),
    };
    push(
        state,
        Actor::Agent(action.agent),
        ProjectEvent::ActionProposed(action.clone()),
    );
    let id = action.id;
    for event in [
        ActionEvent::Classified(classification),
        ActionEvent::Held,
        match decision {
            Decision::Approved => ActionEvent::Approved,
            Decision::Denied => ActionEvent::Denied,
        },
    ] {
        let by = if matches!(event, ActionEvent::Approved | ActionEvent::Denied) {
            Actor::Builder
        } else {
            Actor::Daemon
        };
        push(state, by, ProjectEvent::Action { action: id, event });
    }
}

/// The daemon's default confidence threshold.
pub fn threshold() -> Percent {
    Percent::new(70).unwrap()
}

/// The call every answer of [`Says`] names.
pub const CALL: OneOffCallId = OneOffCallId::from_u128(5);

pub fn answered(kind: ActionKind, risk: Risk, confidence: f64) -> Asked {
    Asked::Answered {
        call: CALL,
        labels: Labels {
            kind,
            risk,
            confidence,
        },
    }
}

pub fn no_answer() -> Asked {
    Asked::NoAnswer {
        call: Some(CALL),
        why: "Jev answered 529".into(),
    }
}

/// A classifier with a fixed reply, which remembers nothing.
pub struct Says(pub Asked);

impl Classifier for Says {
    fn classify(&self, question: &Question) -> Asked {
        assert!(question.input.starts_with("UNTRUSTED."));
        self.0.clone()
    }
}

/// A classifier that must not be called.
pub struct Silent;

impl Classifier for Silent {
    fn classify(&self, _: &Question) -> Asked {
        panic!("the rules cover this action; no call is wanted");
    }
}

pub fn ruling(state: &ProjectState, action: &Action) -> Ruling {
    judge(state, action, Some("/work/ws"), &Silent, threshold())
}

/// The rule id a classification names in its `why`, if a rule decided it.
pub fn rule_of(r: &Ruling) -> Option<String> {
    let why = &r.classification().why;
    let open = why.rfind('[')?;
    let inner = &why[open + 1..why.rfind(']')?];
    Some(inner.split(':').next()?.to_owned())
}

pub fn asks(state: &ProjectState, action: &Action) -> bool {
    matches!(assess(state, action, Some("/work/ws")), Verdict::Ask(_))
}

/// The Fernleaf project at each level, built once for property tests.
pub fn fixtures() -> &'static (Vec<(AskMeAbout, ProjectState)>, Fernleaf) {
    static CELL: std::sync::OnceLock<(Vec<(AskMeAbout, ProjectState)>, Fernleaf)> =
        std::sync::OnceLock::new();
    CELL.get_or_init(|| {
        let f = fernleaf();
        let base = ProjectState::fold(&f.log).expect("fernleaf folds");
        let states = LEVELS
            .iter()
            .map(|level| {
                let mut s = base.clone();
                set_level(&mut s, *level);
                (*level, s)
            })
            .collect();
        (states, f)
    })
}
