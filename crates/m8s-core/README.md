# Provider-independent domain logic

`queue::Controller` is an **unwired scale-proposal kernel**, not an automated
invoker. Callers supply a validated immutable `Policy`, explicit Enabled/Paused
intent, actual desired count, injected Unix seconds and optional queue samples.
There are no default numeric profiles, clocks, metrics readers or effect callbacks.

Configured positive thresholds combine with OR and use strict `>` comparisons.
Pressure must persist across fresh samples for the explicit up duration; idle
requires **both** metrics zero for the explicit idle duration. Windows measure
sample timestamps. Freshness and maximum-gap equality are accepted; larger gaps,
unknown samples and invalid observations reset windows. Receipt times and accepted
sample timestamps must strictly increase; high-watermarks never roll back, even
after errors. Missing/stale/future samples are never evidence of idle. These checks
do not authenticate metrics or prove continuous values between samples.

Proposals contain an absolute target and reason: +1 under pressure (cold start to
max(1,min)), or -1 under idle above min, within max <=16. Zero is permitted input
without automatic minimum enforcement; max=0 disables starts. Paused produces no
proposal. Desired/intent changes and proposals reset windows; this is not global
exactly-once delivery. Replace the controller when replacing policy.

Enabled is input intent, **not permission**. A future coordinator must atomically
compose current desired, durable manual-stop, clock, budget, claim, rate and
authorization checks before any effect. This kernel neither changes desired nor
proves readiness, source authenticity, or execution eligibility.
