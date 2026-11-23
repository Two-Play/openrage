# RPF9 GPU dictionary matching

Standalone Vulkan tools for testing candidate names against exported RPF9 hashes.
No game data, keys, dictionaries, executables, generated shaders or binaries are
included. Nothing here downloads sources, reads archive payloads, or renames files.

## Requirements and commands

Linux with a discrete Vulkan GPU supporting `shaderInt64`, GPU device access,
Vulkan headers/loader, a C++17 compiler, `glslangValidator`, and Python 3.
The runner refuses software Vulkan. No Cargo build is needed.

From the repository root:

```sh
export RPF9_GAME_DIR="/path/to/game"
sh tools/rpf9_gpu_build.sh
python3 tools/rpf9_gpu_hash_algorithms.py --self-test
python3 tools/rpf9_gpu_match.py --validate-only
python3 tools/rpf9_gpu_match.py --limit 20000000
python3 tools/rpf9_gpu_report.py
```

`RPF9_GAME_DIR` must contain `rpf9-hash-export/`. Sources are located relative to
the scripts' own `TOOLS` directory, so invocation works from any directory.
Build products go to `$RPF9_GAME_DIR/rpf9-hash-export/gpu-build`, not the repository.
Optional overrides: `RPF9_GPU_BUILD_DIR`, `RPF9_GLSLANG` (default
`glslangValidator`), `CXX` (default `c++`), and `RPF9_PATH_CANDIDATES`.

## Inputs

Required in `rpf9-hash-export/`:

- `name_hashes_unique.txt`: sorted fixed-width 16-digit hexadecimal hashes.
- `name_hashes32_unique.txt`: sorted fixed-width 8-digit hexadecimal hashes.
- `entries.tsv` for the report, with columns `name_hash_64`, `name_hash_32`.

Optional candidate files:

- `dictionaries/gta6strings-ArchiveItems.txt`
- `dictionaries/gta5-installed.txt`, `dictionaries/rdr2-codex.txt`
- `dictionaries/gtav-objects.txt`, `gtav-peds.txt`, `gtav-vehicles.txt`
- `ida_common_paths.txt`, `rpf9-path-candidates.txt`

The last path can be overridden with `RPF9_PATH_CANDIDATES`. Missing optional
sources are skipped. Obtain dictionaries separately and retain their licenses
and provenance; the GTA6 source used in the original run was
[ahmed605/GTA6-StringsDatabase](https://github.com/ahmed605/GTA6-StringsDatabase),
revision `2ec950606516e5e734230ab9bd74d42e88770627` (MIT).

Optional baseline reports: `crossgame_matches.tsv`, `dictionary_matches.tsv`,
`ida_suffix_matches.tsv`, `numeric_variant_matches.tsv`. Delta counts compare
only these files, not any other concurrent results.

## Algorithm and checks

The shader and Python reference preserve the validated native RPF9 algorithm:
ASCII lowercase and backslash-to-slash normalization, NUL termination,
Murmur-derived 64-bit mixing, and **length modulo 256** in finalization.
This is not interchangeable with a generic MurmurHash library. Jenkins32 is
computed independently, including its leading-quote handling.

Before matching, 1,054 deterministic vectors compare both GPU hashes and target
membership with the CPU reference: tails 0–33, long strings including 256+,
NUL, quotes, normalization, high bytes, and seeded random inputs. Run without
Python `-O`, because validation assertions must remain enabled. The report
recomputes every emitted candidate on CPU before deduplication.

Rules include mount/inner-archive prefix stripping, basenames, extensionless
stems, common RAGE extensions (y/z/i/#), observed data directories, and bounded
numeric neighbours preserving digit width. This is a finite dictionary pass,
not exhaustive character brute force. Duplicate candidates count toward the
limit and throughput; the report deduplicates matches.

## Outputs and limits

All outputs stay in `rpf9-hash-export/`:

- `gpu-validation.json`, `gpu-sources.json`, `gpu-run.log`, `gpu-checkpoint.json`
- `gpu-matches.tsv`, `gpu-unique-matches.tsv`, `gpu-new64-matches.tsv`
- `gpu-summary.json`, `gpu-rules-only.tsv`, `gpu-analysis.sqlite`
- `gpu-names.txt`: unique candidates with a 64-bit match, for optional UI import

A new run without `--skip` replaces the preceding GPU outputs. `--skip N`
appends results after skipping N generated candidates; preserve the same inputs
and rules. Checkpoints can lead buffered work, so restart at least one batch
(4,096 candidates) before the recorded position; duplicates are harmless when
reported. Ctrl-C terminates the child. Already flushed output remains on disk.

Each GPU batch has at most 4,096 candidates in 1,024-byte slots, with a 2 ms pause
between batches and a 30 s fence timeout. Storage buffers are capped at 256 MiB
(~13 MiB for the measured corpus). Dictionaries and entry reports are streamed;
SQLite uses an 8 MiB page cache and disk temporary storage.

Raw output flags: 1 means a 64-bit target match; 2 means a 32-bit target match.
The target sets are independent: flag 3 alone does not identify one archive
entry. The report separately counts candidates matching both fields of the same
entry. A 32-bit-only hit remains an ambiguous hypothesis. Even a 64-bit match
can identify a stem rather than the complete original path and extension.

Original measured run on RX 7800 XT / RADV: 18,959,548 candidates in 36.429 s,
80,958 distinct 64-bit hashes covering 101,484 entries. Of these hashes, 78,978
matched direct GTA6 database strings; 491 required rules beyond all raw input
dictionaries. These are historical measurements, not results generated by
installing these source tools.

## Optional expanded, time-bounded pass

`rpf9_gpu_long.py` shares the same `RPF9_GAME_DIR`, `RPF9_GPU_BUILD_DIR`, shader
runner and CPU hash reference. It expands observed directory prefixes, numeric
neighbours (+/-32), asset suffixes and extensions. If available, it also reads
`rpf9-hash-export/cpu_extensions/extensions.txt`. No downloads are performed.

After building and running the full `--validate-only` check above:

```sh
python3 tools/rpf9_gpu_long.py --minutes 30 \
  --output "$RPF9_GAME_DIR/rpf9-hash-export/gpu-extended-new"
```

The output directory must not already exist. Foreground execution is recommended;
optional `--background` detaches the process and prints its PID. For background
mode, the output's parent directory must already exist; a sibling
`gpu-extended-new.launcher.log` receives its console output. Do not reuse the
output directory of an active run.

The duration (greater than zero, at most 60 minutes) bounds candidate submission;
seed preparation and final CPU verification take additional time. A watchdog
also covers a blocked pipe/GPU dispatch. Raw output is capped at approximately
512 MiB, checked between submissions, so queued batches can overshoot slightly.
This wrapper runs an eight-vector startup check; the separate 1,054-vector
validation is still recommended before launch.

Outputs inside the selected directory:

- `checkpoint.json`: current phase, progress, final counts when complete
- `rules.json`, `validation.json`, `seeds.sqlite`, `gpu.log`
- `matches.raw.tsv`: **raw GPU candidates, not yet CPU-verified results**
- `matches.cpu-verified.tsv`: generated during final CPU verification

While the run is active, the CPU validation report remains pending as a separate
phase. A partially written `matches.cpu-verified.tsv` does not establish that
verification finished: require `checkpoint.json` status `complete`. No final
coverage/baseline report or UI dictionary is automatically produced by this
wrapper. Do not feed its raw output into the UI as confirmed names; 32-bit hits
remain ambiguous even after their arithmetic is checked. This packaging adds
source only and does not start, alter or monitor any pre-existing long run.
