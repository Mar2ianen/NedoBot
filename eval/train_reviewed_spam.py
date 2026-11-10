"""Offline candidate из Rust-токенов ручного корпуса. Никогда не публикует модель."""
import argparse
from collections import Counter
from datetime import datetime
import hashlib
import json
import os
from pathlib import Path
import platform


def private_json(path, value):
    payload = json.dumps(value, ensure_ascii=False, indent=2, allow_nan=False)
    with os.fdopen(os.open(path, os.O_CREAT | os.O_EXCL | os.O_WRONLY, 0o600), "w") as out:
        out.write(payload)


def author(row):
    e = row["example"]
    return e["chat_id"], e["user_id"]


def stamp(value):
    return datetime.fromisoformat(value.replace("Z", "+00:00"))


def groups(rows):
    """Связные компоненты авторов и точных кампаний, без message random split."""
    parent = list(range(len(rows)))

    def root(i):
        while parent[i] != i:
            parent[i] = parent[parent[i]]
            i = parent[i]
        return i

    seen = {}
    for i, row in enumerate(rows):
        # Кампании scoped по chat: инстансы обучаются отдельно.
        for key in [("author", author(row)),
                    ("campaign", (row["example"]["chat_id"], row["exact_campaign"]))]:
            if key in seen:
                parent[root(i)] = root(seen[key])
            else:
                seen[key] = i
    components = {}
    for i in range(len(rows)):
        components.setdefault(root(i), []).append(i)
    return sorted(components.values(), key=lambda indices: indices[0])


def split(rows):
    result = {"train": [], "validation": [], "test": []}
    for component in groups(rows):
        identity = sorted({str(author(rows[i])) for i in component})
        bucket = int(hashlib.sha256("|".join(identity).encode()).hexdigest()[:8], 16) % 10
        result["train" if bucket < 6 else "validation" if bucket < 8 else "test"].extend(component)
    return result


def temporal_split(rows):
    times = sorted(stamp(row["example"]["message_at"]) for row in rows)
    cutoff = times[min(len(times) - 1, len(times) * 4 // 5)]
    train, test = [], []
    for component in groups(rows):
        # Никаких будущих меток или общей кампании по обе стороны cutoff.
        if all(stamp(rows[i]["example"]["label_at"]) < cutoff and
               stamp(rows[i]["example"]["message_at"]) < cutoff for i in component):
            train.extend(component)
        elif all(stamp(rows[i]["example"]["message_at"]) >= cutoff for i in component):
            test.extend(component)
    return train, test


def metrics(rows, indices, probabilities, threshold=0.99):
    decisions = {}
    for i, p in zip(indices, probabilities):
        label = rows[i]["example"]["label"]
        if label not in ("spam", "not_spam"):
            raise ValueError("unsupported author label")
        item = decisions.setdefault(author(rows[i]), [label, False])
        if item[0] != label:
            raise ValueError("conflicting author labels")
        item[1] |= bool(p >= threshold)
    normal = sum(label == "not_spam" for label, _ in decisions.values())
    false_positive = sum(label == "not_spam" and flag for label, flag in decisions.values())
    return {"spam_authors": sum(label == "spam" for label, _ in decisions.values()),
            "normal_authors": normal, "review_spam": sum(label == "spam" and flag for label, flag in decisions.values()),
            "review_normal": false_positive,
            "zero_error_fpr_upper_95": 1 - 0.05 ** (1 / normal) if normal and not false_positive else None}


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("tokens", type=Path)
    parser.add_argument("output", type=Path, help="новая private directory")
    args = parser.parse_args()
    rows = [json.loads(line) for line in args.tokens.read_text().splitlines()]
    if not rows or len({r["example"]["chat_id"] for r in rows}) != 1:
        raise ValueError("require nonempty single-chat input")
    if any(r["preprocessing"] != "unicode-spam-v2" or
           r["example"]["label"] not in ("spam", "not_spam") for r in rows):
        raise ValueError("unsupported tokens or labels")
    args.output.mkdir(mode=0o700, parents=False, exist_ok=False)
    import numpy as np
    import scipy
    from scipy.sparse import hstack
    import sklearn
    from sklearn.feature_extraction.text import CountVectorizer, TfidfTransformer
    from sklearn.linear_model import LogisticRegression

    indices = split(rows)
    y = np.array([int(r["example"]["label"] == "spam") for r in rows])
    report = {"dataset_sha256": hashlib.sha256(args.tokens.read_bytes()).hexdigest(),
              "label_unit": "author_review", "split": {k: len(v) for k, v in indices.items()},
              "environment": {"python": platform.python_version(), "sklearn": sklearn.__version__,
                              "numpy": np.__version__, "scipy": scipy.__version__},
              "auto_promote": False, "auto_enforcement_eligible": False,
              "limitations": ["Author labels are not message-level gold.",
                              "Exact campaigns and authors are grouped; semantic near duplicates can remain.",
                              "Validation selects C; test is untouched until selection.",
                              "An offline candidate is never installed by this script."]}
    train = indices["train"]
    if len(set(y[train])) != 2 or not indices["validation"] or not indices["test"]:
        report["status"] = "insufficient_split"
        private_json(args.output / "report.json", report)
        print(json.dumps({"status": report["status"], "auto_promote": False}))
        return

    def features(fit):
        blocks, all_x = [], []
        for name in ("word", "character"):
            # Токены уже получены тем же Rust preprocessing, что и production.
            vectorizer = CountVectorizer(analyzer=lambda tokens: tokens, lowercase=False, min_df=1)
            values = [r[name] for r in rows]
            counts = vectorizer.fit_transform([values[i] for i in fit])
            transformer = TfidfTransformer()
            transformer.fit(counts)
            all_x.append(transformer.transform(vectorizer.transform(values)))
            blocks.append((vectorizer, transformer))
        return blocks, hstack(all_x).tocsr()

    blocks, x = features(train)
    author_counts = Counter(author(rows[i]) for i in train)
    weights = np.array([1 / author_counts[author(rows[i])] for i in train])
    candidates = []
    for c in [0.1, 0.5, 1.0, 2.0, 4.0, 10.0]:
        model = LogisticRegression(C=c, max_iter=2000, random_state=42)
        model.fit(x[train], y[train], sample_weight=weights)
        validation = metrics(rows, indices["validation"], model.predict_proba(x[indices["validation"]])[:, 1])
        candidates.append((model, c, validation))
    model, c, validation = min(candidates, key=lambda candidate:
                              (candidate[2]["review_normal"], -candidate[2]["review_spam"], candidate[1]))
    report.update(status="candidate_only", selected_c=c, validation=validation,
                  candidates=[{"c": c, "validation": m} for _, c, m in candidates],
                  test=metrics(rows, indices["test"], model.predict_proba(x[indices["test"]])[:, 1]))
    export = {"analyzer": "unicode_word_char_v2", "version": "reviewed-candidate-" + report["dataset_sha256"][:16],
              "preprocessing": "unicode-spam-v2", "intercept": float(model.intercept_[0]),
              # Нулевые supporting scores: candidate не может менять старую калибровку.
              "calibration": {"version": "offline-unapproved", "supporting_threshold": 0.99,
                              "strong_threshold": 0.999, "supporting_score": 0, "strong_score": 0}}
    offset = 0
    for name, (vectorizer, transformer) in zip(("word", "character"), blocks):
        terms = vectorizer.get_feature_names_out()
        export[name] = {"vocab": dict(zip(terms, transformer.idf_.tolist())),
                        "coef": model.coef_[0, offset:offset + len(terms)].tolist(), "weight": 1.0}
        offset += len(terms)
    private_json(args.output / "model.json", export)
    report["model_sha256"] = hashlib.sha256((args.output / "model.json").read_bytes()).hexdigest()
    temporal_train, temporal_test = temporal_split(rows)
    report["temporal"] = {"train": len(temporal_train), "test": len(temporal_test), "status": "insufficient_split"}
    if temporal_test and len(set(y[temporal_train])) == 2:
        _, tx = features(temporal_train)
        tm = LogisticRegression(C=c, max_iter=2000, random_state=42)
        counts = Counter(author(rows[i]) for i in temporal_train)
        tm.fit(tx[temporal_train], y[temporal_train],
               sample_weight=[1 / counts[author(rows[i])] for i in temporal_train])
        report["temporal"].update(status="evaluated", metrics=metrics(rows, temporal_test, tm.predict_proba(tx[temporal_test])[:, 1]))
    # Для независимой проверки Python/Rust inference, без печати текстов/слов.
    private_json(args.output / "parity.json", model.predict_proba(x)[:, 1].tolist())
    private_json(args.output / "report.json", report)
    print(json.dumps({"status": report["status"], "auto_promote": False,
                      "test_normal_authors": report["test"]["normal_authors"]}))


if __name__ == "__main__":
    main()
