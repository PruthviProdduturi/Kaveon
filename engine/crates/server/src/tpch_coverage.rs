//! TPC-H coverage as a gate: the twenty-two statements of
//! `docs/qualification/tpch/trino-queries.sql`, run against a tiny
//! deterministic TPC-H (every table, Trino's column names and types) through
//! the node-local planner and, separately, through the distributed stage
//! planner. The test prints a coverage table and holds the recorded truth:
//! every statement outside `KNOWN_UNSUPPORTED` must parse, plan and execute,
//! and every statement inside it must still fail, so the list cannot go
//! stale in either direction. `docs/qualification/tpch/coverage.md` is the
//! human record of the same table.
use std::fs::File;
use std::path::PathBuf;
use std::sync::{Arc, mpsc};
use std::time::Duration;

use arrow::array::{
    Array, ArrayRef, Date32Array, Float64Array, Int32Array, Int64Array, StringArray,
};
use arrow::compute::cast;
use arrow::datatypes::{DataType, Field, Schema};
use arrow::record_batch::RecordBatch;
use kaveon_core::{
    AccessPattern, BatchOperator, CatalogManager, CatalogProvider, DataFormat, MemoryCatalog,
    QueryMemoryPool, StorageType, TableMeta,
};
use parquet::arrow::ArrowWriter;
use parquet::file::properties::WriterProperties;

const QUERIES: &str = include_str!("../../../../docs/qualification/tpch/trino-queries.sql");

/// Statements that do not run today, with the reason. A statement listed
/// here must still fail at some stage; one that starts working must be
/// removed from the list.
const KNOWN_UNSUPPORTED: &[(&str, &str)] = &[
    ("q2", "parse: scalar subqueries are not yet supported"),
    (
        "q3",
        "plan: comma join lands as a cross product; projection column 'l_orderkey' not in input",
    ),
    (
        "q4",
        "plan: correlated EXISTS (l_orderkey = o_orderkey) is planned as an uncorrelated semi join; lineitem has no column 'o_orderkey'",
    ),
    (
        "q5",
        "plan: comma join lands as a cross product; projection column 'n_name' not in input",
    ),
    (
        "q6",
        "execute: arithmetic between decimal literals (0.06 - 0.01) is unsupported",
    ),
    (
        "q7",
        "execute: six-way comma join runs as a cross product; over budget",
    ),
    (
        "q8",
        "execute: eight-way comma join runs as a cross product; over budget",
    ),
    (
        "q9",
        "plan: comma join lands as a cross product; projection column 'n_name' not in input",
    ),
    (
        "q10",
        "plan: comma join lands as a cross product; projection column 'c_custkey' not in input",
    ),
    ("q11", "parse: scalar subqueries are not yet supported"),
    (
        "q12",
        "plan: comma join lands as a cross product; projection column 'l_shipmode' not in input",
    ),
    (
        "q13",
        "plan: LEFT JOIN ON with a NOT LIKE conjunct; only equality join conditions are supported",
    ),
    ("q15", "parse: scalar subqueries are not yet supported"),
    (
        "q16",
        "plan: comma join lands as a cross product; projection column 'p_brand' not in input",
    ),
    ("q17", "parse: scalar subqueries are not yet supported"),
    (
        "q18",
        "plan: comma join lands as a cross product; group-by column 'c_name' not in input",
    ),
    ("q20", "parse: scalar subqueries are not yet supported"),
    (
        "q21",
        "parse: correlated subqueries are unsupported: l1.l_orderkey",
    ),
    ("q22", "parse: scalar subqueries are not yet supported"),
];

/// Statements without a distributed plan today, with the reason.
const KNOWN_NO_DISTRIBUTED_PLAN: &[(&str, &str)] = &[];

const REGIONS: [&str; 5] = ["AFRICA", "AMERICA", "ASIA", "EUROPE", "MIDDLE EAST"];
const NATIONS: [(&str, i64); 25] = [
    ("ALGERIA", 0),
    ("ARGENTINA", 1),
    ("BRAZIL", 1),
    ("CANADA", 1),
    ("EGYPT", 4),
    ("ETHIOPIA", 0),
    ("FRANCE", 3),
    ("GERMANY", 3),
    ("INDIA", 2),
    ("INDONESIA", 2),
    ("IRAN", 4),
    ("IRAQ", 4),
    ("JAPAN", 2),
    ("JORDAN", 4),
    ("KENYA", 0),
    ("MOROCCO", 0),
    ("MOZAMBIQUE", 0),
    ("PERU", 1),
    ("CHINA", 2),
    ("ROMANIA", 3),
    ("SAUDI ARABIA", 4),
    ("VIETNAM", 2),
    ("RUSSIA", 3),
    ("UNITED KINGDOM", 3),
    ("UNITED STATES", 1),
];
const SEGMENTS: [&str; 5] = [
    "AUTOMOBILE",
    "BUILDING",
    "FURNITURE",
    "MACHINERY",
    "HOUSEHOLD",
];
const PRIORITIES: [&str; 5] = ["1-URGENT", "2-HIGH", "3-MEDIUM", "4-NOT SPECIFIED", "5-LOW"];
const TYPE_SYLLABLE_1: [&str; 6] = ["STANDARD", "SMALL", "MEDIUM", "LARGE", "ECONOMY", "PROMO"];
const TYPE_SYLLABLE_2: [&str; 5] = ["ANODIZED", "BURNISHED", "PLATED", "POLISHED", "BRUSHED"];
const TYPE_SYLLABLE_3: [&str; 5] = ["TIN", "NICKEL", "BRASS", "STEEL", "COPPER"];
const CONTAINER_1: [&str; 5] = ["SM", "LG", "MED", "JUMBO", "WRAP"];
const CONTAINER_2: [&str; 8] = ["CASE", "BOX", "BAG", "JAR", "PKG", "PACK", "CAN", "DRUM"];
const COLORS: [&str; 8] = [
    "forest",
    "green",
    "almond",
    "antique",
    "aquamarine",
    "azure",
    "beige",
    "bisque",
];
const SHIP_INSTRUCTIONS: [&str; 4] = [
    "DELIVER IN PERSON",
    "COLLECT COD",
    "NONE",
    "TAKE BACK RETURN",
];
const SHIP_MODES: [&str; 7] = ["REG AIR", "AIR", "RAIL", "SHIP", "TRUCK", "MAIL", "FOB"];

const SUPPLIERS: i64 = 10;
const PARTS: i64 = 40;
const CUSTOMERS: i64 = 30;
const ORDERS: i64 = 150;

/// A deterministic mixer: the same rows on every run and machine.
fn mix(seed: u64) -> u64 {
    let mut value = seed.wrapping_mul(0x9E37_79B9_7F4A_7C15);
    value ^= value >> 29;
    value = value.wrapping_mul(0xBF58_476D_1CE4_E5B9);
    value ^ (value >> 32)
}

fn pick(seed: u64, salt: u64, modulo: usize) -> usize {
    (mix(seed.wrapping_mul(131).wrapping_add(salt)) % modulo as u64) as usize
}

fn days(date: &str) -> i32 {
    kaveon_core::predicate::date_literal_days(date).unwrap() as i32
}

enum Column {
    Int64(Vec<i64>),
    Int32(Vec<i32>),
    Float64(Vec<f64>),
    Utf8(Vec<String>),
    Date32(Vec<i32>),
}

struct Table {
    name: &'static str,
    columns: Vec<(&'static str, Column)>,
}

/// The number of suppliers of a part is four, as in the specification; the
/// four keys are distinct within a part.
fn part_supplier(partkey: i64, ordinal: i64) -> i64 {
    (partkey - 1 + ordinal * 3) % SUPPLIERS + 1
}

fn phone(nationkey: i64, seed: u64) -> String {
    format!(
        "{}-{:03}-{:03}-{:04}",
        10 + nationkey,
        pick(seed, 1, 900) + 100,
        pick(seed, 2, 900) + 100,
        pick(seed, 3, 9000) + 1000
    )
}

fn region() -> Table {
    Table {
        name: "region",
        columns: vec![
            ("r_regionkey", Column::Int64((0..5).collect())),
            (
                "r_name",
                Column::Utf8(REGIONS.iter().map(|name| (*name).to_owned()).collect()),
            ),
            (
                "r_comment",
                Column::Utf8(
                    REGIONS
                        .iter()
                        .map(|name| format!("{} region", name.to_lowercase()))
                        .collect(),
                ),
            ),
        ],
    }
}

fn nation() -> Table {
    Table {
        name: "nation",
        columns: vec![
            ("n_nationkey", Column::Int64((0..25).collect())),
            (
                "n_name",
                Column::Utf8(NATIONS.iter().map(|(name, _)| (*name).to_owned()).collect()),
            ),
            (
                "n_regionkey",
                Column::Int64(NATIONS.iter().map(|(_, region)| *region).collect()),
            ),
            (
                "n_comment",
                Column::Utf8(
                    NATIONS
                        .iter()
                        .map(|(name, _)| format!("{} nation", name.to_lowercase()))
                        .collect(),
                ),
            ),
        ],
    }
}

fn supplier() -> Table {
    let keys: Vec<i64> = (1..=SUPPLIERS).collect();
    let nationkey = |key: i64| pick(key as u64, 11, NATIONS.len()) as i64;
    Table {
        name: "supplier",
        columns: vec![
            ("s_suppkey", Column::Int64(keys.clone())),
            (
                "s_name",
                Column::Utf8(keys.iter().map(|k| format!("Supplier#{k:09}")).collect()),
            ),
            (
                "s_address",
                Column::Utf8(
                    keys.iter()
                        .map(|k| format!("{} Supplier Street", pick(*k as u64, 12, 900) + 1))
                        .collect(),
                ),
            ),
            (
                "s_nationkey",
                Column::Int64(keys.iter().map(|k| nationkey(*k)).collect()),
            ),
            (
                "s_phone",
                Column::Utf8(
                    keys.iter()
                        .map(|k| phone(nationkey(*k), *k as u64 + 700))
                        .collect(),
                ),
            ),
            (
                "s_acctbal",
                Column::Float64(
                    keys.iter()
                        .map(|k| (pick(*k as u64, 13, 1_099_999) as f64 - 99_999.0) / 100.0)
                        .collect(),
                ),
            ),
            (
                "s_comment",
                Column::Utf8(
                    keys.iter()
                        .map(|k| {
                            if pick(*k as u64, 14, 5) == 0 {
                                "final Customer accounts sleep Complaints".to_owned()
                            } else {
                                "quickly regular packages".to_owned()
                            }
                        })
                        .collect(),
                ),
            ),
        ],
    }
}

fn part() -> Table {
    let keys: Vec<i64> = (1..=PARTS).collect();
    Table {
        name: "part",
        columns: vec![
            ("p_partkey", Column::Int64(keys.clone())),
            (
                "p_name",
                Column::Utf8(
                    keys.iter()
                        .map(|k| {
                            let seed = *k as u64;
                            format!(
                                "{} {} {}",
                                COLORS[pick(seed, 21, COLORS.len())],
                                COLORS[pick(seed, 22, COLORS.len())],
                                COLORS[pick(seed, 23, COLORS.len())]
                            )
                        })
                        .collect(),
                ),
            ),
            (
                "p_mfgr",
                Column::Utf8(
                    keys.iter()
                        .map(|k| format!("Manufacturer#{}", pick(*k as u64, 24, 5) + 1))
                        .collect(),
                ),
            ),
            (
                "p_brand",
                Column::Utf8(
                    keys.iter()
                        .map(|k| {
                            format!(
                                "Brand#{}{}",
                                pick(*k as u64, 25, 5) + 1,
                                pick(*k as u64, 26, 5) + 1
                            )
                        })
                        .collect(),
                ),
            ),
            (
                "p_type",
                Column::Utf8(
                    keys.iter()
                        .map(|k| {
                            let seed = *k as u64;
                            format!(
                                "{} {} {}",
                                TYPE_SYLLABLE_1[pick(seed, 27, TYPE_SYLLABLE_1.len())],
                                TYPE_SYLLABLE_2[pick(seed, 28, TYPE_SYLLABLE_2.len())],
                                TYPE_SYLLABLE_3[pick(seed, 29, TYPE_SYLLABLE_3.len())]
                            )
                        })
                        .collect(),
                ),
            ),
            (
                "p_size",
                Column::Int32(
                    keys.iter()
                        .map(|k| pick(*k as u64, 30, 50) as i32 + 1)
                        .collect(),
                ),
            ),
            (
                "p_container",
                Column::Utf8(
                    keys.iter()
                        .map(|k| {
                            let seed = *k as u64;
                            format!(
                                "{} {}",
                                CONTAINER_1[pick(seed, 31, CONTAINER_1.len())],
                                CONTAINER_2[pick(seed, 32, CONTAINER_2.len())]
                            )
                        })
                        .collect(),
                ),
            ),
            (
                "p_retailprice",
                Column::Float64(
                    keys.iter()
                        .map(|k| 900.0 + (*k % 10) as f64 * 100.0 + (*k % 100) as f64 / 100.0)
                        .collect(),
                ),
            ),
            (
                "p_comment",
                Column::Utf8(keys.iter().map(|_| "regular deposits".to_owned()).collect()),
            ),
        ],
    }
}

fn partsupp() -> Table {
    let mut partkey = Vec::new();
    let mut suppkey = Vec::new();
    let mut availqty = Vec::new();
    let mut supplycost = Vec::new();
    for part in 1..=PARTS {
        for ordinal in 0..4 {
            let seed = (part * 4 + ordinal) as u64;
            partkey.push(part);
            suppkey.push(part_supplier(part, ordinal));
            availqty.push(pick(seed, 41, 9_999) as i32 + 1);
            supplycost.push((pick(seed, 42, 99_900) as f64 + 100.0) / 100.0);
        }
    }
    let rows = partkey.len();
    Table {
        name: "partsupp",
        columns: vec![
            ("ps_partkey", Column::Int64(partkey)),
            ("ps_suppkey", Column::Int64(suppkey)),
            ("ps_availqty", Column::Int32(availqty)),
            ("ps_supplycost", Column::Float64(supplycost)),
            (
                "ps_comment",
                Column::Utf8(vec!["carefully ironic accounts".to_owned(); rows]),
            ),
        ],
    }
}

fn customer() -> Table {
    let keys: Vec<i64> = (1..=CUSTOMERS).collect();
    let nationkey = |key: i64| pick(key as u64, 51, NATIONS.len()) as i64;
    Table {
        name: "customer",
        columns: vec![
            ("c_custkey", Column::Int64(keys.clone())),
            (
                "c_name",
                Column::Utf8(keys.iter().map(|k| format!("Customer#{k:09}")).collect()),
            ),
            (
                "c_address",
                Column::Utf8(
                    keys.iter()
                        .map(|k| format!("{} Customer Avenue", pick(*k as u64, 52, 900) + 1))
                        .collect(),
                ),
            ),
            (
                "c_nationkey",
                Column::Int64(keys.iter().map(|k| nationkey(*k)).collect()),
            ),
            (
                "c_phone",
                Column::Utf8(
                    keys.iter()
                        .map(|k| phone(nationkey(*k), *k as u64 + 900))
                        .collect(),
                ),
            ),
            (
                "c_acctbal",
                Column::Float64(
                    keys.iter()
                        .map(|k| (pick(*k as u64, 53, 1_099_999) as f64 - 99_999.0) / 100.0)
                        .collect(),
                ),
            ),
            (
                "c_mktsegment",
                Column::Utf8(
                    keys.iter()
                        .map(|k| SEGMENTS[pick(*k as u64, 54, SEGMENTS.len())].to_owned())
                        .collect(),
                ),
            ),
            (
                "c_comment",
                Column::Utf8(keys.iter().map(|_| "even requests".to_owned()).collect()),
            ),
        ],
    }
}

struct OrderRows {
    keys: Vec<i64>,
    custkeys: Vec<i64>,
    dates: Vec<i32>,
}

/// A third of the customers place no orders, as in the specification.
fn order_rows() -> OrderRows {
    let keys: Vec<i64> = (1..=ORDERS).collect();
    let custkeys = keys
        .iter()
        .map(|k| {
            let mut custkey = pick(*k as u64, 61, CUSTOMERS as usize) as i64 + 1;
            if custkey % 3 == 0 {
                custkey -= 1;
            }
            custkey
        })
        .collect();
    let epoch = days("1992-01-01");
    let dates = keys
        .iter()
        .map(|k| epoch + pick(*k as u64, 62, 2_405) as i32)
        .collect();
    OrderRows {
        keys,
        custkeys,
        dates,
    }
}

fn orders(rows: &OrderRows, totals: &[f64]) -> Table {
    let keys = &rows.keys;
    Table {
        name: "orders",
        columns: vec![
            ("o_orderkey", Column::Int64(keys.clone())),
            ("o_custkey", Column::Int64(rows.custkeys.clone())),
            (
                "o_orderstatus",
                Column::Utf8(
                    keys.iter()
                        .map(|k| ["F", "O", "P"][pick(*k as u64, 63, 3)].to_owned())
                        .collect(),
                ),
            ),
            ("o_totalprice", Column::Float64(totals.to_vec())),
            ("o_orderdate", Column::Date32(rows.dates.clone())),
            (
                "o_orderpriority",
                Column::Utf8(
                    keys.iter()
                        .map(|k| PRIORITIES[pick(*k as u64, 64, PRIORITIES.len())].to_owned())
                        .collect(),
                ),
            ),
            (
                "o_clerk",
                Column::Utf8(
                    keys.iter()
                        .map(|k| format!("Clerk#{:09}", pick(*k as u64, 65, 20) + 1))
                        .collect(),
                ),
            ),
            ("o_shippriority", Column::Int32(vec![0; keys.len()])),
            (
                "o_comment",
                Column::Utf8(
                    keys.iter()
                        .map(|k| {
                            if pick(*k as u64, 66, 6) == 0 {
                                "blithely special packages requests".to_owned()
                            } else {
                                "furiously final deposits".to_owned()
                            }
                        })
                        .collect(),
                ),
            ),
        ],
    }
}

/// Line items and each order's total price (the sum of its extended prices).
fn lineitem(rows: &OrderRows, retail: &[f64]) -> (Table, Vec<f64>) {
    let mut orderkey = Vec::new();
    let mut partkey = Vec::new();
    let mut suppkey = Vec::new();
    let mut linenumber = Vec::new();
    let mut quantity = Vec::new();
    let mut extendedprice = Vec::new();
    let mut discount = Vec::new();
    let mut tax = Vec::new();
    let mut returnflag = Vec::new();
    let mut linestatus = Vec::new();
    let mut shipdate = Vec::new();
    let mut commitdate = Vec::new();
    let mut receiptdate = Vec::new();
    let mut shipinstruct = Vec::new();
    let mut shipmode = Vec::new();
    let mut totals = Vec::new();
    for (order, (key, orderdate)) in rows.keys.iter().zip(&rows.dates).enumerate() {
        let lines = pick(*key as u64, 71, 7) + 1;
        let mut total = 0.0;
        for line in 1..=lines {
            let seed = (order * 8 + line) as u64;
            let part = pick(seed, 72, PARTS as usize) as i64 + 1;
            let qty = pick(seed, 73, 50) as f64 + 1.0;
            let price = qty * retail[(part - 1) as usize];
            let ship = orderdate + pick(seed, 74, 121) as i32 + 1;
            orderkey.push(*key);
            partkey.push(part);
            suppkey.push(part_supplier(part, pick(seed, 75, 4) as i64));
            linenumber.push(line as i32);
            quantity.push(qty);
            extendedprice.push(price);
            discount.push(pick(seed, 76, 11) as f64 / 100.0);
            tax.push(pick(seed, 77, 9) as f64 / 100.0);
            returnflag.push(["R", "A", "N"][pick(seed, 78, 3)].to_owned());
            linestatus.push(if ship > days("1995-06-17") { "O" } else { "F" }.to_owned());
            shipdate.push(ship);
            commitdate.push(orderdate + pick(seed, 79, 61) as i32 + 30);
            receiptdate.push(ship + pick(seed, 80, 30) as i32 + 1);
            shipinstruct
                .push(SHIP_INSTRUCTIONS[pick(seed, 81, SHIP_INSTRUCTIONS.len())].to_owned());
            shipmode.push(SHIP_MODES[pick(seed, 82, SHIP_MODES.len())].to_owned());
            total += price;
        }
        totals.push(total);
    }
    let count = orderkey.len();
    let table = Table {
        name: "lineitem",
        columns: vec![
            ("l_orderkey", Column::Int64(orderkey)),
            ("l_partkey", Column::Int64(partkey)),
            ("l_suppkey", Column::Int64(suppkey)),
            ("l_linenumber", Column::Int32(linenumber)),
            ("l_quantity", Column::Float64(quantity)),
            ("l_extendedprice", Column::Float64(extendedprice)),
            ("l_discount", Column::Float64(discount)),
            ("l_tax", Column::Float64(tax)),
            ("l_returnflag", Column::Utf8(returnflag)),
            ("l_linestatus", Column::Utf8(linestatus)),
            ("l_shipdate", Column::Date32(shipdate)),
            ("l_commitdate", Column::Date32(commitdate)),
            ("l_receiptdate", Column::Date32(receiptdate)),
            ("l_shipinstruct", Column::Utf8(shipinstruct)),
            ("l_shipmode", Column::Utf8(shipmode)),
            (
                "l_comment",
                Column::Utf8(vec!["pending packages".to_owned(); count]),
            ),
        ],
    };
    (table, totals)
}

fn tables() -> Vec<Table> {
    let part = part();
    let retail: Vec<f64> = match &part.columns[7].1 {
        Column::Float64(values) => values.clone(),
        _ => unreachable!("p_retailprice is the eighth column"),
    };
    let rows = order_rows();
    let (lineitem, totals) = lineitem(&rows, &retail);
    vec![
        region(),
        nation(),
        supplier(),
        part,
        partsupp(),
        customer(),
        orders(&rows, &totals),
        lineitem,
    ]
}

fn batch(table: &Table) -> RecordBatch {
    let mut fields = Vec::new();
    let mut arrays: Vec<ArrayRef> = Vec::new();
    for (name, column) in &table.columns {
        let (data_type, array): (DataType, ArrayRef) = match column {
            Column::Int64(values) => (DataType::Int64, Arc::new(Int64Array::from(values.clone()))),
            Column::Int32(values) => (DataType::Int32, Arc::new(Int32Array::from(values.clone()))),
            Column::Float64(values) => (
                DataType::Float64,
                Arc::new(Float64Array::from(values.clone())),
            ),
            Column::Utf8(values) => (
                DataType::Utf8,
                Arc::new(StringArray::from_iter_values(values.iter())),
            ),
            Column::Date32(values) => (
                DataType::Date32,
                Arc::new(Date32Array::from(values.clone())),
            ),
        };
        fields.push(Field::new(*name, data_type, true));
        arrays.push(array);
    }
    RecordBatch::try_new(Arc::new(Schema::new(fields)), arrays).unwrap()
}

fn write(directory: &std::path::Path, table: &Table) -> TableMeta {
    let file = format!("{}.parquet", table.name);
    let batch = batch(table);
    let properties = WriterProperties::builder()
        .set_max_row_group_size(256)
        .build();
    let mut writer = ArrowWriter::try_new(
        File::create(directory.join(&file)).unwrap(),
        batch.schema(),
        Some(properties),
    )
    .unwrap();
    writer.write(&batch).unwrap();
    writer.close().unwrap();
    TableMeta {
        name: table.name.to_owned(),
        arrow_schema: batch.schema(),
        location: file,
        access: AccessPattern::Shortcut,
        format: DataFormat::Parquet,
    }
}

/// The twenty-two statements in file order, `(id, sql)`.
fn statements() -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut lines = QUERIES.lines();
    while let Some(line) = lines.next() {
        if let Some(number) = line.strip_prefix("-- Q")
            && !number.is_empty()
            && number.chars().all(|c| c.is_ascii_digit())
        {
            let sql = lines
                .next()
                .expect("a statement follows its label")
                .trim()
                .trim_end_matches(';')
                .to_owned();
            out.push((format!("q{number}"), sql));
        }
    }
    assert_eq!(
        out.len(),
        22,
        "the TPC-H file carries twenty-two statements"
    );
    out
}

/// Rows as text, one string per row.
fn rows(operator: &mut dyn BatchOperator) -> kaveon_core::Result<Vec<String>> {
    let mut rows = Vec::new();
    while let Some(batch) = operator.next_batch()? {
        let columns = batch
            .columns()
            .iter()
            .map(|column| cast(column, &DataType::Utf8))
            .collect::<std::result::Result<Vec<_>, _>>()?;
        for row in 0..batch.num_rows() {
            let cells = columns
                .iter()
                .map(|column| {
                    let column = column.as_any().downcast_ref::<StringArray>().unwrap();
                    if column.is_null(row) {
                        "NULL".to_owned()
                    } else {
                        column.value(row).to_owned()
                    }
                })
                .collect::<Vec<_>>();
            rows.push(cells.join(" | "));
        }
    }
    Ok(rows)
}

#[derive(Debug, Default)]
struct Coverage {
    id: String,
    parsed: Option<String>,
    planned: Option<String>,
    executed: Option<String>,
    row_count: usize,
    first_row: String,
    distributed: Option<String>,
}

impl Coverage {
    fn local_ok(&self) -> bool {
        self.parsed.is_none() && self.planned.is_none() && self.executed.is_none()
    }

    fn failure(&self) -> Option<String> {
        self.parsed
            .as_ref()
            .map(|error| format!("parse: {error}"))
            .or_else(|| self.planned.as_ref().map(|error| format!("plan: {error}")))
            .or_else(|| {
                self.executed
                    .as_ref()
                    .map(|error| format!("execute: {error}"))
            })
    }
}

pub(crate) struct Fixture {
    directory: PathBuf,
    pub(crate) manager: CatalogManager,
}

impl Fixture {
    pub(crate) fn new(label: &str) -> Self {
        let directory =
            std::env::temp_dir().join(format!("kaveon-tpch-{label}-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&directory).unwrap();
        let mut catalog = MemoryCatalog::new(
            "tpch",
            StorageType::Local {
                base_path: directory.clone(),
            },
        )
        .with_schema("tiny");
        for table in tables() {
            let meta = write(&directory, &table);
            catalog.register_table("tiny", meta).unwrap();
        }
        let mut manager = CatalogManager::new("tpch", "tiny");
        manager.register_catalog(Box::new(catalog));
        Self { directory, manager }
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.directory);
    }
}

/// The time one statement may take to plan and execute; a statement over
/// budget is recorded as such rather than hanging the gate.
const STATEMENT_BUDGET: Duration = Duration::from_secs(30);

fn cover(fixture: &Arc<Fixture>, id: &str, statement: &str) -> Coverage {
    let mut coverage = Coverage {
        id: id.to_owned(),
        ..Default::default()
    };
    let mut plan = match kaveon_sql::logical_plan::sql_to_logical_plan(statement) {
        Ok(plan) => plan,
        Err(error) => {
            coverage.parsed = Some(error.to_string());
            coverage.distributed = Some("not parsed".to_owned());
            return coverage;
        }
    };
    crate::planner::qualify_tables(&mut plan, "tpch", "tiny");
    let plan = kaveon_optim::rules::push_filter_down(plan);
    let plan = kaveon_optim::rules::push_projection_down(plan);

    coverage.distributed = crate::planner::build_stage_graph(id, &plan, 2)
        .and_then(|_| crate::planner::build_executable_fragments(id, &plan, &fixture.manager, 2))
        .err()
        .map(|error| error.to_string());

    // Planning and execution run on their own thread so a statement whose
    // plan is a runaway product is recorded as over budget, not hung.
    let (sender, receiver) = mpsc::channel();
    let fixture = Arc::clone(fixture);
    let query = id.to_owned();
    std::thread::spawn(move || {
        let pool = QueryMemoryPool::new("tpch", 256 * 1024 * 1024).unwrap();
        let outcome = crate::planner::plan_query_with_memory(&plan, &fixture.manager, &pool)
            .map_err(|error| (true, error.to_string()))
            .and_then(|mut planned| {
                let rows =
                    rows(planned.operator.as_mut()).map_err(|error| (false, error.to_string()));
                drop(planned);
                assert_eq!(
                    pool.snapshot().current_bytes,
                    0,
                    "{query} leaked reservations"
                );
                rows
            });
        let _ = sender.send(outcome);
    });
    match receiver.recv_timeout(STATEMENT_BUDGET) {
        Ok(Ok(rows)) => {
            coverage.row_count = rows.len();
            coverage.first_row = rows.first().cloned().unwrap_or_default();
        }
        Ok(Err((planning, error))) => {
            if planning {
                coverage.planned = Some(error);
            } else {
                coverage.executed = Some(error);
            }
        }
        Err(_) => {
            coverage.executed = Some(format!(
                "exceeded the {} s budget",
                STATEMENT_BUDGET.as_secs()
            ));
        }
    }
    coverage
}

#[test]
fn the_tpch_statements_cover_what_the_record_says() {
    let fixture = Arc::new(Fixture::new("coverage"));
    let mut table = Vec::new();
    let mut problems = Vec::new();
    println!(
        "| query | parsed | planned | executed | rows | distributed | detail |\n|---|---|---|---|---|---|---|"
    );
    for (id, statement) in statements() {
        let coverage = cover(&fixture, &id, &statement);
        let mark = |error: &Option<String>| if error.is_none() { "yes" } else { "no" };
        let detail = coverage
            .failure()
            .or_else(|| {
                coverage
                    .distributed
                    .as_ref()
                    .map(|error| format!("distributed: {error}"))
            })
            .unwrap_or_else(|| coverage.first_row.clone());
        println!(
            "| {} | {} | {} | {} | {} | {} | {} |",
            coverage.id,
            mark(&coverage.parsed),
            mark(&coverage.planned),
            mark(&coverage.executed),
            coverage.row_count,
            mark(&coverage.distributed),
            detail
        );
        let expected_unsupported = KNOWN_UNSUPPORTED.iter().any(|(query, _)| *query == id);
        match (coverage.local_ok(), expected_unsupported) {
            (true, true) => problems.push(format!(
                "{id} now runs ({} rows): remove it from KNOWN_UNSUPPORTED",
                coverage.row_count
            )),
            (false, false) => problems.push(format!(
                "{id} stopped working: {}",
                coverage.failure().unwrap_or_default()
            )),
            _ => {}
        }
        let expected_no_distributed = KNOWN_NO_DISTRIBUTED_PLAN
            .iter()
            .any(|(query, _)| *query == id);
        match (coverage.distributed.is_none(), expected_no_distributed) {
            (true, true) => problems.push(format!(
                "{id} now has a distributed plan: remove it from KNOWN_NO_DISTRIBUTED_PLAN"
            )),
            (false, false) if coverage.local_ok() => problems.push(format!(
                "{id} lost its distributed plan: {}",
                coverage.distributed.clone().unwrap_or_default()
            )),
            _ => {}
        }
        table.push(coverage);
    }
    let running = table.iter().filter(|coverage| coverage.local_ok()).count();
    let distributed = table
        .iter()
        .filter(|coverage| coverage.local_ok() && coverage.distributed.is_none())
        .count();
    println!("\n{running} of 22 run locally; {distributed} of those have a distributed plan");
    assert!(problems.is_empty(), "{}", problems.join("\n"));
}
