use std::future::Future;
use std::pin::Pin;
use std::sync::{
    Arc, Mutex, OnceLock,
    atomic::{AtomicU64, Ordering},
};
use std::task::{Context, Poll, Waker};
use std::time::Instant;
use std::{
    any::Any,
    collections::{HashMap, VecDeque},
};

use serde::{Deserialize, Serialize};

use crate::process_memory::ProcessMemory;
use crate::{KaveonError, Result};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MemorySnapshot {
    pub current_bytes: u64,
    pub peak_bytes: u64,
    pub limit_bytes: u64,
    pub reservation_calls: u64,
    pub reservation_bytes: u64,
}

#[derive(Debug)]
struct QueryMemoryInner {
    query_id: Arc<str>,
    limit_bytes: u64,
    current_bytes: AtomicU64,
    peak_bytes: AtomicU64,
    reservation_calls: AtomicU64,
    reservation_bytes: AtomicU64,
    _admission: Option<AdmissionLease>,
    /// The process guard, when the node runs under a memory limit: a
    /// reservation the process could not honour fails here, before the
    /// allocation, whatever the query budget still allows.
    process: Option<ProcessMemory>,
    resources: QueryResources,
    cancellation: CancellationProbe,
}

#[derive(Default)]
struct CancellationProbe(OnceLock<Arc<dyn Fn() -> bool + Send + Sync>>);

impl std::fmt::Debug for CancellationProbe {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("CancellationProbe")
            .field("installed", &self.0.get().is_some())
            .finish()
    }
}

#[derive(Default)]
struct QueryResources(Mutex<HashMap<&'static str, Arc<dyn Any + Send + Sync>>>);

impl std::fmt::Debug for QueryResources {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("QueryResources")
            .finish_non_exhaustive()
    }
}

/// A thread-safe hard memory limit shared by every operator in one query.
#[derive(Debug, Clone)]
pub struct QueryMemoryPool {
    inner: Arc<QueryMemoryInner>,
}

/// The name of the group an arrival is admitted through when the caller
/// names none, and the policy every unknown group name falls back to.
pub const DEFAULT_ADMISSION_GROUP: &str = "default";

/// How many admission waits a group keeps for its wait percentiles.
const WAIT_SAMPLES: usize = 1_024;

/// One resource group's policy in the admission controller: what the
/// group may hold of the pool, how many of its requests run at once, how
/// many may wait, and its weight in the cross-group order.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdmissionGroupPolicy {
    pub name: String,
    /// The sum of the group's admitted budgets cannot exceed this; a
    /// request over it can never be admitted and is refused on arrival.
    pub limit_bytes: u64,
    /// Admitted requests of the group at once. The controller's implicit
    /// group has no bound.
    pub max_concurrent: usize,
    /// Waiting requests of the group at once; the controller's own queue
    /// limit bounds every group together.
    pub max_queued: usize,
    /// The group's weight in the cross-group order (see
    /// [`MemoryAdmissionController::admit_queued_in`]). Must be positive.
    pub weight: u32,
}

/// One group's counters, as `/v1/node` reports them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdmissionGroupStats {
    pub name: String,
    pub limit_bytes: u64,
    pub max_concurrent: usize,
    pub max_queued: usize,
    pub weight: u32,
    /// Admitted requests now.
    pub running: usize,
    /// Waiting requests now.
    pub queued: usize,
    pub admitted_bytes: u64,
    /// Budgets granted, whether immediately or after a wait.
    pub admitted: u64,
    /// Arrivals that had to wait before a decision.
    pub queued_total: u64,
    /// Arrivals refused: over the group's share, a full queue, no
    /// capacity with no wait allowed, or a wait that expired.
    pub rejected: u64,
    /// Arrivals that left the queue before a decision.
    pub withdrawn: u64,
    /// Percentiles of the admission wait over the group's last 1024
    /// admissions, immediate ones counted as zero; absent before the
    /// first admission.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub wait_ms_p50: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub wait_ms_p95: Option<u64>,
}

/// Why an arrival was not admitted. Every variant names what bound it so
/// the caller can answer with the limit and the group.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AdmissionRefusal {
    /// A budget of zero, over the pool, or an empty request identity.
    Invalid(String),
    /// The budget exceeds the group's share of the pool: it can never be
    /// admitted through this group.
    OverGroupLimit {
        group: String,
        limit_bytes: u64,
        requested: u64,
    },
    /// The group already has `max_queued` requests waiting.
    GroupQueueFull { group: String, max_queued: usize },
    /// The controller already has `queue_limit` requests waiting across
    /// every group.
    QueueFull { queue_limit: usize, queued: usize },
    /// No wait was allowed and the arrival could not be admitted now:
    /// the pool or the group's share or its concurrency is taken, or the
    /// arrival is not next under the admission order.
    NoCapacity {
        group: String,
        requested: u64,
        admitted_bytes: u64,
        limit_bytes: u64,
        group_running: usize,
        group_max_concurrent: usize,
        queued_ahead: usize,
    },
}

impl std::fmt::Display for AdmissionRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Invalid(message) => f.write_str(message),
            Self::OverGroupLimit {
                group,
                limit_bytes,
                requested,
            } => write!(
                f,
                "resource group '{group}' admits at most {limit_bytes} bytes per statement share; {requested} bytes requested"
            ),
            Self::GroupQueueFull { group, max_queued } => write!(
                f,
                "resource group '{group}' queue is full: {max_queued} statements already waiting"
            ),
            Self::QueueFull {
                queue_limit,
                queued,
            } => write!(
                f,
                "memory admission queue is full: {queued} of {queue_limit} statements already waiting"
            ),
            Self::NoCapacity {
                group,
                requested,
                admitted_bytes,
                limit_bytes,
                group_running,
                group_max_concurrent,
                queued_ahead,
            } => {
                write!(
                    f,
                    "memory admission rejected query budget of {requested} bytes: {admitted_bytes} of {limit_bytes} bytes already admitted"
                )?;
                if *group_max_concurrent != usize::MAX {
                    write!(
                        f,
                        ", resource group '{group}' running {group_running} of {group_max_concurrent}"
                    )?;
                }
                if *queued_ahead != 0 {
                    write!(f, ", {queued_ahead} waiting ahead")?;
                }
                Ok(())
            }
        }
    }
}

impl std::error::Error for AdmissionRefusal {}

impl From<AdmissionRefusal> for KaveonError {
    fn from(refusal: AdmissionRefusal) -> Self {
        KaveonError::Execution(refusal.to_string())
    }
}

#[derive(Debug)]
struct AdmissionInner {
    limit_bytes: u64,
    /// Mutated only under `groups`; read without it for snapshots.
    admitted_bytes: AtomicU64,
    peak_admitted_bytes: AtomicU64,
    /// Every group's queue, policy and counters. One lock: an admission
    /// decision reads every group's head.
    groups: Mutex<GroupTable>,
    queue_limit: usize,
    next_waiter: AtomicU64,
    admitted_total: AtomicU64,
    queued_total: AtomicU64,
    rejected_total: AtomicU64,
    withdrawn_total: AtomicU64,
}

#[derive(Debug)]
struct GroupTable {
    policies: HashMap<String, AdmissionGroupPolicy>,
    states: HashMap<String, GroupState>,
}

impl GroupTable {
    fn policy(&self, name: &str) -> &AdmissionGroupPolicy {
        self.policies
            .get(name)
            .or_else(|| self.policies.get(DEFAULT_ADMISSION_GROUP))
            .expect("the default policy is always present")
    }

    fn state_mut(&mut self, name: &str) -> &mut GroupState {
        self.states.entry(name.to_owned()).or_default()
    }

    fn queued(&self) -> usize {
        self.states.values().map(|state| state.queue.len()).sum()
    }
}

#[derive(Debug, Default)]
struct GroupState {
    /// Arrivals of the group that could not be admitted, oldest first.
    queue: VecDeque<Arc<AdmissionWaiter>>,
    running: usize,
    admitted_bytes: u64,
    admitted_total: u64,
    queued_total: u64,
    rejected_total: u64,
    withdrawn_total: u64,
    /// The last admissions' waits in milliseconds, for the percentiles.
    waits: VecDeque<u64>,
}

impl GroupState {
    fn record_wait(&mut self, wait_ms: u64) {
        if self.waits.len() == WAIT_SAMPLES {
            self.waits.pop_front();
        }
        self.waits.push_back(wait_ms);
    }

    fn percentile(&self, fraction: f64) -> Option<u64> {
        if self.waits.is_empty() {
            return None;
        }
        let mut sorted: Vec<u64> = self.waits.iter().copied().collect();
        sorted.sort_unstable();
        let rank = ((sorted.len() as f64 - 1.0) * fraction).round() as usize;
        Some(sorted[rank.min(sorted.len() - 1)])
    }
}

/// The admission controller's counters, as `/v1/node` reports them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdmissionStats {
    /// The sum of admitted budgets cannot exceed this.
    pub limit_bytes: u64,
    pub admitted_bytes: u64,
    pub peak_admitted_bytes: u64,
    /// How many arrivals may wait at once; zero refuses every arrival that
    /// does not fit immediately.
    pub queue_limit: usize,
    /// Arrivals waiting now.
    pub queue_depth: usize,
    /// Budgets granted, whether immediately or after a wait.
    pub admitted: u64,
    /// Arrivals that had to wait before a decision.
    pub queued: u64,
    /// Arrivals refused: no capacity with no wait allowed, a full queue, or
    /// a wait that expired.
    pub rejected: u64,
    /// Arrivals that left the queue before a decision: a cancelled
    /// statement or a client that went away.
    pub withdrawn: u64,
}

/// Reserves query memory budgets before execution begins.
#[derive(Debug, Clone)]
pub struct MemoryAdmissionController {
    inner: Arc<AdmissionInner>,
    process: Option<ProcessMemory>,
}

/// A candidate for the next grant: a group's head, or an arrival that
/// asks whether it would be next.
struct Candidate<'a> {
    group: &'a str,
    bytes: u64,
    /// The waiter's id, or `None` for an arrival not yet queued (it is
    /// younger than every queued waiter).
    waiter: Option<u64>,
    admitted_bytes: u64,
    weight: u32,
}

impl MemoryAdmissionController {
    /// A controller that refuses immediately whatever does not fit: no
    /// queue. See [`Self::with_queue_limit`].
    pub fn new(limit_bytes: u64) -> Result<Self> {
        if limit_bytes == 0 {
            return Err(KaveonError::Execution(
                "memory admission limit must be greater than zero".into(),
            ));
        }
        let mut policies = HashMap::new();
        policies.insert(
            DEFAULT_ADMISSION_GROUP.to_owned(),
            Self::implicit_policy(limit_bytes, 0),
        );
        Ok(Self {
            inner: Arc::new(AdmissionInner {
                limit_bytes,
                admitted_bytes: AtomicU64::new(0),
                peak_admitted_bytes: AtomicU64::new(0),
                groups: Mutex::new(GroupTable {
                    policies,
                    states: HashMap::new(),
                }),
                queue_limit: 0,
                next_waiter: AtomicU64::new(0),
                admitted_total: AtomicU64::new(0),
                queued_total: AtomicU64::new(0),
                rejected_total: AtomicU64::new(0),
                withdrawn_total: AtomicU64::new(0),
            }),
            process: None,
        })
    }

    /// The policy of a controller with no groups configured: one group,
    /// the whole pool, no concurrency bound, the controller's queue.
    fn implicit_policy(limit_bytes: u64, queue_limit: usize) -> AdmissionGroupPolicy {
        AdmissionGroupPolicy {
            name: DEFAULT_ADMISSION_GROUP.to_owned(),
            limit_bytes,
            max_concurrent: usize::MAX,
            max_queued: queue_limit,
            weight: 1,
        }
    }

    /// How many arrivals [`Self::admit_queued`] may keep waiting at once,
    /// across every group. Must be called before the controller is shared.
    #[must_use]
    pub fn with_queue_limit(mut self, queue_limit: usize) -> Self {
        if let Some(inner) = Arc::get_mut(&mut self.inner) {
            inner.queue_limit = queue_limit;
            if let Ok(table) = inner.groups.get_mut()
                && let Some(policy) = table.policies.get_mut(DEFAULT_ADMISSION_GROUP)
            {
                policy.max_queued = queue_limit;
            }
        }
        self
    }

    /// Every admitted query's reservations also answer to the process
    /// guard: what the process really holds against the limit it really
    /// has.
    #[must_use]
    pub fn with_process_memory(mut self, process: ProcessMemory) -> Self {
        self.process = Some(process);
        self
    }

    pub fn process_memory(&self) -> Option<&ProcessMemory> {
        self.process.as_ref()
    }

    /// Replaces every group policy at once; takes effect for the next
    /// decision, waiting arrivals included. A group that is not named
    /// keeps its queue and counters and is decided under the `default`
    /// policy. A policy over the pool, without a weight, or without a
    /// `default` group is refused and nothing changes.
    pub fn set_groups(&self, policies: Vec<AdmissionGroupPolicy>) -> Result<()> {
        let mut table = HashMap::with_capacity(policies.len());
        for policy in policies {
            if policy.name.trim().is_empty() || policy.name != policy.name.trim() {
                return Err(KaveonError::Execution(
                    "resource group names must be nonempty and trimmed".into(),
                ));
            }
            if policy.weight == 0 {
                return Err(KaveonError::Execution(format!(
                    "resource group '{}' needs a positive weight",
                    policy.name
                )));
            }
            if policy.limit_bytes == 0 || policy.limit_bytes > self.inner.limit_bytes {
                return Err(KaveonError::Execution(format!(
                    "resource group '{}' memory share must be between 1 and the admission limit of {} bytes",
                    policy.name, self.inner.limit_bytes
                )));
            }
            if policy.max_concurrent == 0 {
                return Err(KaveonError::Execution(format!(
                    "resource group '{}' needs a positive concurrency limit",
                    policy.name
                )));
            }
            if table.insert(policy.name.clone(), policy).is_some() {
                return Err(KaveonError::Execution(
                    "resource group names must be unique".into(),
                ));
            }
        }
        if !table.contains_key(DEFAULT_ADMISSION_GROUP) {
            return Err(KaveonError::Execution(
                "resource groups must include a 'default' group".into(),
            ));
        }
        let mut groups = self.lock_groups();
        groups.policies = table;
        self.grant_waiting(&mut groups);
        Ok(())
    }

    /// The policies in force, `default` included.
    #[must_use]
    pub fn groups(&self) -> Vec<AdmissionGroupPolicy> {
        let groups = self.lock_groups();
        let mut policies: Vec<_> = groups.policies.values().cloned().collect();
        policies.sort_by(|a, b| a.name.cmp(&b.name));
        policies
    }

    /// Admits now or refuses now, through the `default` group.
    pub fn admit(
        &self,
        query_id: impl Into<String>,
        query_limit_bytes: u64,
    ) -> std::result::Result<AdmittedQueryMemory, AdmissionRefusal> {
        self.admit_in(DEFAULT_ADMISSION_GROUP, query_id, query_limit_bytes)
    }

    /// Admits now or refuses now. An arrival is refused when it would not
    /// be the next grant under the admission order: its budget does not
    /// fit the pool or its group's share, its group has no running slot,
    /// someone is queued ahead of it in its group, or the pool is held
    /// for a group further below its share.
    pub fn admit_in(
        &self,
        group: &str,
        query_id: impl Into<String>,
        query_limit_bytes: u64,
    ) -> std::result::Result<AdmittedQueryMemory, AdmissionRefusal> {
        let query_id = self.validate(query_id, query_limit_bytes)?;
        let mut groups = self.lock_groups();
        if let Err(refusal) = self.check_share(&groups, group, query_limit_bytes) {
            return Err(self.refuse(&mut groups, group, refusal));
        }
        if self.would_grant_next(&groups, group, query_limit_bytes) {
            let memory = self.grant(&mut groups, group, &query_id, query_limit_bytes, 0);
            return Ok(memory);
        }
        let refusal = self.no_capacity(&groups, group, query_limit_bytes);
        Err(self.refuse(&mut groups, group, refusal))
    }

    /// [`Self::admit_queued_in`] through the `default` group.
    pub fn admit_queued(
        &self,
        query_id: impl Into<String>,
        query_limit_bytes: u64,
    ) -> std::result::Result<AdmissionWait, AdmissionRefusal> {
        self.admit_queued_in(DEFAULT_ADMISSION_GROUP, query_id, query_limit_bytes)
    }

    /// Admits now when the arrival is next under the admission order;
    /// otherwise joins its group's queue and resolves once granted.
    ///
    /// The order: within a group, strict arrival order — the head is
    /// granted first and only when its whole budget fits, and nothing
    /// behind it is admitted ahead of it. Across groups, on every release
    /// the groups with an eligible head (a running slot free and the head
    /// within the group's share) are ranked by admitted bytes divided by
    /// weight, lowest first, ties to the older head: the group furthest
    /// below its weighted share of the pool is next. When that group's
    /// head does not fit the pool, the pool is held for it — nothing from
    /// another group is admitted until it fits — so a group under its
    /// share cannot be starved by smaller statements of groups over
    /// theirs; every lease ends, so it fits eventually. A group at its
    /// own concurrency or share is not entitled to more and is skipped,
    /// so it cannot stall the others. The decision is a function of the
    /// current admitted bytes alone: no clocks, no virtual time.
    ///
    /// Refused without queueing when the budget is over the group's share,
    /// the group's queue is full, or the controller's queue is full.
    /// Dropping the returned future leaves the queue at once; see
    /// [`AdmissionWait::expire`] for a wait the caller gives up on.
    pub fn admit_queued_in(
        &self,
        group: &str,
        query_id: impl Into<String>,
        query_limit_bytes: u64,
    ) -> std::result::Result<AdmissionWait, AdmissionRefusal> {
        let query_id = self.validate(query_id, query_limit_bytes)?;
        let mut groups = self.lock_groups();
        if let Err(refusal) = self.check_share(&groups, group, query_limit_bytes) {
            return Err(self.refuse(&mut groups, group, refusal));
        }
        if self.would_grant_next(&groups, group, query_limit_bytes) {
            let memory = self.grant(&mut groups, group, &query_id, query_limit_bytes, 0);
            return Ok(AdmissionWait {
                controller: self.clone(),
                waiter: None,
                granted: Some(memory),
            });
        }
        let queued = groups.queued();
        if queued >= self.inner.queue_limit {
            let refusal = AdmissionRefusal::QueueFull {
                queue_limit: self.inner.queue_limit,
                queued,
            };
            return Err(self.refuse(&mut groups, group, refusal));
        }
        let max_queued = groups.policy(group).max_queued;
        if groups
            .states
            .get(group)
            .is_some_and(|state| state.queue.len() >= max_queued)
        {
            let refusal = AdmissionRefusal::GroupQueueFull {
                group: group.to_owned(),
                max_queued,
            };
            return Err(self.refuse(&mut groups, group, refusal));
        }
        let state = groups.state_mut(group);
        let waiter = Arc::new(AdmissionWaiter {
            id: self.inner.next_waiter.fetch_add(1, Ordering::AcqRel),
            group: group.to_owned(),
            query_id,
            bytes: query_limit_bytes,
            arrived: Instant::now(),
            state: Mutex::new(WaiterState::Queued(None)),
        });
        state.queue.push_back(Arc::clone(&waiter));
        state.queued_total += 1;
        drop(groups);
        self.inner.queued_total.fetch_add(1, Ordering::AcqRel);
        Ok(AdmissionWait {
            controller: self.clone(),
            waiter: Some(waiter),
            granted: None,
        })
    }

    #[must_use]
    pub fn snapshot(&self) -> MemorySnapshot {
        MemorySnapshot {
            current_bytes: self.inner.admitted_bytes.load(Ordering::Acquire),
            peak_bytes: self.inner.peak_admitted_bytes.load(Ordering::Acquire),
            limit_bytes: self.inner.limit_bytes,
            reservation_calls: 0,
            reservation_bytes: 0,
        }
    }

    #[must_use]
    pub fn stats(&self) -> AdmissionStats {
        let queue_depth = self.lock_groups().queued();
        AdmissionStats {
            limit_bytes: self.inner.limit_bytes,
            admitted_bytes: self.inner.admitted_bytes.load(Ordering::Acquire),
            peak_admitted_bytes: self.inner.peak_admitted_bytes.load(Ordering::Acquire),
            queue_limit: self.inner.queue_limit,
            queue_depth,
            admitted: self.inner.admitted_total.load(Ordering::Acquire),
            queued: self.inner.queued_total.load(Ordering::Acquire),
            rejected: self.inner.rejected_total.load(Ordering::Acquire),
            withdrawn: self.inner.withdrawn_total.load(Ordering::Acquire),
        }
    }

    /// Every configured group's counters, and those of any group that has
    /// admitted or queued under a policy since removed, by name.
    #[must_use]
    pub fn group_stats(&self) -> Vec<AdmissionGroupStats> {
        let groups = self.lock_groups();
        let mut names: Vec<&str> = groups
            .policies
            .keys()
            .chain(groups.states.keys())
            .map(String::as_str)
            .collect();
        names.sort_unstable();
        names.dedup();
        let empty = GroupState::default();
        names
            .into_iter()
            .map(|name| {
                let policy = groups.policy(name);
                let state = groups.states.get(name).unwrap_or(&empty);
                AdmissionGroupStats {
                    name: name.to_owned(),
                    limit_bytes: policy.limit_bytes,
                    max_concurrent: policy.max_concurrent,
                    max_queued: policy.max_queued,
                    weight: policy.weight,
                    running: state.running,
                    queued: state.queue.len(),
                    admitted_bytes: state.admitted_bytes,
                    admitted: state.admitted_total,
                    queued_total: state.queued_total,
                    rejected: state.rejected_total,
                    withdrawn: state.withdrawn_total,
                    wait_ms_p50: state.percentile(0.5),
                    wait_ms_p95: state.percentile(0.95),
                }
            })
            .collect()
    }

    fn validate(
        &self,
        query_id: impl Into<String>,
        query_limit_bytes: u64,
    ) -> std::result::Result<String, AdmissionRefusal> {
        if query_limit_bytes == 0 || query_limit_bytes > self.inner.limit_bytes {
            return Err(AdmissionRefusal::Invalid(format!(
                "query memory limit {query_limit_bytes} must be between 1 and the admission limit of {} bytes",
                self.inner.limit_bytes
            )));
        }
        let query_id = query_id.into();
        if query_id.trim().is_empty() {
            return Err(AdmissionRefusal::Invalid(
                "query memory pool requires a non-empty query ID".into(),
            ));
        }
        Ok(query_id)
    }

    fn check_share(
        &self,
        groups: &GroupTable,
        group: &str,
        bytes: u64,
    ) -> std::result::Result<(), AdmissionRefusal> {
        let policy = groups.policy(group);
        if bytes > policy.limit_bytes {
            return Err(AdmissionRefusal::OverGroupLimit {
                group: group.to_owned(),
                limit_bytes: policy.limit_bytes,
                requested: bytes,
            });
        }
        Ok(())
    }

    /// Counts a refusal against the group and the controller.
    fn refuse(
        &self,
        groups: &mut GroupTable,
        group: &str,
        refusal: AdmissionRefusal,
    ) -> AdmissionRefusal {
        groups.state_mut(group).rejected_total += 1;
        self.inner.rejected_total.fetch_add(1, Ordering::AcqRel);
        refusal
    }

    fn lock_groups(&self) -> std::sync::MutexGuard<'_, GroupTable> {
        // The table holds waiter handles, policies and counters; a panic
        // while it was held cannot have left a decision half made.
        self.inner
            .groups
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Whether an arrival in `group` with `bytes` would be the next grant
    /// now: nobody queued ahead of it in its group, and the cross-group
    /// order picks it over every other group's head.
    fn would_grant_next(&self, groups: &GroupTable, group: &str, bytes: u64) -> bool {
        if groups
            .states
            .get(group)
            .is_some_and(|state| !state.queue.is_empty())
        {
            return false;
        }
        let arrival = Candidate {
            group,
            bytes,
            waiter: None,
            admitted_bytes: groups
                .states
                .get(group)
                .map_or(0, |state| state.admitted_bytes),
            weight: groups.policy(group).weight,
        };
        match self.next_grant(groups, Some(arrival)) {
            Some(selected) => selected.waiter.is_none(),
            None => false,
        }
    }

    /// The cross-group order over every group's head (and `arrival`, when
    /// given): the first eligible candidate by admitted bytes over weight,
    /// if its budget fits the pool. `None` when nothing is eligible or the
    /// pool is held for the first candidate.
    fn next_grant<'a>(
        &self,
        groups: &'a GroupTable,
        arrival: Option<Candidate<'a>>,
    ) -> Option<Candidate<'a>> {
        let mut candidates: Vec<Candidate<'a>> = groups
            .states
            .iter()
            .filter_map(|(name, state)| {
                let head = state.queue.front()?;
                let policy = groups.policy(name);
                let eligible = state.running < policy.max_concurrent
                    && state.admitted_bytes.saturating_add(head.bytes) <= policy.limit_bytes;
                eligible.then(|| Candidate {
                    group: name.as_str(),
                    bytes: head.bytes,
                    waiter: Some(head.id),
                    admitted_bytes: state.admitted_bytes,
                    weight: policy.weight,
                })
            })
            .collect();
        if let Some(arrival) = arrival {
            let policy = groups.policy(arrival.group);
            let state = groups.states.get(arrival.group);
            let running = state.map_or(0, |state| state.running);
            if running < policy.max_concurrent
                && arrival.admitted_bytes.saturating_add(arrival.bytes) <= policy.limit_bytes
            {
                candidates.push(arrival);
            }
        }
        candidates.sort_by(|a, b| {
            let a_share = u128::from(a.admitted_bytes) * u128::from(b.weight);
            let b_share = u128::from(b.admitted_bytes) * u128::from(a.weight);
            a_share.cmp(&b_share).then_with(|| {
                // An arrival not yet queued is younger than every waiter.
                match (a.waiter, b.waiter) {
                    (Some(x), Some(y)) => x.cmp(&y),
                    (Some(_), None) => std::cmp::Ordering::Less,
                    (None, Some(_)) => std::cmp::Ordering::Greater,
                    (None, None) => std::cmp::Ordering::Equal,
                }
            })
        });
        let first = candidates.into_iter().next()?;
        let admitted = self.inner.admitted_bytes.load(Ordering::Acquire);
        (admitted.saturating_add(first.bytes) <= self.inner.limit_bytes).then_some(first)
    }

    /// Reserves `bytes` for `group` and builds the admitted pool. Called
    /// with the table lock held after the decision, so the check and the
    /// reservation are one step.
    fn grant(
        &self,
        groups: &mut GroupTable,
        group: &str,
        query_id: &str,
        bytes: u64,
        wait_ms: u64,
    ) -> AdmittedQueryMemory {
        let current = self.inner.admitted_bytes.load(Ordering::Acquire);
        let next = current
            .checked_add(bytes)
            .expect("the decision checked the pool");
        debug_assert!(next <= self.inner.limit_bytes, "grant over the pool");
        self.inner.admitted_bytes.store(next, Ordering::Release);
        self.inner
            .peak_admitted_bytes
            .fetch_max(next, Ordering::AcqRel);
        self.inner.admitted_total.fetch_add(1, Ordering::AcqRel);
        let state = groups.state_mut(group);
        state.running += 1;
        state.admitted_bytes += bytes;
        state.admitted_total += 1;
        state.record_wait(wait_ms);
        let mut pool =
            QueryMemoryPool::new(query_id, bytes).expect("query ID and budget were validated");
        let inner = Arc::get_mut(&mut pool.inner).expect("new query pool is uniquely owned");
        inner._admission = Some(AdmissionLease {
            controller: self.clone(),
            group: group.to_owned(),
            admitted_bytes: bytes,
        });
        inner.process = self.process.clone();
        AdmittedQueryMemory { pool }
    }

    /// Grants heads for as long as the order names one that fits. Called
    /// with the table lock held: a waiter still in a queue is in the
    /// `Queued` state, because leaving the queue and changing state happen
    /// under this same lock.
    fn grant_waiting(&self, groups: &mut GroupTable) {
        while let Some(selected) = self.next_grant(groups, None) {
            let group = selected.group.to_owned();
            let head = groups
                .state_mut(&group)
                .queue
                .pop_front()
                .expect("the selected head was just observed");
            let wait_ms = u64::try_from(head.arrived.elapsed().as_millis()).unwrap_or(u64::MAX);
            let memory = self.grant(groups, &group, &head.query_id, head.bytes, wait_ms);
            let previous = {
                let mut state = head.lock_state();
                std::mem::replace(&mut *state, WaiterState::Granted(Some(memory)))
            };
            if let WaiterState::Queued(Some(waker)) = previous {
                waker.wake();
            }
        }
    }

    fn no_capacity(&self, groups: &GroupTable, group: &str, requested: u64) -> AdmissionRefusal {
        let policy = groups.policy(group);
        let state = groups.states.get(group);
        AdmissionRefusal::NoCapacity {
            group: group.to_owned(),
            requested,
            admitted_bytes: self.inner.admitted_bytes.load(Ordering::Acquire),
            limit_bytes: self.inner.limit_bytes,
            group_running: state.map_or(0, |state| state.running),
            group_max_concurrent: policy.max_concurrent,
            queued_ahead: groups.queued(),
        }
    }

    fn release(&self, group: &str, bytes: u64) {
        let mut groups = self.lock_groups();
        let previous = self.inner.admitted_bytes.fetch_sub(bytes, Ordering::AcqRel);
        debug_assert!(previous >= bytes, "memory admission accounting underflow");
        let state = groups.state_mut(group);
        state.running = state.running.saturating_sub(1);
        state.admitted_bytes = state.admitted_bytes.saturating_sub(bytes);
        self.grant_waiting(&mut groups);
    }

    /// Takes `waiter` out of its queue if it is still there, and serves
    /// whoever the order names next. Returns a budget granted to it in the
    /// meantime, which the caller owns.
    fn withdraw(
        &self,
        waiter: &Arc<AdmissionWaiter>,
        rejected: bool,
    ) -> Option<AdmittedQueryMemory> {
        let mut groups = self.lock_groups();
        let state = groups.state_mut(&waiter.group);
        let position = state.queue.iter().position(|queued| queued.id == waiter.id);
        if let Some(position) = position {
            state.queue.remove(position);
        }
        let granted = {
            let mut waiter_state = waiter.lock_state();
            match std::mem::replace(&mut *waiter_state, WaiterState::Withdrawn) {
                WaiterState::Granted(memory) => memory,
                WaiterState::Queued(_) | WaiterState::Withdrawn => None,
            }
        };
        if granted.is_none() {
            if rejected {
                state.rejected_total += 1;
                self.inner.rejected_total.fetch_add(1, Ordering::AcqRel);
            } else {
                state.withdrawn_total += 1;
                self.inner.withdrawn_total.fetch_add(1, Ordering::AcqRel);
            }
        }
        if position.is_some() {
            self.grant_waiting(&mut groups);
        }
        granted
    }
}

#[derive(Debug)]
struct AdmissionWaiter {
    id: u64,
    group: String,
    query_id: String,
    bytes: u64,
    arrived: Instant,
    state: Mutex<WaiterState>,
}

impl AdmissionWaiter {
    fn lock_state(&self) -> std::sync::MutexGuard<'_, WaiterState> {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

#[derive(Debug)]
enum WaiterState {
    /// In the queue, with the waker of the last poll.
    Queued(Option<Waker>),
    /// Out of the queue with its budget, until the future takes it.
    Granted(Option<AdmittedQueryMemory>),
    /// Out of the queue without a budget.
    Withdrawn,
}

/// A pending admission from [`MemoryAdmissionController::admit_queued`].
/// Resolves to the admitted budget; dropping it leaves the queue at once
/// and returns any budget granted in the meantime.
#[derive(Debug)]
pub struct AdmissionWait {
    controller: MemoryAdmissionController,
    waiter: Option<Arc<AdmissionWaiter>>,
    granted: Option<AdmittedQueryMemory>,
}

impl AdmissionWait {
    /// Whether the budget was granted on arrival, without queueing.
    #[must_use]
    pub fn admitted_immediately(&self) -> bool {
        self.waiter.is_none()
    }

    /// Gives up the wait as a refusal: the arrival is counted as rejected
    /// and leaves the queue. A budget granted just before the caller gave
    /// up is returned instead, so no admission is wasted.
    pub fn expire(mut self) -> Option<AdmittedQueryMemory> {
        if let Some(memory) = self.granted.take() {
            return Some(memory);
        }
        let waiter = self.waiter.take()?;
        self.controller.withdraw(&waiter, true)
    }
}

impl Future for AdmissionWait {
    type Output = AdmittedQueryMemory;

    fn poll(mut self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Self::Output> {
        if let Some(memory) = self.granted.take() {
            self.waiter = None;
            return Poll::Ready(memory);
        }
        let Some(waiter) = self.waiter.as_ref() else {
            panic!("admission wait polled after it resolved");
        };
        let mut state = waiter.lock_state();
        match &mut *state {
            WaiterState::Queued(waker) => {
                match waker {
                    Some(waker) if waker.will_wake(context.waker()) => {}
                    _ => *waker = Some(context.waker().clone()),
                }
                Poll::Pending
            }
            WaiterState::Granted(memory) => {
                let memory = memory.take().expect("a granted budget is taken once");
                *state = WaiterState::Withdrawn;
                drop(state);
                self.waiter = None;
                Poll::Ready(memory)
            }
            WaiterState::Withdrawn => panic!("admission wait polled after it resolved"),
        }
    }
}

impl Drop for AdmissionWait {
    fn drop(&mut self) {
        if let Some(waiter) = self.waiter.take() {
            // A budget granted in the meantime is dropped here, outside the
            // table lock, and its release serves the next waiter.
            drop(self.controller.withdraw(&waiter, false));
        }
    }
}

/// An admitted query budget that releases cluster capacity through RAII.
#[derive(Debug)]
pub struct AdmittedQueryMemory {
    pool: QueryMemoryPool,
}

// Held by the shared pool, including operator accounts and live reservations.
// Canceling/dropping a request must not release admission while its worker still
// owns query memory and has not observed cancellation.
#[derive(Debug)]
struct AdmissionLease {
    controller: MemoryAdmissionController,
    group: String,
    admitted_bytes: u64,
}

impl AdmittedQueryMemory {
    #[must_use]
    pub fn pool(&self) -> &QueryMemoryPool {
        &self.pool
    }
}

impl Drop for AdmissionLease {
    fn drop(&mut self) {
        self.controller.release(&self.group, self.admitted_bytes);
    }
}

impl QueryMemoryPool {
    pub fn new(query_id: impl Into<String>, limit_bytes: u64) -> Result<Self> {
        let query_id = query_id.into();
        if query_id.trim().is_empty() {
            return Err(KaveonError::Execution(
                "query memory pool requires a non-empty query ID".into(),
            ));
        }
        if limit_bytes == 0 {
            return Err(KaveonError::Execution(
                "query memory limit must be greater than zero".into(),
            ));
        }

        Ok(Self {
            inner: Arc::new(QueryMemoryInner {
                query_id: query_id.into(),
                limit_bytes,
                current_bytes: AtomicU64::new(0),
                peak_bytes: AtomicU64::new(0),
                reservation_calls: AtomicU64::new(0),
                reservation_bytes: AtomicU64::new(0),
                _admission: None,
                process: None,
                resources: QueryResources::default(),
                cancellation: CancellationProbe::default(),
            }),
        })
    }

    /// A pool whose reservations also answer to `process`.
    #[must_use]
    pub fn with_process_memory(mut self, process: ProcessMemory) -> Self {
        if let Some(inner) = Arc::get_mut(&mut self.inner) {
            inner.process = Some(process);
        }
        self
    }

    #[must_use]
    pub fn query_id(&self) -> &str {
        &self.inner.query_id
    }

    /// Connects synchronous operator loops to an external cancellation token.
    /// The callback must be fast, nonblocking, and must not retain this pool.
    pub fn set_cancellation_probe(
        &self,
        probe: impl Fn() -> bool + Send + Sync + 'static,
    ) -> Result<()> {
        self.inner.cancellation.0.set(Arc::new(probe)).map_err(|_| {
            KaveonError::Execution("query cancellation probe already installed".into())
        })
    }

    pub fn check_cancelled(&self) -> Result<()> {
        if self.inner.cancellation.0.get().is_some_and(|probe| probe()) {
            return Err(KaveonError::Execution("query canceled".into()));
        }
        Ok(())
    }

    #[must_use]
    pub fn snapshot(&self) -> MemorySnapshot {
        MemorySnapshot {
            current_bytes: self.inner.current_bytes.load(Ordering::Acquire),
            peak_bytes: self.inner.peak_bytes.load(Ordering::Acquire),
            limit_bytes: self.inner.limit_bytes,
            reservation_calls: self.inner.reservation_calls.load(Ordering::Acquire),
            reservation_bytes: self.inner.reservation_bytes.load(Ordering::Acquire),
        }
    }

    /// Shares a typed, query-lifetime resource (for example a spill quota) among
    /// operators. Resource initializers must not recursively access this pool.
    /// Resources must not retain the pool itself, which would create a cycle.
    pub fn shared_resource<T: Any + Send + Sync>(
        &self,
        key: &'static str,
        initialize: impl FnOnce() -> Result<T>,
    ) -> Result<Arc<T>> {
        let mut resources =
            self.inner.resources.0.lock().map_err(|_| {
                KaveonError::Execution("query resource registry lock poisoned".into())
            })?;
        if let Some(resource) = resources.get(key) {
            return Arc::clone(resource).downcast::<T>().map_err(|_| {
                KaveonError::Execution(format!("query resource '{key}' has an incompatible type"))
            });
        }
        let resource = Arc::new(initialize()?);
        resources.insert(key, resource.clone());
        Ok(resource)
    }

    /// A shared resource already registered under `key`, if any.
    pub fn shared_resource_if_present<T: Any + Send + Sync>(
        &self,
        key: &'static str,
    ) -> Result<Option<Arc<T>>> {
        let resources =
            self.inner.resources.0.lock().map_err(|_| {
                KaveonError::Execution("query resource registry lock poisoned".into())
            })?;
        resources
            .get(key)
            .map(|resource| {
                Arc::clone(resource).downcast::<T>().map_err(|_| {
                    KaveonError::Execution(format!(
                        "query resource '{key}' has an incompatible type"
                    ))
                })
            })
            .transpose()
    }

    pub fn operator(&self, operator_id: impl Into<String>) -> Result<OperatorMemoryAccount> {
        let operator_id = operator_id.into();
        if operator_id.trim().is_empty() {
            return Err(KaveonError::Execution(
                "operator memory account requires a non-empty operator ID".into(),
            ));
        }

        Ok(OperatorMemoryAccount {
            query: self.clone(),
            operator_id: operator_id.into(),
            current_bytes: Arc::new(AtomicU64::new(0)),
            peak_bytes: Arc::new(AtomicU64::new(0)),
            prepaid: None,
        })
    }

    fn try_reserve(&self, bytes: u64, operator_id: &str) -> Result<()> {
        self.check_cancelled()?;
        if let Some(process) = &self.inner.process
            && !process.can_reserve(bytes)
        {
            return Err(KaveonError::MemoryLimit(format!(
                "query '{}' operator '{}' cannot reserve {bytes} bytes: the process holds {} of {} bytes with {} bytes kept free",
                self.query_id(),
                operator_id,
                process.allocated_bytes(),
                process.limit_bytes(),
                process.headroom_bytes(),
            )));
        }
        let mut current = self.inner.current_bytes.load(Ordering::Acquire);
        loop {
            let Some(next) = current.checked_add(bytes) else {
                return Err(self.limit_error(operator_id, bytes, current));
            };
            if next > self.inner.limit_bytes {
                return Err(self.limit_error(operator_id, bytes, current));
            }

            match self.inner.current_bytes.compare_exchange_weak(
                current,
                next,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => {
                    self.inner.peak_bytes.fetch_max(next, Ordering::AcqRel);
                    return Ok(());
                }
                Err(observed) => current = observed,
            }
        }
    }

    fn limit_error(&self, operator_id: &str, requested: u64, current: u64) -> KaveonError {
        KaveonError::MemoryLimit(format!(
            "query '{}' operator '{}' cannot reserve {requested} bytes: {current} of {} bytes already reserved",
            self.query_id(),
            operator_id,
            self.inner.limit_bytes
        ))
    }

    fn release(&self, bytes: u64) {
        let previous = self.inner.current_bytes.fetch_sub(bytes, Ordering::AcqRel);
        debug_assert!(previous >= bytes, "query memory accounting underflow");
    }
}

/// Per-operator accounting backed by a query-wide hard limit.
#[derive(Debug, Clone)]
pub struct OperatorMemoryAccount {
    query: QueryMemoryPool,
    operator_id: Arc<str>,
    current_bytes: Arc<AtomicU64>,
    peak_bytes: Arc<AtomicU64>,
    /// Bytes taken from the query once and served from here first: an
    /// account with a balance cannot be starved of it by the query's
    /// other operators.
    prepaid: Option<Arc<Prepaid>>,
}

#[derive(Debug)]
struct Prepaid {
    _guard: MemoryReservation,
    available: AtomicU64,
}

impl Prepaid {
    /// Take up to `bytes` from the balance.
    fn take(&self, bytes: u64) -> u64 {
        let mut available = self.available.load(Ordering::Acquire);
        loop {
            let taken = available.min(bytes);
            match self.available.compare_exchange_weak(
                available,
                available - taken,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => return taken,
                Err(observed) => available = observed,
            }
        }
    }

    fn give_back(&self, bytes: u64) {
        self.available.fetch_add(bytes, Ordering::AcqRel);
    }
}

impl OperatorMemoryAccount {
    /// An account of this operator with `bytes` prepaid: taken from the
    /// query now, held for the account's life, and served first to its
    /// reservations — what a reservation needs beyond the balance comes
    /// from the query as usual, and what is released goes back to the
    /// balance first. An operator that must make progress while others
    /// on the query hold what they can — a thread that has just spilled
    /// its table and needs room for its next batch — reserves through
    /// this.
    pub fn prepaid(&self, bytes: u64) -> Result<Self> {
        let guard = self.reserve(bytes)?;
        Ok(Self {
            query: self.query.clone(),
            operator_id: Arc::clone(&self.operator_id),
            current_bytes: Arc::new(AtomicU64::new(0)),
            peak_bytes: Arc::new(AtomicU64::new(0)),
            prepaid: Some(Arc::new(Prepaid {
                _guard: guard,
                available: AtomicU64::new(bytes),
            })),
        })
    }

    /// The prepaid balance not in use, when the account has one.
    #[must_use]
    pub fn prepaid_available(&self) -> u64 {
        self.prepaid
            .as_ref()
            .map_or(0, |prepaid| prepaid.available.load(Ordering::Acquire))
    }

    pub fn check_cancelled(&self) -> Result<()> {
        self.query.check_cancelled()
    }
    #[must_use]
    pub fn operator_id(&self) -> &str {
        &self.operator_id
    }

    #[must_use]
    pub fn query(&self) -> &QueryMemoryPool {
        &self.query
    }

    #[must_use]
    pub fn snapshot(&self) -> MemorySnapshot {
        MemorySnapshot {
            current_bytes: self.current_bytes.load(Ordering::Acquire),
            peak_bytes: self.peak_bytes.load(Ordering::Acquire),
            limit_bytes: self.query.inner.limit_bytes,
            reservation_calls: self.query.inner.reservation_calls.load(Ordering::Acquire),
            reservation_bytes: self.query.inner.reservation_bytes.load(Ordering::Acquire),
        }
    }

    pub fn reserve(&self, bytes: u64) -> Result<MemoryReservation> {
        self.query.check_cancelled()?;
        let prepaid_bytes = self
            .prepaid
            .as_ref()
            .map_or(0, |prepaid| prepaid.take(bytes));
        let from_query = bytes - prepaid_bytes;
        if from_query > 0
            && let Err(error) = self.query.try_reserve(from_query, self.operator_id())
        {
            if let Some(prepaid) = &self.prepaid {
                prepaid.give_back(prepaid_bytes);
            }
            return Err(error);
        }
        self.query
            .inner
            .reservation_calls
            .fetch_add(1, Ordering::Relaxed);
        self.query
            .inner
            .reservation_bytes
            .fetch_add(bytes, Ordering::Relaxed);
        let current = self.current_bytes.fetch_add(bytes, Ordering::AcqRel) + bytes;
        self.peak_bytes.fetch_max(current, Ordering::AcqRel);

        Ok(MemoryReservation {
            account: self.clone(),
            bytes,
            prepaid_bytes,
        })
    }

    fn release(&self, bytes: u64, prepaid_bytes: u64) {
        let previous = self.current_bytes.fetch_sub(bytes, Ordering::AcqRel);
        debug_assert!(previous >= bytes, "operator memory accounting underflow");
        if let Some(prepaid) = &self.prepaid {
            prepaid.give_back(prepaid_bytes);
        }
        self.query.release(bytes - prepaid_bytes);
    }
}

/// Reservations in slabs: one pool round trip per 64 KiB of admitted
/// entries, not one per entry. Operators that admit millions of small
/// items (a distinct key, a build-side key) reserve through this; one
/// `MemoryReservation` per item — an Arc clone and an atomic on the shared
/// pool each — cost more than the item and held tens of millions of guards.
#[derive(Debug, Default)]
pub struct ReservationSlab {
    guards: Vec<MemoryReservation>,
    available: u64,
}

const RESERVATION_SLAB_BYTES: u64 = 64 * 1024;

impl ReservationSlab {
    /// Charge `bytes`, taking a new slab from `memory` when the current one
    /// cannot cover it; a slab that the budget refuses falls back to the
    /// exact amount before failing.
    pub fn reserve(&mut self, memory: &OperatorMemoryAccount, bytes: u64) -> Result<()> {
        if bytes > self.available {
            let slab_bytes = RESERVATION_SLAB_BYTES.max(bytes);
            let guard = match memory.reserve(slab_bytes) {
                Ok(guard) => guard,
                Err(KaveonError::MemoryLimit(_)) if slab_bytes != bytes => memory.reserve(bytes)?,
                Err(error) => return Err(error),
            };
            self.available = self.available.saturating_add(guard.bytes());
            self.guards.push(guard);
        }
        self.available -= bytes;
        Ok(())
    }

    /// Bytes charged through this slab that are still held.
    pub fn charged_bytes(&self) -> u64 {
        self.guards
            .iter()
            .map(MemoryReservation::bytes)
            .sum::<u64>()
            .saturating_sub(self.available)
    }

    /// Release every slab.
    pub fn clear(&mut self) {
        self.guards.clear();
        self.available = 0;
    }

    pub fn into_guards(self) -> Vec<MemoryReservation> {
        self.guards
    }
}

/// An owned reservation that returns its bytes when dropped.
#[derive(Debug)]
#[must_use = "dropping the reservation immediately releases the reserved memory"]
pub struct MemoryReservation {
    account: OperatorMemoryAccount,
    bytes: u64,
    /// The part served from the account's prepaid balance.
    prepaid_bytes: u64,
}

impl MemoryReservation {
    #[must_use]
    pub fn bytes(&self) -> u64 {
        self.bytes
    }

    pub fn release(self) {
        drop(self);
    }
}

impl Drop for MemoryReservation {
    fn drop(&mut self) {
        self.account.release(self.bytes, self.prepaid_bytes);
    }
}

#[cfg(test)]
mod tests {
    use std::{sync::Arc, thread};

    use super::*;

    const QUERY_LIMIT: u64 = 1_024;

    #[test]
    fn prepaid_account_serves_its_balance_first_and_answers_to_the_query_for_the_rest() {
        let pool = QueryMemoryPool::new("query", QUERY_LIMIT).unwrap();
        let sibling = pool.operator("sibling").unwrap();
        let account = pool.operator("merge").unwrap().prepaid(256).unwrap();
        assert_eq!(pool.snapshot().current_bytes, 256);
        assert_eq!(account.prepaid_available(), 256);

        // Within the balance: nothing more from the query.
        let first = account.reserve(200).unwrap();
        assert_eq!(pool.snapshot().current_bytes, 256);
        assert_eq!(account.prepaid_available(), 56);
        assert_eq!(account.snapshot().current_bytes, 200);

        // Beyond it: the rest from the query, and back to the balance
        // first when released.
        let second = account.reserve(100).unwrap();
        assert_eq!(pool.snapshot().current_bytes, 300);
        assert_eq!(account.prepaid_available(), 0);
        drop(second);
        assert_eq!(pool.snapshot().current_bytes, 256);
        assert_eq!(account.prepaid_available(), 56);

        // The sibling takes what the query has left; the balance is still
        // the account's, and a reservation the query refuses beyond it
        // gives the balance back.
        let held = sibling.reserve(QUERY_LIMIT - 256).unwrap();
        assert!(sibling.reserve(1).is_err());
        let within = account.reserve(56).unwrap();
        assert_eq!(account.prepaid_available(), 0);
        drop(within);
        let error = account.reserve(57).unwrap_err();
        assert!(matches!(error, KaveonError::MemoryLimit(_)), "{error}");
        assert_eq!(account.prepaid_available(), 56);

        drop(first);
        assert_eq!(account.prepaid_available(), 256);
        drop(held);
        assert_eq!(pool.snapshot().current_bytes, 256);
        drop(account);
        assert_eq!(pool.snapshot().current_bytes, 0);
    }

    #[test]
    fn validates_pool_and_operator_identity() {
        assert!(QueryMemoryPool::new("", QUERY_LIMIT).is_err());
        assert!(QueryMemoryPool::new("query", 0).is_err());

        let pool = QueryMemoryPool::new("query", QUERY_LIMIT).unwrap();
        assert!(pool.operator(" ").is_err());
    }

    #[test]
    fn admission_lives_until_last_worker_reservation_is_released() {
        let admission = MemoryAdmissionController::new(1024).unwrap();
        let admitted = admission.admit("canceling", 1024).unwrap();
        let account = admitted.pool().operator("worker").unwrap();
        let reservation = account.reserve(64).unwrap();
        drop(admitted);
        drop(account);
        assert_eq!(admission.snapshot().current_bytes, 1024);
        assert!(admission.admit("premature", 1024).is_err());
        drop(reservation);
        assert_eq!(admission.snapshot().current_bytes, 0);
        assert!(admission.admit("next", 1024).is_ok());
    }

    #[test]
    fn query_resources_initialize_once_and_reject_type_collisions() {
        let pool = QueryMemoryPool::new("resources", 1024).unwrap();
        let first = pool
            .shared_resource("disk", || Ok(AtomicU64::new(7)))
            .unwrap();
        let clone = pool.clone();
        let second = clone
            .shared_resource::<AtomicU64>("disk", || panic!("already initialized"))
            .unwrap();
        assert!(Arc::ptr_eq(&first, &second));
        assert_eq!(second.load(Ordering::Relaxed), 7);
        assert!(pool.shared_resource("disk", || Ok(String::new())).is_err());
        assert!(
            pool.shared_resource::<u64>("retry", || Err(KaveonError::Execution("failed".into())))
                .is_err()
        );
        assert_eq!(*pool.shared_resource("retry", || Ok(9_u64)).unwrap(), 9);
    }

    #[test]
    fn cancellation_probe_stops_new_reservations_without_leaking_existing_ones() {
        let pool = QueryMemoryPool::new("cancel", 1024).unwrap();
        let canceled = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let signal = canceled.clone();
        pool.set_cancellation_probe(move || signal.load(Ordering::Acquire))
            .unwrap();
        let account = pool.operator("operator").unwrap();
        let reservation = account.reserve(64).unwrap();
        canceled.store(true, Ordering::Release);
        assert!(account.check_cancelled().is_err());
        assert!(account.reserve(0).is_err());
        assert_eq!(pool.snapshot().current_bytes, 64);
        drop(reservation);
        assert_eq!(pool.snapshot().current_bytes, 0);
        assert!(pool.set_cancellation_probe(|| false).is_err());
    }

    #[test]
    fn reservations_track_current_and_peak_then_release_on_drop() {
        let pool = QueryMemoryPool::new("query", QUERY_LIMIT).unwrap();
        let account = pool.operator("hash-aggregate").unwrap();

        let first = account.reserve(128).unwrap();
        {
            let _second = account.reserve(256).unwrap();
            assert_eq!(account.snapshot().current_bytes, 384);
            assert_eq!(pool.snapshot().peak_bytes, 384);
            assert_eq!(pool.snapshot().reservation_calls, 2);
            assert_eq!(pool.snapshot().reservation_bytes, 384);
        }
        assert_eq!(account.snapshot().current_bytes, 128);
        first.release();

        assert_eq!(account.snapshot().current_bytes, 0);
        assert_eq!(account.snapshot().peak_bytes, 384);
        assert_eq!(pool.snapshot().current_bytes, 0);
        assert_eq!(pool.snapshot().peak_bytes, 384);
    }

    #[test]
    fn query_limit_is_shared_across_operators() {
        let pool = QueryMemoryPool::new("query", QUERY_LIMIT).unwrap();
        let join = pool.operator("join").unwrap();
        let sort = pool.operator("sort").unwrap();
        let _join_memory = join.reserve(768).unwrap();

        let error = sort.reserve(512).unwrap_err();
        assert!(matches!(&error, KaveonError::MemoryLimit(_)));
        assert!(error.to_string().contains("query 'query' operator 'sort'"));
        assert_eq!(pool.snapshot().current_bytes, 768);
        assert_eq!(sort.snapshot().current_bytes, 0);
    }

    #[test]
    fn concurrent_reservations_never_exceed_the_hard_limit() {
        const THREAD_COUNT: usize = 16;
        const RESERVATION_BYTES: u64 = 128;
        const CONCURRENT_LIMIT: u64 = 512;

        let pool = Arc::new(QueryMemoryPool::new("parallel", CONCURRENT_LIMIT).unwrap());
        let handles = (0..THREAD_COUNT)
            .map(|index| {
                let pool = Arc::clone(&pool);
                thread::spawn(move || {
                    let account = pool.operator(format!("operator-{index}")).unwrap();
                    account.reserve(RESERVATION_BYTES).ok()
                })
            })
            .collect::<Vec<_>>();

        let reservations = handles
            .into_iter()
            .filter_map(|handle| handle.join().unwrap())
            .collect::<Vec<_>>();

        assert_eq!(reservations.len(), 4);
        assert_eq!(pool.snapshot().current_bytes, CONCURRENT_LIMIT);
        assert_eq!(pool.snapshot().peak_bytes, CONCURRENT_LIMIT);
        drop(reservations);
        assert_eq!(pool.snapshot().current_bytes, 0);
    }

    #[test]
    fn zero_byte_reservation_is_balanced() {
        let pool = QueryMemoryPool::new("query", QUERY_LIMIT).unwrap();
        let account = pool.operator("projection").unwrap();
        let reservation = account.reserve(0).unwrap();

        assert_eq!(reservation.bytes(), 0);
        drop(reservation);
        assert_eq!(pool.snapshot().current_bytes, 0);
    }

    #[test]
    fn admission_rejects_oversubscription_and_releases_capacity() {
        let admission = MemoryAdmissionController::new(1_024).unwrap();
        let first = admission.admit("first", 768).unwrap();
        let error = admission.admit("second", 512).unwrap_err().to_string();
        assert!(error.contains("memory admission rejected"));
        assert_eq!(admission.snapshot().current_bytes, 768);

        drop(first);
        let second = admission.admit("second", 512).unwrap();
        assert_eq!(second.pool().snapshot().limit_bytes, 512);
        assert_eq!(admission.snapshot().peak_bytes, 768);
        drop(second);
        assert_eq!(admission.snapshot().current_bytes, 0);
    }

    #[test]
    fn reservations_answer_to_the_process_guard_before_the_query_budget() {
        // The query budget would allow it; the process cannot hold it.
        let live = Arc::new(AtomicU64::new(0));
        let probe = live.clone();
        let process = ProcessMemory::with_probe(1_000, 100, move || probe.load(Ordering::Relaxed));
        let admission = MemoryAdmissionController::new(4_096)
            .unwrap()
            .with_process_memory(process);
        let admitted = admission.admit("guarded", 4_096).unwrap();
        let account = admitted.pool().operator("scan").unwrap();
        let held = account.reserve(900).unwrap();
        live.store(950, Ordering::Relaxed);
        let error = account.reserve(1).unwrap_err().to_string();
        assert!(error.contains("the process holds 950 of 1000 bytes"));
        assert!(error.contains("100 bytes kept free"));
        live.store(0, Ordering::Relaxed);
        let more = account.reserve(1).unwrap();
        assert_eq!(admitted.pool().snapshot().current_bytes, 901);
        drop(more);
        drop(held);
        assert_eq!(admitted.pool().snapshot().current_bytes, 0);
    }

    #[test]
    fn admission_validates_global_and_per_query_limits() {
        assert!(MemoryAdmissionController::new(0).is_err());
        let admission = MemoryAdmissionController::new(1_024).unwrap();
        assert!(admission.admit("query", 0).is_err());
        assert!(admission.admit("query", 2_048).is_err());
        assert!(admission.admit(" ", 128).is_err());
        assert_eq!(admission.snapshot().current_bytes, 0);
    }

    /// Polls a wait once with a waker that records being woken.
    struct Woken(std::sync::atomic::AtomicBool);
    impl std::task::Wake for Woken {
        fn wake(self: Arc<Self>) {
            self.0.store(true, Ordering::Release);
        }
    }
    fn poll_once(wait: &mut AdmissionWait, woken: &Arc<Woken>) -> Poll<AdmittedQueryMemory> {
        let waker = Waker::from(Arc::clone(woken));
        let mut context = Context::from_waker(&waker);
        Pin::new(wait).poll(&mut context)
    }
    fn queued_controller(limit: u64, queue_limit: usize) -> MemoryAdmissionController {
        MemoryAdmissionController::new(limit)
            .unwrap()
            .with_queue_limit(queue_limit)
    }

    #[test]
    fn a_queued_arrival_is_admitted_when_the_budget_is_released() {
        let admission = queued_controller(1_024, 8);
        let running = admission.admit("running", 1_024).unwrap();
        let mut wait = admission.admit_queued("waiting", 512).unwrap();
        assert!(!wait.admitted_immediately());
        let woken = Arc::new(Woken(Default::default()));
        assert!(poll_once(&mut wait, &woken).is_pending());
        let stats = admission.stats();
        assert_eq!((stats.queue_depth, stats.queued, stats.admitted), (1, 1, 1));

        drop(running);
        assert!(woken.0.load(Ordering::Acquire), "release wakes the head");
        let Poll::Ready(admitted) = poll_once(&mut wait, &woken) else {
            panic!("the head is granted once its budget fits");
        };
        assert_eq!(admitted.pool().snapshot().limit_bytes, 512);
        let stats = admission.stats();
        assert_eq!(stats.admitted_bytes, 512);
        assert_eq!(
            (stats.queue_depth, stats.admitted, stats.rejected),
            (0, 2, 0)
        );
        drop(admitted);
        assert_eq!(admission.stats().admitted_bytes, 0);
    }

    #[test]
    fn the_queue_is_served_in_arrival_order_and_the_head_blocks_what_fits_behind_it() {
        let admission = queued_controller(1_024, 8);
        let running = admission.admit("running", 768).unwrap();
        // Does not fit behind 768; a later, smaller arrival would fit but
        // waits its turn.
        let mut large = admission.admit_queued("large", 1_024).unwrap();
        let mut small = admission.admit_queued("small", 256).unwrap();
        let woken = Arc::new(Woken(Default::default()));
        assert!(poll_once(&mut large, &woken).is_pending());
        assert!(poll_once(&mut small, &woken).is_pending());
        // An immediate attempt does not jump the queue either.
        let error = admission.admit("impatient", 128).unwrap_err().to_string();
        assert!(error.contains("2 waiting ahead"), "{error}");

        drop(running);
        let Poll::Ready(large) = poll_once(&mut large, &woken) else {
            panic!("the head is granted first");
        };
        assert!(poll_once(&mut small, &woken).is_pending());
        drop(large);
        assert!(matches!(poll_once(&mut small, &woken), Poll::Ready(_)));
        assert_eq!(admission.stats().rejected, 1);
    }

    #[test]
    fn a_full_queue_refuses_and_a_zero_queue_refuses_everything_that_does_not_fit() {
        let admission = queued_controller(1_024, 1);
        let _running = admission.admit("running", 1_024).unwrap();
        let _first = admission.admit_queued("first", 64).unwrap();
        let error = admission
            .admit_queued("second", 64)
            .unwrap_err()
            .to_string();
        assert!(error.contains("queue is full: 1 of 1"), "{error}");
        let stats = admission.stats();
        assert_eq!((stats.queue_depth, stats.queued, stats.rejected), (1, 1, 1));

        let immediate = MemoryAdmissionController::new(1_024).unwrap();
        let _running = immediate.admit("running", 1_024).unwrap();
        assert!(immediate.admit_queued("waiting", 1).is_err());
        assert_eq!(immediate.stats().rejected, 1);
        // Fits: admitted on arrival even with no queue.
        drop(_running);
        let wait = immediate.admit_queued("fits", 1).unwrap();
        assert!(wait.admitted_immediately());
    }

    #[test]
    fn dropping_a_wait_leaves_the_queue_and_serves_the_next_arrival() {
        let admission = queued_controller(1_024, 8);
        let running = admission.admit("running", 1_024).unwrap();
        let cancelled = admission.admit_queued("cancelled", 1_024).unwrap();
        let mut next = admission.admit_queued("next", 512).unwrap();
        let woken = Arc::new(Woken(Default::default()));
        assert!(poll_once(&mut next, &woken).is_pending());
        assert_eq!(admission.stats().queue_depth, 2);

        drop(cancelled);
        let stats = admission.stats();
        assert_eq!(
            (stats.queue_depth, stats.withdrawn, stats.rejected),
            (1, 1, 0)
        );
        // Still no capacity; the release grants the new head.
        assert!(poll_once(&mut next, &woken).is_pending());
        drop(running);
        assert!(matches!(poll_once(&mut next, &woken), Poll::Ready(_)));
    }

    #[test]
    fn an_expired_wait_is_a_rejection_unless_it_was_granted_meanwhile() {
        let admission = queued_controller(1_024, 8);
        let running = admission.admit("running", 1_024).unwrap();
        let expired = admission.admit_queued("expired", 256).unwrap();
        assert!(expired.expire().is_none());
        let stats = admission.stats();
        assert_eq!(
            (stats.queue_depth, stats.rejected, stats.withdrawn),
            (0, 1, 0)
        );

        let granted = admission.admit_queued("granted", 256).unwrap();
        drop(running);
        // Granted before the caller gave up: the budget is handed over, not
        // wasted, and it is not a rejection.
        let memory = granted.expire().expect("granted while waiting");
        assert_eq!(memory.pool().snapshot().limit_bytes, 256);
        assert_eq!(admission.stats().rejected, 1);
        drop(memory);
        assert_eq!(admission.stats().admitted_bytes, 0);
    }

    #[test]
    fn a_wait_resolves_across_threads() {
        let admission = queued_controller(1_024, 8);
        let running = admission.admit("running", 1_024).unwrap();
        let wait = admission.admit_queued("waiting", 1_024).unwrap();
        let waiter = thread::spawn(move || block_on(wait));
        thread::sleep(std::time::Duration::from_millis(20));
        drop(running);
        let admitted = waiter.join().unwrap();
        assert_eq!(admitted.pool().snapshot().limit_bytes, 1_024);
        assert_eq!(admission.stats().admitted, 2);
    }

    fn policy(
        name: &str,
        limit: u64,
        concurrent: usize,
        queued: usize,
        weight: u32,
    ) -> AdmissionGroupPolicy {
        AdmissionGroupPolicy {
            name: name.into(),
            limit_bytes: limit,
            max_concurrent: concurrent,
            max_queued: queued,
            weight,
        }
    }

    fn grouped_controller(
        limit: u64,
        policies: Vec<AdmissionGroupPolicy>,
    ) -> MemoryAdmissionController {
        let admission = queued_controller(limit, 64);
        admission.set_groups(policies).unwrap();
        admission
    }

    #[test]
    fn within_a_group_arrivals_are_served_in_order_and_across_groups_the_lowest_weighted_share_first()
     {
        let admission = grouped_controller(
            1_000,
            vec![
                policy("default", 1_000, usize::MAX, 64, 1),
                policy("batch", 1_000, usize::MAX, 64, 1),
                policy("interactive", 1_000, usize::MAX, 64, 2),
            ],
        );
        let batch_running = admission.admit_in("batch", "b0", 700).unwrap();
        let woken = Arc::new(Woken(Default::default()));
        // batch is first to queue, but holds 700 against weight 1 and its
        // 400 does not fit; interactive holds nothing against weight 2,
        // so it is next and its 300 fits beside the 700.
        let mut batch = admission.admit_queued_in("batch", "b1", 400).unwrap();
        let mut interactive = admission.admit_queued_in("interactive", "i1", 300).unwrap();
        assert!(!batch.admitted_immediately());
        assert!(
            interactive.admitted_immediately(),
            "the group furthest below its weighted share is next"
        );
        assert!(poll_once(&mut batch, &woken).is_pending());
        let Poll::Ready(interactive) = poll_once(&mut interactive, &woken) else {
            panic!("an immediate grant resolves at the first poll");
        };
        assert!(poll_once(&mut batch, &woken).is_pending());
        // Within batch, a later arrival waits behind b1.
        let mut later = admission.admit_queued_in("batch", "b2", 400).unwrap();
        assert!(poll_once(&mut later, &woken).is_pending());
        drop(batch_running);
        let Poll::Ready(_b1) = poll_once(&mut batch, &woken) else {
            panic!("the head of batch is granted on the release");
        };
        assert!(poll_once(&mut later, &woken).is_pending());
        drop(interactive);
        let Poll::Ready(_b2) = poll_once(&mut later, &woken) else {
            panic!("the next in batch follows once its budget fits");
        };
        let stats = admission.group_stats();
        let batch_stats = stats.iter().find(|group| group.name == "batch").unwrap();
        assert_eq!(
            (
                batch_stats.admitted,
                batch_stats.queued_total,
                batch_stats.running
            ),
            (3, 2, 2)
        );
        assert!(batch_stats.wait_ms_p50.is_some() && batch_stats.wait_ms_p95.is_some());
    }

    #[test]
    fn the_pool_is_held_for_the_group_under_its_share_until_its_head_fits() {
        let admission = grouped_controller(
            1_000,
            vec![
                policy("default", 1_000, usize::MAX, 64, 1),
                policy("a", 1_000, usize::MAX, 64, 1),
                policy("b", 1_000, usize::MAX, 64, 1),
                policy("c", 1_000, usize::MAX, 64, 1),
            ],
        );
        let a = admission.admit_in("a", "a0", 700).unwrap();
        let woken = Arc::new(Woken(Default::default()));
        // b and c both hold nothing: tie, b's head is older. b's 500 does
        // not fit beside the 700, so the pool is held for it: c's 200
        // fits but is not admitted ahead of it, nor is an immediate
        // arrival.
        let mut b = admission.admit_queued_in("b", "b0", 500).unwrap();
        let mut c = admission.admit_queued_in("c", "c0", 200).unwrap();
        assert!(poll_once(&mut b, &woken).is_pending());
        assert!(poll_once(&mut c, &woken).is_pending());
        let refusal = admission.admit_in("c", "c1", 100).unwrap_err();
        assert!(
            matches!(
                refusal,
                AdmissionRefusal::NoCapacity {
                    queued_ahead: 2,
                    ..
                }
            ),
            "{refusal}"
        );
        drop(a);
        // b first; c's 200 then fits beside b's 500 and follows at once.
        assert!(matches!(poll_once(&mut b, &woken), Poll::Ready(_)));
        assert!(matches!(poll_once(&mut c, &woken), Poll::Ready(_)));
        assert_eq!(admission.stats().admitted, 3);
    }

    #[test]
    fn a_group_at_its_own_share_or_concurrency_is_skipped_so_it_cannot_stall_the_others() {
        let admission = grouped_controller(
            1_000,
            vec![
                policy("default", 1_000, usize::MAX, 64, 1),
                policy("capped", 300, 1, 64, 1),
                policy("open", 1_000, usize::MAX, 64, 1),
            ],
        );
        let capped_running = admission.admit_in("capped", "k0", 300).unwrap();
        let woken = Arc::new(Woken(Default::default()));
        // capped's head is over the group's share and its one slot is
        // taken: not entitled to more, so open's head is admitted at once
        // even though capped queued first.
        let mut capped = admission.admit_queued_in("capped", "k1", 100).unwrap();
        let open = admission.admit_queued_in("open", "o0", 400).unwrap();
        assert!(open.admitted_immediately());
        assert!(poll_once(&mut capped, &woken).is_pending());
        drop(capped_running);
        let Poll::Ready(capped) = poll_once(&mut capped, &woken) else {
            panic!("the slot and the share are free again");
        };
        assert_eq!(capped.pool().snapshot().limit_bytes, 100);
        drop(open);
        drop(capped);
        assert_eq!(admission.stats().admitted_bytes, 0);
        assert_eq!(admission.stats().rejected, 0);
    }

    #[test]
    fn over_share_and_full_group_queue_are_refused_on_arrival_naming_the_group() {
        let admission = grouped_controller(
            1_000,
            vec![
                policy("default", 1_000, usize::MAX, 64, 1),
                policy("small", 256, 1, 1, 1),
            ],
        );
        let refusal = admission.admit_queued_in("small", "s0", 512).unwrap_err();
        assert_eq!(
            refusal,
            AdmissionRefusal::OverGroupLimit {
                group: "small".into(),
                limit_bytes: 256,
                requested: 512
            }
        );
        let running = admission.admit_in("small", "s1", 256).unwrap();
        let waiting = admission.admit_queued_in("small", "s2", 256).unwrap();
        assert!(!waiting.admitted_immediately());
        let refusal = admission.admit_queued_in("small", "s3", 256).unwrap_err();
        assert_eq!(
            refusal,
            AdmissionRefusal::GroupQueueFull {
                group: "small".into(),
                max_queued: 1
            }
        );
        assert!(refusal.to_string().contains("resource group 'small'"));
        let small = admission
            .group_stats()
            .into_iter()
            .find(|group| group.name == "small")
            .unwrap();
        assert_eq!((small.rejected, small.queued, small.running), (2, 1, 1));
        drop(waiting);
        drop(running);
        assert_eq!(admission.stats().withdrawn, 1);
    }

    #[test]
    fn replacing_the_policies_takes_effect_for_waiting_arrivals_and_unknown_groups_take_the_default()
     {
        let admission = grouped_controller(
            1_000,
            vec![
                policy("default", 1_000, 1, 64, 1),
                policy("etl", 1_000, 1, 64, 1),
            ],
        );
        let running = admission.admit_in("etl", "e0", 100).unwrap();
        let woken = Arc::new(Woken(Default::default()));
        let mut waiting = admission.admit_queued_in("etl", "e1", 100).unwrap();
        assert!(poll_once(&mut waiting, &woken).is_pending());
        // A group the policies do not name is decided as `default`: one
        // slot, which e0 does not hold, so it runs.
        let unknown = admission
            .admit_in("nobody-configured-this", "u0", 100)
            .unwrap();
        assert!(
            admission
                .admit_in("nobody-configured-this", "u1", 100)
                .is_err()
        );
        drop(unknown);
        // Raising etl's concurrency grants the waiter without a release.
        admission
            .set_groups(vec![
                policy("default", 1_000, 1, 64, 1),
                policy("etl", 1_000, 2, 64, 3),
            ])
            .unwrap();
        assert!(matches!(poll_once(&mut waiting, &woken), Poll::Ready(_)));
        assert_eq!(
            admission.groups(),
            vec![
                policy("default", 1_000, 1, 64, 1),
                policy("etl", 1_000, 2, 64, 3)
            ]
        );
        // Refused policies leave the table unchanged.
        assert!(
            admission
                .set_groups(vec![policy("etl", 1_000, 1, 64, 1)])
                .is_err()
        );
        assert!(
            admission
                .set_groups(vec![policy("default", 2_000, 1, 64, 1)])
                .is_err()
        );
        assert!(
            admission
                .set_groups(vec![policy("default", 1_000, 1, 64, 0)])
                .is_err()
        );
        assert_eq!(admission.groups().len(), 2);
        drop(running);
    }

    /// A minimal executor: parks the thread until the waker unparks it.
    fn block_on(mut wait: AdmissionWait) -> AdmittedQueryMemory {
        struct Unpark(thread::Thread);
        impl std::task::Wake for Unpark {
            fn wake(self: Arc<Self>) {
                self.0.unpark();
            }
        }
        let waker = Waker::from(Arc::new(Unpark(thread::current())));
        let mut context = Context::from_waker(&waker);
        loop {
            if let Poll::Ready(memory) = Pin::new(&mut wait).poll(&mut context) {
                return memory;
            }
            thread::park();
        }
    }
}
