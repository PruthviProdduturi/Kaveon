import base64, importlib.util, json, subprocess, tempfile, shutil, time
from pathlib import Path
spec=importlib.util.spec_from_file_location('q','scripts/qualify-aks-fault-pressure.py'); q=importlib.util.module_from_spec(spec); spec.loader.exec_module(q)
context='kaveon-aks'; namespace='kaveon'; kube=q.Kube(context,namespace); policy='kaveon-storage-fault-injection'
manifest='''apiVersion: networking.k8s.io/v1
kind: NetworkPolicy
metadata:
  name: kaveon-storage-fault-injection
  namespace: kaveon
spec:
  podSelector:
    matchLabels:
      statefulset.kubernetes.io/pod-name: kaveon-worker-0
  policyTypes: [Egress]
  egress:
  - to:
    - ipBlock: {cidr: 10.0.0.0/8}
    - ipBlock: {cidr: 10.244.0.0/16}
  - to:
    - namespaceSelector:
        matchLabels: {kubernetes.io/metadata.name: kube-system}
      podSelector: {matchLabels: {k8s-app: kube-dns}}
    ports:
    - {protocol: UDP, port: 53}
    - {protocol: TCP, port: 53}
'''
report={'query':'SELECT COUNT(*) FROM kaveon_events_enriched WHERE actions >= 0','worker':'kaveon-worker-0','policy':policy}
proc=None; logh=None; work=None; applied=False
try:
 sec=q.engine_secret_data(kube); tls=q.secret_data(kube,q.TLS_SECRET); token,principal=q.choose_principal(base64.b64decode(sec['security.json'])); report['principal']=principal
 work=Path(tempfile.mkdtemp(prefix='kaveon-storage-fault-')); ca=work/'ca.crt'; ca.write_bytes(base64.b64decode(tls['ca.crt'])); port=q.free_port(); logh=(work/'pf.log').open('w')
 proc=subprocess.Popen([*kube.prefix,'port-forward','service/kaveon-coordinator',f'{port}:8080','--address','127.0.0.1'],stdout=logh,stderr=subprocess.STDOUT,creationflags=getattr(subprocess,'CREATE_NO_WINDOW',0)); q.wait_for_port(port,proc); engine=q.Engine(f'https://127.0.0.1:{port}',token,str(ca)); case={'name':'storage_fault_scan','catalog':'OpenSource','schema':'public','sql':report['query']}
 baseline=engine.submit(case,timeout=180); report['baseline']={k:baseline.get(k) for k in ('data','recovery','stage_count','task_count','worker_ids','elapsed_ms','result_sha256')}
 subprocess.run(['kubectl','--context',context,'-n',namespace,'apply','-f','-'],input=manifest,text=True,check=True,capture_output=True); applied=True; report['policy_applied']=True
 started=time.time()
 try:
  result=engine.submit(case,timeout=180); report['fault_result']={k:result.get(k) for k in ('data','recovery','stage_count','task_count','worker_ids','elapsed_ms','result_sha256')}; report['exact']=result.get('data')==baseline.get('data')
 except Exception as e:
  report['fault_error']=str(e)
 report['fault_elapsed_s']=round(time.time()-started,2)
finally:
 if applied:
  subprocess.run(['kubectl','--context',context,'-n',namespace,'delete','networkpolicy',policy,'--ignore-not-found=true'],check=False,capture_output=True); report['policy_removed']=True
 if proc is not None and proc.poll() is None:
  proc.terminate()
  try: proc.wait(timeout=10)
  except subprocess.TimeoutExpired: proc.kill(); proc.wait(timeout=10)
 if logh is not None: logh.close()
 if work is not None:
  try: shutil.rmtree(work)
  except OSError: pass
print(json.dumps(report,indent=2))
