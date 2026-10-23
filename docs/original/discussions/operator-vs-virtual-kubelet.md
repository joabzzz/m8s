# Discussion primer: kube-rs operator vs virtual-kubelet (vs no Kubernetes)

For: Admin's education thread · Decides: [ADR-0008](agor://kb/document/01a15b4d-b4f9-7254-adfb-37d41a4297e1) · Deeper research: [S4 §6](agor://kb/document/01a15b4e-43ee-7461-9bea-17f0edf8e5b3) · Status: open

## The one-paragraph version

Kubernetes is a loop: *you write down what you want, and controllers keep nudging reality toward it.* Our reality, rented GPU boxes on Vast, lives **outside** the cluster. There are two ways to teach Kubernetes about it:

- **Operator.** Invent new nouns: "a `GpuNode` is a thing we want". Write a controller that makes it real through the Vast API.
- **virtual-kubelet.** Pretend Vast is a computer in the cluster. Kubernetes then schedules ordinary pods "onto" it, and a shim turns each pod into a rented instance.

## Picture

```
 OPERATOR (A)                                   VIRTUAL-KUBELET (B)
 ────────────                                   ───────────────────
 you: kubectl apply GpuNode{tier: medium}       you: kubectl apply Pod{nodeSelector: vast}
          │                                              │ scheduler picks virtual Node "vast"
          ▼                                              ▼
 m8s-operator (our code) watches GpuNodes        virtual-kubelet + vast provider (our code)
   reconcile(): search → launch → bootstrap        CreatePod(): translate pod spec → launch
   writes .status{phase, $/h, zdr, heartbeat}      GetPodStatus(): map instance → pod phase
   finalizer: destroy before delete                DeletePod(): destroy
          │                                              │
          ▼                                              ▼
        Vast API                                       Vast API
 k9s: :gpunodes → our columns                   k9s: :pods → looks native, logs/exec expected
```

## Vocabulary (two minutes)

- **CRD** (CustomResourceDefinition): adds a new resource type such as `GpuNode`, with a schema. It's just data in etcd.
- **Controller / reconcile loop**: a function `reconcile(obj) -> Action` that runs whenever the object or the world changes. It must be idempotent.
- **Status vs spec**: `spec` is what you want and `status` is what is true. k9s shows both.
- **Finalizer**: a lock on deletion. Kubernetes won't delete the object until our code removes the finalizer, which it does after `destroy()` succeeds. This is our *teardown guarantee* while the cluster is alive.
- **Owner references**: deleting a `Tier` cascades to its `GpuNode`s.
- **Lease**: leader election, so two operator copies don't both launch instances.
- **additionalPrinterColumns**: tells kubectl and k9s which status fields to show as columns.

## Trade-offs

| | A: Operator + CRDs | B: virtual-kubelet | C: Rust daemon + SQLite |
|---|---|---|---|
| Fits "external rented box" | **Natural**: our nouns, our phases | Awkward: a box is not a pod | Natural |
| k9s experience | Custom views with our columns | Native pods, but `logs`/`exec`/restarts must be faked | None; we build TUI/web |
| Existing prior art | kube-rs controller runtime, many examples | VK providers exist for ACI, Nomad, Fargate; **none for Vast/RunPod** | n/a |
| Our code size | Medium | Large (must emulate pod semantics) | Smallest |
| Teardown guarantee | Finalizers + orphan sweep | DeletePod + sweep | Our own loop + sweep |
| Path into mesh-llm later | Core logic is a plain crate; operator is a thin wrapper | Hard | Easy |
| Mac mini RAM | minikube ~2–4 GB | same | ~0 |

## Recommendation (Metal-0, and S4 agrees)

Use **A**, with the logic in a Kubernetes-free `m8s-core` so that **C** stays available (and mesh-llm embedding stays cheap). Skip B: we would spend P1 emulating pods instead of shipping provisioning.

**Important caveat.** Neither A nor B protects you when the *Mac mini itself* dies, because minikube dies with it. That is why the dead man's switch lives in the container ([ADR-0007](agor://kb/document/01a15b4e-5bfc-72c9-a718-77a74f349922)).

## Questions for the thread

1. Is k9s visibility of *our* columns (A) enough, or do you specifically want pod-native views (B)?
2. Is the 2–4 GB of RAM for minikube on the Mac mini acceptable alongside the agent sessions?
3. Should C (no Kubernetes) be a supported mode for people who don't want minikube?
