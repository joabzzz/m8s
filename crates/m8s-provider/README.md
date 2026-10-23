# Provider adapters

`m8s_provider::{Provider, LaunchSpec, Error, Result}` owns the synchronous provider
contract. `mock::Mock` retains deterministic faults and optional persistent state;
`vast` exposes the existing fixture mapper. Defaults enable no heavy dependencies.

| Feature | Capability |
| --- | --- |
| `local` | Synthetic development processes and recovery; [contract](src/local/README.md) |
| `vast-readonly-https` | Explicit read-only HTTPS transport; [contract](src/vast/README.md) |
| `vast-scoped-destroy-https` | Exact-resource cleanup and independent absence lookup |
| `vast-watchdog` | Separate cleanup process and `m8s-vast-watchdog` executable |
| `vast-launch-https` | Explicit bounded launch, access and provisioning transport |
| `agent-runtime` | Local `m8s-agent` lifecycle exerciser executable |
| `vast-node` | Same `m8s-agent --vast-node` entrypoint with authenticated runtime |

The provider crate assembles the node executable so the dependency direction stays
`m8s-provider -> m8s-agent -> m8s-core`. The agent library has no provider dependency;
core holds only provider-independent domain types and policy. Build the node with
`cargo build -p m8s-provider --features vast-node --bin m8s-agent`. The executable's
name and runtime arguments are unchanged. Source container recipes select this
package and feature explicitly.

`m8sd/local-development` enables `m8s-provider/local`; `m8sd/prototype-vast` enables
the launch and watchdog features. The daemon retains its existing typed integration;
this consolidation introduces no new provider selection or lifecycle behavior.
Existing uncertainty, ownership, accounting, credential and readiness contracts apply.
