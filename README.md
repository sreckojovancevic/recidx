# recidx — Recognition Index

> Can a value find its own place in an **index**, not only in a sorted array?
> A measured proof of concept: the *recognition* principle from
> [recognition_sort](https://github.com/sreckojovancevic/recognition_sort) and My Radix LENS,
> applied to building and querying an index over 4.58 million real database rows,
> compared against SQLite, a Rust `BTreeSet` and a hash map.

**Conceptual parent:** [recognition_sort](https://github.com/sreckojovancevic/recognition_sort) —
an element locates itself from its own identity (its digits / bytes), without pairwise comparison.
This repository asks what that principle does when the job is an *index*: build it, look things up,
answer ranges, and survive inserts, deletes and updates.

> **Scope.** This is not a proposed replacement for SQLite, B-trees, or any production index.
> It measures whether the recognition principle used in My Radix LENS is useful as a component
> for building specialized in-memory indexes.

Full results with sources: **[REPORT.md](REPORT.md)**.

---

## What it is

`recidx` is a single Rust benchmark program. It reads `(size_bytes, rowid)` from a copy of a real
SQLite database (an M365 backup index, 31 GB file, table `backup_index`), and measures:

| Step | Flag | Measures |
|---|---|---|
| 1 | *(default)* | sorting / index build: radix variants vs std & rayon sorts vs SQLite `CREATE INDEX` |
| 2 | `--find` | build + `WHERE size_bytes = ?` |
| 3 | `--range` | `WHERE size_bytes BETWEEN a AND b` — COUNT and FETCH |
| 4 | `--crud` | 100,000 mixed Create / Delete / Update / Read operations, batches of 1 / 1,000 / 100,000 |
| 5 | `--vtab` | the Recognition Index used *from* SQLite: virtual table `recog` + SQL function `recog_count(a, b)`, same SQL as against a B-tree |
| — | `--disk` | adds SQLite on the real database file to steps 1–4 |

Every structure's result is checked against a reference (fingerprint of `(key, rowid)` in order,
and a reference model for every read in CRUD). A wrong answer is printed as `NE` (“no”); all
published runs are `DA` (“yes”) everywhere.

## The Recognition Index

```
key ──recognize──▶ bucket b = f(key)          (no comparison; f = bit length + next 12 bits)
                    │
                    ▼
        start[b] .. start[b+1]  ──binary search inside the bucket only──▶ rowids
```

- **Static:** one array sorted by the LENS radix + a bucket start table made by one counting /
  prefix-sum pass (the same step a radix pass does).
- **Dynamic (B):** one small sorted array per bucket + a Fenwick tree over bucket sizes, so a
  change touches one bucket and a range COUNT is two bucket searches + one Fenwick sum.

## Results in one table

4,581,340 rows, 1 thread, Recognition B (dynamic) vs alternatives
(`results/runs/run_1792405529.txt`):

| | Recognition B | Rust BTreeSet | Hash map | SQLite :memory: |
|---|---|---|---|---|
| Build | **150 ms** | 295 ms | 385 ms | 1,789 ms |
| Memory | 76.5 MiB | 79.4 MiB | 124.7 MiB | 51.2 MiB |
| Lookup `=` | 561 ns | 686 ns | **475 ns** | 3,881 ns* |
| Range COUNT (37k hits) | **316 ns** | 101,452 ns | — | 936,821 ns* |
| CRUD per op | 0.73 µs | 0.59 µs | **0.35 µs** | 3.6–8.7 µs* |

\* through SQL; not a pure structure comparison.

What this says — and does not say:

- As an in-memory index for an integer key, the dynamic Recognition Index is faster than a B-tree
  at building, lookup and especially range counting, and ~1.2× slower at changes.
- The hash map is faster for equality and changes, but has no ranges and uses 1.6× more memory.
- Used *from* SQLite through a virtual table (same SQL as against a B-tree), equality lookups are
  1.48× faster than SQLite's B-tree (2.7 vs 4.1 µs) and `recog_count(a, b)` counts a range in
  ~0.2 µs vs ~1 ms for `count(*)` on the B-tree at 37k hits ([REPORT.md §11](REPORT.md)). Read-only;
  writes through the virtual table were not implemented.
- Sorting is not where SQLite spends its index-build time on disk; no SQLite fork was made.
- The bucket function decides everything: a plain top-16-bit bucket put 79% of rows in one bucket
  and gave no benefit.

Radix sort itself improved by −32% on 9 threads during the work (fused histogram, 11-bit digits),
but `rayon par_sort_unstable` is still 1.29× faster there — see [REPORT.md §8](REPORT.md).

## Run it

Requirements: Rust (stable), a C compiler for the bundled SQLite (MSVC on Windows), and a SQLite
database with a table `backup_index` that has an INTEGER column `size_bytes`
(default path `../db/index.db`, change with `--db`).

```powershell
# keep SQLite temp files on the same disk as the database
$env:TMP="D:\sqliteRecog\tmp"; $env:TEMP="D:\sqliteRecog\tmp"

cargo run --release -- --cache                               # step 1
cargo run --release -- --cache --find --range --crud --disk  # steps 2–4, with SQLite on disk
```

| Option | Meaning |
|---|---|
| `--db PATH` | database file (default `../db/index.db`) |
| `--cache` | extract `(size_bytes, rowid)` once into `results/pairs_cache.bin`, reuse afterwards |
| `--reps N` | repetitions (default 5, median reported) |
| `--threads N` | threads for the parallel part (default: physical cores − 1) |
| `--queries N`, `--ranges N` | number of lookups (200,000) / ranges per width class (2,000) |
| `--crud-ops N`, `--crud-thr N` | CRUD operations (100,000) / buffer threshold of variant C (65,536) |
| `--vtab` | step 5 (virtual table) |
| `--disk`, `--disk-reps N` | include SQLite on the real file (**modifies the database**: creates and drops an index / a table `recidx_t`) |

Output is in Serbian and is also saved to `results/run_<timestamp>.txt`.

## Repository layout

```
src/main.rs        extraction, inventory, step 1 (sorting), command line
src/lens_radix.rs  port of the My Radix LENS radix + experimental variants (third block, fused histogram, digit width)
src/find.rs        static Recognition Index, steps 2 and 3
src/dynidx.rs      dynamic Recognition Index (per-bucket arrays + Fenwick)
src/crud.rs        step 4: workload, reference model, variants A/B/C, BTreeSet, hash, SQLite
src/vtab.rs        step 5: Recognition Index as an SQLite virtual table + recog_count()
REPORT.md          full results, predictions, limits
results/runs/      unedited output of every run (index in results/README.md)
results/spreadsheets/  Excel summaries made during the work (Serbian)
docs/recognition_index_poc.md  research framework the work followed (Serbian)
docs/IDEJA_treci_blok.md       the "third block" idea, predictions and results (Serbian)
docs/PREDVIDJANJE_vtab.md      virtual-table predictions written before measuring + result (Serbian)
```

## Transparency notes

- Only the final code (v0.11) is in `src/`. Runs in `results/runs/` were produced by the code as it
  was at the time (v0.1 … v0.11); they are kept unedited and are mapped to versions in
  [results/README.md](results/README.md).
- `Cargo.toml` still says `version = "0.1.0"`; the program prints its real version (v0.11).
- The program text still points to `D:\sqliteRecog\IDEJA_treci_blok.md`; in this repository the
  file is [`docs/IDEJA_treci_blok.md`](docs/IDEJA_treci_blok.md). The code is published exactly as
  it was measured, so this was not changed.
- The database and `pairs_cache.bin` are not published. Only counts and timings are.
- Predictions were written down before measuring; six of nine missed (one more borderline), and
  they are reported as misses ([REPORT.md §9 and §11](REPORT.md)).

## Acknowledgments

**Srećko Jovančević — concept, direction, measurement.**
The recognition principle and its earlier implementations
([recognition_sort](https://github.com/sreckojovancevic/recognition_sort), My Radix LENS,
whose radix `lens_radix.rs` ports); the research question and the PoC framework
(`docs/recognition_index_poc.md`) the work followed; the choice of a real 4.58M-row database as the
test field; the requirement to show every result, good or bad; the decision to compare against
B-tree, hash and SQLite rather than a convenient opponent; the "third block / ternary flow" idea;
the rule *threads = physical cores − 1*; the scope statement at the top of this README; every run on his own machine, and every result in
`results/runs/`.

**Claude (Anthropic) — implementation, analysis and documentation, working from that direction.**
In a Claude app session configured for Claude Opus 5.5, Claude:

- wrote all of the `recidx` code: extraction, inventory, all benchmarks, the reference models and
  fingerprint checks, the unit tests;
- ported the My Radix LENS radix from `u32` to a 64-bit key with payload, and wrote the variants:
  write-combining ("third block"), fused histogram, variable digit width;
- wrote the SQLite virtual table (`recog`) and the `recog_count` SQL function;
- designed the bucket functions (`Log(S)`, `Linear`), the static and dynamic Recognition Index,
  the Fenwick extension, CRUD variants A/B/C and the CRUD workload;
- proposed the comparisons (BTreeSet, hash, SQLite `synchronous=FULL/OFF`) and the predictions;
- interpreted the results and wrote `REPORT.md`, this README, `results/README.md`, the
  spreadsheets, and the hypothesis/result sections of `docs/IDEJA_treci_blok.md`.

Mistakes Claude made during the work, for the record: a file transfer once delivered stale
sources, so v0.1 was compiled instead of v0.2 (caught by the program's own version line and
re-run); one answer overstated a ratio (SQLite on disk vs in memory: 8–13× instead of 6.8–10.1×; corrected when the spreadsheet was built);
one answer said a 12-bit digit needs no final copy (it needs 3 passes, so it does — corrected
before measuring); six of nine written predictions missed (§9 and §11 of the report) — including
the prediction that the advantage would mostly disappear through a virtual table. The memory-traffic
explanation in §8 is Claude's interpretation; only the timings are measured.

The idea and the decisions are Srećko's; the code and the text were produced by Claude and
checked against measurements on Srećko's machine.

## License

MIT — see [LICENSE](LICENSE).
