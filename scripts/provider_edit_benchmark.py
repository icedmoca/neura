#!/usr/bin/env python3
from __future__ import annotations
import json, pathlib, subprocess, tempfile, textwrap, time, shlex

ROOT=pathlib.Path(__file__).resolve().parents[1]
OUT=ROOT/'benchmark-results'
RUNS=OUT/'provider-edit-runs'
RUNS.mkdir(parents=True, exist_ok=True)

TASKS=[
  {
    'id':'fix_add_function',
    'files':{
      'math_utils.py':'def add(a, b):\n    return a - b\n',
      'test_math_utils.py':'import unittest\nfrom math_utils import add\nclass T(unittest.TestCase):\n    def test_add(self):\n        self.assertEqual(add(2, 3), 5)\n        self.assertEqual(add(-1, 1), 0)\nif __name__ == "__main__": unittest.main()\n'
    },
    'prompt':'Fix the failing Python unit test in this directory. Edit the code, then run python -m unittest. Keep the change minimal.'
  },
  {
    'id':'fix_slugify_edgecase',
    'files':{
      'slug.py':'import re\ndef slugify(text):\n    return re.sub(r"[^a-z0-9]+", "-", text.lower())\n',
      'test_slug.py':'import unittest\nfrom slug import slugify\nclass T(unittest.TestCase):\n    def test_slugify(self):\n        self.assertEqual(slugify(" Hello, Kcode!! "), "hello-kcode")\n        self.assertEqual(slugify("A  B"), "a-b")\nif __name__ == "__main__": unittest.main()\n'
    },
    'prompt':'Fix slugify so the tests pass. Edit files as needed and verify with python -m unittest.'
  },
  {
    'id':'fix_json_config_default',
    'files':{
      'config.py':'def get_timeout(config):\n    return config["timeout"]\n',
      'test_config.py':'import unittest\nfrom config import get_timeout\nclass T(unittest.TestCase):\n    def test_timeout(self):\n        self.assertEqual(get_timeout({"timeout": 10}), 10)\n        self.assertEqual(get_timeout({}), 30)\nif __name__ == "__main__": unittest.main()\n'
    },
    'prompt':'Make the config helper pass the unit tests. The default timeout should be 30. Run python -m unittest before finishing.'
  }
]

def run_cmd(cmd,cwd,timeout=240):
    t=time.perf_counter()
    try:
        p=subprocess.run(cmd,cwd=cwd,text=True,stdout=subprocess.PIPE,stderr=subprocess.PIPE,timeout=timeout)
        return {'returncode':p.returncode,'wall_seconds':round(time.perf_counter()-t,3),'stdout':p.stdout,'stderr':p.stderr}
    except subprocess.TimeoutExpired as e:
        return {'returncode':124,'wall_seconds':round(time.perf_counter()-t,3),'stdout':e.stdout or '', 'stderr':e.stderr or '', 'timeout':True}

def extract_usage(text):
    # kcode --json emits JSON-ish event lines in stdout; collect usage-looking fields if present.
    usage=[]
    for line in text.splitlines():
        line=line.strip()
        if not line.startswith('{'): continue
        try: o=json.loads(line)
        except Exception: continue
        if 'usage' in o or 'input_tokens' in json.dumps(o) or 'output_tokens' in json.dumps(o):
            usage.append(o)
    return usage

def main():
    results=[]
    for task in TASKS:
        with tempfile.TemporaryDirectory(prefix='kcode-edit-bench-') as td:
            wd=pathlib.Path(td)
            for name,content in task['files'].items(): (wd/name).write_text(content)
            before=run_cmd(['python3','-m','unittest'],wd,timeout=30)
            cmd=['/home/dad/.kcode/builds/current/kcode','run','--json','--trace','--quiet','--no-update','--no-selfdev','--cwd',str(wd),task['prompt']]
            provider=run_cmd(cmd,wd,timeout=240)
            after=run_cmd(['python3','-m','unittest'],wd,timeout=30)
            files={p.name:p.read_text(errors='ignore') for p in wd.iterdir() if p.is_file()}
            rec={
              'id':task['id'],'prompt':task['prompt'],'workdir_deleted':True,
              'initial_tests_passed': before['returncode']==0,
              'provider_returncode':provider['returncode'],
              'provider_wall_seconds':provider['wall_seconds'],
              'final_tests_passed': after['returncode']==0,
              'test_stdout_tail':after['stdout'][-1000:], 'test_stderr_tail':after['stderr'][-1000:],
              'provider_stdout_tail':provider['stdout'][-4000:], 'provider_stderr_tail':provider['stderr'][-4000:],
              'usage_events': extract_usage(provider['stdout']+'\n'+provider['stderr']),
              'final_files':files,
            }
            results.append(rec)
            (RUNS/f"{task['id']}.json").write_text(json.dumps(rec,indent=2))
    successes=sum(r['final_tests_passed'] for r in results)
    summary={'metadata':{'benchmark_type':'real_provider_edit_test','tasks':len(results)},'summary':{'tasks':len(results),'successes':successes,'success_rate':successes/len(results),'total_provider_wall_seconds':round(sum(r['provider_wall_seconds'] for r in results),3)},'runs':[{k:r[k] for k in ['id','initial_tests_passed','provider_returncode','provider_wall_seconds','final_tests_passed']} for r in results]}
    (OUT/'provider_edit_benchmark.json').write_text(json.dumps({'metadata':summary['metadata'],'summary':summary['summary'],'runs':results},indent=2))
    (OUT/'provider_edit_benchmark_summary.json').write_text(json.dumps(summary,indent=2))
    print(json.dumps(summary,indent=2))
if __name__=='__main__': main()
