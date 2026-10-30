#!/usr/bin/env python3
"""Compare warmed test builds, equal concurrency and identical property cases."""
import argparse
import json
import os
from pathlib import Path
import re
import statistics
import subprocess
import time

p = argparse.ArgumentParser(description=__doc__)
p.add_argument('--rounds', type=int, default=3)
p.add_argument('--threads', type=int, default=2)
p.add_argument('--output', type=Path, default=Path('target/runner-benchmark/results.json'))
a = p.parse_args()
assert a.rounds > 0 and a.threads > 0
env = os.environ.copy()
env['PROPTEST_CASES'] = '2048'
env['CARGO_BUILD_JOBS'] = '2'
env['CARGO_TERM_COLOR'] = 'never'
subprocess.run(['cargo', 'test', '--locked', '--all-targets', '--no-run'], env=env, check=True)
commands = {
    'cargo-test': ['cargo', 'test', '--locked', '--all-targets', '--', '--test-threads='+str(a.threads)],
    'nextest': ['cargo', 'nextest', 'run', '--locked', '--all-targets', '--test-threads', str(a.threads)],
}
report = {'rustc': subprocess.check_output(['rustc', '-V'], text=True).strip(),
          'nextest': subprocess.check_output(['cargo', 'nextest', '--version'], text=True).splitlines()[0],
          'threads': a.threads, 'property_cases': 2048, 'builds_precompiled': True,
          'commands': commands, 'runs': []}
a.output.parent.mkdir(parents=True, exist_ok=True)
expected_count = None
for round_index in range(a.rounds):
    for name in (list(commands) if round_index % 2 == 0 else list(reversed(commands))):
        start = time.perf_counter()
        result = subprocess.run(commands[name], env=env, capture_output=True, text=True)
        elapsed = time.perf_counter() - start
        log = result.stdout + result.stderr
        count = (sum(map(int, re.findall(r'test result: ok\. (\d+) passed;', log))) if name == 'cargo-test'
                 else (int(re.search(r'(\d+) tests run: \d+ passed', log)[1]) if re.search(r'(\d+) tests run: \d+ passed', log) else 0))
        Path('target/runner-benchmark').mkdir(parents=True, exist_ok=True)
        Path(f'target/runner-benchmark/{name}-{a.threads}threads-{round_index+1}.log').write_text(log)
        if expected_count is None:
            expected_count = count
            report['test_count'] = count
        assert result.returncode == 0 and count > 0 and count == expected_count, f'{name}: failure or unexpected test count; inspect local log'
        report['runs'].append({'runner': name, 'round': round_index+1, 'seconds': elapsed, 'tests_passed': count})
        a.output.write_text(json.dumps(report, indent=2)+'\n')
        print(f'{name} round {round_index+1}: {elapsed:.3f}s, {count} tests', flush=True)
report['median_seconds'] = {name: statistics.median(r['seconds'] for r in report['runs'] if r['runner']==name) for name in commands}
a.output.write_text(json.dumps(report, indent=2)+'\n')
print(json.dumps(report['median_seconds']), flush=True)
