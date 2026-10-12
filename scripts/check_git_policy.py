#!/usr/bin/env python3
"""Validate PR routing/names and require a successful yulius/ci status on the dev head before promotion."""
import json
import os
from pathlib import Path
import re
import sys
from urllib.request import Request, urlopen

TYPES = r"(?:feat|fix|refactor|perf|docs|test|build|ci|chore|revert)"
BRANCH = re.compile(TYPES + r"/[a-z0-9]+(?:-[a-z0-9]+)*\Z")
TITLE = re.compile(TYPES + r"(?:\([a-z0-9]+(?:-[a-z0-9]+)*\))?!?: \S.*\Z")
RELEASE = re.compile(r"release: promote dev to main(?: - \S.*)?\Z")


def validate_pr(pr, repository):
    """Return whether this is a promotion; raise on an invalid route or name."""
    head, base, title = pr["head"]["ref"], pr["base"]["ref"], pr["title"]
    if len(title) > 100 or "\n" in title or "\r" in title:
        raise ValueError("PR title must be one line of at most 100 characters")
    if base == "main":
        if head != "dev" or pr["head"].get("repo", {}).get("full_name") != repository:
            raise ValueError("main accepts promotion PRs only from this repository's dev branch")
        if not RELEASE.fullmatch(title):
            raise ValueError("Promotion title must be 'release: promote dev to main' with an optional ' - label'")
        return True
    if base != "dev":
        raise ValueError("PR base must be dev (changes) or main (promotion)")
    if len(head) > 80 or not BRANCH.fullmatch(head) or not re.search(r"[a-z]", head.split("/", 1)[-1]):
        raise ValueError("Use a typed short-lived branch, e.g. feat/42-task-cancellation (max 80 characters)")
    if not TITLE.fullmatch(title):
        raise ValueError("Use a conventional PR title, e.g. feat(tasks): add cancellation")
    return False


def validate_dev_status(statuses, sha):
    """Require Yulius CI's `yulius/ci` commit status of the exact dev head to be successful
    (Yulius is the only CI, #184). Statuses come newest first; the newest one decides."""
    mine = [status for status in statuses if status.get("context") == "yulius/ci"]
    if not mine:
        raise ValueError(f"No yulius/ci status exists for the promotion head {sha}")
    latest = mine[0]
    if latest.get("state") != "success":
        raise ValueError(f"yulius/ci for {sha} is {latest.get('state')}; wait/fix before promotion")
    return latest.get("id")


def api(path):
    token = os.environ["GH_TOKEN"]
    request = Request("https://api.github.com/" + path, headers={
        "Authorization": f"Bearer {token}", "Accept": "application/vnd.github+json",
        "X-GitHub-Api-Version": "2022-11-28", "User-Agent": "yulius-branch-policy"})
    with urlopen(request, timeout=30) as response:
        return json.load(response)


def main():
    event = json.loads(Path(os.environ["GITHUB_EVENT_PATH"]).read_text())
    if os.environ.get("GITHUB_EVENT_NAME") != "pull_request":
        print("Branch policy: post-merge/scheduled run; quality gates verify this commit")
        return
    repository = os.environ["GITHUB_REPOSITORY"]
    pr = event["pull_request"]
    if validate_pr(pr, repository):
        sha = pr["head"]["sha"]
        current = api(f"repos/{repository}/git/ref/heads/dev")["object"]["sha"]
        if current != sha:
            raise ValueError("dev advanced; use fresh promotion-head checks")
        statuses = api(f"repos/{repository}/commits/{sha}/statuses?per_page=100")
        status_id = validate_dev_status(statuses, sha)
        print(f"Promotion evidence: successful yulius/ci status {status_id} on {sha}")
    print("Branch policy passed")


if __name__ == "__main__":
    try:
        main()
    except (KeyError, OSError, ValueError) as error:
        sys.exit(f"Branch policy failed: {error}")
