"""Drive the kaveon shell through a Windows ConPTY and print what a terminal
shows after each step, so the interactive editor can be checked without a
person at the keyboard: the inline suggestion, Ctrl-R, prefix history, a
multi-line paste (a bare line feed reaches the console as Ctrl-Enter and must
be inserted, not run), Ctrl-U/K/A, Ctrl-C.

Windows only. Needs `pip install pywinpty pyte` and a reachable coordinator.

    python engine/qualification/cli_pty_windows.py --cli engine/target/release/kaveon.exe \
        --server http://localhost:8081 --token <admin token>

Exit code 1 when a step's screen does not contain what it should. The
default scenario asserts on the echoed text only (NO_COLOR is set), so the
dim attribute of the suggestion is not verified here.
"""
import argparse
import os
import sys
import tempfile
import time

import pyte
from winpty import PtyProcess

COLS, ROWS = 120, 32

HISTORY = [
    "SELECT 1;",
    "SHOW TABLES IN OpenSource.kaveon_product;",
    "SELECT count(*) FROM OpenSource.kaveon_product.kaveon_events_users;",
]

# (label, keys to send, text the screen must contain afterwards)
SCENARIO = [
    ("typed prefix shows the suggestion", "SEL",
     "SELECT count(*) FROM OpenSource.kaveon_product.kaveon_events_users;"),
    ("Right takes it", "\x1b[C",
     "SELECT count(*) FROM OpenSource.kaveon_product.kaveon_events_users;"),
    ("Ctrl-A then Ctrl-K kills to the end", "\x01\x0b", "kaveon ›\n"),
    ("Ctrl-R opens the search", "\x12", "reverse search"),
    ("a query previews the newest match", "tab", "SHOW TABLES IN OpenSource.kaveon_product;"),
    ("Ctrl-R again with no older match says so", "\x12", "no match"),
    ("Esc puts the draft back", "\x1b", "kaveon ›\n"),
    ("a pasted statement is inserted, not run", "SELECT 1 AS a,\n\t2 AS b;", "    2 AS b;"),
    ("Enter runs it", "\r", "kaveon ›\n"),
    ("Up keeps to the typed prefix", "SH\x1b[A", "SHOW TABLES IN OpenSource.kaveon_product;"),
    ("Ctrl-C clears", "\x03", "kaveon ›\n"),
]


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--cli", required=True, help="path to kaveon.exe")
    parser.add_argument("--server", default="http://localhost:8081")
    parser.add_argument("--token", default=os.environ.get("KAVEON_TOKEN", ""))
    parser.add_argument("--show", action="store_true", help="print every screen")
    args = parser.parse_args()

    history = os.path.join(tempfile.mkdtemp(prefix="kaveon-pty-"), "history")
    with open(history, "w", encoding="utf-8") as handle:
        handle.write("\n".join(HISTORY) + "\n")
    env = dict(os.environ)
    env.update({
        "KAVEON_TOKEN": args.token,
        "NO_COLOR": "1",
        "KAVEON_HISTORY_FILE": history,
    })
    screen = pyte.Screen(COLS, ROWS)
    stream = pyte.ByteStream(screen)
    proc = PtyProcess.spawn([os.path.abspath(args.cli), "--server", args.server], dimensions=(ROWS, COLS), env=env)

    def pump(seconds: float) -> None:
        end = time.time() + seconds
        while time.time() < end:
            try:
                data = proc.read(65536)
            except EOFError:
                return
            if data:
                stream.feed(data.encode("utf-8", "surrogatepass") if isinstance(data, str) else data)
            else:
                time.sleep(0.05)

    def text() -> str:
        return "\n".join(line.rstrip() for line in screen.display) + "\n"

    pump(3.0)
    failures = 0
    for label, keys, expected in SCENARIO:
        proc.write(keys)
        pump(1.2)
        shown = text()
        ok = expected in shown
        failures += not ok
        print(f"{'ok  ' if ok else 'FAIL'} {label}")
        if args.show or not ok:
            for line in shown.splitlines():
                if line.strip():
                    print("    " + line)
    proc.write("exit")
    pump(0.3)
    proc.write("\r")
    pump(1.0)
    try:
        proc.wait()
    except Exception:  # noqa: BLE001 - the process is gone either way
        pass
    return 1 if failures else 0


if __name__ == "__main__":
    sys.exit(main())
