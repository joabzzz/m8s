# Packaging only: no downloads, model, keys or automatically approving entrypoint.
# Supply an explicitly reviewed Linux base by immutable repository@sha256 digest.
# Context contains only selected Linux cli/ binaries and a native-owner-composed
# product/ tree (mesh-llm, product-manifest.json, native-runtimes/ incl licenses).
ARG RUNTIME_BASE
FROM ${RUNTIME_BASE}
COPY product/ /opt/mesh/
COPY --chmod=0555 cli/m8s /usr/local/bin/m8s
COPY --chmod=0555 cli/m8s-agent /usr/local/bin/m8s-agent
RUN --network=none test -x /opt/mesh/mesh-llm \
    && test -f /opt/mesh/product-manifest.json \
    && test -d /opt/mesh/native-runtimes
ENV MESH_LLM_NATIVE_RUNTIME_BUNDLE_DIR=/opt/mesh/native-runtimes \
    MESH_LLM_ANALYTICS=0 DO_NOT_TRACK=1
# Current agent refuses without --local. Do NOT replace this with --local or raw
# mesh as an implicit admission workaround. The library Docker adapter overrides
# only after the caller's qualified BootGate; no such production gate is wired.
USER 65532:65532
ENTRYPOINT ["/usr/local/bin/m8s-agent"]
