"""Bounded production worker for ordered PostgreSQL-to-KaveonDB replay."""

import os
import threading
import time
from datetime import datetime, timezone

from services import product_replay


_lock = threading.Lock()
_state = {
    "enabled": False,
    "running": False,
    "last_success_at": None,
    "last_error_at": None,
    "last_error_type": None,
    "last_batch_count": 0,
    "applied_total": 0,
}


def _utc_now() -> str:
    return datetime.now(timezone.utc).isoformat().replace("+00:00", "Z")


def _integer_setting(name: str, default: int, minimum: int, maximum: int) -> int:
    raw = os.getenv(name, str(default))
    try:
        value = int(raw)
    except ValueError as error:
        raise RuntimeError(f"{name} must be an integer") from error
    if not minimum <= value <= maximum:
        raise RuntimeError(f"{name} must be between {minimum} and {maximum}")
    return value


def status() -> dict:
    """Return content-free replay telemetry safe for the health endpoint."""
    with _lock:
        return dict(_state)


def run(stop: threading.Event) -> None:
    """Replay bounded prefixes until stopped, preserving source ordering."""
    batch_size = _integer_setting("KAVEON_PRODUCT_REPLAY_BATCH_SIZE", 50, 1, 100)
    interval = _integer_setting("KAVEON_PRODUCT_REPLAY_INTERVAL_SECONDS", 5, 1, 300)
    with _lock:
        _state.update(enabled=True, running=True)
    try:
        while not stop.is_set():
            try:
                report = product_replay.replay_pending(batch_size)
                count = int(report["count"])
                with _lock:
                    _state.update(
                        last_success_at=_utc_now(), last_batch_count=count,
                        applied_total=int(_state["applied_total"]) + count,
                        last_error_type=None,
                    )
                # Drain a backlog without an artificial delay. Empty/partial
                # batches wait so an idle API does not poll PostgreSQL tightly.
                if count == batch_size:
                    continue
            except Exception as error:
                # The replay function records the exact failed event in the
                # source outbox. Expose only its exception type here.
                with _lock:
                    _state.update(
                        last_error_at=_utc_now(), last_error_type=type(error).__name__,
                        last_batch_count=0,
                    )
            stop.wait(interval)
    finally:
        with _lock:
            _state["running"] = False


def start() -> tuple[threading.Event, threading.Thread] | None:
    """Start one daemon worker when explicitly enabled."""
    if os.getenv("KAVEON_PRODUCT_REPLAY_ENABLED") != "true":
        with _lock:
            _state.update(enabled=False, running=False)
        return None
    # Validate before starting so a bad deployment fails API startup instead
    # of leaving a silently dead daemon thread.
    _integer_setting("KAVEON_PRODUCT_REPLAY_BATCH_SIZE", 50, 1, 100)
    _integer_setting("KAVEON_PRODUCT_REPLAY_INTERVAL_SECONDS", 5, 1, 300)
    stop = threading.Event()
    worker = threading.Thread(
        target=run, args=(stop,), name="kaveon-product-replay", daemon=True
    )
    worker.start()
    return stop, worker
