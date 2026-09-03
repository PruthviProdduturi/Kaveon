use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Stable identifier for a node within one query plan.
pub type PlanNodeId = u32;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PlanPhase {
    Logical,
    OptimizedLogical,
    Physical,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct PlanNode {
    pub id: PlanNodeId,
    pub phase: PlanPhase,
    pub operator: String,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub attributes: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub children: Vec<PlanNode>,
}

impl PlanNode {
    pub fn new(id: PlanNodeId, phase: PlanPhase, operator: impl Into<String>) -> Self {
        Self {
            id,
            phase,
            operator: operator.into(),
            attributes: BTreeMap::new(),
            children: Vec::new(),
        }
    }

    pub fn with_attribute(mut self, name: impl Into<String>, value: impl Into<String>) -> Self {
        self.attributes.insert(name.into(), value.into());
        self
    }

    pub fn with_child(mut self, child: PlanNode) -> Self {
        self.children.push(child);
        self
    }
}

/// Monotonic execution counters captured for one physical-plan node.
///
/// Optional values distinguish unsupported instrumentation from a measured zero.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct OperatorMetrics {
    pub input_rows: Option<u64>,
    pub input_bytes: Option<u64>,
    pub output_rows: Option<u64>,
    pub output_bytes: Option<u64>,
    pub elapsed_ns: Option<u64>,
    pub cpu_ns: Option<u64>,
    pub blocked_ns: Option<u64>,
    pub current_memory_bytes: Option<u64>,
    pub peak_memory_bytes: Option<u64>,
    pub spilled_bytes: Option<u64>,
}

/// Storage-specific counters attached to a scan plan node.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct ScanMetrics {
    pub snapshot_version: Option<u64>,
    pub files_considered: Option<u64>,
    pub files_opened: Option<u64>,
    pub files_pruned: Option<u64>,
    pub row_groups_considered: Option<u64>,
    pub row_groups_read: Option<u64>,
    pub row_groups_pruned: Option<u64>,
    pub columns_available: Option<u64>,
    pub columns_read: Option<u64>,
    pub physical_rows: Option<u64>,
    pub physical_bytes: Option<u64>,
    pub metadata_ns: Option<u64>,
    pub storage_read_ns: Option<u64>,
    pub decode_ns: Option<u64>,
    pub first_batch_ns: Option<u64>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct PlanMetricsSnapshot {
    pub sequence: u64,
    pub captured_at_unix_ms: u64,
    pub nodes: BTreeMap<PlanNodeId, NodeMetrics>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct NodeMetrics {
    pub operator: OperatorMetrics,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scan: Option<ScanMetrics>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_a_structured_plan_tree() {
        let scan = PlanNode::new(1, PlanPhase::Physical, "scan")
            .with_attribute("table", "kaveon.default.events");
        let plan = PlanNode::new(2, PlanPhase::Physical, "aggregate").with_child(scan);

        assert_eq!(plan.operator, "aggregate");
        assert_eq!(
            plan.children[0].attributes["table"],
            "kaveon.default.events"
        );
    }

    #[test]
    fn distinguishes_a_measured_zero_from_an_unavailable_metric() {
        let measured = OperatorMetrics {
            spilled_bytes: Some(0),
            ..OperatorMetrics::default()
        };

        assert_eq!(measured.spilled_bytes, Some(0));
        assert_eq!(measured.cpu_ns, None);
    }

    #[test]
    fn snapshots_are_keyed_by_stable_plan_node_ids() {
        let mut snapshot = PlanMetricsSnapshot::default();
        snapshot.nodes.insert(
            7,
            NodeMetrics {
                operator: OperatorMetrics {
                    output_rows: Some(15),
                    ..OperatorMetrics::default()
                },
                scan: None,
            },
        );

        assert_eq!(snapshot.nodes[&7].operator.output_rows, Some(15));
    }
}
