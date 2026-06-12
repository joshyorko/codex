#!/usr/bin/env python3
"""Stamp the Rust workspace version for tap-release packaging.

Upstream main keeps the workspace at 0.0.0. Release tags carry real semver.
The tap-release branch builds snapshots, so packaging must stamp a real release
semver before `cargo build --locked` embeds the CLI version.
"""

from __future__ import annotations

from pathlib import Path
import re
import sys


REPO_ROOT = Path(__file__).resolve().parents[1]
CARGO_TOML = REPO_ROOT / "codex-rs" / "Cargo.toml"
CARGO_LOCK = REPO_ROOT / "codex-rs" / "Cargo.lock"
SEMVER = re.compile(r"^\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?$")


def stamp_workspace_manifest(version: str) -> None:
    text = CARGO_TOML.read_text(encoding="utf-8")
    workspace_match = re.search(
        r"(?ms)^(\[workspace\.package\]\n)(.*?)(?=^\[|\Z)",
        text,
    )
    if not workspace_match:
        raise RuntimeError("missing [workspace.package] in codex-rs/Cargo.toml")

    body = workspace_match.group(2)
    stamped_body, replacements = re.subn(
        r'(?m)^version = "[^"]+"$',
        f'version = "{version}"',
        body,
        count=1,
    )
    if replacements != 1:
        raise RuntimeError("missing workspace.package version in codex-rs/Cargo.toml")

    text = (
        text[: workspace_match.start(2)] + stamped_body + text[workspace_match.end(2) :]
    )
    CARGO_TOML.write_text(text, encoding="utf-8")


def stamp_lockfile(version: str) -> int:
    text = CARGO_LOCK.read_text(encoding="utf-8")
    parts = text.split("\n[[package]]\n")
    stamped = [parts[0]]
    replacements = 0

    for block in parts[1:]:
        if '\nsource = "' not in block:
            block, count = re.subn(
                r'(?m)^version = "0\.0\.0"$',
                f'version = "{version}"',
                block,
                count=1,
            )
            replacements += count
        stamped.append(block)

    CARGO_LOCK.write_text("\n[[package]]\n".join(stamped), encoding="utf-8")
    return replacements


def main() -> int:
    if len(sys.argv) != 2:
        print(
            "usage: scripts/stamp_rust_workspace_version.py <semver>",
            file=sys.stderr,
        )
        return 2

    version = sys.argv[1].strip()
    if not SEMVER.fullmatch(version):
        print(f"invalid semver for Cargo workspace version: {version}", file=sys.stderr)
        return 2

    stamp_workspace_manifest(version)
    lock_replacements = stamp_lockfile(version)
    if lock_replacements == 0:
        print("no local 0.0.0 packages found in codex-rs/Cargo.lock", file=sys.stderr)
        return 1

    print(
        f"Stamped codex-rs workspace version {version} ({lock_replacements} lock packages)"
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
