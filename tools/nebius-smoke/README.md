# One-shot Nebius GPU smoke

This separate operator tool provisions one B300 VM, runs a pinned 135M model on
CUDA, reads its completion through authenticated Nebius serial logs, and deletes
the VM. It does not change `m8s_provider::nebius::ReadonlyHttps` into a live mutation
transport, test authenticated m8s model routing, or advertise Ready.

The [2026-10-23 evidence](evidence-2026-10-23.json) records a B300 completion of
**Hello!**, PyTorch 2.9.0/CUDA 13.0, and confirmed removal of the VM, managed boot
disk and both IP allocations. Final project inventories were empty. It also
retains the initial, prematurely aborted attempt and its confirmed cleanup;
neither attempt's accounting or identity was reset. Billing remains unreconciled.

Supply an explicitly selected private-key path, authorized-key/service-account
IDs, project/tenant IDs, and freshly observed subnet and immutable public image
IDs. No credential values go in argv, output, cloud-init, tests or the repository.
The VM receives no service account, private account key, or SSH access. Only
public model/test code goes into cloud-init.

Before using `launch`, verify tenant/project quota, current all-in prices, physical
capacity eligibility, and complete initially empty instance/disk inventories. The
tool deliberately selects one `gpu-b300-sxm` / `1gpu-24vcpu-346gb`, a 128 GiB managed
boot disk, and a dynamic public IP in `us-north1`. Hardware is region-dependent;
this preset is for an explicitly authorized B300 smoke, not cheapest-GPU discovery.
Use the provider's recommended CUDA 13 image, pinned by its current image ID.

Run a new attempt in an absent private directory outside the repository:

```sh
python3 tools/nebius-smoke/smoke.py launch \
  --directory /absolute/private/new-attempt \
  --private-key /absolute/selected/private.pem \
  --service-account SERVICE_ACCOUNT_ID --key-id AUTHORIZED_KEY_ID \
  --project PROJECT_ID --tenant TENANT_ID --image IMAGE_ID --subnet SUBNET_ID
```

The launcher durably records intent before its single create submission. It never
retries create. A detached process authenticates and acknowledges the exact plan
before create, survives launcher process-group loss, and cleans at completion or
the original 20-minute deadline. The guest also schedules power-off at 18 minutes;
stopped is not deleted, so that timer does not replace owner cleanup. Managed disks
and dynamic IPs follow VM deletion and are independently checked afterward.
Cleanup scans only exact owner labels, retains known resource identities, and
requires a terminal create operation plus complete instance/disk/IP inventories
before writing `cleanup-evidence.json`. An invisible uncertain create remains
unresolved; never replace/retry it based on empty inventory alone.

The run uses PyTorch 2.9.0 CUDA 13 wheels and Transformers 4.57.1, with binary-only
installation (no compilation on the rental). Model:
`HuggingFaceTB/SmolLM2-135M-Instruct` at
`12fd25f77366fa6b3b4b768ec3050bf629380bac`, safetensors only, no remote model code.
Both a CUDA kernel and model parameters/output must execute on the GPU. The result
includes provider metadata instance ID and exact run owner, checked against the
owner's authenticated provider inventory. Serial logs are read over verified
HTTPS, not through an unauthenticated public model endpoint.

If cleanup is unconfirmed, preserve the run directory and reaper; do not infer
absence from HTTP errors, delete acknowledgements, guest shutdown, or local exit.
The detached process continues retrying cleanup rather than silently releasing
responsibility. Credentials remain at the explicitly selected original path for
token renewal; do not remove that key until cleanup is confirmed.

Synthetic checks (no credentials/provider calls):

```sh
python3 -B -m unittest discover -s tools/nebius-smoke -v
```

References: [image compatibility](https://docs.nebius.com/compute/storage/boot-disk-images),
[serial logs](https://docs.nebius.com/compute/monitoring/serial-logs),
[log authentication](https://docs.nebius.com/observability/logs/logcli),
[managed-disk deletion](https://docs.nebius.com/compute/virtual-machines/delete).
