use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use crate::{
    DataFormat, ExchangeId, Expr, KaveonError, Partitioning, Result, StageId, StoragePredicate,
};

pub const EXECUTABLE_FRAGMENT_VERSION: u16 = 2;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct FragmentNodeId(pub u32);

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ExecutableFragment {
    pub version: u16,
    pub stage_id: StageId,
    pub root: FragmentNodeId,
    pub nodes: Vec<FragmentNode>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FragmentNode {
    pub id: FragmentNodeId,
    pub inputs: Vec<FragmentNodeId>,
    pub operator: FragmentOperator,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum FragmentOperator {
    Scan(ScanSpec),
    ExchangeInput(ExchangeInput),
    Filter {
        predicate: Expr,
    },
    Project {
        expressions: Vec<NamedExpr>,
    },
    Aggregate {
        mode: AggregateMode,
        group_by: Vec<NamedExpr>,
        aggregates: Vec<AggregateSpec>,
    },
    Sort {
        keys: Vec<SortSpec>,
    },
    TopN {
        keys: Vec<SortSpec>,
        limit: usize,
    },
    Limit {
        limit: usize,
    },
    ExchangeOutput(ExchangeOutput),
    HashJoin(JoinSpec),
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ScanSpec {
    pub source_uri: String,
    pub format: DataFormat,
    pub table: ScanTable,
    pub projection: Vec<String>,
    pub predicate: Option<StoragePredicate>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScanTable {
    pub catalog: String,
    pub schema: String,
    pub table: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct NamedExpr {
    pub name: String,
    pub expression: Expr,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AggregateMode {
    Single,
    Partial,
    Final,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AggregateFunction {
    Count,
    Sum,
    Min,
    Max,
    Avg,
    CountDistinct,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AggregateSpec {
    pub function: AggregateFunction,
    pub argument: Option<Expr>,
    pub output: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SortSpec {
    pub expression: Expr,
    pub ascending: bool,
    pub nulls_first: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExchangeInput {
    pub exchange_id: ExchangeId,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExchangeOutput {
    pub exchange_id: ExchangeId,
    pub partitioning: Partitioning,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JoinType {
    Inner,
    Left,
    Right,
    Full,
    Cross,
    Semi,
    Anti,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct JoinSpec {
    pub join_type: JoinType,
    pub left_keys: Vec<Expr>,
    pub right_keys: Vec<Expr>,
    pub residual: Option<Expr>,
    pub broadcast: bool,
}

impl ExecutableFragment {
    pub fn validate(&self) -> Result<()> {
        if self.version != EXECUTABLE_FRAGMENT_VERSION {
            return invalid(format!(
                "unsupported executable fragment version {}",
                self.version
            ));
        }
        if self.nodes.is_empty() {
            return invalid("executable fragment must contain at least one node");
        }
        let mut nodes = BTreeMap::new();
        for node in &self.nodes {
            if nodes.insert(node.id, node).is_some() {
                return invalid(format!("duplicate fragment node ID {}", node.id.0));
            }
            node.validate_shape()?;
        }
        if !nodes.contains_key(&self.root) {
            return invalid(format!("fragment root node {} does not exist", self.root.0));
        }
        for node in &self.nodes {
            for input in &node.inputs {
                if !nodes.contains_key(input) {
                    return invalid(format!(
                        "node {} references unknown input {}",
                        node.id.0, input.0
                    ));
                }
            }
        }
        let mut visiting = BTreeSet::new();
        let mut visited = BTreeSet::new();
        visit(self.root, &nodes, &mut visiting, &mut visited)?;
        if visited.len() != nodes.len() {
            return invalid("fragment contains nodes unreachable from its root");
        }
        Ok(())
    }
}

impl FragmentNode {
    fn validate_shape(&self) -> Result<()> {
        let expected = match &self.operator {
            FragmentOperator::Scan(scan) => {
                scan.validate()?;
                0
            }
            FragmentOperator::ExchangeInput(input) => {
                validate_exchange_id(&input.exchange_id)?;
                0
            }
            FragmentOperator::HashJoin(join) => {
                join.validate()?;
                2
            }
            FragmentOperator::ExchangeOutput(output) => {
                validate_exchange_id(&output.exchange_id)?;
                output.partitioning.validate()?;
                1
            }
            FragmentOperator::Filter { .. } => 1,
            FragmentOperator::Project { expressions } => {
                validate_named_expressions(expressions, "project")?;
                1
            }
            FragmentOperator::Aggregate {
                group_by,
                aggregates,
                ..
            } => {
                validate_named_expressions(group_by, "group by")?;
                if aggregates.is_empty() {
                    return invalid("aggregate operator requires at least one aggregate");
                }
                let mut outputs = BTreeSet::new();
                for aggregate in aggregates {
                    validate_name(&aggregate.output, "aggregate output")?;
                    if !outputs.insert(aggregate.output.as_str()) {
                        return invalid(format!("duplicate aggregate output {}", aggregate.output));
                    }
                    if aggregate.function != AggregateFunction::Count
                        && aggregate.argument.is_none()
                    {
                        return invalid(format!("{:?} requires an argument", aggregate.function));
                    }
                }
                1
            }
            FragmentOperator::Sort { keys } => {
                if keys.is_empty() {
                    return invalid("sort requires at least one key");
                }
                1
            }
            FragmentOperator::TopN { keys, limit } => {
                if keys.is_empty() {
                    return invalid("TopN requires at least one key");
                }
                if *limit == 0 {
                    return invalid("TopN limit must be greater than zero");
                }
                1
            }
            FragmentOperator::Limit { limit } => {
                if *limit == 0 {
                    return invalid("limit must be greater than zero");
                }
                1
            }
        };
        if self.inputs.len() != expected {
            return invalid(format!(
                "node {} requires {expected} inputs, found {}",
                self.id.0,
                self.inputs.len()
            ));
        }
        Ok(())
    }
}

impl ScanSpec {
    fn validate(&self) -> Result<()> {
        validate_name(&self.source_uri, "scan source URI")?;
        validate_name(&self.table.catalog, "scan catalog")?;
        validate_name(&self.table.schema, "scan schema")?;
        validate_name(&self.table.table, "scan table")?;
        let mut columns = BTreeSet::new();
        for column in &self.projection {
            validate_name(column, "projected column")?;
            if !columns.insert(column) {
                return invalid(format!("duplicate projected column {column}"));
            }
        }
        Ok(())
    }
}

impl JoinSpec {
    fn validate(&self) -> Result<()> {
        if self.join_type == JoinType::Cross {
            if !self.left_keys.is_empty() || !self.right_keys.is_empty() {
                return invalid("cross join cannot define equality keys");
            }
        } else if self.left_keys.is_empty() || self.left_keys.len() != self.right_keys.len() {
            return invalid("hash join requires equal, non-empty left and right key lists");
        }
        if self.broadcast && matches!(self.join_type, JoinType::Right | JoinType::Full) {
            return invalid("broadcast build is not valid for right or full hash joins");
        }
        Ok(())
    }
}

fn visit(
    id: FragmentNodeId,
    nodes: &BTreeMap<FragmentNodeId, &FragmentNode>,
    visiting: &mut BTreeSet<FragmentNodeId>,
    visited: &mut BTreeSet<FragmentNodeId>,
) -> Result<()> {
    if visited.contains(&id) {
        return Ok(());
    }
    if !visiting.insert(id) {
        return invalid(format!("fragment contains a cycle at node {}", id.0));
    }
    for input in &nodes[&id].inputs {
        visit(*input, nodes, visiting, visited)?;
    }
    visiting.remove(&id);
    visited.insert(id);
    Ok(())
}

fn validate_named_expressions(expressions: &[NamedExpr], context: &str) -> Result<()> {
    let mut names = BTreeSet::new();
    for expression in expressions {
        validate_name(&expression.name, context)?;
        if !names.insert(expression.name.as_str()) {
            return invalid(format!("duplicate {context} output {}", expression.name));
        }
    }
    Ok(())
}

fn validate_exchange_id(id: &ExchangeId) -> Result<()> {
    validate_name(&id.0, "exchange ID")
}

fn validate_name(value: &str, context: &str) -> Result<()> {
    if value.trim().is_empty() {
        return invalid(format!("{context} cannot be empty"));
    }
    Ok(())
}

fn invalid<T>(message: impl Into<String>) -> Result<T> {
    Err(KaveonError::Execution(message.into()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scan(id: u32) -> FragmentNode {
        FragmentNode {
            id: FragmentNodeId(id),
            inputs: vec![],
            operator: FragmentOperator::Scan(ScanSpec {
                source_uri: "file:///events.parquet".into(),
                format: DataFormat::Parquet,
                table: ScanTable {
                    catalog: "local".into(),
                    schema: "default".into(),
                    table: "events".into(),
                },
                projection: vec!["id".into()],
                predicate: None,
            }),
        }
    }

    fn valid_fragment() -> ExecutableFragment {
        ExecutableFragment {
            version: EXECUTABLE_FRAGMENT_VERSION,
            stage_id: StageId(1),
            root: FragmentNodeId(2),
            nodes: vec![
                scan(1),
                FragmentNode {
                    id: FragmentNodeId(2),
                    inputs: vec![FragmentNodeId(1)],
                    operator: FragmentOperator::Limit { limit: 10 },
                },
            ],
        }
    }

    #[test]
    fn validates_executable_fragment() {
        valid_fragment().validate().unwrap();
    }

    #[test]
    fn rejects_unknown_versions() {
        let mut fragment = valid_fragment();
        fragment.version += 1;
        assert!(
            fragment
                .validate()
                .unwrap_err()
                .to_string()
                .contains("version")
        );
    }

    #[test]
    fn rejects_cycles() {
        let mut fragment = valid_fragment();
        fragment.nodes[0].inputs = vec![FragmentNodeId(2)];
        fragment.nodes[0].operator = FragmentOperator::Limit { limit: 1 };
        assert!(
            fragment
                .validate()
                .unwrap_err()
                .to_string()
                .contains("cycle")
        );
    }

    #[test]
    fn rejects_unreachable_nodes() {
        let mut fragment = valid_fragment();
        fragment.nodes.push(scan(3));
        assert!(
            fragment
                .validate()
                .unwrap_err()
                .to_string()
                .contains("unreachable")
        );
    }

    #[test]
    fn rejects_malformed_join() {
        let join = FragmentNode {
            id: FragmentNodeId(3),
            inputs: vec![FragmentNodeId(1), FragmentNodeId(2)],
            operator: FragmentOperator::HashJoin(JoinSpec {
                join_type: JoinType::Inner,
                left_keys: vec![],
                right_keys: vec![],
                residual: None,
                broadcast: false,
            }),
        };
        let error = join.validate_shape().unwrap_err();
        assert!(error.to_string().contains("hash join"));
    }
}
