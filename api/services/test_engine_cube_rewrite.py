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
        plan = rewrite.plan("SELECT AVG(latency) AS ms FROM public.events "
                            "ORDER BY something_else DESC LIMIT 500")
        self.assertEqual(plan.statement, "SELECT AVG(latency) AS ms FROM public.events")
        self.assertEqual(plan.sort_keys, ())
        self.assertIsNone(plan.limit)

    def test_unordered_limit_keeps_the_caller_s_bound_and_invents_no_order(self):
        plan = rewrite.plan("SELECT region, SUM(actions) FROM public.events "
                            "GROUP BY region LIMIT 10")
        self.assertEqual(plan.sort_keys, ())
        self.assertEqual(plan.limit, 10)


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

    def plan(self, sql):
        plan = rewrite.plan(sql)
        self.assertIsNotNone(plan, sql)
        return plan

    def test_numbers_are_ordered_as_numbers_not_as_text(self):
        plan = self.plan("SELECT region, SUM(a) AS v FROM t GROUP BY region ORDER BY v DESC")
        rows = [["a", 9], ["b", 100], ["c", 11.5]]
        self.assertEqual(rewrite.order_rows(rows, plan),
                         [["b", 100], ["c", 11.5], ["a", 9]])

    def test_text_is_ordered_by_code_point_like_arrow(self):
        plan = self.plan("SELECT provider, MAX(e) AS v FROM t GROUP BY provider "
                         "ORDER BY provider DESC")
        rows = [["Anthropic", 1], ["xAI", 2], ["OpenAI", 3]]
        self.assertEqual(rewrite.order_rows(rows, plan),
                         [["xAI", 2], ["OpenAI", 3], ["Anthropic", 1]])

    def test_nulls_rank_above_every_value_in_both_directions(self):
        descending = self.plan("SELECT p, MAX(e) AS v FROM t GROUP BY p ORDER BY v DESC")
        ascending = self.plan("SELECT p, MAX(e) AS v FROM t GROUP BY p ORDER BY v ASC")
        rows = [["a", 2], ["b", None], ["c", 1]]
        self.assertEqual(rewrite.order_rows(rows, descending),
                         [["b", None], ["a", 2], ["c", 1]])
        self.assertEqual(rewrite.order_rows(rows, ascending),
                         [["c", 1], ["a", 2], ["b", None]])

    def test_an_explicit_nulls_clause_is_not_honoured_because_the_engine_ignores_it(self):
        # Measured on the live Engine: DESC NULLS LAST and DESC NULLS FIRST
        # both return nulls first.  Honouring the clause here would change the
        # rows a chart shows rather than preserve them.
        rows = [["a", 2], ["b", None], ["c", 1]]
        for clause in ("NULLS LAST", "NULLS FIRST"):
            plan = self.plan("SELECT p, MAX(e) AS v FROM t GROUP BY p "
                             "ORDER BY v DESC " + clause)
            self.assertEqual(rewrite.order_rows(rows, plan),
                             [["b", None], ["a", 2], ["c", 1]], clause)

    def test_the_limit_is_applied_after_the_ordering(self):
        plan = self.plan("SELECT p, SUM(a) AS v FROM t GROUP BY p ORDER BY v DESC LIMIT 2")
        rows = [["a", 1], ["b", 3], ["c", 2]]
        self.assertEqual(rewrite.order_rows(rows, plan), [["b", 3], ["c", 2]])

    def test_an_unordered_limit_keeps_the_engine_s_own_row_order(self):
        plan = self.plan("SELECT p, SUM(a) AS v FROM t GROUP BY p LIMIT 2")
        rows = [["c", 1], ["a", 3], ["b", 2]]
        self.assertEqual(rewrite.order_rows(rows, plan), [["c", 1], ["a", 3]])

    def test_several_keys_order_by_the_first_key_first(self):
        plan = self.plan("SELECT p, q, SUM(a) AS v FROM t GROUP BY p, q "
                         "ORDER BY p ASC, v DESC")
        rows = [["b", "x", 1], ["a", "y", 1], ["a", "z", 9]]
        self.assertEqual(rewrite.order_rows(rows, plan),
                         [["a", "z", 9], ["a", "y", 1], ["b", "x", 1]])

    def test_ties_keep_the_order_the_engine_returned(self):
        plan = self.plan("SELECT p, SUM(a) AS v FROM t GROUP BY p ORDER BY v DESC")
        rows = [["first", 1], ["second", 1], ["third", 1]]
        self.assertEqual(rewrite.order_rows(rows, plan), rows)

    def test_booleans_order_as_booleans(self):
        plan = self.plan("SELECT flag, SUM(a) AS v FROM t GROUP BY flag ORDER BY flag ASC")
        rows = [[True, 1], [False, 2], [None, 3]]
        self.assertEqual(rewrite.order_rows(rows, plan), [[False, 2], [True, 1], [None, 3]])

    def test_a_sort_column_that_cannot_be_ordered_here_gives_up(self):
        plan = self.plan("SELECT p, SUM(a) AS v FROM t GROUP BY p ORDER BY v DESC")
        self.assertIsNone(rewrite.order_rows([["a", 1], ["b", "two"]], plan))
        self.assertIsNone(rewrite.order_rows([["a", 1], ["b", float("nan")]], plan))
        self.assertIsNone(rewrite.order_rows([["a", True], ["b", 2]], plan))

    def test_an_all_null_sort_column_is_left_as_the_engine_returned_it(self):
        plan = self.plan("SELECT p, SUM(a) AS v FROM t GROUP BY p ORDER BY v DESC")
        rows = [["a", None], ["b", None]]
        self.assertEqual(rewrite.order_rows(rows, plan), rows)

    def test_an_empty_result_stays_empty(self):
        plan = self.plan("SELECT p, SUM(a) AS v FROM t GROUP BY p ORDER BY v DESC LIMIT 5")
        self.assertEqual(rewrite.order_rows([], plan), [])


class EquivalenceTests(unittest.TestCase):
    """The rewrite plus the API-side ordering equals the Engine's own answer."""

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
            self.assertEqual(rewrite.order_rows(self.ENGINE_ROWS, plan),
                             self.engine_order(index, descending)[:limit], sql)


if __name__ == "__main__":
    unittest.main()
