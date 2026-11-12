use x100_core::driver::{DriverEvent, DriverState, Failure, StopReason};
use x100_core::event::ProjectEvent;
use x100_core::id::DriverId;
use x100_core::party::Actor;
use x100_core::process::ProcessEnd;
use x100_core::time::Clock;
use x100_core::transcript::{Entry, Item, TurnEnd};
use x100_store::ProjectStore;

/// Drivers do not outlive the daemon. For a log read after a restart: a driver that was Starting,
/// Running, Compacting or Handing off is Failed with `DaemonRestarted`, a Warm one is Ended with
/// it, and none is left live. A process `start_process` started went with its sandbox. Taking the agent's work over on a new driver is not done here: an
/// agent with unhandled inbox items gets one by the ordinary rule.
pub fn recover(store: &ProjectStore, clock: &dyn Clock) -> x100_store::Result<Vec<DriverId>> {
    let live: Vec<_> = store.log.with_state(|s| {
        s.live_drivers()
            .map(|d| (d.driver.id, d.driver.agent, d.state.clone()))
            .collect()
    });
    let mut recovered = Vec::new();
    for (driver, agent, state) in live {
        let event = match state {
            DriverState::Warm => DriverEvent::Stopped(StopReason::DaemonRestarted),
            DriverState::Running => {
                // A turn was open in the transcript; close it so a reader sees how it ended.
                store.transcripts.append(
                    agent,
                    Entry {
                        at: clock.now(),
                        driver,
                        item: Item::TurnEnded {
                            how: TurnEnd::Failed {
                                detail: "the daemon restarted".into(),
                            },
                        },
                    },
                )?;
                DriverEvent::Failed(Failure::DaemonRestarted)
            }
            _ => DriverEvent::Failed(Failure::DaemonRestarted),
        };
        store
            .log
            .append(Actor::Daemon, ProjectEvent::Driver { driver, event })?;
        recovered.push(driver);
    }
    // Sandboxes do not outlive the daemon either, nor what runs in them.
    let running: Vec<_> = store.log.with_state(|s| {
        s.agents()
            .flat_map(|a| a.processes.keys().map(move |name| (a.id, name.clone())))
            .collect()
    });
    for (agent, name) in running {
        let end = ProcessEnd::SandboxDestroyed;
        store.log.append(
            Actor::Daemon,
            ProjectEvent::ProcessStopped { agent, name, end },
        )?;
    }
    Ok(recovered)
}
