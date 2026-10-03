"""Быстрый обзор удалённых экспортов: типы сообщений, форварды, сервис."""

import json
import sys
from collections import Counter

for path in sys.argv[1:]:
    print(f"== {path} ==", flush=True)
    with open(path, encoding="utf-8") as handle:
        text = handle.read()
    try:
        data = json.loads(text)
        messages = data.get("messages", [])
        print(f"name={data.get('name')} type={data.get('type')} total={len(messages)} (clean)")
    except json.JSONDecodeError:
        # Обрезанный экспорт: забираем все целые сообщения потоково.
        start = text.index('"messages"')
        start = text.index("[", start) + 1
        decoder = json.JSONDecoder()
        messages = []
        index = start
        name = text[:200]
        while True:
            while index < len(text) and text[index] in " \t\r\n,":
                index += 1
            if index >= len(text) or text[index] != "{":
                break
            try:
                message, end = decoder.raw_decode(text, index)
            except json.JSONDecodeError:
                break
            messages.append(message)
            index = end
        print(f"salvaged={len(messages)} header={name[:120]}")
    kinds = Counter()
    kinds = Counter()
    forwards = Counter()
    for message in messages:
        kinds[message.get("type")] += 1
        forwarded = message.get("forwarded_from")
        if forwarded:
            if isinstance(forwarded, dict):
                forwards[forwarded.get("name", "?")] += 1
            else:
                forwards[str(forwarded)] += 1
    print("kinds:", dict(kinds))
    print("forwarded_from top:", forwards.most_common(10))
