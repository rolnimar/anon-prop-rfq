#!/usr/bin/env python3
"""Reject identifying product, author, path, and pre-rename terminology."""

from __future__ import annotations

import re
import subprocess
from pathlib import Path


PATTERNS = {
    "original program crate": re.compile(r"\bonreapp\b", re.IGNORECASE),
    "original token name": re.compile(r"\bonyc\b", re.IGNORECASE),
    "original mechanism name": re.compile(
        r"\bProp[ _-]?AMM\b|\bprop_amm\b|\bPROP_AMM\b"
    ),
    "original program address": re.compile(r"onreuGh"),
    "local absolute path": re.compile(r"/Users/|/home/"),
    "author identity": re.compile(r"Marian|Roln[ií]k", re.IGNORECASE),
}


def main() -> None:
    repo = Path(__file__).resolve().parents[1]
    tracked = subprocess.run(
        ["git", "ls-files", "-z"],
        cwd=repo,
        check=True,
        capture_output=True,
    ).stdout.split(b"\0")
    failures: list[str] = []
    for raw_path in tracked:
        if not raw_path:
            continue
        relative = raw_path.decode()
        if relative == "scripts/check_anonymity.py":
            continue
        path = repo / relative
        try:
            content = path.read_text(encoding="utf-8")
        except UnicodeDecodeError:
            continue
        for label, pattern in PATTERNS.items():
            for line_number, line in enumerate(content.splitlines(), start=1):
                if pattern.search(line):
                    failures.append(f"{relative}:{line_number}: {label}")
    if failures:
        raise SystemExit("anonymity scan failed:\n" + "\n".join(failures))
    print(f"anonymity scan passed for {len(tracked) - 1} tracked files")


if __name__ == "__main__":
    main()
