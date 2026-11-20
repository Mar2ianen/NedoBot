#!/usr/bin/env python3
"""Закрытая offline-проверка ID-модели: account labels, даты, контроль веса.

Зависимости аналитического окружения: numpy, scipy. Сырые ID не выводятся.
Вход — private JSONL export; SQL, сеть и production writes отсутствуют.
"""
import argparse
import collections
import datetime as dt
import hashlib
import json
import math
import os
from pathlib import Path

import numpy as np
from scipy.optimize import minimize

REFERENCE_DATE = dt.date(2026, 9, 21)


def prepare(rows, excluded_ids=()):
    grouped = collections.defaultdict(list)
    excluded = collections.Counter()
    for row in rows:
        if row['is_bot'] or row['user_id'] in excluded_ids:
            excluded['technical_or_bot_rows'] += 1
        else:
            grouped[row['user_id']].append(row)
    accounts = []
    for user, observations in grouped.items():
        manual = [r for r in observations if r['label_source'] in ('owner_manual', 'owner_review')]
        if len({r['label'] for r in manual}) > 1:
            label = 'conflicting_manual'
        elif manual:
            label = manual[0]['label']
        elif any(r['label'] == 'spam' and r['label_source'] == 'legacy_backfill' for r in observations):
            label = 'legacy_spam'
        elif any(r['label'] == 'spam' or r['is_spammer_projection'] for r in observations):
            label = 'unreviewed_spam'
        else:
            label = 'proxy_normal'
        observed = min(r['first_authored_at'] for r in observations)
        accounts.append({'user_id': user, 'label': label, 'observed_date': observed[:10],
                         'max_author_messages': max(r['author_messages'] for r in observations)})
    return accounts, excluded


def predict(ids, days, params):
    floor, ceil, k, midpoint, drift = params
    sigmoid = 1 / (1 + np.exp(-np.clip(k * (ids - midpoint - drift * days), -60, 60)))
    return floor + (ceil - floor) * sigmoid


def arrays(accounts):
    return (np.array([r['user_id'] / 1e9 for r in accounts]),
            np.array([(dt.date.fromisoformat(r['observed_date']) - REFERENCE_DATE).days for r in accounts]),
            np.array([r['label'] == 'spam' for r in accounts], dtype=int))


def fit(accounts, initial, temporal):
    ids, days, labels = arrays(accounts)
    if not len(labels) or labels.min() == labels.max():
        raise ValueError('fit needs both account classes')
    def loss(params):
        p = np.clip(predict(ids, days, params), 1e-9, 1 - 1e-9)
        return float(-np.mean(labels * np.log(p) + (1 - labels) * np.log(1 - p)))
    bounds = [(0, .2), (.8, 1), (.05, 20), (.1, 12), (0, .01) if temporal else (0, 0)]
    results = [minimize(loss, [*initial[:4], d], method='L-BFGS-B', bounds=bounds)
               for d in ([0, .0002, .001] if temporal else [0])]
    valid = [r for r in results if r.success and np.isfinite(r.fun)]
    if not valid:
        raise ValueError('optimizer did not converge')
    return min(valid, key=lambda r: r.fun).x.tolist()


def metrics(accounts, params):
    if not accounts:
        return {'n': 0}
    ids, days, labels = arrays(accounts)
    p = np.clip(predict(ids, days, params), 1e-9, 1 - 1e-9)
    pos, neg = p[labels == 1], p[labels == 0]
    auc = float(np.mean((pos[:, None] > neg) + .5 * (pos[:, None] == neg))) if len(pos) and len(neg) else None
    return {'n': len(labels), 'spam': int(labels.sum()), 'normal': int((1-labels).sum()),
            'log_loss': float(-np.mean(labels*np.log(p)+(1-labels)*np.log(1-p))),
            'brier': float(np.mean((p-labels)**2)), 'auc': auc,
            'thresholds': {str(t): {'tp': int(((p >= t) & (labels == 1)).sum()),
                                   'fp': int(((p >= t) & (labels == 0)).sum())} for t in [.5, .8, .85]}}


def evaluate(rows, runtime, excluded_ids=()):
    accounts, excluded = prepare(rows, excluded_ids)
    model = runtime['settings']['telegram_id']
    original = [model[k] for k in ['floor', 'ceil', 'k', 'midpoint_billion']] + [0]
    # Только ручные положительные account labels. Нормальные proxy выделены явно;
    # отсутствие метки не объявляется проверенной ручной normal annotation.
    selected = [r for r in accounts if r['label'] in ('spam', 'not_spam') or
                (r['label'] == 'proxy_normal' and r['max_author_messages'] >= 5)]
    recent = [r for r in selected if r['observed_date'] >= '2026-06-01']
    train = [r for r in recent if r['observed_date'] < '2026-10-01']
    validation = [r for r in recent if '2026-10-01' <= r['observed_date'] < '2026-11-01']
    test = [r for r in recent if r['observed_date'] >= '2026-11-01']
    fixed = fit(train, original, False)
    temporal = fit(train, original, True)
    candidates = {'deployed_fixed': original, 'refit_fixed': fixed, 'refit_temporal': temporal}
    folds = {name: {split: metrics(sample, params) for split, sample in
                   [('train', train), ('validation', validation), ('test', test),
                    ('explicit_labels_only', [r for r in recent if r['label'] in ('spam', 'not_spam')])]}
             for name, params in candidates.items()}
    label_by_user = {r['user_id']: r['label'] for r in accounts}
    weights = {}
    for weight in [15, 20, 25, 30]:
        tally = collections.defaultdict(lambda: collections.Counter())
        for r in rows:
            if r.get('risk_score') is None or r.get('id_coefficient') is None or r['is_bot'] or r['user_id'] in excluded_ids:
                continue
            coefficient = math.floor(r['id_coefficient'] * weight / 15 + .5)
            score = min(100, r['risk_score'] - r['id_coefficient'] + coefficient)
            group = label_by_user[r['user_id']]
            tally[group]['audit_rows'] += 1
            tally[group]['score_ge_85'] += int(score >= 85)
            tally[group]['new_score_crossings_85'] += int(r['risk_score'] < 85 <= score)
            tally[group]['positive_decision_and_score_ge_85'] += int(r['decision_candidate'] and score >= 85)
        weights[str(weight)] = {k: dict(v) for k, v in tally.items()}
    monthly = []
    for month in sorted({r['observed_date'][:7] for r in accounts}):
        ids = [r['user_id']/1e9 for r in accounts if r['label']=='spam' and r['observed_date'].startswith(month)]
        if len(ids)>=20:
            monthly.append({'month':month,'confirmed_accounts':len(ids),'id_p90_billion_rounded':round(float(np.quantile(ids,.9)),2)})
    return {'confirmed_spam_monthly_frontier':monthly, 'method_version': 'id-eval-time-v1', 'reference_date_utc': REFERENCE_DATE.isoformat(),
            'scipy_method': 'L-BFGS-B', 'raw_rows': len(rows), 'unique_accounts': len(accounts),
            'excluded': dict(excluded), 'labels': dict(collections.Counter(r['label'] for r in accounts)),
            'selected_accounts': len(selected), 'recent_accounts': len(recent),
            'candidate_parameters': {name: dict(zip(['floor','ceil','k','midpoint_billion','midpoint_billion_per_day'], p)) for name,p in candidates.items()},
            'metrics': folds, 'weight_sensitivity_on_persisted_audits': weights,
            'limits': ['Ретроспективный split по первой наблюдаемой дате, не по дате поздней разметки.',
                       'Proxy normal >=5 author messages не являются независимыми ручными normal labels.',
                       'Ручных normal labels мало; case-control вероятности не доказывают population calibration.',
                       'Legacy/system spam исключены из основного fit. Один user ID не пересекает splits.',
                       'Weight sensitivity использует сохранённые audit rows; это не replay исторических action gates.',
                       'Не менять production вес/датированную модель автоматически по этим метрикам.']}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('inputs', nargs='+', type=Path)
    parser.add_argument('--exclude-id', type=int, action='append', default=[])
    parser.add_argument('--runtime-model', required=True, type=Path)
    parser.add_argument('--output', required=True, type=Path)
    args = parser.parse_args()
    rows = [json.loads(line) for path in args.inputs for line in path.read_text().splitlines()]
    result = evaluate(rows, json.loads(args.runtime_model.read_text()), args.exclude_id)
    result['dataset_sha256'] = hashlib.sha256(b''.join(path.read_bytes() for path in args.inputs)).hexdigest()
    with open(args.output, 'x', opener=lambda path, flags: os.open(path, flags, 0o600)) as handle:
        json.dump(result, handle, ensure_ascii=False, indent=2)
        handle.write('\n')
    print(json.dumps({'unique_accounts': result['unique_accounts'], 'labels': result['labels'], 'output': str(args.output)}, ensure_ascii=False))


if __name__ == '__main__':
    main()
