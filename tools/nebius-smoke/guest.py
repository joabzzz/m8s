"""Public cloud-init payload: no credentials, SSH, model server, or Ready claim."""
import json
import os
import time
import urllib.request

MODEL = "HuggingFaceTB/SmolLM2-135M-Instruct"
REVISION = "12fd25f77366fa6b3b4b768ec3050bf629380bac"


def main():
    import torch
    from transformers import AutoModelForCausalLM, AutoTokenizer

    assert torch.cuda.is_available(), "CUDA unavailable"
    assert torch.cuda.device_count() == 1, "expected exactly one GPU"
    gpu = torch.cuda.get_device_name(0)
    assert "B300" in gpu, "unexpected GPU"
    # Exercise a real GPU kernel before loading a model.
    x = torch.arange(1024, dtype=torch.float32, device="cuda")
    assert (x * 2).sum().item() == 1047552
    tokenizer = AutoTokenizer.from_pretrained(MODEL, revision=REVISION, trust_remote_code=False)
    model = AutoModelForCausalLM.from_pretrained(
        MODEL, revision=REVISION, trust_remote_code=False,
        use_safetensors=True, torch_dtype=torch.float32, attn_implementation="eager",
    ).to("cuda").eval()
    assert next(model.parameters()).is_cuda
    prompt = tokenizer.apply_chat_template(
        [{"role": "user", "content": "Say hello in one short sentence."}],
        tokenize=False, add_generation_prompt=True,
    )
    inputs = tokenizer(prompt, return_tensors="pt").to("cuda")
    started = time.monotonic()
    with torch.inference_mode():
        result = model.generate(**inputs, max_new_tokens=32, do_sample=False)
    torch.cuda.synchronize()
    generated = result[0, inputs.input_ids.shape[1]:]
    text = tokenizer.decode(generated, skip_special_tokens=True).strip()
    assert text and len(generated) > 0
    request = urllib.request.Request(
        "http://metadata.nebius.internal/v1/instance-data/id", headers={"Metadata": "true"},
    )
    opener = urllib.request.build_opener(urllib.request.ProxyHandler({}))
    with opener.open(request, timeout=5) as response:
        instance = response.read(256).decode().strip()
    return {
        "schema": "m8s.nebius-gpu-smoke/v1", "owner": os.environ["M8S_SMOKE_OWNER"],
        "instance_id": instance, "success": True, "gpu": gpu,
        "compute_capability": list(torch.cuda.get_device_capability(0)),
        "torch": torch.__version__, "cuda": torch.version.cuda,
        "model": MODEL, "revision": REVISION, "completion": text[:1024],
        "generated_tokens": len(generated), "inference_seconds": round(time.monotonic() - started, 3),
        "authenticated_m8s_routing": False, "ready": False,
    }


if __name__ == "__main__":
    try:
        evidence = main()
    except Exception as error:
        evidence = {"schema": "m8s.nebius-gpu-smoke/v1", "owner": os.environ["M8S_SMOKE_OWNER"],
                    "success": False, "error_type": type(error).__name__, "ready": False}
    print("M8S_NEBIUS_GPU_SMOKE_V1 " + json.dumps(evidence, separators=(",", ":")), flush=True)
