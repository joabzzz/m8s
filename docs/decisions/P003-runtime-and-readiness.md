# P003: Prepared runtime and owner-routed acceptance

Status: implemented for the opt-in prototype; demonstrated 2026-10-23.

## Context

Compiling a native inference runtime on rented machines wastes startup time and
makes a small acceptance demo dependent on network and build-tool availability.
A healthy process does not demonstrate inference through the owner mesh client.

## Decision

Publish an immutable Linux AMD64 image containing the actual native node, CPU
runtime, pinned Qwen2.5-0.5B GGUF, restricted provisioning service, and control
agent. Reuse native/model layers when changing only the Rust control agent.
Rentals pull the prepared image; they do not compile Rust or download model
weights separately. Pin the image by digest and record each source revision.

Acceptance requires verified certificate delivery, discovery of the exact served
GGUF content identity through the model-free owner client, and a real completion
through that client. The API model identity is content-addressed; the artifact
filename is a separate configuration label. Return success only after cleanup.

## Consequences

This establishes the CPU prototype path, not GPU performance, large-model sizing,
fleet scheduling, or production readiness. Native client and server revisions
are recorded separately. Source is private; the runtime image is public under
separate user authorization. Private source hosting does not privatize the image.
Existing Mac binaries are convenience artifacts, not portable or signed releases.

## Evidence and implementation

- [Published image provenance](../../deploy/container/listener-agent-evidence.json)
- [Offline actual-native inference](../../deploy/container/native-model-inference-evidence.json)
- [Live owner-routed inference](../../deploy/container/demo-proof.json)
- [Container build and qualification](../../deploy/container/README.md)
