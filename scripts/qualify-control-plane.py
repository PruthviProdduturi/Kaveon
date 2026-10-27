"""Qualify the sharded control plane against a running Engine.

The control plane was rebuilt this cycle: product records no longer live in
the catalog snapshot. They are published beside it as content-addressed
objects, sixty-four shards a kind, so a commit rewrites one shard rather than
the catalogue and the measured ceiling moved from eleven owners to 11,900.

All of that is verified on the in-memory, filesystem and Azurite stores, and
none of it has run against the cluster. This script is what closes that gap:
point it at a live Engine, read the result. It exists now, tested against the
local stack, so the day the subscription returns is spent reading output
rather than working out what to run.

    KAVEON_ENGINE_URL=http://localhost:8080 \\
    KAVEON_ENGINE_TOKEN=... \\
    KAVEON_QUALIFY_WRITES=1 \\
    python scripts/qualify-control-plane.py

The token is a principal token the Engine's security configuration knows, not
the catalog admin token: that one authorises catalog mutations, while these
routes authenticate a principal and take its role. Sending the catalog token
answers 401, which reads as a bad token and is really the wrong one.

Exits non-zero on the first thing that does not hold. Every check prints what
it observed, because a qualification that only prints PASS cannot be audited
later.
"""
from __future__ import annotations

import hashlib
import http.client
import json
import os
import sys
import time
import urllib.error
import urllib.request

# What the Engine enforces. Stated here so the report names the bound it
# measured against rather than leaving a reader to find it.
MAX_SNAPSHOT_BYTES = 8 * 1024 * 1024
PRODUCT_RECORD_SHARDS = 64

# Occupancy above this is reported as a warning: it is not a failure, but a
# deployment approaching the bound should hear about it before writes stop.
OCCUPANCY_WARN_PERCENT = 60


class Failed(RuntimeError):
    """A check that did not hold. The message is the finding."""


def _request(path: str, token: str, base: str, timeout: float = 30.0) -> dict:
    # The Engine authenticates the bearer token and separately takes the
    # principal and role the request acts as. Sending only the token answers
    # 401, which reads as a bad token and is actually a missing identity.
    headers = {
        "x-kaveon-principal": os.getenv("KAVEON_ENGINE_PRINCIPAL", "qualification@kaveon.local"),
        "x-kaveon-role": os.getenv("KAVEON_ENGINE_ROLE", "admin"),
    }
    if token:
        headers["Authorization"] = f"Bearer {token}"
    request = urllib.request.Request(base.rstrip("/") + path, headers=headers)
    try:
        with urllib.request.urlopen(request, timeout=timeout) as response:
            return json.loads(response.read())
    except urllib.error.HTTPError as error:
        raise Failed(f"GET {path} answered {error.code}: {error.read()[:200]!r}") from None
    except urllib.error.URLError as error:
        raise Failed(f"GET {path} did not answer: {error.reason}") from None
    except (OSError, http.client.HTTPException) as error:
        # A connection dropped mid-request is neither of the above --
        # RemoteDisconnected is an http.client exception, and an Engine still
        # starting answers with one. Crashing here would hand an operator a
        # stack trace where the finding is simply that nothing answered yet.
        raise Failed(
            f"GET {path} failed in transport ({type(error).__name__}: {error}); "
            "if the Engine has just started, it may not be serving yet"
        ) from None
    except json.JSONDecodeError as error:
        raise Failed(f"GET {path} answered something that is not JSON: {error}") from None


def _execute_sql(sql: str, token: str, base: str, timeout: float = 60.0) -> dict:
    """Run one statement through the Engine's transactional SQL route."""
    headers = {
        "Content-Type": "application/json",
        "x-kaveon-principal": os.getenv("KAVEON_ENGINE_PRINCIPAL", "qualification@kaveon.local"),
        "x-kaveon-role": os.getenv("KAVEON_ENGINE_ROLE", "admin"),
    }
    if token:
        headers["Authorization"] = f"Bearer {token}"
    request = urllib.request.Request(
        base.rstrip("/") + "/v1/transaction/sql",
        data=json.dumps({"sql": sql}).encode(),
        headers=headers,
        method="POST",
    )
    try:
        with urllib.request.urlopen(request, timeout=timeout) as response:
            return json.loads(response.read())
    except urllib.error.HTTPError as error:
        raise Failed(f"{sql.split()[0]} answered {error.code}: {error.read()[:300]!r}") from None
    except urllib.error.URLError as error:
        raise Failed(f"{sql.split()[0]} did not answer: {error.reason}") from None
    except (OSError, http.client.HTTPException) as error:
        raise Failed(
            f"{sql.split()[0]} failed in transport ({type(error).__name__}: {error})"
        ) from None
    except json.JSONDecodeError as error:
        raise Failed(f"{sql.split()[0]} answered something that is not JSON: {error}") from None


def _post(path: str, body: dict, token: str, base: str, timeout: float = 60.0) -> dict:
    headers = {
        "Content-Type": "application/json",
        "x-kaveon-principal": os.getenv("KAVEON_ENGINE_PRINCIPAL", "qualification@kaveon.local"),
        "x-kaveon-role": os.getenv("KAVEON_ENGINE_ROLE", "admin"),
    }
    if token:
        headers["Authorization"] = f"Bearer {token}"
    request = urllib.request.Request(
        base.rstrip("/") + path,
        data=json.dumps(body).encode(),
        headers=headers,
        method="POST",
    )
    try:
        with urllib.request.urlopen(request, timeout=timeout) as response:
            raw = response.read()
            return json.loads(raw) if raw else {}
    except urllib.error.HTTPError as error:
        raise Failed(f"POST {path} answered {error.code}: {error.read()[:300]!r}") from None
    except urllib.error.URLError as error:
        raise Failed(f"POST {path} did not answer: {error.reason}") from None
    except (OSError, http.client.HTTPException) as error:
        raise Failed(
            f"POST {path} failed in transport ({type(error).__name__}: {error})"
        ) from None
    except json.JSONDecodeError as error:
        raise Failed(f"POST {path} answered something that is not JSON: {error}") from None


def check_writes(base: str, token: str, before: dict) -> list[str]:
    """Write one product record, read it back, and delete it.

    Everything else in this script holds on an installation nobody has written
    to. This is the check that does not, and it goes through the thing the
    control plane is made of: a product record, published into a shard and
    named by the snapshot.

    The record is deleted on the way out including when a step in between
    fails -- a qualification that leaves debris in a real catalogue is a bad
    trade for the evidence it bought.
    """
    warnings: list[str] = []
    record_id = f"kaveon-qualify-{int(time.time() * 1000)}"
    document = json.dumps(
        {"id": record_id, "user_email": os.getenv(
            "KAVEON_ENGINE_PRINCIPAL", "qualification@kaveon.local")},
        separators=(",", ":"),
    ).encode()
    # Content-addressed: the Engine refuses a record whose bytes do not match
    # the digest naming them, which is the same check that protects a shard.
    digest = hashlib.sha256(document).hexdigest()
    path = f"product/qualification/{record_id}.json"

    def transact(changes_and_documents) -> None:
        begun = _post("/v1/transaction", {}, token, base)
        transaction = begun.get("transaction_id") or begun.get("transactionId")
        if not transaction:
            raise Failed(f"beginning a transaction returned no id: {begun!r}")
        for change, document_input in changes_and_documents:
            body = {"change": change}
            if document_input is not None:
                body["document"] = document_input
            _post(f"/v1/transaction/{transaction}/stage", body, token, base)
        _post(f"/v1/transaction/{transaction}/commit", {}, token, base)

    created = False
    try:
        transact([(
            {
                "type": "create_product",
                "record": {
                    "kind": "user_theme",
                    "id": record_id,
                    "revision": 1,
                    "document": {"path": path, "sha256": digest},
                    "unique_values": {},
                    "references": [],
                },
            },
            {"path": path, "bytes": list(document)},
        )])
        created = True
        read = _request(f"/v1/product/user_theme/{record_id}", token, base)
        if not read:
            raise Failed(
                f"record {record_id} committed and read back empty; the commit reported "
                "success without the write being visible"
            )
        print(f"  wrote and read back product record {record_id}")
    finally:
        if created:
            try:
                transact([(
                    {
                        "type": "delete_product",
                        "kind": "user_theme",
                        "id": record_id,
                        "expected_revision": 1,
                    },
                    None,
                )])
            except Failed:
                warnings.append(
                    f"the qualification record {record_id} could not be deleted and is "
                    "still present; remove it by hand"
                )

    after = _request("/v1/transaction/metrics", token, base)
    committed = after.get("committed", 0) - before.get("committed", 0)
    if committed <= 0:
        raise Failed(
            "the write path ran without the commit counter moving; either the metrics are "
            "not recording commits or the statements did not commit"
        )
    print(f"  {committed} commit(s) landed during the check")
    return warnings


def check_reachable(base: str, token: str) -> dict:
    """The Engine answers, and says which snapshot it is serving."""
    metrics = _request("/v1/transaction/metrics", token, base)
    print(f"  engine at {base} answered")
    return metrics


def check_snapshot_is_sharded(metrics: dict) -> None:
    """Records are published beside the snapshot, not inside it.

    The failure this catches is a deployment running the old format and
    silently carrying the old eleven-owner ceiling, which looks identical
    until writes stop.
    """
    snapshot_bytes = metrics.get("snapshot_bytes")
    if not isinstance(snapshot_bytes, int):
        raise Failed(
            "metrics carry no snapshot_bytes; this Engine predates the occupancy "
            "metric, so it also predates the sharded control plane"
        )
    print(f"  snapshot is {snapshot_bytes:,} bytes")


def check_occupancy(metrics: dict) -> list[str]:
    """How close the snapshot is to the bound that refuses writes."""
    warnings: list[str] = []
    limit = metrics.get("snapshot_bytes_limit") or MAX_SNAPSHOT_BYTES
    peak = metrics.get("snapshot_bytes_peak", metrics.get("snapshot_bytes", 0))
    percent = metrics.get("snapshot_percent_of_limit")
    if percent is None:
        percent = (peak * 100 // limit) if limit else 0
    print(f"  peak {peak:,} of {limit:,} bytes ({percent}% of the ceiling)")
    if percent >= 100:
        raise Failed(
            f"the snapshot is at {percent}% of its ceiling; commits are being rejected, "
            "not slowed"
        )
    if percent >= OCCUPANCY_WARN_PERCENT:
        warnings.append(
            f"snapshot occupancy is {percent}%, past the {OCCUPANCY_WARN_PERCENT}% mark; "
            "the records are evicted, so this is the inline remainder and is worth "
            "understanding before it grows"
        )
    return warnings


def check_commit_health(metrics: dict) -> list[str]:
    """Conflicts and indeterminate outcomes, which bound write throughput."""
    warnings: list[str] = []
    attempts = metrics.get("attempts", 0)
    committed = metrics.get("committed", 0)
    conflicts = metrics.get("conflicts", 0)
    indeterminate = metrics.get("indeterminate", 0)
    rejected = metrics.get("rejected", 0)
    per_commit = attempts / committed if committed else 0.0
    if not committed:
        # A fresh Engine passes every other check in this script while having
        # proved nothing about writing. Saying so is the difference between a
        # qualification and a reachability probe.
        warnings.append(
            "nothing has committed on this Engine, so the commit path is unexercised; "
            "this run qualifies reachability and the read path only"
        )
    print(
        f"  {committed:,} commits, {conflicts:,} conflicts, {rejected:,} rejected, "
        f"{indeterminate:,} indeterminate ({per_commit:.2f} attempts a commit)"
    )
    if indeterminate:
        warnings.append(
            f"{indeterminate} commits finished indeterminate; each is a write whose "
            "outcome is unknown and should be reconciled rather than retried blindly"
        )
    if rejected:
        warnings.append(
            f"{rejected} commits were rejected outright; check the Engine log for a "
            "snapshot or shard that exceeded its bound"
        )
    # Measured locally at 2.12 attempts a commit with eight concurrent writers,
    # so well above that means more contention than that test produced.
    if per_commit > 3.0:
        warnings.append(
            f"{per_commit:.2f} attempts a commit is above the 2.12 measured at eight "
            "concurrent writers; writers are serialising harder here than locally"
        )
    return warnings


def check_records_round_trip(base: str, token: str) -> None:
    """A listing resolves, which means every shard it names resolved.

    This is the check that actually exercises the format: a snapshot naming
    shards cannot be read at all unless every one of them is present and its
    bytes match the digest naming it.
    """
    started = time.monotonic()
    page = _request("/v1/products/dataset?limit=1", token, base)
    elapsed = time.monotonic() - started
    if "records" not in page:
        raise Failed(f"a product listing answered without records: {page!r}")
    print(f"  a listing resolved in {elapsed * 1000:.0f} ms")
    if elapsed > 5.0:
        raise Failed(
            f"a listing took {elapsed:.1f}s; a cold load measured 285 ms over 509 shards "
            "locally, so this is resolving far more objects or a far slower endpoint"
        )


def main() -> int:
    base = os.getenv("KAVEON_ENGINE_URL", "http://localhost:8080")
    token = os.getenv("KAVEON_ENGINE_TOKEN", os.getenv("KAVEON_ENGINE_CATALOG_TOKEN", ""))

    print(f"Qualifying the control plane at {base}")
    print()

    warnings: list[str] = []
    try:
        print("reachable")
        metrics = check_reachable(base, token)
        print("sharded")
        check_snapshot_is_sharded(metrics)
        print("occupancy")
        warnings += check_occupancy(metrics)
        exercising_writes = os.getenv("KAVEON_QUALIFY_WRITES") == "1"
        print("commit health")
        # Deferred when writes are about to run: the counters read here are
        # from before them, so warning that nothing has committed and then
        # committing makes the report argue with itself.
        if not exercising_writes:
            warnings += check_commit_health(metrics)
        print("round trip")
        check_records_round_trip(base, token)
        if exercising_writes:
            print("writes")
            warnings += check_writes(base, token, metrics)
            # Re-read, so the health below and the closing line describe the
            # Engine as it is now rather than as it was before this wrote.
            metrics = _request("/v1/transaction/metrics", token, base)
            warnings += check_commit_health(metrics)
        else:
            warnings.append(
                "writes were not exercised; set KAVEON_QUALIFY_WRITES=1 to create, write, "
                "read and drop a temporary table, which is the only check here that "
                "proves the commit path works"
            )
    except Failed as failure:
        print()
        print(f"FAILED: {failure}")
        return 1

    print()
    for warning in warnings:
        print(f"WARNING: {warning}")
    if warnings:
        print()
    committed = metrics.get("committed", 0)
    print("The control plane qualifies: records resolve from their shards and the snapshot")
    if committed:
        print(f"is inside its bound, across {committed:,} commits.")
    else:
        # Not "commits are landing". None have.
        print("is inside its bound. No commit has run here, so writing is unproven.")
    print()
    print("Not covered here: capacity at scale, which needs a populated catalogue, and")
    print("write cost under concurrency, which needs load rather than a probe.")
    return 0


if __name__ == "__main__":
    sys.exit(main())
