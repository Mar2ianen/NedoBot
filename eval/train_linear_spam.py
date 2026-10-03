"""Обучение word-TF-IDF + LogReg на alt-gnome + нашем корпусе и экспорт весов.

Выход: models/linear_spam_word12_v1.json — единственный артефакт, идущий в репо.
Сырые тексты нашего корпуса НЕ коммитятся. Токенизация обязана совпадать
с teloxide_antispam::nn: lowercase, (?u)\\w+, ngram 1-2, tf raw count,
idf ln((1+n)/(1+df))+1, l2-norm, sigmoid(dot + intercept).
"""

import json
import sys

from datasets import load_dataset
from sklearn.feature_extraction.text import TfidfVectorizer
from sklearn.linear_model import LogisticRegression
from sklearn.metrics import classification_report
from sklearn.model_selection import train_test_split

ALT_DATASET = "alt-gnome/telegram-spam-20251030"
MODEL_VERSION = "linear_spam_word12_v1"
MODEL_PATH = "models/linear_spam_word12_v1.json"


def load_local(path):
    with open(path, encoding="utf-8") as handle:
        return [text for text in handle.read().split("\x1f") if text.strip()]


def main() -> None:
    dataset = load_dataset(ALT_DATASET, split="train")
    alt_texts = [row["text"] for row in dataset]
    alt_labels = [int(row["label"]) for row in dataset]
    our_spam = load_local("eval/our_spam.txt")
    our_ham = load_local("eval/our_ham.txt")
    print(f"alt={len(alt_texts)} ours_spam={len(our_spam)} ours_ham={len(our_ham)}", file=sys.stderr)

    # Честный сплит обеих частей: ALT тоже делится на train/test, иначе
    # метрика holdout невоспроизводима из скрипта.
    alt_train_texts, alt_test_texts, alt_train_labels, alt_test_labels = train_test_split(
        alt_texts, alt_labels, test_size=0.2, random_state=42, stratify=alt_labels
    )
    ours_spam_train, ours_spam_test = train_test_split(our_spam, test_size=0.5, random_state=42)
    ours_ham_train, ours_ham_test = train_test_split(our_ham, test_size=0.5, random_state=42)

    fit_texts = alt_train_texts + ours_spam_train + ours_ham_train
    vectorizer = TfidfVectorizer(
        analyzer="word",
        ngram_range=(1, 2),
        lowercase=True,
        min_df=3,
        max_features=30000,
        token_pattern=r"(?u)\w+",
    )
    vectorizer.fit(fit_texts)
    train_texts = alt_train_texts + ours_spam_train + ours_ham_train
    train_labels = alt_train_labels + [1] * len(ours_spam_train) + [0] * len(ours_ham_train)
    # Наш ham — лучший ham (точный домен), наш спам — золото: вес выше.
    # ALT-часть даёт широту жанров и регуляризацию от переобучения на ~200 образцах.
    weights = (
        [1.0] * len(alt_train_texts)
        + [8.0] * len(ours_spam_train)
        + [2.0] * len(ours_ham_train)
    )
    model = LogisticRegression(max_iter=1000, C=4.0)
    model.fit(vectorizer.transform(train_texts), train_labels, sample_weight=weights)
    print("== alt-gnome holdout ==", file=sys.stderr)
    print(
        classification_report(
            alt_test_labels, model.predict(vectorizer.transform(alt_test_texts)), digits=4
        )
    )

    held_spam = vectorizer.transform(ours_spam_test)
    held_ham = vectorizer.transform(ours_ham_test)
    held_texts = ours_spam_test + ours_ham_test
    held_labels = [1] * len(ours_spam_test) + [0] * len(ours_ham_test)
    print("== held-out ours ==", file=sys.stderr)
    print(
        classification_report(
            held_labels, model.predict(vectorizer.transform(held_texts)), digits=4
        )
    )
    probabilities = model.predict_proba(vectorizer.transform(held_texts))[:, 1]
    for text, label, prob in sorted(
        zip(held_texts, held_labels, probabilities), key=lambda row: row[2]
    )[:5]:
        print(f"LOW p={prob:.3f} label={label} :: {text[:120]}", file=sys.stderr)
    for text, label, prob in sorted(
        zip(held_texts, held_labels, probabilities), key=lambda row: row[2]
    )[-3:]:
        print(f"HIGH p={prob:.3f} label={label} :: {text[:120]}", file=sys.stderr)

    idfs = vectorizer.idf_
    names = vectorizer.get_feature_names_out()
    export = {
        "version": MODEL_VERSION,
        "analyzer": "word_12_lower",
        "vocab": {name: float(idf) for name, idf in zip(names, idfs)},
        "coef": [float(value) for value in model.coef_[0]],
        "intercept": float(model.intercept_[0]),
    }
    with open(MODEL_PATH, "w", encoding="utf-8") as handle:
        json.dump(export, handle, ensure_ascii=False)
    print(f"exported {MODEL_PATH} terms={len(names)}", file=sys.stderr)

    # Паритетные векторы для Rust-тестов: term -> tfidf до нормы и итог p.
    sample = held_texts[0]
    analyzer = vectorizer.build_analyzer()
    print(f"PARITY_TOKENS={analyzer(sample)[:12]}", file=sys.stderr)
    print(f"PARITY_P={model.predict_proba(vectorizer.transform([sample]))[0][1]:.6f}", file=sys.stderr)
    print(f"PARITY_TEXT={sample[:200]}", file=sys.stderr)


if __name__ == "__main__":
    main()
