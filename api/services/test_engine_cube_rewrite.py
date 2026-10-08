import unittest

from services import engine_cube_rewrite as rewrite


class RecognisedShapeTests(unittest.TestCase):
    """The shapes the rewrite accepts, and exactly what it issues instead."""

    def test_metric_sorted_breakdown_drops_order_by_and_limit(self):
        plan = rewrite.plan(
            'SELECT segment, SUM(sessions) AS "Sessions" '
            'FROM public.kaveon_events_enriched GROUP BY segment '
            'ORDER BY "Sessions" DESC NULLS LAST LIMIT 500'
        )
        self.assertEqual(
            plan.statement,
            'SELECT segment, SUM(sessions) AS "Sessions" '
            'FROM public.kaveon_events_enriched GROUP BY segment',
        )
        self.assertEqual(plan.sort_keys, ((1, True),))
        self.assertEqual(plan.limit, 500)
        self.assertEqual(plan.width, 2)
        self.assertTrue(plan.grouped)

    def test_dimension_sorted_breakdown_resolves_the_grouping_column(self):
        plan = rewrite.plan(
            'SELECT country, SUM(actions) AS "Total Actions" '
            'FROM public.kaveon_events_enriched GROUP BY country '
            'ORDER BY country LIMIT 500'
        )
        self.assertEqual(plan.sort_keys, ((0, False),))
        self.assertEqual(plan.limit, 500)

    def test_a_where_clause_is_carried_through_verbatim(self):
        plan = rewrite.plan(
            "SELECT year, SUM(ghg) AS m FROM climate.energy "
            "WHERE country IN ('China', 'United States') "
            "GROUP BY year ORDER BY m ASC NULLS LAST LIMIT 500"
        )
        self.assertEqual(
            plan.statement,
            "SELECT year, SUM(ghg) AS m FROM climate.energy "
            "WHERE country IN ('China', 'United States') GROUP BY year",
        )
        self.assertEqual(plan.sort_keys, ((1, False),))

    def test_repeated_aggregate_expression_resolves_to_its_projection(self):
        plan = rewrite.plan(
            "SELECT region, SUM(actions) FROM public.events "
            "GROUP BY region ORDER BY sum(ACTIONS) DESC"
        )
        self.assertEqual(plan.sort_keys, ((1, True),))
        self.assertIsNone(plan.limit)

    def test_exact_distinct_count_breakdown_is_accepted(self):
        plan = rewrite.plan(
            'SELECT industry, COUNT(DISTINCT user_id) AS "Users" FROM public.events '
            'GROUP BY industry ORDER BY "Users" DESC NULLS LAST LIMIT 12'
        )
        self.assertEqual(plan.sort_keys, ((1, True),))
        self.assertEqual(plan.limit, 12)

    def test_several_sort_keys_are_kept_in_statement_order(self):
        plan = rewrite.plan(
            "SELECT region, segment, SUM(a) AS v FROM s.t "
            "GROUP BY region, segment ORDER BY v DESC, region ASC LIMIT 9"
        )
        self.assertEqual(plan.sort_keys, ((2, True), (0, False)))

    def test_a_trailing_semicolon_and_lower_case_keywords_are_accepted(self):
        plan = rewrite.plan("select a, sum(b) as v from s.t group by a order by v desc limit 5;")
        self.assertEqual(plan.statement, "select a, sum(b) as v from s.t group by a")
        self.assertEqual(plan.sort_keys, ((1, True),))

    def test_table_alias_is_part_of_the_recognised_from_clause(self):
        plan = rewrite.plan("SELECT t.region, SUM(t.a) AS v FROM s.events AS t "
                            "GROUP BY t.region ORDER BY v DESC LIMIT 4")
        self.assertEqual(plan.statement,
                         "SELECT t.region, SUM(t.a) AS v FROM s.events AS t GROUP BY t.region")

    def test_global_aggregate_drops_a_limit_that_cannot_bind(self):
        # No GROUP BY and every projection an aggregate: exactly one row comes
        # back, so the LIMIT is a no-op and there is nothing to re-apply.
        plan = rewrite.plan('SELECT SUM(actions) AS "Actions" FROM public.events LIMIT 500')
        self.assertEqual(plan.statement, 'SELECT SUM(actions) AS "Actions" FROM public.events')
        self.assertEqual(plan.sort_keys, ())
        self.assertIsNone(plan.limit)
        self.assertFalse(plan.grouped)

    def test_global_aggregate_ordering_is_a_no_op_even_when_unresolvable(self):
        plan = rewrite.plan("SELECT MAX(latency) AS ms FROM public.events "
                            "ORDER BY something_else DESC LIMIT 500")
        self.assertEqual(plan.statement, "SELECT MAX(latency) AS ms FROM public.events")
        self.assertEqual(plan.sort_keys, ())
        self.assertIsNone(plan.limit)

    def test_unordered_limit_keeps_the_caller_s_bound_and_invents_no_order(self):
        plan = rewrite.plan("SELECT region, SUM(actions) FROM public.events "
                            "GROUP BY region LIMIT 10")
        self.assertEqual(plan.sort_keys, ())
        self.assertEqual(plan.limit, 10)


class AverageSubstitutionTests(unittest.TestCase):
    """AVG is issued as the sum and the count the cube can answer from."""

    def test_an_average_breakdown_becomes_a_sum_and_a_count(self):
        plan = rewrite.plan(
            'SELECT surface, AVG(latency_p75_ms) AS "Avg Latency (ms)" '
            'FROM public.kaveon_events_enriched GROUP BY surface '
            'ORDER BY "Avg Latency (ms)" DESC NULLS LAST LIMIT 500'
        )
        self.assertEqual(
            plan.statement,
            'SELECT surface, SUM(latency_p75_ms) AS "__kaveon_avg_sum_1", '
            'COUNT(latency_p75_ms) AS "__kaveon_avg_count_1" '
            'FROM public.kaveon_events_enriched GROUP BY surface',
        )
        self.assertEqual(plan.width, 3)
        self.assertEqual(plan.columns, (
            rewrite._Output(source=0, divisor=None, name=None),
            rewrite._Output(source=1, divisor=2, name="Avg Latency (ms)"),
        ))
        self.assertEqual(plan.sort_keys, ((1, True),))

    def test_a_global_average_becomes_a_sum_and_a_count(self):
        plan = rewrite.plan('SELECT AVG(latency_p75_ms) AS "ms" '
                            'FROM public.kaveon_events_enriched LIMIT 500')
        self.assertEqual(
            plan.statement,
            'SELECT SUM(latency_p75_ms) AS "__kaveon_avg_sum_0", '
            'COUNT(latency_p75_ms) AS "__kaveon_avg_count_0" '
            'FROM public.kaveon_events_enriched',
        )
        self.assertEqual(plan.columns,
                         (rewrite._Output(source=0, divisor=1, name="ms"),))

    def test_an_average_beside_other_aggregates_keeps_every_position(self):
        plan = rewrite.plan(
            "SELECT region, SUM(a) AS s, AVG(b) AS m, COUNT(DISTINCT c) AS d "
            "FROM s.t GROUP BY region ORDER BY m DESC LIMIT 7"
        )
        self.assertEqual(
            plan.statement,
            'SELECT region, SUM(a) AS s, SUM(b) AS "__kaveon_avg_sum_2", '
            'COUNT(b) AS "__kaveon_avg_count_2", COUNT(DISTINCT c) AS d '
            "FROM s.t GROUP BY region",
        )
        self.assertEqual(plan.width, 5)
        self.assertEqual([output.source for output in plan.columns], [0, 1, 2, 4])
        self.assertEqual([output.divisor for output in plan.columns],
                         [None, None, 3, None])
        self.assertEqual(plan.sort_keys, ((2, True),))

    def test_several_averages_are_each_substituted(self):
        plan = rewrite.plan("SELECT k, AVG(a) AS x, AVG(b) AS y FROM s.t "
                            "GROUP BY k ORDER BY y DESC LIMIT 3")
        self.assertEqual(
            plan.statement,
            'SELECT k, SUM(a) AS "__kaveon_avg_sum_1", COUNT(a) AS "__kaveon_avg_count_1", '
            'SUM(b) AS "__kaveon_avg_sum_2", COUNT(b) AS "__kaveon_avg_count_2" '
            "FROM s.t GROUP BY k",
        )
        self.assertEqual(plan.width, 5)
        self.assertEqual([output.divisor for output in plan.columns], [None, 2, 4])
        self.assertEqual(plan.sort_keys, ((2, True),))

    def test_the_average_argument_is_taken_exactly_as_written(self):
        plan = rewrite.plan("SELECT k, AVG( t.latency_p75_ms ) AS m FROM s.t AS t "
                            "GROUP BY k ORDER BY m LIMIT 2")
        self.assertEqual(
            plan.statement,
            'SELECT k, SUM(t.latency_p75_ms) AS "__kaveon_avg_sum_1", '
            'COUNT(t.latency_p75_ms) AS "__kaveon_avg_count_1" '
            "FROM s.t AS t GROUP BY k",
        )

    def test_the_average_output_name_and_position_are_the_caller_s(self):
        plan = rewrite.plan('SELECT k, AVG(a) AS "Avg A", SUM(b) AS "B" FROM s.t '
                            'GROUP BY k ORDER BY "Avg A" DESC LIMIT 2')
        names, rows = rewrite.finish_rows(
            ["k", "__kaveon_avg_sum_1", "__kaveon_avg_count_1", "B"],
            [["x", 10, 4, 99], ["y", 9, 3, 98]], plan)
        self.assertEqual(names, ["k", "Avg A", "B"])
        self.assertEqual(rows, [["y", 3.0, 98], ["x", 2.5, 99]])

    def test_the_substituted_columns_never_reach_the_response(self):
        plan = rewrite.plan("SELECT k, AVG(a) AS m FROM s.t GROUP BY k LIMIT 5")
        names, rows = rewrite.finish_rows(
            ["k", "__kaveon_avg_sum_1", "__kaveon_avg_count_1"],
            [["x", 7, 2]], plan)
        self.assertEqual(names, ["k", "m"])
        self.assertEqual(rows, [["x", 3.5]])

    def test_a_group_with_no_non_null_value_is_null_not_zero(self):
        plan = rewrite.plan("SELECT k, AVG(a) AS m FROM s.t GROUP BY k LIMIT 5")
        _, rows = rewrite.finish_rows(
            ["k", "s", "n"],
            [["empty", None, 0], ["also", 0, 0], ["real", 9, 2]], plan)
        self.assertEqual(rows, [["empty", None], ["also", None], ["real", 4.5]])

    def test_the_quotient_is_true_division_and_is_never_rounded(self):
        plan = rewrite.plan("SELECT k, AVG(a) AS m FROM s.t GROUP BY k LIMIT 5")
        _, rows = rewrite.finish_rows(["k", "s", "n"], [["x", 7, 2], ["y", 1, 3]], plan)
        self.assertEqual(rows[0][1], 3.5)
        self.assertEqual(rows[1][1], 1 / 3)
        self.assertIsInstance(rows[0][1], float)

    def test_the_quotient_matches_the_engine_s_own_average_bit_for_bit(self):
        # Measured on public.kaveon_events_enriched: AVG(latency_p75_ms) against
        # SUM(latency_p75_ms) / COUNT(latency_p75_ms) over the same six groups.
        measured = {
            "API": (23022504115, 84100000, 273.75153525564804),
            "Chart Builder": (96525876823, 84100000, 1147.7512107372177),
            "Chat": (24629988691, 84100000, 292.8655016765755),
            "Dashboard": (71484955523, 84100000, 849.9994711414982),
            "Export": (50273982606, 84100000, 597.7881403804994),
            "SQL Lab": (147175025292, 84100000, 1750.0003007372177),
        }
        plan = rewrite.plan("SELECT surface, AVG(latency_p75_ms) AS m "
                            "FROM public.events GROUP BY surface LIMIT 500")
        _, rows = rewrite.finish_rows(
            ["surface", "s", "n"],
            [[name, total, count] for name, (total, count, _) in measured.items()],
            plan)
        for name, value in rows:
            self.assertEqual(value.hex(), measured[name][2].hex(), name)

    def test_an_average_sort_key_is_ordered_after_the_quotient_is_formed(self):
        # The sums alone rank the groups differently from their means, so an
        # ordering applied before the division would return the wrong rows.
        plan = rewrite.plan("SELECT k, AVG(a) AS m FROM s.t GROUP BY k "
                            "ORDER BY m DESC LIMIT 2")
        _, rows = rewrite.finish_rows(
            ["k", "s", "n"],
            [["big sum, small mean", 1000, 1000],
             ["small sum, big mean", 50, 2],
             ["middling", 90, 9]], plan)
        self.assertEqual(rows, [["small sum, big mean", 25.0], ["middling", 10.0]])

    def test_an_average_sort_key_named_by_its_expression_still_resolves(self):
        plan = rewrite.plan("SELECT k, AVG(a) AS m FROM s.t GROUP BY k "
                            "ORDER BY avg(A) ASC LIMIT 5")
        self.assertEqual(plan.sort_keys, ((1, False),))
        _, rows = rewrite.finish_rows(
            ["k", "s", "n"], [["x", 10, 1], ["y", 4, 2]], plan)
        self.assertEqual(rows, [["y", 2.0], ["x", 10.0]])

    def test_a_null_average_sorts_above_every_value_as_the_engine_ranks_it(self):
        plan = rewrite.plan("SELECT k, AVG(a) AS m FROM s.t GROUP BY k "
                            "ORDER BY m DESC LIMIT 3")
        _, rows = rewrite.finish_rows(
            ["k", "s", "n"], [["has", 4, 2], ["none", None, 0], ["more", 9, 3]], plan)
        self.assertEqual(rows, [["none", None], ["more", 3.0], ["has", 2.0]])

    def test_average_distinct_is_refused(self):
        self.assertIsNone(rewrite.plan("SELECT k, AVG(DISTINCT a) AS m FROM s.t "
                                       "GROUP BY k ORDER BY m LIMIT 5"))
        self.assertIsNone(rewrite.plan("SELECT k, avg( distinct a ) AS m FROM s.t "
                                       "GROUP BY k ORDER BY m LIMIT 5"))

    def test_an_unaliased_average_is_refused(self):
        # The Engine names an unaliased projection positionally (expr_1), which
        # is a convention this module will not depend on reproducing.
        self.assertIsNone(rewrite.plan("SELECT k, AVG(a) FROM s.t GROUP BY k "
                                       "ORDER BY k LIMIT 5"))
        self.assertIsNone(rewrite.plan("SELECT AVG(a) FROM s.t LIMIT 5"))

    def test_an_average_that_is_not_one_column_is_refused(self):
        self.assertIsNone(rewrite.plan("SELECT k, AVG(*) AS m FROM s.t "
                                       "GROUP BY k ORDER BY m LIMIT 5"))
        self.assertIsNone(rewrite.plan("SELECT k, AVG(a, b) AS m FROM s.t "
                                       "GROUP BY k ORDER BY m LIMIT 5"))
        self.assertIsNone(rewrite.plan("SELECT k, AVG() AS m FROM s.t "
                                       "GROUP BY k ORDER BY m LIMIT 5"))

    def test_a_statement_spelling_a_reserved_name_is_refused(self):
        self.assertIsNone(rewrite.plan(
            'SELECT k, SUM(a) AS "__kaveon_avg_sum_1" FROM s.t '
            'GROUP BY k ORDER BY k LIMIT 5'))
        self.assertIsNone(rewrite.plan(
            "SELECT k, SUM(__kaveon_avg_count_0) AS v FROM s.t "
            "GROUP BY k ORDER BY k LIMIT 5"))

    def test_a_count_that_is_not_a_whole_number_is_refused(self):
        plan = rewrite.plan("SELECT k, AVG(a) AS m FROM s.t GROUP BY k LIMIT 5")
        self.assertIsNone(rewrite.finish_rows(["k", "s", "n"], [["x", 1, 2.5]], plan))
        self.assertIsNone(rewrite.finish_rows(["k", "s", "n"], [["x", 1, "2"]], plan))
        self.assertIsNone(rewrite.finish_rows(["k", "s", "n"], [["x", 1, -1]], plan))
        self.assertIsNone(rewrite.finish_rows(["k", "s", "n"], [["x", "1", 2]], plan))

    def test_a_result_that_is_not_as_wide_as_the_statement_is_refused(self):
        plan = rewrite.plan("SELECT k, AVG(a) AS m FROM s.t GROUP BY k LIMIT 5")
        self.assertIsNone(rewrite.finish_rows(["k", "s"], [["x", 1]], plan))
        self.assertIsNone(rewrite.finish_rows(["k", "s", "n"], [["x", 1]], plan))


class DistinctCountDetectionTests(unittest.TestCase):
    """Whether a statement asks for a distinct count, read off its tokens."""

    def test_a_distinct_count_is_recognised_however_it_is_spaced_or_cased(self):
        for sql in [
            'SELECT region, COUNT(DISTINCT user_id) AS "Users" FROM t GROUP BY region',
            "SELECT COUNT( DISTINCT t.user_id ) FROM t",
            "select count(distinct x) from t",
            "SELECT COUNT\n(\n  DISTINCT x\n) FROM t",
            "SELECT SUM(a) AS s, COUNT(DISTINCT b) AS d FROM t WHERE c IN (1, 2)",
            'SELECT COUNT(DISTINCT "user id") FROM t',
        ]:
            self.assertTrue(rewrite.counts_distinct(sql), sql)

    def test_a_statement_without_one_is_not_recognised(self):
        for sql in [
            "SELECT region, SUM(actions) FROM t GROUP BY region",
            "SELECT COUNT(x) FROM t",
            "SELECT COUNT(*) FROM t",
            "SELECT DISTINCT region FROM t",
            "SELECT APPROX_COUNT_DISTINCT(x) FROM t",
            "",
        ]:
            self.assertFalse(rewrite.counts_distinct(sql), sql)

    def test_an_identifier_that_merely_contains_the_words_is_not_one(self):
        for sql in [
            "SELECT count_distinct_users FROM t",
            "SELECT my_count_distinct_thing, SUM(a) FROM t GROUP BY my_count_distinct_thing",
            "SELECT counts(distinct_users) FROM t",
            "SELECT distinct_count(x) FROM t",
        ]:
            self.assertFalse(rewrite.counts_distinct(sql), sql)

    def test_the_words_inside_a_literal_are_not_a_distinct_count(self):
        for sql in [
            "SELECT a FROM t WHERE note = 'COUNT(DISTINCT x)'",
            "SELECT 'count(distinct user_id)' AS label, SUM(a) FROM t GROUP BY label",
        ]:
            self.assertFalse(rewrite.counts_distinct(sql), sql)

    def test_the_words_inside_a_quoted_identifier_are_not_a_distinct_count(self):
        self.assertFalse(rewrite.counts_distinct('SELECT "COUNT"("DISTINCT") FROM t'))
        self.assertFalse(rewrite.counts_distinct(
            'SELECT a AS "COUNT(DISTINCT x)" FROM t'))

    def test_a_statement_that_cannot_be_lexed_is_treated_as_exact(self):
        # An exact answer is never the wrong answer, only the slower one, so a
        # statement this module cannot read is left to run exactly.
        for sql in [
            "SELECT a FROM t -- COUNT(DISTINCT x)",
            "SELECT a FROM t /* COUNT(DISTINCT x) */ WHERE b = 1",
            "SELECT COUNT(DISTINCT x) FROM t WHERE y = $$z$$",
            "SELECT COUNT(DISTINCT x) FROM t WHERE y = 'unterminated",
        ]:
            self.assertFalse(rewrite.counts_distinct(sql), sql)

    def test_a_distinct_count_breakdown_is_still_rewritten_for_the_cube(self):
        # Both treatments apply to the same statement: the ordering and limit
        # come off, and the caller runs it under `approximate`.
        plan = rewrite.plan(
            'SELECT region, COUNT(DISTINCT user_id) AS "Users" '
            "FROM public.kaveon_events_enriched GROUP BY region "
            'ORDER BY "Users" DESC NULLS LAST LIMIT 12')
        self.assertEqual(
            plan.statement,
            'SELECT region, COUNT(DISTINCT user_id) AS "Users" '
            "FROM public.kaveon_events_enriched GROUP BY region")
        self.assertTrue(rewrite.counts_distinct(plan.statement))
        self.assertEqual(plan.sort_keys, ((1, True),))
        self.assertEqual(plan.limit, 12)


class RefusedShapeTests(unittest.TestCase):
    """Anything the recognizer is not certain about runs exactly as written."""

    def refuses(self, sql):
        self.assertIsNone(rewrite.plan(sql), sql)

    def test_nothing_to_remove_is_left_alone(self):
        self.refuses("SELECT region, SUM(actions) FROM public.events GROUP BY region")

    def test_ordinal_order_by_is_refused(self):
        # The Engine evaluates an ordinal key as a literal rather than as an
        # output position, so the rows come back unordered and there is no
        # ordering here to reproduce.
        self.refuses("SELECT region, SUM(a) FROM t GROUP BY region ORDER BY 2 DESC")
        self.refuses("SELECT region, SUM(a) FROM t GROUP BY region ORDER BY 1")

    def test_order_by_a_column_that_is_not_projected_is_refused(self):
        self.refuses("SELECT region, SUM(a) FROM t GROUP BY region, country "
                     "ORDER BY country LIMIT 5")

    def test_order_by_an_expression_that_is_not_projected_is_refused(self):
        self.refuses("SELECT region, SUM(a) FROM t GROUP BY region "
                     "ORDER BY SUM(b) DESC LIMIT 5")

    def test_ambiguous_order_by_key_is_refused(self):
        self.refuses("SELECT SUM(a) AS v, SUM(b) AS v, region FROM t "
                     "GROUP BY region ORDER BY v DESC LIMIT 5")

    def test_cte_and_subquery_are_refused(self):
        self.refuses("WITH b AS (SELECT a FROM t) SELECT a, SUM(x) AS v FROM b "
                     "GROUP BY a ORDER BY v LIMIT 5")
        self.refuses("SELECT region, SUM(a) AS v FROM (SELECT * FROM t) x "
                     "GROUP BY region ORDER BY v LIMIT 5")
        self.refuses("SELECT region, SUM(a) AS v FROM t WHERE id IN (SELECT id FROM u) "
                     "GROUP BY region ORDER BY v LIMIT 5")

    def test_set_operations_are_refused(self):
        self.refuses("SELECT region, SUM(a) AS v FROM t GROUP BY region "
                     "UNION SELECT region, SUM(a) AS v FROM u GROUP BY region "
                     "ORDER BY v LIMIT 5")

    def test_joins_are_refused(self):
        self.refuses("SELECT a.region, SUM(b.x) AS v FROM t a JOIN u b ON a.id = b.id "
                     "GROUP BY a.region ORDER BY v LIMIT 5")
        self.refuses("SELECT region, SUM(a) AS v FROM t, u GROUP BY region "
                     "ORDER BY v LIMIT 5")

    def test_select_distinct_is_refused_and_count_distinct_is_not(self):
        self.refuses("SELECT DISTINCT region FROM t ORDER BY region LIMIT 5")
        self.refuses("SELECT ALL region, SUM(a) AS v FROM t GROUP BY region "
                     "ORDER BY v LIMIT 5")
        self.assertIsNotNone(rewrite.plan(
            "SELECT region, COUNT(DISTINCT u) AS v FROM t GROUP BY region "
            "ORDER BY v LIMIT 5"))

    def test_having_window_and_qualify_are_refused(self):
        self.refuses("SELECT region, SUM(a) AS v FROM t GROUP BY region "
                     "HAVING SUM(a) > 0 ORDER BY v LIMIT 5")
        self.refuses("SELECT region, SUM(a) OVER (PARTITION BY region) AS v FROM t "
                     "ORDER BY v LIMIT 5")
        self.refuses("SELECT region, SUM(a) AS v FROM t GROUP BY region "
                     "QUALIFY v > 1 ORDER BY v LIMIT 5")

    def test_grouping_constructs_that_add_rows_are_refused(self):
        self.refuses("SELECT region, SUM(a) AS v FROM t GROUP BY ROLLUP(region) "
                     "ORDER BY v LIMIT 5")
        self.refuses("SELECT region, SUM(a) AS v FROM t GROUP BY CUBE(region) "
                     "ORDER BY v LIMIT 5")

    def test_statements_without_an_aggregate_are_refused(self):
        self.refuses("SELECT region FROM t ORDER BY region LIMIT 5")
        self.refuses("SELECT region, user_id FROM t GROUP BY region, user_id "
                     "ORDER BY region LIMIT 5")
        self.refuses("SELECT SUM(a) AS v, region FROM t ORDER BY v LIMIT 5")

    def test_a_star_projection_is_refused(self):
        self.refuses("SELECT * FROM t ORDER BY region LIMIT 5")
        self.refuses("SELECT region, SUM(a) AS v, b * c AS p FROM t GROUP BY region, b, c "
                     "ORDER BY v LIMIT 5")

    def test_approximate_distinct_counts_are_refused(self):
        # The cube answers it from cell sketches while a scan builds its own,
        # so the two forms need not agree.
        self.refuses("SELECT region, APPROX_COUNT_DISTINCT(u) AS v FROM t "
                     "GROUP BY region ORDER BY v LIMIT 5")

    def test_row_bounds_other_than_a_plain_limit_are_refused(self):
        self.refuses("SELECT region, SUM(a) AS v FROM t GROUP BY region "
                     "ORDER BY v LIMIT 0")
        self.refuses("SELECT region, SUM(a) AS v FROM t GROUP BY region "
                     "ORDER BY v LIMIT 5, 10")
        self.refuses("SELECT region, SUM(a) AS v FROM t GROUP BY region "
                     "ORDER BY v OFFSET 5 LIMIT 5")
        self.refuses("SELECT region, SUM(a) AS v FROM t GROUP BY region "
                     "ORDER BY v FETCH FIRST 5 ROWS ONLY")
        self.refuses("SELECT TOP 5 region, SUM(a) AS v FROM t GROUP BY region "
                     "ORDER BY v")

    def test_more_than_one_statement_is_refused(self):
        self.refuses("SELECT region, SUM(a) AS v FROM t GROUP BY region ORDER BY v; "
                     "DROP TABLE t")

    def test_comments_and_dollar_quoting_are_refused(self):
        self.refuses("SELECT region, SUM(a) AS v FROM t -- note\n"
                     "GROUP BY region ORDER BY v LIMIT 5")
        self.refuses("SELECT region, SUM(a) AS v FROM t /* note */ "
                     "GROUP BY region ORDER BY v LIMIT 5")
        self.refuses("SELECT region, SUM(a) AS v FROM t WHERE x = $$y$$ "
                     "GROUP BY region ORDER BY v LIMIT 5")

    def test_malformed_statements_are_refused(self):
        self.refuses("")
        self.refuses("SELECT region, SUM(a) AS v GROUP BY region ORDER BY v LIMIT 5")
        self.refuses("SELECT region, SUM(a) AS v FROM t GROUP region ORDER BY v LIMIT 5")
        self.refuses("SELECT region, SUM(a FROM t GROUP BY region ORDER BY v LIMIT 5")
        self.refuses("SELECT region, SUM(a) AS v FROM t ORDER BY v GROUP BY region LIMIT 5")
        self.refuses("SELECT region, SUM(a) AS v FROM t GROUP BY region "
                     "ORDER BY v LIMIT 5 LIMIT 6")
        self.refuses("SELECT region, 'unterminated FROM t GROUP BY region ORDER BY v LIMIT 5")

    def test_a_keyword_inside_a_literal_is_not_read_as_structure(self):
        plan = rewrite.plan("SELECT region, SUM(a) AS v FROM t "
                            "WHERE note = 'join union having' GROUP BY region "
                            "ORDER BY v DESC LIMIT 5")
        self.assertEqual(plan.statement,
                         "SELECT region, SUM(a) AS v FROM t "
                         "WHERE note = 'join union having' GROUP BY region")


class OrderingTests(unittest.TestCase):
    """The API-side ordering reproduces what the Engine's Sort/TopN produces."""

    def finish(self, sql, rows):
        plan = rewrite.plan(sql)
        self.assertIsNotNone(plan, sql)
        finished = rewrite.finish_rows(
            [f"c{at}" for at in range(plan.width)], rows, plan)
        return None if finished is None else finished[1]

    def test_numbers_are_ordered_as_numbers_not_as_text(self):
        self.assertEqual(
            self.finish("SELECT region, SUM(a) AS v FROM t GROUP BY region ORDER BY v DESC",
                        [["a", 9], ["b", 100], ["c", 11.5]]),
            [["b", 100], ["c", 11.5], ["a", 9]])

    def test_text_is_ordered_by_code_point_like_arrow(self):
        self.assertEqual(
            self.finish("SELECT provider, MAX(e) AS v FROM t GROUP BY provider "
                        "ORDER BY provider DESC",
                        [["Anthropic", 1], ["xAI", 2], ["OpenAI", 3]]),
            [["xAI", 2], ["OpenAI", 3], ["Anthropic", 1]])

    def test_nulls_rank_above_every_value_in_both_directions(self):
        rows = [["a", 2], ["b", None], ["c", 1]]
        self.assertEqual(
            self.finish("SELECT p, MAX(e) AS v FROM t GROUP BY p ORDER BY v DESC", rows),
            [["b", None], ["a", 2], ["c", 1]])
        self.assertEqual(
            self.finish("SELECT p, MAX(e) AS v FROM t GROUP BY p ORDER BY v ASC", rows),
            [["c", 1], ["a", 2], ["b", None]])

    def test_an_explicit_nulls_clause_is_not_honoured_because_the_engine_ignores_it(self):
        # Measured on the live Engine: DESC NULLS LAST and DESC NULLS FIRST
        # both return nulls first.  Honouring the clause here would change the
        # rows a chart shows rather than preserve them.
        rows = [["a", 2], ["b", None], ["c", 1]]
        for clause in ("NULLS LAST", "NULLS FIRST"):
            self.assertEqual(
                self.finish("SELECT p, MAX(e) AS v FROM t GROUP BY p "
                            "ORDER BY v DESC " + clause, rows),
                [["b", None], ["a", 2], ["c", 1]], clause)

    def test_the_limit_is_applied_after_the_ordering(self):
        self.assertEqual(
            self.finish("SELECT p, SUM(a) AS v FROM t GROUP BY p ORDER BY v DESC LIMIT 2",
                        [["a", 1], ["b", 3], ["c", 2]]),
            [["b", 3], ["c", 2]])

    def test_an_unordered_limit_keeps_the_engine_s_own_row_order(self):
        self.assertEqual(
            self.finish("SELECT p, SUM(a) AS v FROM t GROUP BY p LIMIT 2",
                        [["c", 1], ["a", 3], ["b", 2]]),
            [["c", 1], ["a", 3]])

    def test_several_keys_order_by_the_first_key_first(self):
        self.assertEqual(
            self.finish("SELECT p, q, SUM(a) AS v FROM t GROUP BY p, q "
                        "ORDER BY p ASC, v DESC",
                        [["b", "x", 1], ["a", "y", 1], ["a", "z", 9]]),
            [["a", "z", 9], ["a", "y", 1], ["b", "x", 1]])

    def test_ties_keep_the_order_the_engine_returned(self):
        rows = [["first", 1], ["second", 1], ["third", 1]]
        self.assertEqual(
            self.finish("SELECT p, SUM(a) AS v FROM t GROUP BY p ORDER BY v DESC", rows),
            rows)

    def test_booleans_order_as_booleans(self):
        self.assertEqual(
            self.finish("SELECT flag, SUM(a) AS v FROM t GROUP BY flag ORDER BY flag ASC",
                        [[True, 1], [False, 2], [None, 3]]),
            [[False, 2], [True, 1], [None, 3]])

    def test_a_sort_column_that_cannot_be_ordered_here_gives_up(self):
        sql = "SELECT p, SUM(a) AS v FROM t GROUP BY p ORDER BY v DESC"
        self.assertIsNone(self.finish(sql, [["a", 1], ["b", "two"]]))
        self.assertIsNone(self.finish(sql, [["a", 1], ["b", float("nan")]]))
        self.assertIsNone(self.finish(sql, [["a", True], ["b", 2]]))

    def test_an_all_null_sort_column_is_left_as_the_engine_returned_it(self):
        rows = [["a", None], ["b", None]]
        self.assertEqual(
            self.finish("SELECT p, SUM(a) AS v FROM t GROUP BY p ORDER BY v DESC", rows),
            rows)

    def test_an_empty_result_stays_empty(self):
        self.assertEqual(
            self.finish("SELECT p, SUM(a) AS v FROM t GROUP BY p ORDER BY v DESC LIMIT 5", []),
            [])


class EquivalenceTests(unittest.TestCase):
    """The rewrite plus the API-side finish equals the Engine's own answer."""

    ENGINE_ROWS = [
        ["Africa", 783964414], ["Asia", 1372576329], ["Europe", 1569471524],
        ["North America", 1175746259], ["Oceania", 392284910],
        ["South America", 783957103],
    ]

    def engine_order(self, index, descending):
        """What the Engine returns for one sort key: nulls greatest, stable."""
        keyed = sorted(
            enumerate(self.ENGINE_ROWS),
            key=lambda pair: (pair[1][index] is None, pair[1][index])
            if pair[1][index] is not None else (True, 0),
            reverse=descending,
        )
        return [row for _, row in keyed]

    def test_sorted_and_limited_breakdowns_match_the_engine(self):
        for sql, index, descending, limit in [
            ('SELECT region, SUM(actions) AS "A" FROM public.events GROUP BY region '
             'ORDER BY "A" DESC NULLS LAST LIMIT 500', 1, True, 500),
            ('SELECT region, SUM(actions) AS "A" FROM public.events GROUP BY region '
             'ORDER BY region LIMIT 15', 0, False, 15),
            ('SELECT region, SUM(actions) AS "A" FROM public.events GROUP BY region '
             'ORDER BY "A" ASC LIMIT 3', 1, False, 3),
        ]:
            plan = rewrite.plan(sql)
            self.assertIsNotNone(plan, sql)
            names, rows = rewrite.finish_rows(["region", "A"], self.ENGINE_ROWS, plan)
            self.assertEqual(names, ["region", "A"], sql)
            self.assertEqual(rows, self.engine_order(index, descending)[:limit], sql)


if __name__ == "__main__":
    unittest.main()
