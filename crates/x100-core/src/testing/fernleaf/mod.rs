//! Fernleaf: a project from brief to a team at work, as the daemon would log it.
//!
//! The builder's first message to a new project carries a brief and a sketch. The CEO sets the
//! project up from it on a driver in the scratch directory, then, on a new one, writes the BRD,
//! forms a team of three leads, asks the builder a question and gets an answer. Tech starts Backend and Frontend, and
//! Backend starts Migrations; Product starts Research and asks Tech a question directly. Backend's
//! driver fails and another provider's takes over, then a send-back raising the effort hands its
//! work off on purpose. A production deploy is held and approved, an email out is held and denied.
//! Marketing reaches its budget. A daily job delegates smoke tests once and is cancelled, and
//! Research waits on a job of its own. Files come and go, specs are revised, and two trials run in
//! the lab: one committed, one discarded.
//!
//! It ends with a task in each status, the CEO waiting on a second question, Backend at work on a
//! task sent back to it, and a dev server running in Frontend's sandbox.

mod opening;

use std::time::Duration;

use opening::{Opening, opening};

use super::{LogBuilder, commit, gpt, opus, sonnet};
use crate::action::{ActionEvent, ActionKind, ClassifiedBy, Decision, Request, Risk};
use crate::agent::AgentEvent;
use crate::credential::{CredentialUse, UseOutcome, Via};
use crate::decision::DecisionProvider;
use crate::driver::{
    DriverEvent, Failure, Handoff, HandoffCause, InFlight, PlannedReason, RetireReason,
    StartReason, TakeoverCause,
};
use crate::effort::{Effort, EffortRange};
use crate::event::{Envelope, ProjectEvent};
use crate::harness::{ModelName, OneOffCall, OneOffPurpose};
use crate::id::{
    ActionId, AgentId, CredentialRef, DriverId, FileId, JobId, MessageId, OneOffCallId, TaskId,
    TrialId,
};
use crate::inbox::InboxItem;
use crate::job::JobEvent;
use crate::ledger::{BudgetReached, Charge, ChargeSource, CostBasis, Payer};
use crate::memory::{NoteName, SkillName, SkillRef, SkillScope};
use crate::money::{Money, Usage};
use crate::party::{Actor, Party};
use crate::project::SettingChange;
use crate::task::{Opening as TaskOpening, Task};
use crate::time::Timestamp;
use crate::tool::{Recipient, ToolCall};
use crate::wait::WaitOn;

/// The scenario's log and the ids a test needs to look things up.
pub struct Fernleaf {
    pub log: Vec<Envelope<ProjectEvent>>,
    pub ceo: AgentId,
    pub tech: AgentId,
    pub product: AgentId,
    pub marketing: AgentId,
    pub backend: AgentId,
    pub frontend: AgentId,
    pub migrations: AgentId,
    pub research: AgentId,
    /// The brief, delegated by the builder to the CEO.
    pub brief: TaskId,
    pub site: TaskId,
    pub prd: TaskId,
    pub copy: TaskId,
    pub api: TaskId,
    pub landing: TaskId,
    pub schema: TaskId,
    pub competitors: TaskId,
    pub bug: TaskId,
    pub smoke: TaskId,
    pub question: MessageId,
    pub answer: MessageId,
    pub budget_question: MessageId,
    pub lead_question: MessageId,
    pub deploy: ActionId,
    pub email: ActionId,
    pub smoke_job: JobId,
    pub recheck_job: JobId,
    pub sketch: FileId,
    pub competitors_png: FileId,
    pub screenshot: FileId,
    pub failed_driver: DriverId,
    pub takeover_driver: DriverId,
    pub backend_driver: DriverId,
    pub committed_trial: TrialId,
    pub discarded_trial: TrialId,
}

pub const START: Timestamp = Timestamp::from_millis(1_794_387_600_000);

pub(super) const MINUTE: Duration = Duration::from_secs(60);

pub(super) fn range(min: Effort, max: Effort) -> EffortRange {
    EffortRange::new(min, max).expect("not empty")
}

pub fn deploy_skill() -> SkillRef {
    SkillRef {
        scope: SkillScope::Shared,
        name: SkillName("deploy".into()),
    }
}

/// Builds the scenario. Every event in it folds, or this panics at the one that does not.
pub fn fernleaf() -> Fernleaf {
    let mut b = LogBuilder::new(START);
    let Opening {
        ceo,
        tech,
        product,
        marketing,
        backend,
        frontend,
        research,
        brief,
        site,
        prd,
        copy,
        api,
        landing,
        competitors,
        question,
        answer,
        lead_question,
        smoke_job,
        recheck_job,
        sketch,
        competitors_png,
        dc,
        dt,
        committed_trial,
        discarded_trial,
    } = opening(&mut b);

    // Backend starts Migrations, whose first merge conflicts.
    let db = b.start_driver(
        backend,
        gpt(),
        Effort::Medium,
        StartReason::NewTask { task: api },
    );
    let (mut schema, mut migrations) = (api, backend);
    b.turn(db, 150_000, |b| {
        (schema, migrations) = b.delegate_new(
            db,
            "Migrations",
            "Write the schema migrations",
            "Waitlist table.",
            Effort::Medium,
        );
        b.tool(
            db,
            ToolCall::Remember {
                note: NoteName("auth".into()),
                body: "Sessions in KV.".into(),
            },
            |b| {
                let event = ProjectEvent::MemoryWritten {
                    agent: backend,
                    note: NoteName("auth".into()),
                    commit: commit("memory"),
                };
                b.push(Actor::Daemon, event);
            },
        );
        b.wait(
            db,
            api,
            WaitOn::Report(schema),
            "Routes sketched; schema delegated.",
            30,
        );
    });
    let dg = b.start_driver(
        migrations,
        gpt(),
        Effort::Medium,
        StartReason::NewTask { task: schema },
    );
    b.turn(dg, 25_000, |b| {
        b.report(dg, schema, "Three tables, two indexes.")
    });
    b.went_idle(migrations);
    b.turn(db, 30_000, |b| {
        let merge = b.accept(db, schema);
        b.merge(
            merge,
            schema,
            Some("migrations/0002.sql conflicts with main"),
        );
        b.wait(
            db,
            api,
            WaitOn::Report(schema),
            "Schema merge conflicted; sent back.",
            40,
        );
    });
    b.turn(dg, 20_000, |b| b.report(dg, schema, "Rebased on main."));
    b.went_idle(migrations);
    b.turn(db, 30_000, |b| {
        let merge = b.accept(db, schema);
        b.merge(merge, schema, None);
        b.update(db, api, "Endpoints done, auth next.", 60);
    });
    b.driver_event(dg, DriverEvent::Retired(RetireReason::CannotCompact));

    // Frontend reports while Backend's driver fails mid-turn and another provider's takes over.
    let df = b.start_driver(
        frontend,
        opus(),
        Effort::High,
        StartReason::NewTask { task: landing },
    );
    let (mut screenshot, mut bug) = (sketch, landing);
    b.turn(df, 260_000, |b| {
        b.load_skill(df, deploy_skill(), 2);
        screenshot = b.add_file(df, "hero-overlap.png", 120_004);
        bug = b.id(TaskId::new);
        let call = ToolCall::RaiseTask {
            title: "Hero image overlaps the nav on mobile".into(),
            ask: "Seen at 375px.".into(),
            part: Some("Website".into()),
            attachments: vec![screenshot],
        };
        let task = Task {
            id: bug,
            title: "Hero image overlaps the nav on mobile".into(),
            ask: "Seen at 375px.".into(),
            part: Some("Website".into()),
            raised_by: Party::Agent(frontend),
            attachments: vec![screenshot],
        };
        b.tool(df, call, |b| {
            let opening = TaskOpening::Raised;
            b.push(
                Actor::Agent(frontend),
                ProjectEvent::TaskOpened { task, opening },
            );
        });
        b.process(df, "storybook", Some("npm run storybook"));
        b.process(df, "storybook", None);
        b.process(df, "dev", Some("npm run dev -- --port 4321"));
        let url = "http://localhost:4321".to_owned();
        b.tool(df, ToolCall::SetPreview { url: url.clone() }, |b| {
            b.push(Actor::Agent(frontend), ProjectEvent::PreviewSet { url });
        });
        b.report(df, landing, "Landing page built; preview running.");
    });
    b.went_idle(frontend);

    b.deliver(
        backend,
        InboxItem::Nudge {
            task: api,
            turns: 1,
        },
    );
    let given = b
        .state()
        .agent(backend)
        .expect("on the team")
        .inbox
        .waiting()
        .collect();
    b.driver_event(db, DriverEvent::TurnStarted { given });
    b.driver_event(db, DriverEvent::Failed(Failure::StreamBroke));
    b.push(
        Actor::Daemon,
        ProjectEvent::Checkpointed {
            agent: backend,
            commit: commit("checkpoint"),
        },
    );
    let handoff = Handoff {
        replaces: db,
        cause: HandoffCause::Takeover(TakeoverCause::Failed(Failure::StreamBroke)),
        in_flight: InFlight::Command {
            command: "cargo test".into(),
        },
    };
    let db2 = b.start_driver(
        backend,
        sonnet(),
        Effort::Medium,
        StartReason::Handoff(handoff),
    );
    b.turn(db2, 90_000, |b| {
        b.report(db2, api, "Five routes, auth, tests pass.")
    });
    b.went_idle(backend);

    // Tech accepts the landing page, leaving its merge running, and asks to deploy.
    let mut deploy = ActionId::from_u128(0);
    b.turn(dt, 40_000, |b| {
        b.accept(dt, landing);
        let request = Request::Command {
            driver: dt,
            command: "wrangler deploy --env production".into(),
        };
        let why = "Deploys to production.";
        deploy = b.held(
            tech,
            request,
            ActionKind::DeployToProduction,
            ClassifiedBy::Rules,
            why,
        );
        b.wait(
            dt,
            site,
            WaitOn::Decision(deploy),
            "Landing accepted; deploy held.",
            70,
        );
    });
    b.decide(deploy, Decision::Approved);

    // The smoke-test job fires once, delegating to Tech, and the builder cancels it.
    b.advance(60 * MINUTE);
    let smoke = b.fire(smoke_job).expect("delegates");
    b.push(
        Actor::Builder,
        ProjectEvent::Job {
            job: smoke_job,
            event: JobEvent::Cancelled { by: Actor::Builder },
        },
    );

    // Tech deploys, sends the API back for more effort and does the smoke tests itself.
    b.turn(dt, 80_000, |b| {
        b.action_event(Actor::Daemon, deploy, ActionEvent::CarriedOut);
        let used = CredentialUse {
            credential: CredentialRef::from_u128(21),
            agent: tech,
            driver: dt,
            via: Via::Wrapper {
                command: "wrangler".into(),
            },
            outcome: UseOutcome::Injected,
        };
        b.push(Actor::Daemon, ProjectEvent::CredentialUsed(used));
        b.send_back(
            dt,
            api,
            "Add rate limiting to the public routes.",
            Some(Effort::High),
        );
        b.report(dt, smoke, "Smoke tests pass.");
        b.wait(
            dt,
            site,
            WaitOn::Report(api),
            "Deployed; API back with Backend.",
            75,
        );
    });

    // Backend's driver no longer fits the raised effort, so its work is handed off on purpose.
    let reason = PlannedReason::EffortChanged;
    b.driver_event(db2, DriverEvent::HandoffStarted(reason));
    b.driver_event(db2, DriverEvent::TurnEnded);
    let handoff = Handoff {
        replaces: db2,
        cause: HandoffCause::Planned(reason),
        in_flight: InFlight::Nothing,
    };
    let db3 = b.start_driver(backend, opus(), Effort::High, StartReason::Handoff(handoff));
    b.turn(db3, 140_000, |b| {
        b.update(db3, api, "Rate limiting on two of three routes.", 80)
    });

    // Marketing's email out is held and denied, and it reaches its budget.
    let dm = b.start_driver(
        marketing,
        sonnet(),
        Effort::Low,
        StartReason::NewTask { task: copy },
    );
    let mut email = deploy;
    b.turn(dm, 2_000_000, |b| {
        let request = Request::Egress {
            driver: dm,
            host: "api.mailer.example".into(),
            method: "POST".into(),
        };
        email = b.propose(marketing, request);
        let call = b.id(OneOffCallId::new);
        b.push(
            Actor::Daemon,
            ProjectEvent::OneOffCalled(OneOffCall {
                id: call,
                purpose: OneOffPurpose::Classify { action: email },
                provider: DecisionProvider::Gemini,
                model: ModelName("gemini-decision-1".into()),
            }),
        );
        b.push(
            Actor::Daemon,
            ProjectEvent::Charged(Charge {
                payer: Payer::Lead(marketing),
                source: ChargeSource::OneOff { call },
                usage: Usage {
                    input: 900,
                    ..Usage::default()
                },
                cost: Money::from_micros(90),
                basis: CostBasis::Listed,
            }),
        );
        let by = ClassifiedBy::Classifier(call);
        let why = "Sends email to people outside the project.";
        b.classify(email, ActionKind::SendOutside, Risk::High, by, why);
        b.action_event(Actor::Daemon, email, ActionEvent::Held);
        b.wait(
            dm,
            copy,
            WaitOn::Decision(email),
            "Copy drafted; email held.",
            80,
        );
    });
    b.decide(email, Decision::Denied);
    b.turn(dm, 3_000_000, |b| {
        b.report(dm, copy, "Launch copy written; no email was sent.")
    });
    b.push(
        Actor::Daemon,
        ProjectEvent::BudgetReached(BudgetReached::Lead(marketing)),
    );
    b.deliver(ceo, InboxItem::BudgetReached { lead: marketing });
    b.went_idle(marketing);

    // The CEO accepts the smoke tests, asks about Marketing's budget, and waits on the answer.
    let mut budget_question = question;
    b.turn(dc, 60_000, |b| {
        let merge = b.accept(dc, smoke);
        b.merge(merge, smoke, None);
        budget_question = b.message(
            dc,
            Recipient::Builder,
            "Marketing hit its $5. Raise it?",
            None,
        );
        let on = WaitOn::Reply {
            message: budget_question,
            from: Party::Builder,
        };
        b.wait(
            dc,
            brief,
            on,
            "Copy in review; Marketing at its budget.",
            60,
        );
    });

    // The builder tidies up: a file removed, Tech's budget raised, and asked about more.
    b.push(
        Actor::Builder,
        ProjectEvent::FileRemoved {
            file: competitors_png,
        },
    );
    b.push(
        Actor::Builder,
        ProjectEvent::Agent {
            agent: tech,
            event: AgentEvent::BudgetSet(Money::from_dollars(18)),
        },
    );
    b.push(
        Actor::Builder,
        ProjectEvent::SettingChanged(SettingChange::AskMeAbout(
            crate::action::AskMeAbout::MediumAndUp,
        )),
    );

    Fernleaf {
        log: b.finish(),
        ceo,
        tech,
        product,
        marketing,
        backend,
        frontend,
        migrations,
        research,
        brief,
        site,
        prd,
        copy,
        api,
        landing,
        schema,
        competitors,
        bug,
        smoke,
        question,
        answer,
        budget_question,
        lead_question,
        deploy,
        email,
        smoke_job,
        recheck_job,
        sketch,
        competitors_png,
        screenshot,
        failed_driver: db,
        takeover_driver: db2,
        backend_driver: db3,
        committed_trial,
        discarded_trial,
    }
}
