"""Run the baked native mesh/model offline with a fresh required certificate.

Only a disposable, network-disabled container may run this. The mounted fixture
contains no owner signing key or provider credential. Native output is never
printed or saved; only fixed error classifications and HTTP proof are emitted.
"""
import json
import os
from pathlib import Path
import selectors
import signal
import subprocess
import time
import urllib.request

ROOT = Path('/tmp/m8s-native-model')
MODEL = 'qwen2.5-0.5b-instruct-q4_k_m'
SERVED_MODEL = 'local-gguf/sha256-74a4da8c9fdbcd15bd1f6d01d621410d31c6fc00986f5eb687824e7b93d7a9db'

def main():
    assert os.geteuid() == 0 and not ROOT.exists()
    ROOT.mkdir(mode=0o700)
    os.chown(ROOT, 65532, 65532)
    for name in ('node-key', 'node-ownership.json', 'owner-id'):
        path=ROOT/name
        path.write_bytes((Path('/fixture')/name).read_bytes())
        path.chmod(0o600)
        os.chown(path,65532,65532)
    config=ROOT/'runtime.toml'; config.write_text('');config.chmod(0o600);os.chown(config,65532,65532)
    def identity():
        os.setgroups([]);os.setgid(65532);os.setuid(65532)
    child=subprocess.Popen(['/opt/mesh/mesh-llm','serve','--gguf','/models/'+MODEL+'.gguf',
        '--node-ownership',str(ROOT/'node-ownership.json'),'--node-ownership-required',
        '--config',str(config),'--trust-policy','allowlist','--trust-owner',(ROOT/'owner-id').read_text(),
        '--headless','--log-format','json','--no-default-plugins','--port','9337','--console','3131'],
        env={'HOME':str(ROOT),'MESH_LLM_NODE_KEY_PATH':str(ROOT/'node-key'),
             'MESH_LLM_ANALYTICS':'0','DO_NOT_TRACK':'1'},
        stdin=subprocess.DEVNULL,stdout=subprocess.PIPE,stderr=subprocess.PIPE,
        start_new_session=True,preexec_fn=identity)
    selector=selectors.DefaultSelector()
    for stream in (child.stdout,child.stderr):
        os.set_blocking(stream.fileno(),False);selector.register(stream,selectors.EVENT_READ)
    categories=set();count=0
    patterns={
        b'unexpected argument':'unexpected-argument', b'error:':'cli-error',
        b'permission denied':'filesystem-permission-denied', b'mismatched':'identity-mismatch',
        b'certificate':'certificate-mentioned', b'failed to load':'load-failed',
        b'no compatible':'no-compatible-runtime', b'could not load':'load-unavailable',
        b'shared librar':'shared-library-mentioned',b'cuda':'cuda-mentioned',
        b'error':'error-mentioned',b'panicked':'rust-panic',b'not found':'not-found',
        b'no such file':'missing-file',b'gguf':'gguf-mentioned',b'backend':'backend-mentioned',
        b'cpu':'cpu-mentioned', b'unsupported':'unsupported',b'not supported':'not-supported'}
    def drain():
        nonlocal count
        for event,_ in selector.select(0):
            data=os.read(event.fileobj.fileno(),8192)
            if not data: selector.unregister(event.fileobj);continue
            count+=len(data)
            if count>2*1024*1024: raise RuntimeError('native-output-bound-exceeded')
            lower=data.lower()
            for pattern,category in patterns.items():
                if pattern in lower: categories.add(category)
    opener=urllib.request.build_opener(urllib.request.ProxyHandler({}))
    def request(path,body=None):
        data=None if body is None else json.dumps(body).encode()
        req=urllib.request.Request('http://127.0.0.1:9337'+path,data=data,headers={'Content-Type':'application/json'})
        with opener.open(req,timeout=2 if body is None else 60) as r:
            raw=r.read(65537);assert len(raw)<=65536
            return json.loads(raw)
    try:
        until=time.monotonic()+120
        while time.monotonic()<until:
            drain()
            if child.poll() is not None:
                drain();print(json.dumps({'native_exit':child.returncode,'diagnostic_categories':sorted(categories)}));return 1
            try:
                models=request('/v1/models')
                ids=[m['id'] for m in models.get('data',[])]
                if SERVED_MODEL in ids: break
            except Exception: pass
            time.sleep(.2)
        else:
            print(json.dumps({'model_timeout':True,'diagnostic_categories':sorted(categories)}));return 1
        result=request('/v1/chat/completions',{'model':SERVED_MODEL,'messages':[{'role':'user','content':'Say hello in a short sentence.'}],'max_tokens':24,'stream':False})
        assert result.get('model') == SERVED_MODEL
        assert result.get('object') == 'chat.completion'
        content=result['choices'][0]['message']['content']
        assert isinstance(content,str) and content.strip()
        print(json.dumps({'schema':'m8s.offline-native-model/v1','model':result['model'],'real_native_inference':True,'owner_routed':False,'network':'none','completion':content}))
        return 0
    finally:
        try: os.killpg(child.pid,signal.SIGTERM)
        except ProcessLookupError: pass
        try: child.wait(timeout=5)
        except subprocess.TimeoutExpired:
            os.killpg(child.pid,signal.SIGKILL);child.wait(timeout=5)

if __name__=='__main__':
    raise SystemExit(main())
