# ADR-0001 — Local workspace and repo, no remote

Status: **accepted** · Decided by: Admin (repo/remote), recorded by Metal-0 · Date: 2026-10-19

## Context

m8s had no registered code repo. Admin specified the working directory.

## Decision

- Working dir: `~/Development/m8s/`.
- `~/Development/m8s/m8s` is the m8s repo. It is a fresh Rust crate (`Cargo.toml`, `src/`), on branch `master`, with no commits and no remote.
- `~/Development/m8s/mesh-llm` is a checkout of `github.com/mesh-llm/mesh-llm` that sits alongside m8s. Its role (dependency or reference) will be set in the charter.
- **Nothing is pushed to any remote** for now. That covers both the m8s repo and the mesh-llm origin. Changing this needs Admin approval.
- `preset-io/agor-teammate` is never modified.

## Consequences

- Worker branches are local git worktrees only. Integration happens by local merge, reviewed by Metal-0.
- The m8s repo needs an initial commit before Agor can create worker branches from it. This is pending the charter session.
- Any change to mesh-llm stays on local branches unless Admin decides otherwise.
