//! Table tests: every tool, merge, egress and command row, at every level.

mod common;

use common::*;
use x100_core::action::{ActionKind, AskMeAbout, Decision, Risk};
use x100_core::effort::{Effort, EffortRange};
use x100_core::id::JobId;
use x100_core::job::{Interval, Schedule};
use x100_core::money::Money;
use x100_core::process::ProcessName;
use x100_core::spec::{SpecKind, SpecStatus};
use x100_core::task::{Progress, Verdict};
use x100_core::team::{LeadSpec, TeamChange};
use x100_core::time::Timestamp;
use x100_core::tool::{
    AgentOutcome, Delegated, Delegatee, JobRequest, Proposal, Recipient, ToolCall,
};
use x100_core::wait::WaitOn;
use x100_gate::{holds, rule};

fn progress() -> Progress {
    Progress {
        note: "n".into(),
        done: None,
    }
}

fn lead(budget: Money) -> LeadSpec {
    LeadSpec {
        role: "Tech".into(),
        charter: "c".into(),
        effort: EffortRange::FULL,
        budget,
    }
}

/// One call of every variant, each with the row it should fall under.
fn every_tool(f: &x100_core::testing::Fernleaf) -> Vec<(ToolCall, &'static str)> {
    let t = f.landing;
    let child = Delegatee::Child(f.backend);
    let new_task = || Delegated::New {
        title: "t".into(),
        ask: "a".into(),
        attachments: vec![],
    };
    vec![
        (
            ToolCall::Delegate {
                to: child.clone(),
                task: new_task(),
                effort: Effort::High,
            },
            "tool.delegate",
        ),
        (
            ToolCall::Review {
                task: t,
                verdict: Verdict::Accept,
            },
            "tool.routine",
        ),
        (
            ToolCall::Report {
                task: t,
                outcome: AgentOutcome::Completed,
                summary: "s".into(),
            },
            "tool.routine",
        ),
        (
            ToolCall::Message {
                to: Recipient::Builder,
                body: "b".into(),
                attachments: vec![],
                in_reply_to: None,
            },
            "tool.routine",
        ),
        (
            ToolCall::Wait {
                on: WaitOn::Report(t),
                deadline: None,
                task: t,
                progress: progress(),
            },
            "tool.routine",
        ),
        (
            ToolCall::Schedule {
                when: Schedule::Once {
                    at: Timestamp::from_millis(1),
                },
                does: JobRequest::Wake {
                    agent: f.backend,
                    note: "n".into(),
                },
            },
            "tool.schedule.once",
        ),
        (
            ToolCall::CancelJob {
                job: JobId::from_u128(1),
            },
            "tool.routine",
        ),
        (
            ToolCall::RaiseTask {
                title: "t".into(),
                ask: "a".into(),
                part: None,
                attachments: vec![],
            },
            "tool.routine",
        ),
        (
            ToolCall::UpdateTask {
                task: t,
                progress: progress(),
                effort: None,
            },
            "tool.routine",
        ),
        (
            ToolCall::WriteSpec {
                spec: SpecKind::Brd,
                status: SpecStatus::Draft,
                body: "b".into(),
            },
            "tool.routine",
        ),
        (
            ToolCall::AddFile {
                name: "n".into(),
                media_type: "image/png".into(),
                size: 1,
                hash: x100_core::file::ContentHash("h".into()),
            },
            "tool.routine",
        ),
        (
            ToolCall::SetPreview {
                url: "http://localhost:3000".into(),
            },
            "tool.routine",
        ),
        (
            ToolCall::LoadSkill {
                skill: x100_core::testing::deploy_skill(),
            },
            "tool.routine",
        ),
        (
            ToolCall::StartProcess {
                name: ProcessName("dev".into()),
                command: "npm run dev".into(),
            },
            "tool.start_process",
        ),
        (
            ToolCall::StopProcess {
                name: ProcessName("dev".into()),
            },
            "tool.routine",
        ),
        (
            ToolCall::Remember {
                note: x100_core::memory::NoteName("n".into()),
                body: "b".into(),
                sources: vec![],
            },
            "tool.routine",
        ),
        (
            ToolCall::ReadNote {
                note: x100_core::memory::NoteName("n".into()),
            },
            "tool.recall",
        ),
        (ToolCall::SearchNotes { query: "q".into() }, "tool.recall"),
        (
            ToolCall::SearchLog {
                query: "q".into(),
                limit: None,
            },
            "tool.recall",
        ),
        (
            ToolCall::FormTeam {
                leads: vec![lead(Money::from_dollars(5))],
            },
            "tool.team_change",
        ),
        (
            ToolCall::SetEffort {
                agent: f.backend,
                range: EffortRange::FULL,
            },
            "tool.set_effort.not_raised",
        ),
        (
            ToolCall::SetBudget {
                lead: f.tech,
                budget: Money::ZERO,
            },
            "tool.set_budget.not_raised",
        ),
        (
            ToolCall::ReadLog {
                from: x100_core::event::Seq(1),
                to: None,
            },
            "tool.routine",
        ),
        (ToolCall::ReadMemories { agent: f.backend }, "tool.routine"),
        (
            ToolCall::ProposeChange(Proposal::Team {
                changes: vec![TeamChange::Retire { agent: f.research }],
                telemetry: "t".into(),
                team_toml: String::new(),
            }),
            "tool.team_change",
        ),
        (
            ToolCall::SetUpProject {
                name: "n".into(),
                intent: "i".into(),
                brief: None,
            },
            "tool.routine",
        ),
        (ToolCall::ListIntegrations {}, "tool.list_integrations"),
        (
            ToolCall::LinkProject {
                homebox_project: "foobar".into(),
                environment: None,
            },
            "tool.link_project",
        ),
        (
            ToolCall::Deploy { environment: None },
            "tool.deploy.publish_first",
        ),
    ]
}

/// Maps every variant to a number with no wildcard, so a new `ToolCall` variant stops this test
/// from compiling until it is added here and given a case in `every_tool`.
fn variant_index(call: &ToolCall) -> usize {
    match call {
        ToolCall::Delegate { .. } => 0,
        ToolCall::Review { .. } => 1,
        ToolCall::Report { .. } => 2,
        ToolCall::Message { .. } => 3,
        ToolCall::Wait { .. } => 4,
        ToolCall::Schedule { .. } => 5,
        ToolCall::CancelJob { .. } => 6,
        ToolCall::RaiseTask { .. } => 7,
        ToolCall::UpdateTask { .. } => 8,
        ToolCall::WriteSpec { .. } => 9,
        ToolCall::AddFile { .. } => 10,
        ToolCall::SetPreview { .. } => 11,
        ToolCall::LoadSkill { .. } => 12,
        ToolCall::StartProcess { .. } => 13,
        ToolCall::StopProcess { .. } => 14,
        ToolCall::Remember { .. } => 15,
        ToolCall::FormTeam { .. } => 16,
        ToolCall::SetEffort { .. } => 17,
        ToolCall::SetBudget { .. } => 18,
        ToolCall::ReadLog { .. } => 19,
        ToolCall::ReadMemories { .. } => 20,
        ToolCall::ProposeChange(_) => 21,
        ToolCall::SetUpProject { .. } => 22,
        ToolCall::ListIntegrations {} => 23,
        ToolCall::LinkProject { .. } => 24,
        ToolCall::Deploy { .. } => 25,
        ToolCall::ReadNote { .. } => 26,
        ToolCall::SearchNotes { .. } => 27,
        ToolCall::SearchLog { .. } => 28,
    }
}

#[test]
fn every_tool_variant_has_a_rule() {
    let (state, f) = fern(AskMeAbout::HighRiskOnly);
    let calls = every_tool(&f);
    let mut seen: Vec<usize> = calls.iter().map(|(c, _)| variant_index(c)).collect();
    seen.sort_unstable();
    assert_eq!(seen, (0..29).collect::<Vec<_>>(), "a case per variant");
    for (call, expected) in calls {
        let r = ruling(&state, &tool(f.backend, call.clone()));
        assert_eq!(rule_of(&r).as_deref(), Some(expected), "{call:?}");
    }
}

#[test]
fn a_ruling_is_held_exactly_when_the_level_holds_its_risk() {
    for level in LEVELS {
        let (state, f) = fern(level);
        for (call, _) in every_tool(&f) {
            let r = ruling(&state, &tool(f.backend, call.clone()));
            let c = r.classification();
            let expected = level.holds(c.risk) || c.kind == ActionKind::TeamChange;
            assert_eq!(r.is_held(), expected, "{level:?} {call:?} {c:?}");
            assert_eq!(r.is_held(), holds(level, c));
        }
    }
}

#[test]
fn the_team_after_the_first_is_held_at_every_level() {
    for level in LEVELS {
        let (state, f) = fern(level);
        let again = ToolCall::FormTeam {
            leads: vec![lead(Money::from_dollars(1))],
        };
        assert!(ruling(&state, &tool(f.ceo, again)).is_held(), "{level:?}");
        let propose = ToolCall::ProposeChange(Proposal::Team {
            changes: vec![TeamChange::Retire { agent: f.research }],
            telemetry: "t".into(),
            team_toml: String::new(),
        });
        assert!(ruling(&state, &tool(f.ceo, propose)).is_held(), "{level:?}");
    }
}

#[test]
fn the_first_team_is_not_held_until_every_decision() {
    for level in LEVELS {
        let (state, ceo) = bare(level);
        let first = ToolCall::FormTeam {
            leads: vec![lead(Money::from_dollars(5))],
        };
        let r = ruling(&state, &tool(ceo, first));
        assert_eq!(rule_of(&r).as_deref(), Some("tool.form_team.first"));
        assert_eq!(r.is_held(), level == AskMeAbout::EveryDecision, "{level:?}");
    }
}

#[test]
fn schedules_budgets_and_efforts_read_the_state() {
    let (state, f) = fern(AskMeAbout::MediumAndUp);
    let at = |s| {
        tool(
            f.ceo,
            ToolCall::Schedule {
                when: s,
                does: JobRequest::Wake {
                    agent: f.backend,
                    note: "n".into(),
                },
            },
        )
    };
    let every = |mins: u64| Schedule::Every {
        first: Timestamp::from_millis(1),
        every: Interval::new(std::time::Duration::from_secs(mins * 60)).expect("not zero"),
    };
    assert_eq!(
        rule_of(&ruling(&state, &at(every(60)))).as_deref(),
        Some("tool.schedule.repeating")
    );
    assert_eq!(
        rule_of(&ruling(&state, &at(every(5)))).as_deref(),
        Some("tool.schedule.repeating_fast")
    );
    assert!(
        ruling(&state, &at(every(60))).is_held(),
        "Medium is held at Medium and up"
    );

    // Budgets: raising a lead's is spend, lowering it is not, and passing the project's is High.
    let budget = |b| {
        tool(
            f.ceo,
            ToolCall::SetBudget {
                lead: f.tech,
                budget: b,
            },
        )
    };
    let now = state
        .project()
        .and_then(|p| p.team.lead_budgets().find(|(a, _)| *a == f.tech))
        .map(|(_, b)| b)
        .expect("tech is a lead");
    let up = Money::from_micros(now.micros() + 1);
    let r = ruling(&state, &budget(up));
    assert!(matches!(
        rule_of(&r).as_deref(),
        Some("tool.set_budget.raised" | "tool.set_budget.over_project")
    ));
    assert!(r.is_held());
    let r = ruling(&state, &budget(Money::from_dollars(100_000)));
    assert_eq!(rule_of(&r).as_deref(), Some("tool.set_budget.over_project"));
    assert_eq!(r.classification().risk, Risk::High);
    assert_eq!(
        rule_of(&ruling(&state, &budget(now))).as_deref(),
        Some("tool.set_budget.not_raised")
    );

    // Effort: a range above the agent's is spend; the same range is not.
    let narrow = EffortRange::new(Effort::Low, Effort::Medium).expect("a range");
    let effort = |r| {
        tool(
            f.ceo,
            ToolCall::SetEffort {
                agent: f.tech,
                range: r,
            },
        )
    };
    assert_eq!(
        rule_of(&ruling(&state, &effort(narrow))).as_deref(),
        Some("tool.set_effort.not_raised")
    );
    let current = state
        .project()
        .and_then(|p| p.team.effort(f.tech))
        .expect("a range");
    let r = ruling(&state, &effort(current));
    assert_eq!(rule_of(&r).as_deref(), Some("tool.set_effort.not_raised"));
}

#[test]
fn delegating_at_max_effort_is_spend() {
    let (state, f) = fern(AskMeAbout::MediumAndUp);
    let delegate = |effort| {
        tool(
            f.ceo,
            ToolCall::Delegate {
                to: Delegatee::Child(f.backend),
                task: Delegated::New {
                    title: "t".into(),
                    ask: "a".into(),
                    attachments: vec![],
                },
                effort,
            },
        )
    };
    assert!(ruling(&state, &delegate(Effort::Max)).is_held());
    assert!(!ruling(&state, &delegate(Effort::High)).is_held());
    let worker = tool(
        f.ceo,
        ToolCall::Delegate {
            to: Delegatee::NewWorker {
                role: "r".into(),
                charter: "c".into(),
                effort: Some(EffortRange::FULL),
            },
            task: Delegated::New {
                title: "t".into(),
                ask: "a".into(),
                attachments: vec![],
            },
            effort: Effort::Low,
        },
    );
    assert_eq!(
        rule_of(&ruling(&state, &worker)).as_deref(),
        Some("tool.delegate.max_effort")
    );
}

#[test]
fn a_started_process_is_read_as_a_command() {
    let (state, f) = fern(AskMeAbout::HighRiskOnly);
    let start = |command: &str| {
        tool(
            f.backend,
            ToolCall::StartProcess {
                name: ProcessName("p".into()),
                command: command.into(),
            },
        )
    };
    assert_eq!(
        rule_of(&ruling(&state, &start("npm run dev"))).as_deref(),
        Some("tool.start_process")
    );
    let r = ruling(&state, &start("sudo npm run dev"));
    assert_eq!(rule_of(&r).as_deref(), Some("command.sudo"));
    assert!(r.is_held());
}

#[test]
fn merges_into_main_differ_from_merges_between_branches() {
    // The brief is the builder's task for the CEO, so accepting it merges into main. The landing
    // page was delegated between agents.
    for level in LEVELS {
        let (state, f) = fern(level);
        let main = ruling(&state, &merge(f.ceo, f.brief));
        let branch = ruling(&state, &merge(f.tech, f.landing));
        assert_eq!(rule_of(&main).as_deref(), Some("merge.main"));
        assert_eq!(rule_of(&branch).as_deref(), Some("merge.branch"));
        assert_eq!(main.classification().kind, ActionKind::Merge);
        assert_eq!(
            main.is_held(),
            matches!(level, AskMeAbout::MediumAndUp | AskMeAbout::EveryDecision)
        );
        assert_eq!(branch.is_held(), level == AskMeAbout::EveryDecision);
    }
    let (state, f) = fern(AskMeAbout::MediumAndUp);
    let unknown = ruling(
        &state,
        &merge(f.ceo, x100_core::id::TaskId::from_u128(999_999)),
    );
    assert_eq!(rule_of(&unknown).as_deref(), Some("merge.main"));
}

#[test]
fn egress_is_ruled_by_host_and_method() {
    let (state, f) = fern(AskMeAbout::HighRiskOnly);
    let cases = [
        ("registry.npmjs.org", "GET", "egress.registry.read", false),
        ("registry.npmjs.org", "POST", "egress.registry.write", false),
        ("registry.npmjs.org", "PUT", "egress.registry.publish", true),
        ("crates.io:443", "put", "egress.registry.publish", true),
        ("github.com", "GET", "egress.forge.read", false),
        ("api.github.com", "POST", "egress.forge.write", false),
        ("api.github.com", "DELETE", "egress.forge.delete", true),
        ("hooks.slack.com", "POST", "egress.messaging.write", true),
        ("api.sendgrid.com", "POST", "egress.messaging.write", true),
        ("api.vercel.com", "POST", "egress.deploy.write", true),
        ("api.vercel.com", "GET", "egress.deploy.read", false),
        ("api.stripe.com", "POST", "egress.payment.write", true),
        ("api.stripe.com", "GET", "egress.payment.read", false),
        ("oauth2.googleapis.com", "POST", "egress.auth.write", true),
        // A host that only ends the same way is not that host.
        ("notgithub.com", "GET", "", false),
    ];
    for (host, method, rule_id, held) in cases {
        let a = egress(f.backend, host, method);
        if rule_id.is_empty() {
            assert!(asks(&state, &a), "{host}");
            continue;
        }
        let r = ruling(&state, &a);
        assert_eq!(rule_of(&r).as_deref(), Some(rule_id), "{method} {host}");
        assert_eq!(r.is_held(), held, "{method} {host}");
    }
}

/// What x100 settled itself, the driver's own provider and a host the proxy refuses, is recorded
/// as allowed at every level, under a row of the table, and never asks the classifier.
#[test]
fn egress_x100_settled_is_recorded_under_its_own_rows() {
    for (how, id) in [
        (x100_gate::Settled::Provider, "egress.own_provider"),
        (
            x100_gate::Settled::RefusedByProxy,
            "egress.refused_by_proxy",
        ),
    ] {
        let r = x100_gate::settled(how, "API.Example.com:443", "post");
        assert!(!r.is_held());
        assert_eq!(rule_of(&r).as_deref(), Some(id));
        assert!(r.classification().why.contains("POST api.example.com"));
        assert!(rule::ALL.iter().any(|row| row.id == id));
    }
}

#[test]
fn a_new_host_goes_to_the_classifier_and_a_host_the_builder_approved_does_not() {
    let (mut state, f) = fern(AskMeAbout::HighRiskOnly);
    let new = egress(f.backend, "api.acme.example", "POST");
    assert!(asks(&state, &new));
    assert!(asks(&state, &egress(f.backend, "api.acme.example", "GET")));

    // A request the classifier let through does not make the host familiar...
    let first = egress(f.backend, "api.acme.example", "GET");
    decide(&mut state, &first, Decision::Denied);
    assert!(
        asks(&state, &new),
        "a denied request makes nothing familiar"
    );

    // ...a request the builder approved does.
    let approved = egress(f.backend, "API.acme.example.", "POST");
    decide(&mut state, &approved, Decision::Approved);
    let r = ruling(&state, &new);
    assert_eq!(rule_of(&r).as_deref(), Some("egress.seen_host.write"));
    assert!(!r.is_held(), "Medium is not held at High risk only");
    let r = ruling(&state, &egress(f.backend, "api.acme.example", "GET"));
    assert_eq!(rule_of(&r).as_deref(), Some("egress.seen_host.read"));
}

#[test]
fn the_rule_table_has_unique_ids_and_every_ruling_names_one() {
    let mut ids: Vec<_> = rule::ALL.iter().map(|r| r.id).collect();
    let n = ids.len();
    ids.sort_unstable();
    ids.dedup();
    assert_eq!(ids.len(), n, "ids are unique");
    let (state, f) = fern(AskMeAbout::HighRiskOnly);
    for (call, _) in every_tool(&f) {
        let r = ruling(&state, &tool(f.backend, call));
        assert!(
            rule::ALL
                .iter()
                .any(|row| Some(row.id) == rule_of(&r).as_deref())
        );
    }
}

/// Prints the table for the report: `cargo test -p x100-gate print_the_table -- --nocapture`.
#[test]
fn print_the_table() {
    for row in rule::ALL {
        println!("{row}\n    {}", row.why);
    }
}
