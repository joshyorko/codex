#!/usr/bin/env python3
"""Contract tests for the fork-only Homebrew tap release workflow."""

from pathlib import Path
import re
import unittest


REPO_ROOT = Path(__file__).resolve().parents[2]
WORKFLOW = REPO_ROOT / ".github" / "workflows" / "tap-release-linux.yml"


class TapReleaseLinuxWorkflowTest(unittest.TestCase):
    def workflow(self) -> str:
        return WORKFLOW.read_text(encoding="utf-8")

    def test_workflow_is_scoped_to_the_tap_release_branch(self) -> None:
        workflow = self.workflow()

        self.assertIn("push:", workflow)
        self.assertRegex(workflow, r"branches:\s*\n\s*-\s*tap-release\b")
        self.assertIn("workflow_dispatch:", workflow)

    def test_workflow_builds_only_the_linux_homebrew_artifact(self) -> None:
        workflow = self.workflow()

        self.assertIn("TARGET: x86_64-unknown-linux-gnu", workflow)
        self.assertIn("CARGO_BUILD_JOBS: \"1\"", workflow)
        self.assertIn("--bin codex --bin bwrap", workflow)
        self.assertIn("codex-release-${version}.tar.gz", workflow)
        self.assertIn("softprops/action-gh-release", workflow)

        forbidden = re.compile(
            r"apple-darwin|windows|macos|npm|dotslash|python-runtime",
            re.IGNORECASE,
        )
        self.assertIsNone(forbidden.search(workflow))

    def test_workflow_uses_actions_cache_and_dispatches_the_tap(self) -> None:
        workflow = self.workflow()

        self.assertIn("actions/cache", workflow)
        self.assertIn("codex-rs/target", workflow)
        self.assertIn("HOMEBREW_TOOLS_PAT", workflow)
        self.assertIn(
            "repos/joshyorko/homebrew-tools/actions/workflows/tap-auto-update.yml/dispatches",
            workflow,
        )
        self.assertIn('"slot_id":"codex-release-daily"', workflow)


if __name__ == "__main__":
    unittest.main()
