use kaveon_sql::logical_plan::LogicalPlan;

pub fn push_filter_down(plan: LogicalPlan) -> LogicalPlan {
    // TODO: rule-based filter pushdown
    plan
}
