"""Taking independent product reads at once rather than one after another.

A product listing costs about half a second inside the Engine regardless of
which family it names or how few records it returns, and a point read costs
about the same. That floor is in the coordinator, not in transport — measured
from the API pod, a `/health` round trip is a millisecond — so it cannot be
paid down from here. What can be fixed is paying it repeatedly for reads that
do not depend on one another.

The Library did exactly that. Every list endpoint reads its own family and
then the caller's favorites, and the charts list additionally reads the
dataset-name index; none of the three needs the others' answers, and all three
were taken in turn. Measured in the cluster against the live coordinator:

    GET /api/v1/charts            2.51s   chart 1.36 + favorite 0.53 + dataset 0.62
    GET /api/v1/datasets          1.46s   dataset 0.69 + favorite 0.78
    GET /api/v1/dashboards        1.12s   dashboard 0.58 + favorite 0.53
    GET /api/v1/lab/saved-queries 1.01s   saved_query 0.51 + favorite 0.50

Overlapped, each endpoint costs its slowest read instead of their sum.

The bound matters: a request path that opened a connection per record would
trade one problem for a worse one. These call sites fan out to a handful of
reads, never to a per-record read, and the bound holds them there.
"""

from concurrent.futures import ThreadPoolExecutor
from typing import Callable, TypeVar

T = TypeVar("T")

# How many product reads one request may have in flight. Enough for the widest
# call site (a family, the caller's favorites and the dataset-name index, with
# room to spare), small enough that a burst of requests cannot turn into an
# unreasonable number of concurrent connections to the coordinator.
MAX_CONCURRENT_READS = 4


def gather(*reads: Callable[[], T]) -> list[T]:
    """Run independent product reads concurrently and return them in order.

    A read that raises propagates, so a caller that must fail closed still
    does: these are the reads whose answers make up a response, and a response
    assembled from a listing that failed would be a quietly short list rather
    than an error.
    """
    if not reads:
        return []
    if len(reads) == 1:
        return [reads[0]()]
    with ThreadPoolExecutor(max_workers=min(len(reads), MAX_CONCURRENT_READS),
                            thread_name_prefix="product-read") as pool:
        submitted = [pool.submit(read) for read in reads]
        return [future.result() for future in submitted]
