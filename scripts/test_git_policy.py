"""Behavioral tests for routing, naming and exact-SHA promotion evidence."""
import unittest
from check_git_policy import validate_pr, validate_dev_run

REPO = "Quartio-Studio/yulius-v2"


def pr(head="feat/42-task-cancellation", base="dev", title="feat(tasks): add cancellation", repo=REPO):
    return {"head": {"ref": head, "repo": {"full_name": repo}}, "base": {"ref": base}, "title": title}


def run(sha="abc", status="completed", conclusion="success", event="push", branch="dev", identifier=1):
    return {"head_sha": sha, "status": status, "conclusion": conclusion,
            "event": event, "head_branch": branch, "id": identifier}


class GitPolicy(unittest.TestCase):
    def test_work_branch_and_conventional_title_are_accepted(self):
        self.assertFalse(validate_pr(pr(), REPO))
        self.assertFalse(validate_pr(pr(head="fix/event-replay", title="fix(events)!: replace protocol"), REPO))

    def test_wrong_routes_names_and_titles_are_rejected(self):
        for case in [pr(base="main"), pr(base="prod"), pr(head="main"), pr(head="dev"),
                     pr(head="feat/user/nested"), pr(head="feat/123"), pr(head="feat/Bad_name"),
                     pr(head="feat/" + "x"*80), pr(title="WIP: fix it"), pr(title="fix: x\ny"),
                     pr(title="fix: " + "x"*100)]:
            with self.subTest(case=case), self.assertRaises(ValueError):
                validate_pr(case, REPO)

    def test_only_same_repository_dev_can_promote(self):
        self.assertTrue(validate_pr(pr(head="dev", base="main", title="release: promote dev to main"), REPO))
        with self.assertRaises(ValueError):
            validate_pr(pr(head="dev", base="main", title="release: promote dev to main", repo="other/fork"), REPO)

    def test_exact_successful_dev_push_is_required(self):
        self.assertEqual(validate_dev_run([run()], "abc"), 1)
        for runs in [[], [run(sha="old")], [run(event="pull_request")], [run(branch="main")],
                     [run(status="in_progress", conclusion=None)], [run(conclusion="failure")],
                     [run(conclusion="cancelled")], [run(conclusion="skipped")]]:
            with self.subTest(runs=runs), self.assertRaises(ValueError):
                validate_dev_run(runs, "abc")

    def test_later_failed_run_cannot_be_hidden_by_older_success(self):
        with self.assertRaises(ValueError):
            validate_dev_run([run(), run(identifier=2, conclusion="failure")], "abc")
