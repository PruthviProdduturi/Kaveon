from cache_slo import digest, percentile


def test_digest_is_stable_for_same_result():
    columns = [{"name": "n", "type": "bigint"}]
    assert digest(columns, [[1], [None]]) == digest(columns, [[1], [None]])
    assert digest(columns, [[1], [None]]) != digest(columns, [[2], [None]])


def test_percentile_uses_nearest_rank_and_handles_empty():
    values = [4, 1, 3, 2]
    assert percentile(values, 50) == 2
    assert percentile(values, 95) == 4
    assert percentile([], 99) is None
