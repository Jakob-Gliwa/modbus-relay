#!/usr/bin/env python3
"""Authorize a release using completed, trusted CI for its exact source commit."""
import json
import os
import re
import subprocess

REPO = "Jakob-Gliwa/modbus-relay"
REQUIRED_JOBS = {
    "test (ubuntu-24.04)", "test (macos-15)", "coverage", "dependencies", "secrets",
    "docker (amd64, ubuntu-24.04)", "docker (arm64, ubuntu-24.04-arm)",
}


def verified_run(runs, sha):
    trusted = [run for run in runs
               if run["head_sha"] == sha and run["head_branch"] == "main"
               and run["head_repository"]["full_name"] == REPO
               and run["event"] in ("push", "workflow_dispatch", "schedule")
               and run["path"] == ".github/workflows/ci.yml"]
    if not trusted:
        raise ValueError("No main CI run exists for this exact commit")
    latest = max(trusted, key=lambda run: run["id"])
    if latest["status"] != "completed" or latest["conclusion"] != "success":
        raise ValueError("The latest main CI run for this commit has not passed")
    return latest


def verify_jobs(jobs, sha):
    successful = {job["name"] for job in jobs
                  if job["head_sha"] == sha and job["status"] == "completed"
                  and job["conclusion"] == "success"}
    missing = REQUIRED_JOBS - successful
    if missing:
        raise ValueError("Required CI jobs did not pass: " + ", ".join(sorted(missing)))


def api(path):
    return json.loads(subprocess.check_output(
        ["gh", "api", f"repos/{REPO}/{path}"], text=True))


def main():
    sha = os.environ["GITHUB_SHA"]
    if (os.environ["GITHUB_REPOSITORY"] != REPO or not re.fullmatch("[a-f0-9]{40}", sha)
            or not re.fullmatch(r"refs/tags/v[0-9]+\.[0-9]+\.[0-9]+", os.environ["GITHUB_REF"])):
        raise ValueError("Expected a stable version tag in the release repository")
    # Resolve the tag through the API instead of trusting mutable checkout state.
    tag = os.environ["GITHUB_REF"].removeprefix("refs/tags/")
    if api(f"commits/{tag}")["sha"] != sha:
        raise ValueError("Tag and release commit differ")
    run = verified_run(api(f"actions/workflows/ci.yml/runs?head_sha={sha}&per_page=100")["workflow_runs"], sha)
    # Only the current attempt counts. Old successes cannot mask a failed rerun.
    jobs = api(f"actions/runs/{run['id']}/attempts/{run['run_attempt']}/jobs?per_page=100")["jobs"]
    verify_jobs(jobs, sha)
    print(f"Verified source {sha} against CI run {run['id']}, attempt {run['run_attempt']}")
    with open(os.environ["GITHUB_OUTPUT"], "a") as output:
        output.write(f"source_sha={sha}\n")
    with open(os.environ["GITHUB_STEP_SUMMARY"], "a") as summary:
        summary.write(f"Reusing [successful main CI]({run['html_url']}) for `{sha}`. "
                      "All seven required jobs passed; final images still undergo native smoke tests.\n")


if __name__ == "__main__":
    main()
