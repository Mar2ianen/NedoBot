import contextlib
import base64
import io
from pathlib import Path
import sys
import tempfile
import unittest

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "scripts"))
from run_cargo_test_checked import run_checked, verify_inventory  # noqa: E402


class CheckedCargoTest(unittest.TestCase):
    def test_inventory_requires_exact_test_and_target(self):
        output = "     Running unittests src/lib.rs (target/debug/probe)\n"
        output += "test module::expected ... ok\n"
        self.assertTrue(verify_inventory(output, "src/lib.rs", ["module::expected"]))
        self.assertFalse(verify_inventory(output, "src/lib.rs", ["module::other"]))
        self.assertFalse(verify_inventory(output, "tests/probe.rs", ["module::expected"]))
        self.assertFalse(verify_inventory(output + "test module::other ... ok\n",
                                          "src/lib.rs", ["module::expected"]))

    def fake_cargo(self, root: Path, output: str, exit_code: int) -> Path:
        cargo = root / "cargo"
        encoded_output = base64.b64encode(output.encode()).decode("ascii")
        cargo.write_text(
            "#!/bin/sh\n"
            f"printf '%s' '{encoded_output}' | base64 -d\n"
            f"exit {exit_code}\n",
            encoding="utf-8",
        )
        cargo.chmod(0o755)
        return cargo

    def run_fake(self, output: str, exit_code: int) -> tuple[int, str]:
        with tempfile.TemporaryDirectory() as temporary:
            cargo = self.fake_cargo(Path(temporary), output, exit_code)
            stdout = io.StringIO()
            with contextlib.redirect_stdout(stdout):
                result = run_checked([str(cargo), "test"])
            return result, stdout.getvalue()

    def test_zero_test_success_is_rejected(self):
        result, output = self.run_fake(
            "running 0 tests\ntest result: ok. 0 passed; 0 failed; 0 ignored;\n", 0
        )
        self.assertEqual(result, 1)
        self.assertIn("0 passed", output)

    def test_real_test_failure_is_not_swallowed_by_a_success_summary(self):
        result, output = self.run_fake(
            "test result: ok. 1 passed; 0 failed; 0 ignored;\n", 23
        )
        self.assertEqual(result, 23)
        self.assertIn("1 passed", output)

    def test_passing_test_summary_is_accepted_and_counted(self):
        result, output = self.run_fake(
            "test result: ok. 2 passed; 0 failed; 0 ignored;\n"
            "test result: ok. 3 passed; 0 failed; 1 ignored;\n",
            0,
        )
        self.assertEqual(result, 0)
        self.assertIn("checked test execution: 5 passed, 0 failed, 1 ignored", output)

    def test_ansi_color_does_not_hide_test_summary(self):
        result, output = self.run_fake(
            "\x1b[32mtest result: ok. 1 passed; 0 failed; 0 ignored;\x1b[0m\n",
            0,
        )
        self.assertEqual(result, 0)
        self.assertIn("checked test execution: 1 passed, 0 failed, 0 ignored", output)


if __name__ == "__main__":
    unittest.main()
