#!/usr/bin/env python3
from __future__ import annotations
import json, pathlib, subprocess, tempfile, textwrap, time, re
OUT=pathlib.Path('benchmark-results'); RUNS=OUT/'provider-edit-runs'; RUNS.mkdir(parents=True, exist_ok=True)

def dedent(s): return textwrap.dedent(s).lstrip()
TASKS=[
 {'id':'fix_add_function','files':{'math_utils.py':dedent('''def add(a, b):
    return a - b
'''),'test_math_utils.py':dedent('''import unittest
from math_utils import add
class T(unittest.TestCase):
    def test_add(self):
        self.assertEqual(add(2, 3), 5)
        self.assertEqual(add(-1, 1), 0)
if __name__ == "__main__": unittest.main()
''')},'prompt':'Fix the failing Python unit test in this directory. Edit the code, then run python -m unittest. Keep the change minimal.'},
 {'id':'fix_slugify_edgecase','files':{'slug.py':dedent('''import re
def slugify(text):
    return re.sub(r"[^a-z0-9]+", "-", text.lower())
'''),'test_slug.py':dedent('''import unittest
from slug import slugify
class T(unittest.TestCase):
    def test_slugify(self):
        self.assertEqual(slugify(" Hello, Neura!! "), "hello-neura")
        self.assertEqual(slugify("A  B"), "a-b")
if __name__ == "__main__": unittest.main()
''')},'prompt':'Fix slugify so the tests pass. Edit files as needed and verify with python -m unittest.'},
 {'id':'fix_json_config_default','files':{'config.py':dedent('''def get_timeout(config):
    return config["timeout"]
'''),'test_config.py':dedent('''import unittest
from config import get_timeout
class T(unittest.TestCase):
    def test_timeout(self):
        self.assertEqual(get_timeout({"timeout": 10}), 10)
        self.assertEqual(get_timeout({}), 30)
if __name__ == "__main__": unittest.main()
''')},'prompt':'Make the config helper pass the unit tests. The default timeout should be 30. Run python -m unittest before finishing.'},
 {'id':'fix_multiply_function','files':{'calc.py':dedent('''def multiply(a, b):
    return a + b
'''),'test_calc.py':dedent('''import unittest
from calc import multiply
class T(unittest.TestCase):
    def test_multiply(self):
        self.assertEqual(multiply(3, 4), 12)
        self.assertEqual(multiply(-2, 5), -10)
if __name__ == "__main__": unittest.main()
''')},'prompt':'Fix multiply so the unit tests pass. Run python -m unittest before finishing.'},
 {'id':'fix_list_average_empty','files':{'stats.py':dedent('''def average(xs):
    return sum(xs) / len(xs)
'''),'test_stats.py':dedent('''import unittest
from stats import average
class T(unittest.TestCase):
    def test_average(self):
        self.assertEqual(average([2, 4, 6]), 4)
        self.assertEqual(average([]), 0)
if __name__ == "__main__": unittest.main()
''')},'prompt':'Fix average so empty lists return 0 and tests pass. Run python -m unittest.'},
 {'id':'fix_boolean_parser','files':{'parse_bool.py':dedent('''def parse_bool(value):
    return bool(value)
'''),'test_parse_bool.py':dedent('''import unittest
from parse_bool import parse_bool
class T(unittest.TestCase):
    def test_parse_bool(self):
        self.assertTrue(parse_bool("true"))
        self.assertFalse(parse_bool("false"))
        self.assertFalse(parse_bool("0"))
if __name__ == "__main__": unittest.main()
''')},'prompt':'Fix parse_bool for string booleans and run python -m unittest.'},
 {'id':'fix_unique_preserve_order','files':{'unique.py':dedent('''def unique(xs):
    return list(set(xs))
'''),'test_unique.py':dedent('''import unittest
from unique import unique
class T(unittest.TestCase):
    def test_unique(self):
        self.assertEqual(unique([3, 1, 3, 2, 1]), [3, 1, 2])
if __name__ == "__main__": unittest.main()
''')},'prompt':'Fix unique so it removes duplicates while preserving order. Run python -m unittest.'},
 {'id':'fix_env_parser','files':{'env.py':dedent('''def parse_env(text):
    return dict(line.split("=") for line in text.splitlines())
'''),'test_env.py':dedent('''import unittest
from env import parse_env
class T(unittest.TestCase):
    def test_env(self):
        self.assertEqual(parse_env("A=1\n# comment\nB=two"), {"A":"1", "B":"two"})
        self.assertEqual(parse_env(""), {})
if __name__ == "__main__": unittest.main()
''')},'prompt':'Fix parse_env so comments and empty input work, then run python -m unittest.'},
 {'id':'fix_clamp_bounds','files':{'clamp.py':dedent('''def clamp(x, lo, hi):
    return min(lo, max(hi, x))
'''),'test_clamp.py':dedent('''import unittest
from clamp import clamp
class T(unittest.TestCase):
    def test_clamp(self):
        self.assertEqual(clamp(5, 1, 10), 5)
        self.assertEqual(clamp(-2, 0, 3), 0)
        self.assertEqual(clamp(9, 0, 3), 3)
if __name__ == "__main__": unittest.main()
''')},'prompt':'Fix clamp and verify with python -m unittest.'},
 {'id':'fix_word_count','files':{'words.py':dedent('''def word_count(text):
    return len(text.split(" "))
'''),'test_words.py':dedent('''import unittest
from words import word_count
class T(unittest.TestCase):
    def test_word_count(self):
        self.assertEqual(word_count("hello   neura"), 2)
        self.assertEqual(word_count(""), 0)
if __name__ == "__main__": unittest.main()
''')},'prompt':'Fix word_count for repeated spaces and empty strings. Run python -m unittest.'},
]

def run_cmd(cmd,cwd,timeout=240):
    t=time.perf_counter()
    try:
        p=subprocess.run(cmd,cwd=cwd,text=True,stdout=subprocess.PIPE,stderr=subprocess.PIPE,timeout=timeout)
        return {'returncode':p.returncode,'wall_seconds':round(time.perf_counter()-t,3),'stdout':p.stdout,'stderr':p.stderr}
    except subprocess.TimeoutExpired as e:
        return {'returncode':124,'wall_seconds':round(time.perf_counter()-t,3),'stdout':e.stdout or '', 'stderr':e.stderr or '', 'timeout':True}

def extract_usage(text):
    return {'input_tokens':[int(x) for x in re.findall(r'"input_tokens"\s*:\s*(\d+)', text)], 'output_tokens':[int(x) for x in re.findall(r'"output_tokens"\s*:\s*(\d+)', text)]}

def main():
    results=[]
    for task in TASKS:
        with tempfile.TemporaryDirectory(prefix='neura-edit-bench-') as td:
            wd=pathlib.Path(td)
            for name,content in task['files'].items(): (wd/name).write_text(content)
            before=run_cmd(['python3','-m','unittest'],wd,timeout=30)
            cmd=['/home/dad/.neura/builds/current/neura','run','--json','--trace','--quiet','--no-update','--no-selfdev','--cwd',str(wd),task['prompt']]
            provider=run_cmd(cmd,wd,timeout=240)
            after=run_cmd(['python3','-m','unittest'],wd,timeout=30)
            usage=extract_usage(provider['stdout']+'\n'+provider['stderr'])
            rec={'id':task['id'],'prompt':task['prompt'],'initial_tests_passed':before['returncode']==0,'provider_returncode':provider['returncode'],'provider_wall_seconds':provider['wall_seconds'],'final_tests_passed':after['returncode']==0,'input_tokens':usage['input_tokens'][-1] if usage['input_tokens'] else None,'output_tokens':usage['output_tokens'][-1] if usage['output_tokens'] else None,'test_stdout_tail':after['stdout'][-1000:],'test_stderr_tail':after['stderr'][-1000:],'provider_stdout_tail':provider['stdout'][-4000:],'provider_stderr_tail':provider['stderr'][-4000:]}
            results.append(rec); (RUNS/f"{task['id']}.json").write_text(json.dumps(rec,indent=2))
    successes=sum(r['final_tests_passed'] for r in results); toks=[r['input_tokens'] for r in results if r['input_tokens']]
    summary={'metadata':{'benchmark_type':'real_provider_edit_test','tasks':len(results)},'summary':{'tasks':len(results),'successes':successes,'success_rate':successes/len(results),'total_provider_wall_seconds':round(sum(r['provider_wall_seconds'] for r in results),3),'input_tokens_total':sum(toks),'input_tokens_mean':round(sum(toks)/len(toks),2) if toks else None},'runs':[{k:r[k] for k in ['id','initial_tests_passed','provider_returncode','provider_wall_seconds','final_tests_passed','input_tokens','output_tokens']} for r in results]}
    OUT.mkdir(exist_ok=True)
    (OUT/'provider_edit_benchmark.json').write_text(json.dumps({'metadata':summary['metadata'],'summary':summary['summary'],'runs':results},indent=2))
    (OUT/'provider_edit_benchmark_summary.json').write_text(json.dumps(summary,indent=2))
    print(json.dumps(summary,indent=2))
if __name__=='__main__': main()
