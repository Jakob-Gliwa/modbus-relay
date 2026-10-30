import copy
import importlib.util
from pathlib import Path
import unittest
from unittest.mock import patch

spec = importlib.util.spec_from_file_location("updates", Path(__file__).with_name("automate-updates.py"))
updates = importlib.util.module_from_spec(spec)
spec.loader.exec_module(updates)


class PolicyTests(unittest.TestCase):
    def setUp(self):
        self.old = {"package": [
            {"name": "relay", "version": "0.1.0"},
            {"name": "library", "version": "0.3.10", "source": updates.REGISTRY, "checksum": "old"},
        ]}
        self.new = copy.deepcopy(self.old)
        self.new["package"][1].update(version="0.3.11", checksum="new")

    def test_stable_patch_allowed_including_zero_major(self):
        self.assertTrue(updates.patch_only(self.old, self.new))

    def test_minor_major_prerelease_and_downgrade_need_review(self):
        for value in ["0.4.0", "1.0.0", "0.3.9", "0.3.12-rc.1", "0.3.11+custom"]:
            with self.subTest(value=value):
                self.new["package"][1]["version"] = value
                self.assertFalse(updates.patch_only(self.old, self.new))

    def test_source_changes_new_crates_and_local_changes_need_review(self):
        for mutate in [
            lambda d: d["package"][1].update(source="git+https://example.com/library"),
            lambda d: d["package"].append({"name": "new", "version": "1.0.0"}),
            lambda d: d["package"][0].update(version="0.1.1"),
            lambda d: d["package"].pop(0),
        ]:
            value = copy.deepcopy(self.new)
            mutate(value)
            self.assertFalse(updates.patch_only(self.old, value))

    def test_unchanged_version_cannot_change_checksum(self):
        self.new["package"][1]["version"] = "0.3.10"
        self.assertFalse(updates.patch_only(self.old, self.new))
        self.assertFalse(updates.patch_only(self.old, self.old))

    def test_multiple_release_lines_and_unchanged_build_metadata(self):
        extra = [{"name": "library", "version": "1.0.0", "source": updates.REGISTRY},
                 {"name": "toml", "version": "1.1.8+spec-1.1.0", "source": updates.REGISTRY}]
        self.old["package"].extend(extra)
        self.new["package"].extend(extra)
        self.assertTrue(updates.patch_only(self.old, self.new))

    def test_ambiguous_duplicate_versions_need_review(self):
        self.new["package"].append(copy.deepcopy(self.new["package"][1]))
        self.assertFalse(updates.patch_only(self.old, self.new))

    def test_release_number_is_numeric_and_existing_tag_is_reused(self):
        tags = [("v0.1.9", "old"), ("v0.1.10", "current"), ("v9.0.0-rc.1", "rc")]
        self.assertEqual(updates.release_tag(tags, "new"), ("v0.1.11", True))
        self.assertEqual(updates.release_tag(tags, "current"), ("v0.1.10", False))
        with self.assertRaises(ValueError):
            updates.release_tag([], "new")

    def test_latest_failed_ci_blocks_publication(self):
        runs = [dict(id=1, head_branch="main", event="push", conclusion="success"),
                dict(id=2, head_branch="main", event="schedule", conclusion="failure")]
        with patch.object(updates, "api", side_effect=[{"object": {"sha": "abc"}}, {"workflow_runs": runs}]), \
             patch.object(updates, "dispatch") as dispatch:
            updates.publish_main()
            dispatch.assert_not_called()

    def test_bot_merge_without_ci_dispatches_main_checks(self):
        with patch.object(updates, "api", side_effect=[{"object": {"sha": "abc"}}, {"workflow_runs": []}]), \
             patch.object(updates, "dispatch") as dispatch:
            updates.publish_main()
            dispatch.assert_called_once_with("ci.yml", "main")

    def test_active_release_prevents_overlapping_publication(self):
        runs = [dict(id=1, head_branch="main", event="push", conclusion="success")]
        with patch.object(updates, "api", side_effect=[{"object": {"sha": "abc"}},
                {"workflow_runs": runs}, {"workflow_runs": [{"status": "in_progress"}]}]), \
             patch.object(updates, "dispatch") as dispatch:
            updates.publish_main()
            dispatch.assert_not_called()

    def test_publication_rechecks_main_and_explicitly_dispatches_tag(self):
        runs = [dict(id=1, head_branch="main", event="push", conclusion="success")]
        for current, expected in [("abc", True), ("changed", False)]:
            with self.subTest(current=current), \
                 patch.object(updates, "api", side_effect=[{"object": {"sha": "abc"}},
                     {"workflow_runs": runs}, {"workflow_runs": []},
                     {"object": {"sha": current}}, {}]) as api, \
                 patch.object(updates, "pages", return_value=[{"name": "v0.1.0", "commit": {"sha": "old"}}]), \
                 patch.object(updates, "dispatch") as dispatch:
                updates.publish_main()
                if expected:
                    api.assert_called_with("git/refs", "POST", {"ref": "refs/tags/v0.1.1", "sha": "abc"})
                    dispatch.assert_called_once_with("release.yml", "v0.1.1")
                else:
                    dispatch.assert_not_called()

    def test_published_or_failed_release_is_not_repeated(self):
        runs = [dict(id=1, head_branch="main", event="push", conclusion="success")]
        for conclusion in ["success", "failure"]:
            with self.subTest(conclusion=conclusion), \
                 patch.object(updates, "api", side_effect=[{"object": {"sha": "abc"}},
                     {"workflow_runs": runs}, {"workflow_runs": [dict(status="completed",
                         head_branch="v0.1.0", head_sha="abc", conclusion=conclusion)]}]), \
                 patch.object(updates, "pages", return_value=[{"name": "v0.1.0", "commit": {"sha": "abc"}}]), \
                 patch.object(updates, "dispatch") as dispatch:
                updates.publish_main()
                dispatch.assert_not_called()

    def test_foreign_author_or_fork_is_never_merged(self):
        for author, repo in [("human", updates.REPO), ("dependabot[bot]", "other/fork")]:
            pr = {"user": {"login": author, "type": "Bot"}, "draft": False,
                  "head": {"repo": {"full_name": repo}}}
            with patch.object(updates, "pages", return_value=[pr]), patch.object(updates, "api") as api:
                self.assertFalse(updates.merge_patches())
                api.assert_not_called()


if __name__ == "__main__":
    unittest.main()
