"""Прогон alt-gnome/spam-detector-v0.2 на нашем корпусе (ноутбук, CPU).

Тексты подхватываются из eval/our_spam.txt и eval/our_ham.txt (разделитель
\\x1f, НЕ коммитятся). Печатает precision/recall и худшие ошибки.
"""

import sys

import torch
from sklearn.metrics import classification_report
from transformers import AutoModelForSequenceClassification, AutoTokenizer

MODEL = "alt-gnome/spam-detector-v0.2"
THRESHOLD = 0.5


def load_local(path):
    with open(path, encoding="utf-8") as handle:
        return [text for text in handle.read().split("\x1f") if text.strip()]


def main() -> None:
    spam = load_local("eval/our_spam.txt")
    ham = load_local("eval/our_ham.txt")
    texts = spam + ham
    labels = [1] * len(spam) + [0] * len(ham)
    print(f"spam={len(spam)} ham={len(ham)}", file=sys.stderr)

    tokenizer = AutoTokenizer.from_pretrained(MODEL, trust_remote_code=False)
    model = AutoModelForSequenceClassification.from_pretrained(MODEL, trust_remote_code=False)
    model.eval()
    id2label = {int(key): value for key, value in model.config.id2label.items()}
    print(f"id2label={id2label}", file=sys.stderr)
    spam_label_ids = [
        index for index, name in id2label.items() if "spam" in name.lower()
    ]
    if len(spam_label_ids) == 1:
        spam_index = spam_label_ids[0]
    elif sorted(id2label.values()) == ["LABEL_0", "LABEL_1"]:
        # Binary LABEL_0/LABEL_1 convention: 1 is the positive class. Loud
        # on purpose: a checkpoint with flipped semantics would silently
        # invert the whole benchmark.
        print(
            "WARNING: id2label carries no spam semantics, assuming LABEL_1 is spam",
            file=sys.stderr,
        )
        spam_index = 1
    else:
        raise SystemExit(
            f"cannot resolve the spam class index from id2label={id2label}; "
            "refusing to benchmark with a flipped mapping"
        )

    probabilities = []
    import time

    started = time.time()
    with torch.no_grad():
        for index in range(0, len(texts), 16):
            batch = texts[index : index + 16]
            encoded = tokenizer(
                batch, padding=True, truncation=True, max_length=256, return_tensors="pt"
            )
            logits = model(**encoded).logits
            probabilities.extend(torch.softmax(logits, dim=-1)[:, spam_index].tolist())
    elapsed = time.time() - started
    print(f"inference: {elapsed:.1f}s total, {elapsed / len(texts) * 1000:.0f}ms per message (CPU)", file=sys.stderr)
    print(classification_report(labels, [int(p >= THRESHOLD) for p in probabilities], digits=4))

    ranked = sorted(zip(texts, labels, probabilities), key=lambda row: row[2])
    print("== lowest spam probs among actual spam ==", file=sys.stderr)
    for text, label, prob in [row for row in ranked if row[1] == 1][:5]:
        print(f"p={prob:.3f} :: {text[:140]}", file=sys.stderr)
    print("== highest spam probs among actual ham ==", file=sys.stderr)
    for text, label, prob in [row for row in ranked if row[1] == 0][-5:]:
        print(f"p={prob:.3f} :: {text[:140]}", file=sys.stderr)


if __name__ == "__main__":
    main()
