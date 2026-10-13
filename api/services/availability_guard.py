"""Put the Engine back when something has scaled it to nothing.

This cluster serves the platform and also hosts a benchmark campaign that
gives each query engine the nodes to itself by scaling the other to zero. A
campaign that fails, is interrupted, or runs from a checkout without its
restore fix leaves the Engine at zero replicas — and then Studio reports
KaveonDB unavailable, the catalog claims to hold no tables, and the context
banner offers to generate context that already exists. That happened
repeatedly across one day and was put back by hand every time, which is not a
fix.

So the floor is enforced. Every minute the coordinator is raised to at least
one replica and the workers to at least `MIN_WORKERS`.

It only ever scales **up**, and only to the floor, so it does not fight a
deliberate scale above it: a round measuring with three workers keeps three,
and a campaign that scaled the Engine to zero gets one worker back within a
minute. One worker rather than the full set is the point — a round measuring
the other engine should keep the nodes it was given; the platform needs enough
to answer, not enough to be fast.

This is not high availability and not a substitute for the campaign restoring
what it scaled. It bounds an outage to about a minute instead of to whenever
someone notices.

It talks to the Kubernetes API with the pod's own service-account token and
`urllib`, because the alternative was an image with both a shell and kubectl:
the published `kubectl` image is distroless and has no shell, and the Bitnami
tags that did have one have been retired. Running the API image needs nothing
that is not already on the node.
"""

from __future__ import annotations

import json
import os
import ssl
import sys
import urllib.error
import urllib.request

# The two statefulsets and the floor each must not fall below.
FLOORS = (("kaveon-coordinator", 1), ("kaveon-worker", 1))

SERVICE_HOST_KEY = "KUBERNETES_SERVICE_HOST"
SERVICE_PORT_KEY = "KUBERNETES_SERVICE_PORT"
TOKEN_PATH = "/var/run/secrets/kubernetes.io/serviceaccount/token"
CA_PATH = "/var/run/secrets/kubernetes.io/serviceaccount/ca.crt"
NAMESPACE_PATH = "/var/run/secrets/kubernetes.io/serviceaccount/namespace"
TIMEOUT = 20


def _read(path: str) -> str:
    with open(path, encoding="utf-8") as handle:
        return handle.read().strip()


class Kubernetes:
    """The little of the API this needs, over the pod's own credentials."""

    def __init__(self, host=None, port=None, token=None, namespace=None, context=None):
        self.host = host or os.environ.get(SERVICE_HOST_KEY, "")
        self.port = port or os.environ.get(SERVICE_PORT_KEY, "443")
        if not self.host:
            raise RuntimeError(
                f"{SERVICE_HOST_KEY} is not set; this runs as a pod in the cluster")
        self.token = token if token is not None else _read(TOKEN_PATH)
        self.namespace = namespace or _read(NAMESPACE_PATH)
        self.context = context if context is not None else ssl.create_default_context(cafile=CA_PATH)

    def _url(self, path: str) -> str:
        return f"https://{self.host}:{self.port}{path}"

    def _request(self, method: str, path: str, body: bytes | None = None,
                 content_type: str | None = None) -> dict:
        headers = {"Authorization": f"Bearer {self.token}", "Accept": "application/json"}
        if content_type:
            headers["Content-Type"] = content_type
        request = urllib.request.Request(self._url(path), data=body, method=method,
                                         headers=headers)
        with urllib.request.urlopen(request, timeout=TIMEOUT, context=self.context) as response:
            return json.loads(response.read() or b"{}")

    def scale(self, name: str) -> int | None:
        """The statefulset's requested replica count, or None if unreadable."""
        path = (f"/apis/apps/v1/namespaces/{self.namespace}"
                f"/statefulsets/{name}/scale")
        document = self._request("GET", path)
        replicas = (document.get("spec") or {}).get("replicas")
        return replicas if isinstance(replicas, int) else None

    def set_scale(self, name: str, replicas: int) -> int:
        """Set the replica count. Returns what the API reports afterwards."""
        path = (f"/apis/apps/v1/namespaces/{self.namespace}"
                f"/statefulsets/{name}/scale")
        body = json.dumps({"spec": {"replicas": replicas}}).encode("utf-8")
        document = self._request("PATCH", path, body,
                                 content_type="application/merge-patch+json")
        reported = (document.get("spec") or {}).get("replicas")
        return reported if isinstance(reported, int) else replicas


def floors() -> tuple[tuple[str, int], ...]:
    """The floors, with the worker floor overridable by the deployment."""
    raw = os.environ.get("KAVEON_GUARD_MIN_WORKERS", "").strip()
    try:
        workers = int(raw) if raw else 1
    except ValueError:
        workers = 1
    workers = max(1, workers)
    return (("kaveon-coordinator", 1), ("kaveon-worker", workers))


def enforce(client: Kubernetes) -> dict:
    """Raise anything below its floor. Reports what it found and what it did.

    One statefulset that cannot be read or scaled must not stop the other from
    being checked: the platform is more available with a coordinator and no
    workers than with neither, and the failure is reported rather than raised.
    """
    raised, held, failed = [], [], []
    for name, floor in floors():
        try:
            current = client.scale(name)
        except Exception as error:  # noqa: BLE001 - reported, never raised
            failed.append({"statefulset": name, "stage": "read", "error": str(error)})
            continue
        if current is None:
            failed.append({"statefulset": name, "stage": "read",
                           "error": "the scale subresource reported no replica count"})
            continue
        if current >= floor:
            # At or above the floor is someone's decision, not a fault.
            held.append({"statefulset": name, "replicas": current, "floor": floor})
            continue
        try:
            client.set_scale(name, floor)
        except Exception as error:  # noqa: BLE001
            failed.append({"statefulset": name, "stage": "scale", "error": str(error)})
            continue
        raised.append({"statefulset": name, "from": current, "to": floor})
    return {"raised": raised, "held": held, "failed": failed}


def main() -> int:
    """One pass. Prints a line per statefulset; exits non-zero only on failure.

    A pass that changed nothing is a success and says so, because a guard that
    only spoke up on failure would make "the floor is held" indistinguishable
    from "the guard never ran".
    """
    try:
        client = Kubernetes()
    except Exception as error:  # noqa: BLE001
        print(f"cannot reach the Kubernetes API: {error}", file=sys.stderr)
        return 2
    result = enforce(client)
    for entry in result["held"]:
        print(f"{entry['statefulset']}: {entry['replicas']} replicas, "
              f"at or above its floor of {entry['floor']}")
    for entry in result["raised"]:
        print(f"{entry['statefulset']}: {entry['from']} replicas, below its floor; "
              f"raised to {entry['to']}")
    for entry in result["failed"]:
        print(f"{entry['statefulset']}: could not {entry['stage']} — {entry['error']}",
              file=sys.stderr)
    return 1 if result["failed"] else 0


if __name__ == "__main__":
    raise SystemExit(main())
