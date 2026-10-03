"""Глубокий разбор шумного экспорта: форвардеры, сервис, маркеры."""

import json
import sys
from collections import Counter


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


def load_salvaged(path):
    with open(path, encoding="utf-8") as handle:
        text = handle.read()
    try:
        return json.loads(text)["messages"]
    except json.JSONDecodeError:
        start = text.index('"messages"')
        start = text.index("[", start) + 1
        decoder = json.JSONDecoder()
        messages = []
        index = start
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
        return messages


def main() -> None:
    messages = load_salvaged(sys.argv[1])
    print(f"total={len(messages)}", file=sys.stderr)
    authors = Counter()
    forwarders = Counter()
    forward_sources = Counter()
    service_kinds = Counter()
    invite_posters = Counter()
    for message in messages:
        kind = message.get("type")
        if kind == "service":
            service_kinds[message.get("action", "?")] += 1
            continue
        if kind != "message":
            continue
        from_id = message.get("from_id") or "?"
        authors[from_id] += 1
        forwarded = message.get("forwarded_from")
        if forwarded:
            forwarders[from_id] += 1
            name = forwarded.get("name", "?") if isinstance(forwarded, dict) else str(forwarded)
            forward_sources[name] += 1
        if "t.me/+" in message_text(message):
            invite_posters[from_id] += 1
    print(f"authors={len(authors)}", file=sys.stderr)
    print("top authors:", authors.most_common(8), file=sys.stderr)
    print("top forwarders:", forwarders.most_common(10), file=sys.stderr)
    print("forward sources:", forward_sources.most_common(10), file=sys.stderr)
    print("service:", dict(service_kinds), file=sys.stderr)
    print("invite posters:", invite_posters.most_common(10), file=sys.stderr)


if __name__ == "__main__":
    main()
