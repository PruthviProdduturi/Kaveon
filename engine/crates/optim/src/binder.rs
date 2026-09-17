//! Binding: the pass that gives the logical plan what the SQL text left
//! implicit — which relation each bare column belongs to.
//!
//! The SQL layer lowers `FROM customer, orders WHERE c_custkey = o_custkey`
//! to a cross join under a filter, because without the catalog it cannot
//! tell which side `c_custkey` lives on. With the catalog every scan has a
//! scope (its columns, under its alias or table name), and every join's
//! scope is its inputs' scopes qualified the way the join operator names
//! its output (`customer.c_custkey`). Against those scopes this pass:
//!
//! - routes the conjuncts of a filter over a join tree: a conjunct on one
//!   side moves to that side, an equality between the two sides becomes a
//!   hash-join key (the cross join becomes an inner join), and the rest
//!   stays above the join;
//! - qualifies bare column references under a join with their relation,
//!   so the qualifier-driven passes (filter pushdown, projection pruning,
//!   build-side choice) and the join operator's output names agree, while
//!   a projected column keeps the name it was written with.
//!
//! Above a single scan, a projection or an aggregate nothing is rewritten:
//! the operators resolve bare names against their inputs themselves. A
//! table the catalog cannot describe leaves its references as written for
//! the planner to report.
use kaveon_core::{BinaryOp, CatalogManager, Expr, KaveonError, Result, TableReference};
use kaveon_sql::logical_plan::{AggregateExpr, JoinType, LogicalPlan};

/// Bind `plan` against `catalog`. Table names must already be qualified.
pub fn bind(plan: LogicalPlan, catalog: &CatalogManager) -> Result<LogicalPlan> {
    Binder { catalog }.bind(plan).map(|bound| bound.plan)
}

/// One output column of a plan node: how a query may refer to it, and the
/// name it carries in the operator's output.
#[derive(Clone, Debug, PartialEq)]
struct Column {
    qualifier: Option<String>,
    name: String,
    physical: String,
}

/// The output columns of a plan node.
#[derive(Clone, Debug, Default)]
struct Scope {
    columns: Vec<Column>,
    /// The output of a join: bare references are rewritten to their
    /// physical (qualified) names so the qualifier-driven passes see them.
    joined: bool,
}

enum Resolution {
    Unresolved,
    Unique(String),
    Ambiguous,
}

impl Scope {
    fn resolve(&self, reference: &str) -> Resolution {
        let mut physical: Vec<&str> = match reference.rsplit_once('.') {
            Some((qualifier, name)) => self
                .columns
                .iter()
                .filter(|column| {
                    column.qualifier.as_deref() == Some(qualifier) && column.name == name
                })
                .map(|column| column.physical.as_str())
                .collect(),
            None => self
                .columns
                .iter()
                .filter(|column| column.name == reference)
                .map(|column| column.physical.as_str())
                .collect(),
        };
        physical.sort_unstable();
        physical.dedup();
        match physical.as_slice() {
            [] => Resolution::Unresolved,
            [one] => Resolution::Unique((*one).to_owned()),
            _ => Resolution::Ambiguous,
        }
    }

    fn holds(&self, reference: &str) -> bool {
        matches!(self.resolve(reference), Resolution::Unique(_))
    }

    /// The columns as a join names them: qualified with `qualifier` when
    /// the input is a relation the join qualifies.
    fn qualified_by(mut self, qualifier: Option<&str>) -> Self {
        if let Some(qualifier) = qualifier {
            for column in &mut self.columns {
                column.physical = format!("{qualifier}.{}", column.physical);
            }
        }
        self
    }
}

struct Bound {
    plan: LogicalPlan,
    /// The input scope of the aggregate this node sits on, so aggregate
    /// function references above it (`SUM(l_quantity)` in the projection
    /// or HAVING) bind the way the aggregate bound its arguments.
    aggregate_input: Option<Scope>,
}

/// Where a conjunct of a filter over a join belongs.
enum Placement {
    Left,
    Right,
    Key(String, String),
    Above,
}

struct Binder<'a> {
    catalog: &'a CatalogManager,
}

impl Binder<'_> {
    fn bind(&self, plan: LogicalPlan) -> Result<Bound> {
        match plan {
            LogicalPlan::Scan { .. } => Ok(Bound {
                plan,
                aggregate_input: None,
            }),
            LogicalPlan::Filter { input, predicate } => {
                let input = self.bind(*input)?;
                let (plan, residual) = self.route(conjuncts(predicate), input.plan)?;
                let scope = self.scope_of(&plan);
                let plan = self.filtered(plan, residual, &scope, input.aggregate_input.as_ref())?;
                Ok(Bound {
                    plan,
                    aggregate_input: input.aggregate_input,
                })
            }
            LogicalPlan::Project { input, columns } => {
                let input = self.bind(*input)?;
                let scope = self.scope_of(&input.plan);
                let columns = columns
                    .into_iter()
                    .map(|column| {
                        self.bind_projected(column, &scope, input.aggregate_input.as_ref())
                    })
                    .collect::<Result<_>>()?;
                Ok(Bound {
                    plan: LogicalPlan::Project {
                        input: Box::new(input.plan),
                        columns,
                    },
                    aggregate_input: None,
                })
            }
            LogicalPlan::Aggregate {
                input,
                group_by,
                aggregates,
            } => {
                let input = self.bind(*input)?;
                let scope = self.scope_of(&input.plan);
                let group_by = group_by
                    .into_iter()
                    .map(|key| self.bind_expr(key, &scope, None))
                    .collect::<Result<_>>()?;
                let aggregates = aggregates
                    .into_iter()
                    .map(|aggregate| self.bind_aggregate(aggregate, &scope))
                    .collect::<Result<_>>()?;
                Ok(Bound {
                    plan: LogicalPlan::Aggregate {
                        input: Box::new(input.plan),
                        group_by,
                        aggregates,
                    },
                    aggregate_input: Some(scope),
                })
            }
            LogicalPlan::Sort { input, order_by } => {
                let input = self.bind(*input)?;
                let scope = self.scope_of(&input.plan);
                let order_by = order_by
                    .into_iter()
                    .map(|(key, ascending)| {
                        self.bind_expr(key, &scope, input.aggregate_input.as_ref())
                            .map(|key| (key, ascending))
                    })
                    .collect::<Result<_>>()?;
                Ok(Bound {
                    plan: LogicalPlan::Sort {
                        input: Box::new(input.plan),
                        order_by,
                    },
                    aggregate_input: input.aggregate_input,
                })
            }
            LogicalPlan::Limit { input, count } => {
                let input = self.bind(*input)?;
                Ok(Bound {
                    plan: LogicalPlan::Limit {
                        input: Box::new(input.plan),
                        count,
                    },
                    aggregate_input: input.aggregate_input,
                })
            }
            LogicalPlan::Offset { input, count } => {
                let input = self.bind(*input)?;
                Ok(Bound {
                    plan: LogicalPlan::Offset {
                        input: Box::new(input.plan),
                        count,
                    },
                    aggregate_input: input.aggregate_input,
                })
            }
            LogicalPlan::Distinct { input } => {
                let input = self.bind(*input)?;
                Ok(Bound {
                    plan: LogicalPlan::Distinct {
                        input: Box::new(input.plan),
                    },
                    aggregate_input: input.aggregate_input,
                })
            }
            LogicalPlan::Window {
                input,
                window_exprs,
            } => {
                let input = self.bind(*input)?;
                let scope = self.scope_of(&input.plan);
                let window_exprs = window_exprs
                    .into_iter()
                    .map(|expr| self.bind_expr(expr, &scope, input.aggregate_input.as_ref()))
                    .collect::<Result<_>>()?;
                Ok(Bound {
                    plan: LogicalPlan::Window {
                        input: Box::new(input.plan),
                        window_exprs,
                    },
                    aggregate_input: input.aggregate_input,
                })
            }
            LogicalPlan::Union { inputs, all } => Ok(Bound {
                plan: LogicalPlan::Union {
                    inputs: inputs
                        .into_iter()
                        .map(|input| self.bind(input).map(|bound| bound.plan))
                        .collect::<Result<_>>()?,
                    all,
                },
                aggregate_input: None,
            }),
            LogicalPlan::Intersect { left, right } => Ok(Bound {
                plan: LogicalPlan::Intersect {
                    left: Box::new(self.bind(*left)?.plan),
                    right: Box::new(self.bind(*right)?.plan),
                },
                aggregate_input: None,
            }),
            LogicalPlan::Except { left, right } => Ok(Bound {
                plan: LogicalPlan::Except {
                    left: Box::new(self.bind(*left)?.plan),
                    right: Box::new(self.bind(*right)?.plan),
                },
                aggregate_input: None,
            }),
            LogicalPlan::Join {
                left,
                right,
                join_type,
                condition,
                distribution,
            } => {
                let left = self.bind(*left)?.plan;
                let right = self.bind(*right)?.plan;
                let Some(condition) = condition else {
                    return Ok(Bound {
                        plan: LogicalPlan::Join {
                            left: Box::new(left),
                            right: Box::new(right),
                            join_type,
                            condition: None,
                            distribution,
                        },
                        aggregate_input: None,
                    });
                };
                self.bind_on(left, right, join_type, condition, distribution)
            }
            LogicalPlan::SemiJoin {
                left,
                right,
                left_key,
                right_key,
            } => {
                let (left, right, left_key, right_key) =
                    self.bind_semi_join(*left, *right, left_key, right_key)?;
                Ok(Bound {
                    plan: LogicalPlan::SemiJoin {
                        left,
                        right,
                        left_key,
                        right_key,
                    },
                    aggregate_input: None,
                })
            }
            LogicalPlan::AntiJoin {
                left,
                right,
                left_key,
                right_key,
            } => {
                let (left, right, left_key, right_key) =
                    self.bind_semi_join(*left, *right, left_key, right_key)?;
                Ok(Bound {
                    plan: LogicalPlan::AntiJoin {
                        left,
                        right,
                        left_key,
                        right_key,
                    },
                    aggregate_input: None,
                })
            }
        }
    }

    /// An explicit ON condition: equalities between the two sides are the
    /// hash-join keys. A conjunct on one side alone filters that side
    /// before the join when the join can drop that side's rows (either
    /// side of an inner join, the non-preserved side of an outer join): a
    /// right row failing `o_comment NOT LIKE ...` matches no left row
    /// under a LEFT JOIN, so filtering it away first is the same join. An
    /// inner join filters anything else above itself; an outer join has no
    /// residual and refuses it.
    fn bind_on(
        &self,
        left: LogicalPlan,
        right: LogicalPlan,
        join_type: JoinType,
        condition: Expr,
        distribution: kaveon_sql::logical_plan::JoinDistribution,
    ) -> Result<Bound> {
        let left_scope = self.scope_of(&left).qualified_by(relation_qualifier(&left));
        let right_scope = self
            .scope_of(&right)
            .qualified_by(relation_qualifier(&right));
        let (into_left, into_right) = match join_type {
            JoinType::Inner | JoinType::Cross => (true, true),
            JoinType::Left => (false, true),
            JoinType::Right => (true, false),
            JoinType::Full => (false, false),
        };
        let mut left_parts = Vec::new();
        let mut right_parts = Vec::new();
        let mut keys = Vec::new();
        let mut above = Vec::new();
        for conjunct in conjuncts(condition) {
            match placement(&conjunct, &left_scope, &right_scope) {
                Placement::Key(left, right) => keys.push(Expr::BinaryOp {
                    left: Box::new(Expr::Column(left)),
                    op: BinaryOp::Eq,
                    right: Box::new(Expr::Column(right)),
                }),
                Placement::Left if into_left => left_parts.push(conjunct),
                Placement::Right if into_right => right_parts.push(conjunct),
                _ => above.push(conjunct),
            }
        }
        if !above.is_empty() && !matches!(join_type, JoinType::Inner | JoinType::Cross) {
            return Err(KaveonError::Sql(format!(
                "{} JOIN conditions other than equalities between the two sides are not supported: {:?}",
                match join_type {
                    JoinType::Left => "LEFT",
                    JoinType::Right => "RIGHT",
                    _ => "FULL",
                },
                above[0]
            )));
        }
        let left = self.place(left_parts, left)?;
        let right = self.place(right_parts, right)?;
        let join_type = if keys.is_empty() {
            join_type
        } else if join_type == JoinType::Cross {
            JoinType::Inner
        } else {
            join_type
        };
        let plan = LogicalPlan::Join {
            left: Box::new(left),
            right: Box::new(right),
            join_type,
            condition: conjoin(keys),
            distribution,
        };
        let scope = self.scope_of(&plan);
        let plan = self.filtered(plan, above, &scope, None)?;
        Ok(Bound {
            plan,
            aggregate_input: None,
        })
    }

    fn bind_semi_join(
        &self,
        left: LogicalPlan,
        right: LogicalPlan,
        left_key: Expr,
        right_key: Expr,
    ) -> Result<(Box<LogicalPlan>, Box<LogicalPlan>, Expr, Expr)> {
        let left = self.bind(left)?.plan;
        let right = self.bind(right)?.plan;
        let left_key = self.bind_expr(left_key, &self.scope_of(&left), None)?;
        // The subquery side binds its key by position (`*`) or not at all
        // (a literal marks an uncorrelated EXISTS).
        let right_key = match right_key {
            Expr::Column(name) if name != "*" => {
                self.bind_expr(Expr::Column(name), &self.scope_of(&right), None)?
            }
            other => other,
        };
        Ok((Box::new(left), Box::new(right), left_key, right_key))
    }

    /// Route the conjuncts of a filter into the join tree below it. What
    /// comes back stays above `plan`.
    fn route(&self, conjuncts: Vec<Expr>, plan: LogicalPlan) -> Result<(LogicalPlan, Vec<Expr>)> {
        match plan {
            LogicalPlan::Join {
                left,
                right,
                join_type: JoinType::Cross | JoinType::Inner,
                condition,
                distribution,
            } => {
                let left_scope = self.scope_of(&left).qualified_by(relation_qualifier(&left));
                let right_scope = self
                    .scope_of(&right)
                    .qualified_by(relation_qualifier(&right));
                let mut left_parts = Vec::new();
                let mut right_parts = Vec::new();
                let mut keys = Vec::new();
                let mut above = Vec::new();
                for conjunct in conjuncts {
                    match placement(&conjunct, &left_scope, &right_scope) {
                        Placement::Left => left_parts.push(conjunct),
                        Placement::Right => right_parts.push(conjunct),
                        Placement::Key(left, right) => keys.push((left, right)),
                        Placement::Above => above.push(conjunct),
                    }
                }
                let left = self.place(left_parts, *left)?;
                let right = self.place(right_parts, *right)?;
                let mut condition = condition;
                for (left_key, right_key) in keys {
                    let key = Expr::BinaryOp {
                        left: Box::new(Expr::Column(left_key)),
                        op: BinaryOp::Eq,
                        right: Box::new(Expr::Column(right_key)),
                    };
                    condition = Some(match condition {
                        Some(condition) => Expr::And(Box::new(condition), Box::new(key)),
                        None => key,
                    });
                }
                let join_type = if condition.is_some() {
                    JoinType::Inner
                } else {
                    JoinType::Cross
                };
                Ok((
                    LogicalPlan::Join {
                        left: Box::new(left),
                        right: Box::new(right),
                        join_type,
                        condition,
                        distribution,
                    },
                    above,
                ))
            }
            // A semi or anti join emits rows of its left input, so a filter
            // on its output is a filter on that input.
            LogicalPlan::SemiJoin {
                left,
                right,
                left_key,
                right_key,
            } => Ok((
                LogicalPlan::SemiJoin {
                    left: Box::new(self.place(conjuncts, *left)?),
                    right,
                    left_key,
                    right_key,
                },
                Vec::new(),
            )),
            LogicalPlan::AntiJoin {
                left,
                right,
                left_key,
                right_key,
            } => Ok((
                LogicalPlan::AntiJoin {
                    left: Box::new(self.place(conjuncts, *left)?),
                    right,
                    left_key,
                    right_key,
                },
                Vec::new(),
            )),
            other => Ok((other, conjuncts)),
        }
    }

    /// Route `conjuncts` into `plan`, filtering above it what does not go
    /// further down.
    fn place(&self, conjuncts: Vec<Expr>, plan: LogicalPlan) -> Result<LogicalPlan> {
        if conjuncts.is_empty() {
            return Ok(plan);
        }
        let (plan, residual) = self.route(conjuncts, plan)?;
        let scope = self.scope_of(&plan);
        self.filtered(plan, residual, &scope, None)
    }

    fn filtered(
        &self,
        plan: LogicalPlan,
        conjuncts: Vec<Expr>,
        scope: &Scope,
        aggregate_input: Option<&Scope>,
    ) -> Result<LogicalPlan> {
        let bound = conjuncts
            .into_iter()
            .map(|conjunct| self.bind_expr(conjunct, scope, aggregate_input))
            .collect::<Result<Vec<_>>>()?;
        Ok(match conjoin(bound) {
            Some(predicate) => LogicalPlan::Filter {
                input: Box::new(plan),
                predicate,
            },
            None => plan,
        })
    }

    /// A projected column keeps the name it was written with.
    fn bind_projected(
        &self,
        expr: Expr,
        scope: &Scope,
        aggregate_input: Option<&Scope>,
    ) -> Result<Expr> {
        match expr {
            Expr::Column(name) if name != "*" => {
                let bound = self.bind_name(&name, scope)?;
                Ok(if bound == name {
                    Expr::Column(name)
                } else {
                    Expr::Alias {
                        expr: Box::new(Expr::Column(bound)),
                        name,
                    }
                })
            }
            other => self.bind_expr(other, scope, aggregate_input),
        }
    }

    fn bind_aggregate(&self, aggregate: AggregateExpr, scope: &Scope) -> Result<AggregateExpr> {
        Ok(match aggregate {
            AggregateExpr::Count { expr, distinct } => AggregateExpr::Count {
                expr: self.bind_expr(expr, scope, None)?,
                distinct,
            },
            AggregateExpr::Sum { expr, distinct } => AggregateExpr::Sum {
                expr: self.bind_expr(expr, scope, None)?,
                distinct,
            },
            AggregateExpr::Avg { expr, distinct } => AggregateExpr::Avg {
                expr: self.bind_expr(expr, scope, None)?,
                distinct,
            },
            AggregateExpr::Min(expr) => AggregateExpr::Min(self.bind_expr(expr, scope, None)?),
            AggregateExpr::Max(expr) => AggregateExpr::Max(self.bind_expr(expr, scope, None)?),
        })
    }

    fn bind_name(&self, name: &str, scope: &Scope) -> Result<String> {
        if name == "*" {
            return Ok(name.to_owned());
        }
        match scope.resolve(name) {
            Resolution::Unique(physical) if scope.joined => Ok(physical),
            Resolution::Unique(_) | Resolution::Unresolved => Ok(name.to_owned()),
            Resolution::Ambiguous => Err(KaveonError::Sql(format!("column '{name}' is ambiguous"))),
        }
    }

    fn bind_expr(
        &self,
        expr: Expr,
        scope: &Scope,
        aggregate_input: Option<&Scope>,
    ) -> Result<Expr> {
        let bind = |expr: Box<Expr>| -> Result<Box<Expr>> {
            self.bind_expr(*expr, scope, aggregate_input).map(Box::new)
        };
        Ok(match expr {
            Expr::Column(name) => Expr::Column(self.bind_name(&name, scope)?),
            Expr::Literal(_) | Expr::Star => expr,
            Expr::Alias { expr, name } => Expr::Alias {
                expr: bind(expr)?,
                name,
            },
            Expr::BinaryOp { left, op, right } => Expr::BinaryOp {
                left: bind(left)?,
                op,
                right: bind(right)?,
            },
            Expr::And(left, right) => Expr::And(bind(left)?, bind(right)?),
            Expr::Or(left, right) => Expr::Or(bind(left)?, bind(right)?),
            Expr::Not(inner) => Expr::Not(bind(inner)?),
            Expr::IsNull(inner) => Expr::IsNull(bind(inner)?),
            Expr::IsNotNull(inner) => Expr::IsNotNull(bind(inner)?),
            Expr::Cast { expr, data_type } => Expr::Cast {
                expr: bind(expr)?,
                data_type,
            },
            Expr::Extract { field, expr } => Expr::Extract {
                field,
                expr: bind(expr)?,
            },
            // An aggregate reference above the aggregate binds its
            // argument the way the aggregate did.
            Expr::Function { name, args } => {
                let scope = match aggregate_input {
                    Some(input) if is_aggregate(&name) => input,
                    _ => scope,
                };
                Expr::Function {
                    name,
                    args: args
                        .into_iter()
                        .map(|arg| self.bind_expr(arg, scope, None))
                        .collect::<Result<_>>()?,
                }
            }
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
                    .map(|arg| self.bind_expr(arg, scope, aggregate_input))
                    .collect::<Result<_>>()?,
                partition_by: partition_by
                    .into_iter()
                    .map(|expr| self.bind_expr(expr, scope, aggregate_input))
                    .collect::<Result<_>>()?,
                order_by: order_by
                    .into_iter()
                    .map(|(expr, ascending)| {
                        self.bind_expr(expr, scope, aggregate_input)
                            .map(|expr| (expr, ascending))
                    })
                    .collect::<Result<_>>()?,
                frame,
            },
            Expr::Case {
                operand,
                when_then,
                else_expr,
            } => Expr::Case {
                operand: operand.map(bind).transpose()?,
                when_then: when_then
                    .into_iter()
                    .map(|(when, then)| {
                        Ok((
                            self.bind_expr(when, scope, aggregate_input)?,
                            self.bind_expr(then, scope, aggregate_input)?,
                        ))
                    })
                    .collect::<Result<_>>()?,
                else_expr: else_expr.map(bind).transpose()?,
            },
            Expr::Like {
                expr,
                pattern,
                negated,
                case_insensitive,
            } => Expr::Like {
                expr: bind(expr)?,
                pattern: bind(pattern)?,
                negated,
                case_insensitive,
            },
            Expr::Between {
                expr,
                low,
                high,
                negated,
            } => Expr::Between {
                expr: bind(expr)?,
                low: bind(low)?,
                high: bind(high)?,
                negated,
            },
            Expr::InList {
                expr,
                list,
                negated,
            } => Expr::InList {
                expr: bind(expr)?,
                list: list
                    .into_iter()
                    .map(|item| self.bind_expr(item, scope, aggregate_input))
                    .collect::<Result<_>>()?,
                negated,
            },
        })
    }

    fn join_scope(&self, left: &LogicalPlan, right: &LogicalPlan) -> Scope {
        let mut columns = self
            .scope_of(left)
            .qualified_by(relation_qualifier(left))
            .columns;
        columns.extend(
            self.scope_of(right)
                .qualified_by(relation_qualifier(right))
                .columns,
        );
        Scope {
            columns,
            joined: true,
        }
    }

    /// The output columns of a bound plan.
    fn scope_of(&self, plan: &LogicalPlan) -> Scope {
        match plan {
            LogicalPlan::Scan {
                table,
                alias,
                columns,
            } => {
                let qualifier = alias
                    .clone()
                    .unwrap_or_else(|| table.rsplit('.').next().unwrap_or(table).to_owned());
                let Ok(resolved) = self.catalog.resolve_table(&TableReference::parse(table)) else {
                    return Scope::default();
                };
                Scope {
                    columns: resolved
                        .table
                        .arrow_schema
                        .fields()
                        .iter()
                        .filter(|field| {
                            columns
                                .as_ref()
                                .is_none_or(|columns| columns.contains(field.name()))
                        })
                        .map(|field| Column {
                            qualifier: Some(qualifier.clone()),
                            name: field.name().clone(),
                            physical: field.name().clone(),
                        })
                        .collect(),
                    joined: false,
                }
            }
            LogicalPlan::Join { left, right, .. } => self.join_scope(left, right),
            LogicalPlan::Filter { input, .. }
            | LogicalPlan::Sort { input, .. }
            | LogicalPlan::Limit { input, .. }
            | LogicalPlan::Offset { input, .. }
            | LogicalPlan::Distinct { input }
            | LogicalPlan::Window { input, .. } => self.scope_of(input),
            LogicalPlan::SemiJoin { left, .. }
            | LogicalPlan::AntiJoin { left, .. }
            | LogicalPlan::Intersect { left, .. }
            | LogicalPlan::Except { left, .. } => self.scope_of(left),
            LogicalPlan::Union { inputs, .. } => Scope {
                columns: inputs
                    .first()
                    .map(|input| self.scope_of(input).columns)
                    .unwrap_or_default(),
                joined: false,
            },
            LogicalPlan::Project { input, columns } => {
                let input = self.scope_of(input);
                let mut out = Vec::new();
                for column in columns {
                    match column {
                        Expr::Star => out.extend(input.columns.iter().cloned()),
                        Expr::Alias { name, .. } => out.push(Column {
                            qualifier: None,
                            name: name.clone(),
                            physical: name.clone(),
                        }),
                        // The output field carries the written name.
                        Expr::Column(name) => {
                            let (qualifier, bare) = match name.rsplit_once('.') {
                                Some((qualifier, bare)) => (Some(qualifier.to_owned()), bare),
                                None => (None, name.as_str()),
                            };
                            out.push(Column {
                                qualifier,
                                name: bare.to_owned(),
                                physical: name.clone(),
                            });
                        }
                        // Named by its printed form; not addressable by name.
                        _ => {}
                    }
                }
                Scope {
                    columns: out,
                    joined: false,
                }
            }
            LogicalPlan::Aggregate {
                input,
                group_by,
                aggregates,
            } => {
                let input = self.scope_of(input);
                let mut out = Vec::new();
                for key in group_by {
                    if let Expr::Column(name) = key {
                        out.push(
                            input
                                .columns
                                .iter()
                                .find(|column| &column.physical == name)
                                .cloned()
                                .unwrap_or_else(|| Column {
                                    qualifier: None,
                                    name: name.clone(),
                                    physical: name.clone(),
                                }),
                        );
                    }
                }
                for aggregate in aggregates {
                    let name = aggregate_output_name(aggregate);
                    out.push(Column {
                        qualifier: None,
                        name: name.clone(),
                        physical: name,
                    });
                }
                Scope {
                    columns: out,
                    joined: false,
                }
            }
        }
    }
}

/// Where a filter conjunct over a join belongs, by the sides its columns
/// resolve on.
fn placement(conjunct: &Expr, left: &Scope, right: &Scope) -> Placement {
    let mut references = Vec::new();
    column_references(conjunct, &mut references);
    if references.is_empty() {
        return Placement::Above;
    }
    let on_left = references
        .iter()
        .all(|reference| left.holds(reference) && !right.holds(reference));
    let on_right = references
        .iter()
        .all(|reference| right.holds(reference) && !left.holds(reference));
    if on_left {
        return Placement::Left;
    }
    if on_right {
        return Placement::Right;
    }
    if let Expr::BinaryOp {
        left: a,
        op: BinaryOp::Eq,
        right: b,
    } = conjunct
        && let (Expr::Column(a), Expr::Column(b)) = (a.as_ref(), b.as_ref())
    {
        let side = |reference: &str| match (left.resolve(reference), right.resolve(reference)) {
            (Resolution::Unique(physical), Resolution::Unresolved) => Some((true, physical)),
            (Resolution::Unresolved, Resolution::Unique(physical)) => Some((false, physical)),
            _ => None,
        };
        if let (Some((a_left, a_physical)), Some((b_left, b_physical))) = (side(a), side(b)) {
            if a_left && !b_left {
                return Placement::Key(a_physical, b_physical);
            }
            if !a_left && b_left {
                return Placement::Key(b_physical, a_physical);
            }
        }
    }
    Placement::Above
}

fn column_references(expr: &Expr, into: &mut Vec<String>) {
    match expr {
        Expr::Column(name) => {
            if name != "*" {
                into.push(name.clone());
            }
        }
        Expr::Literal(_) | Expr::Star => {}
        Expr::Alias { expr, .. }
        | Expr::Not(expr)
        | Expr::IsNull(expr)
        | Expr::IsNotNull(expr)
        | Expr::Cast { expr, .. }
        | Expr::Extract { expr, .. } => column_references(expr, into),
        Expr::BinaryOp { left, right, .. } | Expr::And(left, right) | Expr::Or(left, right) => {
            column_references(left, into);
            column_references(right, into);
        }
        Expr::Function { args, .. } => {
            for arg in args {
                column_references(arg, into);
            }
        }
        Expr::WindowFunction {
            args,
            partition_by,
            order_by,
            ..
        } => {
            for expr in args
                .iter()
                .chain(partition_by)
                .chain(order_by.iter().map(|(expr, _)| expr))
            {
                column_references(expr, into);
            }
        }
        Expr::Case {
            operand,
            when_then,
            else_expr,
        } => {
            for expr in operand.iter().chain(else_expr) {
                column_references(expr, into);
            }
            for (when, then) in when_then {
                column_references(when, into);
                column_references(then, into);
            }
        }
        Expr::Like { expr, pattern, .. } => {
            column_references(expr, into);
            column_references(pattern, into);
        }
        Expr::Between {
            expr, low, high, ..
        } => {
            column_references(expr, into);
            column_references(low, into);
            column_references(high, into);
        }
        Expr::InList { expr, list, .. } => {
            column_references(expr, into);
            for item in list {
                column_references(item, into);
            }
        }
    }
}

/// The relation a join qualifies an input's columns with: a scan (or a
/// filtered scan) under its alias or table name; anything else keeps its
/// own output names.
fn relation_qualifier(plan: &LogicalPlan) -> Option<&str> {
    match plan {
        LogicalPlan::Scan { table, alias, .. } => Some(
            alias
                .as_deref()
                .unwrap_or_else(|| table.rsplit('.').next().unwrap_or(table)),
        ),
        LogicalPlan::Filter { input, .. } => relation_qualifier(input),
        _ => None,
    }
}

fn is_aggregate(name: &str) -> bool {
    matches!(name, "COUNT" | "SUM" | "AVG" | "MIN" | "MAX")
}

/// The aggregate's output column as the local planner names it.
fn aggregate_output_name(aggregate: &AggregateExpr) -> String {
    let (function, expr) = match aggregate {
        AggregateExpr::Count { expr, .. } => ("count", expr),
        AggregateExpr::Sum { expr, .. } => ("sum", expr),
        AggregateExpr::Avg { expr, .. } => ("avg", expr),
        AggregateExpr::Min(expr) => ("min", expr),
        AggregateExpr::Max(expr) => ("max", expr),
    };
    let argument = match expr {
        Expr::Column(name) => name.as_str(),
        Expr::Star => "*",
        _ => "expr",
    };
    format!("{function}_{argument}")
}

fn conjuncts(expr: Expr) -> Vec<Expr> {
    fn split(expr: Expr, into: &mut Vec<Expr>) {
        match expr {
            Expr::And(left, right) => {
                split(*left, into);
                split(*right, into);
            }
            other => into.push(other),
        }
    }
    let mut out = Vec::new();
    split(expr, &mut out);
    out
}

fn conjoin(mut conjuncts: Vec<Expr>) -> Option<Expr> {
    let mut result = conjuncts.pop()?;
    while let Some(conjunct) = conjuncts.pop() {
        result = Expr::And(Box::new(conjunct), Box::new(result));
    }
    Some(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use arrow::datatypes::{DataType, Field, Schema};
    use kaveon_core::predicate::ScalarValue;
    use kaveon_core::{
        AccessPattern, CatalogProvider, DataFormat, MemoryCatalog, StorageType, TableMeta,
    };
    use kaveon_sql::logical_plan::sql_to_logical_plan;
    use std::path::PathBuf;
    use std::sync::Arc;

    fn catalog() -> CatalogManager {
        let mut memory = MemoryCatalog::new(
            "lake",
            StorageType::Local {
                base_path: PathBuf::from("."),
            },
        )
        .with_schema("tpch");
        let tables: [(&str, &[&str]); 4] = [
            (
                "customer",
                &["c_custkey", "c_name", "c_nationkey", "c_mktsegment"],
            ),
            ("orders", &["o_orderkey", "o_custkey", "o_orderdate"]),
            ("lineitem", &["l_orderkey", "l_quantity", "l_shipdate"]),
            ("nation", &["n_nationkey", "n_name", "n_regionkey"]),
        ];
        for (table, columns) in tables {
            let fields = columns
                .iter()
                .map(|column| Field::new(*column, DataType::Int64, true))
                .collect::<Vec<_>>();
            memory
                .register_table(
                    "tpch",
                    TableMeta {
                        name: table.to_owned(),
                        arrow_schema: Arc::new(Schema::new(fields)),
                        location: format!("{table}.parquet"),
                        access: AccessPattern::Shortcut,
                        format: DataFormat::Parquet,
                    },
                )
                .unwrap();
        }
        let mut manager = CatalogManager::new("lake", "tpch");
        manager.register_catalog(Box::new(memory));
        manager
    }

    fn bound(sql: &str) -> LogicalPlan {
        let mut plan = sql_to_logical_plan(sql).unwrap();
        qualify(&mut plan);
        bind(plan, &catalog()).unwrap()
    }

    fn qualify(plan: &mut LogicalPlan) {
        match plan {
            LogicalPlan::Scan { table, .. } => *table = format!("lake.tpch.{table}"),
            LogicalPlan::Join { left, right, .. }
            | LogicalPlan::SemiJoin { left, right, .. }
            | LogicalPlan::AntiJoin { left, right, .. } => {
                qualify(left);
                qualify(right);
            }
            LogicalPlan::Filter { input, .. }
            | LogicalPlan::Project { input, .. }
            | LogicalPlan::Aggregate { input, .. }
            | LogicalPlan::Sort { input, .. }
            | LogicalPlan::Limit { input, .. }
            | LogicalPlan::Offset { input, .. }
            | LogicalPlan::Distinct { input }
            | LogicalPlan::Window { input, .. } => qualify(input),
            _ => unreachable!("not built by these tests"),
        }
    }

    fn column(name: &str) -> Expr {
        Expr::Column(name.into())
    }

    fn equal(left: &str, right: &str) -> Expr {
        Expr::BinaryOp {
            left: Box::new(column(left)),
            op: BinaryOp::Eq,
            right: Box::new(column(right)),
        }
    }

    #[test]
    fn a_comma_join_with_a_bare_equality_becomes_an_inner_hash_join() {
        let plan = bound(
            "SELECT c_name, o_orderdate FROM customer, orders WHERE c_custkey = o_custkey AND c_mktsegment = 'BUILDING' AND o_orderdate < 9000",
        );
        let LogicalPlan::Project { input, columns } = plan else {
            panic!("projection");
        };
        // Projected columns bind to the join's names and keep their own.
        assert_eq!(
            columns,
            vec![
                Expr::Alias {
                    expr: Box::new(column("customer.c_name")),
                    name: "c_name".into()
                },
                Expr::Alias {
                    expr: Box::new(column("orders.o_orderdate")),
                    name: "o_orderdate".into()
                },
            ]
        );
        let LogicalPlan::Join {
            left,
            right,
            join_type,
            condition,
            ..
        } = *input
        else {
            panic!("the filter went into the join, nothing stays above it");
        };
        assert_eq!(join_type, JoinType::Inner);
        assert_eq!(
            condition,
            Some(equal("customer.c_custkey", "orders.o_custkey"))
        );
        // Single-side conjuncts sit on their side, as bare names for the
        // scan to resolve.
        let LogicalPlan::Filter { predicate, input } = *left else {
            panic!("customer filter");
        };
        assert!(matches!(*input, LogicalPlan::Scan { .. }));
        assert_eq!(
            predicate,
            Expr::BinaryOp {
                left: Box::new(column("c_mktsegment")),
                op: BinaryOp::Eq,
                right: Box::new(Expr::Literal(ScalarValue::Utf8("BUILDING".into()))),
            }
        );
        let LogicalPlan::Filter { predicate, .. } = *right else {
            panic!("orders filter");
        };
        assert!(matches!(
            predicate,
            Expr::BinaryOp {
                op: BinaryOp::Lt,
                ..
            }
        ));
    }

    #[test]
    fn a_three_way_comma_join_places_each_key_at_its_join_and_qualifies_the_aggregate() {
        let plan = bound(
            "SELECT l_orderkey, o_orderdate, SUM(l_quantity) AS q FROM customer, orders, lineitem WHERE c_custkey = o_custkey AND l_orderkey = o_orderkey AND c_mktsegment = 'BUILDING' GROUP BY l_orderkey, o_orderdate ORDER BY q DESC",
        );
        let LogicalPlan::Sort { input, order_by } = plan else {
            panic!("sort");
        };
        // Above the projection nothing is rewritten.
        assert_eq!(order_by, vec![(column("q"), false)]);
        let LogicalPlan::Project { input, columns } = *input else {
            panic!("projection");
        };
        // The aggregate reference binds its argument like the aggregate.
        assert_eq!(
            columns[2],
            Expr::Alias {
                expr: Box::new(Expr::Function {
                    name: "SUM".into(),
                    args: vec![column("lineitem.l_quantity")]
                }),
                name: "q".into()
            }
        );
        let LogicalPlan::Aggregate {
            input,
            group_by,
            aggregates,
        } = *input
        else {
            panic!("aggregate");
        };
        assert_eq!(
            group_by,
            vec![column("lineitem.l_orderkey"), column("orders.o_orderdate")]
        );
        assert!(matches!(
            &aggregates[0],
            AggregateExpr::Sum { expr, .. } if expr == &column("lineitem.l_quantity")
        ));
        let LogicalPlan::Join {
            left,
            right,
            condition,
            join_type,
            ..
        } = *input
        else {
            panic!("outer join");
        };
        assert_eq!(join_type, JoinType::Inner);
        assert_eq!(
            condition,
            Some(equal("orders.o_orderkey", "lineitem.l_orderkey"))
        );
        assert!(matches!(*right, LogicalPlan::Scan { .. }));
        let LogicalPlan::Join {
            left, condition, ..
        } = *left
        else {
            panic!("inner join");
        };
        assert_eq!(
            condition,
            Some(equal("customer.c_custkey", "orders.o_custkey"))
        );
        assert!(matches!(*left, LogicalPlan::Filter { .. }));
    }

    #[test]
    fn mixed_conjuncts_stay_above_the_join_and_self_joins_need_qualifiers() {
        let plan = bound(
            "SELECT n1.n_name FROM nation n1, nation n2 WHERE n1.n_regionkey = n2.n_regionkey AND (n1.n_name = 1 OR n2.n_name = 2)",
        );
        let LogicalPlan::Project { input, columns } = plan else {
            panic!("projection");
        };
        assert_eq!(columns, vec![column("n1.n_name")]);
        let LogicalPlan::Filter { input, predicate } = *input else {
            panic!("the OR stays above the join");
        };
        assert!(matches!(predicate, Expr::Or(..)));
        let LogicalPlan::Join { condition, .. } = *input else {
            panic!("join");
        };
        assert_eq!(condition, Some(equal("n1.n_regionkey", "n2.n_regionkey")));
        let mut plan = sql_to_logical_plan(
            "SELECT n_name FROM nation n1, nation n2 WHERE n1.n_regionkey = n2.n_regionkey",
        )
        .unwrap();
        qualify(&mut plan);
        let error = bind(plan, &catalog()).unwrap_err().to_string();
        assert!(error.contains("'n_name' is ambiguous"), "{error}");
    }

    #[test]
    fn a_single_relation_and_an_unknown_relation_are_left_as_written() {
        let single = "SELECT c_name FROM customer WHERE c_custkey = 1 ORDER BY c_name";
        let expected = format!("{:?}", {
            let mut plan = sql_to_logical_plan(single).unwrap();
            qualify(&mut plan);
            plan
        });
        assert_eq!(format!("{:?}", bound(single)), expected);
        let unknown = "SELECT a, b FROM unknown, customer WHERE a = c_custkey AND b = 2";
        let mut plan = sql_to_logical_plan(unknown).unwrap();
        qualify(&mut plan);
        let plan = bind(plan, &catalog()).unwrap();
        let LogicalPlan::Project { input, .. } = plan else {
            panic!("projection");
        };
        // Nothing resolves on the unknown side, so the filter stays whole.
        let LogicalPlan::Filter { input, .. } = *input else {
            panic!("filter above the join");
        };
        assert!(matches!(
            *input,
            LogicalPlan::Join {
                join_type: JoinType::Cross,
                condition: None,
                ..
            }
        ));
    }

    #[test]
    fn an_outer_join_filters_its_non_preserved_side_by_the_on_clause_and_refuses_residuals() {
        let plan = bound(
            "SELECT c_custkey, COUNT(o_orderkey) AS n FROM customer LEFT OUTER JOIN orders ON c_custkey = o_custkey AND o_orderdate <> 7 GROUP BY c_custkey",
        );
        let LogicalPlan::Project { input, .. } = plan else {
            panic!("projection");
        };
        let LogicalPlan::Aggregate { input, .. } = *input else {
            panic!("aggregate");
        };
        let LogicalPlan::Join {
            left,
            right,
            join_type,
            condition,
            ..
        } = *input
        else {
            panic!("the join is the aggregate's input; nothing stays above it");
        };
        assert_eq!(join_type, JoinType::Left);
        assert_eq!(
            condition,
            Some(equal("customer.c_custkey", "orders.o_custkey"))
        );
        assert!(matches!(*left, LogicalPlan::Scan { .. }));
        let LogicalPlan::Filter { predicate, .. } = *right else {
            panic!("orders filtered before the join");
        };
        assert!(matches!(
            predicate,
            Expr::BinaryOp {
                op: BinaryOp::Ne,
                ..
            }
        ));
        // The preserved side cannot be filtered by ON, and there is no
        // residual for an outer join.
        for sql in [
            "SELECT c_custkey FROM customer LEFT JOIN orders ON c_custkey = o_custkey AND c_nationkey <> 7",
            "SELECT c_custkey FROM customer LEFT JOIN orders ON c_custkey = o_custkey AND c_nationkey <> o_orderdate",
            "SELECT c_custkey FROM customer FULL JOIN orders ON c_custkey = o_custkey AND o_orderdate <> 7",
        ] {
            let mut plan = sql_to_logical_plan(sql).unwrap();
            qualify(&mut plan);
            let error = bind(plan, &catalog()).unwrap_err().to_string();
            assert!(error.contains("JOIN conditions"), "{sql}: {error}");
        }
        // An inner join filters a mixed conjunct above itself.
        let plan = bound(
            "SELECT c_custkey FROM customer JOIN orders ON c_custkey = o_custkey AND c_nationkey <> o_orderdate",
        );
        let LogicalPlan::Project { input, .. } = plan else {
            panic!("projection");
        };
        let LogicalPlan::Filter { input, predicate } = *input else {
            panic!("residual above the inner join");
        };
        assert!(matches!(
            predicate,
            Expr::BinaryOp {
                op: BinaryOp::Ne,
                ..
            }
        ));
        assert!(matches!(
            *input,
            LogicalPlan::Join {
                join_type: JoinType::Inner,
                condition: Some(_),
                ..
            }
        ));
    }

    #[test]
    fn an_explicit_join_condition_binds_and_the_where_routes_below_it() {
        let plan = bound(
            "SELECT c.c_name FROM customer c JOIN orders o ON c.c_custkey = o.o_custkey WHERE o_orderdate > 1 AND c_mktsegment = 'BUILDING'",
        );
        let LogicalPlan::Project { input, .. } = plan else {
            panic!("projection");
        };
        let LogicalPlan::Join {
            left,
            right,
            condition,
            ..
        } = *input
        else {
            panic!("join with both filters below it");
        };
        assert_eq!(condition, Some(equal("c.c_custkey", "o.o_custkey")));
        assert!(matches!(*left, LogicalPlan::Filter { .. }));
        assert!(matches!(*right, LogicalPlan::Filter { .. }));
    }
}
