# Desktop Qwen 27B demo

The desktop integration is implemented on
[joabzzz/mesh-console, demo/m8s-qwen27](https://github.com/joabzzz/mesh-console/tree/demo/m8s-qwen27).
The [67-second demo video](assets/qwen27-desktop-demo.mp4?raw=true) and
[30-second README GIF](assets/qwen27-desktop-demo.gif) are included in this repository.
It uses the Tauri app's existing chat renderer and live peer sidebar, with an
opt-in Add compute / Remove compute flow. Upstream Mesh-LLM/desktop-app is
unchanged. This first demo creates a dedicated network using m8s's certified,
model-free owner client; attaching compute to an arbitrary existing desktop
network remains future work.

The desktop reads a private attachment descriptor containing only public owner
identity and loopback ports. Keys and provider credentials stay outside the
WebView. Ready requires a live verified client, the exact discovered model and
the coordinator's actual owner-routed canary. Chat pins both request and response
model identities and preserves a bounded two-turn conversation. Removal requires
the exact-absence stage and a matching provider-removal receipt.

See the fork's [setup and recording instructions](https://github.com/joabzzz/mesh-console/blob/demo/m8s-qwen27/docs/M8S_DEMO.md).
The manual recorder drives the actual frontend against the running desktop
backend; it does not require macOS screen-recording permission. This captures the
app interface without the native window frame. Synthetic rehearsals are not GPU
proof.

The operator supplies a fresh private plan following [the Qwen runbook](QWEN38_DEMO.md).
The current Qwen profile searches up to USD3/hour; this is a search ceiling, not
spending authority. The durable total allowance remains independently enforced.
Hourly policy caps include transfer, contingency and cleanup amortized over the
serving runtime, so they differ from the GPU's advertised price.

Qualification: desktop Rust tests, strict Clippy, UI build/lint/formatting and
30 mocked browser tests passed. m8s scripts/check passed. A full-check rerun hit
one timing-sensitive synthetic native-ID test; the subsequent complete run with
two test threads passed. Live recording and its removal evidence are recorded
separately; do not substitute these source checks for live acceptance.

On 2026-10-26, the desktop completed two actual conversational turns through
Qwen 27B on Vast instance **57838752**, then confirmed exact teardown and empty
provider instance/volume inventories. See the [live evidence](../deploy/container/desktop-qwen27-live-proof.json).
The attempt reserved USD0.990478 under a USD1.20 allowance and the user’s USD5
cap. Its full reservation and all earlier liabilities remain retained; credit
observations are not settled invoices. The local viewing copy is in the fork’s
ignored `recordings/qwen27-20261026/demo.mp4`, alongside the uncut WebM.
