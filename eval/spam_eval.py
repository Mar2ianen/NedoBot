"""Eval harness v1: baseline текстовых моделей на alt-gnome + нашем корпусе.

НЕ коммитит данные: датасет качается из HF, наш корпус — из prod только
локально. В репозиторий идёт только скрипт и итоговые веса.
"""

import json
import sys

from datasets import load_dataset
from sklearn.feature_extraction.text import TfidfVectorizer
from sklearn.linear_model import LogisticRegression
from sklearn.metrics import classification_report
from sklearn.model_selection import train_test_split

ALT_DATASET = "alt-gnome/telegram-spam"


def load_local(path):
    """Файлы нашего корпуса: записи разделены \\x1f, НЕ коммитятся."""
    import os

    if not os.path.exists(path):
        return []
    with open(path, encoding="utf-8") as handle:
        return [text for text in handle.read().split("\x1f") if text.strip()]


def main() -> None:
    dataset = load_dataset(ALT_DATASET, split="train")
    texts = [row["text"] for row in dataset]
    labels = [int(row["label"]) for row in dataset]
    print(f"alt-gnome: {len(texts)} texts", file=sys.stderr)

    train_texts, test_texts, train_labels, test_labels = train_test_split(
        texts, labels, test_size=0.2, random_state=42, stratify=labels
    )
    vectorizer = TfidfVectorizer(
        analyzer="word",
        ngram_range=(1, 2),
        lowercase=True,
        min_df=3,
        max_features=30000,
        token_pattern=r"(?u)\w+",
    )
    train_matrix = vectorizer.fit_transform(train_texts)
    model = LogisticRegression(max_iter=1000, C=4.0)
    model.fit(train_matrix, train_labels)
    predicted = model.predict(vectorizer.transform(test_texts))
    print(classification_report(test_labels, predicted, digits=4))
    print(f"vocab={len(vectorizer.vocabulary_)}", file=sys.stderr)
    our_spam = load_local("eval/our_spam.txt")
    our_ham = load_local("eval/our_ham.txt")
    if our_spam or our_ham:
        domain_texts = our_spam + our_ham
        domain_labels = [1] * len(our_spam) + [0] * len(our_ham)
        domain_predicted = model.predict(vectorizer.transform(domain_texts))
        print("== our corpus (alt-gnome model, zero-shot) ==", file=sys.stderr)
        print(classification_report(domain_labels, domain_predicted, digits=4))
        # Дообучение на объединении: наши примеры с весом, чтобы жанр
        # воронок не утонул в 21k чужих.
        combined_texts = train_texts + our_spam + our_ham
        combined_labels = train_labels + [1] * len(our_spam) + [0] * len(our_ham)
        weights = [1.0] * len(train_texts) + [8.0] * (len(our_spam) + len(our_ham))
        combined_matrix = vectorizer.transform(combined_texts)
        tuned = LogisticRegression(max_iter=1000, C=4.0)
        tuned.fit(combined_matrix, combined_labels, sample_weight=weights)
        tuned_predicted = tuned.predict(vectorizer.transform(domain_texts))
        print("== our corpus (combined model, in-sample) ==", file=sys.stderr)
        print(classification_report(domain_labels, tuned_predicted, digits=4))
        tuned_holdout = tuned.predict(vectorizer.transform(test_texts))
        print("== alt-gnome holdout (combined model) ==", file=sys.stderr)
        print(classification_report(test_labels, tuned_holdout, digits=4))


if __name__ == "__main__":
    main()
