import json
import tempfile
import unittest
from pathlib import Path

from scripts.validate_public_url_corpus import validate


class PublicCorpusPrivacyTests(unittest.TestCase):
    def test_rejects_non_synthetic_url_host(self):
        self.assert_rejected(lambda row: row.update(canonical_url_identity="https://real.example/path"))

    def test_rejects_token_like_url_query(self):
        self.assert_rejected(lambda row: row.update(canonical_url_identity="https://example.org/?token=abcdefghijklmnopqrstuvwxyz123456"))

    def test_rejects_token_like_strings(self):
        self.assert_rejected(lambda row: row.update(cross_field_assumptions="bot123456:abcdefghijklmnopqrstuvwxyz"))

    def test_rejects_numeric_person_identity(self):
        self.assert_rejected(lambda row: row.update(cross_field_assumptions="user 123456789"))

    def assert_rejected(self, mutation):
        path = Path(__file__).resolve().parents[1] / "testing/corpus/public/synthetic_url_history.jsonl"
        row = json.loads(path.read_text(encoding="utf-8").splitlines()[0])
        mutation(row)
        with tempfile.TemporaryDirectory() as directory:
            fixture = Path(directory) / "bad.jsonl"
            fixture.write_text(json.dumps(row), encoding="utf-8")
            with self.assertRaises(ValueError):
                validate(fixture)


if __name__ == "__main__":
    unittest.main()
