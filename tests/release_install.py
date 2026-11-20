"""Проверки installer-а в temp dirs, без доступа к production путям."""
import os
from pathlib import Path
import subprocess
import tempfile
import unittest

SCRIPT = Path(__file__).resolve().parents[1] / "scripts/install_release_binaries.sh"
BUNDLE = SCRIPT.with_name("release_bundle.sh")
BINARIES = ("tg_ai_bot_teloxide", "nedonews_mcp_http", "chat_db_mcp",
            "probe_new_user_audit", "spam_cascade", "spam_decision", "retry_pending_comments", "reconcile_comment_delivery", "backfill_rich_messages",
            "backfill_post_history_embeddings", "backfill_audit_embeddings",
            "backfill_chat_embeddings",
            "import_telegram_export", "migrate", "provision_mcp_instance_scope",
            "generate_mcp_db_manifest", "validate_instance_config", "probe_ask_route")


class ReleaseInstall(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.root = Path(self.temp.name)
        self.env = {**os.environ, "NEDOBOT_RELEASE_ROOT": str(self.root / "opt"),
                    "NEDOBOT_RELEASE_LOCK": str(self.root / "release.lock"),
                    "NEDOBOT_BACKUP_ROOT": str(self.root / "backups"),
                    "NEDOBOT_CONFIG_ROOT": str(self.root / "etc")}
        config = self.root / "etc"
        config.mkdir()
        for profile in ("llm_profiles.toml", "pvo-llm_profiles.toml"):
            (config / profile).write_text(f"synthetic {profile}")
        self.destinations = [self.root / "opt" / instance / "target/release"
                             for instance in ("tg-ai-bot-teloxide", "nedobot-pvo")]
        for destination in self.destinations:
            destination.mkdir(parents=True)
            for binary in BINARIES:
                (destination / binary).write_text("old")
                (destination / binary).chmod(0o755)
        self.addCleanup(self.temp.cleanup)

    def stage(self, name):
        stage = self.root / name
        stage.mkdir()
        for binary in BINARIES:
            path = stage / binary
            path.write_text(name * 10000)
            path.chmod(0o755)
        return stage

    def invoke(self, stage, *args):
        return subprocess.run(["bash", str(SCRIPT), str(stage), *args], env=self.env,
                              capture_output=True, check=False)

    def test_bundle_matches_installer_after_staging_and_mode_loss(self):
        build = self.stage("built")
        bundle = self.root / "bundle"
        subprocess.run(["bash", str(BUNDLE), "pack", str(build), str(bundle)], check=True)
        for path in bundle.iterdir():
            path.chmod(0o644)
        verify = ["bash", str(bundle / "release_bundle.sh"), "verify", str(bundle)]
        subprocess.run(verify, check=True)
        self.assertEqual(self.invoke(bundle).returncode, 0)
        for destination in self.destinations:
            self.assertEqual((destination / "spam_decision").read_bytes(),
                             (build / "spam_decision").read_bytes())
        (bundle / "spam_decision").unlink()
        # Even a valid checksum file for a shortened bundle cannot bypass inventory.
        sums = bundle / "SHA256SUMS"
        sums.write_text("".join(line for line in sums.read_text().splitlines(keepends=True)
                                if not line.rstrip().endswith("  spam_decision")))
        result = subprocess.run(verify, capture_output=True, check=False)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn(b"does not match installer inventory", result.stderr)

    def test_verified_backup_can_restore_both_previous_installations(self):
        stage = self.stage("candidate")
        result = self.invoke(stage, "--backup")
        self.assertEqual(result.returncode, 0, result.stderr)
        backup, = (self.root / "backups").iterdir()
        subprocess.run(["sha256sum", "-c", "SHA256SUMS"], cwd=backup,
                       check=True, capture_output=True)
        self.assertEqual(backup.stat().st_mode & 0o777, 0o700)
        for destination in self.destinations:
            instance = destination.parents[1].name
            for binary in BINARIES:
                installed = destination / binary
                self.assertEqual(installed.read_bytes(), (stage / binary).read_bytes())
                subprocess.run(["install", "-m", "0755", str(backup / instance / binary),
                                str(installed)], check=True)
                self.assertEqual(installed.read_text(), "old")
        for profile in ("llm_profiles.toml", "pvo-llm_profiles.toml"):
            self.assertEqual((backup / profile).stat().st_mode & 0o777, 0o600)
            self.assertEqual((backup / profile).read_bytes(),
                             (self.root / "etc" / profile).read_bytes())

    def test_backup_failure_changes_neither_instance(self):
        (self.root / "etc" / "pvo-llm_profiles.toml").unlink()
        self.assertNotEqual(self.invoke(self.stage("candidate"), "--backup").returncode, 0)
        for destination in self.destinations:
            self.assertEqual((destination / "tg_ai_bot_teloxide").read_text(), "old")

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
