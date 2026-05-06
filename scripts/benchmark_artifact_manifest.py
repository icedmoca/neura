#!/usr/bin/env python3
from __future__ import annotations
import hashlib, json, pathlib, time
ROOT=pathlib.Path('.').resolve()
patterns=['BENCHMARKS.md','scripts/*benchmark*.py','benchmark-results/**/*.json']
files=[]
for pat in patterns:
    files.extend(pathlib.Path('.').glob(pat))
rows=[]
for p in sorted(set(files)):
    if p.is_file():
        data=p.read_bytes()
        rows.append({'path':str(p),'bytes':len(data),'sha256':hashlib.sha256(data).hexdigest()})
manifest={'metadata':{'generated_at_unix':time.time(),'artifact_count':len(rows),'repo':str(ROOT)},'artifacts':rows}
out=pathlib.Path('benchmark-results/artifact_manifest.json')
out.parent.mkdir(exist_ok=True)
out.write_text(json.dumps(manifest,indent=2))
print(json.dumps({'artifact_count':len(rows),'manifest':str(out)},indent=2))
