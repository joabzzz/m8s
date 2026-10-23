#!/usr/bin/env python3
"""One explicit Nebius GPU smoke attempt. No ambient credentials or create retries."""
import argparse
import base64
import hashlib
import json
import os
from pathlib import Path
import re
import secrets
import shutil
import stat
import subprocess
import sys
import time
import urllib.error
import urllib.parse
import urllib.request

API = "https://api.nebius.cloud"
AUTH = "https://auth.eu.nebius.com/oauth2/token/exchange"
LOGS = "https://read.logging.api.nebius.cloud"
MARKER = "M8S_NEBIUS_GPU_SMOKE_V1 "
MODEL = "HuggingFaceTB/SmolLM2-135M-Instruct"
REVISION = "12fd25f77366fa6b3b4b768ec3050bf629380bac"


class Refused(Exception):
    pass


class NoRedirect(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, *args, **kwargs):
        raise Refused("redirect refused")


def component(value):
    if not isinstance(value, str) or not re.fullmatch(r"[a-zA-Z0-9._-]{1,128}", value):
        raise Refused("invalid identifier")
    return value


def write(path, value):
    # Each file has one writer: launcher, or independently surviving reaper.
    tmp = path.with_suffix(".new")
    with os.fdopen(os.open(tmp, os.O_CREAT | os.O_EXCL | os.O_WRONLY, 0o600), "w") as f:
        json.dump(value, f, sort_keys=True)
        f.flush()
        os.fsync(f.fileno())
    os.replace(tmp, path)
    fd = os.open(path.parent, os.O_RDONLY)
    try:
        os.fsync(fd)
    finally:
        os.close(fd)


class Client:
    def __init__(self, plan):
        self.plan = plan
        self.token = None
        self.expires = 0
        self.opener = urllib.request.build_opener(urllib.request.ProxyHandler({}), NoRedirect())

    def raw(self, url, method="GET", data=None, headers=None):
        if urllib.parse.urlparse(url).scheme != "https":
            raise Refused("HTTPS required")
        if urllib.parse.urlparse(url).netloc not in {
            urllib.parse.urlparse(x).netloc for x in [API, AUTH, LOGS]
        }:
            raise Refused("unexpected API origin")
        h = {"User-Agent": "curl/8.7.1", "Accept": "application/json"}
        h.update(headers or {})
        try:
            with self.opener.open(urllib.request.Request(url, data=data, method=method, headers=h), timeout=20) as f:
                body = f.read(8 * 1024 * 1024 + 1)
                if len(body) > 8 * 1024 * 1024:
                    raise Refused("oversized provider response")
                value = json.loads(body)
                if not isinstance(value, dict):
                    raise Refused("invalid provider envelope")
                return value
        except urllib.error.HTTPError as e:
            # Never print raw provider replies, headers, URLs, tokens, or causes.
            raise Refused("provider HTTP " + str(e.code)) from None
        except (OSError, ValueError):
            raise Refused("provider outcome unavailable or uncertain") from None

    def authenticate(self):
        if self.token and time.time() < self.expires:
            return
        key = Path(self.plan["private_key"])
        m = key.lstat()
        if not stat.S_ISREG(m.st_mode) or m.st_uid != os.getuid() or m.st_nlink != 1 or m.st_mode & 0o077:
            raise Refused("selected credential must be a private regular file")
        def b64(data):
            return base64.urlsafe_b64encode(data).rstrip(b"=")
        now = int(time.time())
        head = b64(json.dumps({"alg": "RS256", "typ": "JWT", "kid": self.plan["key_id"]}).encode())
        body = b64(json.dumps({"iss": self.plan["service_account"], "sub": self.plan["service_account"],
                               "iat": now, "exp": now + 300}).encode())
        payload = head + b"." + body
        signed = subprocess.run(["openssl", "dgst", "-sha256", "-sign", str(key)],
                                input=payload, capture_output=True, timeout=10)
        if signed.returncode:
            raise Refused("credential signing failed")
        jwt = (payload + b"." + b64(signed.stdout)).decode()
        data = urllib.parse.urlencode({"grant_type": "urn:ietf:params:oauth:grant-type:token-exchange",
            "requested_token_type": "urn:ietf:params:oauth:token-type:access_token",
            "subject_token": jwt, "subject_token_type": "urn:ietf:params:oauth:token-type:jwt"}).encode()
        value = self.raw(AUTH, "POST", data, {"Content-Type": "application/x-www-form-urlencoded"})
        self.token = value["access_token"]
        self.expires = now + min(int(value.get("expires_in", 300)), 3600) - 30

    def call(self, path, method="GET", body=None, origin=API):
        self.authenticate()
        data = json.dumps(body).encode() if body is not None else None
        return self.raw(origin + path, method, data,
                        {"Authorization": "Bearer " + self.token, "Content-Type": "application/json"})

    def inventory(self, kind):
        prefix = "/vpc/v1/" if kind == "allocations" else "/compute/v1/"
        page = ""
        seen, ids, rows = set(), set(), []
        for _ in range(100):
            query = {"parentId": self.plan["project"], "pageSize": 100}
            if page:
                query["pageToken"] = page
            value = self.call(prefix + kind + "?" + urllib.parse.urlencode(query))
            if set(value) - {"items", "nextPageToken"}:
                raise Refused("ambiguous inventory envelope")
            for row in value.get("items", []):
                md = row["metadata"]
                ident = component(md["id"])
                if md["parentId"] != self.plan["project"] or ident in ids:
                    raise Refused("foreign or duplicate inventory row")
                ids.add(ident)
                rows.append(row)
            page = value.get("nextPageToken", "")
            if not page:
                return rows
            if page in seen:
                raise Refused("repeated inventory page")
            seen.add(page)
        raise Refused("incomplete inventory")


def owned(plan, row):
    md = row.get("metadata", {})
    return (md.get("parentId") == plan["project"]
            and md.get("labels", {}).get("m8s-owner") == plan["owner"])


def create_failed(operation):
    # A just-created VM can be STOPPED while create is still reconciling.
    # Only the terminal operation can declare provisioning failure.
    return "status" in operation and operation["status"].get("code", 0) != 0


def cleanup(client, plan, directory):
    """Only the reaper deletes. Acknowledgement never counts as absence."""
    known = {"instances": set(), "disks": set(), "allocations": set()}
    journal = directory / "cleanup-journal.json"
    if journal.exists():
        previous = json.loads(journal.read_text())
        known = {k: set(previous[k]) for k in known}
    settled = not (directory / "launch-intent.json").exists()
    operation_file = directory / "operation.json"
    if operation_file.exists():
        operation = json.loads(operation_file.read_text())
        op_id = component(operation["id"])
        current = client.call("/compute/v1/instances/operations/" + op_id)
        # A terminal failed operation still needs inventory reconciliation.
        settled = "status" in current
    elif not settled:
        # Recover an uncertain create response, never resubmit it. Scope operation
        # discovery to the exact immutable owner/node labels in the original request.
        page = ""
        seen = set()
        for _ in range(100):
            query = {"parentId": plan["project"], "pageSize": 100}
            if page:
                query["pageToken"] = page
            response = client.call("/compute/v1/instances/list-operations-by-parent?" + urllib.parse.urlencode(query))
            for op in response.get("operations", response.get("items", [])):
                if owned(plan, op.get("request", {})):
                    write(operation_file, {k: op[k] for k in ["id", "resourceId", "status"] if k in op})
                    settled = "status" in op
                    break
            page = response.get("nextPageToken", "")
            if operation_file.exists() or not page:
                break
            if page in seen:
                raise Refused("incomplete operation inventory")
            seen.add(page)
    # Continue until explicit absence; a transient API error cannot discharge liability.
    for kind in ["instances", "disks", "allocations"]:
        rows = client.inventory(kind)
        exact = [row for row in rows if owned(plan, row)]
        for row in exact:
            ident = row["metadata"]["id"]
            known[kind].add(ident)
            if kind == "instances":
                for disk in row.get("status", {}).get("diskAttachments", []):
                    disk_id = disk.get("id", disk.get("diskId"))
                    if disk_id:
                        known["disks"].add(component(disk_id))
                for interface in row.get("status", {}).get("networkInterfaces", []):
                    for field in ["ipAddress", "publicIpAddress"]:
                        allocation = interface.get(field, {}).get("allocationId")
                        if allocation:
                            known["allocations"].add(component(allocation))
            write(journal, {k: sorted(v) for k, v in known.items()})
            # Dynamic IPs and managed disks follow instance deletion. Orphan disks
            # are deleted only with exact owner labels; unrelated resources stay untouched.
            if kind != "allocations":
                prefix = "/compute/v1/" + kind + "/" + component(ident)
                client.call(prefix, "DELETE")
    if not settled:
        # A complete empty snapshot cannot resolve an in-flight/invisible CREATE.
        return False
    # A second complete independent inventory, after acknowledgements.
    for kind in known:
        rows = client.inventory(kind)
        ids = {row["metadata"]["id"] for row in rows}
        if any(owned(plan, row) for row in rows) or ids & known[kind]:
            return False
    write(directory / "cleanup-evidence.json", {
        "schema": "m8s.nebius-smoke-cleanup/v1", "owner": plan["owner"],
        "provider_confirmed_absent": True, "observed_at": int(time.time()),
        "exact_resources": {k: sorted(v) for k, v in known.items()},
    })
    return True


def reaper(directory):
    plan = json.loads((directory / "plan.json").read_text())
    client = Client(plan)
    # Authenticate and read a complete inventory before promising cleanup custody.
    client.authenticate()
    client.inventory("instances")
    write(directory / "reaper-ready.json", {"owner": plan["owner"], "deadline": plan["deadline"],
          "pid": os.getpid(), "plan_sha256": hashlib.sha256((directory / "plan.json").read_bytes()).hexdigest()})
    while not (directory / "cleanup-requested.json").exists() and time.time() < plan["deadline"]:
        time.sleep(1)
    while True:
        try:
            if cleanup(client, plan, directory):
                return
        except Exception:
            # Uncertainty retains responsibility. No raw exceptions or credentials in logs.
            pass
        time.sleep(5)


def cloud_config(owner, guest):
    component(owner)
    script = """#!/bin/bash
set -euo pipefail
exec > >(tee -a /var/log/m8s-smoke.log /dev/ttyS0) 2>&1
echo M8S_NEBIUS_SMOKE_STAGE_PREPARING
# This stops local compute on runaway setup; exact deletion belongs to the owner reaper.
systemd-run --unit=m8s-smoke-deadline --on-active=18m /sbin/poweroff
nvidia-smi
export DEBIAN_FRONTEND=noninteractive
timeout 180 apt-get update -qq
timeout 180 apt-get install -y -qq python3-venv
python3 -m venv /opt/m8s-smoke/venv
timeout 480 /opt/m8s-smoke/venv/bin/pip install --disable-pip-version-check --only-binary=:all: torch==2.9.0 --index-url https://download.pytorch.org/whl/cu130
timeout 180 /opt/m8s-smoke/venv/bin/pip install --disable-pip-version-check --only-binary=:all: transformers==4.57.1
echo M8S_NEBIUS_SMOKE_STAGE_INFERENCE
export M8S_SMOKE_OWNER=OWNER_VALUE
timeout 180 /opt/m8s-smoke/venv/bin/python /opt/m8s-smoke/guest.py
""".replace("OWNER_VALUE", owner)
    value = {"ssh_pwauth": False, "disable_root": True, "write_files": [
        {"path": "/opt/m8s-smoke/guest.py", "permissions": "0444", "content": guest},
        {"path": "/opt/m8s-smoke/run.sh", "permissions": "0500", "content": script},
    ], "runcmd": [["bash", "-c", "/opt/m8s-smoke/run.sh || echo M8S_NEBIUS_SMOKE_SETUP_FAILED"]]}
    return "#cloud-config\n" + json.dumps(value)


def log_result(client, plan, start, instance):
    query = '{__bucket__="sp_serial"} |= "' + plan["owner"] + '"'
    params = {"query": query, "start": str(start * 1_000_000_000),
              "end": str(int(time.time()) * 1_000_000_000), "limit": "100", "direction": "backward"}
    value = client.call("/projects/" + plan["project"] + "/loki/api/v1/query_range?"
                        + urllib.parse.urlencode(params), origin=LOGS)
    for stream in value.get("data", {}).get("result", []):
        for _, line in stream.get("values", []):
            if MARKER not in line:
                continue
            # Nebius serial ingestion may wrap a line in a JSON log envelope.
            try:
                wrapper = json.loads(line)
                if isinstance(wrapper, dict):
                    line = wrapper.get("message", line)
            except ValueError:
                pass
            marker = line.find(MARKER)
            result, _ = json.JSONDecoder().raw_decode(line[marker + len(MARKER):])
            if result.get("owner") != plan["owner"]:
                continue
            if result.get("success") and result.get("instance_id") != instance:
                raise Refused("serial result has wrong instance identity")
            if result.get("success") and (
                result.get("schema") != "m8s.nebius-gpu-smoke/v1" or
                result.get("model") != MODEL or result.get("revision") != REVISION or
                "B300" not in result.get("gpu", "") or
                not isinstance(result.get("completion"), str) or not result["completion"].strip() or
                not isinstance(result.get("generated_tokens"), int) or
                not 1 <= result["generated_tokens"] <= 32 or
                result.get("ready") is not False or result.get("authenticated_m8s_routing") is not False
            ):
                raise Refused("invalid GPU smoke result")
            return result
    return None


def launch(args):
    directory = Path(args.directory).absolute()
    directory.mkdir(mode=0o700, parents=False, exist_ok=False)
    for value in [args.service_account, args.key_id, args.project, args.tenant, args.image, args.subnet]:
        component(value)
    start = int(time.time())
    plan = {"schema": "m8s.nebius-smoke-plan/v1", "owner": "m8s-smoke-" + secrets.token_hex(12),
        "project": args.project, "tenant": args.tenant, "region": "us-north1",
        "service_account": args.service_account, "key_id": args.key_id,
        "private_key": str(Path(args.private_key).absolute()), "image": args.image,
        "subnet": args.subnet, "started_at": start, "deadline": start + 1200,
        "platform": "gpu-b300-sxm", "preset": "1gpu-24vcpu-346gb", "disk_gib": 128}
    write(directory / "plan.json", plan)
    client = Client(plan)
    quota_path = "/quotas/v1/quota-allowances/get-by-name?" + urllib.parse.urlencode({
        "parentId": plan["tenant"], "region": plan["region"], "name": "compute.instance.gpu.b300"})
    quota = client.call(quota_path)
    qs, qt = quota.get("spec", {}), quota.get("status", {})
    if (qt.get("state") != "STATE_ACTIVE" or qt.get("usageState") not in
            ["USAGE_STATE_USED", "USAGE_STATE_NOT_USED"] or
            int(qs.get("limit", 0)) - int(qt.get("usage", 0)) < 1):
        raise Refused("active tenant GPU allowance unavailable")
    write(directory / "quote.json", {"compute_hourly_micro_usd": 9500000,
          "quote_source": "https://docs.nebius.com/compute/resources/pricing",
          "compute_reserve_micro_usd": 3166667, "runtime_seconds": 1200,
          "storage_gib": 128, "storage_and_network_additional": True,
          "user_scope": "one B300 smoke; no separate agent dollar cap"})
    baseline = {k: [i["metadata"]["id"] for i in client.inventory(k)]
                for k in ["instances", "disks", "allocations"]}
    if baseline["instances"] or baseline["disks"]:
        raise Refused("smoke requires initially empty project VM and disk inventories")
    write(directory / "baseline.json", baseline)
    # Copy the exact reviewed source, not any credentials, into the private attempt.
    shutil.copyfile(__file__, directory / "reaper.py")
    os.chmod(directory / "reaper.py", 0o500)
    with (directory / "reaper.log").open("xb") as log:
        child = subprocess.Popen([sys.executable, str(directory / "reaper.py"), "reaper", "--directory", str(directory)],
            stdin=subprocess.DEVNULL, stdout=log, stderr=log, start_new_session=True, close_fds=True)
    wait = time.monotonic() + 40
    while not (directory / "reaper-ready.json").exists():
        if child.poll() is not None or time.monotonic() >= wait:
            raise Refused("independent cleanup could not be armed; no create submitted")
        time.sleep(0.25)
    ready = json.loads((directory / "reaper-ready.json").read_text())
    expected = hashlib.sha256((directory / "plan.json").read_bytes()).hexdigest()
    if ready.get("owner") != plan["owner"] or ready.get("deadline") != plan["deadline"] or ready.get("plan_sha256") != expected:
        raise Refused("cleanup receipt mismatch")
    guest = Path(__file__).with_name("guest.py").read_text()
    labels = {"m8s-owner": plan["owner"], "m8s-node-uid": plan["owner"], "m8s-tier": "gpu-smoke", "m8s-version": "1"}
    body = {"metadata": {"parentId": args.project, "name": plan["owner"], "labels": labels},
        "spec": {"resources": {"platform": plan["platform"], "preset": plan["preset"]},
            "recoveryPolicy": "FAIL", "onDemand": {}, "bootDisk": {"attachMode": "READ_WRITE", "managedDisk": {
                "name": plan["owner"] + "-boot", "labels": labels,
                "spec": {"type": "NETWORK_SSD", "sizeGibibytes": "128", "sourceImageId": args.image}}},
            "networkInterfaces": [{"name": "eth0", "subnetId": args.subnet, "ipAddress": {}, "publicIpAddress": {}}],
            "cloudInitUserData": cloud_config(plan["owner"], guest)}}
    result = None
    failure = None
    try:
        if time.time() + 300 >= plan["deadline"] or child.poll() is not None:
            raise Refused("cleanup window no longer available; no create submitted")
        # Durable intent before the only create submission. No retry, including HTTP errors.
        write(directory / "launch-intent.json", {"owner": plan["owner"], "submitted_at": int(time.time()),
              "request_sha256": hashlib.sha256(json.dumps(body, sort_keys=True).encode()).hexdigest()})
        op = client.call("/compute/v1/instances", "POST", body)
        write(directory / "operation.json", {k: op[k] for k in ["id", "resourceId", "status"] if k in op})
        if create_failed(op):
            raise Refused("provider rejected create operation")
        instance = op.get("resourceId")
        print("Create submitted; independent cleanup armed", flush=True)
        while time.time() < plan["deadline"] - 150:
            current = client.call("/compute/v1/instances/operations/" + component(op["id"]))
            if create_failed(current):
                raise Refused("provider create operation failed")
            instances = [i for i in client.inventory("instances") if owned(plan, i)]
            if len(instances) > 1:
                raise Refused("ambiguous owned instances; cleanup required")
            if instances:
                observed = instances[0]["metadata"]["id"]
                if instance and instance != observed:
                    raise Refused("create and inventory identities disagree")
                instance = observed
                write(directory / "instance.json", {"id": instance, "owner": plan["owner"]})
                try:
                    result = log_result(client, plan, start, instance)
                except Refused as e:
                    if str(e) != "provider HTTP 404":
                        raise
                if result:
                    write(directory / "inference-evidence.json", result)
                    if not result.get("success"):
                        raise Refused("guest GPU inference failed")
                    break
            time.sleep(10)
        else:
            raise Refused("GPU smoke deadline expired")
    except Exception as e:
        failure = str(e) if isinstance(e, Refused) else type(e).__name__
    finally:
        write(directory / "cleanup-requested.json", {"requested_at": int(time.time())})
        wait = time.monotonic() + 300
        while not (directory / "cleanup-evidence.json").exists() and time.monotonic() < wait:
            time.sleep(2)
    if not (directory / "cleanup-evidence.json").exists():
        raise Refused("cleanup unconfirmed; reaper remains responsible, retain liability")
    if failure:
        raise Refused(failure + "; provider-confirmed cleanup completed")
    print(json.dumps({"inference": result, "cleanup": json.loads((directory / "cleanup-evidence.json").read_text())}), flush=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    sub = parser.add_subparsers(dest="command", required=True)
    p = sub.add_parser("launch")
    for name in ["directory", "private-key", "service-account", "key-id", "project", "tenant", "image", "subnet"]:
        p.add_argument("--" + name, required=True)
    p = sub.add_parser("reaper")
    p.add_argument("--directory", required=True)
    args = parser.parse_args()
    if args.command == "launch":
        launch(args)
    else:
        reaper(Path(args.directory))


if __name__ == "__main__":
    try:
        main()
    except Exception as e:
        print("Nebius smoke refused: " + (str(e) if isinstance(e, Refused) else type(e).__name__), file=sys.stderr)
        sys.exit(1)
