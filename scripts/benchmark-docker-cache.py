#!/usr/bin/env python3
"""Measure warm and source-only Docker rebuilds in an isolated context."""
import argparse
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import tempfile
import time


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path, default=Path("target/docker-cache-benchmark.json"))
    for target in ("test", "runtime"):
        parser.add_argument(f"--{target}-cache-from",
                            help=f"BuildKit cache source for the {target} target")
    args = parser.parse_args()
    root = Path(__file__).resolve().parents[1]
    results = []
    with tempfile.TemporaryDirectory(prefix="modbus-cache-benchmark-") as directory:
        context = Path(directory)
        for name in ("Dockerfile", ".dockerignore", "Cargo.toml", "Cargo.lock", "LICENSE",
                     "config.example.yaml", "src", "tests", "examples"):
            source = root / name
            if source.is_dir():
                shutil.copytree(source, context / name)
            else:
                shutil.copy2(source, context / name)
        for case in ("warm", "source-change"):
            if case == "source-change":
                with (context / "src/lib.rs").open("a") as source:
                    source.write("\n// Isolated cache benchmark: source changed, dependencies unchanged.\n")
            for target in ("test", "runtime"):
                command = ["docker", "buildx", "build", "--progress=plain",
                           "--output=type=cacheonly", "--target", target]
                cache = getattr(args, target + "_cache_from")
                if cache:
                    command += ["--cache-from", cache]
                if os.environ.get("BUILD_CA_FILE"):
                    command += ["--secret", "id=build_ca,src=" + os.environ["BUILD_CA_FILE"]]
                command.append(str(context))
                started = time.monotonic()
                result = subprocess.run(command, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True)
                elapsed = time.monotonic() - started
                # Preserve full diagnostics on failure; successful reports contain no host paths.
                if result.returncode:
                    print(result.stdout)
                    raise SystemExit(result.returncode)
                compiled = re.findall(r"Compiling ([A-Za-z0-9_-]+) v", result.stdout)
                row = {"case": case, "target": target, "seconds": round(elapsed, 3),
                       "compiled_crates": sorted(set(compiled)),
                       "cached_steps": result.stdout.count("CACHED")}
                results.append(row)
                print(json.dumps(row), flush=True)
                args.output.parent.mkdir(parents=True, exist_ok=True)
                args.output.write_text(json.dumps({"measurements": results}, indent=2) + "\n")
                if case == "source-change" and set(compiled) - {"modbus-relay"}:
                    print(result.stdout, flush=True)
                    raise SystemExit("Dependency cache miss on a source-only edit")


if __name__ == "__main__":
    main()
