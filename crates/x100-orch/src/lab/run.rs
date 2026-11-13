//! One lab driver's life: the router's choice for a lab driver of the agent, a fresh sandbox over
//! a throwaway copy made from a checkpoint, a start pack read on its arm's branch, its turns, the
//! checks run in its sandbox afterwards, and its end. Its start, turns, charges and end are in the
//! project's log, its charges on the lab's line, and its output in the agent's transcript for the
//! builder to see. Nothing else of the project's is touched.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use x100_core::driver::{
    Driver, DriverEvent, Failure, Incompatible, Purpose, StartReason, StopReason,
};
use x100_core::event::ProjectEvent;
use x100_core::id::{AgentId, DriverId};
use x100_core::ledger::BudgetReached;
use x100_core::money::Money;
use x100_core::party::Actor;
use x100_core::transcript::{Entry, Item, TurnEnd as Ended};
use x100_drivers::{Decision, DriverControl, DriverHandle, DriverOutput, TurnEnd};
use x100_memory::Target;
use x100_router::{Choice, Need, World};
use x100_vault::Requester;

use super::calls::LabCalls;
use crate::Shared;
use crate::ctx::ProjectCtx;
use crate::sandbox::LabBox;
use crate::store::blocking;
use crate::{ids::new_id, redact, route, spec};

/// How long a script check may run.
const CHECK_TIME: Duration = Duration::from_secs(300);

/// What a lab driver is started for.
pub(crate) struct Setup {
    pub purpose: Purpose,
    pub agent: AgentId,
    pub target: Target,
    pub pack: String,
    /// Its working directory, already made: a copy from a checkpoint, or empty for a judge.
    pub dir: PathBuf,
}

/// Why a run could not be had at all, which ends the trial.
pub(crate) struct Stop(pub String);

/// A lab driver that is running, between its turns.
pub(crate) struct Run {
    pub id: DriverId,
    record: Driver,
    lab: Option<LabBox>,
    handle: DriverHandle,
    control: DriverControl,
    pub calls: Arc<LabCalls>,
    pub turns: u32,
    failed: bool,
    dir: PathBuf,
}

/// What a finished run cost and how it went, besides its checks.
pub(crate) struct Driven {
    pub reported: Option<(bool, String)>,
    pub tool_errors: u32,
    pub turns: u32,
    pub cost: Money,
}

/// The router's choice for a lab driver of `agent`, with its room taken. `Err` when none may
/// start: the lab's cap or the project's total is reached, or no harness fits.
fn choose(
    shared: &Shared,
    ctx: &ProjectCtx,
    agent: AgentId,
    purpose: Purpose,
) -> Result<(DriverId, Choice), Vec<Incompatible>> {
    let installed = shared.installed();
    let now = shared.clock.now();
    shared.fleet.allocate(|occupancy| {
        let decided = ctx.store.log.with_state(|p| {
            shared.daemon.with_state(|d| {
                let world = World {
                    daemon: d,
                    project: p,
                    installed: &installed,
                    occupancy,
                    now,
                };
                let need = Need {
                    agent,
                    purpose,
                    warm: None,
                };
                x100_router::choose(&world, &need).map(|decision| match decision {
                    x100_router::Decision::Start { choice, .. } => {
                        let slot = x100_router::slot(d, choice.harness, choice.machine);
                        (choice, slot)
                    }
                    x100_router::Decision::Reuse { .. } => {
                        unreachable!("a lab driver is never warm")
                    }
                })
            })
        });
        match decided {
            Ok((choice, slot)) => {
                let driver = new_id(&*shared.clock, DriverId::new);
                (Ok((driver, choice.clone())), Some((driver, slot)))
            }
            Err(why) => (Err(why), None),
        }
    })
}

/// How long a lab run waits for room on the machine or a login before the trial stops.
const ROOM_WAIT: Duration = Duration::from_secs(600);

/// A lab driver's choice, waiting while the machine or the login is only full for now: the
/// project's own work comes first, and a trial can wait for it.
async fn room(
    shared: &Shared,
    ctx: &ProjectCtx,
    agent: AgentId,
    purpose: Purpose,
) -> Result<(DriverId, Choice), Stop> {
    let until = std::time::Instant::now() + ROOM_WAIT;
    loop {
        match choose(shared, ctx, agent, purpose) {
            Ok(chosen) => return Ok(chosen),
            Err(why)
                if std::time::Instant::now() < until
                    && why.iter().all(|w| {
                        matches!(
                            w,
                            Incompatible::MachineFull { .. } | Incompatible::LoginFull { .. }
                        )
                    }) =>
            {
                tokio::time::sleep(Duration::from_millis(500)).await;
            }
            Err(why) => return Err(Stop(not_started(&why))),
        }
    }
}

/// Why no lab driver could start, in words.
fn not_started(why: &[Incompatible]) -> String {
    if why.contains(&Incompatible::Budget(BudgetReached::Lab)) {
        "the lab reached its cap".into()
    } else if why.contains(&Incompatible::Budget(BudgetReached::Project)) {
        "the project reached its budget".into()
    } else {
        format!("no driver could run in the lab: {why:?}")
    }
}

async fn append(ctx: &Arc<ProjectCtx>, events: Vec<ProjectEvent>) -> Result<(), String> {
    let ctx = Arc::clone(ctx);
    let events = events.into_iter().map(|e| (Actor::Daemon, e)).collect();
    blocking(move || ctx.writer().append(events).map(drop))
        .await
        .map_err(|e| format!("the log refused a lab driver's events: {e}"))
}

async fn transcript(shared: &Shared, ctx: &Arc<ProjectCtx>, run: (AgentId, DriverId), item: Item) {
    let Some(item) = redact::value(&shared.redactor, &item) else {
        return;
    };
    let entry = Entry {
        at: ctx.clock.now(),
        driver: run.1,
        item,
    };
    let ctx = Arc::clone(ctx);
    if let Err(e) = blocking(move || ctx.store.transcripts.append(run.0, entry)).await {
        eprintln!("x100: transcript of a lab run: {e}");
    }
}

impl Run {
    /// Starts a lab driver. `Ok(None)` if it was recorded and failed to start, which scores as a
    /// run that did nothing; `Err` if none could be had.
    pub async fn begin(
        shared: &Arc<Shared>,
        ctx: &Arc<ProjectCtx>,
        setup: Setup,
    ) -> Result<Option<Run>, Stop> {
        let (id, choice) = room(shared, ctx, setup.agent, setup.purpose).await?;
        let release = |reason: String| {
            shared.fleet.release(id);
            Stop(reason)
        };
        let Some(harness) = shared.harness(choice.harness) else {
            return Err(release(format!(
                "harness {} is not registered",
                choice.harness
            )));
        };
        let record = Driver {
            id,
            agent: setup.agent,
            harness: choice.harness,
            model: choice.model.clone(),
            effort: choice.effort,
            machine: choice.machine,
            purpose: setup.purpose,
            view: harness.view(),
            why: StartReason::Wake,
        };
        append(ctx, vec![ProjectEvent::DriverStarted(record.clone())])
            .await
            .map_err(release)?;
        let requester = Requester {
            project: ctx.id,
            agent: setup.agent,
            driver: id,
            purpose: setup.purpose,
        };
        let placed = shared
            .sandboxes
            .place_lab(
                &(shared.settings)(),
                requester,
                &setup.dir,
                shared.desk.tools_addr(),
                spec::logins(shared),
                &harness.login_hosts(),
                &spec::provider_hosts(&*harness),
            )
            .await;
        let lab = match placed {
            Ok(lab) => lab,
            Err(why) => {
                let failed = DriverEvent::Failed(Failure::Error {
                    detail: why.clone(),
                    provider: None,
                });
                let _ = append(
                    ctx,
                    vec![ProjectEvent::Driver {
                        driver: id,
                        event: failed,
                    }],
                )
                .await;
                return Err(release(why));
            }
        };
        let code_dir = blocking({
            let ctx = Arc::clone(ctx);
            move || ctx.git().map(|g| g.project_dir().to_owned())
        })
        .await
        .unwrap_or_default();
        let calls = Arc::new(LabCalls::new(setup.agent, code_dir, setup.target));
        let endpoint = shared
            .desk
            .issue_lab(id, Arc::clone(ctx), Arc::clone(&calls));
        transcript(
            shared,
            ctx,
            (setup.agent, id),
            Item::StartPack {
                text: setup.pack.clone(),
            },
        )
        .await;
        let spec = spec::build(
            shared,
            ctx,
            &*harness,
            id,
            &choice,
            setup.dir.clone(),
            setup.pack,
            &lab.placed,
            endpoint,
        );
        let run = match harness.start(spec).await {
            Ok(handle) => Run {
                id,
                record,
                lab: Some(lab),
                control: handle.control(),
                handle,
                calls,
                turns: 0,
                failed: false,
                dir: setup.dir,
            },
            Err(e) => {
                let failed = DriverEvent::Failed(Failure::Error {
                    detail: e.to_string(),
                    provider: None,
                });
                let _ = append(
                    ctx,
                    vec![ProjectEvent::Driver {
                        driver: id,
                        event: failed,
                    }],
                )
                .await;
                shared.desk.withdraw(id);
                let processes = Arc::clone(&shared.processes);
                let _ = blocking(move || processes.forget(id)).await;
                shared.sandboxes.end_lab(lab).await;
                shared.fleet.release(id);
                return Ok(None);
            }
        };
        Ok(Some(run))
    }

    /// Gives it turns, starting with `input`, until it reports, fails, or has had
    /// [`x100_lab::TURNS_PER_ITEM`] turns.
    pub async fn work(&mut self, shared: &Shared, ctx: &Arc<ProjectCtx>, input: String) {
        let mut input = input;
        while !self.failed && self.turns < x100_lab::TURNS_PER_ITEM {
            self.turns += 1;
            let started = DriverEvent::TurnStarted { given: Vec::new() };
            if append(ctx, vec![self.event(started)]).await.is_err() {
                self.failed = true;
                return;
            }
            transcript(shared, ctx, self.who(), Item::TurnStarted).await;
            let _ = self.control.turn(input).await;
            if !self.turn(shared, ctx).await {
                return;
            }
            let _ = append(ctx, vec![self.event(DriverEvent::TurnEnded)]).await;
            transcript(
                shared,
                ctx,
                self.who(),
                Item::TurnEnded {
                    how: Ended::Completed,
                },
            )
            .await;
            if self.calls.reported().is_some() {
                return;
            }
            input = x100_lab::words::NUDGE.to_owned();
        }
    }

    fn who(&self) -> (AgentId, DriverId) {
        (self.record.agent, self.id)
    }

    fn event(&self, event: DriverEvent) -> ProjectEvent {
        ProjectEvent::Driver {
            driver: self.id,
            event,
        }
    }

    /// Follows one turn to its end. `false` if the driver failed.
    async fn turn(&mut self, shared: &Shared, ctx: &Arc<ProjectCtx>) -> bool {
        loop {
            let Some(out) = self.handle.next().await else {
                return self.fail(shared, ctx, Failure::StreamBroke).await;
            };
            match out {
                DriverOutput::Text(text) => {
                    transcript(shared, ctx, self.who(), Item::Text { text }).await
                }
                DriverOutput::ToolCall { id, name, input } => {
                    let item = Item::ToolCall {
                        call: id,
                        name,
                        input,
                    };
                    transcript(shared, ctx, self.who(), item).await;
                }
                DriverOutput::ToolResult {
                    id,
                    output,
                    is_error,
                } => {
                    let item = Item::ToolResult {
                        call: id,
                        output,
                        is_error,
                    };
                    transcript(shared, ctx, self.who(), item).await;
                }
                // Everything it runs is inside its own sandbox, made for this run alone.
                DriverOutput::PermissionRequest(r) => {
                    let _ = self.control.answer(r.id, Decision::Allow).await;
                }
                DriverOutput::Spent(spent) => {
                    let (ctx2, record) = (Arc::clone(ctx), self.record.clone());
                    let charged = blocking(move || {
                        ctx2.commit(|s, clock| {
                            let e = route::charged(
                                s,
                                clock,
                                &record,
                                spent.usage,
                                spent.cost,
                                spent.basis,
                            );
                            (e, ())
                        })
                    })
                    .await;
                    if let Err(e) = charged {
                        eprintln!("x100: charge for lab driver {}: {e}", self.id);
                    }
                }
                DriverOutput::Context(fill) => {
                    let _ = append(ctx, vec![self.event(DriverEvent::ContextReported(fill))]).await;
                }
                DriverOutput::TurnEnded(TurnEnd::Completed) => return true,
                DriverOutput::TurnEnded(TurnEnd::Interrupted) => {
                    let failure = Failure::Error {
                        detail: "a lab turn was interrupted".into(),
                        provider: None,
                    };
                    return self.fail(shared, ctx, failure).await;
                }
                DriverOutput::Failed(f) => return self.fail(shared, ctx, f).await,
                DriverOutput::Warning(w) => {
                    eprintln!(
                        "x100: lab driver {}: {}",
                        self.id,
                        shared.redactor.redact_str(&w)
                    );
                }
                DriverOutput::Compacted | DriverOutput::Terminal(_) => {}
            }
        }
    }

    async fn fail(&mut self, shared: &Shared, ctx: &Arc<ProjectCtx>, failure: Failure) -> bool {
        let failure = redact::value(&shared.redactor, &failure).unwrap_or(Failure::StreamBroke);
        let detail = format!("{failure:?}");
        transcript(
            shared,
            ctx,
            self.who(),
            Item::TurnEnded {
                how: Ended::Failed { detail },
            },
        )
        .await;
        let _ = append(ctx, vec![self.event(DriverEvent::Failed(failure))]).await;
        self.failed = true;
        false
    }

    /// Runs `script` in the run's sandbox, in its working directory: passed when it exits 0
    /// within [`CHECK_TIME`].
    pub async fn check(&self, script: &str) -> bool {
        let Some(lab) = &self.lab else { return false };
        let line = lab.command(script);
        let mut command = tokio::process::Command::new(&line.program);
        command
            .args(&line.args)
            .current_dir(&self.dir)
            .env_clear()
            .envs(lab.placed.env.iter().cloned())
            .envs(
                ["PATH", "LANG"]
                    .into_iter()
                    .filter_map(|k| std::env::var(k).ok().map(|v| (k.to_owned(), v))),
            )
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .kill_on_drop(true);
        let Ok(mut child) = command.spawn() else {
            return false;
        };
        matches!(
            tokio::time::timeout(CHECK_TIME, child.wait()).await,
            Ok(Ok(status)) if status.success()
        )
    }

    /// What the run changed in its copy, for a judge to read.
    pub async fn diff(&self) -> String {
        let dir = self.dir.clone();
        blocking(move || {
            let git = |args: &[&str]| {
                std::process::Command::new("git")
                    .arg("-C")
                    .arg(&dir)
                    .args(args)
                    .output()
            };
            let _ = git(&["add", "-A"]);
            git(&["diff", "--cached", "--stat", "-p", "HEAD"])
                .map(|o| {
                    String::from_utf8_lossy(&o.stdout)
                        .chars()
                        .take(20_000)
                        .collect()
                })
                .unwrap_or_default()
        })
        .await
    }

    /// Ends the run: its end in the log, its process and sandbox gone, its copy deleted.
    pub async fn end(mut self, shared: &Shared, ctx: &Arc<ProjectCtx>) -> Driven {
        if !self.failed {
            let stopped = DriverEvent::Stopped(StopReason::LabRunEnded);
            let _ = append(ctx, vec![self.event(stopped)]).await;
        }
        self.control.stop().await;
        shared.desk.withdraw(self.id);
        let (processes, id) = (Arc::clone(&shared.processes), self.id);
        let _ = blocking(move || processes.forget(id)).await;
        if let Some(lab) = self.lab.take() {
            shared.sandboxes.end_lab(lab).await;
        }
        shared.fleet.release(self.id);
        remove(&self.dir).await;
        let id = self.id;
        let cost = {
            let ctx = Arc::clone(ctx);
            blocking(move || {
                ctx.store
                    .log
                    .with_state(|s| s.driver(id).map_or(Money::ZERO, |d| d.spent.cost))
            })
            .await
        };
        Driven {
            reported: self.calls.reported(),
            tool_errors: self.calls.tool_errors(),
            turns: self.turns,
            cost,
        }
    }
}

/// Removes a run's copy. It was made for the run, so nothing else is in it.
pub(crate) async fn remove(dir: &Path) {
    let dir = dir.to_path_buf();
    blocking(move || {
        if let Err(e) = std::fs::remove_dir_all(&dir)
            && e.kind() != std::io::ErrorKind::NotFound
        {
            eprintln!(
                "x100: could not remove the lab's copy {}: {e}",
                dir.display()
            );
        }
    })
    .await;
}
