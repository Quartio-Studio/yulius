"""Behavioral tests for routing, naming and exact-SHA promotion evidence."""
import unittest
from check_git_policy import validate_pr, validate_dev_status

REPO = "Quartio-Studio/yulius-v2"


def pr(head="feat/42-task-cancellation", base="dev", title="feat(tasks): add cancellation", repo=REPO):
    return {"head": {"ref": head, "repo": {"full_name": repo}}, "base": {"ref": base}, "title": title}


def status(state="success", context="yulius/ci", identifier=1):
    return {"state": state, "context": context, "id": identifier}


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

    def test_a_successful_yulius_ci_status_on_the_dev_head_is_required(self):
        self.assertEqual(validate_dev_status([status()], "abc"), 1)
        for statuses in [[], [status(context="ci/other")], [status(state="pending")],
                         [status(state="failure")], [status(state="error")]]:
            with self.subTest(statuses=statuses), self.assertRaises(ValueError):
                validate_dev_status(statuses, "abc")

    def test_a_newer_failure_cannot_be_hidden_by_an_older_success(self):
        with self.assertRaises(ValueError):
            validate_dev_status([status(state="failure", identifier=2), status()], "abc")
