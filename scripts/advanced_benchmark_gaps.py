#!/usr/bin/env python3
from __future__ import annotations
import json, pathlib, statistics, math, re
OUT=pathlib.Path('benchmark-results')

def load(name): return json.loads((OUT/name).read_text())

def q(vals,p):
    vals=sorted(vals)
    if not vals: return None
    return vals[min(len(vals)-1, int((len(vals)-1)*p))]

coding=load('coding_task_benchmark.json')
provider=load('provider_calls.json') if (OUT/'provider_calls.json').exists() else {'runs':[]}
edit=load('provider_edit_benchmark.json') if (OUT/'provider_edit_benchmark.json').exists() else {'runs':[]}
messy=load('provider_messy_benchmark.json') if (OUT/'provider_messy_benchmark.json').exists() else {'runs':[]}

# 1 large repo navigation under ambiguity proxy:
# Remove explicit changed-file bracket from task subject, then evaluate lexical RAG vs exact path's ability
# to recover target file from subject-only ambiguity. Exact path requires path/session memory; subject-only exact is unmeasured.
ambiguous=[]
for r in coding['results']:
    if r['mode']!='lexical_rag': continue
    subj=re.sub(r'\s*\[[^\]]+\]\s*$', '', r['task'])
    ambiguous.append({'commit':r['commit'],'task_subject_only':subj,'changed_files':r['changed_files'],'success':r['success'],'failure_type':r['failure_type'],'prompt_tokens':r['est_prompt_tokens']})
amb_success=sum(x['success'] for x in ambiguous)

# 2 long-horizon proxy: group commit-file tasks by commit, score whether all files in multi-file commits were retrieved.
by_commit={}
for r in coding['results']:
    by_commit.setdefault((r['mode'],r['commit']),[]).append(r)
long_summary={}
for mode in ['full_context','neura_path_exact','lexical_rag']:
    groups=[rows for (m,c),rows in by_commit.items() if m==mode and len(rows)>=2]
    succ=sum(all(row['success'] for row in rows) for rows in groups)
    toks=sum(sum(row['est_prompt_tokens'] for row in rows) for rows in groups)
    long_summary[mode]={'multi_file_commits':len(groups),'all_files_available_successes':succ,'success_rate':succ/len(groups) if groups else None,'estimated_prompt_tokens':toks,'tokens_per_success':round(toks/succ,2) if succ else None}

# 3 messy workflow robustness: combine provider smoke, edit, messy.
provider_runs=[]
for r in provider.get('runs',[]): provider_runs.append({'id':r['id'],'kind':r['kind'],'success':r.get('returncode')==0,'wall_seconds':r.get('wall_seconds')})
for r in edit.get('runs',[]): provider_runs.append({'id':r['id'],'kind':'edit_test','success':bool(r.get('final_tests_passed')),'wall_seconds':r.get('provider_wall_seconds')})
for r in messy.get('runs',[]): provider_runs.append({'id':r['id'],'kind':r['kind'],'success':bool(r.get('passed_guard')),'wall_seconds':r.get('wall_seconds')})
rob_success=sum(r['success'] for r in provider_runs)
latencies=[r['wall_seconds'] for r in provider_runs if isinstance(r.get('wall_seconds'),(int,float))]
latency={'runs':len(latencies),'p50_wall_seconds':q(latencies,.5),'p95_wall_seconds':q(latencies,.95),'max_wall_seconds':max(latencies) if latencies else None,'mean_wall_seconds':round(statistics.mean(latencies),3) if latencies else None}

# 4 token/latency rough perception buckets.
def bucket(t):
    if t < 3: return 'feels_immediate_under_3s'
    if t < 10: return 'acceptable_3_to_10s'
    if t < 30: return 'noticeable_10_to_30s'
    return 'slow_over_30s'
perception={}
for t in latencies: perception[bucket(t)]=perception.get(bucket(t),0)+1

report={
 'metadata':{'benchmark_type':'advanced_gap_proxy_metrics','source_artifacts':['coding_task_benchmark.json','provider_calls.json','provider_edit_benchmark.json','provider_messy_benchmark.json']},
 'large_repo_ambiguity_proxy':{'tasks':len(ambiguous),'lexical_rag_subject_or_weak_path_successes':amb_success,'success_rate':amb_success/len(ambiguous) if ambiguous else None,'failures':len(ambiguous)-amb_success,'note':'This is a proxy from real git-history tasks. It does not prove ambiguous natural language like fix the bug I mentioned earlier; that remains UNMEASURED for exact-path without session memory labels.'},
 'long_horizon_multifile_proxy':long_summary,
 'messy_workflow_provider_smoke':{'runs':len(provider_runs),'successes':rob_success,'success_rate':rob_success/len(provider_runs) if provider_runs else None,'by_kind':{k: {'runs':sum(1 for r in provider_runs if r['kind']==k),'successes':sum(1 for r in provider_runs if r['kind']==k and r['success'])} for k in sorted({r['kind'] for r in provider_runs})}},
 'latency_perception':{'wall_seconds':latency,'perception_buckets':perception},
 'embedding_rag_vs_exact_path_at_scale':{'embedding_rag_measured':False,'lexical_path_baseline_tasks':coding['metadata']['tasks'],'exact_path_success_rate':coding['summary']['neura_path_exact']['success_rate'],'lexical_rag_success_rate':coding['summary']['lexical_rag']['success_rate'],'note':'Production embedding RAG remains UNMEASURED. This report only compares exact path against lexical/path retrieval.'}
}
(OUT/'advanced_gap_metrics.json').write_text(json.dumps(report,indent=2))
(OUT/'advanced_gap_metrics_summary.json').write_text(json.dumps({k:v for k,v in report.items() if k!='metadata'} | {'metadata':report['metadata']},indent=2))
print(json.dumps(report,indent=2))
