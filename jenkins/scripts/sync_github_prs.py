#!/usr/bin/env python3
"""Sync open GitHub pull requests into this Gitea repo as branches + PRs.

For every open PR on the upstream GitHub repo:
  1. Push its head commit into this Gitea repo as branch ``gh-pr-<number>``
  2. Open a Gitea pull request for that branch, unless one already exists
"""
import base64
import json
import os
import subprocess
import sys
import urllib.error
import urllib.request

GITEA_SERVER = os.environ["GITEA_SERVER"].rstrip("/")
GITEA_OWNER = os.environ["GITEA_OWNER"]
GITEA_REPO = os.environ["GITEA_REPO"]
GITEA_USER = os.environ["GITEA_USER"]
GITEA_TOKEN = os.environ["GITEA_TOKEN"]
GITHUB_OWNER = os.environ["GITHUB_OWNER"]
GITHUB_REPO = os.environ["GITHUB_REPO"]
GITHUB_TOKEN = os.environ.get("GITHUB_TOKEN", "")
BASE_BRANCH = os.environ.get("BASE_BRANCH", "main")
BRANCH_PREFIX = "gh-pr-"


def http_json(url, token=None, basic_auth=None, method="GET", data=None):
    headers = {"Accept": "application/json"}
    if basic_auth:
        creds = base64.b64encode(f"{basic_auth[0]}:{basic_auth[1]}".encode()).decode()
        headers["Authorization"] = f"Basic {creds}"
    elif token:
        headers["Authorization"] = f"token {token}"
    body = None
    if data is not None:
        body = json.dumps(data).encode()
        headers["Content-Type"] = "application/json"
    req = urllib.request.Request(url, data=body, headers=headers, method=method)
    try:
        with urllib.request.urlopen(req) as resp:
            raw = resp.read()
            return json.loads(raw) if raw else None
    except urllib.error.HTTPError as e:
        detail = e.read().decode(errors="replace")
        raise RuntimeError(f"{method} {url} -> {e.code}: {detail}") from e


def github_paginated(path):
    items = []
    page = 1
    while True:
        sep = "&" if "?" in path else "?"
        url = f"https://api.github.com/repos/{GITHUB_OWNER}/{GITHUB_REPO}{path}{sep}per_page=100&page={page}"
        chunk = http_json(url, token=GITHUB_TOKEN or None)
        if not chunk:
            break
        items.extend(chunk)
        if len(chunk) < 100:
            break
        page += 1
    return items


def gitea_open_prs():
    prs = []
    page = 1
    while True:
        url = (
            f"{GITEA_SERVER}/api/v1/repos/{GITEA_OWNER}/{GITEA_REPO}/pulls"
            f"?state=open&limit=50&page={page}"
        )
        chunk = http_json(url, basic_auth=(GITEA_USER, GITEA_TOKEN))
        if not chunk:
            break
        prs.extend(chunk)
        if len(chunk) < 50:
            break
        page += 1
    return prs


def run(*args):
    print("+", " ".join(args))
    subprocess.run(args, check=True)


def main():
    open_github_prs = github_paginated("/pulls?state=open")
    closed_github_prs = github_paginated("/pulls?state=closed")
    gitea_prs_by_branch = {pr["head"]["ref"]: pr for pr in gitea_open_prs()}

    print(f"Found {len(open_github_prs)} open GitHub PR(s).")

    for pr in open_github_prs:
        number = pr["number"]
        branch = f"{BRANCH_PREFIX}{number}"
        local_ref = f"refs/remotes/github/pr/{number}"
        author = pr["user"]["login"]
        title = pr["title"]
        html_url = pr["html_url"]

        run("git", "push", "origin", f"{local_ref}:refs/heads/{branch}", "--force")

        existing = gitea_prs_by_branch.get(branch)
        if existing:
            print(f"PR #{number}: branch '{branch}' updated, Gitea PR #{existing['number']} already open.")
            continue

        body = (pr.get("body") or "").strip()
        pr_body = f"Mirrors {html_url} by @{author}.\n\n---\n\n{body}".strip()
        created = http_json(
            f"{GITEA_SERVER}/api/v1/repos/{GITEA_OWNER}/{GITEA_REPO}/pulls",
            basic_auth=(GITEA_USER, GITEA_TOKEN),
            method="POST",
            data={
                "head": branch,
                "base": BASE_BRANCH,
                "title": f"[GH#{number}] {title}",
                "body": pr_body,
            },
        )
        print(f"PR #{number}: opened Gitea PR #{created['number']} from '{branch}'.")

    # Close mirrored Gitea PRs whose GitHub PR was closed without merging.
    closed_numbers = {p["number"] for p in closed_github_prs if not p.get("merged_at")}
    for branch, gitea_pr in gitea_prs_by_branch.items():
        if not branch.startswith(BRANCH_PREFIX):
            continue
        try:
            number = int(branch[len(BRANCH_PREFIX):])
        except ValueError:
            continue
        if number in closed_numbers:
            http_json(
                f"{GITEA_SERVER}/api/v1/repos/{GITEA_OWNER}/{GITEA_REPO}/pulls/{gitea_pr['number']}",
                basic_auth=(GITEA_USER, GITEA_TOKEN),
                method="PATCH",
                data={"state": "closed"},
            )
            print(f"PR #{number}: closed on GitHub without merge, closed Gitea PR #{gitea_pr['number']}.")


if __name__ == "__main__":
    try:
        main()
    except Exception as exc:
        print(f"error: {exc}", file=sys.stderr)
        sys.exit(1)
