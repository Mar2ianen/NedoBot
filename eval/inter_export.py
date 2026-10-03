"""Разбор смежных экспортов (AyuGram Desktop ChatExport_*) для антиспама.

НЕ коммитит данные: читает result.json из ~/Downloads, пишет только
агрегаты/списки текстов в eval/ (gitignored) и отчёт в stdout.
Запуск: python eval/inter_export.py [ChatExport_...] — без аргументов
обрабатываются все ChatExport_* каталоги, ham объединяется.
"""

import glob
import json
import os
import random
import sys

EXPORT_GLOB = "/home/chechulin/Downloads/AyuGram Desktop/ChatExport_*"
EXPORT = "/home/chechulin/Downloads/AyuGram Desktop/ChatExport_2026-10-03/result.json"

# Наши подтверждённые спамеры (оба чата) для проверки пересечений.
OUR_SPAMMERS = {
    8790432422, 8905047433, 8181383491, 8812311346, 8615315004, 8907693393,
    8420333922, 6137183643, 8758430518, 8856312186, 8812748874, 7792047618,
    8891436896, 8910542268, 8987037847, 8650417683, 8729923271, 8619229793,
    8691346778, 8854361689, 8849313828, 8869706467,
}

BOOK_MARKERS = ["время - деньги", "время-деньги", "брайан трейси", "генри форд"]
FUNNEL_MARKERS = ["пишите в личку", "пишите в лс", "могу переслать", "могу скинуть"]

ADULT_MARKERS = ["onlyfans", "онлифанс", "милф", "слив", "вход для своих", "для взрослых"]
MONEY_MARKERS = ["нужны бабки", "нужны деньги", "помогу с финансами", "шабашка", "дополнительная прибыль", "курьер в шоп", "даю деньги за"]
REFERRAL_MARKERS = ["t.me/+", "telegram.me/+"]


def message_text(message):
    text = message.get("text", "")
    if isinstance(text, str):
        return text
    parts = []
    for entity in text:
        if isinstance(entity, str):
            parts.append(entity)
        elif isinstance(entity, dict):
            parts.append(entity.get("text", ""))
    return "".join(parts)


def main() -> None:
    export_dirs = sys.argv[1:] or sorted(glob.glob(EXPORT_GLOB))
    all_ham = []
    for export_dir in export_dirs:
        result_path = os.path.join(export_dir, "result.json")
        if not os.path.exists(result_path):
            print(f"skip {export_dir}: no result.json", file=sys.stderr)
            continue
        print(f"== {export_dir} ==", file=sys.stderr)
        try:
            all_ham.extend(process_export(result_path))
        except Exception as error:
            print(f"skip {export_dir}: broken export ({error})", file=sys.stderr)
    random.seed(42)
    sample = random.sample(all_ham, min(15000, len(all_ham)))
    with open("eval/inter_ham.txt", "w", encoding="utf-8") as handle:
        handle.write("\x1f".join(sample))
    print(f"ham sample written: {len(sample)} from {len(export_dirs)} exports", file=sys.stderr)


def process_export(path: str) -> list:
    with open(path, encoding="utf-8") as handle:
        data = json.load(handle)
    print(f"chat={data.get('name')} type={data.get('type')}", file=sys.stderr)
    messages = data.get("messages", [])
    print(f"total={len(messages)}", file=sys.stderr)

    from collections import Counter

    authors = Counter()
    overlap = []
    book_hits = []
    funnel_hits = []
    for message in messages:
        if message.get("type") != "message":
            continue
        from_id = (message.get("from_id") or "")
        user_id = int(from_id.replace("user", "")) if from_id.startswith("user") else None
        if user_id is not None:
            authors[user_id] += 1
            if user_id in OUR_SPAMMERS:
                overlap.append((user_id, message.get("id"), message_text(message)[:200]))
        text = message_text(message).lower()
        if any(marker in text for marker in BOOK_MARKERS):
            book_hits.append((user_id, message.get("id"), message_text(message)[:200]))
        if any(marker in text for marker in FUNNEL_MARKERS):
            funnel_hits.append((user_id, message.get("id"), message_text(message)[:200]))

    print(f"unique authors={len(authors)}", file=sys.stderr)
    print(f"OUR spammers present: {sorted({user for user, _, _ in overlap})}", file=sys.stderr)
    print(f"overlap messages={len(overlap)}", file=sys.stderr)
    for row in overlap[:20]:
        print(f"OVERLAP user={row[0]} msg={row[1]} :: {row[2]}", file=sys.stderr)
    print(f"book funnel messages={len(book_hits)}", file=sys.stderr)
    for row in book_hits[:15]:
        print(f"BOOK user={row[0]} msg={row[1]} :: {row[2]}", file=sys.stderr)
    print(f"dm funnel messages={len(funnel_hits)}", file=sys.stderr)
    for row in funnel_hits[:15]:
        print(f"FUNNEL user={row[0]} msg={row[1]} :: {row[2]}", file=sys.stderr)

    # Эвристический майнинг их спама: adult/money/invite-ссылки.
    adult_hits = []
    money_hits = []
    invite_users = {}
    ham_sample = []
    for message in messages:
        if message.get("type") != "message":
            continue
        text = message_text(message)
        low = text.lower()
        if len(text.strip()) > 20:
            ham_sample.append(text.replace("\x1f", " "))
        if any(marker in low for marker in ADULT_MARKERS):
            adult_hits.append((message.get("from_id"), message.get("id"), text[:200]))
        if any(marker in low for marker in MONEY_MARKERS):
            money_hits.append((message.get("from_id"), message.get("id"), text[:200]))
        if "t.me/+" in text or "telegram.me/+" in text:
            from_id = message.get("from_id") or ""
            invite_users[from_id] = invite_users.get(from_id, 0) + 1
    print(f"adult hits={len(adult_hits)} money hits={len(money_hits)}", file=sys.stderr)
    for row in adult_hits[:10]:
        print(f"ADULT user={row[0]} msg={row[1]} :: {row[2]}", file=sys.stderr)
    for row in money_hits[:10]:
        print(f"MONEY user={row[0]} msg={row[1]} :: {row[2]}", file=sys.stderr)
    multi_invite = sorted(invite_users.items(), key=lambda kv: kv[1], reverse=True)[:15]
    print(f"top invite-link posters={multi_invite}", file=sys.stderr)
    return ham_sample


if __name__ == "__main__":
    main()
