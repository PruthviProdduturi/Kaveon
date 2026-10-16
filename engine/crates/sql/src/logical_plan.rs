use std::cell::Cell;
use std::collections::HashMap;
use std::rc::Rc;

use crate::parser::parse_sql;
use kaveon_core::predicate::ScalarValue;
use kaveon_core::{
    BinaryOp, CastTarget, DateField, Expr, KaveonError, Percentiles, Result, WindowFrame,
    WindowFrameBound, WindowFrameUnits, expression_contains_column, is_aggregate_function,
};
use sqlparser::ast;
use sqlparser::ast::visit_expressions_mut;
use std::ops::ControlFlow;

/// A top-level ordering key: expression, direction, and null placement.
/// Window specifications keep their existing `(Expr, bool)` representation.
pub type OrderKey = (Expr, bool, bool);

#[derive(Debug, Clone, PartialEq)]
pub enum AggregateExpr {
    Count {
        expr: Expr,
        distinct: bool,
    },
    Sum {
        expr: Expr,
        distinct: bool,
    },
    Avg {
        expr: Expr,
        distinct: bool,
    },
    Min(Expr),
    Max(Expr),
    /// `APPROX_COUNT_DISTINCT(expr)` (`APPROX_DISTINCT` in Trino's name):
    /// the distinct count from a HyperLogLog sketch. `as_count` marks a
    /// `COUNT(DISTINCT expr)` the `approximate` session setting turned
    /// into a sketch: the output keeps COUNT's name, so the projection
    /// written over it still binds.
    ApproxDistinct {
        expr: Expr,
        as_count: bool,
    },
    /// `APPROX_PERCENTILE(expr, p)` or `APPROX_PERCENTILE(expr, ARRAY[p, …])`:
    /// the values at the fractions from a KLL sketch.
    ApproxPercentile {
        expr: Expr,
        percentiles: Percentiles,
    },
    /// `APPROX_COUNT_DISTINCT_STATE(expr)`: the HyperLogLog sketch itself,
    /// as base64 text of its compact encoding, mergeable with any other.
    ApproxDistinctState(Expr),
    /// `COLUMN_STATISTICS(expr)`: the column's read profile over the rows
    /// aggregated — null count, exact bounds, the distinct-count and
    /// quantile sketches — as the JSON `ANALYZE` stores per column.
    ColumnStatistics(Expr),
}

impl AggregateExpr {
    /// The function's canonical upper-case name.
    pub fn function_name(&self) -> &'static str {
        match self {
            Self::Count { .. } => "COUNT",
            Self::Sum { .. } => "SUM",
            Self::Avg { .. } => "AVG",
            Self::Min(_) => "MIN",
            Self::Max(_) => "MAX",
            Self::ApproxDistinct { .. } => "APPROX_COUNT_DISTINCT",
            Self::ApproxPercentile { .. } => "APPROX_PERCENTILE",
            Self::ApproxDistinctState(_) => "APPROX_COUNT_DISTINCT_STATE",
            Self::ColumnStatistics(_) => "COLUMN_STATISTICS",
        }
    }

    /// The expression the aggregate is over (`*` for `COUNT(*)`).
    pub fn argument(&self) -> &Expr {
        match self {
            Self::Count { expr, .. }
            | Self::Sum { expr, .. }
            | Self::Avg { expr, .. }
            | Self::Min(expr)
            | Self::Max(expr)
            | Self::ApproxDistinct { expr, .. }
            | Self::ApproxPercentile { expr, .. }
            | Self::ApproxDistinctState(expr)
            | Self::ColumnStatistics(expr) => expr,
        }
    }

    pub fn argument_mut(&mut self) -> &mut Expr {
        match self {
            Self::Count { expr, .. }
            | Self::Sum { expr, .. }
            | Self::Avg { expr, .. }
            | Self::Min(expr)
            | Self::Max(expr)
            | Self::ApproxDistinct { expr, .. }
            | Self::ApproxPercentile { expr, .. }
            | Self::ApproxDistinctState(expr)
            | Self::ColumnStatistics(expr) => expr,
        }
    }

    /// The function name the statement wrote: `COUNT` for a distinct
    /// count the `approximate` setting answers from a sketch.
    pub fn written_name(&self) -> &'static str {
        match self {
            Self::ApproxDistinct { as_count: true, .. } => "COUNT",
            other => other.function_name(),
        }
    }

    /// The call's arguments as written: the expression, then for
    /// `APPROX_PERCENTILE` the fraction (or `ARRAY[…]` of fractions) as
    /// literals — what a projection's `Expr::Function` over the aggregate
    /// carries, so both name the output alike.
    pub fn arguments(&self) -> Vec<Expr> {
        let mut args = vec![self.argument().clone()];
        if let Self::ApproxPercentile { percentiles, .. } = self {
            let literals = percentiles
                .fractions
                .iter()
                .map(|fraction| Expr::Literal(ScalarValue::Float64(*fraction)))
                .collect::<Vec<_>>();
            args.push(if percentiles.list {
                Expr::Function {
                    name: "ARRAY".into(),
                    args: literals,
                }
            } else {
                literals.into_iter().next().expect("validated non-empty")
            });
        }
        args
    }

    /// The call as an expression, as a projection writes it.
    pub fn as_function(&self) -> Expr {
        Expr::Function {
            name: self.function_name().into(),
            args: self.arguments(),
        }
    }

    /// The output column the planners name this aggregate by when the
    /// statement gives no alias.
    pub fn output_name(&self) -> String {
        kaveon_core::aggregate_output_name(self.written_name(), &self.arguments())
    }

    /// Whether the result is an estimate from a sketch.
    pub fn is_approximate(&self) -> bool {
        matches!(
            self,
            Self::ApproxDistinct { .. } | Self::ApproxPercentile { .. }
        )
    }

    /// `COUNT(DISTINCT expr)` as `APPROX_COUNT_DISTINCT(expr)`: what the
    /// `approximate` session setting asks for. Every other aggregate is
    /// unchanged.
    pub fn approximated(self) -> Self {
        match self {
            Self::Count {
                expr,
                distinct: true,
            } => Self::ApproxDistinct {
                expr,
                as_count: true,
            },
            other => other,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JoinType {
    Inner,
    Left,
    Right,
    Full,
    Cross,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JoinDistribution {
    Partitioned,
    BroadcastRight,
}

#[derive(Debug, Clone)]
pub enum LogicalPlan {
    Scan {
        table: String,
        alias: Option<String>,
        columns: Option<Vec<String>>,
    },
    /// Array expressions expanded into rows by `UNNEST` in `FROM`. Constant
    /// relations use a zero-input fragment; a correlated expression is kept
    /// on the right side of a cross join so the physical planner can route it
    /// through the input-aware lateral operator.
    Unnest {
        expressions: Vec<Expr>,
        columns: Vec<String>,
        with_ordinality: bool,
        ordinality_column: String,
        ordinality_start: i64,
    },
    Join {
        left: Box<LogicalPlan>,
        right: Box<LogicalPlan>,
        join_type: JoinType,
        condition: Option<Expr>,
        /// Column names from `JOIN ... USING`; used by binding to suppress
        /// the duplicate right-hand output columns for wildcard projections.
        using_columns: Option<Vec<String>>,
        distribution: JoinDistribution,
    },
    Filter {
        input: Box<LogicalPlan>,
        predicate: Expr,
    },
    Project {
        input: Box<LogicalPlan>,
        columns: Vec<Expr>,
    },
    Aggregate {
        input: Box<LogicalPlan>,
        group_by: Vec<Expr>,
        aggregates: Vec<AggregateExpr>,
    },
    Sort {
        input: Box<LogicalPlan>,
        order_by: Vec<OrderKey>,
    },
    Limit {
        input: Box<LogicalPlan>,
        count: usize,
    },
    Offset {
        input: Box<LogicalPlan>,
        count: usize,
    },
    Distinct {
        input: Box<LogicalPlan>,
    },
    Union {
        inputs: Vec<LogicalPlan>,
        all: bool,
    },
    Window {
        input: Box<LogicalPlan>,
        window_exprs: Vec<Expr>,
    },
    Intersect {
        left: Box<LogicalPlan>,
        right: Box<LogicalPlan>,
        all: bool,
    },
    Except {
        left: Box<LogicalPlan>,
        right: Box<LogicalPlan>,
        all: bool,
    },
    /// Rows of `left` with a match in `right` on the key. `residual`, when
    /// present, is evaluated over each (left row, right row) pair sharing
    /// a key, and a left row matches when any pair holds: the shape a
    /// correlated `EXISTS` takes when its WHERE carries more than the
    /// equality (`l2.l_orderkey = l1.l_orderkey AND l2.l_suppkey <>
    /// l1.l_suppkey`). Only the binder sets it.
    SemiJoin {
        left: Box<LogicalPlan>,
        right: Box<LogicalPlan>,
        left_key: Expr,
        right_key: Expr,
        residual: Option<Expr>,
    },
    /// Rows of `left` without a match in `right`; `residual` as for
    /// `SemiJoin` (a left row is kept when no pair holds).
    AntiJoin {
        left: Box<LogicalPlan>,
        right: Box<LogicalPlan>,
        left_key: Expr,
        right_key: Expr,
        residual: Option<Expr>,
    },
}

/// A sort whose output is cut to a window: the sorted input, its keys, the
/// rows to skip and the rows to keep.
#[derive(Debug, Clone, Copy)]
pub struct TopNShape<'a> {
    pub input: &'a LogicalPlan,
    pub order_by: &'a [OrderKey],
    pub skip: usize,
    pub fetch: usize,
}

impl TopNShape<'_> {
    /// The rows a top-N must retain to serve the window: the skipped rows
    /// and the fetched ones.
    pub fn retained(&self) -> usize {
        self.skip.saturating_add(self.fetch)
    }
}

impl LogicalPlan {
    /// The aggregates the plan computes, top down.
    pub fn aggregates(&self) -> Vec<&AggregateExpr> {
        let mut out = Vec::new();
        if let LogicalPlan::Aggregate { aggregates, .. } = self {
            out.extend(aggregates.iter());
        }
        for input in self.inputs() {
            out.extend(input.aggregates());
        }
        out
    }

    /// The plan's direct inputs.
    pub fn inputs(&self) -> Vec<&LogicalPlan> {
        match self {
            LogicalPlan::Scan { .. } | LogicalPlan::Unnest { .. } => Vec::new(),
            LogicalPlan::Filter { input, .. }
            | LogicalPlan::Project { input, .. }
            | LogicalPlan::Aggregate { input, .. }
            | LogicalPlan::Sort { input, .. }
            | LogicalPlan::Limit { input, .. }
            | LogicalPlan::Offset { input, .. }
            | LogicalPlan::Distinct { input }
            | LogicalPlan::Window { input, .. } => vec![input.as_ref()],
            LogicalPlan::Join { left, right, .. }
            | LogicalPlan::Intersect { left, right, .. }
            | LogicalPlan::Except { left, right, .. }
            | LogicalPlan::SemiJoin { left, right, .. }
            | LogicalPlan::AntiJoin { left, right, .. } => vec![left.as_ref(), right.as_ref()],
            LogicalPlan::Union { inputs, .. } => inputs.iter().collect(),
        }
    }

    pub fn inputs_mut(&mut self) -> Vec<&mut LogicalPlan> {
        match self {
            LogicalPlan::Scan { .. } | LogicalPlan::Unnest { .. } => Vec::new(),
            LogicalPlan::Filter { input, .. }
            | LogicalPlan::Project { input, .. }
            | LogicalPlan::Aggregate { input, .. }
            | LogicalPlan::Sort { input, .. }
            | LogicalPlan::Limit { input, .. }
            | LogicalPlan::Offset { input, .. }
            | LogicalPlan::Distinct { input }
            | LogicalPlan::Window { input, .. } => vec![input.as_mut()],
            LogicalPlan::Join { left, right, .. }
            | LogicalPlan::Intersect { left, right, .. }
            | LogicalPlan::Except { left, right, .. }
            | LogicalPlan::SemiJoin { left, right, .. }
            | LogicalPlan::AntiJoin { left, right, .. } => vec![left.as_mut(), right.as_mut()],
            LogicalPlan::Union { inputs, .. } => inputs.iter_mut().collect(),
        }
    }

    /// `LIMIT n` straight over a sort, with or without an `OFFSET` between
    /// them: the shape a top-N serves without ever holding the whole
    /// sorted input. `ORDER BY … OFFSET 1000 LIMIT 10` keeps 1010 rows,
    /// not the eighteen million it sorts.
    pub fn top_n(&self) -> Option<TopNShape<'_>> {
        let LogicalPlan::Limit { input, count } = self else {
            return None;
        };
        let (input, skip) = match input.as_ref() {
            LogicalPlan::Offset { input, count } => (input.as_ref(), *count),
            other => (other, 0),
        };
        let LogicalPlan::Sort { input, order_by } = input else {
            return None;
        };
        Some(TopNShape {
            input,
            order_by,
            skip,
            fetch: *count,
        })
    }
}

/// Lower `sql` to a plan that runs as lowered. A subquery may reference
/// only its own relations: an executor that resolves columns by name would
/// read `l1.l_orderkey` inside a subquery over `l2` as `l2`'s own
/// `l_orderkey` and answer wrongly, so a reference to the enclosing query
/// is refused here. The embedded CLI planner takes this path.
pub fn sql_to_logical_plan(sql: &str) -> Result<LogicalPlan> {
    lower(sql, false, false)
}

/// Lower `sql` for the binder (`kaveon_optim::binder::bind`), which runs
/// before the plan does: a subquery may reference the enclosing query, and
/// the binder turns each such reference into a key or a residual of the
/// join that brings the subquery in, or refuses it by name. Both API
/// pipelines take this path; a plan lowered this way must not run unbound.
pub fn sql_to_logical_plan_for_binder(sql: &str) -> Result<LogicalPlan> {
    lower(sql, true, false)
}

/// Lower a statement after binding typed positional values to `$1`, `$2`, …
/// placeholders in the parsed AST. Values are replaced as AST literals, never
/// interpolated into SQL text.
pub fn sql_to_logical_plan_for_binder_with_parameters(
    sql: &str,
    parameters: &[ScalarValue],
) -> Result<LogicalPlan> {
    let parameters = parameters
        .iter()
        .cloned()
        .map(ParameterValue::Scalar)
        .collect::<Vec<_>>();
    sql_to_logical_plan_for_binder_with_parameter_values(sql, &parameters, &HashMap::new())
}

/// A parameter value accepted by the statement binder. Arrays are lowered to
/// the same typed `ARRAY[...]` expression used by SQL literals; nested arrays
/// remain representable without adding container variants to execution
/// `ScalarValue`.
#[derive(Debug, Clone, PartialEq)]
pub enum ParameterValue {
    Scalar(ScalarValue),
    Array(Vec<ParameterValue>),
    /// A map parameter is lowered to the same typed `MAP(keys, values)`
    /// expression used by ordinary SQL. Keys are kept as parameters too so
    /// nested arrays/maps retain their type and null semantics until the
    /// normal expression lowerer validates them.
    Map(Vec<(ParameterValue, ParameterValue)>),
}

/// Lower SQL after binding both positional (`$1`, `?`) and named (`:name`)
/// values to typed AST literals. Binding happens before planning so values are
/// never interpolated into SQL text or sent as an untyped worker parameter.
pub fn sql_to_logical_plan_for_binder_with_parameter_bindings(
    sql: &str,
    parameters: &[ScalarValue],
    named_parameters: &HashMap<String, ScalarValue>,
) -> Result<LogicalPlan> {
    let parameters = parameters
        .iter()
        .cloned()
        .map(ParameterValue::Scalar)
        .collect::<Vec<_>>();
    let named_parameters = named_parameters
        .iter()
        .map(|(name, value)| (name.clone(), ParameterValue::Scalar(value.clone())))
        .collect::<HashMap<_, _>>();
    sql_to_logical_plan_for_binder_with_parameter_values(sql, &parameters, &named_parameters)
}

/// Lower SQL after binding scalar or array parameter values to typed AST
/// literals. Array parameters are recursively converted to `ARRAY[...]`.
pub fn sql_to_logical_plan_for_binder_with_parameter_values(
    sql: &str,
    parameters: &[ParameterValue],
    named_parameters: &HashMap<String, ParameterValue>,
) -> Result<LogicalPlan> {
    let mut statements = parse_sql(sql)?;
    bind_parameters(&mut statements, parameters, named_parameters)?;
    lower_statements(statements, true, false)
}

/// Parameterized form of [`sql_to_logical_plan_for_binder_approximate`].
pub fn sql_to_logical_plan_for_binder_approximate_with_parameters(
    sql: &str,
    parameters: &[ScalarValue],
) -> Result<LogicalPlan> {
    sql_to_logical_plan_for_binder_approximate_with_parameter_bindings(
        sql,
        parameters,
        &HashMap::new(),
    )
}

/// Approximate-count variant of
/// [`sql_to_logical_plan_for_binder_with_parameter_bindings`].
pub fn sql_to_logical_plan_for_binder_approximate_with_parameter_bindings(
    sql: &str,
    parameters: &[ScalarValue],
    named_parameters: &HashMap<String, ScalarValue>,
) -> Result<LogicalPlan> {
    let parameters = parameters
        .iter()
        .cloned()
        .map(ParameterValue::Scalar)
        .collect::<Vec<_>>();
    let named_parameters = named_parameters
        .iter()
        .map(|(name, value)| (name.clone(), ParameterValue::Scalar(value.clone())))
        .collect::<HashMap<_, _>>();
    sql_to_logical_plan_for_binder_approximate_with_parameter_values(
        sql,
        &parameters,
        &named_parameters,
    )
}

/// Approximate-count variant of
/// [`sql_to_logical_plan_for_binder_with_parameter_values`].
pub fn sql_to_logical_plan_for_binder_approximate_with_parameter_values(
    sql: &str,
    parameters: &[ParameterValue],
    named_parameters: &HashMap<String, ParameterValue>,
) -> Result<LogicalPlan> {
    let mut statements = parse_sql(sql)?;
    bind_parameters(&mut statements, parameters, named_parameters)?;
    lower_statements(statements, true, true)
}

/// [`sql_to_logical_plan_for_binder`] with every plain `COUNT(DISTINCT
/// expr)` lowered as `APPROX_COUNT_DISTINCT` named as COUNT
/// ([`AggregateExpr::approximated`]): the `approximate` session setting.
/// Decided here, before a lone `COUNT(DISTINCT)` becomes a DISTINCT stage.
pub fn sql_to_logical_plan_for_binder_approximate(sql: &str) -> Result<LogicalPlan> {
    lower(sql, true, true)
}

fn lower(sql: &str, outer_references: bool, approximate: bool) -> Result<LogicalPlan> {
    let stmts = parse_sql(sql)?;
    lower_statements(stmts, outer_references, approximate)
}

fn lower_statements(
    stmts: Vec<ast::Statement>,
    outer_references: bool,
    approximate: bool,
) -> Result<LogicalPlan> {
    if stmts.is_empty() {
        return Err(sql_err("empty query"));
    }
    if stmts.len() > 1 {
        return Err(sql_err("only single statements are supported"));
    }
    match &stmts[0] {
        ast::Statement::Query(query) => query_to_plan(
            query,
            &Lowering {
                outer_references,
                approximate,
                ..Lowering::default()
            },
        ),
        _ => Err(sql_err("only SELECT queries are supported")),
    }
}

fn bind_parameters(
    statements: &mut Vec<ast::Statement>,
    parameters: &[ParameterValue],
    named_parameters: &HashMap<String, ParameterValue>,
) -> Result<()> {
    let mut error = None;
    let mut question_index = 0usize;
    let mut maximum_positional_index = 0usize;
    let mut used_named_parameters = std::collections::HashSet::new();
    let mut bind = |expr: &mut ast::Expr| {
        let ast::Expr::Value(ast::Value::Placeholder(name)) = expr else {
            return ControlFlow::<()>::Continue(());
        };
        let index = if name == "?" {
            question_index += 1;
            question_index
        } else if let Some(index) = name.strip_prefix('$').and_then(|n| n.parse::<usize>().ok()) {
            maximum_positional_index = maximum_positional_index.max(index);
            index
        } else if let Some(key) = name.strip_prefix(':') {
            used_named_parameters.insert(key.to_owned());
            if let Some(value) = named_parameters.get(key) {
                match parameter_to_ast_expr(value) {
                    Ok(value) => *expr = value,
                    Err(error_value) => {
                        error = Some(error_value);
                        return ControlFlow::Break(());
                    }
                }
                return ControlFlow::Continue(());
            }
            if named_parameters.is_empty() {
                error = Some(sql_err(format!(
                    "unsupported parameter placeholder '{name}'; use $1, $2, …, ?, or named_parameters"
                )));
            } else {
                error = Some(sql_err(format!(
                    "parameter {name} is not bound (received {} named values)",
                    named_parameters.len()
                )));
            }
            return ControlFlow::Break(());
        } else {
            error = Some(sql_err(format!(
                "unsupported parameter placeholder '{name}'; use $1, $2, … or ?"
            )));
            return ControlFlow::Break(());
        };
        maximum_positional_index = maximum_positional_index.max(index);
        if index == 0 || index > parameters.len() {
            error = Some(sql_err(format!(
                "parameter {name} is not bound (received {} values)",
                parameters.len()
            )));
            return ControlFlow::Break(());
        }
        match parameter_to_ast_expr(&parameters[index - 1]) {
            Ok(value) => *expr = value,
            Err(error_value) => {
                error = Some(error_value);
                return ControlFlow::Break(());
            }
        }
        ControlFlow::Continue(())
    };
    let _ = visit_expressions_mut(statements, &mut bind);
    if error.is_none() {
        if parameters.len() > maximum_positional_index {
            error = Some(sql_err(format!(
                "received {} positional values but the query binds {}",
                parameters.len(),
                maximum_positional_index
            )));
        } else if named_parameters
            .keys()
            .any(|name| !used_named_parameters.contains(name))
        {
            let unused = named_parameters
                .keys()
                .find(|name| !used_named_parameters.contains(*name))
                .expect("unused named parameter exists");
            error = Some(sql_err(format!(
                "named parameter :{unused} was supplied but is not used by the query"
            )));
        }
    }
    error.map_or(Ok(()), Err)
}

fn parameter_to_ast_expr(value: &ParameterValue) -> Result<ast::Expr> {
    match value {
        ParameterValue::Scalar(value) => Ok(ast::Expr::Value(scalar_to_ast_value(value))),
        ParameterValue::Array(values) => Ok(ast::Expr::Array(ast::Array {
            elem: values
                .iter()
                .map(parameter_to_ast_expr)
                .collect::<Result<Vec<_>>>()?,
            named: false,
        })),
        ParameterValue::Map(entries) => {
            let keys = entries
                .iter()
                .map(|(key, _)| parameter_to_ast_expr(key))
                .collect::<Result<Vec<_>>>()?;
            let values = entries
                .iter()
                .map(|(_, value)| parameter_to_ast_expr(value))
                .collect::<Result<Vec<_>>>()?;
            Ok(ast::Expr::Function(ast::Function {
                name: ast::ObjectName(vec![ast::Ident::new("MAP")]),
                args: ast::FunctionArguments::List(ast::FunctionArgumentList {
                    args: vec![
                        ast::FunctionArg::Unnamed(ast::FunctionArgExpr::Expr(ast::Expr::Array(
                            ast::Array {
                                elem: keys,
                                named: false,
                            },
                        ))),
                        ast::FunctionArg::Unnamed(ast::FunctionArgExpr::Expr(ast::Expr::Array(
                            ast::Array {
                                elem: values,
                                named: false,
                            },
                        ))),
                    ],
                    duplicate_treatment: None,
                    clauses: vec![],
                }),
                filter: None,
                null_treatment: None,
                over: None,
                uses_odbc_syntax: false,
                parameters: ast::FunctionArguments::None,
                within_group: vec![],
            }))
        }
    }
}

fn scalar_to_ast_value(value: &ScalarValue) -> ast::Value {
    match value {
        ScalarValue::Null => ast::Value::Null,
        ScalarValue::Bool(value) => ast::Value::Boolean(*value),
        ScalarValue::Int64(value) => ast::Value::Number(value.to_string(), false),
        ScalarValue::Float64(value) => ast::Value::Number(value.to_string(), false),
        ScalarValue::Utf8(value) => ast::Value::SingleQuotedString(value.clone()),
        ScalarValue::Decimal128 {
            value,
            precision: _,
            scale,
        } => {
            let negative = *value < 0;
            let digits = value.unsigned_abs().to_string();
            let scale = (*scale).max(0) as usize;
            let text = if scale == 0 {
                digits
            } else if digits.len() <= scale {
                format!("0.{}{}", "0".repeat(scale - digits.len()), digits)
            } else {
                let split = digits.len() - scale;
                format!("{}.{}", &digits[..split], &digits[split..])
            };
            ast::Value::Number(if negative { format!("-{text}") } else { text }, true)
        }
    }
}

/// What a query level lowers against: the CTEs in scope, the
/// statement-wide counter that names scalar subquery columns, and whether
/// a subquery may reference the enclosing query (only when the binder
/// will resolve the reference).
#[derive(Clone, Default)]
struct Lowering {
    ctes: HashMap<String, ast::Query>,
    scalars: Rc<Cell<usize>>,
    outer_references: bool,
    /// Plain distinct counts lower as sketches.
    approximate: bool,
}

impl Lowering {
    fn scalar_name(&self) -> String {
        let ordinal = self.scalars.get();
        self.scalars.set(ordinal + 1);
        format!("__kaveon_scalar_{ordinal}")
    }

    /// A subquery's plan may reference the enclosing query only when the
    /// binder will resolve the reference.
    fn check_subquery(&self, plan: &LogicalPlan) -> Result<()> {
        if self.outer_references {
            Ok(())
        } else {
            validate_uncorrelated(plan)
        }
    }
}

fn query_to_plan(query: &ast::Query, parent: &Lowering) -> Result<LogicalPlan> {
    let mut context = parent.clone();
    if let Some(with) = &query.with {
        if with.recursive {
            return Err(sql_err("recursive CTEs are not supported"));
        }
        for cte in &with.cte_tables {
            let cte_name = cte.alias.name.value.to_lowercase();
            context.ctes.insert(cte_name, *cte.query.clone());
        }
    }
    let ctes = &context;

    // A single SELECT hands back the bindings its aggregate lowering made,
    // so an ORDER BY that repeats a lowered expression (`ORDER BY a - a % 60`
    // beside `GROUP BY a - a % 60`) resolves to the same column.
    let (plan, bindings) = match query.body.as_ref() {
        ast::SetExpr::Select(select) => select_to_plan_with_bindings(select, ctes)?,
        body => (set_expr_to_plan(body, ctes)?, Vec::new()),
    };

    let (plan, visible) = match &query.order_by {
        Some(ob) => build_order_by(plan, &ob.exprs, &bindings)?,
        None => (plan, None),
    };
    let plan = build_limit_offset(plan, &query.limit, &query.offset, query.fetch.as_ref())?;
    // ORDER BY on a column the query does not select: the column rode
    // along through the sort and limit and is dropped here.
    let plan = match visible {
        Some(columns) => LogicalPlan::Project {
            input: Box::new(plan),
            columns,
        },
        None => plan,
    };

    Ok(plan)
}

fn set_expr_to_plan(body: &ast::SetExpr, ctes: &Lowering) -> Result<LogicalPlan> {
    match body {
        ast::SetExpr::Select(select) => select_to_plan(select, ctes),
        ast::SetExpr::Values(values) => {
            let rows = &values.rows;
            if rows.is_empty() {
                return Ok(LogicalPlan::Unnest {
                    expressions: Vec::new(),
                    columns: Vec::new(),
                    with_ordinality: false,
                    ordinality_column: "ordinality".to_owned(),
                    ordinality_start: 1,
                });
            }
            let width = rows[0].len();
            if width == 0 {
                return Err(sql_err("VALUES requires at least one column"));
            }
            if rows.iter().any(|row| row.len() != width) {
                return Err(sql_err("VALUES rows must have the same number of columns"));
            }
            let expressions = (0..width)
                .map(|column| {
                    Ok(Expr::Function {
                        name: "ARRAY".to_owned(),
                        args: rows
                            .iter()
                            .map(|row| ast_expr_to_expr(&row[column]))
                            .collect::<Result<Vec<_>>>()?,
                    })
                })
                .collect::<Result<Vec<_>>>()?;
            Ok(LogicalPlan::Unnest {
                expressions,
                columns: (0..width)
                    .map(|column| format!("column{}", column + 1))
                    .collect(),
                with_ordinality: false,
                ordinality_column: "ordinality".to_owned(),
                ordinality_start: 1,
            })
        }
        ast::SetExpr::SetOperation {
            op,
            left,
            right,
            set_quantifier,
        } => {
            let left_plan = set_expr_to_plan(left, ctes)?;
            let right_plan = set_expr_to_plan(right, ctes)?;
            match op {
                ast::SetOperator::Union => {
                    let all = matches!(set_quantifier, ast::SetQuantifier::All);
                    let plan = LogicalPlan::Union {
                        inputs: vec![left_plan, right_plan],
                        all,
                    };
                    if all {
                        Ok(plan)
                    } else {
                        Ok(LogicalPlan::Distinct {
                            input: Box::new(plan),
                        })
                    }
                }
                ast::SetOperator::Intersect => {
                    let plan = LogicalPlan::Intersect {
                        left: Box::new(left_plan),
                        right: Box::new(right_plan),
                        all: matches!(set_quantifier, ast::SetQuantifier::All),
                    };
                    if !matches!(set_quantifier, ast::SetQuantifier::All) {
                        Ok(LogicalPlan::Distinct {
                            input: Box::new(plan),
                        })
                    } else {
                        Ok(plan)
                    }
                }
                ast::SetOperator::Except => {
                    let plan = LogicalPlan::Except {
                        left: Box::new(left_plan),
                        right: Box::new(right_plan),
                        all: matches!(set_quantifier, ast::SetQuantifier::All),
                    };
                    if !matches!(set_quantifier, ast::SetQuantifier::All) {
                        Ok(LogicalPlan::Distinct {
                            input: Box::new(plan),
                        })
                    } else {
                        Ok(plan)
                    }
                }
            }
        }
        ast::SetExpr::Query(q) => query_to_plan(q, ctes),
        _ => Err(sql_err("unsupported query form")),
    }
}

fn select_to_plan(select: &ast::Select, ctes: &Lowering) -> Result<LogicalPlan> {
    Ok(select_to_plan_with_bindings(select, ctes)?.0)
}

fn resolve_named_window(
    name: &str,
    definitions: &HashMap<String, ast::NamedWindowExpr>,
    visiting: &mut Vec<String>,
) -> Result<ast::WindowSpec> {
    if visiting.iter().any(|candidate| candidate == name) {
        return Err(sql_err(format!(
            "cyclic named window reference involving '{name}'"
        )));
    }
    let definition = definitions
        .get(name)
        .ok_or_else(|| sql_err(format!("unknown named window '{name}'")))?;
    visiting.push(name.to_owned());
    let mut spec = match definition {
        ast::NamedWindowExpr::WindowSpec(spec) => {
            let mut inherited = spec
                .window_name
                .as_ref()
                .map(|parent| resolve_named_window(&parent.value, definitions, visiting))
                .transpose()?
                .unwrap_or(ast::WindowSpec {
                    window_name: None,
                    partition_by: Vec::new(),
                    order_by: Vec::new(),
                    window_frame: None,
                });
            if !spec.partition_by.is_empty() {
                inherited.partition_by = spec.partition_by.clone();
            }
            if !spec.order_by.is_empty() {
                inherited.order_by = spec.order_by.clone();
            }
            if spec.window_frame.is_some() {
                inherited.window_frame = spec.window_frame.clone();
            }
            inherited
        }
        ast::NamedWindowExpr::NamedWindow(parent) => {
            resolve_named_window(&parent.value, definitions, visiting)?
        }
    };
    visiting.pop();
    // The name is useful only while resolving the declaration. Keeping it in
    // the expanded spec would make the normal WindowSpec path try to resolve
    // it again, so the physical planner receives a fully concrete definition.
    spec.window_name = None;
    Ok(spec)
}

/// Expand the SQL `WINDOW name AS (...)` shorthand before lowering expressions.
/// The normal expression lowerer deliberately accepts only concrete specs; doing
/// this at the AST boundary keeps that invariant while supporting direct and
/// chained named-window declarations.
fn expand_named_windows(select: &ast::Select) -> Result<ast::Select> {
    if select.named_window.is_empty() {
        return Ok(select.clone());
    }
    let definitions = select
        .named_window
        .iter()
        .map(|definition| (definition.0.value.clone(), definition.1.clone()))
        .collect::<HashMap<_, _>>();
    let mut expanded = select.clone();
    let mut error = None;
    let mut rewrite = |expr: &mut ast::Expr| {
        let ast::Expr::Function(function) = expr else {
            return ControlFlow::<()>::Continue(());
        };
        let Some(ast::WindowType::NamedWindow(name)) = &function.over else {
            return ControlFlow::Continue(());
        };
        match resolve_named_window(&name.value, &definitions, &mut Vec::new()) {
            Ok(spec) => function.over = Some(ast::WindowType::WindowSpec(spec)),
            Err(err) => {
                error = Some(err);
                return ControlFlow::Break(());
            }
        }
        ControlFlow::Continue(())
    };
    let _ = visit_expressions_mut(&mut expanded, &mut rewrite);
    error.map_or(Ok(expanded), Err)
}

fn select_to_plan_with_bindings(
    select: &ast::Select,
    ctes: &Lowering,
) -> Result<(LogicalPlan, ExpressionBindings)> {
    let select = expand_named_windows(select)?;
    let qualify = select.qualify.as_ref().map(ast_expr_to_expr).transpose()?;
    // Projection scalar subqueries are lowered after the outer aggregation and
    // window stages.  That keeps their single-row relation available to the
    // final projection without making it a grouping key or aggregate input.
    let (projection, projection_scalars) = replace_projection_scalars(&select, ctes)?;
    let mut aliases = Vec::new();
    for item in &projection {
        if let ast::SelectItem::ExprWithAlias { expr, alias } = item {
            aliases.push((Expr::Column(alias.value.clone()), ast_expr_to_expr(expr)?));
        }
    }
    let qualify = qualify.map(|predicate| replace_bound_expression(predicate, &aliases));
    let plan = build_from_clause(&select, ctes)?;
    let plan = build_where(plan, &select, ctes)?;

    let has_aggregates = select.projection.iter().any(contains_aggregate_select_item)
        || select.having.as_ref().is_some_and(contains_aggregate_expr)
        || matches!(&select.group_by, ast::GroupByExpr::Expressions(exprs, _) if !exprs.is_empty());

    let plan = if has_aggregates {
        build_aggregate(plan, &select, ctes.approximate)?
    } else {
        plan
    };

    let (plan, having_scalars) = build_having(plan, &select, ctes)?;

    let window_exprs = collect_window_exprs(&select)?;
    let plan = if window_exprs.is_empty() {
        plan
    } else {
        LogicalPlan::Window {
            input: Box::new(plan),
            window_exprs,
        }
    };

    // QUALIFY runs after window evaluation and before the final projection,
    // matching Trino's evaluation order. Window expressions resolve through
    // the names materialized by the Window operator.
    let plan = if let Some(predicate) = qualify {
        LogicalPlan::Filter {
            input: Box::new(plan),
            predicate,
        }
    } else {
        plan
    };

    let plan = attach_projection_scalars(plan, projection_scalars);
    let plan = build_projection(plan, &projection, has_aggregates)?;

    let plan = if matches!(select.distinct, Some(ast::Distinct::Distinct)) {
        LogicalPlan::Distinct {
            input: Box::new(plan),
        }
    } else {
        plan
    };

    // Every aggregate is lowered to a named column when something above
    // the aggregate must find it by name: a HAVING that compares with a
    // scalar subquery references the aggregate's output across the join
    // that carries the subquery's value, and a projection that computes
    // with an aggregate (`0.2 * avg(x)`) evaluates the expression over the
    // aggregate's output rather than the aggregate itself.
    let force = !having_scalars.is_empty() || select.projection.iter().any(computes_with_aggregate);
    let (plan, bindings) = lower_aggregate_expressions(plan, force)?;
    Ok((attach_having_scalars(plan, having_scalars), bindings))
}

fn replace_projection_scalars(
    select: &ast::Select,
    ctes: &Lowering,
) -> Result<(Vec<ast::SelectItem>, Vec<LogicalPlan>)> {
    let mut projection = Vec::with_capacity(select.projection.len());
    let mut scalars = Vec::new();
    for item in &select.projection {
        let item = match item {
            ast::SelectItem::UnnamedExpr(expr) => ast::SelectItem::UnnamedExpr(
                replace_scalar_subqueries(expr, ctes, &mut |scalar| scalars.push(scalar))?,
            ),
            ast::SelectItem::ExprWithAlias { expr, alias } => ast::SelectItem::ExprWithAlias {
                expr: replace_scalar_subqueries(expr, ctes, &mut |scalar| scalars.push(scalar))?,
                alias: alias.clone(),
            },
            other => other.clone(),
        };
        projection.push(item);
    }
    Ok((projection, scalars))
}

fn attach_projection_scalars(plan: LogicalPlan, scalars: Vec<LogicalPlan>) -> LogicalPlan {
    scalars
        .into_iter()
        .fold(plan, |left, right| LogicalPlan::Join {
            left: Box::new(left),
            right: Box::new(right),
            join_type: JoinType::Cross,
            condition: None,
            using_columns: None,
            distribution: JoinDistribution::Partitioned,
        })
}

/// A projected item that is an expression over an aggregate, rather than
/// an aggregate (or an aliased aggregate) itself.
fn computes_with_aggregate(item: &ast::SelectItem) -> bool {
    let expr = match item {
        ast::SelectItem::UnnamedExpr(expr) | ast::SelectItem::ExprWithAlias { expr, .. } => expr,
        _ => return false,
    };
    let expr = match expr {
        ast::Expr::Nested(inner) => inner.as_ref(),
        other => other,
    };
    !matches!(expr, ast::Expr::Function(_)) && contains_aggregate_expr(expr)
}

/// The scalar subqueries of a HAVING join the aggregate's output: one
/// single-row relation per subquery, cross-joined below the HAVING filter.
fn attach_having_scalars(plan: LogicalPlan, scalars: Vec<LogicalPlan>) -> LogicalPlan {
    if scalars.is_empty() {
        return plan;
    }
    match plan {
        // The first filter from the top is the HAVING: WHERE sits below
        // the aggregate.
        LogicalPlan::Filter { input, predicate } => {
            let mut input = *input;
            for scalar in scalars {
                input = LogicalPlan::Join {
                    left: Box::new(input),
                    right: Box::new(scalar),
                    join_type: JoinType::Cross,
                    condition: None,
                    using_columns: None,
                    distribution: JoinDistribution::Partitioned,
                };
            }
            LogicalPlan::Filter {
                input: Box::new(input),
                predicate: bind_count_star(predicate),
            }
        }
        LogicalPlan::Project { input, columns } => LogicalPlan::Project {
            input: Box::new(attach_having_scalars(*input, scalars)),
            columns,
        },
        LogicalPlan::Distinct { input } => LogicalPlan::Distinct {
            input: Box::new(attach_having_scalars(*input, scalars)),
        },
        LogicalPlan::Window {
            input,
            window_exprs,
        } => LogicalPlan::Window {
            input: Box::new(attach_having_scalars(*input, scalars)),
            window_exprs,
        },
        other => other,
    }
}

/// `COUNT(*)` has no argument to lower, so a HAVING that carries it
/// across the scalar join names its output column directly.
fn bind_count_star(expr: Expr) -> Expr {
    let bind = |expr: Box<Expr>| Box::new(bind_count_star(*expr));
    match expr {
        Expr::Function { name, args }
            if name == "COUNT" && matches!(args.as_slice(), [Expr::Star]) =>
        {
            Expr::Column("count_*".into())
        }
        Expr::BinaryOp { left, op, right } => Expr::BinaryOp {
            left: bind(left),
            op,
            right: bind(right),
        },
        Expr::And(left, right) => Expr::And(bind(left), bind(right)),
        Expr::Or(left, right) => Expr::Or(bind(left), bind(right)),
        Expr::Not(inner) => Expr::Not(bind(inner)),
        Expr::Between {
            expr,
            low,
            high,
            negated,
        } => Expr::Between {
            expr: bind(expr),
            low: bind(low),
            high: bind(high),
            negated,
        },
        other => other,
    }
}

fn build_from_clause(select: &ast::Select, ctes: &Lowering) -> Result<LogicalPlan> {
    if select.from.is_empty() {
        // Trino permits scalar SELECT statements without a relation, for
        // example `SELECT 1` and `SELECT current_date`.  Represent the
        // implicit single row as a one-element UNNEST relation so the normal
        // projection, ordering, limit, and distributed fragment machinery can
        // evaluate the SELECT exactly once without inventing a storage table.
        return Ok(LogicalPlan::Unnest {
            expressions: vec![Expr::Function {
                name: "ARRAY".into(),
                args: vec![Expr::Literal(kaveon_core::ScalarValue::Int64(0))],
            }],
            columns: vec!["_kaveon_unit".into()],
            with_ordinality: false,
            ordinality_column: "_kaveon_ordinality".into(),
            ordinality_start: 1,
        });
    }
    let mut inputs = select.from.iter();
    let first = inputs.next().expect("FROM was checked as non-empty");
    let mut plan = table_factor_to_plan(&first.relation, ctes)?;
    plan = apply_joins(plan, &first.joins, ctes)?;
    for input in inputs {
        let right = apply_joins(
            table_factor_to_plan(&input.relation, ctes)?,
            &input.joins,
            ctes,
        )?;
        plan = LogicalPlan::Join {
            left: Box::new(plan),
            right: Box::new(right),
            join_type: JoinType::Cross,
            condition: None,
            using_columns: None,
            distribution: JoinDistribution::Partitioned,
        };
    }
    Ok(plan)
}

fn is_array_unnest_expression(expression: &Expr) -> bool {
    matches!(expression, Expr::Column(_))
        || matches!(
            expression,
            Expr::Function { name, .. }
                if name.eq_ignore_ascii_case("ARRAY")
                    // SEQUENCE is a first-class Trino array constructor.  It
                    // is evaluated by the expression engine into a typed
                    // LIST, so it is safe to use as a row-producing input.
                    || name.eq_ignore_ascii_case("SEQUENCE")
                    // These functions are also typed LIST producers. Keep
                    // the allow-list explicit so scalar functions and
                    // arbitrary table functions remain fail-closed.
                    || name.eq_ignore_ascii_case("ARRAY_CONCAT")
                    || name.eq_ignore_ascii_case("ARRAY_DISTINCT")
                    || name.eq_ignore_ascii_case("ARRAY_FLATTEN")
                    || name.eq_ignore_ascii_case("ARRAY_REPEAT")
                    || name.eq_ignore_ascii_case("ARRAY_REVERSE")
                    || name.eq_ignore_ascii_case("ARRAY_SLICE")
                    || name.eq_ignore_ascii_case("ARRAY_SORT")
                    || name.eq_ignore_ascii_case("ARRAY_ZIP")
                    || name.eq_ignore_ascii_case("MAP_KEYS")
                    || name.eq_ignore_ascii_case("MAP_VALUES")
                    || name.eq_ignore_ascii_case("MAP_ENTRIES")
        )
}

fn table_factor_to_plan(factor: &ast::TableFactor, ctes: &Lowering) -> Result<LogicalPlan> {
    match factor {
        ast::TableFactor::Table {
            name,
            alias,
            args,
            with_ordinality,
            ..
        } => {
            let table_name = name.to_string();
            let alias_name = alias.as_ref().map(|a| a.name.value.clone());
            if table_name
                .rsplit('.')
                .next()
                .is_some_and(|name| name.eq_ignore_ascii_case("UNNEST"))
            {
                let args = args
                    .as_ref()
                    .ok_or_else(|| sql_err("UNNEST in FROM requires an ARRAY expression"))?;
                if args.args.is_empty() {
                    return Err(sql_err(
                        "UNNEST in FROM requires at least one ARRAY expression",
                    ));
                }
                let expressions = args
                    .args
                    .iter()
                    .map(|argument| {
                        let ast::FunctionArg::Unnamed(ast::FunctionArgExpr::Expr(array)) = argument
                        else {
                            return Err(sql_err("UNNEST in FROM requires an expression"));
                        };
                        let expression = ast_expr_to_expr(array)?;
                        if !is_array_unnest_expression(&expression) {
                            return Err(sql_err("UNNEST in FROM requires an ARRAY expression"));
                        }
                        Ok(expression)
                    })
                    .collect::<Result<Vec<_>>>()?;
                if expressions.iter().any(expression_contains_column) {
                    return Err(sql_err("correlated UNNEST requires CROSS JOIN LATERAL"));
                }
                let output_aliases = alias
                    .as_ref()
                    .map(|alias| {
                        alias
                            .columns
                            .len()
                            .saturating_sub(usize::from(*with_ordinality))
                    })
                    .unwrap_or(expressions.len());
                let columns = alias
                    .as_ref()
                    .map(|alias| {
                        if output_aliases > expressions.len() {
                            alias
                                .columns
                                .iter()
                                .take(output_aliases)
                                .map(|column| column.name.value.clone())
                                .collect()
                        } else {
                            expressions
                                .iter()
                                .enumerate()
                                .map(|(index, _)| {
                                    alias
                                        .columns
                                        .get(index)
                                        .map(|column| column.name.value.clone())
                                        .unwrap_or_else(|| {
                                            if index == 0 {
                                                "unnest".to_owned()
                                            } else {
                                                format!("unnest_{}", index + 1)
                                            }
                                        })
                                })
                                .collect()
                        }
                    })
                    .unwrap_or_else(|| {
                        (0..expressions.len())
                            .map(|index| {
                                if index == 0 {
                                    "unnest".to_owned()
                                } else {
                                    format!("unnest_{}", index + 1)
                                }
                            })
                            .collect()
                    });
                let ordinality_column = alias
                    .as_ref()
                    .and_then(|alias| alias.columns.get(output_aliases))
                    .map(|column| column.name.value.clone())
                    .unwrap_or_else(|| "ordinality".to_owned());
                return Ok(LogicalPlan::Unnest {
                    expressions,
                    columns,
                    with_ordinality: *with_ordinality,
                    ordinality_column,
                    ordinality_start: 1,
                });
            }
            // DuckDB's sqlparser dialect tokenizes row-producing UNNEST in
            // FROM as a table-shaped factor. Handle its function arguments
            // before ordinary table resolution so it cannot become a scan of
            // a table named UNNEST.
            let lookup = alias_name.as_deref().unwrap_or(&table_name).to_lowercase();
            if let Some(cte_query) = ctes
                .ctes
                .get(&table_name.to_lowercase())
                .or_else(|| ctes.ctes.get(&lookup))
            {
                let mut plan = query_to_plan(
                    &ast::Query {
                        with: None,
                        body: cte_query.body.clone(),
                        order_by: cte_query.order_by.clone(),
                        limit: cte_query.limit.clone(),
                        offset: cte_query.offset.clone(),
                        ..cte_query.clone()
                    },
                    ctes,
                )?;
                if alias_name.is_some()
                    && let LogicalPlan::Scan { ref mut alias, .. } = plan
                {
                    *alias = alias_name.clone();
                }
                Ok(plan)
            } else {
                Ok(LogicalPlan::Scan {
                    table: table_name,
                    alias: alias_name,
                    columns: None,
                })
            }
        }
        ast::TableFactor::Derived {
            subquery, alias, ..
        } => {
            let mut plan = query_to_plan(subquery, ctes)?;
            if let Some(alias) = alias
                && !alias.columns.is_empty()
                && let LogicalPlan::Unnest { columns, .. } = &mut plan
            {
                if alias.columns.len() != columns.len() {
                    return Err(sql_err(format!(
                        "derived relation alias has {} columns but the relation has {}",
                        alias.columns.len(),
                        columns.len()
                    )));
                }
                *columns = alias
                    .columns
                    .iter()
                    .map(|column| column.name.value.clone())
                    .collect();
            }
            Ok(plan)
        }
        ast::TableFactor::Function {
            name,
            args,
            alias,
            lateral,
            ..
        } if name
            .0
            .last()
            .is_some_and(|part| part.value.eq_ignore_ascii_case("UNNEST")) =>
        {
            if args.is_empty() {
                return Err(sql_err(
                    "UNNEST in FROM requires at least one ARRAY expression",
                ));
            }
            let expressions = args
                .iter()
                .map(|argument| {
                    let ast::FunctionArg::Unnamed(ast::FunctionArgExpr::Expr(array)) = argument
                    else {
                        return Err(sql_err("UNNEST in FROM requires an expression"));
                    };
                    let expression = ast_expr_to_expr(array)?;
                    if !is_array_unnest_expression(&expression) {
                        return Err(sql_err("UNNEST in FROM requires an ARRAY expression"));
                    }
                    Ok(expression)
                })
                .collect::<Result<Vec<_>>>()?;
            if expressions.iter().any(expression_contains_column) && !*lateral {
                return Err(sql_err("correlated UNNEST requires CROSS JOIN LATERAL"));
            }
            let columns = alias
                .as_ref()
                .map(|alias| {
                    if alias.columns.len() > expressions.len() {
                        alias
                            .columns
                            .iter()
                            .map(|column| column.name.value.clone())
                            .collect()
                    } else {
                        expressions
                            .iter()
                            .enumerate()
                            .map(|(index, _)| {
                                alias
                                    .columns
                                    .get(index)
                                    .map(|column| column.name.value.clone())
                                    .unwrap_or_else(|| {
                                        if index == 0 {
                                            "unnest".to_owned()
                                        } else {
                                            format!("unnest_{}", index + 1)
                                        }
                                    })
                            })
                            .collect()
                    }
                })
                .unwrap_or_else(|| {
                    (0..expressions.len())
                        .map(|index| {
                            if index == 0 {
                                "unnest".to_owned()
                            } else {
                                format!("unnest_{}", index + 1)
                            }
                        })
                        .collect()
                });
            let ordinality_column = alias
                .as_ref()
                .and_then(|alias| alias.columns.get(expressions.len()))
                .map(|column| column.name.value.clone())
                .unwrap_or_else(|| "ordinality".to_owned());
            Ok(LogicalPlan::Unnest {
                expressions,
                columns,
                with_ordinality: false,
                ordinality_column,
                ordinality_start: 1,
            })
        }
        ast::TableFactor::UNNEST {
            alias,
            array_exprs,
            with_offset,
            with_offset_alias,
            with_ordinality,
            ..
        } => {
            if array_exprs.is_empty() {
                return Err(sql_err(
                    "UNNEST in FROM requires at least one ARRAY expression",
                ));
            }
            let expressions = array_exprs
                .iter()
                .map(ast_expr_to_expr)
                .collect::<Result<Vec<_>>>()?;
            if expressions
                .iter()
                .any(|expression| !is_array_unnest_expression(expression))
            {
                return Err(sql_err("UNNEST in FROM requires an ARRAY expression"));
            }
            let columns = alias
                .as_ref()
                .map(|alias| {
                    let output_aliases = alias
                        .columns
                        .len()
                        .saturating_sub(usize::from(*with_offset || *with_ordinality));
                    if output_aliases > expressions.len() {
                        alias
                            .columns
                            .iter()
                            .take(output_aliases)
                            .map(|column| column.name.value.clone())
                            .collect()
                    } else {
                        expressions
                            .iter()
                            .enumerate()
                            .map(|(index, _)| {
                                alias
                                    .columns
                                    .get(index)
                                    .map(|column| column.name.value.clone())
                                    .unwrap_or_else(|| {
                                        if index == 0 {
                                            "unnest".to_owned()
                                        } else {
                                            format!("unnest_{}", index + 1)
                                        }
                                    })
                            })
                            .collect()
                    }
                })
                .unwrap_or_else(|| {
                    (0..expressions.len())
                        .map(|index| {
                            if index == 0 {
                                "unnest".to_owned()
                            } else {
                                format!("unnest_{}", index + 1)
                            }
                        })
                        .collect()
                });
            let ordinality_column = if *with_offset {
                with_offset_alias
                    .as_ref()
                    .map(|column| column.value.clone())
                    .or_else(|| {
                        alias
                            .as_ref()
                            .and_then(|alias| alias.columns.get(expressions.len()))
                            .map(|column| column.name.value.clone())
                    })
                    .unwrap_or_else(|| "offset".to_owned())
            } else {
                alias
                    .as_ref()
                    .and_then(|alias| alias.columns.get(expressions.len()))
                    .map(|column| column.name.value.clone())
                    .unwrap_or_else(|| "ordinality".to_owned())
            };
            Ok(LogicalPlan::Unnest {
                expressions,
                columns,
                with_ordinality: *with_ordinality || *with_offset,
                ordinality_column,
                ordinality_start: if *with_offset { 0 } else { 1 },
            })
        }
        _ => Err(sql_err(
            "only table references and subqueries are supported in FROM",
        )),
    }
}

fn apply_joins(mut left: LogicalPlan, joins: &[ast::Join], ctes: &Lowering) -> Result<LogicalPlan> {
    for join in joins {
        let right = table_factor_to_plan(&join.relation, ctes)?;
        let left_qualifier = join_relation_qualifier(&left);
        let right_qualifier = join_relation_qualifier(&right);
        let (join_type, constraint) = match &join.join_operator {
            ast::JoinOperator::Inner(c) => (JoinType::Inner, Some(c)),
            ast::JoinOperator::LeftOuter(c) => (JoinType::Left, Some(c)),
            ast::JoinOperator::RightOuter(c) => (JoinType::Right, Some(c)),
            ast::JoinOperator::FullOuter(c) => (JoinType::Full, Some(c)),
            ast::JoinOperator::CrossJoin => (JoinType::Cross, None),
            other => return Err(sql_err(format!("unsupported join type: {other:?}"))),
        };
        let using_columns = match constraint {
            Some(ast::JoinConstraint::Using(columns)) => {
                Some(columns.iter().map(|column| column.value.clone()).collect())
            }
            _ => None,
        };
        let condition = match constraint {
            None | Some(ast::JoinConstraint::None) => None,
            Some(ast::JoinConstraint::On(expr)) => Some(ast_expr_to_expr(expr)?),
            Some(ast::JoinConstraint::Using(columns)) => {
                let left_qualifier = left_qualifier.ok_or_else(|| {
                    sql_err("JOIN USING requires a single base relation on the left")
                })?;
                let right_qualifier = right_qualifier.ok_or_else(|| {
                    sql_err("JOIN USING requires a single base relation on the right")
                })?;
                let predicates = columns
                    .iter()
                    .map(|column| {
                        let name = column.value.clone();
                        Expr::BinaryOp {
                            left: Box::new(Expr::Column(format!("{left_qualifier}.{name}"))),
                            op: BinaryOp::Eq,
                            right: Box::new(Expr::Column(format!("{right_qualifier}.{name}"))),
                        }
                    })
                    .collect::<Vec<_>>();
                predicates
                    .into_iter()
                    .reduce(|left, right| Expr::And(Box::new(left), Box::new(right)))
                    .ok_or_else(|| sql_err("JOIN USING requires at least one column"))
                    .map(Some)?
            }
            Some(other) => return Err(sql_err(format!("unsupported join constraint: {other:?}"))),
        };
        if join_type != JoinType::Cross && condition.is_none() {
            return Err(sql_err("non-cross joins require an ON condition"));
        }
        // The physical hash join requires equality keys. An inner join with a
        // residual predicate is still semantically well-defined: lower it to
        // a cross join followed by the original filter so the executor keeps
        // SQL's three-valued predicate semantics. Outer residual joins remain
        // rejected by the physical planner because null-extension requires a
        // dedicated residual-aware outer join implementation.
        if join_type == JoinType::Inner
            && condition
                .as_ref()
                .is_some_and(|expr| !is_equality_join_condition(expr))
        {
            let predicate = condition.expect("checked above");
            left = LogicalPlan::Join {
                left: Box::new(left),
                right: Box::new(right),
                join_type: JoinType::Cross,
                condition: None,
                using_columns: using_columns.clone(),
                distribution: JoinDistribution::Partitioned,
            };
            left = LogicalPlan::Filter {
                input: Box::new(left),
                predicate,
            };
        } else {
            left = LogicalPlan::Join {
                left: Box::new(left),
                right: Box::new(right),
                join_type,
                condition,
                using_columns,
                distribution: JoinDistribution::Partitioned,
            };
        }
    }
    Ok(left)
}

/// Return the qualifier used by a simple relation.  `USING` can be lowered
/// without catalog metadata when both sides are base relations; a composite
/// left side needs name resolution against its complete output schema and is
/// kept explicit until that binder exists.
fn join_relation_qualifier(plan: &LogicalPlan) -> Option<String> {
    match plan {
        LogicalPlan::Scan { table, alias, .. } => Some(
            alias
                .clone()
                .unwrap_or_else(|| table.rsplit('.').next().unwrap_or(table).to_owned()),
        ),
        LogicalPlan::Filter { input, .. } => join_relation_qualifier(input),
        _ => None,
    }
}

fn is_equality_join_condition(expr: &Expr) -> bool {
    match expr {
        Expr::BinaryOp {
            left,
            op: kaveon_core::BinaryOp::Eq,
            right,
        } => matches!((&**left, &**right), (Expr::Column(_), Expr::Column(_))),
        Expr::And(left, right) => {
            is_equality_join_condition(left) && is_equality_join_condition(right)
        }
        _ => false,
    }
}

fn build_where(plan: LogicalPlan, select: &ast::Select, ctes: &Lowering) -> Result<LogicalPlan> {
    match &select.selection {
        None => Ok(plan),
        Some(expr) => {
            let mut plan = plan;
            let mut remaining = Vec::new();
            extract_subquery_predicates(expr, &mut plan, &mut remaining, ctes)?;
            if remaining.is_empty() {
                Ok(plan)
            } else {
                let predicate = remaining
                    .into_iter()
                    .reduce(|a, b| Expr::And(Box::new(a), Box::new(b)))
                    .unwrap();
                Ok(LogicalPlan::Filter {
                    input: Box::new(plan),
                    predicate,
                })
            }
        }
    }
}

fn extract_subquery_predicates(
    expr: &ast::Expr,
    plan: &mut LogicalPlan,
    remaining: &mut Vec<Expr>,
    ctes: &Lowering,
) -> Result<()> {
    match expr {
        ast::Expr::BinaryOp {
            left,
            op: ast::BinaryOperator::And,
            right,
        } => {
            extract_subquery_predicates(left, plan, remaining, ctes)?;
            extract_subquery_predicates(right, plan, remaining, ctes)?;
            Ok(())
        }
        ast::Expr::InSubquery {
            expr: lhs,
            subquery,
            negated,
        } => {
            let left_key = ast_expr_to_expr(lhs)?;
            let sub_plan = query_to_plan(subquery, ctes)?;
            ctes.check_subquery(&sub_plan)?;
            // The physical operator binds the sole projected output by position.
            let right_key = Expr::Column("*".into());
            let current = std::mem::replace(
                plan,
                LogicalPlan::Scan {
                    table: String::new(),
                    alias: None,
                    columns: None,
                },
            );
            if *negated {
                *plan = LogicalPlan::AntiJoin {
                    left: Box::new(current),
                    right: Box::new(sub_plan),
                    left_key,
                    right_key,
                    residual: None,
                };
            } else {
                *plan = LogicalPlan::SemiJoin {
                    left: Box::new(current),
                    right: Box::new(sub_plan),
                    left_key,
                    right_key,
                    residual: None,
                };
            }
            Ok(())
        }
        ast::Expr::Exists { subquery, negated } => {
            let sub_plan = query_to_plan(subquery, ctes)?;
            ctes.check_subquery(&sub_plan)?;
            // Existence depends on row cardinality, including NULL-valued
            // rows. A literal key marks the subquery as uncorrelated as
            // lowered; the binder replaces it with the correlation's key.
            let right_key = Expr::Literal(ScalarValue::Int64(1));
            let left_key = right_key.clone();
            let current = std::mem::replace(
                plan,
                LogicalPlan::Scan {
                    table: String::new(),
                    alias: None,
                    columns: None,
                },
            );
            if *negated {
                *plan = LogicalPlan::AntiJoin {
                    left: Box::new(current),
                    right: Box::new(sub_plan),
                    left_key,
                    right_key,
                    residual: None,
                };
            } else {
                *plan = LogicalPlan::SemiJoin {
                    left: Box::new(current),
                    right: Box::new(sub_plan),
                    left_key,
                    right_key,
                    residual: None,
                };
            }
            Ok(())
        }
        ast::Expr::Nested(inner) => extract_subquery_predicates(inner, plan, remaining, ctes),
        other => {
            let rewritten = replace_scalar_subqueries(other, ctes, &mut |scalar| {
                let current = std::mem::replace(
                    plan,
                    LogicalPlan::Scan {
                        table: String::new(),
                        alias: None,
                        columns: None,
                    },
                );
                *plan = LogicalPlan::Join {
                    left: Box::new(current),
                    right: Box::new(scalar),
                    join_type: JoinType::Cross,
                    condition: None,
                    using_columns: None,
                    distribution: JoinDistribution::Partitioned,
                };
            })?;
            remaining.push(ast_expr_to_expr(&rewritten)?);
            Ok(())
        }
    }
}

/// Each scalar subquery in `expr` becomes one column of a single-row
/// relation, handed to `sink` to join into the query, and the expression
/// refers to that column. `x < (SELECT avg(y) FROM t)` is `x <
/// __kaveon_scalar_0` beside a cross join with the one-row aggregate; the
/// binder later turns a subquery correlated with the query into a join on
/// the correlation. Only subqueries in comparison positions are lowered;
/// one nested deeper keeps its error.
fn replace_scalar_subqueries(
    expr: &ast::Expr,
    ctes: &Lowering,
    sink: &mut dyn FnMut(LogicalPlan),
) -> Result<ast::Expr> {
    Ok(match expr {
        ast::Expr::Subquery(query) => {
            let name = ctes.scalar_name();
            let plan = query_to_plan(query, ctes)?;
            ctes.check_subquery(&plan)?;
            sink(scalar_subquery_plan(plan, &name)?);
            ast::Expr::Identifier(ast::Ident::new(name))
        }
        ast::Expr::BinaryOp { left, op, right } => ast::Expr::BinaryOp {
            left: Box::new(replace_scalar_subqueries(left, ctes, sink)?),
            op: op.clone(),
            right: Box::new(replace_scalar_subqueries(right, ctes, sink)?),
        },
        ast::Expr::Nested(inner) => {
            ast::Expr::Nested(Box::new(replace_scalar_subqueries(inner, ctes, sink)?))
        }
        ast::Expr::UnaryOp { op, expr } => ast::Expr::UnaryOp {
            op: *op,
            expr: Box::new(replace_scalar_subqueries(expr, ctes, sink)?),
        },
        ast::Expr::Between {
            expr,
            negated,
            low,
            high,
        } => ast::Expr::Between {
            expr: Box::new(replace_scalar_subqueries(expr, ctes, sink)?),
            negated: *negated,
            low: Box::new(replace_scalar_subqueries(low, ctes, sink)?),
            high: Box::new(replace_scalar_subqueries(high, ctes, sink)?),
        },
        other => other.clone(),
    })
}

/// The subquery's plan as a single-row relation whose one column is
/// `name`: an ungrouped aggregate (possibly under HAVING) selecting one
/// expression. Anything else could yield several rows, which a scalar
/// position cannot take.
fn scalar_subquery_plan(plan: LogicalPlan, name: &str) -> Result<LogicalPlan> {
    if let LogicalPlan::Limit { input, count } = &plan
        && *count > 0
        && let LogicalPlan::Offset {
            input: offset_input,
            count: offset_count,
        } = input.as_ref()
        && let LogicalPlan::Sort {
            input: sorted_input,
            order_by,
        } = offset_input.as_ref()
        && let LogicalPlan::Project {
            input: projected_input,
            columns,
        } = sorted_input.as_ref()
    {
        let [column] = columns.as_slice() else {
            return Err(sql_err("a scalar subquery must select exactly one column"));
        };
        if matches!(column, Expr::Star) {
            return Err(sql_err("a scalar subquery must select exactly one column"));
        }
        let aliased = LogicalPlan::Project {
            input: projected_input.clone(),
            columns: vec![
                column.clone(),
                Expr::Alias {
                    expr: Box::new(column.clone()),
                    name: name.to_owned(),
                },
            ],
        };
        let sorted = LogicalPlan::Sort {
            input: Box::new(aliased),
            order_by: order_by.clone(),
        };
        let bounded = LogicalPlan::Limit {
            input: Box::new(LogicalPlan::Offset {
                input: Box::new(sorted),
                count: *offset_count,
            }),
            count: *count,
        };
        return Ok(LogicalPlan::Project {
            input: Box::new(bounded),
            columns: vec![Expr::Alias {
                expr: Box::new(Expr::Column(name.to_owned())),
                name: name.to_owned(),
            }],
        });
    }
    if let LogicalPlan::Limit { input, count } = &plan
        && *count > 0
        && let LogicalPlan::Sort {
            input: sorted_input,
            order_by,
        } = input.as_ref()
        && let LogicalPlan::Project {
            input: projected_input,
            columns,
        } = sorted_input.as_ref()
    {
        let [column] = columns.as_slice() else {
            return Err(sql_err("a scalar subquery must select exactly one column"));
        };
        if matches!(column, Expr::Star) {
            return Err(sql_err("a scalar subquery must select exactly one column"));
        }
        let aliased = LogicalPlan::Project {
            input: projected_input.clone(),
            columns: vec![
                column.clone(),
                Expr::Alias {
                    expr: Box::new(column.clone()),
                    name: name.to_owned(),
                },
            ],
        };
        let sorted = LogicalPlan::Sort {
            input: Box::new(aliased),
            order_by: order_by.clone(),
        };
        return Ok(LogicalPlan::Project {
            input: Box::new(LogicalPlan::Limit {
                input: Box::new(sorted),
                count: *count,
            }),
            columns: vec![Expr::Alias {
                expr: Box::new(Expr::Column(name.to_owned())),
                name: name.to_owned(),
            }],
        });
    }
    // A bounded scalar relation may carry an OFFSET below its positive LIMIT.
    // Keep the bound (and any ordering) while aliasing the source expression;
    // without this case the planner rejects the otherwise safe
    // `SELECT value FROM t OFFSET n LIMIT 1` shape because the Offset node is
    // hidden beneath the scalar's top-level Limit.
    if let LogicalPlan::Limit { input, count } = &plan
        && *count > 0
        && let LogicalPlan::Offset {
            input: offset_input,
            count: offset_count,
        } = input.as_ref()
        && let LogicalPlan::Project {
            input: projected_input,
            columns,
        } = offset_input.as_ref()
    {
        let [column] = columns.as_slice() else {
            return Err(sql_err("a scalar subquery must select exactly one column"));
        };
        if matches!(column, Expr::Star) {
            return Err(sql_err("a scalar subquery must select exactly one column"));
        }
        let source = LogicalPlan::Project {
            input: projected_input.clone(),
            columns: vec![
                column.clone(),
                Expr::Alias {
                    expr: Box::new(column.clone()),
                    name: name.to_owned(),
                },
            ],
        };
        let bounded = LogicalPlan::Limit {
            input: Box::new(LogicalPlan::Offset {
                input: Box::new(source),
                count: *offset_count,
            }),
            count: *count,
        };
        return Ok(LogicalPlan::Project {
            input: Box::new(bounded),
            columns: vec![Expr::Alias {
                expr: Box::new(Expr::Column(name.to_owned())),
                name: name.to_owned(),
            }],
        });
    }
    // An ungrouped aggregate is already a one-row relation. A positive
    // top-level LIMIT cannot change that scalar value, so remove it before
    // validating the scalar shape. LIMIT 0 is retained: an empty scalar
    // requires null-fill/Apply semantics that this lowering does not provide.
    let (plan, positive_limit, zero_limit, positive_offset) = match plan {
        LogicalPlan::Limit { input, count } if count > 0 => {
            (LogicalPlan::Limit { input, count }, true, false, false)
        }
        LogicalPlan::Limit { input, count: 0 } => (*input, false, true, false),
        LogicalPlan::Offset { input, count } if count > 0 => (*input, false, false, true),
        plan => (plan, false, false, false),
    };
    let (bounded_limit, input, columns) = match plan {
        LogicalPlan::Limit { input, count } if positive_limit => {
            let LogicalPlan::Project { input, columns } = *input else {
                return Err(sql_err("a scalar subquery must select exactly one column"));
            };
            (Some(count), input, columns)
        }
        LogicalPlan::Project { input, columns } if matches!(input.as_ref(), LogicalPlan::Limit { count, .. } if *count > 0) =>
        {
            let count = match input.as_ref() {
                LogicalPlan::Limit { count, .. } => *count,
                _ => unreachable!(),
            };
            (Some(count), input, columns)
        }
        LogicalPlan::Project { input, columns } => (None, input, columns),
        _ => return Err(sql_err("a scalar subquery must select exactly one column")),
    };
    let [column] = columns.as_slice() else {
        return Err(sql_err("a scalar subquery must select exactly one column"));
    };
    if matches!(column, Expr::Star) {
        return Err(sql_err("a scalar subquery must select exactly one column"));
    }
    fn single_row(plan: &LogicalPlan) -> bool {
        match plan {
            LogicalPlan::Aggregate { group_by, .. } => group_by.is_empty(),
            // A HAVING filter over an ungrouped aggregate still emits one
            // row (possibly with NULL aggregate values). A WHERE filter over
            // a constant relation can emit zero rows, so it is not safe to
            // model as a cross-joined scalar without an Apply/null-fill
            // operator.
            LogicalPlan::Filter { input, .. } => {
                matches!(input.as_ref(), LogicalPlan::Aggregate { group_by, .. } if group_by.is_empty())
            }
            // `SELECT 42` and constant `UNNEST(ARRAY[...])` relations are
            // statically single-row when every array has at most one value.
            LogicalPlan::Unnest { expressions, .. } => expressions.iter().all(|expr| {
                matches!(
                    expr,
                    Expr::Function { name, args }
                        if name == "ARRAY" && args.len() <= 1
                )
            }),
            _ => false,
        }
    }
    if zero_limit || (positive_offset && single_row(&input)) {
        return Ok(LogicalPlan::Unnest {
            expressions: vec![Expr::Function {
                name: "ARRAY".to_owned(),
                args: vec![Expr::Literal(ScalarValue::Null)],
            }],
            columns: vec![name.to_owned()],
            with_ordinality: false,
            ordinality_column: "ordinality".to_owned(),
            ordinality_start: 1,
        });
    }
    if positive_offset {
        return Err(sql_err(
            "a positive scalar OFFSET requires an ungrouped aggregate or a constant single row",
        ));
    }
    if let Some(count) = bounded_limit
        && !single_row(&input)
    {
        let source = LogicalPlan::Project {
            input,
            columns: vec![Expr::Alias {
                expr: Box::new(column.clone()),
                name: name.to_owned(),
            }],
        };
        return Ok(LogicalPlan::Project {
            input: Box::new(LogicalPlan::Limit {
                input: Box::new(source),
                count,
            }),
            columns: vec![Expr::Alias {
                expr: Box::new(Expr::Column(name.to_owned())),
                name: name.to_owned(),
            }],
        });
    }
    if !single_row(&input) {
        return Err(sql_err(
            "a scalar subquery must be an ungrouped aggregate, so that it yields one row",
        ));
    }
    let expr = match column {
        Expr::Alias { expr, .. } => (**expr).clone(),
        other => other.clone(),
    };
    Ok(LogicalPlan::Project {
        input,
        columns: vec![Expr::Alias {
            expr: Box::new(expr),
            name: name.to_owned(),
        }],
    })
}

fn build_aggregate(
    plan: LogicalPlan,
    select: &ast::Select,
    approximate: bool,
) -> Result<LogicalPlan> {
    const MAX_GROUPING_SET_BRANCHES: usize = 4096;
    let (group_by_ast, grouping_sets_ast) = match &select.group_by {
        ast::GroupByExpr::Expressions(exprs, modifiers) => {
            if !modifiers.is_empty() {
                return Err(sql_err(
                    "GROUP BY WITH ROLLUP, CUBE, and TOTALS are not supported",
                ));
            }
            let sets = exprs.iter().find_map(|expr| match expr {
                ast::Expr::GroupingSets(sets) => Some(sets.clone()),
                ast::Expr::Cube(keys) => {
                    // CUBE is a finite expansion of every subset. Keep an
                    // explicit bound so malformed/generated SQL cannot
                    // allocate an exponential plan without a clear error.
                    let keys = keys.iter().flatten().cloned().collect::<Vec<_>>();
                    if keys.len() >= 16 {
                        return Some(Vec::new());
                    }
                    let mut sets = Vec::with_capacity(1usize << keys.len());
                    for mask in 0..(1usize << keys.len()) {
                        sets.push(
                            keys.iter()
                                .enumerate()
                                .filter(|(index, _)| mask & (1usize << *index) != 0)
                                .map(|(_, key)| key.clone())
                                .collect(),
                        );
                    }
                    Some(sets)
                }
                ast::Expr::Rollup(keys) => {
                    let keys = keys.iter().flatten().cloned().collect::<Vec<_>>();
                    let mut sets = Vec::with_capacity(keys.len() + 1);
                    for length in (0..=keys.len()).rev() {
                        sets.push(keys.iter().take(length).cloned().collect());
                    }
                    Some(sets)
                }
                _ => None,
            });
            (exprs.clone(), sets)
        }
        ast::GroupByExpr::All(_) => {
            return Err(sql_err("GROUP BY ALL is not supported"));
        }
    };
    let group_by: Vec<Expr> = if grouping_sets_ast.is_some() {
        Vec::new()
    } else {
        group_by_ast
            .iter()
            .map(ast_expr_to_expr)
            .collect::<Result<Vec<_>>>()?
            .into_iter()
            // A literal contributes the same value to every input row.  Keeping
            // it as a hash key needlessly widens every partial group and exchange
            // frame (ClickBench q35 uses `GROUP BY 1, URL`).  The projection
            // above the aggregate still evaluates the literal for each output
            // row, so SQL results and column order are unchanged.
            .filter(|expr| !matches!(expr, Expr::Literal(_)))
            .collect()
    };

    let mut aggregates = Vec::new();
    for item in &select.projection {
        collect_aggregates_from_select_item(item, &mut aggregates)?;
    }
    // HAVING may aggregate what the projection does not (`SELECT k ...
    // GROUP BY k HAVING SUM(x) > 300`); an aggregate it shares with the
    // projection is computed once.
    if let Some(having) = &select.having {
        let mut from_having = Vec::new();
        collect_aggregates_from_ast_expr(having, &mut from_having)?;
        for aggregate in from_having {
            if !aggregates.contains(&aggregate) {
                aggregates.push(aggregate);
            }
        }
    }
    if approximate {
        aggregates = aggregates
            .into_iter()
            .map(AggregateExpr::approximated)
            .collect();
    }

    // Lower the basic Trino `GROUPING SETS ((a), (b), …)` form into
    // equivalent aggregate branches. Each branch keeps the same typed output
    // columns: keys absent from a set are represented by a CASE expression
    // whose column arm preserves the source type and whose value is NULL.
    // Grouping metadata functions remain separate because they need a
    // grouping-id column in addition to these expanded aggregate branches.
    if let Some(sets_ast) = grouping_sets_ast {
        if sets_ast.is_empty() {
            return Err(sql_err(
                "GROUP BY CUBE with 16 or more keys is not supported",
            ));
        }
        if sets_ast.len() > MAX_GROUPING_SET_BRANCHES {
            return Err(sql_err(format!(
                "GROUPING SETS expands to too many branches (maximum {MAX_GROUPING_SET_BRANCHES})"
            )));
        }
        if aggregates.is_empty() {
            return Err(sql_err("GROUPING SETS requires at least one aggregate"));
        }
        let ordinary_group_by = group_by_ast
            .iter()
            .filter(|expression| {
                !matches!(
                    expression,
                    ast::Expr::GroupingSets(_) | ast::Expr::Cube(_) | ast::Expr::Rollup(_)
                )
            })
            .map(ast_expr_to_expr)
            .collect::<Result<Vec<_>>>()?;
        let mut full_group_by = Vec::new();
        // Ordinary keys beside a grouping specification are present in every
        // expanded branch (for example, `GROUP BY region, CUBE (platform)`).
        // Keep them first so the visible output order follows the query.
        for expression in ordinary_group_by {
            if !matches!(expression, Expr::Column(_)) {
                return Err(sql_err(
                    "GROUPING SETS currently requires column grouping keys",
                ));
            }
            if !full_group_by.contains(&expression) {
                full_group_by.push(expression);
            }
        }
        let ordinary_count = full_group_by.len();
        for set in &sets_ast {
            for expression in set {
                let expression = ast_expr_to_expr(expression)?;
                if !matches!(expression, Expr::Column(_)) {
                    return Err(sql_err(
                        "GROUPING SETS currently requires column grouping keys",
                    ));
                }
                if !full_group_by.contains(&expression) {
                    full_group_by.push(expression);
                }
            }
        }
        let mut grouping_columns = Vec::new();
        for item in &select.projection {
            if let ast::SelectItem::UnnamedExpr(expr)
            | ast::SelectItem::ExprWithAlias { expr, .. } = item
            {
                collect_grouping_columns_from_ast_expr(expr, &mut grouping_columns)?;
            }
        }
        if let Some(having) = &select.having {
            collect_grouping_columns_from_ast_expr(having, &mut grouping_columns)?;
        }
        for column in &grouping_columns {
            if !full_group_by.contains(&Expr::Column(column.clone())) {
                return Err(sql_err(format!(
                    "GROUPING column '{column}' is not present in the grouping keys"
                )));
            }
        }
        let mut inputs = Vec::with_capacity(sets_ast.len());
        for set in sets_ast {
            let mut set = set
                .iter()
                .map(ast_expr_to_expr)
                .collect::<Result<Vec<_>>>()?;
            // Ordinary keys are the prefix of full_group_by and are present
            // in every branch; set-specific keys are not.
            for expression in &full_group_by[..ordinary_count] {
                if !set.contains(expression) {
                    set.push(expression.clone());
                }
            }
            let mut group_by: Vec<Expr> = full_group_by
                .iter()
                .map(|expression| {
                    let Expr::Column(name) = expression else {
                        unreachable!("validated grouping key")
                    };
                    let expression = if set.contains(expression) {
                        Expr::Column(name.clone())
                    } else {
                        Expr::Case {
                            operand: None,
                            when_then: vec![(
                                Expr::Literal(ScalarValue::Bool(false)),
                                Expr::Column(name.clone()),
                            )],
                            else_expr: Some(Box::new(Expr::Literal(ScalarValue::Null))),
                        }
                    };
                    Expr::Alias {
                        expr: Box::new(expression),
                        name: name.clone(),
                    }
                })
                .collect();
            for column in &grouping_columns {
                let flag = if set.contains(&Expr::Column(column.clone())) {
                    0
                } else {
                    1
                };
                group_by.push(Expr::Alias {
                    expr: Box::new(Expr::Literal(ScalarValue::Int64(flag))),
                    name: grouping_marker_name(column),
                });
            }
            inputs.push(LogicalPlan::Aggregate {
                input: Box::new(plan.clone()),
                group_by,
                aggregates: aggregates.clone(),
            });
        }
        return Ok(LogicalPlan::Union { inputs, all: true });
    }

    // GROUP BY over plain columns with nothing to aggregate is DISTINCT over
    // those columns.
    if aggregates.is_empty()
        && !group_by.is_empty()
        && group_by.iter().all(|expr| matches!(expr, Expr::Column(_)))
    {
        return Ok(LogicalPlan::Distinct {
            input: Box::new(LogicalPlan::Project {
                input: Box::new(plan),
                columns: group_by,
            }),
        });
    }

    // COUNT(DISTINCT x) [GROUP BY k] is COUNT(x) over the distinct (k, x)
    // rows: the distinct step partitions across the workers by value, so no
    // single task has to hold every distinct value.
    if let [
        AggregateExpr::Count {
            expr: Expr::Column(column),
            distinct: true,
        },
    ] = aggregates.as_slice()
        && group_by.iter().all(|expr| matches!(expr, Expr::Column(_)))
    {
        let counted = Expr::Column(column.clone());
        let mut columns = group_by.clone();
        if !columns.contains(&counted) {
            columns.push(counted.clone());
        }
        return Ok(LogicalPlan::Aggregate {
            input: Box::new(LogicalPlan::Distinct {
                input: Box::new(LogicalPlan::Project {
                    input: Box::new(plan),
                    columns,
                }),
            }),
            group_by,
            aggregates: vec![AggregateExpr::Count {
                expr: counted,
                distinct: false,
            }],
        });
    }

    Ok(LogicalPlan::Aggregate {
        input: Box::new(plan),
        group_by,
        aggregates,
    })
}

/// The HAVING filter, and the single-row relations of its scalar
/// subqueries, which join the aggregate's output once the aggregate is
/// lowered (see `attach_having_scalars`).
fn build_having(
    plan: LogicalPlan,
    select: &ast::Select,
    ctes: &Lowering,
) -> Result<(LogicalPlan, Vec<LogicalPlan>)> {
    match &select.having {
        None => Ok((plan, Vec::new())),
        Some(expr) => {
            let mut scalars = Vec::new();
            let rewritten =
                replace_scalar_subqueries(expr, ctes, &mut |scalar| scalars.push(scalar))?;
            let predicate = ast_expr_to_expr(&rewritten)?;
            Ok((
                LogicalPlan::Filter {
                    input: Box::new(plan),
                    predicate,
                },
                scalars,
            ))
        }
    }
}

type ExpressionBindings = Vec<(Expr, Expr)>;

/// Compute aggregate/group expressions once below aggregation, then bind
/// references above it to the resulting columns. Both execution paths receive
/// column-only aggregate arguments while preserving the original SQL types.
fn lower_aggregate_expressions(
    plan: LogicalPlan,
    force: bool,
) -> Result<(LogicalPlan, ExpressionBindings)> {
    match plan {
        LogicalPlan::Project { input, columns } => {
            let (input, bindings) = lower_aggregate_expressions(*input, force)?;
            let columns = columns
                .into_iter()
                .map(|e| replace_bound_expression(e, &bindings))
                .collect();
            Ok((
                LogicalPlan::Project {
                    input: Box::new(input),
                    columns,
                },
                bindings,
            ))
        }
        LogicalPlan::Filter { input, predicate } => {
            let (input, bindings) = lower_aggregate_expressions(*input, force)?;
            let predicate = replace_bound_expression(predicate, &bindings);
            Ok((
                LogicalPlan::Filter {
                    input: Box::new(input),
                    predicate,
                },
                bindings,
            ))
        }
        LogicalPlan::Distinct { input } => {
            let (input, bindings) = lower_aggregate_expressions(*input, force)?;
            Ok((
                LogicalPlan::Distinct {
                    input: Box::new(input),
                },
                bindings,
            ))
        }
        LogicalPlan::Window {
            input,
            window_exprs,
        } => {
            let (input, bindings) = lower_aggregate_expressions(*input, force)?;
            let window_exprs = window_exprs
                .into_iter()
                .map(|e| replace_bound_expression(e, &bindings))
                .collect();
            Ok((
                LogicalPlan::Window {
                    input: Box::new(input),
                    window_exprs,
                },
                bindings,
            ))
        }
        LogicalPlan::Union { inputs, all } => {
            let mut lowered = Vec::with_capacity(inputs.len());
            let mut bindings = Vec::new();
            for input in inputs {
                let (input, input_bindings) = lower_aggregate_expressions(input, force)?;
                if bindings.is_empty() {
                    bindings = input_bindings;
                }
                lowered.push(input);
            }
            Ok((
                LogicalPlan::Union {
                    inputs: lowered,
                    all,
                },
                bindings,
            ))
        }
        LogicalPlan::Aggregate {
            input,
            mut group_by,
            mut aggregates,
        } => {
            let complex = group_by.iter().any(|e| !matches!(e, Expr::Column(_)))
                || aggregates
                    .iter()
                    .any(|a| !matches!(a.argument(), Expr::Column(_) | Expr::Star));
            if !complex && !force {
                return Ok((
                    LogicalPlan::Aggregate {
                        input,
                        group_by,
                        aggregates,
                    },
                    vec![],
                ));
            }
            let mut projection = Vec::new();
            let mut bindings = Vec::new();
            for (i, expr) in group_by.iter_mut().enumerate() {
                let name = match expr {
                    Expr::Column(name) => name.clone(),
                    Expr::Alias { name, .. } => name.clone(),
                    _ => format!("__kaveon_group_{i}"),
                };
                let original = expr.clone();
                projection.push(Expr::Alias {
                    expr: Box::new(expr.clone()),
                    name: name.clone(),
                });
                let column = Expr::Column(name);
                bindings.push((original.clone(), column.clone()));
                if let Expr::Alias { name, .. } = original
                    && let Some(grouping_column) = name.strip_prefix("__kaveon_grouping_")
                {
                    bindings.push((
                        Expr::Function {
                            name: "GROUPING".to_owned(),
                            args: vec![Expr::Column(grouping_column.to_owned())],
                        },
                        column.clone(),
                    ));
                }
                *expr = column;
            }
            for (i, aggregate) in aggregates.iter_mut().enumerate() {
                if matches!(aggregate.argument(), Expr::Star) {
                    continue;
                }
                let original = aggregate.as_function();
                let name = format!("__kaveon_arg_{i}");
                projection.push(Expr::Alias {
                    expr: Box::new(aggregate.argument().clone()),
                    name: name.clone(),
                });
                *aggregate.argument_mut() = Expr::Column(name.clone());
                bindings.push((original, Expr::Column(aggregate.output_name())));
            }
            Ok((
                LogicalPlan::Aggregate {
                    input: Box::new(LogicalPlan::Project {
                        input,
                        columns: projection,
                    }),
                    group_by,
                    aggregates,
                },
                bindings,
            ))
        }
        other => Ok((other, vec![])),
    }
}

fn replace_bound_expression(expr: Expr, bindings: &ExpressionBindings) -> Expr {
    if let Some((_, replacement)) = bindings.iter().find(|(original, _)| original == &expr) {
        return replacement.clone();
    }
    match expr {
        Expr::Alias { expr, name } => Expr::Alias {
            expr: Box::new(replace_bound_expression(*expr, bindings)),
            name,
        },
        Expr::Cast { expr, data_type } => Expr::Cast {
            expr: Box::new(replace_bound_expression(*expr, bindings)),
            data_type,
        },
        Expr::Extract { expr, field } => Expr::Extract {
            expr: Box::new(replace_bound_expression(*expr, bindings)),
            field,
        },
        Expr::BinaryOp { left, op, right } => Expr::BinaryOp {
            left: Box::new(replace_bound_expression(*left, bindings)),
            op,
            right: Box::new(replace_bound_expression(*right, bindings)),
        },
        Expr::IsNull(expr) => Expr::IsNull(Box::new(replace_bound_expression(*expr, bindings))),
        Expr::IsNotNull(expr) => {
            Expr::IsNotNull(Box::new(replace_bound_expression(*expr, bindings)))
        }
        Expr::Not(expr) => Expr::Not(Box::new(replace_bound_expression(*expr, bindings))),
        Expr::And(a, b) => Expr::And(
            Box::new(replace_bound_expression(*a, bindings)),
            Box::new(replace_bound_expression(*b, bindings)),
        ),
        Expr::Or(a, b) => Expr::Or(
            Box::new(replace_bound_expression(*a, bindings)),
            Box::new(replace_bound_expression(*b, bindings)),
        ),
        Expr::Function { name, args } if name == "GROUPING_ID" => {
            let resolved = args
                .into_iter()
                .map(|e| replace_bound_expression(e, bindings))
                .collect::<Vec<_>>();
            let grouping = resolved.iter().all(|expr| matches!(expr, Expr::Column(_)));
            if grouping {
                let argument_count = resolved.len();
                let mut value = Expr::Literal(ScalarValue::Int64(0));
                for (index, column) in resolved.into_iter().enumerate() {
                    let bit = replace_bound_expression(
                        Expr::Function {
                            name: "GROUPING".to_owned(),
                            args: vec![column],
                        },
                        bindings,
                    );
                    let weight = 1_i64 << (argument_count - index - 1);
                    value = Expr::BinaryOp {
                        left: Box::new(value),
                        op: BinaryOp::Plus,
                        right: Box::new(Expr::BinaryOp {
                            left: Box::new(bit),
                            op: BinaryOp::Multiply,
                            right: Box::new(Expr::Literal(ScalarValue::Int64(weight))),
                        }),
                    };
                }
                replace_bound_expression(value, bindings)
            } else {
                Expr::Function {
                    name,
                    args: resolved,
                }
            }
        }
        Expr::Function { name, args } => Expr::Function {
            name,
            args: args
                .into_iter()
                .map(|e| replace_bound_expression(e, bindings))
                .collect(),
        },
        Expr::WindowFunction {
            name,
            args,
            partition_by,
            order_by,
            frame,
        } => Expr::WindowFunction {
            name,
            args: args
                .into_iter()
                .map(|e| replace_bound_expression(e, bindings))
                .collect(),
            partition_by: partition_by
                .into_iter()
                .map(|e| replace_bound_expression(e, bindings))
                .collect(),
            order_by: order_by
                .into_iter()
                .map(|(e, b, nulls_first)| (replace_bound_expression(e, bindings), b, nulls_first))
                .collect(),
            frame,
        },
        Expr::Case {
            operand,
            when_then,
            else_expr,
        } => Expr::Case {
            operand: operand.map(|e| Box::new(replace_bound_expression(*e, bindings))),
            when_then: when_then
                .into_iter()
                .map(|(a, b)| {
                    (
                        replace_bound_expression(a, bindings),
                        replace_bound_expression(b, bindings),
                    )
                })
                .collect(),
            else_expr: else_expr.map(|e| Box::new(replace_bound_expression(*e, bindings))),
        },
        other => other,
    }
}

fn build_projection(
    plan: LogicalPlan,
    projection: &[ast::SelectItem],
    has_aggregates: bool,
) -> Result<LogicalPlan> {
    let mut columns = Vec::new();
    let mut is_star = false;
    let mut has_qualified_star = false;

    for item in projection {
        match item {
            ast::SelectItem::UnnamedExpr(expr) => {
                columns.push(ast_projection_expr_to_expr(expr)?);
            }
            ast::SelectItem::ExprWithAlias { expr, alias } => {
                let e = ast_projection_expr_to_expr(expr)?;
                columns.push(Expr::Alias {
                    expr: Box::new(e),
                    name: alias.value.clone(),
                });
            }
            ast::SelectItem::Wildcard(_) => {
                is_star = true;
                columns.push(Expr::Star);
            }
            ast::SelectItem::QualifiedWildcard(_, _) => {
                if let ast::SelectItem::QualifiedWildcard(name, _) = item {
                    has_qualified_star = true;
                    columns.push(Expr::Column(format!("{}.*", name)));
                }
            }
        }
    }

    if is_star
        && !has_qualified_star
        && columns.len() == 1
        && !has_aggregates
        && !contains_using_join(&plan)
    {
        return Ok(plan);
    }

    Ok(LogicalPlan::Project {
        input: Box::new(plan),
        columns,
    })
}

fn contains_using_join(plan: &LogicalPlan) -> bool {
    match plan {
        LogicalPlan::Join {
            left,
            right,
            using_columns,
            ..
        } => using_columns.is_some() || contains_using_join(left) || contains_using_join(right),
        LogicalPlan::Filter { input, .. }
        | LogicalPlan::Project { input, .. }
        | LogicalPlan::Sort { input, .. }
        | LogicalPlan::Limit { input, .. }
        | LogicalPlan::Offset { input, .. }
        | LogicalPlan::Distinct { input }
        | LogicalPlan::Window { input, .. }
        | LogicalPlan::Aggregate { input, .. } => contains_using_join(input),
        LogicalPlan::Union { inputs, .. } => inputs.iter().any(contains_using_join),
        LogicalPlan::Intersect { left, right, .. } | LogicalPlan::Except { left, right, .. } => {
            contains_using_join(left) || contains_using_join(right)
        }
        LogicalPlan::SemiJoin { left, right, .. } | LogicalPlan::AntiJoin { left, right, .. } => {
            contains_using_join(left) || contains_using_join(right)
        }
        LogicalPlan::Scan { .. } | LogicalPlan::Unnest { .. } => false,
    }
}

/// Keep date predicates in their compact day-number form for storage pruning,
/// but preserve the SQL type when a DATE literal is projected to a client.
fn ast_projection_expr_to_expr(expr: &ast::Expr) -> Result<Expr> {
    if let ast::Expr::TypedString {
        data_type: ast::DataType::Date,
        value,
    } = expr
    {
        parse_date_days(value)
            .ok_or_else(|| sql_err(format!("invalid DATE literal: '{value}'")))?;
        return Ok(Expr::Cast {
            expr: Box::new(Expr::Literal(ScalarValue::Utf8(value.clone()))),
            data_type: CastTarget::Date32,
        });
    }
    ast_expr_to_expr(expr)
}

/// The sorted plan and, when a key had to be carried through the
/// projection, the visible columns to re-project at the top.
fn build_order_by(
    plan: LogicalPlan,
    order_by: &[ast::OrderByExpr],
    bindings: &ExpressionBindings,
) -> Result<(LogicalPlan, Option<Vec<Expr>>)> {
    if order_by.is_empty() {
        return Ok((plan, None));
    }
    let mut items = Vec::new();
    for ob in order_by {
        let expr = replace_bound_expression(ast_expr_to_expr(&ob.expr)?, bindings);
        let asc = ob.asc.unwrap_or(true);
        // Trino defaults to NULLS LAST for ascending and NULLS FIRST for
        // descending order. Preserve that default while honoring an explicit
        // NULLS FIRST/LAST clause.
        let nulls_first = ob.nulls_first.unwrap_or(!asc);
        items.push((expr, asc, nulls_first));
    }
    let (plan, visible) = bind_order_keys_to_projection(plan, &mut items)?;
    Ok((
        LogicalPlan::Sort {
            input: Box::new(plan),
            order_by: items,
        },
        visible,
    ))
}

/// An ORDER BY expression that repeats a select item — `ORDER BY COUNT(*)`
/// beside `SELECT k, COUNT(*)` — orders by that item's output column. An
/// item without a name receives one (`expr_<position>`) so the key can
/// name it.
fn bind_order_keys_to_projection(
    plan: LogicalPlan,
    keys: &mut [OrderKey],
) -> Result<(LogicalPlan, Option<Vec<Expr>>)> {
    let LogicalPlan::Project { input, mut columns } = plan else {
        return Ok((plan, None));
    };
    if columns.iter().any(|column| matches!(column, Expr::Star)) {
        return Ok((LogicalPlan::Project { input, columns }, None));
    }
    let output_names = |columns: &[Expr]| {
        columns
            .iter()
            .filter_map(|column| match column {
                Expr::Alias { name, .. } => Some(name.clone()),
                Expr::Column(name) => Some(name.rsplit('.').next().unwrap_or(name).to_owned()),
                _ => None,
            })
            .collect::<Vec<_>>()
    };
    let visible = columns.clone();
    let mut hidden = Vec::new();
    for (key, _, _) in keys.iter_mut() {
        // Trino treats a positive integer in ORDER BY as a one-based select
        // item ordinal. Resolve it to the projected output name before the
        // sort is planned; leaving it as a literal would silently produce an
        // unsorted result.
        if let Expr::Literal(ScalarValue::Int64(ordinal)) = key {
            let position = ordinal
                .checked_sub(1)
                .and_then(|value| usize::try_from(value).ok())
                .ok_or_else(|| sql_err("ORDER BY position must be greater than zero"))?;
            if position >= columns.len() {
                return Err(sql_err(format!(
                    "ORDER BY position {ordinal} is not in the select list"
                )));
            }
            let selected = columns[position].clone();
            let name = match selected {
                Expr::Alias { name, .. } => name,
                Expr::Column(name) => name,
                other => {
                    let name = format!("expr_{position}");
                    columns[position] = Expr::Alias {
                        expr: Box::new(other),
                        name: name.clone(),
                    };
                    name
                }
            };
            *key = Expr::Column(name);
            continue;
        }
        if let Expr::Column(name) = key {
            // `ORDER BY u.country` when the query selects `u.country AS
            // user_country` orders by that output column.
            let aliased = columns.iter().find_map(|column| match column {
                Expr::Alias { expr, name: alias }
                    if matches!(expr.as_ref(), Expr::Column(selected) if selected == name) =>
                {
                    Some(alias.clone())
                }
                _ => None,
            });
            if let Some(alias) = aliased {
                *key = Expr::Column(alias);
                continue;
            }
            // A column the query does not select rides along through the
            // sort; the caller re-projects the visible columns above. A
            // qualified key is selected only by the same qualified column
            // (`t.country` does not stand in for `u.country`); a bare key
            // by any column of that name.
            let selected_exactly = columns
                .iter()
                .any(|column| matches!(column, Expr::Column(selected) if selected == name));
            let bare = name.rsplit('.').next().unwrap_or(name).to_owned();
            let selected_bare = !name.contains('.') && output_names(&columns).contains(&bare);
            if !selected_exactly && !selected_bare && !hidden.contains(name) {
                hidden.push(name.clone());
                columns.push(Expr::Column(name.clone()));
            }
            continue;
        }
        if matches!(key, Expr::Literal(_)) {
            continue;
        }
        let position = columns.iter().position(|column| match column {
            Expr::Alias { expr, .. } => expr.as_ref() == key,
            other => other == key,
        });
        let Some(position) = position else {
            continue;
        };
        let name = match &columns[position] {
            Expr::Alias { name, .. } => name.clone(),
            other => {
                let name = format!("expr_{position}");
                columns[position] = Expr::Alias {
                    expr: Box::new(other.clone()),
                    name: name.clone(),
                };
                name
            }
        };
        *key = Expr::Column(name);
    }
    let visible = (!hidden.is_empty()).then(|| {
        visible
            .iter()
            .enumerate()
            .map(|(position, column)| match column {
                Expr::Alias { name, .. } => Expr::Column(name.clone()),
                Expr::Column(name) => Expr::Column(name.clone()),
                other => {
                    // An unnamed expression is addressed by the name it
                    // received in the extended projection.
                    let name = format!("expr_{position}");
                    columns[position] = Expr::Alias {
                        expr: Box::new(other.clone()),
                        name: name.clone(),
                    };
                    Expr::Column(name)
                }
            })
            .collect::<Vec<_>>()
    });
    Ok((LogicalPlan::Project { input, columns }, visible))
}

fn build_limit_offset(
    plan: LogicalPlan,
    limit: &Option<ast::Expr>,
    offset: &Option<ast::Offset>,
    fetch: Option<&ast::Fetch>,
) -> Result<LogicalPlan> {
    let offset_count = match offset {
        Some(ast::Offset { value, .. }) => ast_expr_to_usize(value)?,
        None => 0,
    };
    let fetch_percent = match fetch {
        Some(fetch) if fetch.percent => {
            let quantity = fetch
                .quantity
                .as_ref()
                .ok_or_else(|| sql_err("FETCH PERCENT requires a percentage"))?;
            Some(ast_expr_to_fetch_percent(quantity)?)
        }
        _ => None,
    };
    let fetch_limit = match fetch {
        None => None,
        Some(fetch) if fetch.percent => None,
        Some(fetch) => Some(
            fetch
                .quantity
                .as_ref()
                .ok_or_else(|| sql_err("FETCH requires a row count"))?,
        ),
    };
    let with_ties = fetch.is_some_and(|fetch| fetch.with_ties);
    if let Some(percent) = fetch_percent {
        if limit.is_some() {
            return Err(sql_err("LIMIT and FETCH cannot both be specified"));
        }
        if with_ties && !matches!(plan, LogicalPlan::Sort { .. }) {
            return Err(sql_err(
                "FETCH PERCENT WITH TIES requires an ORDER BY clause",
            ));
        }
        let (order_by, input) = match plan {
            LogicalPlan::Sort { input, order_by } => (order_by, *input),
            other => (Vec::new(), other),
        };
        let total = Expr::WindowFunction {
            name: "COUNT".to_owned(),
            args: vec![Expr::Star],
            partition_by: Vec::new(),
            order_by: Vec::new(),
            frame: Some(WindowFrame {
                units: WindowFrameUnits::Rows,
                start: WindowFrameBound::UnboundedPreceding,
                end: WindowFrameBound::UnboundedFollowing,
            }),
        };
        let rank = Expr::WindowFunction {
            name: if with_ties {
                "RANK".to_owned()
            } else {
                "ROW_NUMBER".to_owned()
            },
            args: Vec::new(),
            partition_by: Vec::new(),
            order_by: order_by.clone(),
            frame: None,
        };
        let total_name = format!("__kaveon_window_{total:?}");
        let rank_name = format!("__kaveon_window_{rank:?}");
        let threshold = Expr::Function {
            name: "CEIL".to_owned(),
            args: vec![Expr::BinaryOp {
                left: Box::new(Expr::BinaryOp {
                    left: Box::new(Expr::Column(total_name)),
                    op: BinaryOp::Multiply,
                    right: Box::new(percent),
                }),
                op: BinaryOp::Divide,
                right: Box::new(Expr::Literal(ScalarValue::Float64(100.0))),
            }],
        };
        let bounded = LogicalPlan::Filter {
            input: Box::new(LogicalPlan::Window {
                input: Box::new(input),
                window_exprs: vec![total, rank],
            }),
            predicate: Expr::BinaryOp {
                left: Box::new(Expr::Column(rank_name)),
                op: BinaryOp::Le,
                right: Box::new(threshold),
            },
        };
        return if offset_count == 0 {
            Ok(if order_by.is_empty() {
                bounded
            } else {
                LogicalPlan::Sort {
                    input: Box::new(bounded),
                    order_by,
                }
            })
        } else {
            Ok(LogicalPlan::Offset {
                input: Box::new(if order_by.is_empty() {
                    bounded
                } else {
                    LogicalPlan::Sort {
                        input: Box::new(bounded),
                        order_by,
                    }
                }),
                count: offset_count,
            })
        };
    }
    match (limit, fetch_limit) {
        (Some(_), Some(_)) => Err(sql_err("LIMIT and FETCH cannot both be specified")),
        (None, None) if offset_count == 0 => Ok(plan),
        (None, None) => Ok(LogicalPlan::Offset {
            input: Box::new(plan),
            count: offset_count,
        }),
        (Some(expr), None) => {
            if matches!(expr, ast::Expr::Identifier(identifier) if identifier.value.eq_ignore_ascii_case("ALL"))
            {
                return if offset_count == 0 {
                    Ok(plan)
                } else {
                    Ok(LogicalPlan::Offset {
                        input: Box::new(plan),
                        count: offset_count,
                    })
                };
            }
            let count = ast_expr_to_usize(expr)?;
            let plan = if offset_count == 0 {
                plan
            } else {
                LogicalPlan::Offset {
                    input: Box::new(plan),
                    count: offset_count,
                }
            };
            Ok(LogicalPlan::Limit {
                input: Box::new(plan),
                count,
            })
        }
        (None, Some(expr)) => {
            let count = ast_expr_to_usize(expr)?;
            if !with_ties {
                let plan = if offset_count == 0 {
                    plan
                } else {
                    LogicalPlan::Offset {
                        input: Box::new(plan),
                        count: offset_count,
                    }
                };
                return Ok(LogicalPlan::Limit {
                    input: Box::new(plan),
                    count,
                });
            }
            let LogicalPlan::Sort { input, order_by } = plan else {
                return Err(sql_err("FETCH WITH TIES requires an ORDER BY clause"));
            };
            let rank = Expr::WindowFunction {
                name: "RANK".to_owned(),
                args: Vec::new(),
                partition_by: Vec::new(),
                order_by: order_by.clone(),
                frame: None,
            };
            let rank_name = format!("__kaveon_window_{rank:?}");
            let ranked = LogicalPlan::Window {
                input,
                window_exprs: vec![rank],
            };
            let bounded = LogicalPlan::Filter {
                input: Box::new(ranked),
                predicate: Expr::BinaryOp {
                    left: Box::new(Expr::Column(rank_name)),
                    op: kaveon_core::BinaryOp::Le,
                    right: Box::new(Expr::Literal(ScalarValue::Int64(
                        i64::try_from(offset_count.saturating_add(count))
                            .map_err(|_| sql_err("FETCH row count is too large"))?,
                    ))),
                },
            };
            let bounded = if offset_count == 0 {
                bounded
            } else {
                LogicalPlan::Offset {
                    input: Box::new(bounded),
                    count: offset_count,
                }
            };
            Ok(LogicalPlan::Sort {
                input: Box::new(bounded),
                order_by,
            })
        }
    }
}

fn ast_expr_to_fetch_percent(expr: &ast::Expr) -> Result<Expr> {
    let value = ast_expr_to_expr(expr)?;
    let numeric = match value {
        Expr::Literal(ScalarValue::Int64(value)) => value as f64,
        Expr::Literal(ScalarValue::Float64(value)) => value,
        Expr::Literal(ScalarValue::Decimal128 { value, scale, .. }) => {
            value as f64 / 10_f64.powi(scale as i32)
        }
        _ => return Err(sql_err("FETCH PERCENT must be a literal number")),
    };
    if !numeric.is_finite() || !(0.0..=100.0).contains(&numeric) {
        return Err(sql_err("FETCH PERCENT must be between 0 and 100"));
    }
    Ok(Expr::Literal(ScalarValue::Float64(numeric)))
}

fn ast_expr_to_usize(expr: &ast::Expr) -> Result<usize> {
    match expr {
        ast::Expr::Value(v) => match v {
            ast::Value::Number(n, _) => n
                .parse::<usize>()
                .map_err(|_| sql_err(format!("invalid limit: {n}"))),
            _ => Err(sql_err("LIMIT must be a number")),
        },
        _ => Err(sql_err("LIMIT must be a literal number")),
    }
}

/// Days since 1970-01-01 for a `YYYY-MM-DD` string (proleptic Gregorian).
fn parse_date_days(value: &str) -> Option<i64> {
    kaveon_core::predicate::date_literal_days(value)
}

fn ast_expr_to_expr(expr: &ast::Expr) -> Result<Expr> {
    match expr {
        ast::Expr::Identifier(ident) => Ok(Expr::Column(ident.value.clone())),
        ast::Expr::CompoundIdentifier(parts) => {
            let name = parts
                .iter()
                .map(|p| p.value.as_str())
                .collect::<Vec<_>>()
                .join(".");
            Ok(Expr::Column(name))
        }
        ast::Expr::Value(v) => ast_value_to_expr(v),
        ast::Expr::BinaryOp { left, op, right } => {
            if let Some(expr) = interval_arithmetic(left, op, right)? {
                return Ok(expr);
            }
            let l = ast_expr_to_expr(left)?;
            let r = ast_expr_to_expr(right)?;
            match ast_binop_to_binop(op) {
                Some(binop) => Ok(Expr::BinaryOp {
                    left: Box::new(l),
                    op: binop,
                    right: Box::new(r),
                }),
                None => match op {
                    ast::BinaryOperator::And => Ok(Expr::And(Box::new(l), Box::new(r))),
                    ast::BinaryOperator::Or => Ok(Expr::Or(Box::new(l), Box::new(r))),
                    _ => Err(sql_err(format!("unsupported operator: {op}"))),
                },
            }
        }
        ast::Expr::UnaryOp {
            op: ast::UnaryOperator::Not,
            expr,
        } => {
            let inner = ast_expr_to_expr(expr)?;
            Ok(Expr::Not(Box::new(inner)))
        }
        ast::Expr::UnaryOp {
            op: ast::UnaryOperator::Minus,
            expr,
        } => {
            let inner = ast_expr_to_expr(expr)?;
            Ok(Expr::BinaryOp {
                left: Box::new(Expr::Literal(ScalarValue::Int64(0))),
                op: BinaryOp::Minus,
                right: Box::new(inner),
            })
        }
        ast::Expr::IsNull(expr) => Ok(Expr::IsNull(Box::new(ast_expr_to_expr(expr)?))),
        ast::Expr::IsNotNull(expr) => Ok(Expr::IsNotNull(Box::new(ast_expr_to_expr(expr)?))),
        ast::Expr::IsDistinctFrom(left, right) => {
            let left = ast_expr_to_expr(left)?;
            let right = ast_expr_to_expr(right)?;
            Ok(null_safe_comparison(left, right, false))
        }
        ast::Expr::IsNotDistinctFrom(left, right) => {
            let left = ast_expr_to_expr(left)?;
            let right = ast_expr_to_expr(right)?;
            Ok(null_safe_comparison(left, right, true))
        }
        ast::Expr::Nested(inner) => ast_expr_to_expr(inner),
        ast::Expr::Function(func) => ast_function_to_expr(func),
        ast::Expr::CompositeAccess { expr, key } => Ok(Expr::Function {
            name: "ROW_FIELD".into(),
            args: vec![
                ast_expr_to_expr(expr)?,
                Expr::Literal(ScalarValue::Utf8(key.value.clone())),
            ],
        }),
        ast::Expr::MapAccess { column, keys } => {
            if keys.is_empty() {
                return Err(sql_err("map access requires a key"));
            }
            let mut value = ast_expr_to_expr(column)?;
            for key in keys {
                value = Expr::Function {
                    name: "MAP_ELEMENT_AT".into(),
                    args: vec![value, ast_expr_to_expr(&key.key)?],
                };
            }
            Ok(value)
        }
        ast::Expr::Subscript { expr, subscript } => match subscript.as_ref() {
            ast::Subscript::Index { index } => Ok(Expr::Function {
                name: "ELEMENT_AT".into(),
                args: vec![ast_expr_to_expr(expr)?, ast_expr_to_expr(index)?],
            }),
            ast::Subscript::Slice {
                lower_bound,
                upper_bound,
                stride,
            } => Ok(Expr::Function {
                name: "ARRAY_SLICE".into(),
                args: vec![
                    ast_expr_to_expr(expr)?,
                    lower_bound
                        .as_ref()
                        .map(ast_expr_to_expr)
                        .transpose()?
                        .unwrap_or(Expr::Literal(ScalarValue::Null)),
                    upper_bound
                        .as_ref()
                        .map(ast_expr_to_expr)
                        .transpose()?
                        .unwrap_or(Expr::Literal(ScalarValue::Null)),
                    stride
                        .as_ref()
                        .map(ast_expr_to_expr)
                        .transpose()?
                        .unwrap_or(Expr::Literal(ScalarValue::Int64(1))),
                ],
            }),
        },
        // `ARRAY[…]` is carried as a call named ARRAY. The executor evaluates
        // homogeneous literals as typed Arrow lists; aggregate lowering also
        // consumes constant percentile lists here.
        ast::Expr::Array(array) => Ok(Expr::Function {
            name: "ARRAY".into(),
            args: array
                .elem
                .iter()
                .map(ast_expr_to_expr)
                .collect::<Result<_>>()?,
        }),
        ast::Expr::Wildcard(_) => Ok(Expr::Star),

        // ── CASE ────────────────────────────────────────────────────────
        ast::Expr::Case {
            operand,
            conditions,
            results,
            else_result,
        } => {
            let operand = operand
                .as_ref()
                .map(|e| ast_expr_to_expr(e).map(Box::new))
                .transpose()?;
            let when_then: Vec<(Expr, Expr)> = conditions
                .iter()
                .zip(results.iter())
                .map(|(c, r)| Ok((ast_expr_to_expr(c)?, ast_expr_to_expr(r)?)))
                .collect::<Result<_>>()?;
            let else_expr = else_result
                .as_ref()
                .map(|e| ast_expr_to_expr(e).map(Box::new))
                .transpose()?;
            Ok(Expr::Case {
                operand,
                when_then,
                else_expr,
            })
        }

        // ── LIKE / ILIKE ────────────────────────────────────────────────
        ast::Expr::Like {
            expr,
            pattern,
            negated,
            ..
        } => Ok(Expr::Like {
            expr: Box::new(ast_expr_to_expr(expr)?),
            pattern: Box::new(ast_expr_to_expr(pattern)?),
            negated: *negated,
            case_insensitive: false,
        }),
        ast::Expr::ILike {
            expr,
            pattern,
            negated,
            ..
        } => Ok(Expr::Like {
            expr: Box::new(ast_expr_to_expr(expr)?),
            pattern: Box::new(ast_expr_to_expr(pattern)?),
            negated: *negated,
            case_insensitive: true,
        }),

        // ── BETWEEN ─────────────────────────────────────────────────────
        ast::Expr::Between {
            expr,
            low,
            high,
            negated,
        } => Ok(Expr::Between {
            expr: Box::new(ast_expr_to_expr(expr)?),
            low: Box::new(ast_expr_to_expr(low)?),
            high: Box::new(ast_expr_to_expr(high)?),
            negated: *negated,
        }),

        // ── IN (list) ───────────────────────────────────────────────────
        ast::Expr::InList {
            expr,
            list,
            negated,
        } => {
            let e = ast_expr_to_expr(expr)?;
            let items: Vec<Expr> = list.iter().map(ast_expr_to_expr).collect::<Result<_>>()?;
            Ok(Expr::InList {
                expr: Box::new(e),
                list: items,
                negated: *negated,
            })
        }

        // Trino's `value [NOT] IN UNNEST(array)` has the same three-valued
        // membership semantics as CONTAINS for the typed list values the
        // executor supports. Keep the lowering explicit so this does not
        // imply row-producing UNNEST support in FROM.
        ast::Expr::InUnnest {
            expr,
            array_expr,
            negated,
        } => {
            let contains = Expr::Function {
                name: "CONTAINS".into(),
                args: vec![ast_expr_to_expr(array_expr)?, ast_expr_to_expr(expr)?],
            };
            if *negated {
                Ok(Expr::Not(Box::new(contains)))
            } else {
                Ok(contains)
            }
        }

        // Equality forms are equivalent to membership and use the same
        // vectorized nullable path. Other operators lower to an explicit
        // reduction so empty-array and NULL rules stay visible in execution.
        ast::Expr::AnyOp {
            left,
            compare_op: ast::BinaryOperator::Eq,
            right,
            ..
        } => Ok(Expr::Function {
            name: "CONTAINS".into(),
            args: vec![ast_expr_to_expr(right)?, ast_expr_to_expr(left)?],
        }),
        ast::Expr::AnyOp {
            left,
            compare_op,
            right,
            ..
        } => {
            let suffix = match compare_op {
                ast::BinaryOperator::NotEq => "NE",
                ast::BinaryOperator::Lt => "LT",
                ast::BinaryOperator::LtEq => "LE",
                ast::BinaryOperator::Gt => "GT",
                ast::BinaryOperator::GtEq => "GE",
                other => {
                    return Err(sql_err(format!(
                        "unsupported quantified comparison: {other}"
                    )));
                }
            };
            Ok(Expr::Function {
                name: format!("ARRAY_ANY_{suffix}"),
                args: vec![ast_expr_to_expr(right)?, ast_expr_to_expr(left)?],
            })
        }
        ast::Expr::AllOp {
            left,
            compare_op: ast::BinaryOperator::NotEq,
            right,
        } => Ok(Expr::Not(Box::new(Expr::Function {
            name: "CONTAINS".into(),
            args: vec![ast_expr_to_expr(right)?, ast_expr_to_expr(left)?],
        }))),
        ast::Expr::AllOp {
            left,
            compare_op,
            right,
        } => {
            let suffix = match compare_op {
                ast::BinaryOperator::Eq => "EQ",
                ast::BinaryOperator::Lt => "LT",
                ast::BinaryOperator::LtEq => "LE",
                ast::BinaryOperator::Gt => "GT",
                ast::BinaryOperator::GtEq => "GE",
                other => {
                    return Err(sql_err(format!(
                        "unsupported quantified comparison: {other}"
                    )));
                }
            };
            Ok(Expr::Function {
                name: format!("ARRAY_ALL_{suffix}"),
                args: vec![ast_expr_to_expr(right)?, ast_expr_to_expr(left)?],
            })
        }

        ast::Expr::TypedString {
            data_type: ast::DataType::Decimal(_) | ast::DataType::Numeric(_),
            value,
        } => Ok(Expr::Literal(parse_decimal_literal(value)?)),

        ast::Expr::TypedString {
            data_type: ast::DataType::Time(_, _),
            value,
        } => Ok(Expr::Function {
            name: "TIME_LITERAL".into(),
            args: vec![Expr::Literal(ScalarValue::Utf8(value.clone()))],
        }),

        // A DATE literal is its day number: the Engine's date columns are
        // days since the epoch, so `EventDate >= DATE '2013-07-01'` compares
        // integers with integers.
        ast::Expr::TypedString {
            data_type: ast::DataType::Date,
            value,
        } => {
            let days = parse_date_days(value)
                .ok_or_else(|| sql_err(format!("invalid DATE literal: '{value}'")))?;
            Ok(Expr::Literal(ScalarValue::Int64(days)))
        }
        ast::Expr::TypedString {
            data_type: ast::DataType::Timestamp(_, timezone),
            value,
        } => Ok(Expr::Cast {
            expr: Box::new(Expr::Literal(ScalarValue::Utf8(value.clone()))),
            data_type: CastTarget::TimestampMicros {
                with_timezone: matches!(
                    timezone,
                    ast::TimezoneInfo::WithTimeZone | ast::TimezoneInfo::Tz
                ),
            },
        }),

        // Trino's typed JSON literal (`JSON '{"key": 1}'`) is equivalent to
        // parsing the UTF-8 payload, while Arrow carries JSON values as text.
        // Keep it on the same validated JSON_PARSE path used by CAST(... AS
        // JSON), so malformed literals fail with the normal JSON error.
        ast::Expr::TypedString {
            data_type: ast::DataType::JSON | ast::DataType::JSONB,
            value,
        } => Ok(Expr::Function {
            name: "JSON_PARSE".into(),
            args: vec![Expr::Literal(ScalarValue::Utf8(value.clone()))],
        }),

        // ── CAST ────────────────────────────────────────────────────────
        ast::Expr::Cast {
            kind,
            expr,
            data_type,
            ..
        } => {
            let inner = ast_expr_to_expr(expr)?;
            // JSON is represented as validated UTF-8 text in Arrow.  Lower
            // the cast to a dedicated function so invalid documents fail at
            // execution (and TRY_CAST can preserve SQL's NULL-on-error rule)
            // instead of being accepted as arbitrary strings.
            if matches!(data_type, ast::DataType::JSON | ast::DataType::JSONB) {
                return Ok(Expr::Function {
                    name: if matches!(kind, ast::CastKind::TryCast | ast::CastKind::SafeCast) {
                        "TRY_JSON_PARSE".into()
                    } else {
                        "JSON_PARSE".into()
                    },
                    args: vec![inner],
                });
            }
            let target = ast_data_type_to_cast_target(data_type)?;
            if matches!(kind, ast::CastKind::TryCast | ast::CastKind::SafeCast) {
                Ok(Expr::Function {
                    name: format!("TRY_CAST_{}", cast_target_name(target)),
                    args: vec![inner],
                })
            } else {
                Ok(Expr::Cast {
                    expr: Box::new(inner),
                    data_type: target,
                })
            }
        }

        // ── EXTRACT ──────────────────────────────────────────────────────
        ast::Expr::Extract { field, expr, .. } => {
            let date_field = ast_date_field_to_date_field(field)?;
            Ok(Expr::Extract {
                field: date_field,
                expr: Box::new(ast_expr_to_expr(expr)?),
            })
        }

        // ── Subqueries ──────────────────────────────────────────────────
        // IN/EXISTS subqueries are handled at plan level by build_where()
        ast::Expr::InSubquery { .. } => Err(sql_err(
            "IN subquery in this position is not supported; use it in WHERE",
        )),
        ast::Expr::Exists { .. } => Err(sql_err(
            "EXISTS in this position is not supported; use it in WHERE",
        )),
        ast::Expr::Subquery(_) => Err(sql_err("scalar subqueries are not yet supported")),

        ast::Expr::Interval(_) => Err(sql_err(
            "an INTERVAL is only supported added to or subtracted from a date",
        )),

        _ => Err(sql_err(format!("unsupported expression: {expr}"))),
    }
}

/// Lower SQL's null-safe comparisons into the existing CASE/null predicate
/// expression model. `IS DISTINCT FROM` is always boolean: two NULLs compare
/// equal, and a NULL compares different from every non-NULL value.
fn null_safe_comparison(left: Expr, right: Expr, not_distinct: bool) -> Expr {
    let left_is_null = Expr::IsNull(Box::new(left.clone()));
    let right_is_null = Expr::IsNull(Box::new(right.clone()));
    let right_is_not_null = Expr::IsNotNull(Box::new(right.clone()));
    let equal = Expr::BinaryOp {
        left: Box::new(left.clone()),
        op: BinaryOp::Eq,
        right: Box::new(right.clone()),
    };
    let different = Expr::BinaryOp {
        left: Box::new(left),
        op: BinaryOp::Ne,
        right: Box::new(right),
    };
    if not_distinct {
        Expr::Case {
            operand: None,
            when_then: vec![
                (left_is_null, right_is_null.clone()),
                (right_is_null, Expr::Literal(ScalarValue::Bool(false))),
            ],
            else_expr: Some(Box::new(equal)),
        }
    } else {
        Expr::Case {
            operand: None,
            when_then: vec![
                (left_is_null, right_is_not_null),
                (right_is_null, Expr::Literal(ScalarValue::Bool(true))),
            ],
            else_expr: Some(Box::new(different)),
        }
    }
}

fn cast_target_name(target: CastTarget) -> String {
    match target {
        CastTarget::Boolean => "BOOLEAN".into(),
        CastTarget::Int32 => "INT32".into(),
        CastTarget::Int64 => "INT64".into(),
        CastTarget::Float64 => "FLOAT64".into(),
        CastTarget::Utf8 => "UTF8".into(),
        CastTarget::Date32 => "DATE32".into(),
        CastTarget::TimestampMicros {
            with_timezone: false,
        } => "TIMESTAMP".into(),
        CastTarget::TimestampMicros {
            with_timezone: true,
        } => "TIMESTAMP_TZ".into(),
        CastTarget::Decimal128 { precision, scale } => format!("DECIMAL_{precision}_{scale}"),
    }
}

/// `date + INTERVAL 'n' DAY` and `date - INTERVAL 'n' DAY` lower to
/// arithmetic on the day number, so any date expression takes a day
/// interval. Month and year intervals shift the calendar, which the Engine
/// evaluates at lowering time against a DATE literal (`DATE '1993-07-01' +
/// INTERVAL '3' MONTH` is the day number of 1993-10-01); a day past the end
/// of the target month clamps to that month's last day, as Trino does.
/// Returns None when neither operand is an interval.
fn interval_arithmetic(
    left: &ast::Expr,
    op: &ast::BinaryOperator,
    right: &ast::Expr,
) -> Result<Option<Expr>> {
    let (date, interval, negate) = match (left, op, right) {
        (_, ast::BinaryOperator::Plus, ast::Expr::Interval(interval)) => (left, interval, false),
        (_, ast::BinaryOperator::Minus, ast::Expr::Interval(interval)) => (left, interval, true),
        (ast::Expr::Interval(interval), ast::BinaryOperator::Plus, _) => (right, interval, false),
        (ast::Expr::Interval(_), _, _) | (_, _, ast::Expr::Interval(_)) => {
            return Err(sql_err(format!(
                "unsupported INTERVAL arithmetic: {left} {op} {right}"
            )));
        }
        _ => return Ok(None),
    };
    let text = match interval.value.as_ref() {
        ast::Expr::Value(ast::Value::SingleQuotedString(text)) => text.trim(),
        ast::Expr::Value(ast::Value::Number(text, _)) => text.as_str(),
        other => return Err(sql_err(format!("unsupported INTERVAL value: {other}"))),
    };
    let count: i64 = text
        .parse()
        .map_err(|_| sql_err(format!("invalid INTERVAL value: '{text}'")))?;
    let count = if negate {
        count
            .checked_neg()
            .ok_or_else(|| sql_err("negative INTERVAL value overflows INT64"))?
    } else {
        count
    };
    if interval.last_field.is_some() {
        return Err(sql_err(format!(
            "unsupported INTERVAL field range: {interval}"
        )));
    }
    let date = ast_expr_to_expr(date)?;
    match &interval.leading_field {
        Some(ast::DateTimeField::Day) | Some(ast::DateTimeField::Week(_)) | None => {
            let days = if matches!(interval.leading_field, Some(ast::DateTimeField::Week(_))) {
                count
                    .checked_mul(7)
                    .ok_or_else(|| sql_err("INTERVAL week offset overflows INT64"))?
            } else {
                count
            };
            let arithmetic = Expr::BinaryOp {
                left: Box::new(date.clone()),
                op: BinaryOp::Plus,
                right: Box::new(Expr::Literal(ScalarValue::Int64(days))),
            };
            // DATE literals are represented as day-number integers and keep
            // the compact arithmetic form. Typed DATE/TIMESTAMP columns need
            // a runtime unit-aware operation instead.
            if matches!(&date, Expr::Literal(ScalarValue::Int64(_))) {
                Ok(Some(arithmetic))
            } else {
                Ok(Some(Expr::Function {
                    name: "DATE_ADD_DAYS".into(),
                    args: vec![date, Expr::Literal(ScalarValue::Int64(days))],
                }))
            }
        }
        Some(ast::DateTimeField::Hour)
        | Some(ast::DateTimeField::Minute)
        | Some(ast::DateTimeField::Second) => {
            let multiplier = match interval.leading_field {
                Some(ast::DateTimeField::Hour) => 3_600,
                Some(ast::DateTimeField::Minute) => 60,
                _ => 1,
            };
            let seconds = count
                .checked_mul(multiplier)
                .ok_or_else(|| sql_err("INTERVAL second offset overflows INT64"))?;
            if matches!(&date, Expr::Literal(ScalarValue::Int64(_))) {
                return Err(sql_err(
                    "sub-day INTERVAL arithmetic requires a timestamp operand",
                ));
            }
            Ok(Some(Expr::Function {
                name: "DATE_ADD_SECONDS".into(),
                args: vec![date, Expr::Literal(ScalarValue::Int64(seconds))],
            }))
        }
        Some(ast::DateTimeField::Month)
        | Some(ast::DateTimeField::Quarter)
        | Some(ast::DateTimeField::Year) => {
            let multiplier = match interval.leading_field {
                Some(ast::DateTimeField::Year) => 12,
                Some(ast::DateTimeField::Quarter) => 3,
                _ => 1,
            };
            let months = count
                .checked_mul(multiplier)
                .ok_or_else(|| sql_err("INTERVAL calendar offset overflows INT64"))?;
            if let Expr::Literal(ScalarValue::Int64(days)) = date {
                Ok(Some(Expr::Literal(ScalarValue::Int64(add_months(
                    days, months,
                )))))
            } else {
                Ok(Some(Expr::Function {
                    name: "DATE_ADD_MONTHS".into(),
                    args: vec![date, Expr::Literal(ScalarValue::Int64(months))],
                }))
            }
        }
        Some(other) => Err(sql_err(format!("unsupported INTERVAL field: {other}"))),
    }
}

/// The day number `months` calendar months after the day number `days`;
/// a day past the end of the target month clamps to that month's last day.
fn add_months(days: i64, months: i64) -> i64 {
    let (year, month, day) = civil_from_days(days);
    let total = year * 12 + (month - 1) + months;
    let year = total.div_euclid(12);
    let month = total.rem_euclid(12) + 1;
    let day = day.min(days_in_month(year, month));
    parse_date_days(&format!("{year:04}-{month:02}-{day:02}"))
        .expect("a calendar day within its month is a valid date")
}

/// (year, month, day) for a day number since 1970-01-01, proleptic
/// Gregorian; the inverse of `date_literal_days`.
fn civil_from_days(days: i64) -> (i64, i64, i64) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let year = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    (if month <= 2 { year + 1 } else { year }, month, day)
}

fn days_in_month(year: i64, month: i64) -> i64 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        _ => {
            if (year % 4 == 0 && year % 100 != 0) || year % 400 == 0 {
                29
            } else {
                28
            }
        }
    }
}

fn ast_data_type_to_cast_target(dt: &ast::DataType) -> Result<CastTarget> {
    match dt {
        ast::DataType::Boolean => Ok(CastTarget::Boolean),
        ast::DataType::Int(_) | ast::DataType::Integer(_) | ast::DataType::Int4(_) => {
            Ok(CastTarget::Int32)
        }
        ast::DataType::BigInt(_) | ast::DataType::Int8(_) => Ok(CastTarget::Int64),
        ast::DataType::Float(_)
        | ast::DataType::Double
        | ast::DataType::DoublePrecision
        | ast::DataType::Real
        | ast::DataType::Float8 => Ok(CastTarget::Float64),
        ast::DataType::Varchar(_)
        | ast::DataType::Text
        | ast::DataType::String(_)
        | ast::DataType::Char(_)
        | ast::DataType::CharVarying(_)
        // Arrow has no JSON scalar type.  Kaveon carries JSON as UTF-8 JSON
        // text, which is also the representation consumed by the JSON_*()
        // functions.  Keeping JSON textual preserves exact bytes and avoids
        // silently inventing a binary encoding at the SQL boundary.
        | ast::DataType::JSON
        | ast::DataType::JSONB => Ok(CastTarget::Utf8),
        ast::DataType::Date => Ok(CastTarget::Date32),
        ast::DataType::Timestamp(_, timezone) => Ok(CastTarget::TimestampMicros {
            with_timezone: matches!(
                timezone,
                ast::TimezoneInfo::WithTimeZone | ast::TimezoneInfo::Tz
            ),
        }),
        ast::DataType::Decimal(info) | ast::DataType::Numeric(info) => {
            let (p, s) = match info {
                ast::ExactNumberInfo::PrecisionAndScale(p, s) => {
                    let precision = u8::try_from(*p)
                        .map_err(|_| sql_err("DECIMAL precision must be between 1 and 38"))?;
                    let scale = i8::try_from(*s)
                        .map_err(|_| sql_err("DECIMAL scale must be between 0 and precision"))?;
                    (precision, scale)
                }
                ast::ExactNumberInfo::Precision(p) => (
                    u8::try_from(*p)
                        .map_err(|_| sql_err("DECIMAL precision must be between 1 and 38"))?,
                    0,
                ),
                ast::ExactNumberInfo::None => (38, 10),
            };
            if !(1..=38).contains(&p) {
                return Err(sql_err("DECIMAL precision must be between 1 and 38"));
            }
            if s < 0 || s as u8 > p {
                return Err(sql_err("DECIMAL scale must be between 0 and precision"));
            }
            Ok(CastTarget::Decimal128 {
                precision: p,
                scale: s,
            })
        }
        other => Err(sql_err(format!("unsupported CAST target type: {other}"))),
    }
}

fn ast_value_to_expr(value: &ast::Value) -> Result<Expr> {
    match value {
        ast::Value::Number(n, _) => {
            if let Ok(i) = n.parse::<i64>() {
                Ok(Expr::Literal(ScalarValue::Int64(i)))
            } else if n.contains('.') {
                let (int_part, frac_part) = n.split_once('.').unwrap();
                let scale = frac_part.len() as i8;
                let combined = format!("{int_part}{frac_part}");
                if let Ok(v) = combined.parse::<i128>() {
                    let precision = n.replace(['-', '.'], "").len() as u8;
                    let precision = precision.max(scale as u8 + 1);
                    Ok(Expr::Literal(ScalarValue::Decimal128 {
                        value: v,
                        precision: precision.min(38),
                        scale,
                    }))
                } else if let Ok(f) = n.parse::<f64>() {
                    Ok(Expr::Literal(ScalarValue::Float64(f)))
                } else {
                    Err(sql_err(format!("invalid number: {n}")))
                }
            } else if let Ok(f) = n.parse::<f64>() {
                Ok(Expr::Literal(ScalarValue::Float64(f)))
            } else {
                Err(sql_err(format!("invalid number: {n}")))
            }
        }
        ast::Value::SingleQuotedString(s) | ast::Value::DoubleQuotedString(s) => {
            Ok(Expr::Literal(ScalarValue::Utf8(s.clone())))
        }
        ast::Value::Boolean(b) => Ok(Expr::Literal(ScalarValue::Bool(*b))),
        ast::Value::Null => Ok(Expr::Literal(ScalarValue::Null)),
        ast::Value::Placeholder(name) => Err(sql_err(format!(
            "prepared parameter '{name}' is not supported; bind values before submitting SQL"
        ))),
        _ => Err(sql_err(format!("unsupported value: {value}"))),
    }
}

fn parse_decimal_literal(text: &str) -> Result<ScalarValue> {
    let text = text.trim();
    let unsigned = text
        .strip_prefix('+')
        .or_else(|| text.strip_prefix('-'))
        .unwrap_or(text);
    let (whole, fraction) = unsigned.split_once('.').unwrap_or((unsigned, ""));
    if whole.is_empty() && fraction.is_empty()
        || !whole.chars().all(|c| c.is_ascii_digit())
        || !fraction.chars().all(|c| c.is_ascii_digit())
    {
        return Err(sql_err(format!("invalid DECIMAL literal: '{text}'")));
    }
    let scale =
        i8::try_from(fraction.len()).map_err(|_| sql_err("DECIMAL literal scale is too large"))?;
    let digits = format!("{whole}{fraction}");
    let magnitude = digits
        .parse::<i128>()
        .map_err(|_| sql_err(format!("DECIMAL literal is out of range: '{text}'")))?;
    let value = if text.starts_with('-') {
        magnitude
            .checked_neg()
            .ok_or_else(|| sql_err(format!("DECIMAL literal is out of range: '{text}'")))?
    } else {
        magnitude
    };
    let precision = u8::try_from(digits.trim_start_matches('0').len().max(1))
        .unwrap_or(38)
        .min(38);
    Ok(ScalarValue::Decimal128 {
        value,
        precision: precision.max(scale.max(0) as u8 + 1),
        scale,
    })
}

fn ast_binop_to_binop(op: &ast::BinaryOperator) -> Option<BinaryOp> {
    match op {
        ast::BinaryOperator::Eq => Some(BinaryOp::Eq),
        ast::BinaryOperator::NotEq => Some(BinaryOp::Ne),
        ast::BinaryOperator::Lt => Some(BinaryOp::Lt),
        ast::BinaryOperator::LtEq => Some(BinaryOp::Le),
        ast::BinaryOperator::Gt => Some(BinaryOp::Gt),
        ast::BinaryOperator::GtEq => Some(BinaryOp::Ge),
        ast::BinaryOperator::Plus => Some(BinaryOp::Plus),
        ast::BinaryOperator::Minus => Some(BinaryOp::Minus),
        ast::BinaryOperator::Multiply => Some(BinaryOp::Multiply),
        ast::BinaryOperator::Divide => Some(BinaryOp::Divide),
        ast::BinaryOperator::Modulo => Some(BinaryOp::Modulo),
        ast::BinaryOperator::StringConcat => Some(BinaryOp::StringConcat),
        _ => None,
    }
}

fn ast_function_to_expr(func: &ast::Function) -> Result<Expr> {
    let mut name = canonical_function_name(func);
    let is_count_if = name == "COUNT_IF";
    let is_json_object = name == "JSON_OBJECT";
    if name == "MAP" {
        // MAP is lowered as a regular function. The executor validates that
        // both arguments are list values with matching offsets.
    }
    let mut json_object_absent_on_null = false;
    let mut args: Vec<Expr> = match &func.args {
        ast::FunctionArguments::List(arg_list) => {
            if is_json_object {
                for clause in &arg_list.clauses {
                    if matches!(
                        clause,
                        ast::FunctionArgumentClause::JsonNullClause(
                            ast::JsonNullClause::AbsentOnNull
                        )
                    ) {
                        json_object_absent_on_null = true;
                    }
                }
            }
            let mut lowered = Vec::new();
            for arg in &arg_list.args {
                match arg {
                    ast::FunctionArg::Unnamed(ast::FunctionArgExpr::Expr(e)) => {
                        lowered.push(ast_expr_to_expr(e)?);
                    }
                    ast::FunctionArg::Unnamed(ast::FunctionArgExpr::Wildcard) => {
                        lowered.push(Expr::Star);
                    }
                    ast::FunctionArg::ExprNamed { name, arg, .. } if is_json_object => {
                        lowered.push(ast_expr_to_expr(name)?);
                        match arg {
                            ast::FunctionArgExpr::Expr(e) => lowered.push(ast_expr_to_expr(e)?),
                            ast::FunctionArgExpr::Wildcard => lowered.push(Expr::Star),
                            _ => {
                                return Err(sql_err(format!(
                                    "unsupported JSON_OBJECT value argument: {arg}"
                                )));
                            }
                        }
                    }
                    ast::FunctionArg::Named { name, arg, .. } if is_json_object => {
                        lowered.push(Expr::Literal(ScalarValue::Utf8(name.value.clone())));
                        match arg {
                            ast::FunctionArgExpr::Expr(e) => lowered.push(ast_expr_to_expr(e)?),
                            ast::FunctionArgExpr::Wildcard => lowered.push(Expr::Star),
                            _ => {
                                return Err(sql_err(format!(
                                    "unsupported JSON_OBJECT value argument: {arg}"
                                )));
                            }
                        }
                    }
                    _ => return Err(sql_err(format!("unsupported function argument: {arg}"))),
                }
            }
            lowered
        }
        ast::FunctionArguments::None => Vec::new(),
        _ => return Err(sql_err(format!("unsupported function arguments in {name}"))),
    };

    if is_json_object && json_object_absent_on_null {
        name = "JSON_OBJECT_ABSENT_ON_NULL".into();
    }

    // Trino's TRY(expr) turns failures in a scalar expression into NULL.  A
    // safe cast is the most common and most useful form in lake queries, and
    // JSON_PARSE has the same row-local failure contract.  Lower both forms
    // to the vectorized safe evaluators already used by TRY_CAST and
    // TRY_JSON_PARSE.  Other expressions are rejected explicitly until the
    // executor can mask arbitrary per-row failures without changing their
    // normal error semantics.
    if name == "TRY" {
        let [argument] = args.as_slice() else {
            return Err(sql_err("TRY requires exactly one argument"));
        };
        return match argument.clone() {
            Expr::Cast { expr, data_type } => Ok(Expr::Function {
                name: format!("TRY_CAST_{}", cast_target_name(data_type)),
                args: vec![*expr],
            }),
            Expr::Function {
                name: inner_name,
                args: inner_args,
            } if inner_args.len() == 1
                && ["JSON_PARSE", "JSON_FORMAT"]
                    .iter()
                    .any(|supported| inner_name.eq_ignore_ascii_case(supported)) =>
            {
                Ok(Expr::Function {
                    name: format!("TRY_{inner_name}"),
                    args: inner_args,
                })
            }
            Expr::Function {
                name: inner_name,
                args: inner_args,
            } if inner_args.len() == 2 && inner_name.eq_ignore_ascii_case("JSON_ARRAY_GET") => {
                Ok(Expr::Function {
                    name: "TRY_JSON_ARRAY_GET".into(),
                    args: inner_args,
                })
            }
            _ => Err(sql_err(
                "TRY currently supports CAST, JSON_PARSE, JSON_FORMAT, and JSON_ARRAY_GET expressions",
            )),
        };
    }

    // Trino's COUNT_IF(predicate) is COUNT(*) FILTER (WHERE predicate).
    // Lower it to the existing null-preserving COUNT path so local,
    // distributed, spill, and window execution retain one implementation.
    if name == "COUNT_IF" {
        let [predicate] = args.as_slice() else {
            return Err(sql_err("COUNT_IF requires exactly one boolean argument"));
        };
        if matches!(predicate, Expr::Star) {
            return Err(sql_err("COUNT_IF requires a boolean argument"));
        }
        let condition = if let Some(filter) = &func.filter {
            Expr::And(
                Box::new(predicate.clone()),
                Box::new(ast_expr_to_expr(filter)?),
            )
        } else {
            predicate.clone()
        };
        args = vec![Expr::Case {
            operand: None,
            when_then: vec![(condition, Expr::Literal(ScalarValue::Int64(1)))],
            else_expr: Some(Box::new(Expr::Literal(ScalarValue::Null))),
        }];
        name = "COUNT".to_owned();
    }

    if let Some(filter) = &func.filter
        && !is_count_if
    {
        if !is_aggregate_function(&name) {
            return Err(sql_err(format!(
                "FILTER is only supported on aggregate functions, not {name}"
            )));
        }
        let argument = args.first().cloned().unwrap_or(Expr::Star);
        args = vec![aggregate_filter_expr(&name, argument, filter)?];
    }

    if let Some(over) = &func.over {
        // Aggregate FILTER has already been lowered above to a
        // null-preserving CASE argument, so window execution can reuse the
        // same implementation as grouped aggregates.  Ordered-set clauses
        // still require separate semantics.  Value windows carry IGNORE NULLS
        // in the internal name so the executor can apply it to the frame.
        if !func.within_group.is_empty() {
            return Err(sql_err("window WITHIN GROUP is unsupported"));
        }
        let window_name = match func.null_treatment {
            None | Some(ast::NullTreatment::RespectNulls) => name.clone(),
            Some(ast::NullTreatment::IgnoreNulls)
                if matches!(
                    name.as_str(),
                    "FIRST_VALUE" | "LAST_VALUE" | "NTH_VALUE" | "LAG" | "LEAD"
                ) =>
            {
                format!("{name}_IGNORE_NULLS")
            }
            Some(ast::NullTreatment::IgnoreNulls) => {
                return Err(sql_err(format!(
                    "IGNORE NULLS is unsupported for window function {name}"
                )));
            }
        };
        let window_distinct = matches!(
            &func.args,
            ast::FunctionArguments::List(list)
                if matches!(list.duplicate_treatment, Some(ast::DuplicateTreatment::Distinct))
        );
        if let ast::FunctionArguments::List(list) = &func.args
            && !list.clauses.is_empty()
        {
            return Err(sql_err(
                "argument clauses in window functions are unsupported",
            ));
        }
        if window_distinct && !matches!(name.as_str(), "COUNT" | "SUM" | "AVG" | "MIN" | "MAX") {
            return Err(sql_err(format!(
                "DISTINCT is unsupported for window function {name}"
            )));
        }
        let (partition_by, order_by, frame) = match over {
            ast::WindowType::WindowSpec(spec) => {
                let partition_by = spec
                    .partition_by
                    .iter()
                    .map(ast_expr_to_expr)
                    .collect::<Result<Vec<_>>>()?;
                let order_by = spec
                    .order_by
                    .iter()
                    .map(|ob| {
                        let expr = ast_expr_to_expr(&ob.expr)?;
                        let asc = ob.asc.unwrap_or(true);
                        // Trino defaults window ordering to NULLS LAST; retain
                        // an explicit NULLS FIRST/LAST clause when supplied.
                        let nulls_first = ob.nulls_first.unwrap_or(false);
                        Ok((expr, asc, nulls_first))
                    })
                    .collect::<Result<Vec<_>>>()?;
                let frame = spec
                    .window_frame
                    .as_ref()
                    .map(ast_window_frame)
                    .transpose()?;
                (partition_by, order_by, frame)
            }
            ast::WindowType::NamedWindow(_) => {
                return Err(sql_err("named windows are not supported"));
            }
        };
        return Ok(Expr::WindowFunction {
            name: if window_distinct {
                format!("{window_name}_DISTINCT")
            } else {
                window_name
            },
            args,
            partition_by,
            order_by,
            frame,
        });
    }

    let args = if name == "APPROX_PERCENTILE" {
        args.into_iter().map(normalise_fraction).collect()
    } else {
        args
    };
    Ok(Expr::Function { name, args })
}

/// Lower a supported aggregate `FILTER (WHERE predicate)` into the existing
/// null-preserving CASE expression. This keeps aggregate execution and merge
/// semantics unchanged: rows that fail the predicate contribute NULL, while
/// COUNT(*) receives a non-null constant only when the predicate is true.
fn aggregate_filter_expr(name: &str, argument: Expr, filter: &ast::Expr) -> Result<Expr> {
    // Sketch aggregates consume non-null values, so lowering FILTER to a
    // null-preserving CASE is equivalent to filtering their input rows. A
    // column profile is different: it deliberately counts nulls and therefore
    // cannot inherit aggregate FILTER semantics through this lowering.
    if name == "COLUMN_STATISTICS" {
        return Err(sql_err(format!("FILTER is unsupported for {name}")));
    }
    let value = if matches!(argument, Expr::Star) {
        Expr::Literal(ScalarValue::Int64(1))
    } else {
        argument
    };
    Ok(Expr::Case {
        operand: None,
        when_then: vec![(ast_expr_to_expr(filter)?, value)],
        else_expr: Some(Box::new(Expr::Literal(ScalarValue::Null))),
    })
}

/// A percentile as written (`0.5`, `1`, `ARRAY[0.9, 0.99]`) with its
/// numbers as doubles: the form [`AggregateExpr::arguments`] renders, so a
/// projection's call and the aggregate it stands for are the same
/// expression.
fn normalise_fraction(expr: Expr) -> Expr {
    match expr {
        Expr::Literal(ScalarValue::Int64(value)) => {
            Expr::Literal(ScalarValue::Float64(value as f64))
        }
        Expr::Literal(ScalarValue::Decimal128 { value, scale, .. }) => Expr::Literal(
            ScalarValue::Float64(value as f64 / 10f64.powi(i32::from(scale))),
        ),
        Expr::Function { name, args } if name == "ARRAY" => Expr::Function {
            name,
            args: args.into_iter().map(normalise_fraction).collect(),
        },
        other => other,
    }
}

fn contains_aggregate_select_item(item: &ast::SelectItem) -> bool {
    match item {
        ast::SelectItem::UnnamedExpr(expr) | ast::SelectItem::ExprWithAlias { expr, .. } => {
            contains_aggregate_expr(expr)
        }
        _ => false,
    }
}

fn grouping_marker_name(column: &str) -> String {
    format!("__kaveon_grouping_{column}")
}

fn collect_grouping_columns_from_ast_expr(expr: &ast::Expr, out: &mut Vec<String>) -> Result<()> {
    match expr {
        ast::Expr::Function(func) => {
            let name = canonical_function_name(func);
            if name == "GROUPING" || name == "GROUPING_ID" {
                let ast::FunctionArguments::List(args) = &func.args else {
                    return Err(sql_err(format!("{name} requires grouping columns")));
                };
                if args.args.is_empty() || (name == "GROUPING" && args.args.len() != 1) {
                    return Err(sql_err(format!("{name} requires grouping columns")));
                }
                for argument in &args.args {
                    let ast::FunctionArg::Unnamed(ast::FunctionArgExpr::Expr(column)) = argument
                    else {
                        return Err(sql_err(format!(
                            "{name} currently requires column arguments"
                        )));
                    };
                    let Expr::Column(column) = ast_expr_to_expr(column)? else {
                        return Err(sql_err(format!(
                            "{name} currently requires column arguments"
                        )));
                    };
                    if !out.contains(&column) {
                        out.push(column);
                    }
                }
                return Ok(());
            }
            if let ast::FunctionArguments::List(args) = &func.args {
                for argument in &args.args {
                    if let ast::FunctionArg::Unnamed(ast::FunctionArgExpr::Expr(expr)) = argument {
                        collect_grouping_columns_from_ast_expr(expr, out)?;
                    }
                }
            }
            Ok(())
        }
        ast::Expr::BinaryOp { left, right, .. } => {
            collect_grouping_columns_from_ast_expr(left, out)?;
            collect_grouping_columns_from_ast_expr(right, out)
        }
        ast::Expr::UnaryOp { expr, .. }
        | ast::Expr::Nested(expr)
        | ast::Expr::Cast { expr, .. } => collect_grouping_columns_from_ast_expr(expr, out),
        ast::Expr::Case {
            operand,
            conditions,
            results,
            else_result,
        } => {
            if let Some(operand) = operand {
                collect_grouping_columns_from_ast_expr(operand, out)?;
            }
            for condition in conditions {
                collect_grouping_columns_from_ast_expr(condition, out)?;
            }
            for result in results {
                collect_grouping_columns_from_ast_expr(result, out)?;
            }
            if let Some(else_result) = else_result {
                collect_grouping_columns_from_ast_expr(else_result, out)?;
            }
            Ok(())
        }
        _ => Ok(()),
    }
}

fn contains_aggregate_expr(expr: &ast::Expr) -> bool {
    match expr {
        ast::Expr::Function(func) => {
            if func.over.is_some() {
                return false;
            }
            is_aggregate_function(&canonical_function_name(func))
        }
        ast::Expr::BinaryOp { left, right, .. } => {
            contains_aggregate_expr(left) || contains_aggregate_expr(right)
        }
        ast::Expr::UnaryOp { expr, .. } => contains_aggregate_expr(expr),
        ast::Expr::Nested(inner) => contains_aggregate_expr(inner),
        ast::Expr::Case {
            operand,
            conditions,
            results,
            else_result,
        } => {
            operand.as_ref().is_some_and(|e| contains_aggregate_expr(e))
                || conditions.iter().any(contains_aggregate_expr)
                || results.iter().any(contains_aggregate_expr)
                || else_result
                    .as_ref()
                    .is_some_and(|e| contains_aggregate_expr(e))
        }
        ast::Expr::Cast { expr, .. } => contains_aggregate_expr(expr),
        _ => false,
    }
}

fn collect_aggregates_from_select_item(
    item: &ast::SelectItem,
    out: &mut Vec<AggregateExpr>,
) -> Result<()> {
    match item {
        ast::SelectItem::UnnamedExpr(expr) | ast::SelectItem::ExprWithAlias { expr, .. } => {
            collect_aggregates_from_ast_expr(expr, out)
        }
        _ => Ok(()),
    }
}

fn collect_aggregates_from_ast_expr(expr: &ast::Expr, out: &mut Vec<AggregateExpr>) -> Result<()> {
    match expr {
        ast::Expr::Function(func) => {
            if func.over.is_some() {
                return Ok(());
            }
            let name = canonical_function_name(func);
            if is_aggregate_function(&name) {
                // A sketch needs a column and folds every value: no `*`,
                // no DISTINCT.
                let approximate = name.starts_with("APPROX_") || name == "COLUMN_STATISTICS";
                let arg = match &func.args {
                    ast::FunctionArguments::List(args) => {
                        if args.args.is_empty() {
                            Expr::Star
                        } else {
                            match &args.args[0] {
                                ast::FunctionArg::Unnamed(ast::FunctionArgExpr::Wildcard) => {
                                    Expr::Star
                                }
                                ast::FunctionArg::Unnamed(ast::FunctionArgExpr::Expr(e)) => {
                                    ast_expr_to_expr(e)?
                                }
                                _ => return Err(sql_err("unsupported aggregate argument")),
                            }
                        }
                    }
                    _ => Expr::Star,
                };
                if approximate && matches!(arg, Expr::Star) {
                    return Err(sql_err(format!("{name} requires a column argument")));
                }
                let distinct = matches!(
                    &func.args,
                    ast::FunctionArguments::List(args)
                        if args.duplicate_treatment == Some(ast::DuplicateTreatment::Distinct)
                );
                if distinct && matches!(arg, Expr::Star) {
                    return Err(sql_err(format!("{name}(DISTINCT *) is not supported")));
                }
                if distinct && approximate {
                    return Err(sql_err(format!("DISTINCT is not supported for {name}")));
                }
                let arg = if name == "COUNT_IF" {
                    if matches!(&arg, Expr::Star) {
                        return Err(sql_err("COUNT_IF requires a boolean argument"));
                    }
                    let condition = if let Some(filter) = &func.filter {
                        Expr::And(Box::new(arg.clone()), Box::new(ast_expr_to_expr(filter)?))
                    } else {
                        arg.clone()
                    };
                    Expr::Case {
                        operand: None,
                        when_then: vec![(condition, Expr::Literal(ScalarValue::Int64(1)))],
                        else_expr: Some(Box::new(Expr::Literal(ScalarValue::Null))),
                    }
                } else if let Some(filter) = &func.filter {
                    aggregate_filter_expr(&name, arg, filter)?
                } else {
                    arg
                };
                let agg = match name.as_str() {
                    "COUNT" | "COUNT_IF" => AggregateExpr::Count {
                        expr: arg,
                        distinct,
                    },
                    "SUM" => AggregateExpr::Sum {
                        expr: arg,
                        distinct,
                    },
                    "AVG" => AggregateExpr::Avg {
                        expr: arg,
                        distinct,
                    },
                    "MIN" => AggregateExpr::Min(arg),
                    "MAX" => AggregateExpr::Max(arg),
                    "APPROX_COUNT_DISTINCT" => AggregateExpr::ApproxDistinct {
                        expr: arg,
                        as_count: false,
                    },
                    "APPROX_PERCENTILE" => AggregateExpr::ApproxPercentile {
                        expr: arg,
                        percentiles: approx_percentiles(func)?,
                    },
                    "APPROX_COUNT_DISTINCT_STATE" => AggregateExpr::ApproxDistinctState(arg),
                    "COLUMN_STATISTICS" => AggregateExpr::ColumnStatistics(arg),
                    _ => unreachable!(),
                };
                out.push(agg);
            }
            Ok(())
        }
        ast::Expr::BinaryOp { left, right, .. } => {
            collect_aggregates_from_ast_expr(left, out)?;
            collect_aggregates_from_ast_expr(right, out)
        }
        ast::Expr::UnaryOp { expr, .. } => collect_aggregates_from_ast_expr(expr, out),
        ast::Expr::Nested(inner) => collect_aggregates_from_ast_expr(inner, out),
        ast::Expr::Case {
            operand,
            conditions,
            results,
            else_result,
        } => {
            if let Some(op) = operand {
                collect_aggregates_from_ast_expr(op, out)?;
            }
            for c in conditions {
                collect_aggregates_from_ast_expr(c, out)?;
            }
            for r in results {
                collect_aggregates_from_ast_expr(r, out)?;
            }
            if let Some(e) = else_result {
                collect_aggregates_from_ast_expr(e, out)?;
            }
            Ok(())
        }
        ast::Expr::Cast { expr, .. } => collect_aggregates_from_ast_expr(expr, out),
        _ => Ok(()),
    }
}

/// A function's upper-case name, Trino's `APPROX_DISTINCT` as
/// `APPROX_COUNT_DISTINCT`.
fn canonical_function_name(func: &ast::Function) -> String {
    let name = func.name.to_string().to_uppercase();
    if name == "APPROX_DISTINCT" {
        "APPROX_COUNT_DISTINCT".to_owned()
    } else {
        name
    }
}

/// `APPROX_PERCENTILE`'s second argument: a number in `[0, 1]`, or
/// `ARRAY[…]` of them. Constants only — the fractions shape the result.
fn approx_percentiles(func: &ast::Function) -> Result<Percentiles> {
    let ast::FunctionArguments::List(args) = &func.args else {
        return Err(sql_err(
            "APPROX_PERCENTILE requires a column and a percentile",
        ));
    };
    let [
        _,
        ast::FunctionArg::Unnamed(ast::FunctionArgExpr::Expr(second)),
    ] = args.args.as_slice()
    else {
        return Err(sql_err(
            "APPROX_PERCENTILE takes a column and one percentile or ARRAY[…] of percentiles",
        ));
    };
    let fraction = |expr: &ast::Expr| -> Result<f64> {
        let (negative, expr) = match expr {
            ast::Expr::UnaryOp {
                op: ast::UnaryOperator::Minus,
                expr,
            } => (true, expr.as_ref()),
            other => (false, other),
        };
        let ast::Expr::Value(ast::Value::Number(text, _)) = expr else {
            return Err(sql_err(
                "APPROX_PERCENTILE percentiles must be numeric constants",
            ));
        };
        let value = text
            .parse::<f64>()
            .map_err(|_| sql_err(format!("invalid number: {text}")))?;
        Ok(if negative { -value } else { value })
    };
    let percentiles = match second {
        ast::Expr::Array(array) => Percentiles {
            fractions: array.elem.iter().map(fraction).collect::<Result<_>>()?,
            list: true,
        },
        other => Percentiles {
            fractions: vec![fraction(other)?],
            list: false,
        },
    };
    percentiles.validate()?;
    Ok(percentiles)
}

fn collect_window_exprs(select: &ast::Select) -> Result<Vec<Expr>> {
    let mut window_exprs = Vec::new();
    for item in &select.projection {
        match item {
            ast::SelectItem::UnnamedExpr(expr) | ast::SelectItem::ExprWithAlias { expr, .. } => {
                collect_windows_from_ast_expr(expr, &mut window_exprs)?;
            }
            _ => {}
        }
    }
    if let Some(qualify) = &select.qualify {
        collect_windows_from_ast_expr(qualify, &mut window_exprs)?;
    }
    Ok(window_exprs)
}

fn collect_windows_from_ast_expr(expr: &ast::Expr, out: &mut Vec<Expr>) -> Result<()> {
    match expr {
        ast::Expr::Function(func) if func.over.is_some() => {
            out.push(ast_function_to_expr(func)?);
            Ok(())
        }
        ast::Expr::BinaryOp { left, right, .. } => {
            collect_windows_from_ast_expr(left, out)?;
            collect_windows_from_ast_expr(right, out)
        }
        ast::Expr::Nested(inner)
        | ast::Expr::Cast { expr: inner, .. }
        | ast::Expr::UnaryOp { expr: inner, .. } => collect_windows_from_ast_expr(inner, out),
        ast::Expr::Case {
            operand,
            conditions,
            results,
            else_result,
        } => {
            if let Some(op) = operand {
                collect_windows_from_ast_expr(op, out)?;
            }
            for c in conditions {
                collect_windows_from_ast_expr(c, out)?;
            }
            for r in results {
                collect_windows_from_ast_expr(r, out)?;
            }
            if let Some(e) = else_result {
                collect_windows_from_ast_expr(e, out)?;
            }
            Ok(())
        }
        _ => Ok(()),
    }
}

fn ast_date_field_to_date_field(field: &ast::DateTimeField) -> Result<DateField> {
    match field {
        ast::DateTimeField::Year => Ok(DateField::Year),
        ast::DateTimeField::Month => Ok(DateField::Month),
        ast::DateTimeField::Day => Ok(DateField::Day),
        ast::DateTimeField::Hour => Ok(DateField::Hour),
        ast::DateTimeField::Minute => Ok(DateField::Minute),
        ast::DateTimeField::Second => Ok(DateField::Second),
        ast::DateTimeField::Dow => Ok(DateField::DayOfWeek),
        ast::DateTimeField::Doy => Ok(DateField::DayOfYear),
        ast::DateTimeField::Quarter => Ok(DateField::Quarter),
        ast::DateTimeField::Week(_) => Ok(DateField::Week),
        ast::DateTimeField::Epoch => Ok(DateField::Epoch),
        other => Err(sql_err(format!("unsupported EXTRACT field: {other}"))),
    }
}

fn ast_window_frame(frame: &ast::WindowFrame) -> Result<WindowFrame> {
    let units = match frame.units {
        ast::WindowFrameUnits::Rows => WindowFrameUnits::Rows,
        ast::WindowFrameUnits::Range => WindowFrameUnits::Range,
        ast::WindowFrameUnits::Groups => WindowFrameUnits::Groups,
    };
    let start = ast_window_frame_bound(&frame.start_bound)?;
    let end = frame
        .end_bound
        .as_ref()
        .map(ast_window_frame_bound)
        .transpose()?
        .unwrap_or(WindowFrameBound::CurrentRow);
    Ok(WindowFrame { units, start, end })
}

fn ast_window_frame_bound(bound: &ast::WindowFrameBound) -> Result<WindowFrameBound> {
    match bound {
        ast::WindowFrameBound::CurrentRow => Ok(WindowFrameBound::CurrentRow),
        ast::WindowFrameBound::Preceding(None) => Ok(WindowFrameBound::UnboundedPreceding),
        ast::WindowFrameBound::Preceding(Some(expr)) => match expr.as_ref() {
            ast::Expr::Value(v) => match v {
                ast::Value::Number(n, _) => n
                    .parse::<u64>()
                    .map(WindowFrameBound::Preceding)
                    .map_err(|_| sql_err(format!("invalid frame offset: {n}"))),
                _ => Err(sql_err("frame offset must be a number")),
            },
            _ => Err(sql_err("frame offset must be a literal")),
        },
        ast::WindowFrameBound::Following(None) => Ok(WindowFrameBound::UnboundedFollowing),
        ast::WindowFrameBound::Following(Some(expr)) => match expr.as_ref() {
            ast::Expr::Value(v) => match v {
                ast::Value::Number(n, _) => n
                    .parse::<u64>()
                    .map(WindowFrameBound::Following)
                    .map_err(|_| sql_err(format!("invalid frame offset: {n}"))),
                _ => Err(sql_err("frame offset must be a number")),
            },
            _ => Err(sql_err("frame offset must be a literal")),
        },
    }
}

fn validate_uncorrelated(plan: &LogicalPlan) -> Result<()> {
    fn visit<'a>(
        plan: &'a LogicalPlan,
        relations: &mut Vec<String>,
        expressions: &mut Vec<&'a Expr>,
    ) {
        match plan {
            LogicalPlan::Scan { table, alias, .. } => {
                relations.push(alias.clone().unwrap_or_else(|| table.clone()));
                if alias.is_none() {
                    relations.push(table.rsplit('.').next().unwrap().to_owned());
                }
            }
            LogicalPlan::Unnest {
                expressions: values,
                ..
            } => expressions.extend(values),
            LogicalPlan::Filter { input, predicate } => {
                expressions.push(predicate);
                visit(input, relations, expressions);
            }
            LogicalPlan::Project { input, columns } => {
                expressions.extend(columns);
                visit(input, relations, expressions);
            }
            LogicalPlan::Sort { input, order_by } => {
                expressions.extend(order_by.iter().map(|(e, _, _)| e));
                visit(input, relations, expressions);
            }
            LogicalPlan::Window {
                input,
                window_exprs,
            } => {
                expressions.extend(window_exprs);
                visit(input, relations, expressions);
            }
            LogicalPlan::Aggregate {
                input,
                group_by,
                aggregates,
            } => {
                expressions.extend(group_by);
                expressions.extend(aggregates.iter().map(AggregateExpr::argument));
                visit(input, relations, expressions);
            }
            LogicalPlan::Limit { input, .. }
            | LogicalPlan::Offset { input, .. }
            | LogicalPlan::Distinct { input } => visit(input, relations, expressions),
            LogicalPlan::Union { inputs, .. } => {
                for input in inputs {
                    visit(input, relations, expressions);
                }
            }
            LogicalPlan::Join {
                left,
                right,
                condition,
                ..
            } => {
                expressions.extend(condition);
                visit(left, relations, expressions);
                visit(right, relations, expressions);
            }
            LogicalPlan::SemiJoin {
                left,
                right,
                left_key,
                right_key,
                residual,
            }
            | LogicalPlan::AntiJoin {
                left,
                right,
                left_key,
                right_key,
                residual,
            } => {
                expressions.extend([left_key, right_key]);
                expressions.extend(residual);
                visit(left, relations, expressions);
                visit(right, relations, expressions);
            }
            LogicalPlan::Intersect { left, right, .. }
            | LogicalPlan::Except { left, right, .. } => {
                visit(left, relations, expressions);
                visit(right, relations, expressions);
            }
        }
    }
    fn check(expr: &Expr, relations: &[String]) -> Result<()> {
        match expr {
            Expr::Column(name) => {
                if let Some((qualifier, _)) = name.rsplit_once('.')
                    && !relations.iter().any(|r| r.eq_ignore_ascii_case(qualifier))
                {
                    return Err(sql_err(format!(
                        "correlated subqueries are unsupported: {name}"
                    )));
                }
            }
            Expr::BinaryOp { left, right, .. } | Expr::And(left, right) | Expr::Or(left, right) => {
                check(left, relations)?;
                check(right, relations)?;
            }
            Expr::IsNull(e)
            | Expr::IsNotNull(e)
            | Expr::Not(e)
            | Expr::Alias { expr: e, .. }
            | Expr::Cast { expr: e, .. }
            | Expr::Extract { expr: e, .. } => check(e, relations)?,
            Expr::Function { args, .. } => {
                for e in args {
                    check(e, relations)?;
                }
            }
            Expr::WindowFunction {
                args,
                partition_by,
                order_by,
                ..
            } => {
                for e in args
                    .iter()
                    .chain(partition_by)
                    .chain(order_by.iter().map(|(e, _, _)| e))
                {
                    check(e, relations)?;
                }
            }
            Expr::Case {
                operand,
                when_then,
                else_expr,
            } => {
                for e in operand.iter().chain(else_expr) {
                    check(e, relations)?;
                }
                for (a, b) in when_then {
                    check(a, relations)?;
                    check(b, relations)?;
                }
            }
            Expr::Like { expr, pattern, .. } => {
                check(expr, relations)?;
                check(pattern, relations)?;
            }
            Expr::Between {
                expr, low, high, ..
            } => {
                check(expr, relations)?;
                check(low, relations)?;
                check(high, relations)?;
            }
            Expr::InList { expr, list, .. } => {
                check(expr, relations)?;
                for e in list {
                    check(e, relations)?;
                }
            }
            Expr::Literal(_) | Expr::Star => {}
        }
        Ok(())
    }
    let mut relations = Vec::new();
    let mut expressions = Vec::new();
    visit(plan, &mut relations, &mut expressions);
    for expr in expressions {
        check(expr, &relations)?;
    }
    Ok(())
}

fn sql_err(msg: impl Into<String>) -> KaveonError {
    KaveonError::Sql(msg.into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn union_defaults_to_distinct_and_set_quantifiers_are_preserved() {
        assert!(matches!(
            sql_to_logical_plan("SELECT x FROM a UNION SELECT x FROM b").unwrap(),
            LogicalPlan::Distinct { .. }
        ));
        assert!(matches!(
            sql_to_logical_plan("SELECT x FROM a INTERSECT ALL SELECT x FROM b").unwrap(),
            LogicalPlan::Intersect { all: true, .. }
        ));
        assert!(matches!(
            sql_to_logical_plan("SELECT x FROM a EXCEPT ALL SELECT x FROM b").unwrap(),
            LogicalPlan::Except { all: true, .. }
        ));
    }

    #[test]
    fn order_by_a_repeated_select_item_orders_by_its_output() {
        let plan =
            sql_to_logical_plan("SELECT k, COUNT(*) FROM t GROUP BY k ORDER BY COUNT(*) DESC")
                .unwrap();
        let LogicalPlan::Sort { input, order_by } = plan else {
            panic!("sort");
        };
        assert_eq!(order_by, vec![(Expr::Column("expr_1".into()), false, true)]);
        let LogicalPlan::Project { columns, .. } = *input else {
            panic!("projection");
        };
        assert!(matches!(&columns[1], Expr::Alias { name, .. } if name == "expr_1"));
        // An aliased item keeps its alias; a column key is untouched.
        let plan =
            sql_to_logical_plan("SELECT k, SUM(v) AS total FROM t GROUP BY k ORDER BY SUM(v), k")
                .unwrap();
        let LogicalPlan::Sort { order_by, .. } = plan else {
            panic!("sort");
        };
        assert_eq!(
            order_by,
            vec![
                (Expr::Column("total".into()), true, false),
                (Expr::Column("k".into()), true, false)
            ]
        );
    }

    #[test]
    fn order_by_select_ordinal_resolves_to_the_projected_output() {
        let plan = sql_to_logical_plan("SELECT k, COUNT(*) AS n FROM t GROUP BY k ORDER BY 2 DESC")
            .unwrap();
        let LogicalPlan::Sort { order_by, .. } = plan else {
            panic!("sort");
        };
        assert_eq!(order_by, vec![(Expr::Column("n".into()), false, true)]);
    }

    #[test]
    fn order_by_rejects_invalid_select_ordinals() {
        for (sql, message) in [
            ("SELECT k FROM t ORDER BY 0", "greater than zero"),
            ("SELECT k FROM t ORDER BY 2", "not in the select list"),
        ] {
            let error = sql_to_logical_plan(sql).unwrap_err();
            assert!(error.to_string().contains(message), "{sql}: {error}");
        }
    }

    #[test]
    fn order_by_preserves_explicit_null_placement_and_trino_defaults() {
        let LogicalPlan::Sort { order_by, .. } =
            sql_to_logical_plan("SELECT x FROM t ORDER BY x NULLS FIRST").unwrap()
        else {
            panic!("sort");
        };
        assert_eq!(order_by, vec![(Expr::Column("x".into()), true, true)]);

        let LogicalPlan::Sort { order_by, .. } =
            sql_to_logical_plan("SELECT x FROM t ORDER BY x DESC NULLS LAST").unwrap()
        else {
            panic!("sort");
        };
        assert_eq!(order_by, vec![(Expr::Column("x".into()), false, false)]);

        let LogicalPlan::Sort { order_by, .. } =
            sql_to_logical_plan("SELECT x FROM t ORDER BY x").unwrap()
        else {
            panic!("sort");
        };
        assert_eq!(order_by, vec![(Expr::Column("x".into()), true, false)]);
    }

    #[test]
    fn order_by_a_lowered_group_expression_resolves_to_its_column() {
        let plan = sql_to_logical_plan(
            "SELECT t - t % 60 AS m, COUNT(*) AS n FROM h GROUP BY t - t % 60 ORDER BY t - t % 60 LIMIT 10",
        )
        .unwrap();
        // The lowered expression is selected as `m`: the sort orders by
        // that output column and nothing rides along.
        let LogicalPlan::Limit { input, .. } = plan else {
            panic!("limit");
        };
        let LogicalPlan::Sort { order_by, input } = *input else {
            panic!("sort");
        };
        assert_eq!(order_by, vec![(Expr::Column("m".into()), true, false)]);
        assert!(matches!(*input, LogicalPlan::Project { .. }));
    }

    #[test]
    fn order_by_a_qualified_column_selected_under_an_alias_uses_the_alias() {
        // `u.country AS user_country` is what `ORDER BY u.country` means;
        // `t.country` does not stand in for it just because both are
        // "country".
        let plan = sql_to_logical_plan(
            "SELECT t.country, u.country AS user_country, COUNT(*) AS n FROM t JOIN u ON u.id = t.id GROUP BY t.country, u.country ORDER BY n DESC, t.country, u.country LIMIT 3",
        )
        .unwrap();
        let LogicalPlan::Limit { input, .. } = plan else {
            panic!("limit");
        };
        let LogicalPlan::Sort { order_by, .. } = *input else {
            panic!("sort");
        };
        assert_eq!(
            order_by,
            vec![
                (Expr::Column("n".into()), false, true),
                (Expr::Column("t.country".into()), true, false),
                (Expr::Column("user_country".into()), true, false),
            ]
        );
        // An unselected qualified column still rides along, even when a
        // column of the same bare name is selected.
        let plan = sql_to_logical_plan(
            "SELECT t.country, COUNT(*) AS n FROM t JOIN u ON u.id = t.id GROUP BY t.country, u.country ORDER BY u.country LIMIT 3",
        )
        .unwrap();
        let LogicalPlan::Project { input, columns } = plan else {
            panic!("visible projection above the ride-along");
        };
        assert_eq!(
            columns,
            vec![Expr::Column("t.country".into()), Expr::Column("n".into())]
        );
        let LogicalPlan::Limit { input, .. } = *input else {
            panic!("limit");
        };
        let LogicalPlan::Sort { order_by, .. } = *input else {
            panic!("sort");
        };
        assert_eq!(
            order_by,
            vec![(Expr::Column("u.country".into()), true, false)]
        );
    }

    #[test]
    fn order_by_an_unselected_column_rides_through_and_is_dropped() {
        let plan = sql_to_logical_plan(
            "SELECT phrase FROM t WHERE phrase <> '' ORDER BY event_time, phrase LIMIT 10",
        )
        .unwrap();
        let LogicalPlan::Project { input, columns } = plan else {
            panic!("visible projection on top");
        };
        assert_eq!(columns, vec![Expr::Column("phrase".into())]);
        let LogicalPlan::Limit { input, count: 10 } = *input else {
            panic!("limit under the visible projection");
        };
        let LogicalPlan::Sort { input, order_by } = *input else {
            panic!("sort");
        };
        assert_eq!(
            order_by,
            vec![
                (Expr::Column("event_time".into()), true, false),
                (Expr::Column("phrase".into()), true, false)
            ]
        );
        let LogicalPlan::Project { columns, .. } = *input else {
            panic!("extended projection");
        };
        assert_eq!(
            columns,
            vec![
                Expr::Column("phrase".into()),
                Expr::Column("event_time".into())
            ]
        );
        // A selected key changes nothing.
        let plan = sql_to_logical_plan("SELECT a, b FROM t ORDER BY b").unwrap();
        assert!(matches!(plan, LogicalPlan::Sort { .. }));
    }

    #[test]
    fn date_literals_are_day_numbers() {
        assert_eq!(parse_date_days("1970-01-01"), Some(0));
        assert_eq!(parse_date_days("2013-07-01"), Some(15887));
        assert_eq!(parse_date_days("1969-12-31"), Some(-1));
        assert_eq!(parse_date_days("2013-13-01"), None);
        let plan = sql_to_logical_plan("SELECT x FROM t WHERE d >= DATE '2013-07-01'").unwrap();
        let LogicalPlan::Project { input, .. } = plan else {
            panic!("projection");
        };
        let LogicalPlan::Filter { predicate, .. } = *input else {
            panic!("filter");
        };
        assert_eq!(
            predicate,
            Expr::BinaryOp {
                left: Box::new(Expr::Column("d".into())),
                op: BinaryOp::Ge,
                right: Box::new(Expr::Literal(ScalarValue::Int64(15887))),
            }
        );
    }

    #[test]
    fn projected_date_literals_keep_date_type() {
        let plan = sql_to_logical_plan("SELECT DATE '2013-07-01' AS d").unwrap();
        let LogicalPlan::Project { columns, .. } = plan else {
            panic!("projection");
        };
        assert!(matches!(
            &columns[0],
            Expr::Alias { expr, name }
                if name == "d"
                    && matches!(expr.as_ref(), Expr::Cast { data_type: CastTarget::Date32, .. })
        ));
    }

    #[test]
    fn decimal_typed_literals_lower_to_exact_decimal_values() {
        let plan = sql_to_logical_plan("SELECT DECIMAL '1.20' AS amount").unwrap();
        let LogicalPlan::Project { columns, .. } = plan else {
            panic!("projection");
        };
        assert!(matches!(
            &columns[0],
            Expr::Alias { expr, name }
                if name == "amount"
                    && matches!(expr.as_ref(), Expr::Literal(ScalarValue::Decimal128 { value: 120, scale: 2, .. }))
        ));
    }

    #[test]
    fn time_typed_literals_lower_to_runtime_time_values() {
        let plan = sql_to_logical_plan("SELECT TIME '12:34:56.789' AS t").unwrap();
        let LogicalPlan::Project { columns, .. } = plan else {
            panic!("projection");
        };
        assert!(matches!(
            &columns[0],
            Expr::Alias { expr, name }
                if name == "t"
                    && matches!(expr.as_ref(), Expr::Function { name, .. } if name == "TIME_LITERAL")
        ));
    }

    #[test]
    fn timestamp_literals_lower_to_typed_timestamp_casts() {
        let plan = sql_to_logical_plan(
            "SELECT TIMESTAMP '2026-10-13 12:34:56.789' AS ts, TIMESTAMP WITH TIME ZONE '2026-10-13 12:34:56Z' AS utc_ts FROM t",
        )
        .unwrap();
        let LogicalPlan::Project { columns, .. } = plan else {
            panic!("projection");
        };
        assert_eq!(
            columns[0],
            Expr::Alias {
                expr: Box::new(Expr::Cast {
                    expr: Box::new(Expr::Literal(ScalarValue::Utf8(
                        "2026-10-13 12:34:56.789".into(),
                    ))),
                    data_type: CastTarget::TimestampMicros {
                        with_timezone: false,
                    },
                }),
                name: "ts".into(),
            }
        );
        assert!(matches!(
            &columns[1],
            Expr::Alias {
                expr,
                name
            } if name == "utc_ts"
                && matches!(expr.as_ref(), Expr::Cast {
                    data_type: CastTarget::TimestampMicros { with_timezone: true },
                    ..
                })
        ));
    }

    #[test]
    fn interval_arithmetic_on_dates_lowers_to_day_numbers() {
        // A day interval keeps day-number arithmetic for DATE literals and
        // uses a runtime unit-aware function for typed columns.
        let filter = |sql: &str| {
            let plan = sql_to_logical_plan(sql).unwrap();
            let LogicalPlan::Project { input, .. } = plan else {
                panic!("projection");
            };
            let LogicalPlan::Filter { predicate, .. } = *input else {
                panic!("filter");
            };
            predicate
        };
        let day = |days: i64| Box::new(Expr::Literal(ScalarValue::Int64(days)));
        assert_eq!(
            filter("SELECT x FROM t WHERE d <= DATE '1998-12-01' - INTERVAL '90' DAY"),
            Expr::BinaryOp {
                left: Box::new(Expr::Column("d".into())),
                op: BinaryOp::Le,
                right: Box::new(Expr::BinaryOp {
                    left: day(parse_date_days("1998-12-01").unwrap()),
                    op: BinaryOp::Plus,
                    right: day(-90),
                }),
            }
        );
        assert_eq!(
            filter("SELECT x FROM t WHERE d + INTERVAL '7' DAY < e"),
            Expr::BinaryOp {
                left: Box::new(Expr::Function {
                    name: "DATE_ADD_DAYS".into(),
                    args: vec![
                        Expr::Column("d".into()),
                        Expr::Literal(ScalarValue::Int64(7))
                    ],
                }),
                op: BinaryOp::Lt,
                right: Box::new(Expr::Column("e".into())),
            }
        );
        // Month and year intervals shift the calendar of a DATE literal.
        let shifted = |sql: &str, expected: &str| {
            assert_eq!(
                filter(sql),
                Expr::BinaryOp {
                    left: Box::new(Expr::Column("d".into())),
                    op: BinaryOp::Lt,
                    right: day(parse_date_days(expected).unwrap()),
                },
                "{sql}"
            );
        };
        shifted(
            "SELECT x FROM t WHERE d < DATE '1993-07-01' + INTERVAL '3' MONTH",
            "1993-10-01",
        );
        shifted(
            "SELECT x FROM t WHERE d < DATE '1994-01-01' + INTERVAL '1' YEAR",
            "1995-01-01",
        );
        shifted(
            "SELECT x FROM t WHERE d < DATE '1995-11-15' + INTERVAL '3' MONTH",
            "1996-02-15",
        );
        shifted(
            "SELECT x FROM t WHERE d < DATE '1998-01-31' + INTERVAL '1' MONTH",
            "1998-02-28",
        );
        shifted(
            "SELECT x FROM t WHERE d < DATE '1996-01-31' + INTERVAL '1' MONTH",
            "1996-02-29",
        );
        shifted(
            "SELECT x FROM t WHERE d < DATE '1996-03-31' - INTERVAL '1' MONTH",
            "1996-02-29",
        );
        shifted(
            "SELECT x FROM t WHERE d < DATE '2000-02-29' - INTERVAL '1' YEAR",
            "1999-02-28",
        );
        shifted(
            "SELECT x FROM t WHERE d < DATE '1993-01-01' - INTERVAL '13' MONTH",
            "1991-12-01",
        );
        shifted(
            "SELECT x FROM t WHERE d < DATE '1995-01-31' + INTERVAL '1' QUARTER",
            "1995-04-30",
        );
        // A calendar shift of a typed column remains a runtime operation.
        assert_eq!(
            filter("SELECT x FROM t WHERE d + INTERVAL '1' MONTH < e"),
            Expr::BinaryOp {
                left: Box::new(Expr::Function {
                    name: "DATE_ADD_MONTHS".into(),
                    args: vec![
                        Expr::Column("d".into()),
                        Expr::Literal(ScalarValue::Int64(1))
                    ],
                }),
                op: BinaryOp::Lt,
                right: Box::new(Expr::Column("e".into())),
            }
        );
        assert_eq!(
            filter("SELECT x FROM t WHERE ts + INTERVAL '2' HOUR < e"),
            Expr::BinaryOp {
                left: Box::new(Expr::Function {
                    name: "DATE_ADD_SECONDS".into(),
                    args: vec![
                        Expr::Column("ts".into()),
                        Expr::Literal(ScalarValue::Int64(7_200))
                    ],
                }),
                op: BinaryOp::Lt,
                right: Box::new(Expr::Column("e".into())),
            }
        );
        assert_eq!(
            filter("SELECT x FROM t WHERE d + INTERVAL '2' WEEK < e"),
            Expr::BinaryOp {
                left: Box::new(Expr::Function {
                    name: "DATE_ADD_DAYS".into(),
                    args: vec![
                        Expr::Column("d".into()),
                        Expr::Literal(ScalarValue::Int64(14))
                    ],
                }),
                op: BinaryOp::Lt,
                right: Box::new(Expr::Column("e".into())),
            }
        );
        // The binder supplies the source type; logical planning preserves the
        // operation and the executor rejects a DATE column explicitly.
        assert!(sql_to_logical_plan("SELECT x FROM t WHERE d + INTERVAL '1' HOUR < e").is_ok());
        let error = sql_to_logical_plan(
            "SELECT x FROM t WHERE d < DATE '2000-01-01' + INTERVAL '9223372036854775807' YEAR",
        )
        .unwrap_err()
        .to_string();
        assert!(error.contains("calendar offset overflows"), "{error}");
        assert!(sql_to_logical_plan("SELECT INTERVAL '1' DAY FROM t").is_err());
        assert!(
            sql_to_logical_plan("SELECT x FROM t WHERE d < DATE '1993-07-01' + INTERVAL '1' HOUR")
                .is_err()
        );
    }

    #[test]
    fn civil_dates_round_trip_through_day_numbers() {
        for date in [
            "1970-01-01",
            "1969-12-31",
            "1992-01-01",
            "1996-02-29",
            "1998-12-31",
            "2000-02-29",
            "2026-09-16",
            "1900-03-01",
            "2100-02-28",
        ] {
            let days = parse_date_days(date).unwrap();
            let (year, month, day) = civil_from_days(days);
            assert_eq!(format!("{year:04}-{month:02}-{day:02}"), date);
        }
    }

    #[test]
    fn group_by_without_aggregates_is_distinct_over_the_keys() {
        let plan =
            sql_to_logical_plan("SELECT country FROM u WHERE locale = 1 GROUP BY country").unwrap();
        let LogicalPlan::Project { input, .. } = plan else {
            panic!("outer projection");
        };
        let LogicalPlan::Distinct { input } = *input else {
            panic!("distinct over the keys");
        };
        let LogicalPlan::Project { input, columns } = *input else {
            panic!("key projection");
        };
        assert_eq!(columns, vec![Expr::Column("country".into())]);
        assert!(matches!(*input, LogicalPlan::Filter { .. }));
        // An expression key still aggregates.
        assert!(matches!(
            sql_to_logical_plan("SELECT UPPER(country) FROM u GROUP BY UPPER(country)").unwrap(),
            LogicalPlan::Project { input, .. } if matches!(*input, LogicalPlan::Aggregate { .. })
        ));
    }

    #[test]
    fn grouping_modifiers_fail_closed_until_their_lowering_exists() {
        for sql in [
            "SELECT country, COUNT(*) FROM u GROUP BY country WITH ROLLUP",
            "SELECT country, COUNT(*) FROM u GROUP BY country WITH CUBE",
            "SELECT country, COUNT(*) FROM u GROUP BY country WITH TOTALS",
            "SELECT country, COUNT(*) FROM u GROUP BY ALL",
        ] {
            assert!(sql_to_logical_plan(sql).is_err(), "{sql} must fail closed");
        }
    }

    #[test]
    fn lowers_group_and_aggregate_expressions_below_aggregation() {
        let plan = sql_to_logical_plan("SELECT CAST(CAST(x AS DECIMAL(20,4)) AS VARCHAR) AS k, SUM(CAST(x AS DECIMAL(20,4))) FROM t GROUP BY CAST(x AS DECIMAL(20,4))").unwrap();
        let LogicalPlan::Project { input, columns } = plan else {
            panic!("project");
        };
        let LogicalPlan::Aggregate {
            input,
            group_by,
            aggregates,
        } = *input
        else {
            panic!("aggregate");
        };
        assert!(matches!(&*input, LogicalPlan::Project { .. }));
        assert!(matches!(&group_by[0], Expr::Column(name) if name == "__kaveon_group_0"));
        assert!(
            matches!(&aggregates[0], AggregateExpr::Sum { expr: Expr::Column(name), .. } if name == "__kaveon_arg_0")
        );
        assert!(matches!(&columns[1], Expr::Column(name) if name == "sum___kaveon_arg_0"));
    }

    #[test]
    fn constant_group_key_is_projected_without_hashing() {
        let plan = sql_to_logical_plan(
            "SELECT 1, URL, COUNT(*) AS c FROM hits GROUP BY 1, URL ORDER BY c DESC LIMIT 10",
        )
        .unwrap();
        let LogicalPlan::Limit { input, .. } = plan else {
            panic!("limit");
        };
        let LogicalPlan::Sort { input, .. } = *input else {
            panic!("sort");
        };
        let LogicalPlan::Project { input, .. } = *input else {
            panic!("projection");
        };
        let LogicalPlan::Aggregate { group_by, .. } = *input else {
            panic!("aggregate");
        };
        assert_eq!(group_by, vec![Expr::Column("URL".into())]);
    }

    #[test]
    fn an_expression_over_a_simple_aggregate_is_lowered_to_its_column() {
        let plan = sql_to_logical_plan("SELECT 0.2 * avg(x) AS a, max(y) FROM t").unwrap();
        let LogicalPlan::Project { input, columns } = plan else {
            panic!("project");
        };
        assert_eq!(
            columns[0],
            Expr::Alias {
                expr: Box::new(Expr::BinaryOp {
                    left: Box::new(Expr::Literal(ScalarValue::Decimal128 {
                        value: 2,
                        precision: 2,
                        scale: 1
                    })),
                    op: BinaryOp::Multiply,
                    right: Box::new(Expr::Column("avg___kaveon_arg_0".into())),
                }),
                name: "a".into()
            }
        );
        assert_eq!(columns[1], Expr::Column("max___kaveon_arg_1".into()));
        let LogicalPlan::Aggregate { aggregates, .. } = *input else {
            panic!("aggregate");
        };
        assert!(
            matches!(&aggregates[0], AggregateExpr::Avg { expr: Expr::Column(name), .. } if name == "__kaveon_arg_0")
        );
        // Plain and aliased aggregates keep their direct form and names.
        let plan = sql_to_logical_plan("SELECT sum(x), avg(y) AS a FROM t").unwrap();
        let LogicalPlan::Project { columns, .. } = plan else {
            panic!("project");
        };
        assert!(matches!(&columns[0], Expr::Function { name, .. } if name == "SUM"));
    }

    #[test]
    fn parses_simple_select() {
        let plan = sql_to_logical_plan("SELECT * FROM users").unwrap();
        match plan {
            LogicalPlan::Scan { table, .. } => assert_eq!(table, "users"),
            _ => panic!("expected Scan for SELECT *"),
        }
    }

    #[test]
    fn parses_constant_array_unnest_in_from() {
        let plan = sql_to_logical_plan("SELECT * FROM UNNEST(ARRAY[1, 2]) AS t(value)").unwrap();
        assert!(matches!(
            plan,
            LogicalPlan::Unnest {
                columns,
                with_ordinality: false,
                ..
            } if columns == vec!["value"]
        ));
    }

    #[test]
    fn parses_sequence_array_unnest_in_from() {
        let plan = sql_to_logical_plan("SELECT * FROM UNNEST(SEQUENCE(1, 3)) AS t(value)").unwrap();
        assert!(matches!(
            plan,
            LogicalPlan::Unnest { columns, .. } if columns == vec!["value"]
        ));
    }

    #[test]
    fn parses_array_function_unnest_in_from() {
        let plan = sql_to_logical_plan(
            "SELECT * FROM UNNEST(ARRAY_CONCAT(ARRAY[1], ARRAY[2, 3])) AS t(value)",
        )
        .unwrap();
        assert!(matches!(
            plan,
            LogicalPlan::Unnest { columns, .. } if columns == vec!["value"]
        ));
    }

    #[test]
    fn parses_map_array_function_unnest_in_from() {
        let plan = sql_to_logical_plan(
            "SELECT * FROM UNNEST(MAP_KEYS(MAP(ARRAY['a'], ARRAY[1]))) AS t(value)",
        )
        .unwrap();
        assert!(matches!(
            plan,
            LogicalPlan::Unnest { columns, .. } if columns == vec!["value"]
        ));
    }

    #[test]
    fn parses_map_entries_array_unnest_in_from() {
        let plan = sql_to_logical_plan(
            "SELECT * FROM UNNEST(MAP_ENTRIES(MAP(ARRAY['a'], ARRAY[1]))) AS t(key, value)",
        )
        .unwrap();
        assert!(matches!(
            plan,
            LogicalPlan::Unnest { columns, .. } if columns == vec!["key", "value"]
        ));
    }

    #[test]
    fn parses_row_array_unnest_in_from() {
        let plan = sql_to_logical_plan(
            "SELECT * FROM UNNEST(ARRAY[ROW(1, 'a'), ROW(2, 'b')]) AS t(id, label)",
        )
        .unwrap();
        assert!(matches!(
            plan,
            LogicalPlan::Unnest { columns, .. } if columns == vec!["id", "label"]
        ));
    }

    #[test]
    fn parses_lateral_function_unnest_in_from() {
        let plan = sql_to_logical_plan(
            "SELECT * FROM (VALUES (1)) AS x(id) CROSS JOIN LATERAL UNNEST(ARRAY[10, 20]) AS u(value)",
        )
        .unwrap();
        let rendered = format!("{plan:?}");
        assert!(
            rendered.contains("Unnest"),
            "expected lateral UNNEST plan: {rendered}"
        );
        assert!(
            rendered.contains("value"),
            "expected alias in lateral UNNEST plan: {rendered}"
        );
    }

    #[test]
    fn binds_correlated_lateral_unnest_against_the_left_scope() {
        let plan = sql_to_logical_plan(
            "SELECT x.id, u.value FROM (VALUES (1)) AS x(id) CROSS JOIN LATERAL UNNEST(ARRAY[x.id, 2]) AS u(value)",
        )
        .expect("a lateral array may reference the left relation");
        let rendered = format!("{plan:?}");
        assert!(
            rendered.contains("join_type: Cross"),
            "expected lateral cross join: {rendered}"
        );
        assert!(
            rendered.contains("x.id"),
            "expected bound outer column: {rendered}"
        );
    }

    #[test]
    fn binds_correlated_lateral_unnest_with_ordinality() {
        let plan = sql_to_logical_plan(
            "SELECT x.id, u.value, u.ord FROM (VALUES (1)) AS x(id) CROSS JOIN LATERAL UNNEST(ARRAY[x.id, 2]) WITH ORDINALITY AS u(value, ord)",
        )
        .expect("a lateral array may carry ordinality");
        let rendered = format!("{plan:?}");
        assert!(rendered.contains("with_ordinality: true"), "{rendered}");
        assert!(
            rendered.contains("ordinality_column: \"ord\""),
            "{rendered}"
        );
    }

    #[test]
    fn binds_correlated_lateral_unnest_with_offset_alias_order() {
        let plan = sql_to_logical_plan(
            "SELECT x.id, u.value, u.pos FROM (VALUES (1)) AS x(id) CROSS JOIN LATERAL UNNEST(ARRAY[x.id, 2]) AS u(value) WITH OFFSET AS pos",
        )
        .expect("the dialect offset form should parse");
        let rendered = format!("{plan:?}");
        assert!(rendered.contains("ordinality_start: 0"), "{rendered}");
    }

    #[test]
    fn parses_multiple_constant_array_unnest_in_from() {
        let plan = sql_to_logical_plan(
            "SELECT * FROM UNNEST(ARRAY[1, 2], ARRAY[3]) AS t(left_col, right_col)",
        )
        .unwrap();
        assert!(matches!(
            plan,
            LogicalPlan::Unnest {
                expressions,
                columns,
                with_ordinality: false,
                ..
            } if expressions.len() == 2 && columns == vec!["left_col", "right_col"]
        ));
    }

    #[test]
    fn parses_constant_array_unnest_with_ordinality() {
        let plan = sql_to_logical_plan(
            "SELECT * FROM UNNEST(ARRAY['a', 'b']) WITH ORDINALITY AS t(value, n)",
        )
        .unwrap();
        assert!(matches!(
            plan,
            LogicalPlan::Unnest {
                columns,
                with_ordinality: true,
                ordinality_column,
                ..
            } if columns == vec!["value"] && ordinality_column == "n"
        ));
    }

    #[test]
    fn parses_constant_array_unnest_with_zero_based_offset() {
        let plan =
            sql_to_logical_plan("SELECT * FROM UNNEST(ARRAY['a', 'b']) WITH OFFSET AS n").unwrap();
        assert!(matches!(
            plan,
            LogicalPlan::Unnest {
                columns,
                with_ordinality: true,
                ordinality_column,
                ordinality_start: 0,
                ..
            } if columns == vec!["unnest"] && ordinality_column == "n"
        ));
    }

    #[test]
    fn parses_select_with_columns() {
        let plan = sql_to_logical_plan("SELECT name, age FROM users").unwrap();
        match plan {
            LogicalPlan::Project { columns, .. } => assert_eq!(columns.len(), 2),
            _ => panic!("expected Project"),
        }
    }

    #[test]
    fn parses_where_clause() {
        let plan = sql_to_logical_plan("SELECT * FROM users WHERE age > 21").unwrap();
        match plan {
            LogicalPlan::Filter { predicate, .. } => match predicate {
                Expr::BinaryOp { op, .. } => assert_eq!(op, BinaryOp::Gt),
                _ => panic!("expected BinaryOp"),
            },
            _ => panic!("expected Filter"),
        }
    }

    #[test]
    fn parses_aggregate() {
        let plan = sql_to_logical_plan("SELECT city, COUNT(*) FROM users GROUP BY city").unwrap();
        match plan {
            LogicalPlan::Project { input, .. } => match *input {
                LogicalPlan::Aggregate {
                    group_by,
                    aggregates,
                    ..
                } => {
                    assert_eq!(group_by.len(), 1);
                    assert_eq!(aggregates.len(), 1);
                }
                _ => panic!("expected Aggregate under Project"),
            },
            _ => panic!("expected Project"),
        }
    }

    #[test]
    fn parses_order_by() {
        let plan = sql_to_logical_plan("SELECT * FROM users ORDER BY name ASC, age DESC").unwrap();
        match plan {
            LogicalPlan::Sort { order_by, .. } => {
                assert_eq!(order_by.len(), 2);
                assert!(order_by[0].1);
                assert!(!order_by[1].1);
            }
            _ => panic!("expected Sort"),
        }
    }

    #[test]
    fn parses_limit() {
        let plan = sql_to_logical_plan("SELECT * FROM users LIMIT 10").unwrap();
        match plan {
            LogicalPlan::Limit { count, .. } => assert_eq!(count, 10),
            _ => panic!("expected Limit"),
        }
    }

    #[test]
    fn parses_complex_query() {
        let plan = sql_to_logical_plan(
            "SELECT city, SUM(amount) FROM orders WHERE status = 'completed' GROUP BY city ORDER BY city LIMIT 5",
        )
        .unwrap();
        match plan {
            LogicalPlan::Limit { count: 5, input } => match *input {
                LogicalPlan::Sort { input, .. } => match *input {
                    LogicalPlan::Project { input, .. } => match *input {
                        LogicalPlan::Aggregate { input, .. } => match *input {
                            LogicalPlan::Filter { input, .. } => match *input {
                                LogicalPlan::Scan { table, .. } => {
                                    assert_eq!(table, "orders");
                                }
                                _ => panic!("expected Scan"),
                            },
                            _ => panic!("expected Filter"),
                        },
                        _ => panic!("expected Aggregate"),
                    },
                    _ => panic!("expected Project"),
                },
                _ => panic!("expected Sort"),
            },
            _ => panic!("expected Limit"),
        }
    }

    #[test]
    fn rejects_empty_query() {
        assert!(sql_to_logical_plan("").is_err());
    }

    #[test]
    fn rejects_insert() {
        assert!(sql_to_logical_plan("INSERT INTO users VALUES (1)").is_err());
    }

    #[test]
    fn preserves_count_distinct_semantics() {
        // A lone COUNT(DISTINCT x) counts the distinct x rows instead.
        let plan = sql_to_logical_plan("SELECT COUNT(DISTINCT user_id) FROM events").unwrap();
        let LogicalPlan::Project { input, .. } = plan else {
            panic!("expected project")
        };
        let LogicalPlan::Aggregate {
            aggregates, input, ..
        } = *input
        else {
            panic!("expected aggregate")
        };
        assert!(matches!(
            aggregates.as_slice(),
            [AggregateExpr::Count { distinct: false, expr: Expr::Column(column) }] if column == "user_id"
        ));
        let LogicalPlan::Distinct { input } = *input else {
            panic!("distinct rows feed the count");
        };
        assert!(matches!(*input, LogicalPlan::Project { ref columns, .. }
            if columns == &[Expr::Column("user_id".into())]));
        // Mixed with another aggregate, the exact distinct state remains.
        let plan =
            sql_to_logical_plan("SELECT k, COUNT(DISTINCT user_id), SUM(v) FROM events GROUP BY k")
                .unwrap();
        let LogicalPlan::Project { input, .. } = plan else {
            panic!("expected project")
        };
        let LogicalPlan::Aggregate { aggregates, .. } = *input else {
            panic!("expected aggregate")
        };
        assert!(matches!(
            aggregates.as_slice(),
            [
                AggregateExpr::Count { distinct: true, .. },
                AggregateExpr::Sum { .. }
            ]
        ));
    }

    #[test]
    fn plans_supported_join_types() {
        for (keyword, expected) in [
            ("INNER JOIN", JoinType::Inner),
            ("LEFT JOIN", JoinType::Left),
            ("RIGHT JOIN", JoinType::Right),
            ("FULL JOIN", JoinType::Full),
        ] {
            let sql = format!("SELECT * FROM users u {keyword} orders o ON u.id = o.user_id");
            let LogicalPlan::Join {
                join_type,
                condition,
                ..
            } = sql_to_logical_plan(&sql).unwrap()
            else {
                panic!("expected join")
            };
            assert_eq!(join_type, expected);
            assert!(condition.is_some());
        }
        let LogicalPlan::Join {
            join_type,
            condition,
            ..
        } = sql_to_logical_plan("SELECT * FROM users CROSS JOIN regions").unwrap()
        else {
            panic!("expected cross join")
        };
        assert_eq!(join_type, JoinType::Cross);
        assert!(condition.is_none());
    }

    #[test]
    fn lowers_join_using_to_qualified_equality_keys() {
        let plan =
            sql_to_logical_plan("SELECT u.id, o.amount FROM users u JOIN orders o USING (id)")
                .unwrap();
        let LogicalPlan::Project { input, .. } = plan else {
            panic!("unexpected plan: {plan:?}")
        };
        let LogicalPlan::Join {
            join_type: JoinType::Inner,
            condition: Some(Expr::BinaryOp { left, op, right }),
            ..
        } = *input
        else {
            panic!("JOIN USING should lower to an inner equality join")
        };
        assert_eq!(op, BinaryOp::Eq);
        assert_eq!(*left, Expr::Column("u.id".into()));
        assert_eq!(*right, Expr::Column("o.id".into()));
    }

    #[test]
    fn rejects_using_over_a_composite_left_relation_until_bound() {
        let error =
            sql_to_logical_plan("SELECT * FROM (SELECT id FROM users) u JOIN orders o USING (id)")
                .unwrap_err();
        assert!(
            error
                .to_string()
                .contains("single base relation on the left")
        );
    }

    #[test]
    fn lowers_inner_residual_join_to_cross_join_and_filter() {
        let plan = sql_to_logical_plan(
            "SELECT * FROM users u JOIN orders o ON u.id = o.user_id AND u.value > o.value",
        )
        .unwrap();
        let LogicalPlan::Filter { input, predicate } = plan else {
            panic!("an inner residual join should be filtered after its cross join");
        };
        assert!(matches!(predicate, Expr::And(_, _)));
        let LogicalPlan::Join {
            join_type: JoinType::Cross,
            condition: None,
            ..
        } = *input
        else {
            panic!("residual inner join should lower to a cross join");
        };
    }

    #[test]
    fn parses_case_when() {
        let plan = sql_to_logical_plan("SELECT CASE WHEN x > 1 THEN 'big' ELSE 'small' END FROM t")
            .unwrap();
        assert!(matches!(plan, LogicalPlan::Project { .. }));
    }

    #[test]
    fn parses_like() {
        let plan = sql_to_logical_plan("SELECT * FROM t WHERE name LIKE '%foo%'").unwrap();
        match plan {
            LogicalPlan::Filter { predicate, .. } => {
                assert!(matches!(predicate, Expr::Like { negated: false, .. }));
            }
            _ => panic!("expected Filter"),
        }
    }

    #[test]
    fn parses_not_like() {
        let plan = sql_to_logical_plan("SELECT * FROM t WHERE name NOT LIKE 'a%'").unwrap();
        match plan {
            LogicalPlan::Filter { predicate, .. } => {
                assert!(matches!(predicate, Expr::Like { negated: true, .. }));
            }
            _ => panic!("expected Filter"),
        }
    }

    #[test]
    fn parses_between() {
        let plan = sql_to_logical_plan("SELECT * FROM t WHERE x BETWEEN 1 AND 10").unwrap();
        match plan {
            LogicalPlan::Filter { predicate, .. } => {
                assert!(matches!(predicate, Expr::Between { negated: false, .. }));
            }
            _ => panic!("expected Filter"),
        }
    }

    #[test]
    fn parses_in_list() {
        let plan = sql_to_logical_plan("SELECT * FROM t WHERE x IN (1, 2, 3)").unwrap();
        match plan {
            LogicalPlan::Filter { predicate, .. } => {
                assert!(matches!(
                    predicate,
                    Expr::InList {
                        negated: false,
                        ref list,
                        ..
                    } if list.len() == 3
                ));
            }
            _ => panic!("expected Filter"),
        }
    }

    #[test]
    fn parses_cast() {
        let plan = sql_to_logical_plan("SELECT CAST(x AS BIGINT) FROM t").unwrap();
        match plan {
            LogicalPlan::Project { columns, .. } => {
                assert!(matches!(
                    columns[0],
                    Expr::Cast {
                        data_type: CastTarget::Int64,
                        ..
                    }
                ));
            }
            _ => panic!("expected Project"),
        }
    }

    #[test]
    fn parses_cast_to_date() {
        let plan = sql_to_logical_plan("SELECT CAST(event_date AS DATE) FROM events").unwrap();
        let LogicalPlan::Project { columns, .. } = plan else {
            panic!("expected Project");
        };
        assert!(matches!(
            columns[0],
            Expr::Cast {
                data_type: CastTarget::Date32,
                ..
            }
        ));
    }

    #[test]
    fn parses_cast_to_timestamp_with_timezone() {
        let plan =
            sql_to_logical_plan("SELECT CAST(event_time AS TIMESTAMP WITH TIME ZONE) FROM events")
                .unwrap();
        let LogicalPlan::Project { columns, .. } = plan else {
            panic!("expected Project");
        };
        assert!(matches!(
            columns[0],
            Expr::Cast {
                data_type: CastTarget::TimestampMicros {
                    with_timezone: true
                },
                ..
            }
        ));
    }

    #[test]
    fn parses_having() {
        let plan =
            sql_to_logical_plan("SELECT city, COUNT(*) FROM t GROUP BY city HAVING COUNT(*) > 5")
                .unwrap();
        match plan {
            LogicalPlan::Project { input, .. } => match *input {
                LogicalPlan::Filter { input, .. } => {
                    assert!(matches!(*input, LogicalPlan::Aggregate { .. }));
                }
                _ => panic!("expected Filter (HAVING) after Aggregate"),
            },
            _ => panic!("expected Project"),
        }
    }

    #[test]
    fn having_aggregates_are_computed_even_when_the_projection_does_not_select_them() {
        let plan = sql_to_logical_plan(
            "SELECT l_orderkey FROM lineitem GROUP BY l_orderkey HAVING sum(l_quantity) > 300",
        )
        .unwrap();
        let LogicalPlan::Project { input, columns } = plan else {
            panic!("projection");
        };
        assert_eq!(columns, vec![Expr::Column("l_orderkey".into())]);
        let LogicalPlan::Filter { input, predicate } = *input else {
            panic!("HAVING filter");
        };
        assert!(matches!(
            predicate,
            Expr::BinaryOp {
                op: BinaryOp::Gt,
                ..
            }
        ));
        let LogicalPlan::Aggregate {
            group_by,
            aggregates,
            ..
        } = *input
        else {
            panic!("an aggregate, not the DISTINCT a bare GROUP BY lowers to");
        };
        assert_eq!(group_by, vec![Expr::Column("l_orderkey".into())]);
        assert_eq!(
            aggregates,
            vec![AggregateExpr::Sum {
                expr: Expr::Column("l_quantity".into()),
                distinct: false
            }]
        );
        // An aggregate the projection already carries is computed once.
        let plan = sql_to_logical_plan(
            "SELECT k, SUM(x) AS s FROM t GROUP BY k HAVING SUM(x) > 1 AND COUNT(*) > 2",
        )
        .unwrap();
        let LogicalPlan::Project { input, .. } = plan else {
            panic!("projection");
        };
        let LogicalPlan::Filter { input, .. } = *input else {
            panic!("HAVING filter");
        };
        let LogicalPlan::Aggregate { aggregates, .. } = *input else {
            panic!("aggregate");
        };
        assert_eq!(
            aggregates,
            vec![
                AggregateExpr::Sum {
                    expr: Expr::Column("x".into()),
                    distinct: false
                },
                AggregateExpr::Count {
                    expr: Expr::Star,
                    distinct: false
                }
            ]
        );
    }

    #[test]
    fn a_scalar_subquery_in_where_is_a_single_row_join() {
        let plan =
            sql_to_logical_plan("SELECT a FROM t WHERE b > (SELECT avg(b) FROM t WHERE c = 1)")
                .unwrap();
        let LogicalPlan::Project { input, .. } = plan else {
            panic!("projection");
        };
        let LogicalPlan::Filter { input, predicate } = *input else {
            panic!("filter");
        };
        assert_eq!(
            predicate,
            Expr::BinaryOp {
                left: Box::new(Expr::Column("b".into())),
                op: BinaryOp::Gt,
                right: Box::new(Expr::Column("__kaveon_scalar_0".into())),
            }
        );
        let LogicalPlan::Join {
            left,
            right,
            join_type: JoinType::Cross,
            condition: None,
            ..
        } = *input
        else {
            panic!("cross join with the one-row aggregate");
        };
        assert!(matches!(*left, LogicalPlan::Scan { .. }));
        let LogicalPlan::Project { input, columns } = *right else {
            panic!("the subquery projects its one column under the scalar's name");
        };
        assert_eq!(
            columns,
            vec![Expr::Alias {
                expr: Box::new(Expr::Function {
                    name: "AVG".into(),
                    args: vec![Expr::Column("b".into())]
                }),
                name: "__kaveon_scalar_0".into()
            }]
        );
        assert!(matches!(*input, LogicalPlan::Aggregate { .. }));
        // Two scalars take distinct names.
        let plan = sql_to_logical_plan(
            "SELECT a FROM t WHERE b > (SELECT avg(b) FROM t) AND c < (SELECT max(c) FROM u)",
        )
        .unwrap();
        let LogicalPlan::Project { input, .. } = plan else {
            panic!("projection");
        };
        let LogicalPlan::Filter { predicate, .. } = *input else {
            panic!("filter");
        };
        let Expr::And(_, second) = predicate else {
            panic!("both conjuncts");
        };
        assert!(matches!(
            *second,
            Expr::BinaryOp { right, .. } if *right == Expr::Column("__kaveon_scalar_1".into())
        ));
        // A subquery that could yield several rows is refused.
        for sql in [
            "SELECT a FROM t WHERE b > (SELECT b FROM t)",
            "SELECT a FROM t WHERE b > (SELECT max(b) FROM t GROUP BY c)",
            "SELECT a FROM t WHERE b > (SELECT max(b), min(b) FROM t)",
        ] {
            assert!(sql_to_logical_plan(sql).is_err(), "{sql}");
        }
        let plan = sql_to_logical_plan("SELECT (SELECT max(b) FROM t) AS max_b FROM t")
            .expect("single-row scalar projections are supported");
        let LogicalPlan::Project { input, columns } = plan else {
            panic!("projection");
        };
        assert_eq!(
            columns,
            vec![Expr::Alias {
                expr: Box::new(Expr::Column("__kaveon_scalar_0".into())),
                name: "max_b".into(),
            }]
        );
        let LogicalPlan::Join {
            left,
            right,
            join_type: JoinType::Cross,
            condition: None,
            ..
        } = *input
        else {
            panic!("projection scalar should cross join its one-row relation");
        };
        assert!(matches!(*left, LogicalPlan::Scan { .. }));
        assert!(matches!(*right, LogicalPlan::Project { .. }));

        let plan = sql_to_logical_plan("SELECT (SELECT 42) AS answer FROM t")
            .expect("a scalar literal subquery has one implicit row");
        let LogicalPlan::Project { input, columns } = plan else {
            panic!("literal scalar projection");
        };
        assert_eq!(
            columns,
            vec![Expr::Alias {
                expr: Box::new(Expr::Column("__kaveon_scalar_0".into())),
                name: "answer".into(),
            }]
        );
        let LogicalPlan::Join { right, .. } = *input else {
            panic!("literal scalar should cross join its one-row relation");
        };
        assert!(matches!(*right, LogicalPlan::Project { .. }));
        assert!(sql_to_logical_plan("SELECT (SELECT 42 WHERE false) FROM t").is_err());
    }

    #[test]
    fn scalar_subquery_rejects_non_single_row_shapes_with_actionable_errors() {
        let cases = [
            (
                "SELECT a FROM t WHERE b > (SELECT b FROM t)",
                "a scalar subquery must be an ungrouped aggregate, so that it yields one row",
            ),
            (
                "SELECT a FROM t WHERE b > (SELECT max(b), min(b) FROM t)",
                "a scalar subquery must select exactly one column",
            ),
            (
                "SELECT (SELECT b FROM t) FROM t",
                "a scalar subquery must be an ungrouped aggregate, so that it yields one row",
            ),
        ];
        for (sql, expected) in cases {
            let error = sql_to_logical_plan(sql).expect_err(sql);
            assert!(
                error.to_string().contains(expected),
                "{sql}: expected `{expected}` in `{error}`"
            );
        }
    }

    #[test]
    fn scalar_aggregate_positive_limit_keeps_the_scalar_shape() {
        let plan = sql_to_logical_plan("SELECT (SELECT avg(b) FROM t LIMIT 1) AS answer FROM t")
            .expect("a positive limit cannot change an ungrouped aggregate's one row");
        let debug = format!("{plan:?}");
        assert!(debug.contains("__kaveon_scalar_0"), "{debug}");
        assert!(!debug.contains("Limit"), "{debug}");
    }

    #[test]
    fn scalar_non_aggregate_positive_limit_is_bounded_to_one_row() {
        let plan = sql_to_logical_plan("SELECT (SELECT b FROM t LIMIT 1) AS answer FROM t")
            .expect("a positive scalar limit bounds a one-column relation");
        let debug = format!("{plan:?}");
        assert!(debug.contains("__kaveon_scalar_0"), "{debug}");
        assert!(debug.contains("Limit"), "{debug}");
    }

    #[test]
    fn scalar_non_aggregate_ordered_positive_limit_is_bounded_to_one_row() {
        let plan =
            sql_to_logical_plan("SELECT (SELECT b FROM t ORDER BY b LIMIT 1) AS answer FROM t")
                .expect("an ordered scalar limit remains bounded to one row");
        let debug = format!("{plan:?}");
        assert!(debug.contains("__kaveon_scalar_0"), "{debug}");
        assert!(debug.contains("Limit"), "{debug}");
    }

    #[test]
    fn scalar_non_aggregate_offset_then_positive_limit_is_bounded() {
        let plan =
            sql_to_logical_plan("SELECT (SELECT b FROM t OFFSET 1 LIMIT 1) AS answer FROM t")
                .expect("an offset followed by a positive scalar limit remains bounded");
        let debug = format!("{plan:?}");
        assert!(debug.contains("__kaveon_scalar_0"), "{debug}");
        assert!(debug.contains("Offset"), "{debug}");
        assert!(debug.contains("Limit"), "{debug}");
    }

    #[test]
    fn scalar_non_aggregate_ordered_offset_then_limit_is_bounded() {
        let plan = sql_to_logical_plan(
            "SELECT (SELECT b FROM t ORDER BY b OFFSET 1 LIMIT 1) AS answer FROM t",
        )
        .expect("an ordered offset followed by a scalar limit remains bounded");
        let debug = format!("{plan:?}");
        assert!(debug.contains("__kaveon_scalar_0"), "{debug}");
        assert!(debug.contains("Offset"), "{debug}");
        assert!(debug.contains("Limit"), "{debug}");
    }

    #[test]
    fn scalar_zero_limit_becomes_a_single_null_row() {
        let plan = sql_to_logical_plan("SELECT (SELECT b FROM t LIMIT 0) AS answer FROM t")
            .expect("an empty scalar subquery returns NULL");
        let debug = format!("{plan:?}");
        assert!(debug.contains("__kaveon_scalar_0"), "{debug}");
        assert!(!debug.contains("Limit"), "{debug}");
    }

    #[test]
    fn scalar_aggregate_positive_offset_becomes_a_single_null_row() {
        let plan = sql_to_logical_plan("SELECT (SELECT avg(b) FROM t OFFSET 1) AS answer FROM t")
            .expect("offsetting a one-row aggregate produces a NULL scalar");
        let debug = format!("{plan:?}");
        assert!(debug.contains("__kaveon_scalar_0"), "{debug}");
        assert!(!debug.contains("Offset"), "{debug}");
    }

    #[test]
    fn prepared_parameters_fail_with_an_explicit_binding_diagnostic() {
        let error = sql_to_logical_plan("SELECT * FROM t WHERE user_id = $1").unwrap_err();
        assert!(error.to_string().contains(
            "prepared parameter '$1' is not supported; bind values before submitting SQL"
        ));
    }

    #[test]
    fn binds_positional_parameters_as_typed_ast_literals() {
        let plan = sql_to_logical_plan_for_binder_with_parameters(
            "SELECT * FROM t WHERE user_id = $1 AND active = $2",
            &[ScalarValue::Int64(42), ScalarValue::Bool(true)],
        )
        .unwrap();
        let text = format!("{plan:?}");
        assert!(
            text.contains("Int64(42)"),
            "integer parameter was not bound"
        );
        assert!(
            text.contains("Bool(true)"),
            "boolean parameter was not bound"
        );
    }

    #[test]
    fn binds_array_parameters_as_typed_array_expressions() {
        let plan = sql_to_logical_plan_for_binder_with_parameter_values(
            "SELECT 2 IN UNNEST(?) FROM t",
            &[ParameterValue::Array(vec![
                ParameterValue::Scalar(ScalarValue::Int64(1)),
                ParameterValue::Scalar(ScalarValue::Int64(2)),
            ])],
            &HashMap::new(),
        )
        .unwrap();
        let debug = format!("{plan:?}");
        assert!(debug.contains("name: \"CONTAINS\""), "{debug}");
        assert!(debug.contains("name: \"ARRAY\""), "{debug}");
    }

    #[test]
    fn rejects_missing_or_binds_sequential_parameters_before_planning() {
        let missing = sql_to_logical_plan_for_binder_with_parameters(
            "SELECT * FROM t WHERE user_id = $2",
            &[ScalarValue::Int64(42)],
        )
        .unwrap_err();
        assert!(missing.to_string().contains("parameter $2 is not bound"));
        let named = sql_to_logical_plan_for_binder_with_parameters(
            "SELECT * FROM t WHERE user_id = ? AND region = ?",
            &[ScalarValue::Int64(42), ScalarValue::Utf8("EU".into())],
        )
        .unwrap();
        let text = format!("{named:?}");
        assert!(text.contains("Int64(42)"));
        assert!(text.contains("Utf8(\"EU\")"));
        let named = sql_to_logical_plan_for_binder_with_parameters(
            "SELECT * FROM t WHERE region = :region",
            &[ScalarValue::Utf8("EU".into())],
        )
        .unwrap_err();
        assert!(
            named
                .to_string()
                .contains("unsupported parameter placeholder ':region'")
        );
    }

    #[test]
    fn null_safe_comparisons_lower_to_null_total_case_expressions() {
        for (sql, expected) in [
            ("SELECT * FROM t WHERE a IS DISTINCT FROM b", "Ne"),
            ("SELECT * FROM t WHERE a IS NOT DISTINCT FROM b", "Eq"),
        ] {
            let plan = sql_to_logical_plan(sql).unwrap();
            let debug = format!("{plan:?}");
            assert!(debug.contains("Case"), "{sql} did not lower to CASE");
            assert!(debug.contains(expected), "{sql} lost its comparison");
        }
    }

    #[test]
    fn try_cast_lowers_to_a_safe_typed_expression() {
        let plan = sql_to_logical_plan("SELECT TRY_CAST(value AS BIGINT) FROM t").unwrap();
        let debug = format!("{plan:?}");
        assert!(debug.contains("TRY_CAST_INT64"));
    }

    #[test]
    fn try_lowers_cast_and_json_parse_to_safe_vectorized_forms() {
        let cast = sql_to_logical_plan("SELECT TRY(CAST(value AS BIGINT)) FROM t").unwrap();
        assert!(format!("{cast:?}").contains("TRY_CAST_INT64"));
        let json = sql_to_logical_plan("SELECT TRY(JSON_PARSE(payload)) FROM t").unwrap();
        assert!(format!("{json:?}").contains("TRY_JSON_PARSE"));
    }

    #[test]
    fn try_lowers_json_format_and_array_get() {
        let format = sql_to_logical_plan("SELECT TRY(JSON_FORMAT(payload)) FROM t").unwrap();
        assert!(format!("{format:?}").contains("TRY_JSON_FORMAT"));
        let array = sql_to_logical_plan("SELECT TRY(JSON_ARRAY_GET(payload, 0)) FROM t").unwrap();
        assert!(format!("{array:?}").contains("TRY_JSON_ARRAY_GET"));
    }

    #[test]
    fn try_rejects_unsupported_expression_with_actionable_error() {
        let error = sql_to_logical_plan("SELECT TRY(value / divisor) FROM t").unwrap_err();
        assert!(
            error.to_string().contains(
                "TRY currently supports CAST, JSON_PARSE, JSON_FORMAT, and JSON_ARRAY_GET"
            )
        );
    }

    #[test]
    fn row_constructor_is_retained_as_a_typed_expression() {
        let plan = sql_to_logical_plan("SELECT ROW(id, name) FROM t").unwrap();
        let debug = format!("{plan:?}");
        assert!(debug.contains("Function { name: \"ROW\""));
    }

    #[test]
    fn row_field_access_lowers_to_row_field() {
        let plan = sql_to_logical_plan("SELECT (ROW(id, name)).field0 FROM t").unwrap();
        let debug = format!("{plan:?}");
        assert!(debug.contains("Function { name: \"ROW_FIELD\""));
    }

    #[test]
    fn studio_bracketed_identifier_lowers_to_a_column() {
        let plan = sql_to_logical_plan("SELECT DISTINCT [year] FROM climate_energy.energy_annual")
            .unwrap();
        let debug = format!("{plan:?}");
        assert!(debug.contains("Column(\"year\")"), "{debug}");
        assert!(!debug.contains("name: \"ARRAY\""), "{debug}");
    }

    #[test]
    fn map_key_access_lowers_to_map_element_at() {
        let plan = sql_to_logical_plan("SELECT MAP(ARRAY['a'], ARRAY[1])['a'] FROM t").unwrap();
        let debug = format!("{plan:?}");
        assert!(debug.contains("Function { name: \"ELEMENT_AT\""), "{debug}");
    }

    #[test]
    fn array_subscript_lowers_to_element_at() {
        let plan = sql_to_logical_plan("SELECT ARRAY[10, 20][2] FROM t").unwrap();
        let debug = format!("{plan:?}");
        assert!(debug.contains("Function { name: \"ELEMENT_AT\""));
    }

    #[test]
    fn array_slice_lowers_to_array_slice_function() {
        let plan = sql_to_logical_plan("SELECT ARRAY[10, 20, 30][2:3] FROM t").unwrap();
        let debug = format!("{plan:?}");
        assert!(
            debug.contains("Function { name: \"ARRAY_SLICE\""),
            "{debug}"
        );
    }

    #[test]
    fn binds_named_parameters_as_typed_ast_literals() {
        let mut named = HashMap::new();
        named.insert("region".to_owned(), ScalarValue::Utf8("EU".to_owned()));
        named.insert("active".to_owned(), ScalarValue::Bool(true));
        let plan = sql_to_logical_plan_for_binder_with_parameter_bindings(
            "SELECT * FROM t WHERE region = :region AND active = :active",
            &[],
            &named,
        )
        .unwrap();
        let text = format!("{plan:?}");
        assert!(text.contains("Utf8(\"EU\")"));
        assert!(text.contains("Bool(true)"));
    }

    #[test]
    fn missing_named_parameters_fail_before_planning() {
        let mut named = HashMap::new();
        named.insert("other".to_owned(), ScalarValue::Utf8("EU".to_owned()));
        let error = sql_to_logical_plan_for_binder_with_parameter_bindings(
            "SELECT * FROM t WHERE region = :region",
            &[],
            &named,
        )
        .unwrap_err();
        assert!(error.to_string().contains("parameter :region is not bound"));
    }

    #[test]
    fn extra_positional_and_named_parameters_fail_before_planning() {
        let positional = sql_to_logical_plan_for_binder_with_parameters(
            "SELECT * FROM t WHERE id = ?",
            &[ScalarValue::Int64(1), ScalarValue::Int64(2)],
        )
        .unwrap_err();
        assert!(
            positional
                .to_string()
                .contains("received 2 positional values but the query binds 1")
        );

        let mut named = HashMap::new();
        named.insert("region".to_owned(), ScalarValue::Utf8("EU".to_owned()));
        named.insert("unused".to_owned(), ScalarValue::Utf8("x".to_owned()));
        let named = sql_to_logical_plan_for_binder_with_parameter_bindings(
            "SELECT * FROM t WHERE region = :region",
            &[],
            &named,
        )
        .unwrap_err();
        assert!(
            named
                .to_string()
                .contains("named parameter :unused was supplied but is not used")
        );
    }

    #[test]
    fn a_scalar_subquery_in_having_joins_the_aggregate_output() {
        let plan = sql_to_logical_plan(
            "SELECT k, sum(v) AS total FROM t GROUP BY k HAVING sum(v) > (SELECT sum(v) * 0.5 FROM t) AND count(*) > 1 ORDER BY total DESC",
        )
        .unwrap();
        let LogicalPlan::Sort { input, .. } = plan else {
            panic!("sort");
        };
        let LogicalPlan::Project { input, columns } = *input else {
            panic!("projection");
        };
        // Every aggregate is a named column above the join.
        assert_eq!(
            columns[1],
            Expr::Alias {
                expr: Box::new(Expr::Column("sum___kaveon_arg_0".into())),
                name: "total".into()
            }
        );
        let LogicalPlan::Filter { input, predicate } = *input else {
            panic!("HAVING filter");
        };
        assert_eq!(
            predicate,
            Expr::And(
                Box::new(Expr::BinaryOp {
                    left: Box::new(Expr::Column("sum___kaveon_arg_0".into())),
                    op: BinaryOp::Gt,
                    right: Box::new(Expr::Column("__kaveon_scalar_0".into())),
                }),
                Box::new(Expr::BinaryOp {
                    left: Box::new(Expr::Column("count_*".into())),
                    op: BinaryOp::Gt,
                    right: Box::new(Expr::Literal(ScalarValue::Int64(1))),
                }),
            )
        );
        let LogicalPlan::Join {
            left,
            right,
            join_type: JoinType::Cross,
            ..
        } = *input
        else {
            panic!("the scalar joins the aggregate output");
        };
        assert!(matches!(*left, LogicalPlan::Aggregate { .. }));
        let LogicalPlan::Project { columns, .. } = *right else {
            panic!("scalar projection");
        };
        assert!(matches!(&columns[0], Expr::Alias { name, .. } if name == "__kaveon_scalar_0"));
    }

    #[test]
    fn parses_distinct() {
        let plan = sql_to_logical_plan("SELECT DISTINCT city FROM t").unwrap();
        match plan {
            LogicalPlan::Distinct { input } => {
                assert!(matches!(*input, LogicalPlan::Project { .. }));
            }
            _ => panic!("expected Distinct"),
        }
    }

    #[test]
    fn parses_offset() {
        let plan = sql_to_logical_plan("SELECT * FROM t LIMIT 10 OFFSET 5").unwrap();
        match plan {
            LogicalPlan::Limit { input, count: 10 } => match *input {
                LogicalPlan::Offset { count: 5, .. } => {}
                _ => panic!("expected Offset under Limit"),
            },
            _ => panic!("expected Limit"),
        }
    }

    #[test]
    fn fetch_first_rows_lowers_to_limit() {
        let plan = sql_to_logical_plan("SELECT * FROM t ORDER BY id FETCH FIRST 10 ROWS ONLY")
            .expect("FETCH FIRST should lower to a bounded plan");
        let LogicalPlan::Limit { input, count: 10 } = plan else {
            panic!("expected Limit");
        };
        assert!(matches!(*input, LogicalPlan::Sort { .. }));
    }

    #[test]
    fn fetch_percent_lowers_through_windows_and_with_ties_preserves_rank_semantics() {
        let percent = sql_to_logical_plan("SELECT * FROM t FETCH FIRST 10 PERCENT ROWS ONLY")
            .expect("FETCH PERCENT should lower through a bounded window");
        let debug = format!("{percent:?}");
        assert!(debug.contains("name: \"COUNT\""), "{debug}");
        assert!(debug.contains("name: \"ROW_NUMBER\""), "{debug}");
        assert!(debug.contains("name: \"CEIL\""), "{debug}");
        assert!(debug.contains("Float64(10.0)"), "{debug}");

        let plan = sql_to_logical_plan(
            "SELECT id, score FROM t ORDER BY score DESC FETCH FIRST 2 ROWS WITH TIES",
        )
        .expect("FETCH WITH TIES should lower through rank");
        let debug = format!("{plan:?}");
        assert!(debug.contains("WindowFunction"), "{debug}");
        assert!(debug.contains("__kaveon_window_"), "{debug}");
        assert!(debug.contains("Filter"), "{debug}");
        assert!(debug.contains("Sort"), "{debug}");

        let offset = sql_to_logical_plan(
            "SELECT id FROM t ORDER BY score OFFSET 3 ROWS FETCH NEXT 2 ROWS WITH TIES",
        )
        .expect("offset with ties should be accepted");
        let debug = format!("{offset:?}");
        assert!(
            debug.contains("Int64(5)"),
            "rank boundary must include offset: {debug}"
        );
        assert!(
            debug.contains("count: 3"),
            "offset must apply after ties: {debug}"
        );
    }

    #[test]
    fn limit_all_keeps_the_input_unbounded() {
        let plan = sql_to_logical_plan("SELECT * FROM t ORDER BY id LIMIT ALL").unwrap();
        assert!(matches!(plan, LogicalPlan::Sort { .. }));
        assert!(!format!("{plan:?}").contains("Limit"));
    }

    #[test]
    fn a_limit_over_an_offset_over_a_sort_is_a_top_n() {
        let plan = sql_to_logical_plan("SELECT a FROM t ORDER BY a OFFSET 5 LIMIT 10").unwrap();
        let top_n = plan.top_n().expect("top-n shape");
        assert_eq!((top_n.skip, top_n.fetch, top_n.retained()), (5, 10, 15));
        assert!(matches!(top_n.input, LogicalPlan::Project { .. }));
        let plain = sql_to_logical_plan("SELECT a FROM t ORDER BY a LIMIT 10").unwrap();
        assert_eq!(plain.top_n().map(|t| (t.skip, t.fetch)), Some((0, 10)));
        assert!(
            sql_to_logical_plan("SELECT a FROM t LIMIT 10 OFFSET 5")
                .unwrap()
                .top_n()
                .is_none()
        );
        assert!(
            sql_to_logical_plan("SELECT a FROM t ORDER BY a OFFSET 5")
                .unwrap()
                .top_n()
                .is_none()
        );
    }

    #[test]
    fn parses_union_all() {
        let plan = sql_to_logical_plan("SELECT a FROM t1 UNION ALL SELECT b FROM t2").unwrap();
        assert!(matches!(plan, LogicalPlan::Union { all: true, .. }));
    }

    #[test]
    fn lowers_basic_grouping_sets_to_union_all_aggregates() {
        let plan = sql_to_logical_plan(
            "SELECT region, platform, COUNT(*) FROM events GROUP BY GROUPING SETS ((region), (platform))",
        )
        .unwrap();
        let debug = format!("{plan:?}");
        assert!(
            debug.contains("Union"),
            "expected UNION ALL branches: {debug}"
        );
        assert_eq!(debug.matches("Aggregate").count(), 2, "{debug}");
        assert!(debug.contains("region"), "{debug}");
        assert!(debug.contains("platform"), "{debug}");
    }

    #[test]
    fn expands_cube_and_rollup_into_union_all_aggregates() {
        for sql in [
            "SELECT region, platform, COUNT(*) FROM events GROUP BY CUBE (region, platform)",
            "SELECT region, platform, COUNT(*) FROM events GROUP BY ROLLUP (region, platform)",
        ] {
            let plan = sql_to_logical_plan(sql).unwrap();
            let debug = format!("{plan:?}");
            assert!(debug.contains("Union"), "{sql}: {debug}");
            assert!(debug.matches("Aggregate").count() >= 3, "{sql}: {debug}");
        }
    }

    #[test]
    fn grouping_sets_keep_ordinary_keys_in_every_branch() {
        let plan = sql_to_logical_plan(
            "SELECT region, platform, COUNT(*) FROM events GROUP BY region, GROUPING SETS ((platform), ())",
        )
        .unwrap();
        let debug = format!("{plan:?}");
        assert_eq!(debug.matches("Aggregate").count(), 2, "{debug}");
        assert!(debug.contains("region"), "{debug}");
        assert!(debug.contains("platform"), "{debug}");
    }

    #[test]
    fn grouping_sets_supports_the_grand_total_form() {
        let plan =
            sql_to_logical_plan("SELECT COUNT(*) FROM events GROUP BY GROUPING SETS (())").unwrap();
        let debug = format!("{plan:?}");
        assert_eq!(debug.matches("Aggregate").count(), 1, "{debug}");
    }

    #[test]
    fn grouping_sets_branch_expansion_is_bounded() {
        let sets = std::iter::repeat_n("(region)", 4097)
            .collect::<Vec<_>>()
            .join(", ");
        let sql = format!("SELECT COUNT(*) FROM events GROUP BY GROUPING SETS ({sets})");
        let error = sql_to_logical_plan(&sql).unwrap_err();
        assert!(error.to_string().contains("maximum 4096"), "{error}");
    }

    #[test]
    fn grouping_metadata_uses_branch_specific_grouping_flags() {
        let plan = sql_to_logical_plan(
            "SELECT region, GROUPING(region), COUNT(*) FROM events GROUP BY GROUPING SETS ((region), ())",
        )
        .unwrap();
        let debug = format!("{plan:?}");
        assert!(debug.contains("__kaveon_grouping_region"), "{debug}");
        assert!(!debug.contains("name: \"GROUPING\""), "{debug}");
    }

    #[test]
    fn grouping_id_combines_branch_flags_in_trino_bit_order() {
        let plan = sql_to_logical_plan(
            "SELECT GROUPING_ID(region, platform), COUNT(*) FROM events GROUP BY CUBE (region, platform)",
        )
        .unwrap();
        let debug = format!("{plan:?}");
        assert!(debug.contains("__kaveon_grouping_region"), "{debug}");
        assert!(debug.contains("__kaveon_grouping_platform"), "{debug}");
        assert!(!debug.contains("name: \"GROUPING_ID\""), "{debug}");
    }

    #[test]
    fn parses_cte() {
        let plan = sql_to_logical_plan(
            "WITH active AS (SELECT * FROM users WHERE status = 'active') SELECT * FROM active",
        )
        .unwrap();
        match plan {
            LogicalPlan::Filter { .. } => {}
            _ => panic!("expected CTE to inline as Filter (from WHERE in CTE body)"),
        }
    }

    #[test]
    fn parses_string_concat_operator() {
        let plan = sql_to_logical_plan("SELECT a || b FROM t").unwrap();
        match plan {
            LogicalPlan::Project { columns, .. } => {
                assert!(matches!(
                    columns[0],
                    Expr::BinaryOp {
                        op: BinaryOp::StringConcat,
                        ..
                    }
                ));
            }
            _ => panic!("expected Project"),
        }
    }

    #[test]
    fn parses_ilike() {
        let plan = sql_to_logical_plan("SELECT * FROM t WHERE name ILIKE '%foo%'").unwrap();
        match plan {
            LogicalPlan::Filter { predicate, .. } => {
                assert!(matches!(
                    predicate,
                    Expr::Like {
                        case_insensitive: true,
                        ..
                    }
                ));
            }
            _ => panic!("expected Filter"),
        }
    }

    #[test]
    fn parses_scalar_functions() {
        let plan = sql_to_logical_plan("SELECT UPPER(name), LENGTH(name) FROM t").unwrap();
        match plan {
            LogicalPlan::Project { columns, .. } => {
                assert_eq!(columns.len(), 2);
                assert!(matches!(columns[0], Expr::Function { ref name, .. } if name == "UPPER"));
                assert!(matches!(columns[1], Expr::Function { ref name, .. } if name == "LENGTH"));
            }
            _ => panic!("expected Project"),
        }
    }

    #[test]
    fn parses_window_frame_rows_between() {
        let plan = sql_to_logical_plan(
            "SELECT SUM(amount) OVER (ORDER BY id ROWS BETWEEN 2 PRECEDING AND CURRENT ROW) FROM t",
        )
        .unwrap();
        let LogicalPlan::Project { input, .. } = plan else {
            panic!("expected Project");
        };
        let LogicalPlan::Window { window_exprs, .. } = *input else {
            panic!("expected Window");
        };
        assert_eq!(window_exprs.len(), 1);
        let Expr::WindowFunction { frame, .. } = &window_exprs[0] else {
            panic!("expected WindowFunction");
        };
        let frame = frame.as_ref().expect("frame should be Some");
        assert_eq!(frame.units, WindowFrameUnits::Rows);
        assert_eq!(frame.start, WindowFrameBound::Preceding(2));
        assert_eq!(frame.end, WindowFrameBound::CurrentRow);
    }

    #[test]
    fn parses_window_frame_unbounded() {
        let plan = sql_to_logical_plan(
            "SELECT SUM(x) OVER (PARTITION BY g ORDER BY id ROWS BETWEEN UNBOUNDED PRECEDING AND UNBOUNDED FOLLOWING) FROM t",
        )
        .unwrap();
        let LogicalPlan::Project { input, .. } = plan else {
            panic!("expected Project");
        };
        let LogicalPlan::Window { window_exprs, .. } = *input else {
            panic!("expected Window");
        };
        let Expr::WindowFunction { frame, .. } = &window_exprs[0] else {
            panic!("expected WindowFunction");
        };
        let frame = frame.as_ref().expect("frame should be Some");
        assert_eq!(frame.start, WindowFrameBound::UnboundedPreceding);
        assert_eq!(frame.end, WindowFrameBound::UnboundedFollowing);
    }

    #[test]
    fn lowers_window_ignore_nulls_for_value_functions() {
        let plan = sql_to_logical_plan(
            "SELECT FIRST_VALUE(value) IGNORE NULLS OVER (ORDER BY id) FROM events",
        )
        .unwrap();
        let LogicalPlan::Project { input, .. } = plan else {
            panic!("expected Project");
        };
        let LogicalPlan::Window { window_exprs, .. } = *input else {
            panic!("expected Window");
        };
        assert!(matches!(
            window_exprs.as_slice(),
            [Expr::WindowFunction { name, .. }] if name == "FIRST_VALUE_IGNORE_NULLS"
        ));
    }

    #[test]
    fn parses_decimal_literal() {
        let plan = sql_to_logical_plan("SELECT 123.45 FROM t").unwrap();
        let LogicalPlan::Project { columns, .. } = plan else {
            panic!("expected Project");
        };
        match &columns[0] {
            Expr::Literal(ScalarValue::Decimal128 {
                value,
                precision,
                scale,
            }) => {
                assert_eq!(*value, 12345);
                assert_eq!(*scale, 2);
                assert!(*precision >= 5);
            }
            other => panic!("expected Decimal128 literal, got {other:?}"),
        }
    }

    #[test]
    fn expands_direct_and_chained_named_windows() {
        let plan = sql_to_logical_plan(
            "SELECT SUM(amount) OVER ordered, ROW_NUMBER() OVER partitioned FROM t WINDOW base AS (PARTITION BY region), partitioned AS (base), ordered AS (ORDER BY id ROWS BETWEEN 1 PRECEDING AND CURRENT ROW)",
        )
        .unwrap();
        let debug = format!("{plan:?}");
        assert!(
            debug.contains("partition_by: [Column(\"region\")]"),
            "{debug}"
        );
        assert!(
            debug.contains("order_by: [(Column(\"id\"), true, false)]"),
            "{debug}"
        );
        assert!(debug.contains("Rows"), "{debug}");
    }

    #[test]
    fn rejects_unknown_and_cyclic_named_windows() {
        let unknown = sql_to_logical_plan(
            "SELECT ROW_NUMBER() OVER missing FROM t WINDOW declared AS (ORDER BY id)",
        )
        .unwrap_err();
        assert!(
            unknown
                .to_string()
                .contains("unknown named window 'missing'")
        );

        let cyclic = sql_to_logical_plan(
            "SELECT ROW_NUMBER() OVER first_window FROM t WINDOW first_window AS (second_window), second_window AS (first_window)",
        )
        .unwrap_err();
        assert!(
            cyclic
                .to_string()
                .contains("cyclic named window reference involving 'first_window'")
        );
    }

    #[test]
    fn parses_cast_to_decimal() {
        let plan = sql_to_logical_plan("SELECT CAST(x AS DECIMAL(10,2)) FROM t").unwrap();
        let LogicalPlan::Project { columns, .. } = plan else {
            panic!("expected Project");
        };
        assert!(matches!(
            columns[0],
            Expr::Cast {
                data_type: CastTarget::Decimal128 {
                    precision: 10,
                    scale: 2,
                },
                ..
            }
        ));
    }

    #[test]
    fn parses_numeric_array_literal_as_an_expression() {
        let plan = sql_to_logical_plan("SELECT ARRAY[1, 2.5] FROM t").unwrap();
        let LogicalPlan::Project { columns, .. } = plan else {
            panic!("expected Project");
        };
        assert!(matches!(
            &columns[0],
            Expr::Function { name, args }
                if name == "ARRAY" && args.len() == 2
        ));
    }

    #[test]
    fn parses_cardinality_over_an_array_literal() {
        let plan = sql_to_logical_plan("SELECT CARDINALITY(ARRAY[1, 2.5]) FROM t").unwrap();
        let LogicalPlan::Project { columns, .. } = plan else {
            panic!("projection");
        };
        assert!(matches!(
            &columns[0],
            Expr::Function { name, args }
                if name == "CARDINALITY"
                    && matches!(&args[0], Expr::Function { name, args } if name == "ARRAY" && args.len() == 2)
        ));
    }

    #[test]
    fn parses_contains_over_an_array_literal() {
        let plan = sql_to_logical_plan("SELECT contains(ARRAY[1, 2], 2) FROM t").unwrap();
        let LogicalPlan::Project { columns, .. } = plan else {
            panic!("projection");
        };
        assert!(matches!(
            &columns[0],
            Expr::Function { name, args }
                if name == "CONTAINS"
                    && matches!(&args[0], Expr::Function { name, args } if name == "ARRAY" && args.len() == 2)
        ));
    }

    #[test]
    fn lowers_in_unnest_to_nullable_contains() {
        let plan = sql_to_logical_plan("SELECT 2 IN UNNEST(ARRAY[1, 2]) FROM t").unwrap();
        let LogicalPlan::Project { columns, .. } = plan else {
            panic!("projection");
        };
        assert!(matches!(
            &columns[0],
            Expr::Function { name, args }
                if name == "CONTAINS"
                    && args.len() == 2
                    && matches!(&args[0], Expr::Function { name, .. } if name == "ARRAY")
        ));

        let negated = sql_to_logical_plan("SELECT 3 NOT IN UNNEST(ARRAY[1, 2]) FROM t").unwrap();
        let LogicalPlan::Project { columns, .. } = negated else {
            panic!("projection");
        };
        assert!(matches!(&columns[0], Expr::Not(inner) if matches!(
            inner.as_ref(),
            Expr::Function { name, .. } if name == "CONTAINS"
        )));
    }

    #[test]
    fn rejects_row_producing_unnest_with_an_actionable_diagnostic() {
        for (sql, expected) in [
            ("SELECT * FROM UNNEST(1, 2)", "requires an ARRAY expression"),
            (
                "SELECT * FROM UNNEST(values) AS t(value)",
                "correlated UNNEST requires CROSS JOIN LATERAL",
            ),
        ] {
            let error = sql_to_logical_plan(sql).unwrap_err();
            assert!(error.to_string().contains(expected), "{error}");
        }
    }

    #[test]
    fn lowers_supported_quantified_array_comparisons() {
        for (sql, negated) in [
            ("SELECT 2 = ANY(ARRAY[1, 2]) FROM t", false),
            ("SELECT 3 <> ALL(ARRAY[1, 2]) FROM t", true),
        ] {
            let plan = sql_to_logical_plan(sql).unwrap();
            let LogicalPlan::Project { columns, .. } = plan else {
                panic!("projection");
            };
            let expression = if negated {
                match &columns[0] {
                    Expr::Not(inner) => inner.as_ref(),
                    other => panic!("expected NOT, got {other:?}"),
                }
            } else {
                &columns[0]
            };
            assert!(matches!(expression, Expr::Function { name, .. } if name == "CONTAINS"));
        }
        for (sql, expected) in [
            ("SELECT 2 > ANY(ARRAY[1, 2]) FROM t", "ARRAY_ANY_GT"),
            ("SELECT 2 <= ANY(ARRAY[1, 2]) FROM t", "ARRAY_ANY_LE"),
            ("SELECT 2 < ALL(ARRAY[1, 2]) FROM t", "ARRAY_ALL_LT"),
            ("SELECT 2 >= ALL(ARRAY[1, 2]) FROM t", "ARRAY_ALL_GE"),
            ("SELECT 2 <> ANY(ARRAY[1, 2]) FROM t", "ARRAY_ANY_NE"),
        ] {
            let plan = sql_to_logical_plan(sql).unwrap();
            let LogicalPlan::Project { columns, .. } = plan else {
                panic!("projection");
            };
            assert!(matches!(&columns[0], Expr::Function { name, .. } if name == expected));
        }
    }

    #[test]
    fn parses_array_position_over_an_array_literal() {
        let plan =
            sql_to_logical_plan("SELECT array_position(ARRAY['red', 'blue'], 'blue') FROM t")
                .unwrap();
        let LogicalPlan::Project { columns, .. } = plan else {
            panic!("projection");
        };
        assert!(matches!(
            &columns[0],
            Expr::Function { name, args }
                if name == "ARRAY_POSITION"
                    && args.len() == 2
                    && matches!(&args[0], Expr::Function { name, args } if name == "ARRAY" && args.len() == 2)
        ));
    }

    #[test]
    fn map_constructor_is_retained_as_a_typed_expression() {
        let plan = sql_to_logical_plan("SELECT MAP(ARRAY['a'], ARRAY[1]) FROM t").unwrap();
        let debug = format!("{plan:?}");
        assert!(debug.contains("Function { name: \"MAP\""));
    }

    #[test]
    fn rejects_invalid_decimal_precision_and_scale() {
        for (sql, expected) in [
            (
                "SELECT CAST(x AS DECIMAL(39,2)) FROM t",
                "DECIMAL precision must be between 1 and 38",
            ),
            (
                "SELECT CAST(x AS DECIMAL(8,9)) FROM t",
                "DECIMAL scale must be between 0 and precision",
            ),
        ] {
            let error = sql_to_logical_plan(sql).unwrap_err();
            assert!(error.to_string().contains(expected), "{sql}: {error}");
        }
    }

    #[test]
    fn casts_json_as_utf8_for_json_functions() {
        let plan = sql_to_logical_plan("SELECT CAST(payload AS JSON) FROM events").unwrap();
        let debug = format!("{plan:?}");
        assert!(debug.contains("JSON_PARSE"), "{debug}");

        let plan = sql_to_logical_plan("SELECT TRY_CAST(payload AS JSONB) FROM events").unwrap();
        let debug = format!("{plan:?}");
        assert!(debug.contains("TRY_JSON_PARSE"), "{debug}");
    }

    #[test]
    fn parses_trino_typed_json_literal() {
        let plan = sql_to_logical_plan("SELECT JSON '{\"a\": 7}'").unwrap();
        let debug = format!("{plan:?}");
        assert!(debug.contains("Function { name: \"JSON_PARSE\""), "{debug}");
    }

    #[test]
    fn qualifies_after_window_evaluation() {
        let plan = sql_to_logical_plan(
            "SELECT row_number() OVER (PARTITION BY region ORDER BY user_id) AS rn FROM events QUALIFY rn = 1",
        )
        .unwrap();
        let LogicalPlan::Project { input, .. } = plan else {
            panic!("expected final projection");
        };
        let LogicalPlan::Filter { input, .. } = *input else {
            panic!("expected QUALIFY filter");
        };
        assert!(matches!(*input, LogicalPlan::Window { .. }));
    }

    #[test]
    fn parses_in_subquery() {
        let plan = sql_to_logical_plan(
            "SELECT * FROM orders WHERE user_id IN (SELECT id FROM users WHERE active = true)",
        )
        .unwrap();
        assert!(matches!(plan, LogicalPlan::SemiJoin { .. }));
    }

    #[test]
    fn parses_not_in_subquery() {
        let plan =
            sql_to_logical_plan("SELECT * FROM orders WHERE user_id NOT IN (SELECT id FROM users)")
                .unwrap();
        assert!(matches!(plan, LogicalPlan::AntiJoin { .. }));
    }

    #[test]
    fn parses_exists_subquery() {
        let plan = sql_to_logical_plan(
            "SELECT * FROM orders WHERE EXISTS (SELECT NULL FROM users WHERE users.id = 1)",
        )
        .unwrap();
        assert!(matches!(plan, LogicalPlan::SemiJoin { .. }));
    }

    #[test]
    fn rejects_correlated_subqueries_instead_of_rebinding_outer_columns() {
        for sql in [
            "SELECT * FROM orders WHERE EXISTS (SELECT 1 FROM users WHERE users.id = orders.user_id)",
            "SELECT * FROM orders o WHERE o.id IN (SELECT u.id FROM users u WHERE u.id = o.id)",
            "SELECT * FROM orders o WHERE o.total > (SELECT avg(u.score) FROM users u WHERE u.id = o.id)",
            "SELECT (SELECT avg(u.score) FROM users u WHERE u.id = o.id) FROM orders o",
        ] {
            assert!(
                sql_to_logical_plan(sql)
                    .unwrap_err()
                    .to_string()
                    .contains("correlated")
            );
        }
    }

    #[test]
    fn correlated_scalar_subqueries_are_rejected_with_explicit_scope_diagnostic() {
        let sql = "SELECT o.id FROM orders o WHERE o.total > (SELECT AVG(u.score) FROM users u WHERE u.id = o.id)";
        let error = sql_to_logical_plan(sql).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("correlated subqueries are unsupported"),
            "unexpected diagnostic: {error}"
        );

        // The binder lowering entry point deliberately preserves the outer
        // reference in the subquery filter. This is only a lowering contract;
        // an Apply/lateral execution operator is still required before this
        // shape can be admitted to runtime execution.
        let binder_plan = sql_to_logical_plan_for_binder(sql).unwrap();
        let text = format!("{binder_plan:?}");
        assert!(text.contains("Column(\"o.id\")"), "correlation was lost");
        assert!(text.contains("Column(\"u.id\")"), "inner key was lost");
    }

    /// Lowered for the binder, the same statements keep the outer
    /// reference in the subquery's own filter for the binder to lift; the
    /// join carries no residual until the binder sets one.
    #[test]
    fn lowers_correlated_subqueries_for_the_binder_with_the_reference_in_place() {
        let plan = sql_to_logical_plan_for_binder(
            "SELECT user_id FROM orders WHERE EXISTS (SELECT 1 FROM users WHERE users.id = orders.user_id)",
        )
        .unwrap();
        let LogicalPlan::Project { input, .. } = plan else {
            panic!("projection")
        };
        let LogicalPlan::SemiJoin {
            right, residual, ..
        } = *input
        else {
            panic!("semi join")
        };
        assert!(residual.is_none());
        let LogicalPlan::Project { input, .. } = *right else {
            panic!("subquery projection")
        };
        let LogicalPlan::Filter { predicate, .. } = *input else {
            panic!("the subquery keeps its filter")
        };
        assert_eq!(
            predicate,
            Expr::BinaryOp {
                left: Box::new(Expr::Column("users.id".into())),
                op: BinaryOp::Eq,
                right: Box::new(Expr::Column("orders.user_id".into())),
            }
        );
        assert!(matches!(
            sql_to_logical_plan_for_binder(
                "SELECT * FROM orders o WHERE o.id IN (SELECT u.id FROM users u WHERE u.id = o.id)"
            ),
            Ok(LogicalPlan::SemiJoin { .. })
        ));
        assert!(matches!(
            sql_to_logical_plan_for_binder(
                "SELECT * FROM orders o WHERE o.total > (SELECT avg(u.score) FROM users u WHERE u.id = o.id)"
            ),
            Ok(LogicalPlan::Filter { .. })
        ));
    }

    #[test]
    fn lowers_distinct_window_aggregates() {
        let plan = sql_to_logical_plan("SELECT COUNT(DISTINCT x) OVER () FROM t").unwrap();
        let text = format!("{plan:?}");
        assert!(text.contains("COUNT_DISTINCT"), "{text}");
    }

    #[test]
    fn window_order_preserves_explicit_nulls_first() {
        let plan = sql_to_logical_plan("SELECT RANK() OVER (ORDER BY x NULLS FIRST) FROM t")
            .expect("window NULLS FIRST is supported");
        let text = format!("{plan:?}");
        assert!(text.contains("true"), "{text}");
    }

    #[test]
    fn parses_not_exists_subquery() {
        let plan =
            sql_to_logical_plan("SELECT * FROM orders WHERE NOT EXISTS (SELECT 1 FROM users)")
                .unwrap();
        assert!(matches!(plan, LogicalPlan::AntiJoin { .. }));
    }

    #[test]
    fn parses_sum_distinct() {
        let plan = sql_to_logical_plan("SELECT SUM(DISTINCT amount) FROM t").unwrap();
        let LogicalPlan::Project { input, .. } = plan else {
            panic!("expected project")
        };
        let LogicalPlan::Aggregate { aggregates, .. } = *input else {
            panic!("expected aggregate")
        };
        assert!(matches!(
            aggregates.as_slice(),
            [AggregateExpr::Sum { distinct: true, .. }]
        ));
    }

    #[test]
    fn parses_avg_distinct() {
        let plan = sql_to_logical_plan("SELECT AVG(DISTINCT score) FROM t").unwrap();
        let LogicalPlan::Project { input, .. } = plan else {
            panic!("expected project")
        };
        let LogicalPlan::Aggregate { aggregates, .. } = *input else {
            panic!("expected aggregate")
        };
        assert!(matches!(
            aggregates.as_slice(),
            [AggregateExpr::Avg { distinct: true, .. }]
        ));
    }

    #[test]
    fn parses_distinct_extrema_as_regular_extrema() {
        for sql in [
            "SELECT MIN(DISTINCT amount) FROM t",
            "SELECT MAX(DISTINCT amount) FROM t",
        ] {
            sql_to_logical_plan(sql).expect("DISTINCT extrema are Trino-compatible");
        }
    }

    #[test]
    fn lowers_aggregate_filter_to_null_preserving_case() {
        let (_, aggregates) = aggregates_of(
            "SELECT COUNT(*) FILTER (WHERE active), SUM(amount) FILTER (WHERE amount > 0) FROM events",
        );
        assert_eq!(aggregates.len(), 2);
        let plan = sql_to_logical_plan(
            "SELECT COUNT(*) FILTER (WHERE active), SUM(amount) FILTER (WHERE amount > 0) FROM events",
        )
        .unwrap();
        assert!(format!("{plan:?}").contains("Case {"));
    }

    #[test]
    fn lowers_window_aggregate_filter_to_null_preserving_case() {
        let plan = sql_to_logical_plan(
            "SELECT SUM(amount) FILTER (WHERE amount > 0) OVER (PARTITION BY region) FROM events",
        )
        .unwrap();
        let debug = format!("{plan:?}");
        assert!(debug.contains("WindowFunction"), "{debug}");
        assert!(debug.contains("Case {"), "{debug}");
        assert!(!debug.contains("FILTER is unsupported"), "{debug}");
    }

    #[test]
    fn lowers_approximate_aggregate_filters_to_null_preserving_case() {
        let plan = sql_to_logical_plan(
            "SELECT APPROX_COUNT_DISTINCT(user_id) FILTER (WHERE active), \
                    APPROX_PERCENTILE(latency, 0.95) FILTER (WHERE status = 'ok') \
             FROM events",
        )
        .unwrap();
        let debug = format!("{plan:?}");
        assert!(debug.matches("Case {").count() >= 2, "{debug}");
        assert!(!debug.contains("FILTER is unsupported"), "{debug}");
    }

    #[test]
    fn keeps_column_statistics_filter_explicitly_unsupported() {
        let error = sql_to_logical_plan(
            "SELECT COLUMN_STATISTICS(value) FILTER (WHERE active) FROM events",
        )
        .unwrap_err()
        .to_string();
        assert_eq!(error, "sql: FILTER is unsupported for COLUMN_STATISTICS");
    }

    #[test]
    fn lowers_count_if_to_the_existing_filtered_count_state() {
        let (columns, aggregates) = aggregates_of(
            "SELECT region, COUNT_IF(actions > 5) AS active FROM events GROUP BY region",
        );
        assert!(matches!(
            aggregates.as_slice(),
            [AggregateExpr::Count {
                expr: Expr::Column(name),
                distinct: false
            }] if name == "__kaveon_arg_0"
        ));
        assert!(format!("{:?}", columns[1]).contains("count___kaveon_arg_0"));
        assert!(
            sql_to_logical_plan(
                "SELECT COUNT_IF(actions > 5) OVER (PARTITION BY region) FROM events"
            )
            .is_ok()
        );
        let filtered = sql_to_logical_plan(
            "SELECT COUNT_IF(actions > 5) FILTER (WHERE region = 'EU') FROM events",
        )
        .unwrap();
        assert!(format!("{filtered:?}").contains("And("));
    }

    #[test]
    fn count_if_requires_one_predicate() {
        for sql in [
            "SELECT COUNT_IF() FROM events",
            "SELECT COUNT_IF(actions > 5, errors = 0) FROM events",
            "SELECT COUNT_IF(*) FROM events",
        ] {
            let error = sql_to_logical_plan(sql).unwrap_err().to_string();
            assert!(!error.is_empty(), "{sql}");
        }
    }

    fn aggregates_of(sql: &str) -> (Vec<Expr>, Vec<AggregateExpr>) {
        let plan = sql_to_logical_plan(sql).unwrap();
        let LogicalPlan::Project { input, columns } = plan else {
            panic!("expected Project");
        };
        let LogicalPlan::Aggregate { aggregates, .. } = *input else {
            panic!("expected Aggregate under Project");
        };
        (columns, aggregates)
    }

    #[test]
    fn parses_approximate_aggregates_and_names_their_outputs() {
        let (columns, aggregates) = aggregates_of(
            "SELECT APPROX_COUNT_DISTINCT(user_id), approx_distinct(country) AS c, \
             APPROX_PERCENTILE(latency, 0.5), APPROX_PERCENTILE(latency, ARRAY[0.9, 0.99]) \
             FROM events",
        );
        assert_eq!(
            aggregates,
            vec![
                AggregateExpr::ApproxDistinct {
                    expr: Expr::Column("user_id".into()),
                    as_count: false,
                },
                AggregateExpr::ApproxDistinct {
                    expr: Expr::Column("country".into()),
                    as_count: false,
                },
                AggregateExpr::ApproxPercentile {
                    expr: Expr::Column("latency".into()),
                    percentiles: Percentiles {
                        fractions: vec![0.5],
                        list: false
                    }
                },
                AggregateExpr::ApproxPercentile {
                    expr: Expr::Column("latency".into()),
                    percentiles: Percentiles {
                        fractions: vec![0.9, 0.99],
                        list: true
                    }
                },
            ]
        );
        assert!(aggregates.iter().all(AggregateExpr::is_approximate));
        // The projection carries the calls under their canonical names, so
        // the planners bind them to the same output columns the aggregate
        // names itself by.
        let names = aggregates
            .iter()
            .map(AggregateExpr::output_name)
            .collect::<Vec<_>>();
        assert_eq!(
            names,
            [
                "approx_count_distinct_user_id",
                "approx_count_distinct_country",
                "approx_percentile_latency, 0.5",
                "approx_percentile_latency, array[0.9, 0.99]",
            ]
        );
        for (column, aggregate) in columns.iter().zip(&aggregates) {
            let call = match column {
                Expr::Alias { expr, .. } => expr.as_ref(),
                other => other,
            };
            let Expr::Function { name, args } = call else {
                panic!("projection carries the call: {column:?}");
            };
            assert_eq!(
                kaveon_core::aggregate_output_name(name, args),
                aggregate.output_name()
            );
        }
    }

    #[test]
    fn approximate_aggregates_over_expressions_lower_like_exact_ones() {
        let (columns, aggregates) =
            aggregates_of("SELECT APPROX_PERCENTILE(latency * 2, 0.5) + 1 FROM events");
        assert!(matches!(
            &aggregates[0],
            AggregateExpr::ApproxPercentile { expr: Expr::Column(name), .. } if name == "__kaveon_arg_0"
        ));
        assert_eq!(
            columns[0],
            Expr::BinaryOp {
                left: Box::new(Expr::Column("approx_percentile___kaveon_arg_0, 0.5".into())),
                op: BinaryOp::Plus,
                right: Box::new(Expr::Literal(ScalarValue::Int64(1))),
            }
        );
    }

    #[test]
    fn approximate_aggregates_refuse_what_they_cannot_answer() {
        for (sql, message) in [
            (
                "SELECT APPROX_COUNT_DISTINCT(*) FROM events",
                "requires a column argument",
            ),
            (
                "SELECT APPROX_COUNT_DISTINCT(DISTINCT user_id) FROM events",
                "DISTINCT is not supported",
            ),
            (
                "SELECT APPROX_PERCENTILE(latency) FROM events",
                "one percentile",
            ),
            (
                "SELECT APPROX_PERCENTILE(latency, 1.5) FROM events",
                "not between 0 and 1",
            ),
            (
                "SELECT APPROX_PERCENTILE(latency, ARRAY[]) FROM events",
                "at least one percentile",
            ),
            (
                "SELECT APPROX_PERCENTILE(latency, p) FROM events",
                "numeric constants",
            ),
        ] {
            let error = sql_to_logical_plan(sql).unwrap_err().to_string();
            assert!(error.contains(message), "{sql}: {error}");
        }
    }

    #[test]
    fn the_approximate_setting_turns_exact_distinct_counts_into_sketches() {
        let sql = "SELECT COUNT(DISTINCT user_id), COUNT(user_id), SUM(DISTINCT n) FROM events";
        let LogicalPlan::Project { input, .. } =
            sql_to_logical_plan_for_binder_approximate(sql).unwrap()
        else {
            panic!("expected Project");
        };
        let LogicalPlan::Aggregate {
            aggregates: approximated,
            ..
        } = *input
        else {
            panic!("expected Aggregate under Project");
        };
        // Without the setting the same statement keeps its exact count.
        assert!(matches!(
            aggregates_of(sql).1[0],
            AggregateExpr::Count { distinct: true, .. }
        ));
        // A lone COUNT(DISTINCT) is otherwise a DISTINCT stage; the
        // setting makes it a sketch instead.
        let LogicalPlan::Project { input, .. } = sql_to_logical_plan_for_binder_approximate(
            "SELECT COUNT(DISTINCT user_id) FROM events",
        )
        .unwrap() else {
            panic!("expected Project");
        };
        assert!(matches!(
            *input,
            LogicalPlan::Aggregate { ref aggregates, ref input, .. }
                if matches!(aggregates[0], AggregateExpr::ApproxDistinct { as_count: true, .. })
                    && matches!(**input, LogicalPlan::Scan { .. })
        ));
        assert_eq!(
            approximated[0].output_name(),
            "count_user_id",
            "the output keeps COUNT's name so the projection binds"
        );
        assert_eq!(
            approximated,
            vec![
                AggregateExpr::ApproxDistinct {
                    expr: Expr::Column("user_id".into()),
                    as_count: true,
                },
                AggregateExpr::Count {
                    expr: Expr::Column("user_id".into()),
                    distinct: false
                },
                AggregateExpr::Sum {
                    expr: Expr::Column("n".into()),
                    distinct: true
                },
            ]
        );
    }

    #[test]
    fn scalar_select_without_from_uses_one_implicit_row() {
        let plan = sql_to_logical_plan("SELECT CAST(1 AS BIGINT) AS value").unwrap();
        let LogicalPlan::Project { input, columns } = plan else {
            panic!("expected projection");
        };
        assert_eq!(columns.len(), 1);
        assert!(matches!(
            *input,
            LogicalPlan::Unnest { ref columns, .. } if columns == &["_kaveon_unit"]
        ));
    }

    #[test]
    fn scalar_select_set_operations_have_a_row_per_branch() {
        let plan = sql_to_logical_plan("SELECT 1 UNION ALL SELECT 2").unwrap();
        let LogicalPlan::Union { inputs, all } = plan else {
            panic!("expected union");
        };
        assert!(all);
        assert_eq!(inputs.len(), 2);
        assert!(inputs.iter().all(|input| matches!(
            input,
            LogicalPlan::Project { input, .. }
                if matches!(input.as_ref(), LogicalPlan::Unnest { .. })
        )));
    }

    #[test]
    fn values_relation_lowers_to_columnar_unnest() {
        let plan = sql_to_logical_plan("VALUES (1, 'a'), (2, 'b')").unwrap();
        let LogicalPlan::Unnest {
            expressions,
            columns,
            with_ordinality,
            ..
        } = plan
        else {
            panic!("expected values unnest");
        };
        assert_eq!(columns, ["column1", "column2"]);
        assert!(!with_ordinality);
        assert_eq!(expressions.len(), 2);
        assert!(expressions.iter().all(|expr| matches!(
            expr,
            Expr::Function { name, args } if name == "ARRAY" && args.len() == 2
        )));
    }

    #[test]
    fn derived_values_honor_relation_column_aliases() {
        let plan = sql_to_logical_plan(
            "SELECT t.id, t.label FROM (VALUES (1, 'a'), (2, 'b')) AS t(id, label)",
        )
        .unwrap();
        let LogicalPlan::Project { input, .. } = plan else {
            panic!("expected projection");
        };
        assert!(matches!(
            input.as_ref(),
            LogicalPlan::Unnest { columns, .. }
                if columns == &["id".to_owned(), "label".to_owned()]
        ));
    }
}
