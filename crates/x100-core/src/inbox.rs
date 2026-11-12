//! The inbox: everything that can make an agent act, and which of it has not been handled yet.
//!
//! Items are given to a driver between turns, never mid-turn. An item counts as handled only when
//! the turn it was given in completes, so if the driver fails or is stopped first, the item goes to
//! the next driver.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::action::Decision;
use crate::driver::Incompatible;
use crate::id::{ActionId, AgentId, DriverId, InboxItemId, JobId, MessageId, TaskId, WaitId};
use crate::wait::WaitEnd;

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InboxItem {
    /// A task delegated to it, or sent back to it.
    Task { task: TaskId },
    /// A message from another agent, or for the CEO, from the builder.
    Message { message: MessageId },
    /// The report on a task it delegated.
    Report { task: TaskId },
    /// The decision on one of its held actions. It reaches the agent even if its driver changed
    /// meanwhile.
    Decision {
        action: ActionId,
        decision: Decision,
    },
    /// A job woke it.
    Job { job: JobId, note: String },
    /// One of its waits ran out, or waits on something that can no longer happen.
    WaitEnded { wait: WaitId, end: WaitEnd },
    /// One of its children is waiting for a driver, and why.
    NoDriver {
        agent: AgentId,
        why: Vec<Incompatible>,
    },
    /// For the CEO: a lead reached its budget.
    BudgetReached { lead: AgentId },
    /// Its last `turns` turns ended with the task neither reported nor waited on: carry on, wait or
    /// report.
    Nudge { task: TaskId, turns: u32 },
}

/// Which items an agent has been given and not yet handled.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
pub struct Inbox {
    /// `None` for an item waiting for a turn, or the driver whose current turn holds it.
    items: BTreeMap<InboxItemId, Option<DriverId>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, thiserror::Error)]
pub enum InboxError {
    #[error("{0} was already delivered")]
    AlreadyDelivered(InboxItemId),
    #[error("{0} is not waiting for a turn")]
    NotWaiting(InboxItemId),
}

impl Inbox {
    pub fn deliver(&mut self, item: InboxItemId) -> Result<(), InboxError> {
        if self.items.contains_key(&item) {
            return Err(InboxError::AlreadyDelivered(item));
        }
        self.items.insert(item, None);
        Ok(())
    }

    /// Items waiting for a turn.
    pub fn waiting(&self) -> impl Iterator<Item = InboxItemId> + '_ {
        self.items
            .iter()
            .filter(|(_, given)| given.is_none())
            .map(|(id, _)| *id)
    }

    /// Everything delivered and not yet handled, whether or not a running turn holds it.
    pub fn unhandled(&self) -> impl Iterator<Item = InboxItemId> + '_ {
        self.items.keys().copied()
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    /// Gives items to the turn `driver` is starting. Either all are given or none.
    pub fn give(&mut self, items: &[InboxItemId], driver: DriverId) -> Result<(), InboxError> {
        if let Some(bad) = items.iter().find(|i| self.items.get(*i) != Some(&None)) {
            return Err(InboxError::NotWaiting(*bad));
        }
        for item in items {
            self.items.insert(*item, Some(driver));
        }
        Ok(())
    }

    /// The turn on `driver` completed: what it was given is handled. Returns those items.
    pub fn turn_completed(&mut self, driver: DriverId) -> Vec<InboxItemId> {
        let handled: Vec<_> = self.held_by(driver).collect();
        for item in &handled {
            self.items.remove(item);
        }
        handled
    }

    /// The turn on `driver` will not complete: what it was given waits for the next turn. Returns
    /// those items.
    pub fn turn_lost(&mut self, driver: DriverId) -> Vec<InboxItemId> {
        let back: Vec<_> = self.held_by(driver).collect();
        for item in &back {
            self.items.insert(*item, None);
        }
        back
    }

    fn held_by(&self, driver: DriverId) -> impl Iterator<Item = InboxItemId> + '_ {
        self.items
            .iter()
            .filter(move |(_, given)| **given == Some(driver))
            .map(|(id, _)| *id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(n: u128) -> InboxItemId {
        InboxItemId::from_u128(n)
    }

    #[test]
    fn an_item_is_handled_only_when_its_turn_completes() {
        let d = DriverId::from_u128(1);
        let mut inbox = Inbox::default();
        inbox.deliver(item(1)).unwrap();
        inbox.give(&[item(1)], d).unwrap();
        assert_eq!(inbox.waiting().count(), 0);
        assert_eq!(inbox.unhandled().collect::<Vec<_>>(), vec![item(1)]);
        assert_eq!(inbox.turn_completed(d), vec![item(1)]);
        assert!(inbox.is_empty());
    }

    #[test]
    fn a_lost_turn_returns_its_items_for_the_next_driver() {
        let (first, second) = (DriverId::from_u128(1), DriverId::from_u128(2));
        let mut inbox = Inbox::default();
        inbox.deliver(item(1)).unwrap();
        inbox.give(&[item(1)], first).unwrap();
        assert_eq!(inbox.turn_lost(first), vec![item(1)]);
        assert_eq!(inbox.waiting().collect::<Vec<_>>(), vec![item(1)]);
        inbox.give(&[item(1)], second).unwrap();
        assert_eq!(inbox.turn_completed(first), vec![]);
        assert_eq!(inbox.turn_completed(second), vec![item(1)]);
    }

    #[test]
    fn an_item_arriving_mid_turn_waits_for_the_next_turn() {
        let d = DriverId::from_u128(1);
        let mut inbox = Inbox::default();
        inbox.deliver(item(1)).unwrap();
        inbox.give(&[item(1)], d).unwrap();
        inbox.deliver(item(2)).unwrap();
        assert_eq!(inbox.turn_completed(d), vec![item(1)]);
        assert_eq!(inbox.waiting().collect::<Vec<_>>(), vec![item(2)]);
    }

    #[test]
    fn an_item_cannot_be_given_twice_or_delivered_twice() {
        let d = DriverId::from_u128(1);
        let mut inbox = Inbox::default();
        inbox.deliver(item(1)).unwrap();
        assert_eq!(
            inbox.deliver(item(1)),
            Err(InboxError::AlreadyDelivered(item(1)))
        );
        inbox.give(&[item(1)], d).unwrap();
        assert_eq!(
            inbox.give(&[item(1)], d),
            Err(InboxError::NotWaiting(item(1)))
        );
        assert_eq!(
            inbox.give(&[item(9)], d),
            Err(InboxError::NotWaiting(item(9)))
        );
    }

    #[test]
    fn giving_is_all_or_nothing() {
        let d = DriverId::from_u128(1);
        let mut inbox = Inbox::default();
        inbox.deliver(item(1)).unwrap();
        assert!(inbox.give(&[item(1), item(2)], d).is_err());
        assert_eq!(inbox.waiting().collect::<Vec<_>>(), vec![item(1)]);
    }
}
