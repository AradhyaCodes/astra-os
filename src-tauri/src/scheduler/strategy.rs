//! Pluggable scheduling policies for the Aaru-OS virtual CPU.
//!
//! The mechanical tick loop in [`super::Scheduler`] is identical for every
//! algorithm: age WAITING jobs, service the running cores, then fill idle cores
//! from the READY queue. Only three decisions differ between algorithms —
//! *which* ready job to dispatch, *whether* there is a time slice, and
//! *whether* a newcomer may preempt a running job — so each algorithm is its
//! own small independent type implementing [`SchedulingStrategy`] rather than a
//! branch inside one giant function.

use crate::kernel::SchedulerAlgorithm;
use crate::process::Pid;

/// Round-Robin time slice, in deterministic simulation ticks.
pub const ROUND_ROBIN_QUANTUM: u64 = 4;

/// One READY-queue entry as seen by a strategy when it decides what to run next.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReadyEntry {
    pub pid: Pid,
    /// Lower value == more urgent (see [`super::priority_level`]).
    pub priority_level: u8,
    /// Simulation tick on which the job most recently entered the READY queue.
    pub enqueued_tick: u64,
}

/// A scheduling policy. Implementors are tiny, independent, and stateless — the
/// live queue/core/metric state all lives in [`super::Scheduler`].
pub trait SchedulingStrategy: Send + Sync + std::fmt::Debug {
    /// The kernel-level identifier for this policy.
    fn algorithm(&self) -> SchedulerAlgorithm;

    /// Time slice after which a RUNNING job is preempted, if the policy has one.
    fn quantum(&self) -> Option<u64> {
        None
    }

    /// Choose which READY entry to dispatch next. `ready` is in FIFO order
    /// (index `0` has waited longest). Return the index to remove and run, or
    /// `None` to leave the core idle.
    fn pick(&self, ready: &[ReadyEntry]) -> Option<usize>;

    /// May a freshly-READY job immediately bump a RUNNING one off its core?
    /// Every policy in this phase is non-preemptive on arrival (Round-Robin
    /// preempts on *quantum expiry*, handled by the tick loop, not here), but
    /// the hook keeps the loop generic for a future preemptive policy.
    fn preempts(&self, _incoming: &ReadyEntry, _running: &ReadyEntry) -> bool {
        false
    }
}

/// Round-Robin: strict FIFO dispatch with a fixed [`ROUND_ROBIN_QUANTUM`] slice.
#[derive(Debug, Default, Clone, Copy)]
pub struct RoundRobinScheduler;

impl SchedulingStrategy for RoundRobinScheduler {
    fn algorithm(&self) -> SchedulerAlgorithm {
        SchedulerAlgorithm::RoundRobin
    }

    fn quantum(&self) -> Option<u64> {
        Some(ROUND_ROBIN_QUANTUM)
    }

    fn pick(&self, ready: &[ReadyEntry]) -> Option<usize> {
        (!ready.is_empty()).then_some(0)
    }
}

/// First-Come, First-Served: strict FIFO dispatch, run to completion (or until
/// the job blocks for I/O on its own).
#[derive(Debug, Default, Clone, Copy)]
pub struct FcfsScheduler;

impl SchedulingStrategy for FcfsScheduler {
    fn algorithm(&self) -> SchedulerAlgorithm {
        SchedulerAlgorithm::Fcfs
    }

    fn pick(&self, ready: &[ReadyEntry]) -> Option<usize> {
        (!ready.is_empty()).then_some(0)
    }
}

/// Priority: dispatch the most urgent READY job first, breaking ties by longest
/// wait then lowest PID so the choice is fully deterministic. Non-preemptive.
#[derive(Debug, Default, Clone, Copy)]
pub struct PriorityScheduler;

impl SchedulingStrategy for PriorityScheduler {
    fn algorithm(&self) -> SchedulerAlgorithm {
        SchedulerAlgorithm::Priority
    }

    fn pick(&self, ready: &[ReadyEntry]) -> Option<usize> {
        ready
            .iter()
            .enumerate()
            .min_by(|(_, a), (_, b)| {
                a.priority_level
                    .cmp(&b.priority_level)
                    .then(a.enqueued_tick.cmp(&b.enqueued_tick))
                    .then(a.pid.cmp(&b.pid))
            })
            .map(|(index, _)| index)
    }
}

/// Build the strategy object for a kernel algorithm identifier.
pub fn strategy_for(algorithm: SchedulerAlgorithm) -> Box<dyn SchedulingStrategy> {
    match algorithm {
        SchedulerAlgorithm::RoundRobin => Box::new(RoundRobinScheduler),
        SchedulerAlgorithm::Fcfs => Box::new(FcfsScheduler),
        SchedulerAlgorithm::Priority => Box::new(PriorityScheduler),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(pid: Pid, level: u8, enqueued: u64) -> ReadyEntry {
        ReadyEntry {
            pid,
            priority_level: level,
            enqueued_tick: enqueued,
        }
    }

    #[test]
    fn round_robin_and_fcfs_take_the_front_of_the_queue() {
        let ready = [entry(7, 2, 5), entry(3, 0, 1), entry(9, 3, 2)];
        assert_eq!(RoundRobinScheduler.pick(&ready), Some(0));
        assert_eq!(FcfsScheduler.pick(&ready), Some(0));
        assert_eq!(RoundRobinScheduler.quantum(), Some(4));
        assert_eq!(FcfsScheduler.quantum(), None);
    }

    #[test]
    fn priority_picks_the_most_urgent_then_breaks_ties_deterministically() {
        // Lowest priority_level wins.
        let ready = [entry(7, 2, 1), entry(3, 0, 4), entry(9, 1, 2)];
        assert_eq!(PriorityScheduler.pick(&ready), Some(1));

        // Equal priority → the one that has waited longest (smallest tick).
        let tie = [entry(7, 1, 9), entry(3, 1, 4), entry(9, 1, 4)];
        // indices 1 and 2 share enqueued_tick 4 → lowest PID (3) wins → index 1.
        assert_eq!(PriorityScheduler.pick(&tie), Some(1));
    }

    #[test]
    fn empty_queue_yields_no_pick() {
        assert_eq!(RoundRobinScheduler.pick(&[]), None);
        assert_eq!(FcfsScheduler.pick(&[]), None);
        assert_eq!(PriorityScheduler.pick(&[]), None);
    }
}
