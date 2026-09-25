"""Operator entrypoint for the bounded query-history retirement replay."""

from run_system_authority_replay import main
import sys

sys.stdout.reconfigure(line_buffering=True)
sys.stderr.reconfigure(line_buffering=True)


if __name__ == "__main__":
    main(["--family", "query_history"])
