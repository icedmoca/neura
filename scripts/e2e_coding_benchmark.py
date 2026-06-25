#!/usr/bin/env python3
"""E2E coding benchmark report generator.

This is intentionally explicit about what is measured locally vs what remains
UNMEASURED without remote model runs. It reuses committed context benchmarks and
adds an oracle-patch E2E feasibility estimate derived from real git-history tasks.
"""
from __future__ import annotations
import json, pathlib, time, subprocess

ROOT = pathlib.Path(__file__).resolve().parents[1]
OUT = ROOT / 'benchmark-results'


def load(name):
    return json.loads((OUT / name).read_text())


def cargo_test_probe(timeout=60):
    t0=time.perf_counter()
    try:
        p=subprocess.run(['cargo','test','dynamic_tool_filter_tests','--lib'], cwd=ROOT, text=True, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, timeout=timeout)
        ok=p.returncode==0
        out=p.stdout[-2000:]
    except subprocess.TimeoutExpired as e:
        ok=False; out='timeout';
    return {'command':'cargo test dynamic_tool_filter_tests --lib','success':ok,'wall_seconds':round(time.perf_counter()-t0,3),'tail':out[-500:]}


def main():
    coding=load('coding_task_benchmark.json')
    context=load('context_benchmark.json')
    probe=cargo_test_probe()
    e2e={}
    for mode, s in coding['summary'].items():
        tasks=s['tasks']; ctx_success=s['successes']; tokens=s['estimated_prompt_tokens']
        # Oracle-patch estimate: if required file context is present, the ground-truth
        # commit patch is assumed available to the oracle editor. This measures the
        # context layer upper bound, not autonomous model quality.
        e2e[mode]={
            'tasks':tasks,
            'oracle_patch_successes_upper_bound':ctx_success,
            'oracle_patch_success_rate_upper_bound':s['success_rate'],
            'estimated_prompt_tokens':tokens,
            'estimated_tokens_per_oracle_success':s['estimated_tokens_per_success'],
            'autonomous_model_success_rate':'UNMEASURED',
            'autonomous_model_tokens':'UNMEASURED'
        }
    report={
        'metadata':{
            'benchmark_type':'combined_benchmark_report',
            'generated_at_unix':time.time(),
            'repo':'~/.neura/build-src/neura',
            'note':'Remote-model autonomous coding runs are marked UNMEASURED. Local results measure context availability, oracle upper bounds, and focused test execution.'
        },
        'task_success_vs_token_cost': e2e,
        'context_recall_accuracy': context['summary'],
        'real_repo_context_retrieval': coding['summary'],
        'local_test_probe': probe,
        'unmeasured':[
            'autonomous remote-model coding success rate',
            'provider-billed token cost for full multi-turn coding runs',
            'messy ambiguous prompt behavior with live user interaction',
            'long-session degradation over remote model rollouts',
            'human-graded hallucination rate on natural language answers'
        ]
    }
    (OUT/'e2e_coding_runs.json').write_text(json.dumps(report, indent=2))
    summary={'metadata':report['metadata'],'task_success_vs_token_cost':e2e,'local_test_probe':probe,'unmeasured':report['unmeasured']}
    (OUT/'e2e_summary.json').write_text(json.dumps(summary, indent=2))
    print(json.dumps(summary, indent=2))

if __name__=='__main__': main()
