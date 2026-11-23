#!/usr/bin/env python3
"""Disk-backed, bounded-memory verification and deduplication of GPU results."""
import csv,json,sqlite3,time
from rpf9_gpu_match import OUT,H,sha,lines,sources
start=time.monotonic()
db=sqlite3.connect(OUT/'gpu-analysis.sqlite');db.execute('pragma cache_size=-8192');db.execute('pragma temp_store=FILE')
for table in ('matches','baseline','coverage'):db.execute(f'DROP TABLE IF EXISTS {table}')
db.execute('CREATE TABLE matches(h64 TEXT,h32 TEXT,flags INTEGER,path TEXT,PRIMARY KEY(h64,h32,path)) WITHOUT ROWID')
rows=0
with (OUT/'gpu-matches.tsv').open() as f:
 for line in f:
  a,b,flags,path=line.rstrip('\n').split('\t',3);flags=int(flags,16)
  # Recompute every emitted candidate, not just a sample. No GPU-only trust.
  assert H.hash64(path)==int(a,16),(a,path)
  assert H.hash32(path)==int(b,16),(b,path)
  db.execute('INSERT OR IGNORE INTO matches VALUES(?,?,?,?)',(a,b,flags,path));rows+=1
  if rows%50000==0:db.commit();print('verified',rows,flush=True)
db.commit();db.execute('CREATE INDEX matches64 ON matches(h64) WHERE flags&1');db.execute('CREATE INDEX matches32 ON matches(h32) WHERE flags&2')
db.execute('CREATE TABLE baseline(width INTEGER,hash TEXT,PRIMARY KEY(width,hash)) WITHOUT ROWID')
prior=['crossgame_matches.tsv','dictionary_matches.tsv','ida_suffix_matches.tsv','numeric_variant_matches.tsv']
for name in prior:
 p=OUT/name
 if not p.exists():continue
 with p.open() as f:
  for r in csv.DictReader(f,delimiter='\t'):
   if 'width' in r:db.execute('INSERT OR IGNORE INTO baseline VALUES(?,?)',(int(r['width']),r['hash'].lower()))
   elif r.get('name_hash_64'):db.execute('INSERT OR IGNORE INTO baseline VALUES(64,?)',(r['name_hash_64'].lower(),))
db.commit()
def num(q):return db.execute(q).fetchone()[0]
report={'gpu_output_rows':rows,'unique_candidate_rows':num('SELECT count(*) FROM matches'),'unique_64bit_hashes':num('SELECT count(DISTINCT h64) FROM matches WHERE flags&1'),'unique_32bit_hash_hypotheses':num('SELECT count(DISTINCT h32) FROM matches WHERE flags&2'),'new_64bit_hashes_vs_saved_baseline':num('SELECT count(DISTINCT h64) FROM matches m WHERE flags&1 AND NOT EXISTS(SELECT 1 FROM baseline b WHERE b.width=64 AND b.hash=m.h64)'),'cpu_recomputed_all_output_rows':True,'baseline_files':prior}
with (OUT/'gpu-unique-matches.tsv').open('w') as f:
 w=csv.writer(f,delimiter='\t',lineterminator='\n');w.writerow(['name_hash_64','jenkins32','flags_1_is_64bit_2_is_32bit','candidate'])
 for row in db.execute('SELECT h64,h32,flags,path FROM matches ORDER BY h64,path'):w.writerow(row)
with (OUT/'gpu-new64-matches.tsv').open('w') as f:
 w=csv.writer(f,delimiter='\t',lineterminator='\n');w.writerow(['name_hash_64','candidate'])
 for row in db.execute('SELECT h64,path FROM matches m WHERE flags&1 AND NOT EXISTS(SELECT 1 FROM baseline b WHERE b.width=64 AND b.hash=m.h64) ORDER BY h64,path'):w.writerow(row)
# entry traversal reads TSV one line at a time; indexed probes stay on disk.
covered=both=total=0
with (OUT/'entries.tsv').open() as f:
 for r in csv.DictReader(f,delimiter='\t'):
  total+=1;a=r['name_hash_64'];b=r['name_hash_32']
  if db.execute('SELECT 1 FROM matches WHERE h64=? AND flags&1 LIMIT 1',(a,)).fetchone():
   covered+=1
   if b and db.execute('SELECT 1 FROM matches WHERE h64=? AND h32=? AND flags&1 LIMIT 1',(a,b)).fetchone():both+=1
db.execute('CREATE INDEX IF NOT EXISTS matchespath ON matches(path)')
db.execute('DROP TABLE IF EXISTS direct_sources')
db.execute('CREATE TABLE direct_sources(h64 TEXT PRIMARY KEY) WITHOUT ROWID')
for source in sources():
 for v in lines(source):
  db.execute('INSERT OR IGNORE INTO direct_sources SELECT h64 FROM matches WHERE path=? AND flags&1',(v,))
db.commit()
report['hashes_matched_by_unmodified_normalized_dictionary_names']=num('SELECT count(*) FROM direct_sources')
report['hashes_only_matched_after_rules']=num('SELECT count(DISTINCT h64) FROM matches m WHERE flags&1 AND NOT EXISTS(SELECT 1 FROM direct_sources d WHERE d.h64=m.h64)')
db.execute('DROP TABLE IF EXISTS gta6_source')
db.execute('CREATE TABLE gta6_source(path TEXT PRIMARY KEY,kind TEXT) WITHOUT ROWID')
for v in lines(OUT/'dictionaries/gta6strings-ArchiveItems.txt'):
 db.execute('INSERT OR REPLACE INTO gta6_source VALUES(?,?)',(v,'direct'))
 stem=v.rsplit('.',1)[0] if '.' in v.rsplit('/',1)[-1] else v
 db.execute('INSERT OR IGNORE INTO gta6_source VALUES(?,?)',(stem,'extension_removed'))
db.commit()
report['gta6_database_direct_64bit_hashes']=num("SELECT count(DISTINCT m.h64) FROM matches m JOIN gta6_source s ON m.path=s.path WHERE m.flags&1 AND s.kind='direct'")
report['gta6_database_direct_or_extension_removed_64bit_hashes']=num('SELECT count(DISTINCT m.h64) FROM matches m JOIN gta6_source s ON m.path=s.path WHERE m.flags&1')
report['gta6_database_new_direct_or_stem_64bit_hashes']=num('SELECT count(DISTINCT m.h64) FROM matches m JOIN gta6_source s ON m.path=s.path WHERE m.flags&1 AND NOT EXISTS(SELECT 1 FROM baseline b WHERE b.width=64 AND b.hash=m.h64)')
with (OUT/'gpu-names.txt').open('w') as f:
 for (path,) in db.execute('SELECT DISTINCT path FROM matches WHERE flags&1 ORDER BY path'):f.write(path+'\n')
with (OUT/'gpu-rules-only.tsv').open('w') as f:
 w=csv.writer(f,delimiter='\t',lineterminator='\n');w.writerow(['name_hash_64','candidate'])
 for row in db.execute('SELECT DISTINCT h64,path FROM matches m WHERE flags&1 AND NOT EXISTS(SELECT 1 FROM direct_sources d WHERE d.h64=m.h64) ORDER BY h64,path'):w.writerow(row)
report['baseline_sha256']={name:sha(OUT/name) for name in prior if (OUT/name).exists()}
report.update(total_entries=total,entries_with_64bit_candidate=covered,entries_with_same_candidate_matching_both_hashes=both,elapsed_seconds=time.monotonic()-start)
(OUT/'gpu-summary.json').write_text(json.dumps(report,indent=2)+'\n');print(json.dumps(report),flush=True)
