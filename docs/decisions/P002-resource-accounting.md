# P002: Durable intent and exact-resource cleanup

Status: implemented for the opt-in prototype; demonstrated 2026-10-23.

## Context

Provider effects can succeed despite a lost response. Killing a local process
or receiving a destroy acknowledgment does not prove removal or final billing.

## Decision

Persist launch intent and reserve bounded liability before provider creation.
A durable one-use invocation prevents repeating CREATE after uncertainty.
Run at most one rented prototype instance at a time. A separately surviving
watchdog is armed before creation and binds to the exact returned instance.
The serving deadline leaves an explicit cleanup interval within the reservation.

Cleanup scopes by exact owner and resource identity. Destroy acknowledgment is
followed by explicit absence observation. An independent creator-account query
confirms absence before the CLI releases the completion. Inventory checks include
volumes. Unsettled final billing retains liability; neither an empty inventory
nor a low current charge releases accounting holds. Prior claims are never reset.

## Consequences

An unsuccessful attempt can consume conservative budget without comparable
actual spend. Replacement attempts require available cumulative authorization,
prior-resource absence, and new durable records. A detached local watchdog does
not by itself guarantee recovery after total owner-machine loss; node DMS and
provider cleanup failures require further fault qualification and operations.

## Evidence and implementation

- [Cleanup receipt and watchdog events](../../deploy/container/demo-proof.json)
- [Independent inventories and retained reservations](../../deploy/container/final-account-evidence.json)
- [Daemon persistence contract](../../crates/m8sd/README.md)
- [Vast provider contract](../../crates/m8s-provider/src/vast/README.md)
