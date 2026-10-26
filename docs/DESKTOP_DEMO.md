# Desktop Qwen 27B recording

Preparation status: source inspection only; no desktop integration or live
recording has been qualified. Desktop upstream is read-only for this work.
Whether a separate local desktop prototype may be edited is awaiting clarification.

## Inspected source

Desktop repository: https://github.com/Mesh-LLM/desktop-app, revision
`275c1622d9d8819517b9283b5046ba8df795f9db` (inspected 2026-10-26).
Its `Cargo.lock` pins mesh-llm to
`b4b33ef8cdf97017db5fb64ff6cebc3c9c6fa3b7`. Compatibility with the
[proven Qwen runtime](QWEN38_DEMO.md) is not established.

| Surface | Existing behavior | Integration needed |
| --- | --- | --- |
| `ui/src/screens/Main.tsx` | Chat alongside a live mesh sidebar; local Mac and discovered peers | Entry point for Add compute and rental lifecycle |
| `ui/src/components/MeshPanel.tsx` | Host/participant counts, peer models, device search, invites | Show rental progress separately from verified mesh readiness |
| `ui/src/components/Chat.tsx` | Sends model `auto` | Verify actual Qwen response identity; do not infer attribution from a visible peer |
| `src-tauri/src/server.rs` | Local HTTP lifecycle API and SSE; no Tauri IPC | Narrow backend adapter to m8sd, with explicit bounded launch inputs |
| `src-tauri/src/node.rs` | Embedded `MeshNode`; host or join using an invite | Owner-mediated authorization for adding a certified node to this network |
| `src-tauri/src/agent.rs` | Goose chat through the local node's OpenAI API | Validate ordinary conversation and follow-up with the pinned 4096 context profile |

The app currently exposes neither an m8s control endpoint nor an owner-signing
endpoint. The SDK's owner-key builder option is not a signing service: it does
not justify giving m8s the desktop owner's private key.

## Intended recording

1. Show the desktop's local participant, with no local model serving, in a
   dedicated private demo mesh.
2. Choose Add compute, Qwen 27B, and a finite total cap and duration. Show the
   selected network before dispatch. Credentials stay in the backend.
3. Show provisioning, model loading, then verified availability. Only certificate
   verification, exact model discovery and an owner-routed canary permit Ready.
4. Keep the mesh sidebar visible while asking: “Explain a mesh network in two
   sentences.” Follow with: “How does adding this GPU help my laptop?” Preserve
   conversation context. Record response attribution to the exact Qwen identity.
5. Choose Remove compute. Show cleanup pending until the provider confirms exact
   absence. Finish with the local participant remaining and the rental removed.

Capture only the dedicated app window. Keep invite dialogs, credential selection,
private run directories and raw native/provider output outside the recording.
Keep an uncut capture; label any shortened download/loading interval in the edit.

## Implementation sequence

1. Resolve whether a separate local desktop prototype is allowed. Upstream
   pushes, PRs and publication remain outside scope.
2. Implement and test the owner boundary before a rental: bind the request to the
   active network and exact node, enforce expiry and single-use admission, and
   keep signing within the mesh owner. Preserve m8s's independent cleanup.
3. Wire the desktop backend to the one-attempt `m8sd --vast-demo` lifecycle;
   expose typed progress and removal state to the UI. UI closure or cancellation
   must not extend the deadline or disable the surviving watchdog.
4. Test with synthetic fixtures: duplicate launch, wrong network, failed canary,
   disconnect, cancellation, restart, uncertain CREATE and pending removal.
   Mock displays must be explicitly identified as rehearsal.
5. Qualify actual desktop/native protocol and chat compatibility locally before
   spending. Desktop `auto` routing and Goose prompts/tool schemas are additional
   behavior beyond the proven single fixed canary.
6. Obtain the current total USD cap and explicitly selected credential file,
   prepare a fresh bounded plan under the existing runbook, then record once.
   Retain evidence and verify provider inventories after teardown.

## Unmodified-desktop alternative

If desktop source must remain unchanged, keep provisioning in m8s and use the
desktop's existing Join/Chat flow for the visual part. This would demonstrate
desktop inference on an m8s-created network, rather than a shipped Add compute
feature. It remains an integration candidate, not a working fallback: desktop
admission to the certificate-required network and runtime compatibility must
first be established without weakening trust checks.

The current m8s coordinator creates its own ephemeral owner and model-free client,
sends one fixed canary, then holds until down or the original deadline. It does
not currently expose a desktop attachment contract. A raw invite alone must not
be treated as authorization to bypass the required certificate policy.

## Budget and acceptance

m8s already enforces bounded admission. The current demo implementation limits
each attempt to at most USD6 and runtime plus cleanup to 1800 seconds, including
at least 240 seconds for cleanup. These ceilings are not spending authorization.
Account for previous spending and retained liabilities; use fresh qualification
and at most one rental. Follow [the demo runbook](QWEN38_DEMO.md).

Completion requires a real desktop conversation, exact Qwen attribution,
provider-confirmed destruction and a reviewable video. Screenshots, synthetic
rehearsals and the historical canary alone do not meet that acceptance.
