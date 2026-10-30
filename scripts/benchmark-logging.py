#!/usr/bin/env python3
"""Compare producer cost and fully drained cost; every enabled record is counted."""
import argparse
import json
from pathlib import Path
import shlex
import statistics
import subprocess


def main():
    p=argparse.ArgumentParser(description=__doc__)
    p.add_argument('--prefix',default='docker exec modbus-relay-optimization-study')
    p.add_argument('--rounds',type=int,default=5)
    p.add_argument('--variant',action='append',help='binary label under /bins')
    p.add_argument('--output',type=Path,required=True)
    args=p.parse_args()
    rows=[]
    labels=args.variant or ['baseline','targets','ftlog','ftlog-buffered']
    for level in ['info','debug']:
        for repeat in range(args.rounds):
            offset=repeat % len(labels)
            order=labels[offset:]+labels[:offset]
            for label in order:
                result=subprocess.run(shlex.split(args.prefix)+[f'/bins/{label}-logging-bench',level],capture_output=True,text=True,check=True,timeout=60)
                row=json.loads(result.stdout)
                assert row['records_written']==(row['iterations'] if level=='debug' else 0)
                rows.append({'round':repeat+1,**row})
                print(f"{label}/{level}: producer {row['producer_ns_per_event']:.1f} ns, drained {row['drained_ns_per_event']:.1f} ns",flush=True)
                args.output.parent.mkdir(parents=True,exist_ok=True)
                args.output.write_text(json.dumps({'runs':rows},indent=2)+'\n')
    summary={}
    for label in labels:
        summary[label]={}
        for level in ['info','debug']:
            selected=[r for r in rows if r['variant']==label and r['level']==level]
            summary[label][level]={k:statistics.median(r[k] for r in selected) for k in ['producer_ns_per_event','drained_ns_per_event']}
    args.output.write_text(json.dumps({'runs':rows,'summary':summary},indent=2)+'\n')
    print(json.dumps(summary,indent=2))


if __name__=='__main__':
    main()
