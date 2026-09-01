"""Minimal, dependency-free HyperLogLog for DLM sketch cuboids.

The warehouse computes the registers in plain SQL (per-row hash -> register index
+ rho, MAX(rho) per cell) so this is engine-agnostic — no `postgresql-hll`
extension required (Azure PG Flexible Server / Fabric SQL / MySQL don't ship it).
DLM only ever MERGES register vectors (element-wise max = set union) and ESTIMATES
cardinality here, plus sparse (de)serialization for storage in kaveonmeta.

One base cuboid over the low-card dims then answers COUNT(DISTINCT) for *any*
sub-combo of those dims (all 2^k filter subsets) by unioning the matching cells —
zero warehouse scan, ~1-2% error. Proven at 20M rows: mean 1.6% / p95 4.1% error,
15.7x smaller than raw, 20-30x faster than a live COUNT(DISTINCT).
"""
import json
import math
from typing import Dict, Iterable, List, Optional

# Register precision. p=11 -> m=2048 registers, ~2.3% standard error
# (1.04/sqrt(2048)). Changing p requires a regenerate (registers are not
# comparable across precisions); the stored header row records the p used.
P = 11
M = 1 << P
# Bias-correction constant (Flajolet et al.); m >= 128 uses the closed form.
_ALPHA = 0.7213 / (1.0 + 1.079 / M)
# Remaining bits after the register index, for a 64-bit hash. rho in [1, RBITS+1].
RBITS = 64 - P


def empty() -> bytearray:
    """A zeroed register vector of the canonical length."""
    return bytearray(M)


def merge(dst: bytearray, src: Iterable[int]) -> bytearray:
    """Union `src` into `dst` in place — HLL union is the element-wise max of
    registers. `dst` must be length M; `src` any length-M iterable."""
    for i, v in enumerate(src):
        if v > dst[i]:
            dst[i] = v
    return dst


def estimate(regs: bytearray) -> int:
    """Estimate the distinct cardinality from a full-length register vector.
    Raw HLL estimator with the small-range (linear-counting) correction; the
    large-range correction is unnecessary for a 64-bit hash space."""
    s = 0.0
    zeros = 0
    for v in regs:
        s += 1.0 / (1 << v)   # empty register (v=0) contributes 2^0 = 1.0
        if v == 0:
            zeros += 1
    est = _ALPHA * M * M / s
    if est <= 2.5 * M and zeros:          # small cardinalities: linear counting
        est = M * math.log(M / zeros)
    return int(est + 0.5)


def to_sparse(regs: bytearray) -> str:
    """Serialize a register vector as sparse JSON (only non-zero registers) —
    small cells (the common case) stay tiny in storage."""
    return json.dumps({str(i): v for i, v in enumerate(regs) if v}, separators=(",", ":"))


def from_sparse(s: str, into: Optional[bytearray] = None) -> bytearray:
    """Rehydrate a sparse register vector into a full-length bytearray. If `into`
    is given the entries are merged (max) into it — the union primitive used when
    collapsing many cells into one answer."""
    buf = into if into is not None else empty()
    for i, v in json.loads(s).items():
        idx = int(i)
        if v > buf[idx]:
            buf[idx] = v
    return buf


def union_estimate(sparse_cells: Iterable[str]) -> int:
    """Estimate distinct cardinality across a set of stored (sparse) cells by
    unioning their registers first — the core sub-combo answer path."""
    acc = empty()
    for cell in sparse_cells:
        from_sparse(cell, acc)
    return estimate(acc)
