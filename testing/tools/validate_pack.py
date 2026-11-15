#!/usr/bin/env python3
"""Validate archive scaffolding and synthetic-only corpus format; no repo access/network."""
from pathlib import Path
import json,re,sys

ROOT = Path(__file__).resolve().parents[2]
errors=[]
for relative in ['README.md','testing/START_CODEX.md','testing/README_TASKS.md','testing/references/INVARIANTS.md','testing/references/CORPUS_PRIVACY.md','.agents/skills/telegram-input-hunter/SKILL.md']:
 if not (ROOT/relative).is_file(): errors.append('missing: '+relative)
skill=ROOT/'.agents/skills/telegram-input-hunter/SKILL.md'
if skill.is_file():
 s=skill.read_text(encoding='utf8')
 if not s.startswith('---\nname: telegram-input-hunter\ndescription:'): errors.append('bad skill frontmatter')
for i in range(1,17):
 p=ROOT/f'testing/tasks/T{i:02}.md'
 if not p.is_file(): errors.append('missing task: '+str(p))
 elif not p.read_text(encoding='utf8').startswith(f'# T{i:02} —'): errors.append('task title mismatch: '+str(p))
for name in ['synthetic_urls.jsonl','synthetic_updates.jsonl']:
 path=ROOT/'testing/corpus/public'/name
 if not path.is_file(): errors.append('missing corpus: '+name); continue
 for n,line in enumerate(path.read_text(encoding='utf8').splitlines(),1):
  try: obj=json.loads(line)
  except Exception as exc:errors.append(f'{name}:{n}: bad JSON: {exc}');continue
  if not obj.get('id') or not obj.get('validity'):errors.append(f'{name}:{n}: missing id/validity')
  if name=='synthetic_urls.jsonl' and not re.search(r'example\.org|\.invalid|^@|^tg://|^https://t\.me/|2001:db8::',obj.get('input',''),re.IGNORECASE):
   errors.append(f'{name}:{n}: not obviously synthetic; check manually')
if errors:
 print('\n'.join(errors),file=sys.stderr);sys.exit(1)
print('OK: skill, 16 ordered tasks, reference docs, synthetic JSONL corpora. Manual privacy review is still required.')
