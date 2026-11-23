#!/usr/bin/env python3
"""Finite Vulkan RPF9 dictionary pass. No archive payload reads. Resume by --skip.
GPU calculates both hashes and searches sorted target arrays. 32-bit hits are
hypotheses only. Outputs preserve ambiguity, and never auto-rename game files.
"""
import argparse, array, bisect, hashlib, json, os, random, re, subprocess, time
from pathlib import Path
import rpf9_gpu_hash_algorithms as H
TOOLS=Path(__file__).resolve().parent
if not __debug__:
    raise SystemExit('Run without -O: validation assertions must remain enabled.')
if not os.environ.get('RPF9_GAME_DIR'):
    raise SystemExit('Set RPF9_GAME_DIR to the directory containing rpf9-hash-export.')
ROOT=Path(os.environ['RPF9_GAME_DIR']).expanduser().resolve()
OUT=ROOT/'rpf9-hash-export'
BUILD=Path(os.environ.get('RPF9_GPU_BUILD_DIR',str(OUT/'gpu-build'))).expanduser().resolve()
def runner(mode):
    return [str(BUILD/'rpf9-gpu-runner'),str(BUILD/'rpf9-gpu.spv'),str(OUT/'name_hashes_unique.txt'),str(OUT/'name_hashes32_unique.txt'),mode]
def validate():
    rng=random.Random(0x52504639)
    vectors=[b'a'*n for n in list(range(34))+[63,64,65,127,128,129,255,256,257,511,512,513,1023]]
    vectors += [b'DATA\\graphics/graphics_definitions.meta',b'data/camera/cam_core',b'ADDER',b'"adder"unused',b'ABC\0ignored',b'\\A/B',b'\xff\xc0A']
    vectors += [bytes(rng.choice(b'ABCDxyz/\\_0123456789. -"') for _ in range(rng.randrange(900))) for _ in range(1000)]
    targets=[]
    for name,kind in [('name_hashes_unique.txt','Q'),('name_hashes32_unique.txt','I')]:
        with (OUT/name).open() as f: values=array.array(kind,(int(line.strip(),16) for line in f if line.strip()))
        assert all(values[i-1]<=values[i] for i in range(1,len(values))), name
        targets.append(values)
    result=subprocess.run(runner('all'),input=b'\n'.join(vectors)+b'\n',stdout=subprocess.PIPE,stderr=subprocess.PIPE,check=True,timeout=120)
    rows=result.stdout.split(b'\n')[:-1]
    assert len(rows)==len(vectors),(len(rows),len(vectors))
    for i,(v,row) in enumerate(zip(vectors,rows)):
        a,b,flags,path=row.split(b'\t',3)
        assert (int(a,16),int(b,16))==(H.hash64(v),H.hash32(v)),(i,v,a,b)
        expected=0
        for values,value,flag in zip(targets,(int(a,16),int(b,16)),(1,2)):
            pos=bisect.bisect_left(values,value)
            if pos<len(values) and values[pos]==value: expected|=flag
        assert int(flags,16)==expected,(i,flags,expected)
        assert path==v
    evidence={'vectors':len(vectors),'seed':'0x52504639','passed':True,'stderr':result.stderr.decode(),'shader_sha256':sha(TOOLS/'rpf9_gpu.comp'),'cpp_sha256':sha(TOOLS/'rpf9_gpu.cpp')}
    (OUT/'gpu-validation.json').write_text(json.dumps(evidence,indent=2)+'\n')
    print(json.dumps(evidence),flush=True)
def sha(path):
    h=hashlib.sha256()
    with path.open('rb') as f:
        for chunk in iter(lambda:f.read(65536),b''):h.update(chunk)
    return h.hexdigest()
def lines(path):
    if not path.exists():return
    with path.open('r',errors='replace') as f:
        for line in f:
            # All inputs are line dictionaries; limit individual accepted candidates.
            v=line.strip().replace('\\','/').lower()
            if v and len(v.encode())<1024 and '\t' not in v and '\0' not in v:yield v
EXT=['','.meta','.xml','.dat','.ymt','.ytd','.ydr','.ydd','.yft','.ybn','.ycd','.ytyp','.ymap','.awc','.rel','.ysc','.rpf','.txt','.ypt','.yld','.yvr','.yfd','.yld','.yhn','.yad','.cut','.gfx','.zt','.it','.ztd','.itd','.zdr','.idr','#mt','#td','#dr','#bn']
EXT=list(dict.fromkeys(EXT + ['.'+prefix+ext[2:] for ext in EXT if ext.startswith('.y') for prefix in ('z','i','#')]))
PREFIX=['','data/','data/ai/','data/control/','data/audio/','data/camera/','data/levels/americas/','data/levels/americas/water/','data/timecycle/','data/graphics/']
def variants(v,rich=False):
    yield v
    unmount=re.sub(r'^[a-z0-9_]+:/','',v)
    forms=[unmount]
    if '.rpf/' in unmount:forms.append(unmount.rsplit('.rpf/',1)[1])
    if unmount.startswith('common/'):forms.append(unmount[7:])
    forms += [unmount.rsplit('/',1)[-1]]
    emitted=set()
    for f in forms:
        stem=f.rsplit('.',1)[0] if '.' in f.rsplit('/',1)[-1] else f
        for x in (f,stem):
            if x and x not in emitted:emitted.add(x);yield x
        if rich:
            for ext in EXT:
                x=stem+ext
                if x not in emitted:emitted.add(x);yield x
            # Directory reconstruction for bare metadata-like identifiers, never
            # cartesian product of all directories, words and extensions.
            if '/' not in stem and not stem.isdecimal():
                for prefix in PREFIX[1:]:
                    for ext in ('','.meta','.xml','.dat'):
                        x=prefix+stem+ext
                        if x not in emitted:emitted.add(x);yield x
            # Neighbour indices preserve width and observed suffix/extension.
            m=re.search(r'(\d{1,3})(?=[^0-9]*$)',f)
            if m:
                n=int(m.group());width=len(m.group())
                for k in range(max(0,n-2),min(999,n+2)+1):
                    if k!=n:yield f[:m.start()]+str(k).zfill(width)+f[m.end():]
def sources():
    return [OUT/'dictionaries/gta6strings-ArchiveItems.txt',Path(os.environ.get('RPF9_PATH_CANDIDATES',str(OUT/'rpf9-path-candidates.txt'))),OUT/'ida_common_paths.txt',OUT/'dictionaries/gta5-installed.txt',OUT/'dictionaries/rdr2-codex.txt',OUT/'dictionaries/gtav-objects.txt',OUT/'dictionaries/gtav-peds.txt',OUT/'dictionaries/gtav-vehicles.txt']
def candidates():
    for source in sources():
        rich=source.name.startswith('gta6strings-') or source.name in ('ida_common_paths.txt','rpf9-path-candidates.txt')
        for v in lines(source):yield from variants(v,rich)
def run(args):
    validate() # Never launch matching if any CPU/GPU vector differs.
    provenance={'sources':[{'path':str(p),'bytes':p.stat().st_size,'sha256':sha(p)} for p in sources() if p.exists()], 'github':'https://github.com/ahmed605/GTA6-StringsDatabase','extensions':EXT,'prefixes':PREFIX,'max_candidates':args.limit,'skip':args.skip,'max_gpu_buffers_MiB':256,'batch':4096,'yield_ms':2,'note':'32-bit hits are ambiguous hypotheses; 64-bit hits need context for extension semantics.'}
    (OUT/'gpu-sources.json').write_text(json.dumps(provenance,indent=2)+'\n')
    start=time.monotonic();count=0
    with (OUT/'gpu-matches.tsv').open('ab' if args.skip else 'wb') as dest,(OUT/'gpu-run.log').open('ab' if args.skip else 'wb') as log:
        p=subprocess.Popen(runner('matches'),stdin=subprocess.PIPE,stdout=dest,stderr=log)
        try:
            for index,candidate in enumerate(candidates()):
                if index<args.skip:continue
                if count>=args.limit:break
                raw=candidate.encode('utf-8')
                if len(raw)>1023:continue
                p.stdin.write(raw+b'\n');count+=1
                if count%100000==0:
                    checkpoint={'generated':count,'resume_skip':index+1,'elapsed_seconds':time.monotonic()-start,'status':'running; resume may replay last GPU batch'}
                    (OUT/'gpu-checkpoint.json').write_text(json.dumps(checkpoint)+'\n')
            p.stdin.close();code=p.wait(timeout=120)
            if code:raise RuntimeError(f'GPU runner failed {code}; see gpu-run.log')
        except BaseException:
            p.terminate();p.wait(timeout=30);raise
    report={'candidates':count,'skip':args.skip,'elapsed_seconds':time.monotonic()-start,'completed':True,'limit':args.limit}
    (OUT/'gpu-checkpoint.json').write_text(json.dumps(report,indent=2)+'\n');print(json.dumps(report),flush=True)
if __name__=='__main__':
    p=argparse.ArgumentParser(description=__doc__);p.add_argument('--validate-only',action='store_true');p.add_argument('--limit',type=int,default=15000000);p.add_argument('--skip',type=int,default=0);a=p.parse_args()
    if a.validate_only:validate()
    else:run(a)
