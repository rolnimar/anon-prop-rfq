#!/usr/bin/env python3
"""Reject local filesystem paths and personal email addresses."""

from __future__ import annotations

import re
import subprocess
from pathlib import Path


LOCAL_PATH = re.compile(r"(?:/Users|/home)/[^/\s]+/")
EMAIL = re.compile(r"\b[A-Z0-9._%+-]+@([A-Z0-9.-]+\.[A-Z]{2,})\b", re.IGNORECASE)
ALLOWED_EMAIL_DOMAINS = {"example.invalid"}


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
        for line_number, line in enumerate(content.splitlines(), start=1):
            if LOCAL_PATH.search(line):
                failures.append(f"{relative}:{line_number}: local absolute path")
            for match in EMAIL.finditer(line):
                if match.group(1).lower() not in ALLOWED_EMAIL_DOMAINS:
                    failures.append(f"{relative}:{line_number}: personal email address")
    if failures:
        raise SystemExit("anonymity scan failed:\n" + "\n".join(failures))
    print(f"anonymity scan passed for {len(tracked) - 1} tracked files")


if __name__ == "__main__":
    main()
