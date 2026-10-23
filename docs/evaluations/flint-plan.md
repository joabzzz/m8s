# Recursive Flint evaluation plan

Status: source-tested preparation checkpoint; no published Flint image or live
Flint measurements yet. This is a plan, not a completed performance analysis.

This experiment evaluates the refactored m8s demo with Recursive Flint through
its certificate-enforcing, model-free owner client. It does not use a separate
llama-server endpoint. The code/ADR cleanup is tracked separately in
[PR #3](https://github.com/joabzzz/m8s/pull/3).

Native runtime changes are a separate dependency, tracked in
[m8s-mesh-llm draft PR #1](https://github.com/joabzzz/m8s-mesh-llm/pull/1).
That PR is not the m8s evaluation deliverable and is not part of the completed
code/ADR refactor. This report must distinguish results using that experimental
runtime from results on the previously qualified runtime.

## Model and runtime

The selected artifact is
[RecursiveSI/flint-nvfp4-gguf](https://huggingface.co/RecursiveSI/flint-nvfp4-gguf/tree/6ec43e19b565fc8d560e2d04985d7724f20673bf),
revision `6ec43e19b565fc8d560e2d04985d7724f20673bf`. The GGUF is
22,863,084,160 bytes (22.86 GB, 21.29 GiB), below the requested 64 GB limit.
Its verified SHA-256 is
`97cc8009ff9b132d5cf3c488e6767962bb5a5dc158a90ed661bbbddf876b8bde`.
The file contains packed NVFP4 weights, tokenizer and chat template; the pinned
Apache-2.0 license is available in the pinned upstream repository. Weights are
not baked into the runtime image: the admitted node selects this immutable
Hugging Face reference and MeshLLM downloads it through its native cache flow.

The prototype's old llama.cpp pin did not contain Flint. The experiment moves
its Skippy dependency to upstream `a37cc46104eb50900d5ab4a9ba02a0abdac039db`
and rebases the owned patch queue. The fully patched revision is
`6085189ee2b42cad0a296bebdab486cae3a58dbe`. Whole-source graph annotation
regeneration transformed 143 builders with zero unsupported shapes or errors;
the second pass found all 143 already transformed and made no further edits.
All 281 CPU native tests passed on the final patched source. The graph-contract
probe also passed against the actual pinned Flint GGUF, checking layer-cut plans
and state ownership without weight inference. CUDA native compilation and
runtime packaging passed; full image assembly has not been qualified.

The broader native Rust test group is not fully green: the split-certification
roster still names the old native recipe, and one shell-based test requires Bash
`mapfile`, unavailable in the system macOS Bash. Six process-scope/timing failures
passed when isolated serially. The certification roster has not been relabeled
to imply evidence that this experiment does not provide.

The CUDA image targets Blackwell SM120a with CUDA 12.8. Its native build keeps
CUDA graph capture disabled, as required by the existing runtime build policy.
GPU placement otherwise uses the native product defaults. This differs from
Recursive's optimized inference engine; the vendor's published speed figures
are context, not a baseline measured by this experiment.

## Measurement method

The owner performs one arithmetic request immediately after model discovery,
then three repetitions of each of these requests at concurrency one:

| Case | Purpose | Check |
| --- | --- | --- |
| Arithmetic | Short answer: 17 × 23 | Exact text `391` |
| JSON | Return a specified object | Parsed JSON equality |
| Code | Implement interval merging with explanation/examples | Output retained; not executed or scored |

Each request allows 512 output tokens and uses temperature 1.0, top-p 0.95,
top-k 20 and min-p 0.0. These sampling values follow the
[pinned model instructions](https://huggingface.co/RecursiveSI/flint-nvfp4-gguf/blob/6ec43e19b565fc8d560e2d04985d7724f20673bf/README.md).

Times use the owner process's monotonic clock, starting before its loopback API
request. They therefore include owner routing and remote request latency.

- First SSE: first parsed event, which can contain only an assistant role.
- TTFT text: first nonempty content or reasoning delta observed by the owner.
- First answer text: first nonempty answer-content delta.
- End-to-end tok/s: server-reported completion tokens divided by total request time.
- Decode-window estimate: those tokens divided by the interval from first to last
  nonempty output delta. Events can batch tokens, so this is not an exact device
  decode benchmark. A zero-length interval produces no estimate.

Missing server token usage produces null token rates. Event counts are never
used as token counts. Each completed sample is retained before another request
starts. The first request is reported separately from warm samples; discovery
has already happened, so it is not a measurement of container or model startup.
Repeated prompts can benefit from prompt caching. Three repetitions support a
small smoke-test summary, not a statistically robust quality evaluation.

## Resource accounting and acceptance

The operator authorized the selected credential file and spending from existing
Vast credits. No top-up is authorized. The experiment uses one rental at a time,
a fresh credit/empty-inventory check, durable intent before creation, and an
independent cleanup watchdog. Historical reservations remain in the ledger.
A completed request alone does not constitute final demo acceptance: the exact
owned instance must be independently confirmed absent by the provider.

The preflight receipt retains provider-reported hardware and observed prices.
These are not independently measured GPU specifications or a settled invoice.
Storage reservation deliberately uses the larger hourly interpretation of the
provider's ambiguous storage rate; reserved liability is not actual spend.

## Live results

Pending the qualified image and bounded live run. No throughput or latency
numbers have been inferred from build success, vendor claims, or the previous
Qwen demo.

## Checkpoint validation and remaining acceptance

The m8s repository checks passed with 782 tests, zero failures and five ignored
tests using the documented `--no-pty --no-acl-mutation` profile. That profile
filters seven environment-specific tests; it is not an unfiltered full run.
Strict Clippy and all 19 Python container tests passed. Focused tests cover
native `--model` launch without a local weight file, exact pinned Flint
provisioning, preserved admission flags, and fast-network GPU qualification.

Before a paid run:

1. Complete the runtime-only image build using the separately reviewed native
   source and record the immutable image digest. Confirm that no weights are
   present and that the prepared runtime loads its CUDA libraries.
2. Refresh the selected fast-network offer, account credit and empty inventory;
   prepare a fresh bounded plan retaining historical liabilities.
3. Demonstrate native model download, certificate-enforced admission and
   owner-routed inference, then confirm exact-resource destruction.
4. Publish measured samples, hardware/runtime provenance and accounting in the
   subsequent evaluation analysis. Build success alone is not demo acceptance.
