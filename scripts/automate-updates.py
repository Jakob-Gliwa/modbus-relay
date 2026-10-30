#!/usr/bin/env python3
"""Trusted-main automation. Never checks out or executes pull-request code."""
import base64
import json
import os
import re
import subprocess
import tomllib

REPO = "Jakob-Gliwa/modbus-relay"
REGISTRY = "registry+https://github.com/rust-lang/crates.io-index"


def version(value):
    if not re.fullmatch(r"(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)", value):
        raise ValueError(f"Not a stable version: {value}")
    return tuple(map(int, value.split(".")))


def patch_only(before, after):
    """Conservative policy: same crates and sources, stable patch bumps only."""
    def packages(data):
        result = {}
        for package in data["package"]:
            key = (package["name"], package.get("source", ""),
                   tuple(package["version"].split(".")[:2]))
            if key in result:
                raise ValueError("Multiple versions of a crate need review")
            result[key] = package
        return result

    try:
        old, new = packages(before), packages(after)
        if old.keys() != new.keys():
            return False
        changed = False
        for key, current in new.items():
            previous = old[key]
            if current == previous:
                continue
            if key[1] != REGISTRY:
                return False
            a, b = version(previous["version"]), version(current["version"])
            if a[:2] != b[:2] or b[2] <= a[2]:
                return False
            changed = True
        return changed
    except (KeyError, TypeError, ValueError):
        return False


def release_tag(tags, sha):
    stable = [(version(tag[1:]), commit, tag) for tag, commit in tags
              if re.fullmatch(r"v[0-9]+\.[0-9]+\.[0-9]+", tag)]
    if not stable:
        raise ValueError("An initial stable release is required")
    latest, commit, name = max(stable)
    if commit == sha:
        return name, False
    return f"v{latest[0]}.{latest[1]}.{latest[2] + 1}", True


def gh(*args):
    return subprocess.check_output(["gh", *args], text=True)


def api(path, method="GET", payload=None):
    command = ["gh", "api", f"repos/{REPO}/{path}", "--method", method]
    if payload is not None:
        command += ["--input", "-"]
    result = subprocess.run(command, input=json.dumps(payload) if payload is not None else None,
                            capture_output=True, text=True, check=True)
    return json.loads(result.stdout) if result.stdout.strip() else None


def pages(path):
    return [item for page in json.loads(gh("api", f"repos/{REPO}/{path}", "--paginate", "--slurp"))
            for item in page]


def lockfile(sha):
    result = api(f"contents/Cargo.lock?ref={sha}")
    return tomllib.loads(base64.b64decode(result["content"]).decode())


def dispatch(workflow, ref):
    api(f"actions/workflows/{workflow}/dispatches", "POST", {"ref": ref})
    print(f"Dispatched {workflow} on {ref}", flush=True)


def merge_patches():
    for pr in pages("pulls?state=open&base=main&per_page=100"):
        if (pr["user"]["login"] != "dependabot[bot]" or pr["user"]["type"] != "Bot"
                or pr["draft"] or not pr["head"].get("repo")
                or pr["head"]["repo"]["full_name"] != REPO):
            continue
        number, sha = pr["number"], pr["head"]["sha"]
        files = pages(f"pulls/{number}/files?per_page=100")
        if len(files) != 1 or files[0]["filename"] != "Cargo.lock" or files[0]["status"] != "modified":
            print(f"PR #{number}: manual review (files)")
            continue
        if not patch_only(lockfile(pr["base"]["sha"]), lockfile(sha)):
            print(f"PR #{number}: manual review (dependency changes)")
            continue
        detail = api(f"pulls/{number}")
        if detail["head"]["sha"] != sha or detail["mergeable_state"] != "clean":
            print(f"PR #{number}: waiting for required checks / rebase")
            continue
        # GitHub enforces the required CI checks and an up-to-date base. The
        # expected SHA prevents merging a different revision after inspection.
        result = api(f"pulls/{number}/merge", "PUT", {"sha": sha, "merge_method": "squash"})
        if not result["merged"]:
            raise RuntimeError(f"PR #{number} was not merged")
        print(f"Merged patch update #{number}", flush=True)
        dispatch("ci.yml", "main")
        return True  # Serialize updates: test the resulting main first.
    return False


def publish_main():
    sha = api("git/ref/heads/main")["object"]["sha"]
    runs = api(f"actions/workflows/ci.yml/runs?head_sha={sha}&per_page=100")["workflow_runs"]
    runs = [r for r in runs if r["head_branch"] == "main"
            and r["event"] in ("push", "workflow_dispatch", "schedule")]
    # The most recent run must pass; a previous green run cannot hide a new failure.
    if not runs or max(runs, key=lambda r: r["id"])["conclusion"] != "success":
        if not runs:
            dispatch("ci.yml", "main")
        print("Waiting for successful CI on current main")
        return
    releases = api("actions/workflows/release.yml/runs?per_page=100")["workflow_runs"]
    if any(r["status"] != "completed" for r in releases):
        print("A release is already running")
        return
    tags = [(t["name"], t["commit"]["sha"]) for t in pages("tags?per_page=100")]
    tag, create = release_tag(tags, sha)
    if any(r["head_branch"] == tag and r["head_sha"] == sha and r["conclusion"] == "success"
           for r in releases):
        print(f"{tag} already published")
        return
    # Do not keep retrying a broken release hourly; fix it and explicitly rerun.
    if any(r["head_branch"] == tag and r["head_sha"] == sha for r in releases):
        print(f"{tag} needs a manual release rerun")
        return
    if api("git/ref/heads/main")["object"]["sha"] != sha:
        print("Main changed during inspection; waiting for next run")
        return
    if create:
        api("git/refs", "POST", {"ref": f"refs/tags/{tag}", "sha": sha})
    # GITHUB_TOKEN-created tags do not trigger push workflows.
    dispatch("release.yml", tag)


def main():
    if os.environ.get("GITHUB_REPOSITORY") != REPO:
        raise RuntimeError("Unexpected repository")
    publish_main()
    merge_patches()


if __name__ == "__main__":
    main()
