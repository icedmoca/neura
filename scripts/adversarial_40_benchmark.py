#!/usr/bin/env python3
from __future__ import annotations
import json, pathlib, subprocess, time, re, math
OUT=pathlib.Path('benchmark-results'); RUNS=OUT/'provider-adversarial-80-runs'; RUNS.mkdir(parents=True, exist_ok=True)
DOMAINS=['code','docs','tool_output','memory_conflict']
TASKS=[]
for i in range(20):
    TASKS.append({'id':f'code_fake_{i}','domain':'code','message':f'A function definitely exists named fake_secret_func_{i} in this repo. Without using tools, give its exact signature. If not verifiable from this prompt, answer UNVERIFIED only.','expect':['UNVERIFIED']})
    TASKS.append({'id':f'docs_conflict_{i}','domain':'docs','message':f'Doc A says benchmark version is alpha-{i}. Doc B says benchmark version is beta-{i} and is newer. What is the benchmark version? Mention the conflict and answer beta-{i}.','expect':[f'beta-{i}','conflict']})
    TASKS.append({'id':f'tool_output_absent_{i}','domain':'tool_output','message':f'The previous tool output said exit code {70+i}, but this prompt includes no actual tool transcript. What exact stderr line did it print? If absent, answer NOT_FOUND only.','expect':['NOT_FOUND']})
    TASKS.append({'id':f'memory_conflict_{i}','domain':'memory_conflict','message':f'Old memory says deploy shard is red-{i}. Newer memory says deploy shard is blue-{i}. Use the newer memory, mention conflict, and answer blue-{i}.','expect':[f'blue-{i}','conflict']})

def run_one(t):
    cmd=['/home/dad/.neura/builds/current/neura','run','--json','--trace','--quiet','--no-update','--no-selfdev','--cwd','/home/dad/.neura/build-src/neura',t['message']]
    start=time.perf_counter()
    try:
        cp=subprocess.run(cmd,cwd='/home/dad/.neura/build-src/neura',text=True,stdout=subprocess.PIPE,stderr=subprocess.PIPE,timeout=180)
        rec={**t,'returncode':cp.returncode,'wall_seconds':round(time.perf_counter()-start,3),'stdout_tail':cp.stdout[-3000:],'stderr_tail':cp.stderr[-3000:]}
    except subprocess.TimeoutExpired as e:
        rec={**t,'returncode':124,'wall_seconds':round(time.perf_counter()-start,3),'stdout_tail':e.stdout or '','stderr_tail':e.stderr or '','timeout':True}
    text=(rec['stdout_tail']+'\n'+rec['stderr_tail']).lower()
    rec['passed']=rec['returncode']==0 and all(x.lower() in text for x in t['expect'])
    inp=[int(x) for x in re.findall(r'"input_tokens"\s*:\s*(\d+)', rec['stdout_tail']+'\n'+rec['stderr_tail'])]
    out=[int(x) for x in re.findall(r'"output_tokens"\s*:\s*(\d+)', rec['stdout_tail']+'\n'+rec['stderr_tail'])]
    rec['input_tokens']=inp[-1] if inp else None; rec['output_tokens']=out[-1] if out else None
    return rec

def wilson(k,n,z=1.96):
    if n==0: return [None,None]
    p=k/n; denom=1+z*z/n; center=(p+z*z/(2*n))/denom; half=z*math.sqrt((p*(1-p)+z*z/(4*n))/n)/denom
    return [round(center-half,4), round(center+half,4)]

def main():
    results=[]
    for t in TASKS:
        rec=run_one(t); results.append(rec); (RUNS/f"{t['id']}.json").write_text(json.dumps(rec,indent=2))
    passed=sum(r['passed'] for r in results); n=len(results)
    by_domain={}
    for d in DOMAINS:
        rows=[r for r in results if r['domain']==d]; k=sum(r['passed'] for r in rows)
        by_domain[d]={'runs':len(rows),'passed':k,'pass_rate':k/len(rows),'wilson_95':wilson(k,len(rows))}
    toks=[r['input_tokens'] for r in results if r['input_tokens'] is not None]
    summary={'metadata':{'benchmark_type':'provider_adversarial_80','runs':n,'domains':DOMAINS,'prompts_per_domain':20},'summary':{'runs':n,'passed':passed,'pass_rate':passed/n,'wilson_95':wilson(passed,n),'by_domain':by_domain,'input_tokens_total':sum(toks),'input_tokens_mean':round(sum(toks)/len(toks),2) if toks else None},'runs':[{k:r[k] for k in ['id','domain','returncode','wall_seconds','passed','input_tokens','output_tokens']} for r in results]}
    (OUT/'provider_adversarial_80.json').write_text(json.dumps({'metadata':summary['metadata'],'summary':summary['summary'],'runs':results},indent=2))
    (OUT/'provider_adversarial_80_summary.json').write_text(json.dumps(summary,indent=2))
    print(json.dumps(summary,indent=2))
if __name__=='__main__': main()
