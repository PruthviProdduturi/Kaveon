"""
Sliding-window rate limiter for SQL execution endpoints.

Backend selection (automatic):
  - If REDIS_URL is set in the environment, uses Redis for cross-worker accuracy.
  - Otherwise falls back to an in-process counter (resets on worker restart;
    fine for single-worker dev, insufficient for multi-worker production).

Usage:
    sql_execute_limiter.check(user_email)   # raises HTTP 429 if over limit
"""

import os
import time
from collections import defaultdict
from threading import Lock

from fastapi import HTTPException


# ── Redis backend ──────────────────────────────────────────────────────────────

def _try_redis(url: str):
    """Return a connected Redis client, or None if unavailable."""
    try:
        import redis  # type: ignore
        client = redis.from_url(url, socket_connect_timeout=2, socket_timeout=2)
        client.ping()
        return client
    except Exception:
        return None


class _RedisRateLimiter:
    """Sliding-window counter backed by Redis sorted sets — safe across workers."""

    def __init__(self, client, max_calls: int, window_seconds: int):
        self._redis = client
        self._max = max_calls
        self._window = window_seconds

    def check(self, key: str) -> None:
        now = time.time()
        window_start = now - self._window
        redis_key = f"ratelimit:{key}"

        pipe = self._redis.pipeline()
        pipe.zremrangebyscore(redis_key, "-inf", window_start)
        pipe.zadd(redis_key, {str(now): now})
        pipe.zcard(redis_key)
        pipe.expire(redis_key, self._window + 1)
        results = pipe.execute()

        count = results[2]
        if count > self._max:
            raise HTTPException(
                status_code=429,
                detail=f"Rate limit exceeded — maximum {self._max} requests per {self._window}s.",
            )


# ── In-process fallback ────────────────────────────────────────────────────────

class _InProcessRateLimiter:
    """Thread-safe sliding-window counter. Resets on worker restart."""

    def __init__(self, max_calls: int, window_seconds: int):
        self._max = max_calls
        self._window = window_seconds
        self._log: dict = defaultdict(list)
        self._lock = Lock()

    def check(self, key: str) -> None:
        now = time.monotonic()
        cutoff = now - self._window
        with self._lock:
            self._log[key] = [t for t in self._log[key] if t > cutoff]
            if len(self._log[key]) >= self._max:
                raise HTTPException(
                    status_code=429,
                    detail=f"Rate limit exceeded — maximum {self._max} requests per {self._window}s.",
                )
            self._log[key].append(now)


# Public class so tests can instantiate their own limiter
SlidingWindowRateLimiter = _InProcessRateLimiter


# ── Factory ────────────────────────────────────────────────────────────────────

def _build_limiter(max_calls: int, window_seconds: int):
    redis_url = os.environ.get("REDIS_URL", "")
    if redis_url:
        client = _try_redis(redis_url)
        if client:
            print(f"[RateLimit] Using Redis backend ({redis_url})")
            return _RedisRateLimiter(client, max_calls, window_seconds)
        print("[RateLimit] Redis unreachable — falling back to in-process counter")
    return _InProcessRateLimiter(max_calls, window_seconds)


# Shared limiter: 120 SQL executions per user per minute
sql_execute_limiter = _build_limiter(max_calls=120, window_seconds=60)
