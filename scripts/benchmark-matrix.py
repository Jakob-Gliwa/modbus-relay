#!/usr/bin/env python3
"""Run a balanced, seeded binary comparison; retain every raw run as JSON.

No parallel load: variants alternate within each round. Use --prefix 'docker exec
CONTAINER' for a Linux study; binary and log paths then refer to the container.
"""
import argparse
import json
import os
from pathlib import Path
import random
import shlex
import statistics
import subprocess
import time


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument('--benchmark', default='target/release/examples/benchmark')
    p.add_argument('--prefix', default='')
    p.add_argument('--variant', action='append', required=True, help='label=binary path')
    p.add_argument('--rounds', type=int, default=5)
    p.add_argument('--clients', type=int, default=32)
    p.add_argument('--requests-per-client', type=int, default=2000)
    p.add_argument('--device-delay-us', type=int, default=0)
    p.add_argument('--log-level', default='info')
    p.add_argument('--verify-logs', action='store_true')
    p.add_argument('--seed', type=int, default=1729)
    p.add_argument('--output', type=Path, required=True)
    args = p.parse_args()
    variants = dict(v.split('=', 1) for v in args.variant)
    if len(variants) != len(args.variant) or args.rounds < 1:
        p.error('labels must be unique and rounds positive')
    rng = random.Random(args.seed)
    prefix = shlex.split(args.prefix)
    runs = []
    report = {'parameters': vars(args) | {'output': str(args.output)}, 'runs': runs}
    args.output.parent.mkdir(parents=True, exist_ok=True)
    order = list(variants)
    rng.shuffle(order)
    for round_index in range(args.rounds):
        # Rotate a fixed seeded order: each candidate occupies each position.
        labels = order[round_index % len(order):] + order[:round_index % len(order)]
        for label in labels:
            cmd = prefix + [args.benchmark, '--binary', variants[label],
                            '--clients', str(args.clients), '--requests-per-client', str(args.requests_per_client),
                            '--device-delay-us', str(args.device_delay_us), '--log-level', args.log_level]
            log_file = f'/tmp/modbus-relay-study-{os.getpid()}.log'
            if args.verify_logs:
                cmd += ['--log-file', log_file]
            started = time.time()
            try:
                result = subprocess.run(cmd, check=True, capture_output=True, text=True, timeout=600)
                measurement = json.loads(result.stdout)
                assert measurement['backend_connections'] == 1, measurement
                assert measurement['peak_sampled_rss_kib'] > 0, 'RSS sampling failed'
                runs.append({'variant': label, 'round': round_index + 1, 'started_unix': started, **measurement})
                args.output.write_text(json.dumps(report, indent=2) + '\n')
                print(f"{label} round {round_index+1}: {measurement['requests_per_second']:.0f} req/s, "
                      f"p99 {measurement['latency_us']['p99']} us, RSS {measurement['peak_sampled_rss_kib']} KiB", flush=True)
            except subprocess.CalledProcessError as exc:
                print(exc.stdout, exc.stderr, flush=True)
                raise
            finally:
                if args.verify_logs:
                    subprocess.run(prefix + ['rm', '-f', log_file], check=True)
    summary = {}
    for label in variants:
        selected = [r for r in runs if r['variant'] == label]
        rates = [r['requests_per_second'] for r in selected]
        summary[label] = {
            'runs': len(selected), 'throughput_median': statistics.median(rates),
            'throughput_min': min(rates), 'throughput_max': max(rates),
            'p99_us_median': statistics.median(r['latency_us']['p99'] for r in selected),
            'rss_kib_median': statistics.median(r['peak_sampled_rss_kib'] for r in selected),
        }
    report['summary'] = summary
    args.output.write_text(json.dumps(report, indent=2) + '\n')
    print(json.dumps(summary, indent=2), flush=True)


if __name__ == '__main__':
    main()
