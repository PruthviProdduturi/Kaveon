use kaveon_sql::logical_plan::{LogicalPlan, sql_to_logical_plan};

#[test]
fn lowers_tsql_isnull_to_vectorized_coalesce() {
    let plan = sql_to_logical_plan("SELECT ISNULL(value, 0) FROM sample").unwrap();
    let debug = format!("{plan:?}");
    assert!(debug.contains("name: \"COALESCE\""), "{debug}");
    assert!(!debug.contains("name: \"ISNULL\""), "{debug}");
}

#[test]
fn rejects_non_binary_tsql_isnull() {
    let error = sql_to_logical_plan("SELECT ISNULL(a, b, c) FROM sample").unwrap_err();
    assert!(
        error
            .to_string()
            .contains("ISNULL requires exactly two arguments")
    );
}

#[test]
fn accepts_tsql_top_and_fetch_forms() {
    for sql in [
        "SELECT TOP 2 value FROM sample",
        "SELECT TOP (2) value FROM sample",
        "SELECT value FROM sample ORDER BY value OFFSET 1 ROWS FETCH NEXT 2 ROWS ONLY",
    ] {
        let plan = sql_to_logical_plan(sql).unwrap();
        assert!(matches!(plan, LogicalPlan::Limit { .. }), "{sql}: {plan:?}");
    }
}

#[test]
fn lowers_tsql_date_functions_to_existing_temporal_kernels() {
    for (sql, expected) in [
        (
            "SELECT DATEADD(day, 2, event_date) FROM sample",
            "DATE_ADD_DAYS",
        ),
        (
            "SELECT DATEADD(month, 1, event_date) FROM sample",
            "DATE_ADD_MONTHS",
        ),
        (
            "SELECT DATEADD(hour, 1, event_ts) FROM sample",
            "DATE_ADD_SECONDS",
        ),
        (
            "SELECT DATEDIFF(day, start_date, end_date) FROM sample",
            "DATE_DIFF",
        ),
        ("SELECT DATEPART(year, event_date) FROM sample", "DATE_PART"),
        (
            "SELECT YEAR(event_date), MONTH(event_date), DAY(event_date) FROM sample",
            "DATE_PART",
        ),
        ("SELECT GETDATE(), GETUTCDATE() FROM sample", "NOW"),
    ] {
        let plan = sql_to_logical_plan(sql).unwrap();
        let debug = format!("{plan:?}");
        assert!(debug.contains(expected), "{sql}: {debug}");
    }
}

#[test]
fn rejects_dynamic_or_unsupported_tsql_dateparts() {
    for sql in [
        "SELECT DATEADD(millisecond, 1, event_ts) FROM sample",
        "SELECT DATEPART(millisecond, event_ts) FROM sample",
        "SELECT DATEADD(datepart_column, 1, event_date) FROM sample",
    ] {
        assert!(
            sql_to_logical_plan(sql).is_err(),
            "expected rejection: {sql}"
        );
    }
}

#[test]
fn lowers_convert_and_try_convert_to_existing_cast_kernels() {
    for (sql, expected) in [
        ("SELECT CONVERT(INT, value) FROM sample", "Cast"),
        (
            "SELECT TRY_CONVERT(INT, value) FROM sample",
            "TRY_CAST_INT32",
        ),
        ("SELECT CONVERT(DATE, value, 101) FROM sample", "Date32"),
        (
            "SELECT CONVERT(INT, CAST(NULL AS VARCHAR)) FROM sample",
            "Int32",
        ),
    ] {
        let plan = sql_to_logical_plan(sql).unwrap();
        let debug = format!("{plan:?}");
        assert!(debug.contains(expected), "{sql}: {debug}");
    }
}

#[test]
fn records_unimplemented_fabric_query_shapes_as_rejected() {
    let pivot =
        sql_to_logical_plan("SELECT * FROM sample PIVOT (SUM(value) FOR category IN ('a')) AS p")
            .unwrap_err();
    assert!(pivot.to_string().contains("PIVOT"), "{pivot}");
    assert!(
        pivot.to_string().contains("conditional aggregates"),
        "{pivot}"
    );

    let unpivot =
        sql_to_logical_plan("SELECT * FROM sample UNPIVOT (value FOR category IN (a, b)) AS u")
            .unwrap_err();
    assert!(unpivot.to_string().contains("UNPIVOT"), "{unpivot}");
    assert!(unpivot.to_string().contains("UNION ALL"), "{unpivot}");

    let outer_apply =
        sql_to_logical_plan("SELECT * FROM sample s OUTER APPLY (SELECT s.value) AS x")
            .unwrap_err();
    assert!(
        outer_apply.to_string().contains("OUTER APPLY"),
        "{outer_apply}"
    );
    assert!(
        outer_apply.to_string().contains("LEFT JOIN"),
        "{outer_apply}"
    );
}

#[test]
fn lowers_uncorrelated_cross_apply_to_a_cross_join() {
    let plan =
        sql_to_logical_plan("SELECT * FROM sample s CROSS APPLY (SELECT 1 AS x) AS x").unwrap();
    assert!(format!("{plan:?}").contains("join_type: Cross"), "{plan:?}");

    let error = sql_to_logical_plan("SELECT * FROM sample s CROSS APPLY (SELECT s.value) AS x")
        .unwrap_err();
    assert!(error.to_string().contains("uncorrelated"), "{error}");
}

#[test]
fn qualifies_existing_window_surface_and_bracket_identifiers() {
    let qualify = sql_to_logical_plan(
        "SELECT [year], ROW_NUMBER() OVER (ORDER BY [year]) AS rn FROM sample QUALIFY rn = 1",
    )
    .unwrap();
    assert!(format!("{qualify:?}").contains("Window"), "{qualify:?}");

    let bracketed = sql_to_logical_plan("SELECT [year] FROM sample").unwrap();
    assert!(format!("{bracketed:?}").contains("Column(\"year\")"));
}
