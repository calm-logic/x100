//! Resuming a held action, the "allowed once" record, and what the builder is shown.

mod common;

use common::*;
use x100_core::action::{ActionKind, AskMeAbout, ClassifiedBy, Decision, Request, Risk};
use x100_core::id::{AgentId, DriverId, TaskId};
use x100_core::inbox::InboxItem;
use x100_gate::{AllowedOnce, AtOnce, Effect, Shown, resume, tell_agent};

fn some_agent_with_a_task() -> (x100_core::fold::ProjectState, AgentId, TaskId) {
    let (state, _) = fern(AskMeAbout::HighRiskOnly);
    let (agent, task) = state
        .agents()
        .find_map(|a| a.open_tasks.last().map(|t| (a.id, *t)))
        .expect("fernleaf ends with an agent that has a task");
    (state, agent, task)
}

#[test]
fn each_kind_of_request_resumes_its_own_way() {
    let a = AgentId::from_u128(1);
    let tool_call = tool(
        a,
        x100_core::tool::ToolCall::ReadLog {
            from: x100_core::event::Seq(1),
            to: None,
        },
    );
    let merge = merge(a, TaskId::from_u128(1));
    let table = [
        (
            tool_call.request,
            AtOnce::ToolReplyHeld,
            Effect::RunToolCall,
            Effect::Drop,
        ),
        (
            command(a, "git push -f").request,
            AtOnce::HookDenialSaysHeld,
            Effect::AllowCommandOnce,
            Effect::Drop,
        ),
        (
            egress(a, "h", "POST").request,
            AtOnce::RefuseSaysHeld,
            Effect::AllowEgressOnce,
            Effect::Drop,
        ),
        (
            merge.request,
            AtOnce::MergeWaits,
            Effect::MergeBranch,
            Effect::SendTaskBack,
        ),
    ];
    for (request, at_once, approve, deny) in table {
        let r = resume(&request);
        assert_eq!(r.at_once, at_once);
        assert_eq!(r.effect(Decision::Approved), approve);
        assert_eq!(r.effect(Decision::Denied), deny);
    }
}

#[test]
fn the_decision_always_goes_to_the_inbox() {
    let id = x100_core::id::ActionId::from_u128(9);
    for d in [Decision::Approved, Decision::Denied] {
        assert_eq!(
            tell_agent(id, d),
            InboxItem::Decision {
                action: id,
                decision: d
            }
        );
    }
}

#[test]
fn an_approved_command_is_allowed_once_for_that_agent_only() {
    let (_, f) = fern(AskMeAbout::HighRiskOnly);
    let (me, other) = (f.backend, f.frontend);
    let held = command(me, "wrangler deploy --env production");
    let mut once = AllowedOnce::new();
    assert!(once.grant(&held, None));
    assert!(!once.grant(&held, None), "approving twice grants once");

    // Not another agent, not a similar command, not an egress request.
    assert_eq!(once.take(other, &held.request), None);
    for near in [
        "wrangler deploy --env production ",
        "wrangler deploy",
        "WRANGLER deploy --env production",
        "wrangler deploy --env production;",
    ] {
        assert_eq!(once.take(me, &command(me, near).request), None, "{near}");
    }
    assert_eq!(once.take(me, &egress(me, "wrangler", "POST").request), None);

    // The exact command passes once, from whichever driver it now arrives at.
    let from_new_driver = Request::Command {
        driver: DriverId::from_u128(4_000),
        command: "wrangler deploy --env production".into(),
    };
    assert_eq!(once.take(me, &from_new_driver), Some(held.id));
    assert_eq!(once.take(me, &held.request), None, "used up");
    assert!(once.grants().is_empty());
}

#[test]
fn an_approved_egress_request_matches_host_and_method_exactly() {
    let (_, f) = fern(AskMeAbout::HighRiskOnly);
    let held = egress(f.backend, "api.acme.example", "POST");
    let mut once = AllowedOnce::new();
    assert!(once.grant(&held, None));
    assert_eq!(
        once.take(
            f.backend,
            &egress(f.backend, "api.acme.example", "PUT").request
        ),
        None
    );
    assert_eq!(
        once.take(
            f.backend,
            &egress(f.backend, "api.acme.example:8443", "POST").request
        ),
        None
    );
    assert_eq!(
        once.take(
            f.backend,
            &egress(f.backend, "api.acme.example", "POST").request
        ),
        Some(held.id)
    );
    assert_eq!(once.take(f.backend, &held.request), None);
}

#[test]
fn only_commands_and_egress_leave_a_grant() {
    let (_, f) = fern(AskMeAbout::HighRiskOnly);
    let mut once = AllowedOnce::new();
    let read_log = tool(
        f.ceo,
        x100_core::tool::ToolCall::ReadLog {
            from: x100_core::event::Seq(1),
            to: None,
        },
    );
    assert!(!once.grant(&read_log, None));
    assert!(!once.grant(&merge(f.ceo, f.brief), None));
    assert!(once.grants().is_empty());
}

#[test]
fn two_approvals_allow_two_uses() {
    let (_, f) = fern(AskMeAbout::HighRiskOnly);
    let (a, b) = (
        command(f.backend, "npm publish"),
        command(f.backend, "npm publish"),
    );
    let mut once = AllowedOnce::new();
    once.grant(&a, None);
    once.grant(&b, None);
    assert_eq!(
        once.take(f.backend, &a.request),
        Some(a.id),
        "the older grant first"
    );
    assert_eq!(once.take(f.backend, &a.request), Some(b.id));
    assert_eq!(once.take(f.backend, &a.request), None);
}

#[test]
fn a_grant_is_forgotten_when_the_task_ends_or_the_agent_leaves() {
    let (_, f) = fern(AskMeAbout::HighRiskOnly);
    let (mine, theirs, other_task) = (f.landing, f.api, f.copy);
    let a = command(f.backend, "npm publish");
    let b = command(f.backend, "cargo publish");
    let c = command(f.frontend, "npm publish");
    let d = command(f.backend, "docker push x");
    let mut once = AllowedOnce::new();
    once.grant(&a, Some(mine));
    once.grant(&b, Some(theirs));
    once.grant(&c, Some(mine));
    once.grant(&d, None);

    once.task_ended(f.backend, mine);
    let left: Vec<_> = once.grants().iter().map(|g| g.action).collect();
    assert_eq!(
        left,
        vec![b.id, c.id],
        "its grants for that task, and those with no task, go"
    );

    once.task_ended(f.backend, other_task);
    assert_eq!(once.grants().len(), 2, "another task's end changes nothing");
    once.agent_left(f.backend);
    assert_eq!(
        once.grants().iter().map(|g| g.action).collect::<Vec<_>>(),
        vec![c.id]
    );
}

#[test]
fn the_record_is_rebuilt_from_the_log() {
    let (mut state, agent, task) = some_agent_with_a_task();
    let approved = command(agent, "wrangler deploy --env production");
    let denied = command(agent, "npm publish");
    decide(&mut state, &approved, Decision::Approved);
    decide(&mut state, &denied, Decision::Denied);

    let mut once = AllowedOnce::from_state(&state);
    assert_eq!(once.grants().len(), 1, "a denied action leaves nothing");
    assert_eq!(once.grants()[0].task, Some(task));
    assert_eq!(once.take(agent, &approved.request), Some(approved.id));

    // Using a grant is the action's CarriedOut event, so after it a rebuild finds nothing.
    push(
        &mut state,
        x100_core::party::Actor::Daemon,
        x100_core::event::ProjectEvent::Action {
            action: approved.id,
            event: x100_core::action::ActionEvent::CarriedOut,
        },
    );
    assert!(AllowedOnce::from_state(&state).grants().is_empty());
}

#[test]
fn what_the_builder_sees_for_the_held_deploy() {
    let (state, f) = fern(AskMeAbout::HighRiskOnly);
    let record = state.action(f.deploy).expect("the deploy");
    let shown = Shown::of(&state, record).expect("it was classified");
    assert_eq!(shown.action, f.deploy);
    assert!(
        shown.what.starts_with("Run the command `wrangler deploy"),
        "{}",
        shown.what
    );
    assert_eq!(
        (shown.kind, shown.risk),
        (ActionKind::DeployToProduction, Risk::High)
    );
    assert!(!shown.why.is_empty());
    assert_eq!(shown.who.agent, record.action.agent);
    assert!(shown.who.role.is_some());
    assert!(!shown.always_held);
    assert_eq!(
        shown.judged_by,
        record.classification.as_ref().expect("classified").by
    );
    assert!(matches!(
        shown.judged_by,
        ClassifiedBy::Rules | ClassifiedBy::Classifier(_)
    ));
}

#[test]
fn a_merge_and_a_tool_call_read_in_plain_words() {
    let (mut state, f) = fern(AskMeAbout::MediumAndUp);
    let m = merge(f.ceo, f.brief);
    decide(&mut state, &m, Decision::Approved);
    let shown = Shown::of(&state, state.action(m.id).expect("recorded")).expect("classified");
    assert!(
        shown.what.contains("into the main branch"),
        "{}",
        shown.what
    );
    assert_eq!(shown.task.as_ref().map(|t| t.id), Some(f.brief));

    let team = tool(
        f.ceo,
        x100_core::tool::ToolCall::ProposeChange(x100_core::tool::Proposal::Team {
            changes: vec![x100_core::team::TeamChange::Retire { agent: f.research }],
            telemetry: "Research was idle for a week".into(),
            team_toml: String::new(),
        }),
    );
    let ruling = ruling(&state, &team);
    assert!(ruling.is_held());
    decide(&mut state, &team, Decision::Denied);
    let shown = Shown::of(&state, state.action(team.id).expect("recorded")).expect("classified");
    assert!(shown.what.contains("Research was idle"), "{}", shown.what);
}

// The same cases as the plan's end-to-end check, as far as a pure crate can take them: the
// rulings and the instructions. The orchestrator's test runs them with the fake harness.
#[test]
fn production_deploy_and_file_edit_at_each_level() {
    for (level, deploy_held, edit_held) in [
        (AskMeAbout::Nothing, false, false),
        (AskMeAbout::HighRiskOnly, true, false),
        (AskMeAbout::MediumAndUp, true, false),
        (AskMeAbout::EveryDecision, true, true),
    ] {
        let (state, f) = fern(level);
        let deploy = command(f.backend, "wrangler deploy --env production");
        let edit = command(f.backend, "sed -i 's/Fernleaf/Fernleaf Plants/' index.html");
        assert_eq!(
            ruling(&state, &deploy).is_held(),
            deploy_held,
            "{level:?} deploy"
        );
        assert_eq!(ruling(&state, &edit).is_held(), edit_held, "{level:?} edit");
        assert_eq!(
            ruling(&state, &deploy).classification().kind,
            ActionKind::DeployToProduction
        );
    }
}

#[test]
fn approve_lets_the_retry_through_once_and_deny_does_not() {
    let (mut state, f) = fern(AskMeAbout::HighRiskOnly);
    let first = command(f.backend, "wrangler deploy --env production");
    let r = ruling(&state, &first);
    assert!(r.is_held());
    let plan = resume(&first.request);
    assert_eq!(plan.at_once, AtOnce::HookDenialSaysHeld);

    // Approve: the instruction is to allow it once; the retry arrives from a replaced driver.
    decide(&mut state, &first, Decision::Approved);
    assert_eq!(plan.effect(Decision::Approved), Effect::AllowCommandOnce);
    let mut once = AllowedOnce::new();
    once.grant(&first, None);
    let retry = Request::Command {
        driver: DriverId::from_u128(5_000),
        command: "wrangler deploy --env production".into(),
    };
    assert_eq!(once.take(f.backend, &retry), Some(first.id));
    assert_eq!(
        once.take(f.backend, &retry),
        None,
        "the next attempt is held again"
    );

    // Deny: nothing is granted, and the agent is told through its inbox.
    let second = command(f.backend, "wrangler deploy --env production");
    assert_eq!(plan.effect(Decision::Denied), Effect::Drop);
    assert_eq!(
        tell_agent(second.id, Decision::Denied),
        InboxItem::Decision {
            action: second.id,
            decision: Decision::Denied
        }
    );
    assert_eq!(once.take(f.backend, &second.request), None);
}
