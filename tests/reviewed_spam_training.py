"""Границы offline split: автор и кампания не пересекаются с holdout."""
import importlib.util
from pathlib import Path
import unittest

spec = importlib.util.spec_from_file_location("trainer", Path(__file__).resolve().parents[1] / "eval/train_reviewed_spam.py")
trainer = importlib.util.module_from_spec(spec)
spec.loader.exec_module(trainer)


def row(author, campaign, message="2026-01-01T00:00:00Z", label="2026-01-02T00:00:00Z"):
    return {"example": {"chat_id": -1, "user_id": author, "message_at": message, "label_at": label},
            "exact_campaign": campaign}


class TrainingSplit(unittest.TestCase):
    def test_transitive_campaign_and_author_leakage_is_purged(self):
        rows = [row(1, "a"), row(2, "a"), row(2, "b"), row(3, "b"), row(4, "c")]
        components = trainer.groups(rows)
        self.assertIn([0, 1, 2, 3], components)
        split = trainer.split(rows)
        self.assertEqual(split, trainer.split(rows))
        self.assertEqual(sorted(sum(split.values(), [])), list(range(len(rows))))
        for component in components:
            self.assertEqual(sum(any(i in partition for i in component) for partition in split.values()), 1)

    def test_future_labels_and_cross_cutoff_campaigns_never_train_temporal_model(self):
        rows = [row(1, "a"), row(2, "b", label="2026-03-01T00:00:00Z"),
                row(3, "c"), row(4, "d"), row(5, "a", message="2026-02-01T00:00:00Z")]
        train, test = trainer.temporal_split(rows)
        self.assertNotIn(1, train)
        self.assertNotIn(0, train)
        self.assertNotIn(4, train)
        self.assertTrue(set(train).isdisjoint(test))


if __name__ == "__main__":
    unittest.main()
