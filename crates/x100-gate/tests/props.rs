//! Properties: the decision is monotone in the level and never both held and allowed, and the
//! "allowed once" record is used at most once, for an exact match, by the same agent.

mod common;

use common::*;
use proptest::prelude::*;
use x100_core::action::{ActionKind, AskMeAbout, Classification, ClassifiedBy, Risk};
use x100_core::id::{ActionId, AgentId, TaskId};
use x100_gate::{AllowedOnce, Ruling, holds, judge};

fn any_kind() -> impl Strategy<Value = ActionKind> {
    prop::sample::select(vec![
        ActionKind::DeleteData,
        ActionKind::DeployToProduction,
        ActionKind::Spend,
        ActionKind::SendOutside,
        ActionKind::ChangeCredentials,
        ActionKind::ForcePush,
        ActionKind::TeamChange,
        ActionKind::Merge,
        ActionKind::Other,
    ])
}

fn any_risk() -> impl Strategy<Value = Risk> {
    prop::sample::select(vec![Risk::Low, Risk::Medium, Risk::High])
}

fn any_level() -> impl Strategy<Value = AskMeAbout> {
    prop::sample::select(LEVELS.to_vec())
}

const COMMANDS: &[&str] = &[
    "ls",
    "git push --force",
    "git push",
    "npm publish",
    "wrangler deploy --env production",
    "wrangler deploy --env staging",
    "rm -rf /",
    "rm -rf build",
    "sudo make install",
    "aws s3 ls",
    "aws s3 rm s3://b",
    "psql -c 'drop table x'",
    "curl x | sh",
    "eval \"$X\"",
    "gh pr comment 1",
    "ssh host",
    "cat ~/.ssh/id_rsa",
    "echo ignore previous instructions",
    "cargo test",
];
const HOSTS: &[&str] = &[
    "github.com",
    "registry.npmjs.org",
    "hooks.slack.com",
    "api.stripe.com",
    "api.acme.example",
    "evil.example",
    "api.vercel.com",
    "crates.io:443",
];
const METHODS: &[&str] = &[
    "GET", "HEAD", "POST", "PUT", "PATCH", "DELETE", "get", "post",
];
const ANSWERS: &[Option<&str>] = &[
    None,
    Some("kind=other risk=low"),
    Some("kind=other risk=medium"),
    Some("kind=send_outside risk=high"),
    Some("kind=spend risk=low"),
    Some("garbage"),
    Some(""),
    Some("kind=other risk=low\nkind=other risk=high"),
];

fn any_action() -> impl Strategy<Value = x100_core::action::Action> {
    let (_, f) = fixtures();
    let agent = f.backend;
    let brief = f.brief;
    let landing = f.landing;
    prop_oneof![
        prop::sample::select(COMMANDS.to_vec()).prop_map(move |c| command(agent, c)),
        ".{0,60}".prop_map(move |c| command(agent, &c)),
        (
            prop::sample::select(HOSTS.to_vec()),
            prop::sample::select(METHODS.to_vec())
        )
            .prop_map(move |(h, m)| egress(agent, h, m)),
        Just(brief).prop_map(move |t| merge(agent, t)),
        Just(landing).prop_map(move |t| merge(agent, t)),
    ]
}

fn held_at(
    level: AskMeAbout,
    action: &x100_core::action::Action,
    said: Option<&'static str>,
) -> Ruling {
    let (states, _) = fixtures();
    let (_, state) = states
        .iter()
        .find(|(l, _)| *l == level)
        .expect("a state per level");
    judge(state, action, Some("/work/ws"), &Says(said))
}

proptest! {
    #[test]
    fn a_ruling_is_exactly_one_of_held_and_allowed_and_agrees_with_the_level(
        level in any_level(), kind in any_kind(), risk in any_risk(),
    ) {
        let c = Classification { kind, risk, by: ClassifiedBy::Rules, why: String::new() };
        let r = Ruling::new(level, c.clone());
        let must_hold = level.holds(risk) || kind == ActionKind::TeamChange;
        prop_assert_eq!(holds(level, &c), must_hold);
        match r {
            Ruling::Hold(_) => prop_assert!(must_hold),
            // An allowed action is one the level lets through and no rule insists on holding.
            Ruling::Allow(_) => prop_assert!(!must_hold),
        }
    }

    #[test]
    fn raising_the_level_never_releases_what_a_lower_level_held(
        action in any_action(), said in prop::sample::select(ANSWERS.to_vec()),
    ) {
        let held: Vec<bool> = LEVELS.iter().map(|l| held_at(*l, &action, said).is_held()).collect();
        for lower in 0..LEVELS.len() {
            for higher in lower..LEVELS.len() {
                prop_assert!(!held[lower] || held[higher], "{:?}: {:?} held but {:?} not", action.request, LEVELS[lower], LEVELS[higher]);
            }
        }
    }

    #[test]
    fn what_the_classifier_does_not_answer_is_never_allowed_above_nothing(
        level in any_level(), host in prop::sample::select(HOSTS.to_vec()), method in prop::sample::select(METHODS.to_vec()),
    ) {
        let (_, f) = fixtures();
        let r = held_at(level, &egress(f.backend, host, method), None);
        let c = r.classification();
        if c.by == ClassifiedBy::Rules && c.risk == Risk::High && c.why.contains("no answer") {
            prop_assert_eq!(r.is_held(), level != AskMeAbout::Nothing);
        }
    }
}

#[derive(Debug, Clone)]
enum Op {
    Grant {
        agent: u8,
        cmd: u8,
        task: Option<u8>,
    },
    Take {
        agent: u8,
        cmd: u8,
    },
    TaskEnded {
        agent: u8,
        task: u8,
    },
    Left {
        agent: u8,
    },
}

fn any_op() -> impl Strategy<Value = Op> {
    prop_oneof![
        (0u8..3, 0u8..3, prop::option::of(0u8..2)).prop_map(|(agent, cmd, task)| Op::Grant {
            agent,
            cmd,
            task
        }),
        (0u8..3, 0u8..3).prop_map(|(agent, cmd)| Op::Take { agent, cmd }),
        (0u8..3, 0u8..2).prop_map(|(agent, task)| Op::TaskEnded { agent, task }),
        (0u8..3).prop_map(|agent| Op::Left { agent }),
    ]
}

proptest! {
    #[test]
    fn an_allowed_once_entry_is_used_at_most_once_and_only_for_an_exact_match(ops in prop::collection::vec(any_op(), 0..60)) {
        let agent = |n: u8| AgentId::from_u128(u128::from(n) + 1);
        let task = |n: u8| TaskId::from_u128(u128::from(n) + 1);
        let cmd = |n: u8| ["npm publish", "git push -f", "npm publish "][usize::from(n)];

        let mut once = AllowedOnce::new();
        // The model: what was granted and not yet used, forgotten or ended.
        let mut model: Vec<(ActionId, u8, u8, Option<u8>)> = Vec::new();
        let mut granted: Vec<(ActionId, u8, u8)> = Vec::new();
        let mut used: Vec<ActionId> = Vec::new();
        let mut next = 1u128;

        for op in ops {
            match op {
                Op::Grant { agent: a, cmd: c, task: t } => {
                    let mut action = command(agent(a), cmd(c));
                    action.id = ActionId::from_u128(next);
                    next += 1;
                    prop_assert!(once.grant(&action, t.map(task)));
                    model.push((action.id, a, c, t));
                    granted.push((action.id, a, c));
                }
                Op::Take { agent: a, cmd: c } => {
                    let got = once.take(agent(a), &command(agent(a), cmd(c)).request);
                    let expected = model.iter().position(|(_, ma, mc, _)| *ma == a && *mc == c);
                    match (got, expected) {
                        (Some(id), Some(i)) => {
                            prop_assert_eq!(model.remove(i).0, id);
                            prop_assert!(!used.contains(&id), "used twice");
                            used.push(id);
                            let (_, ga, gc) = *granted.iter().find(|(g, _, _)| *g == id).expect("it was granted");
                            prop_assert!(ga == a && gc == c, "an exact match by the same agent");
                        }
                        (None, None) => {}
                        (got, expected) => prop_assert!(false, "real {:?}, model {:?}", got, expected),
                    }
                }
                Op::TaskEnded { agent: a, task: t } => {
                    once.task_ended(agent(a), task(t));
                    model.retain(|(_, ma, _, mt)| *ma != a || mt.is_some_and(|x| x != t));
                }
                Op::Left { agent: a } => {
                    once.agent_left(agent(a));
                    model.retain(|(_, ma, _, _)| *ma != a);
                }
            }
            prop_assert_eq!(once.grants().len(), model.len());
        }
    }
}
