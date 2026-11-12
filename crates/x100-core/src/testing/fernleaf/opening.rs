//! The scenario's opening: the builder's first message, the project set up, the team formed, workers started, a question answered, and
//! the PRD accepted with two trials run.

use super::{MINUTE, deploy_skill, range};
use crate::agent::{Agent, Position};
use crate::driver::{DriverEvent, RetireReason, StartReason};
use crate::effort::{Effort, EffortRange};
use crate::event::ProjectEvent;
use crate::file::{ContentHash, FileRef};
use crate::id::{AgentId, DriverId, FileId, JobId, MessageId, TaskId, TrialId};
use crate::inbox::InboxItem;
use crate::job::{Interval, Schedule};
use crate::memory::{SkillName, SkillRef, SkillScope};
use crate::message::{Message, Route};
use crate::money::{Money, Percent};
use crate::party::{Actor, Party};
use crate::project::{Identity, Settings};
use crate::spec::{SpecKind, SpecStatus};
use crate::task::{Assignment, Opening as TaskOpening, Task};
use crate::team::LeadSpec;
use crate::testing::{LogBuilder, gpt, opus, sonnet};
use crate::tool::{JobRequest, Recipient, ToolCall};
use crate::trial::TrialChange;
use crate::wait::WaitOn;

/// What the rest of the scenario needs from its opening.
pub(super) struct Opening {
    pub(super) ceo: AgentId,
    pub(super) tech: AgentId,
    pub(super) product: AgentId,
    pub(super) marketing: AgentId,
    pub(super) backend: AgentId,
    pub(super) frontend: AgentId,
    pub(super) research: AgentId,
    pub(super) brief: TaskId,
    pub(super) site: TaskId,
    pub(super) prd: TaskId,
    pub(super) copy: TaskId,
    pub(super) api: TaskId,
    pub(super) landing: TaskId,
    pub(super) competitors: TaskId,
    pub(super) question: MessageId,
    pub(super) answer: MessageId,
    pub(super) lead_question: MessageId,
    pub(super) smoke_job: JobId,
    pub(super) recheck_job: JobId,
    pub(super) sketch: FileId,
    pub(super) competitors_png: FileId,
    pub(super) dc: DriverId,
    pub(super) dt: DriverId,
    pub(super) committed_trial: TrialId,
    pub(super) discarded_trial: TrialId,
}

pub(super) fn opening(b: &mut LogBuilder) -> Opening {
    let ceo = b.id(AgentId::new);
    let settings = Settings {
        ask_me_about: crate::action::AskMeAbout::HighRiskOnly,
        budget: Money::from_dollars(50),
        lab_share: Percent::new(10).expect("a percentage"),
        allowed: [opus(), sonnet(), gpt()].into(),
    };
    b.push(
        Actor::Builder,
        ProjectEvent::Created {
            ceo,
            charter: "Own Fernleaf's launch, from brief to a live site.".into(),
            effort: EffortRange::FULL,
            settings,
        },
    );

    // The builder's first message, with a sketch. The project is not set up, so it opens no task.
    let brief_text = "A landing page for Fernleaf, a plant shop, with a waitlist. Sketch attached.";
    let brief_message = b.id(MessageId::new);
    let sketch = b.id(FileId::new);
    b.push(
        Actor::Builder,
        ProjectEvent::FileAdded(FileRef {
            id: sketch,
            name: "sketch.png".into(),
            media_type: "image/png".into(),
            size: 48_213,
            hash: ContentHash("sha256:sketch".into()),
            added_by: Party::Builder,
            attached_to: Some(brief_message),
        }),
    );
    b.push(
        Actor::Builder,
        ProjectEvent::MessageSent(Message {
            id: brief_message,
            route: Route::BuilderToCeo,
            body: brief_text.into(),
            attachments: vec![sketch],
            in_reply_to: None,
        }),
    );
    b.deliver(
        ceo,
        InboxItem::Message {
            message: brief_message,
        },
    );

    // The CEO, on a driver in a scratch directory, sets the project up, which opens the builder's
    // task. That driver cannot move into the new code directory, so it retires with the turn.
    let scratch = b.start_driver(ceo, opus(), Effort::Low, StartReason::Wake);
    let brief = b.id(TaskId::new);
    b.turn(scratch, 30_000, |b| {
        let call = ToolCall::SetUpProject {
            name: "Fernleaf".into(),
            intent: "A waitlist page for Fernleaf, a plant shop.".into(),
            brief: Some(brief_text.into()),
        };
        b.tool(scratch, call, |b| {
            let by = Actor::Agent(ceo);
            let dir = "/home/builder/x100/fernleaf".into();
            b.push(by, ProjectEvent::CodeDirSet { dir });
            b.push(
                by,
                ProjectEvent::SetUp(Identity {
                    name: "Fernleaf".into(),
                    intent: "A waitlist page for Fernleaf, a plant shop.".into(),
                }),
            );
            b.push(
                by,
                ProjectEvent::TaskOpened {
                    task: Task {
                        id: brief,
                        title: "Launch Fernleaf".into(),
                        ask: brief_text.into(),
                        part: None,
                        raised_by: Party::Builder,
                        attachments: vec![sketch],
                    },
                    opening: TaskOpening::Delegated(Assignment {
                        delegator: Party::Builder,
                        assignee: ceo,
                        effort: Effort::High,
                    }),
                },
            );
            b.deliver(ceo, InboxItem::Task { task: brief });
        });
    });
    b.push(
        Actor::Daemon,
        ProjectEvent::Driver {
            driver: scratch,
            event: DriverEvent::Retired(RetireReason::ProjectSetUp),
        },
    );

    // The CEO's first turn: BRD, team, two delegations, and a question for the builder.
    let dc = b.start_driver(
        ceo,
        opus(),
        Effort::High,
        StartReason::NewTask { task: brief },
    );
    let mut leads = Vec::new();
    let (mut site, mut prd, mut question) = (brief, brief, brief_message);
    b.turn(dc, 420_000, |b| {
        b.message(
            dc,
            Recipient::Builder,
            "On it: BRD first, then a team.",
            None,
        );
        b.write_spec(
            dc,
            SpecKind::Brd,
            SpecStatus::Draft,
            "# BRD\nA waitlist page.",
        );
        let specs = [
            ("Tech", range(Effort::Medium, Effort::Max), 15),
            ("Product", range(Effort::Low, Effort::High), 8),
            ("Marketing", range(Effort::Low, Effort::Medium), 5),
        ]
        .map(|(role, effort, dollars)| LeadSpec {
            role: role.into(),
            charter: format!("{role} for Fernleaf."),
            effort,
            budget: Money::from_dollars(dollars),
        });
        let call = ToolCall::FormTeam {
            leads: specs.to_vec(),
        };
        b.tool(dc, call, |b| {
            for spec in specs {
                let id = b.id(AgentId::new);
                let agent = Agent {
                    id,
                    role: spec.role,
                    charter: spec.charter,
                    position: Position::Lead {
                        effort: spec.effort,
                        budget: spec.budget,
                    },
                };
                b.push(Actor::Agent(ceo), ProjectEvent::AgentAdded(agent));
                leads.push(id);
            }
        });
        site = b.delegate(
            dc,
            leads[0],
            "Build the site",
            "Astro site with a waitlist.",
            Effort::Medium,
        );
        prd = b.delegate(dc, leads[1], "Write the PRD", "From the BRD.", Effort::Low);
        question = b.message(
            dc,
            Recipient::Builder,
            "Forest greens or ocean blues?",
            None,
        );
        let on = WaitOn::Reply {
            message: question,
            from: Party::Builder,
        };
        b.wait(
            dc,
            brief,
            on,
            "BRD drafted, team formed. Waiting on the palette.",
            15,
        );
    });
    let [tech, product, marketing] = [leads[0], leads[1], leads[2]];

    // Tech starts two workers.
    let dt = b.start_driver(
        tech,
        gpt(),
        Effort::Medium,
        StartReason::NewTask { task: site },
    );
    let (mut api, mut backend, mut landing, mut frontend) = (site, tech, site, tech);
    b.turn(dt, 310_000, |b| {
        b.load_skill(dt, deploy_skill(), 2);
        b.write_spec(
            dt,
            SpecKind::Add,
            SpecStatus::Draft,
            "# ADD\nAstro, Tailwind, Workers.",
        );
        (api, backend) = b.delegate_new(
            dt,
            "Backend",
            "Build the API",
            "Waitlist API.",
            Effort::Medium,
        );
        (landing, frontend) = b.delegate_new(
            dt,
            "Frontend",
            "Build the landing page",
            "From the sketch.",
            Effort::High,
        );
        b.wait(
            dt,
            site,
            WaitOn::Report(landing),
            "API and landing page delegated.",
            10,
        );
    });

    // Product starts Research and asks Tech directly.
    let dp = b.start_driver(
        product,
        sonnet(),
        Effort::Low,
        StartReason::NewTask { task: prd },
    );
    let (mut competitors, mut research, mut lead_question) = (prd, product, question);
    b.turn(dp, 120_000, |b| {
        (competitors, research) = b.delegate_new(
            dp,
            "Research",
            "Research competitors",
            "Three plant shops.",
            Effort::Low,
        );
        lead_question = b.message(
            dp,
            Recipient::Agent(tech),
            "Which framework is the site on?",
            None,
        );
        let on = WaitOn::Reply {
            message: lead_question,
            from: Party::Agent(tech),
        };
        b.wait(
            dp,
            prd,
            on,
            "Outline done; waiting on Tech for the stack.",
            20,
        );
    });
    b.turn(dt, 60_000, |b| {
        b.message(
            dt,
            Recipient::Agent(product),
            "Astro with Tailwind.",
            Some(lead_question),
        );
        b.wait(
            dt,
            site,
            WaitOn::Report(api),
            "Waiting on Backend and Frontend.",
            10,
        );
    });
    b.turn(dp, 90_000, |b| {
        b.write_spec(
            dp,
            SpecKind::Prd,
            SpecStatus::Draft,
            "# PRD\nWaitlist, plant grid.",
        );
        b.wait(
            dp,
            prd,
            WaitOn::Report(competitors),
            "PRD drafted; waiting on research.",
            50,
        );
    });

    // The builder answers. The CEO approves the BRD, starts Marketing and schedules smoke tests.
    b.advance(20 * MINUTE);
    let answer = b.builder_says("Forest greens.", Some(question));
    let mut smoke_job = JobId::from_u128(0);
    let mut copy = brief;
    b.turn(dc, 180_000, |b| {
        b.write_spec(
            dc,
            SpecKind::Brd,
            SpecStatus::Approved,
            "# BRD\nForest greens.",
        );
        copy = b.delegate(
            dc,
            marketing,
            "Write the launch copy",
            "Short and warm.",
            Effort::Low,
        );
        let when = Schedule::Every {
            first: b.now() + 60 * MINUTE,
            every: Interval::new(24 * 60 * MINUTE).expect("not zero"),
        };
        let does = JobRequest::Delegate {
            to: tech,
            title: "Run the smoke tests".into(),
            ask: "Every morning.".into(),
            effort: Effort::Low,
        };
        smoke_job = b.schedule(dc, when, does);
        b.wait(
            dc,
            brief,
            WaitOn::Report(site),
            "BRD approved; smoke tests daily.",
            30,
        );
    });

    // Research waits on a job of its own, across its driver's warm period.
    let dr = b.start_driver(
        research,
        sonnet(),
        Effort::Low,
        StartReason::NewTask { task: competitors },
    );
    let mut competitors_png = sketch;
    let mut recheck_job = smoke_job;
    b.turn(dr, 40_000, |b| {
        competitors_png = b.add_file(dr, "competitors.png", 812_331);
        let when = Schedule::Once {
            at: b.now() + 60 * MINUTE,
        };
        let does = JobRequest::Wake {
            agent: research,
            note: "Pricing pages update at noon; look again.".into(),
        };
        recheck_job = b.schedule(dr, when, does);
        b.wait(
            dr,
            competitors,
            WaitOn::Job(recheck_job),
            "Three found; pricing to recheck.",
            70,
        );
    });
    b.warm_period_ends(dr, 5 * MINUTE);
    b.advance(60 * MINUTE);
    b.fire(recheck_job);
    let dr2 = b.start_driver(research, sonnet(), Effort::Low, StartReason::Wake);
    b.turn(dr2, 30_000, |b| {
        b.report(dr2, competitors, "Two of three price per seat.")
    });
    b.went_idle(research);
    b.warm_period_ends(dr2, 5 * MINUTE);

    // Product finishes; the CEO accepts, and its driver is compacted after the accepted task.
    b.turn(dp, 70_000, |b| {
        let merge = b.accept(dp, competitors);
        b.merge(merge, competitors, None);
        b.write_spec(dp, SpecKind::Prd, SpecStatus::Approved, "# PRD\nApproved.");
        b.report(dp, prd, "PRD approved.");
    });
    b.went_idle(product);
    let (mut committed_trial, mut discarded_trial) = (TrialId::from_u128(0), TrialId::from_u128(0));
    b.turn(dc, 50_000, |b| {
        let merge = b.accept(dc, prd);
        b.merge(merge, prd, None);
        let edit = TrialChange::EditSkill {
            skill: deploy_skill(),
            body: "Deploy to staging first.".into(),
        };
        committed_trial = b.trial(dc, tech, edit, true);
        let new = TrialChange::NewSkill {
            skill: SkillRef {
                scope: SkillScope::Agent(frontend),
                name: SkillName("responsive-check".into()),
            },
            body: "Check three widths.".into(),
        };
        discarded_trial = b.trial(dc, frontend, new, false);
        b.wait(
            dc,
            brief,
            WaitOn::Report(copy),
            "PRD in; one skill change committed.",
            40,
        );
    });
    b.driver_event(dp, DriverEvent::CompactionStarted);
    b.driver_event(dp, DriverEvent::Compacted);

    Opening {
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
    }
}
