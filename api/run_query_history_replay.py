"""Operator entrypoint for the bounded query-history retirement replay."""

import sys

sys.stdout.reconfigure(line_buffering=True)
sys.stderr.reconfigure(line_buffering=True)
print("query_history replay entrypoint starting", flush=True)

from run_system_authority_replay import main


if __name__ == "__main__":
    main(["--family", "query_history"])
