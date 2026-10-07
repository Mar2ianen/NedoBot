"""Проверки installer-а в temp dirs, без доступа к production путям."""
import os
from pathlib import Path
import subprocess
import tempfile
import unittest

SCRIPT = Path(__file__).resolve().parents[1] / "scripts/install_release_binaries.sh"
BINARIES = ("tg_ai_bot_teloxide", "nedonews_mcp_http", "chat_db_mcp",
            "retry_pending_comments", "reconcile_comment_delivery", "backfill_rich_messages",
            "backfill_post_history_embeddings", "backfill_audit_embeddings",
            "backfill_chat_embeddings",
            "import_telegram_export")


class ReleaseInstall(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.root = Path(self.temp.name)
        self.env = {**os.environ, "NEDOBOT_RELEASE_ROOT": str(self.root / "opt"),
                    "NEDOBOT_RELEASE_LOCK": str(self.root / "release.lock")}
        self.destinations = [self.root / "opt" / instance / "target/release"
                             for instance in ("tg-ai-bot-teloxide", "nedobot-pvo")]
        for destination in self.destinations:
            destination.mkdir(parents=True)
            for binary in BINARIES:
                (destination / binary).write_text("old")
        self.addCleanup(self.temp.cleanup)

    def stage(self, name):
        stage = self.root / name
        stage.mkdir()
        for binary in BINARIES:
            path = stage / binary
            path.write_text(name * 10000)
            path.chmod(0o755)
        return stage

    def invoke(self, stage):
        return subprocess.run(["bash", str(SCRIPT), str(stage)], env=self.env,
                              capture_output=True, check=False)

    def test_missing_binary_changes_neither_instance(self):
        stage = self.stage("incomplete")
        (stage / BINARIES[-1]).unlink()
        self.assertNotEqual(self.invoke(stage).returncode, 0)
        for destination in self.destinations:
            for binary in BINARIES:
                self.assertEqual((destination / binary).read_text(), "old")

    def test_concurrent_installs_keep_one_complete_release_in_both_instances(self):
        stages = [self.stage("alpha"), self.stage("beta")]
        processes = [subprocess.Popen(["bash", str(SCRIPT), str(stage)], env=self.env,
                                      stdout=subprocess.PIPE, stderr=subprocess.PIPE)
                     for stage in stages]
        for process in processes:
            process.communicate(timeout=20)
            self.assertEqual(process.returncode, 0)
        contents = [(destination / binary).read_text()
                    for destination in self.destinations for binary in BINARIES]
        self.assertEqual(len(set(contents)), 1)
        self.assertIn(contents[0], ["alpha" * 10000, "beta" * 10000])
        for destination in self.destinations:
            self.assertFalse(list(destination.glob(".release.*")))


if __name__ == "__main__":
    unittest.main()
