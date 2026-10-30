import copy
import importlib.util
from pathlib import Path
import unittest

spec = importlib.util.spec_from_file_location("verify", Path(__file__).with_name("verify-release.py"))
verify = importlib.util.module_from_spec(spec)
spec.loader.exec_module(verify)
SHA = "a" * 40


class ReleaseGateTests(unittest.TestCase):
    def setUp(self):
        self.run = dict(id=1, head_sha=SHA, head_branch="main", event="push",
                        head_repository={"full_name": verify.REPO},
                        path=".github/workflows/ci.yml", status="completed", conclusion="success")
        self.jobs = [dict(name=name, head_sha=SHA, status="completed", conclusion="success")
                     for name in verify.REQUIRED_JOBS]

    def test_same_commit_successful_main_ci_is_accepted(self):
        for event in ("push", "workflow_dispatch", "schedule"):
            self.run["event"] = event
            self.assertEqual(verify.verified_run([self.run], SHA)["id"], 1)
        verify.verify_jobs(self.jobs, SHA)

    def test_pr_fork_different_commit_branch_and_workflow_cannot_authorize_release(self):
        for change in [dict(head_sha="b" * 40), dict(head_branch="feature"),
                       dict(event="pull_request"), dict(head_repository={"full_name": "other/fork"}),
                       dict(path=".github/workflows/other.yml")]:
            with self.subTest(change=change), self.assertRaises(ValueError):
                verify.verified_run([dict(self.run, **change)], SHA)

    def test_latest_failure_or_pending_run_blocks_older_success(self):
        for status, conclusion in [("completed", "failure"), ("completed", "cancelled"),
                                   ("queued", None), ("in_progress", None)]:
            newer = dict(self.run, id=2, status=status, conclusion=conclusion)
            with self.subTest(status=status, conclusion=conclusion), self.assertRaises(ValueError):
                verify.verified_run([newer, self.run], SHA)

    def test_missing_skipped_or_failed_required_job_blocks_release(self):
        with self.assertRaises(ValueError):
            verify.verify_jobs(self.jobs[:-1], SHA)
        for change in [dict(conclusion="skipped"), dict(conclusion="failure"),
                       dict(status="in_progress"), dict(head_sha="b" * 40)]:
            jobs = copy.deepcopy(self.jobs)
            jobs[0].update(change)
            with self.subTest(change=change), self.assertRaises(ValueError):
                verify.verify_jobs(jobs, SHA)

    def test_no_success_without_ci(self):
        with self.assertRaises(ValueError):
            verify.verified_run([], SHA)


if __name__ == "__main__":
    unittest.main()
