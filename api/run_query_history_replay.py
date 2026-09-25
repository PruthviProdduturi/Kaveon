"""Operator entrypoint for the bounded query-history retirement replay."""

from run_system_authority_replay import main


if __name__ == "__main__":
    main(["--family", "query_history"])
