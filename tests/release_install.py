"""Проверки installer-а в temp dirs, без доступа к production путям."""
import os
from pathlib import Path
import subprocess
import tempfile
import textwrap
import unittest

SCRIPT = Path(__file__).resolve().parents[1] / "scripts/install_release_binaries.sh"
BUNDLE = SCRIPT.with_name("release_bundle.sh")
PRIVACY_POLICY = SCRIPT.with_name("set_privacy_mode_observe.sh")
WORKFLOW = SCRIPT.parents[1] / ".github/workflows/release.yml"
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
            env_file = destination.parent.parent / ".env"
            env_file.write_text(f"synthetic {destination.parent.parent.name}")
            env_file.chmod(0o640)
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

    def restore(self, backup):
        return subprocess.run(["bash", str(SCRIPT), "--restore", str(backup)], env=self.env,
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
        original_inodes = {
            path: path.stat().st_ino
            for destination in self.destinations
            for path in (destination / binary for binary in BINARIES)
        }
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
                self.assertEqual(installed.stat().st_ino, (stage / binary).stat().st_ino)
                self.assertEqual((backup / instance / binary).read_text(), "old")
                self.assertEqual(
                    (backup / instance / binary).stat().st_ino,
                    original_inodes[destination / binary],
                )
                env_file = destination.parent.parent / ".env"
                self.assertEqual((backup / instance / ".env").read_text(),
                                 f"synthetic {instance}")
                self.assertEqual((backup / instance / ".env").stat().st_mode & 0o777, 0o640)
                env_file.write_text("changed after backup")
        for profile in ("llm_profiles.toml", "pvo-llm_profiles.toml"):
            self.assertEqual((backup / profile).stat().st_mode & 0o777, 0o600)
            self.assertEqual((backup / profile).read_bytes(),
                             (self.root / "etc" / profile).read_bytes())
        restored = self.restore(backup)
        self.assertEqual(restored.returncode, 0, restored.stderr)
        for destination in self.destinations:
            instance = destination.parents[1].name
            for binary in BINARIES:
                self.assertEqual((destination / binary).read_text(), "old")
            env_file = destination.parent.parent / ".env"
            self.assertEqual(env_file.read_text(), f"synthetic {instance}")
            self.assertEqual(env_file.stat().st_mode & 0o777, 0o640)

    def test_backup_failure_changes_neither_instance(self):
        (self.root / "etc" / "pvo-llm_profiles.toml").unlink()
        self.assertNotEqual(self.invoke(self.stage("candidate"), "--backup").returncode, 0)
        self.assertEqual(list((self.root / "backups").iterdir()), [])
        for destination in self.destinations:
            self.assertEqual((destination / "tg_ai_bot_teloxide").read_text(), "old")

    def test_rollback_rejects_corrupt_backup_before_changing_installation(self):
        result = self.invoke(self.stage("candidate"), "--backup")
        self.assertEqual(result.returncode, 0, result.stderr)
        backup, = (self.root / "backups").iterdir()
        (backup / "tg-ai-bot-teloxide" / "tg_ai_bot_teloxide").write_text("corrupt")
        restore = self.restore(backup)
        self.assertNotEqual(restore.returncode, 0)
        for destination in self.destinations:
            self.assertEqual((destination / "tg_ai_bot_teloxide").read_text(),
                             "candidate" * 10000)

    def test_rollback_removes_artifacts_that_were_absent_before_install(self):
        missing = self.destinations[0] / "reconcile_comment_delivery"
        missing.unlink()
        result = self.invoke(self.stage("candidate"), "--backup")
        self.assertEqual(result.returncode, 0, result.stderr)
        backup, = (self.root / "backups").iterdir()
        restore = self.restore(backup)
        self.assertEqual(restore.returncode, 0, restore.stderr)
        self.assertFalse(missing.exists())
        self.assertEqual(
            (self.destinations[1] / "reconcile_comment_delivery").read_text(),
            "old",
        )

    def test_privacy_policy_update_is_idempotent_preserves_other_values_and_file_mode(self):
        environment_file = self.root / "opt" / "tg-ai-bot-teloxide" / ".env"
        environment_file.write_text(
            "TOKEN=synthetic-secret\n"
            "NEDOBOT_PRIVACY_MODE_POLICY=reconcile\n"
            "# NEDOBOT_PRIVACY_MODE_POLICY=strict\n"
            "NEDOBOT_PRIVACY_MODE_POLICY = strict\n"
        )
        environment_file.chmod(0o640)
        result = subprocess.run(["bash", str(PRIVACY_POLICY), str(environment_file)],
                                env=self.env, capture_output=True, check=False)
        self.assertEqual(result.returncode, 0, result.stderr)
        expected = (
            "TOKEN=synthetic-secret\n"
            "# NEDOBOT_PRIVACY_MODE_POLICY=strict\n"
            "NEDOBOT_PRIVACY_MODE_POLICY=observe\n"
        )
        self.assertEqual(environment_file.read_text(), expected)
        self.assertEqual(environment_file.stat().st_mode & 0o777, 0o640)
        result = subprocess.run(["bash", str(PRIVACY_POLICY), str(environment_file)],
                                env=self.env, capture_output=True, check=False)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(environment_file.read_text(), expected)
        self.assertNotIn(b"synthetic-secret", result.stdout + result.stderr)

    def test_release_workflow_shell_syntax_and_sequential_restart_rollback_gates(self):
        workflow = WORKFLOW.read_text()
        self.assertIn(
            "dist/set_privacy_mode_observe.sh \"$VPS_USER@$VPS_HOST:$stage/dist/\"",
            workflow,
        )
        start = workflow.index("<<'REMOTE'\n") + len("<<'REMOTE'\n")
        end = workflow.index("\n          REMOTE\n", start)
        remote = textwrap.dedent(workflow[start:end])
        syntax = subprocess.run(["bash", "-n"], input=remote, text=True,
                                capture_output=True, check=False)
        self.assertEqual(syntax.returncode, 0, syntax.stderr)
        for required in (
            "trap rollback_on_failure EXIT",
            'install_release_binaries.sh" --restore "$backup_dir"',
            'bash "$stage_dir/dist/set_privacy_mode_observe.sh"',
            "assert_database_at_expected_migration tg_ai_bot",
            "assert_database_at_expected_migration tg_ai_bot_pvo",
            '[ "$migration_state" != "$expected_migration_version:false" ]',
            "assert_privacy_policy tg-ai-bot-teloxide",
            "assert_privacy_policy nedobot-pvo",
            "deployment_complete=true",
        ):
            self.assertIn(required, remote)
        self.assertNotIn(
            "systemctl restart tg-ai-bot-teloxide nedobot-pvo",
            remote,
        )
        rollout = remote.index("# Keep the two polling services on separate restart/health steps.")
        gate_order = [
            remote.index("systemctl restart tg-ai-bot-teloxide", rollout),
            remote.index("wait_for_database tg_ai_bot tg-ai-bot-teloxide", rollout),
            remote.index("systemctl restart nedobot-pvo", rollout),
            remote.index("wait_for_database tg_ai_bot_pvo nedobot-pvo", rollout),
        ]
        self.assertEqual(gate_order, sorted(gate_order))
        self.assertLess(
            remote.index("assert_database_at_expected_migration tg_ai_bot"),
            remote.index('install_output="$(bash "$stage_dir/dist/install_release_binaries.sh"'),
        )
        self.assertLess(
            remote.index("assert_database_at_expected_migration tg_ai_bot_pvo"),
            remote.index("# Keep the two polling services on separate restart/health steps."),
        )

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
