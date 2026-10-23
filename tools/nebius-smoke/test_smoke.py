import importlib.util
import json
from pathlib import Path
import tempfile
import unittest

spec = importlib.util.spec_from_file_location("smoke", Path(__file__).with_name("smoke.py"))
smoke = importlib.util.module_from_spec(spec)
spec.loader.exec_module(smoke)


class Fake:
    def __init__(self, rows=None, op=None, delayed=False):
        self.rows = rows or {"instances": [], "disks": [], "allocations": []}
        self.op = op if op is not None else {"id": "operation-fixture", "status": {}}
        self.delayed = delayed
        self.calls = []

    def inventory(self, kind):
        return list(self.rows[kind])

    def call(self, path, method="GET", body=None, origin=smoke.API):
        self.calls.append((method, path))
        if method == "DELETE":
            if not self.delayed:
                ident = path.rsplit("/", 1)[1]
                for kind in self.rows:
                    self.rows[kind] = [r for r in self.rows[kind] if r["metadata"]["id"] != ident]
            return {"id": "operation-delete"}
        if "list-operations" in path:
            return {"operations": []}
        return self.op


class SafetyTests(unittest.TestCase):
    def setUp(self):
        self.plan = {"project": "project-fixture", "owner": "m8s-smoke-fixture"}
        self.tmp = tempfile.TemporaryDirectory()
        self.directory = Path(self.tmp.name)

    def tearDown(self):
        self.tmp.cleanup()

    def row(self, ident, owner=None):
        return {"metadata": {"id": ident, "parentId": "project-fixture",
                             "labels": {"m8s-owner": owner or self.plan["owner"]}}}

    def test_cleanup_only_deletes_exact_owner_and_verifies_absence(self):
        foreign = self.row("instance-foreign", "m8s-smoke-fixture-other")
        f = Fake({"instances": [self.row("instance-owned"), foreign],
                  "disks": [self.row("disk-owned")], "allocations": []})
        self.assertTrue(smoke.cleanup(f, self.plan, self.directory))
        self.assertEqual(f.rows["instances"], [foreign])
        self.assertEqual([c[1] for c in f.calls],
                         ["/compute/v1/instances/instance-owned", "/compute/v1/disks/disk-owned"])
        evidence = json.loads((self.directory / "cleanup-evidence.json").read_text())
        self.assertTrue(evidence["provider_confirmed_absent"])

    def test_delete_acknowledgement_is_not_absence(self):
        f = Fake({"instances": [self.row("instance-owned")], "disks": [], "allocations": []}, delayed=True)
        self.assertFalse(smoke.cleanup(f, self.plan, self.directory))
        self.assertFalse((self.directory / "cleanup-evidence.json").exists())
        journal = json.loads((self.directory / "cleanup-journal.json").read_text())
        self.assertEqual(journal["instances"], ["instance-owned"])

    def test_empty_inventory_cannot_resolve_unknown_create(self):
        smoke.write(self.directory / "launch-intent.json", {})
        self.assertFalse(smoke.cleanup(Fake(), self.plan, self.directory))
        self.assertFalse((self.directory / "cleanup-evidence.json").exists())

    def test_inflight_operation_cannot_release_liability(self):
        smoke.write(self.directory / "launch-intent.json", {})
        smoke.write(self.directory / "operation.json", {"id": "operation-fixture"})
        self.assertFalse(smoke.cleanup(Fake(op={"id": "operation-fixture"}), self.plan, self.directory))

    def test_inflight_create_is_not_failed_by_transient_stopped_vm(self):
        self.assertFalse(smoke.create_failed({"id": "operation-fixture"}))
        self.assertFalse(smoke.create_failed({"id": "operation-fixture", "status": {}}))
        self.assertTrue(smoke.create_failed({"id": "operation-fixture", "status": {"code": 10}}))

    def test_completed_operation_and_independent_inventory_allow_absence(self):
        smoke.write(self.directory / "launch-intent.json", {})
        smoke.write(self.directory / "operation.json", {"id": "operation-fixture"})
        self.assertTrue(smoke.cleanup(Fake(), self.plan, self.directory))

    def test_bound_unlabelled_allocation_must_disappear(self):
        smoke.write(self.directory / "cleanup-journal.json",
                    {"instances": [], "disks": [], "allocations": ["allocation-owned"]})
        f = Fake({"instances": [], "disks": [], "allocations": [self.row("allocation-owned", "other")]})
        self.assertFalse(smoke.cleanup(f, self.plan, self.directory))

    def test_live_attachment_identity_is_retained_before_managed_disk_disappears(self):
        row = self.row("instance-owned")
        row["status"] = {"diskAttachments": [{"id": "disk-owned", "isManaged": True}]}
        f = Fake({"instances": [row], "disks": [], "allocations": []})
        self.assertTrue(smoke.cleanup(f, self.plan, self.directory))
        journal = json.loads((self.directory / "cleanup-journal.json").read_text())
        self.assertEqual(journal["disks"], ["disk-owned"])

    def test_partial_inventory_and_duplicates_fail_closed(self):
        for value in [{"error": "fixture"}, {"items": [self.row("id"), self.row("id")]}]:
            client = smoke.Client(self.plan)
            client.call = lambda path: value
            with self.assertRaises(smoke.Refused):
                client.inventory("instances")

    def test_repeated_inventory_page_refused(self):
        client = smoke.Client(self.plan)
        client.call = lambda path: {"nextPageToken": "repeated"}
        with self.assertRaises(smoke.Refused):
            client.inventory("instances")

    def test_cloud_init_has_no_provider_credentials_or_ssh(self):
        text = smoke.cloud_config("m8s-smoke-fixture", "# synthetic guest")
        config = json.loads(text.split("\n", 1)[1])
        self.assertFalse(config["ssh_pwauth"])
        for bad in ["private.pem", "access_token", "serviceaccount-", "authorized_keys", "BEGIN PRIVATE"]:
            self.assertNotIn(bad, text)
        self.assertIn("--only-binary=:all:", text)

    def test_serial_success_is_bound_to_instance_model_and_actual_output(self):
        result = {"schema": "m8s.nebius-gpu-smoke/v1", "owner": self.plan["owner"],
                  "instance_id": "instance-fixture", "success": True, "model": smoke.MODEL,
                  "revision": smoke.REVISION, "gpu": "NVIDIA B300", "completion": "Hello!",
                  "generated_tokens": 2, "ready": False, "authenticated_m8s_routing": False}
        class Logs:
            def call(self, *args, **kwargs):
                return {"data": {"result": [{"values": [["1", smoke.MARKER + json.dumps(result)]]}]}}
        self.assertTrue(smoke.log_result(Logs(), self.plan, 0, "instance-fixture")["success"])
        for key, bad in [("instance_id", "foreign"), ("model", "other"),
                         ("completion", ""), ("generated_tokens", 0), ("ready", True)]:
            previous = result[key]
            result[key] = bad
            with self.assertRaises(smoke.Refused):
                smoke.log_result(Logs(), self.plan, 0, "instance-fixture")
            result[key] = previous


if __name__ == "__main__":
    unittest.main()
