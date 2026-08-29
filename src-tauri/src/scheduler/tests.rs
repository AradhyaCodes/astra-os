//! Deterministic scheduler tests. Every test drives [`Scheduler::tick`]
//! explicitly — nothing depends on wall-clock time or thread timing.

use super::{ScheduleClass, Scheduler, Workload};
use crate::kernel::SchedulerAlgorithm;
use crate::process::{Pid, Priority, ProcessState};

fn sched(algorithm: SchedulerAlgorithm) -> Scheduler {
    Scheduler::with_algorithm(algorithm)
}

fn admit(scheduler: &mut Scheduler, pid: Pid, priority: Priority, workload: Workload) {
    scheduler.admit(pid, ScheduleClass::Interactive, priority, workload);
}

/// The PID on each virtual core, `[core 0, core 1]`.
fn cores(scheduler: &Scheduler) -> [Option<Pid>; 2] {
    let snapshot = scheduler.snapshot();
    [snapshot.cores[0].pid, snapshot.cores[1].pid]
}

fn state_of(scheduler: &Scheduler, pid: Pid) -> Option<ProcessState> {
    scheduler
        .snapshot()
        .processes
        .into_iter()
        .find(|view| view.pid == pid)
        .map(|view| view.state)
}

#[test]
fn round_robin_fills_both_cores_front_first_then_rotates_by_quantum() {
    let mut scheduler = sched(SchedulerAlgorithm::RoundRobin);
    for pid in [10, 11, 12, 13] {
        admit(&mut scheduler, pid, Priority::Normal, Workload::burst(100));
    }

    // Tick 1: the two longest-waiting jobs take the two cores.
    scheduler.tick();
    assert_eq!(cores(&scheduler), [Some(10), Some(11)]);
    assert_eq!(scheduler.snapshot().ready_queue, vec![12, 13]);
    assert_eq!(scheduler.context_switches(), 2);

    // The quantum is 4 ticks: 10 and 11 hold their cores through ticks 2-4.
    for _ in 0..3 {
        scheduler.tick();
    }
    assert_eq!(cores(&scheduler), [Some(10), Some(11)]);

    // Tick 5: quantum spent — 10 and 11 are preempted, 12 and 13 take over.
    scheduler.tick();
    assert_eq!(cores(&scheduler), [Some(12), Some(13)]);
    assert_eq!(scheduler.snapshot().ready_queue, vec![10, 11]);
    assert_eq!(scheduler.context_switches(), 4);

    // One more quantum and the first pair rotates back on.
    for _ in 0..4 {
        scheduler.tick();
    }
    assert_eq!(cores(&scheduler), [Some(10), Some(11)]);
    assert_eq!(scheduler.context_switches(), 6);
}

#[test]
fn round_robin_preempts_a_running_process_after_exactly_the_quantum() {
    let mut scheduler = sched(SchedulerAlgorithm::RoundRobin);
    for pid in [1, 2, 3, 4, 5] {
        admit(&mut scheduler, pid, Priority::Normal, Workload::burst(50));
    }

    scheduler.tick();
    assert_eq!(cores(&scheduler), [Some(1), Some(2)]);

    for _ in 0..3 {
        scheduler.tick();
    }
    assert_eq!(
        cores(&scheduler),
        [Some(1), Some(2)],
        "a running job holds its core for the whole quantum"
    );

    scheduler.tick();
    assert_eq!(
        cores(&scheduler),
        [Some(3), Some(4)],
        "preemption happens on the tick the quantum is exhausted"
    );
    // The preempted jobs go to the back of the READY queue, in core order.
    assert_eq!(scheduler.snapshot().ready_queue, vec![5, 1, 2]);
}

#[test]
fn fcfs_runs_each_process_to_completion_in_arrival_order() {
    let mut scheduler = sched(SchedulerAlgorithm::Fcfs);
    for pid in [1, 2, 3, 4] {
        admit(&mut scheduler, pid, Priority::Normal, Workload::burst(3));
    }
    assert_eq!(scheduler.quantum(), None);

    scheduler.tick();
    assert_eq!(cores(&scheduler), [Some(1), Some(2)]);

    scheduler.tick();
    scheduler.tick();
    assert_eq!(
        cores(&scheduler),
        [Some(1), Some(2)],
        "FCFS has no time slice — no preemption"
    );

    // Tick 4: the 3-tick bursts finish, 3 and 4 are dispatched.
    scheduler.tick();
    assert_eq!(cores(&scheduler), [Some(3), Some(4)]);
    assert!(!scheduler.is_tracked(1));
    assert!(!scheduler.is_tracked(2));

    for _ in 0..3 {
        scheduler.tick();
    }
    assert!(!scheduler.is_tracked(3));
    assert!(!scheduler.is_tracked(4));

    // Exactly one dispatch per process.
    assert_eq!(scheduler.context_switches(), 4);
    assert_eq!(scheduler.snapshot().averages.completed, 4);
}

#[test]
fn priority_dispatches_the_most_urgent_ready_processes_first() {
    let mut scheduler = sched(SchedulerAlgorithm::Priority);
    admit(&mut scheduler, 1, Priority::Low, Workload::burst(100));
    admit(&mut scheduler, 2, Priority::System, Workload::burst(100));
    admit(&mut scheduler, 3, Priority::High, Workload::burst(100));
    admit(&mut scheduler, 4, Priority::Normal, Workload::burst(100));

    scheduler.tick();
    assert_eq!(
        cores(&scheduler),
        [Some(2), Some(3)],
        "System then High take the two cores"
    );

    let ready = scheduler.snapshot().ready_queue;
    assert_eq!(ready.len(), 2);
    assert!(ready.contains(&1) && ready.contains(&4));

    // Non-preemptive: Normal and Low keep waiting behind never-yielding jobs.
    for _ in 0..10 {
        scheduler.tick();
    }
    assert_eq!(cores(&scheduler), [Some(2), Some(3)]);
}

#[test]
fn suspended_processes_are_excluded_and_return_on_resume() {
    let mut scheduler = sched(SchedulerAlgorithm::Fcfs);
    for pid in [1, 2, 3] {
        admit(&mut scheduler, pid, Priority::Normal, Workload::burst(100));
    }

    scheduler.suspend(1);
    scheduler.tick();

    assert_eq!(cores(&scheduler), [Some(2), Some(3)]);
    assert!(!scheduler.snapshot().ready_queue.contains(&1));
    assert_eq!(state_of(&scheduler, 1), Some(ProcessState::Suspended));

    for _ in 0..5 {
        scheduler.tick();
    }
    assert!(
        scheduler.core_of(1).is_none(),
        "a suspended process is never dispatched"
    );

    scheduler.resume(1);
    assert!(scheduler.snapshot().ready_queue.contains(&1));
    assert_eq!(state_of(&scheduler, 1), Some(ProcessState::Ready));
}

#[test]
fn a_completed_process_is_removed_from_the_scheduler() {
    let mut scheduler = sched(SchedulerAlgorithm::Fcfs);
    admit(&mut scheduler, 42, Priority::Normal, Workload::burst(2));

    scheduler.tick();
    assert_eq!(scheduler.core_of(42), Some(0));

    scheduler.tick();
    scheduler.tick();

    assert!(!scheduler.is_tracked(42));
    assert!(scheduler.core_of(42).is_none());
    assert!(!scheduler.snapshot().ready_queue.contains(&42));
    assert!(scheduler
        .snapshot()
        .processes
        .iter()
        .all(|view| view.pid != 42));
    assert_eq!(scheduler.snapshot().averages.completed, 1);
}

#[test]
fn context_switches_count_every_dispatch_onto_a_core() {
    let mut scheduler = sched(SchedulerAlgorithm::RoundRobin);
    for pid in [1, 2, 3] {
        admit(&mut scheduler, pid, Priority::Normal, Workload::burst(100));
    }

    scheduler.tick();
    assert_eq!(scheduler.context_switches(), 2);

    for _ in 0..3 {
        scheduler.tick();
    }
    assert_eq!(
        scheduler.context_switches(),
        2,
        "no dispatch while the running jobs keep their cores"
    );

    scheduler.tick();
    assert_eq!(
        scheduler.context_switches(),
        4,
        "quantum expiry frees both cores and dispatches two more jobs"
    );
}

#[test]
fn switching_algorithm_requeues_running_jobs_and_keeps_cumulative_metrics() {
    let mut scheduler = sched(SchedulerAlgorithm::RoundRobin);
    for pid in [1, 2, 3, 4] {
        admit(&mut scheduler, pid, Priority::Normal, Workload::burst(100));
    }

    scheduler.tick();
    let switches_before = scheduler.context_switches();
    let tick_before = scheduler.current_tick();

    scheduler.set_algorithm(SchedulerAlgorithm::Fcfs);
    assert_eq!(scheduler.algorithm(), SchedulerAlgorithm::Fcfs);
    assert_eq!(scheduler.quantum(), None);
    assert_eq!(cores(&scheduler), [None, None]);
    assert_eq!(scheduler.snapshot().ready_queue, vec![1, 2, 3, 4]);
    assert_eq!(scheduler.context_switches(), switches_before);
    assert_eq!(scheduler.current_tick(), tick_before);

    scheduler.tick();
    assert_eq!(cores(&scheduler), [Some(1), Some(2)]);

    for _ in 0..10 {
        scheduler.tick();
    }
    assert_eq!(
        cores(&scheduler),
        [Some(1), Some(2)],
        "FCFS never preempts, unlike the Round-Robin run before the switch"
    );
    assert_eq!(scheduler.context_switches(), switches_before + 2);
}

#[test]
fn a_process_cycles_through_waiting_on_io_and_returns_to_ready() {
    let mut scheduler = sched(SchedulerAlgorithm::Fcfs);
    scheduler.admit(
        1,
        ScheduleClass::Interactive,
        Priority::Normal,
        Workload {
            service_ticks: 6,
            io_every: 3,
            io_burst: 2,
            looping: true,
        },
    );

    scheduler.tick();
    assert_eq!(scheduler.core_of(1), Some(0));

    // Runs ticks 2-4; on tick 4 it has run 3 ticks since its last I/O and blocks.
    for _ in 0..3 {
        scheduler.tick();
    }
    assert_eq!(state_of(&scheduler, 1), Some(ProcessState::Waiting));
    assert!(scheduler.core_of(1).is_none());

    scheduler.tick(); // io_remaining 2 -> 1
    scheduler.tick(); // io_remaining -> 0: back to READY, redispatched same tick
    assert_eq!(scheduler.core_of(1), Some(0));
}

#[test]
fn only_two_cores_are_ever_used() {
    let mut scheduler = sched(SchedulerAlgorithm::RoundRobin);
    for pid in 1..=6 {
        admit(&mut scheduler, pid, Priority::Normal, Workload::burst(100));
    }
    scheduler.tick();
    let snapshot = scheduler.snapshot();
    assert_eq!(snapshot.cores.len(), 2);
    let running = snapshot
        .processes
        .iter()
        .filter(|view| view.core.is_some())
        .count();
    assert_eq!(running, 2, "never more than CORE_COUNT jobs run at once");
    assert_eq!(snapshot.ready_queue.len(), 4);
}
