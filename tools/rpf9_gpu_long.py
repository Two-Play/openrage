#!/usr/bin/env python3
"""Time-bounded expanded GPU dictionary pass; never opens game archives."""
import argparse, collections, hashlib, json, os, re, signal
import sqlite3, subprocess, sys, time
from pathlib import Path

# Share the portable game/export/build paths and validated CPU reference.
from rpf9_gpu_match import ROOT, OUT as EXPORT, TOOLS, BUILD, H, runner
STOP=False
def stop(*_):
    global STOP
    STOP=True
def lines(path):
    with path.open('rb') as f:
        while raw:=f.readline(65537):
            if len(raw)>65536:raise ValueError('source line exceeds64KiB')
            yield raw.decode('utf-8',errors='replace').strip()
def state(out,**data):
    tmp=out/'checkpoint.tmp'
    tmp.write_text(json.dumps(dict(pid=os.getpid(),**data),indent=2)+'\n')
    tmp.replace(out/'checkpoint.json')
def validate(out):
    samples=['a','ADDER','data/graphics/graphics_definitions.meta','data/camera/cam_core','a'*256,'a'*257,'A\\B','"adder"unused']
    r=subprocess.run(runner('all'),input=('\n'.join(samples)+'\n').encode(),capture_output=True,check=True,timeout=30)
    rows=r.stdout.splitlines()
    if len(rows)!=len(samples):raise ValueError('GPU validation row count')
    for sample,row in zip(samples,rows):
        a,b,_,_=row.decode().split('\t',3)
        if (int(a,16),int(b,16))!=(H.hash64(sample),H.hash32(sample)):raise ValueError('GPU hash mismatch')
    (out/'validation.json').write_text(json.dumps({'passed':True,'vectors':len(samples),'device_log':r.stderr.decode()},indent=2))
def main():
    global STOP
    p=argparse.ArgumentParser(description=__doc__)
    p.add_argument('--minutes',type=float,default=30)
    p.add_argument('--output',type=Path,required=True)
    p.add_argument('--background',action='store_true')
    args=p.parse_args()
    if not 0<args.minutes<=60:raise ValueError('duration must be <=60minutes')
    if args.background:
        if args.output.exists():raise ValueError('output already exists')
        with args.output.with_suffix('.launcher.log').open('xb') as log:
            child=subprocess.Popen([sys.executable,str(Path(__file__).resolve()),'--minutes',str(args.minutes),'--output',str(args.output.resolve())],stdin=subprocess.DEVNULL,stdout=log,stderr=log,start_new_session=True)
        print(json.dumps({'pid':child.pid,'output':str(args.output),'minutes':args.minutes}),flush=True)
        return
    out=args.output;out.mkdir(parents=True,exist_ok=False)
    signal.signal(signal.SIGTERM,stop);signal.signal(signal.SIGINT,stop)
    state(out,status='validating')
    validate(out)
    db=sqlite3.connect(out/'seeds.sqlite')
    db.execute('PRAGMA cache_size=-8192');db.execute('PRAGMA temp_store=FILE')
    db.execute('CREATE TABLE seeds(path TEXT PRIMARY KEY,priority INTEGER,rank INTEGER) WITHOUT ROWID')
    paths=[EXPORT/'dictionaries/gta6strings-ArchiveItems.txt',EXPORT/'ida_common_paths.txt',Path(os.environ.get('RPF9_PATH_CANDIDATES',str(EXPORT/'rpf9-path-candidates.txt'))),EXPORT/'dictionaries/gta5-installed.txt',EXPORT/'dictionaries/rdr2-codex.txt']
    dirs=collections.Counter();n=0
    for priority,path in enumerate(paths):
        if not path.exists():continue
        for line in lines(path):
            n+=1
            line=line.split('=',1)[0].strip().strip('"').lower().replace('\\','/')
            if not line or len(line)>512 or line.startswith(('//','#','[')) or not line.isascii() or any(c not in 'abcdefghijklmnopqrstuvwxyz0123456789_./:-@+ #' for c in line):continue
            line=re.sub(r'^[a-z0-9_]+:/','',line).rsplit('.rpf/',1)[-1].removeprefix('x64/')
            base=line.rsplit('/',1)[-1]
            stem=line.rsplit('.',1)[0] if '.' in base else line
            if '/' in stem and priority<3:
                directory=stem.rsplit('/',1)[0]+'/'
                if directory in dirs or len(dirs)<32768:dirs[directory]+=1
            for s in set((stem,stem.rsplit('/',1)[-1])):
                if not s or len(s)<3 or s.isdecimal():continue
                rank=int.from_bytes(hashlib.blake2s(s.encode(),digest_size=4).digest(),'little')
                db.execute('INSERT OR IGNORE INTO seeds VALUES(?,?,?)',(s,priority,rank))
            if n%10000==0:
                db.commit();state(out,status='preparing',source=str(path),input_lines=n)
                if STOP:state(out,status='stopped during preparation');return
        db.commit()
    db.execute('CREATE INDEX seed_order ON seeds(priority,rank)');db.commit()
    prefixes=list(dict.fromkeys(['data/','data/ai/','data/audio/','data/camera/','data/graphics/','data/levels/americas/']+[d for d,_ in dirs.most_common(128)]))
    extensions=['','.meta','.xml','.dat','.txt','.rpf','.ymt','.ytd','.ydr','.ydd','.yft','.ybn','.ycd','.ymap','.ytyp','.ysc','.awc','.rel','.gfx','.dat151.rel','.dat54.rel']
    ep=EXPORT/'cpu_extensions/extensions.txt'
    if ep.exists():extensions=list(dict.fromkeys(extensions+list(lines(ep))))
    common=['','.meta','.xml','.rpf','.dat','.ymt','.ytd','.ydr','.ydd','.yft','.ybn','.awc']
    suffixes=['_lod','_lod0','_lod1','_lod2','_slod','_slod2','_hi','_high','_low','_hd','_sd','_default','_interior','_exterior','_male','_female','_mp','_sp']+['_'+str(i) for i in range(32)]+['_v'+str(i) for i in range(1,17)]
    (out/'rules.json').write_text(json.dumps({'sources':[str(x) for x in paths],'prefixes':prefixes,'extensions':extensions,'suffixes':suffixes,'numeric_radius':32,'gpu_seconds':args.minutes*60,'max_gpu_buffers_mib':256,'note':'32-bit matches are hypotheses, not resolved names'},indent=2))
    start=time.monotonic();deadline=start+args.minutes*60;count=0;seeds=0;reason='candidate space exhausted';last=start
    with (out/'matches.raw.tsv').open('xb') as dest,(out/'gpu.log').open('xb') as log:
        gpu=subprocess.Popen(runner('matches'),stdin=subprocess.PIPE,stdout=dest,stderr=log)
        # Hard stop also covers a blocked pipe or a stalled Vulkan dispatch.
        import threading
        def watchdog():
            global STOP
            STOP=True
            if gpu.poll() is None:gpu.terminate()
        timer=threading.Timer(args.minutes*60+15,watchdog);timer.daemon=True;timer.start()
        try:
            for stem,priority,rank in db.execute('SELECT path,priority,rank FROM seeds ORDER BY priority,rank'):
                seeds+=1
                def candidates():
                    for ext in extensions:yield stem+ext
                    # Preserve directory and digit width; never enumerate arbitrary bytes.
                    match=re.search(r'\d{1,6}(?=\D*$)',stem)
                    if match:
                        value=int(match.group());width=len(match.group())
                        for k in range(max(0,value-32),value+33):
                            if k==value:continue
                            variant=stem[:match.start()]+str(k).zfill(width)+stem[match.end():]
                            for ext in common:yield variant+ext
                    for suffix in suffixes:
                        for ext in common:yield stem+suffix+ext
                    if '/' not in stem:
                        for prefix in prefixes:
                            for ext in common:yield prefix+stem+ext
                for candidate in candidates():
                    now=time.monotonic()
                    if STOP or now>=deadline:reason='time limit' if not STOP else 'stop requested';break
                    if dest.tell()>512*1024*1024:reason='512MiB output cap';STOP=True;break
                    gpu.stdin.write(candidate.encode()+b'\n');count+=1
                    if now-last>=10:
                        state(out,status='running',candidates_submitted=count,seeds_processed=seeds,elapsed_seconds=now-start,remaining_seconds=max(0,deadline-now),current_seed=stem,priority=priority,rank=rank,gpu_pid=gpu.pid)
                        print(f'{count} candidates, {now-start:.1f}s, seed={stem}',flush=True);last=now
                if STOP or time.monotonic()>=deadline:break
            gpu.stdin.close();code=gpu.wait(timeout=30)
            if code and not STOP:raise RuntimeError(f'GPU exited {code}')
        except BrokenPipeError:
            if not STOP:raise
        finally:
            timer.cancel()
            if gpu.poll() is None:gpu.terminate();gpu.wait(timeout=15)
    state(out,status='validating results',candidates_submitted=count,seeds_processed=seeds,elapsed_seconds=time.monotonic()-start,reason=reason)
    checked=0;hits64=set();hits32=set()
    with (out/'matches.cpu-verified.tsv').open('x') as verified:
        for line in lines(out/'matches.raw.tsv'):
            a,b,flags,path=line.split('\t',3)
            if (int(a,16),int(b,16))!=(H.hash64(path),H.hash32(path)):raise ValueError('CPU/GPU mismatch in results')
            checked+=1
            if int(flags,16)&1:hits64.add(a)
            if int(flags,16)&2:hits32.add(b)
            verified.write(line+'\n')
            if len(hits64)+len(hits32)>500000:raise ValueError('verification memory cap')
    state(out,status='complete',reason=reason,candidates_submitted=count,seeds_processed=seeds,elapsed_seconds=time.monotonic()-start,result_rows_cpu_verified=checked,distinct_hashes64=len(hits64),distinct_hashes32_unverified_names=len(hits32))
    db.close()
if __name__=='__main__':main()
