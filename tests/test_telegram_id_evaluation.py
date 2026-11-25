"""Синтетические oracle для offline ID evaluation; требуют analytical env."""
import importlib.util
import pathlib
import unittest

SPEC = importlib.util.spec_from_file_location('id_eval', pathlib.Path(__file__).resolve().parents[1]/'scripts/evaluate_telegram_id_risk.py')
module = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(module)


class EvaluationTests(unittest.TestCase):
    def row(self, user, label, source, date='2026-09-01', bot=False):
        return dict(user_id=user,label=label,label_source=source,is_bot=bot,
                    is_spammer_projection=label=='spam',first_authored_at=date+'T00:00:00Z',author_messages=10)

    def test_deduplicates_authors_and_keeps_label_provenance(self):
        rows=[self.row(100,'spam','owner_manual'),self.row(100,'spam','owner_manual','2026-10-01'),
              self.row(101,'spam','system'),self.row(102,'spam','legacy_backfill'),
              self.row(103,'not_spam','owner_review'),self.row(104,'not_spam','system'),
              self.row(105,'spam','owner_manual',bot=True)]
        accounts,excluded=module.prepare(rows)
        self.assertEqual(len(accounts),5)
        self.assertEqual({r['user_id']:r['label'] for r in accounts},
                         {100:'spam',101:'unreviewed_spam',102:'legacy_spam',103:'not_spam',104:'proxy_normal'})
        self.assertEqual(accounts[0]['observed_date'],'2026-09-01')
        self.assertEqual(excluded['technical_or_bot_rows'],1)

    def test_conflicting_manual_labels_are_not_silently_spam(self):
        rows=[self.row(100,'spam','owner_manual'),self.row(100,'not_spam','owner_review')]
        self.assertEqual(module.prepare(rows)[0][0]['label'],'conflicting_manual')
        self.assertEqual(module.prepare(rows, [100])[0],[])

    def test_temporal_prediction_and_tie_aware_auc_have_independent_oracles(self):
        np=module.np
        p=module.predict(np.array([8.,8.,8.1]),np.array([0,100,100]),[0,1,2,8,.001])
        self.assertAlmostEqual(p[0],.5)
        self.assertLess(p[1],.5)
        self.assertAlmostEqual(p[2],.5)
        same=[dict(user_id=8_000_000_000,observed_date='2026-09-21',label=label) for label in ['spam','not_spam']]
        self.assertEqual(module.metrics(same,[0,1,2,8,0])['auc'],.5)

    def test_temporal_fit_recovers_a_moving_synthetic_boundary(self):
        rows=[]
        for day in [-100,0,100]:
            for x in module.np.linspace(7.7,8.3,41):
                rows.append(dict(user_id=round(x*1e9),
                     observed_date=(module.REFERENCE_DATE+module.dt.timedelta(days=day)).isoformat(),
                     label='spam' if x>8+day*.001 else 'not_spam'))
        p=module.fit(rows,[0,1,2,8,0],True)
        self.assertGreater(p[4],.0005)
        self.assertLess(p[4],.0015)


if __name__=='__main__':
    unittest.main()
