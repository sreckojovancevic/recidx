# recidx — Report

**Question.** Can the *recognition* principle from
[recognition_sort](https://github.com/sreckojovancevic/recognition_sort) and My Radix LENS — an
element finds its own place from its own value, without comparisons — be used not only to sort,
but to *build and serve an index* in a database? Compared against what SQLite and standard
in-memory structures already do.

**Short answer (for this data, this machine).** Yes, as an in-memory index for an integer key:
the dynamic Recognition Index (per-bucket arrays + Fenwick tree) builds 2× faster than a Rust
`BTreeSet`, answers equality lookups 1.22× faster, counts range hits 9.5–321× faster, and is
about 1.2× *slower* on inserts/deletes/updates. Sorting alone is not where the gain is: inside
SQLite's own `CREATE INDEX`, sorting is a small part of the total time.
When data clusters, a fixed bucket function loses on changes; an adaptive variant (ARI, §12) that
refines only overloaded buckets brings changes on clustered data from 14.7 µs to 0.75 µs per
operation — still slower than `BTreeSet` (0.56 µs). Almost every building block has a known name
in the literature (§13).

Everything below is measured; every number has a source file in [`results/runs/`](results/runs).
Ratios are relative, on one machine and one dataset — read them as behaviour, not as a leaderboard.

---

## 1. Setup

| | |
|---|---|
| Data | copy of a real SQLite database of an M365 backup system (31 GB file), table `backup_index` |
| Key | `size_bytes` (INTEGER), 4,581,340 rows, 939,428 distinct values, min 1, max 22,055,494,656 (35 bits), 47 values > u32::MAX, no NULLs |
| Machine | Intel i9, 10 physical / 20 logical cores, 32 GB RAM, Windows; database on SATA SSD |
| SQLite | 3.46.0 (bundled via `rusqlite`), PRAGMA defaults unless stated |
| Method | median of 5 repetitions; method order rotated between repetitions; every structure's output checked against a reference by fingerprint (FNV over `(key, rowid)` in order) — all checks passed in all reported runs |

The database itself is **not** in this repository. Only counts and timings are published.

## 2. Structures compared

| Name | What it is |
|---|---|
| **Recognition Index (static)** | array of `(key, rowid)` sorted by the LENS radix + a table of bucket start positions built by one counting/prefix-sum pass. Lookup: the key computes its bucket (no comparisons), then binary search only inside that bucket. |
| Bucket functions | `Log(S)`: bit length of the key + next `S` bits (monotone, log scale); `Linear(16)`: top 16 bits of the key range (classic radix bucket) |
| **Recognition B (dynamic)** | one sorted array per bucket (`Log(12)`) + Fenwick tree over bucket sizes; inserts/deletes touch only one bucket; range COUNT = two bucket searches + one Fenwick sum |
| Recognition A / C | A: one array, every change shifts it; C: static array + change buffer, periodic rebuild |
| Sorted array + binary search | same sorted data, no bucket table |
| Rust `BTreeSet<(key,rowid)>` | in-memory B-tree |
| Hash map (FxHash) | key → rowids; equality only, no ranges |
| SQLite B-tree | `CREATE INDEX` in `:memory:` and on disk; queried through SQL |

## 3. Index build (step 1–2)

Run `run_1792405529.txt` (v0.8), 1 thread.

| Structure | Build | Memory |
|---|---|---|
| Recognition Index, Log(12) | 124.9 ms | 70.3 MiB |
| Recognition B (dynamic) | 150.3 ms | 76.5 MiB |
| Sorted array | 116.3 ms | 69.9 MiB |
| Rust BTreeSet | 294.5 ms | 79.4 MiB |
| Hash map | 385.1 ms | 124.7 MiB |
| SQLite `CREATE INDEX`, :memory: | 1,788.6 ms | 51.2 MiB (index pages) |
| SQLite `CREATE INDEX`, on disk (whole operation) | 4,974.3 ms | on disk |

Where the time goes in SQLite on disk: the table scan alone is ~3.3 s warm (`run_1792401979.txt`) (**60.4 s cold**, first read of the 31 GB file, `run_1792401552.txt`), so sorting is a
small share of building an index on disk. Replacing SQLite's sorter with a radix was therefore
**not** pursued (no fork of SQLite).

## 4. Equality lookup `WHERE size_bytes = ?` (step 2)

`run_1792405529.txt`, 200,000 queries (½ existing values weighted by frequency, ¼ random distinct
values, ¼ random values in range), 10,774,851 rows returned by every structure.

| Structure | ns / query |
|---|---|
| Hash map | 475 |
| **Recognition Index, Log(12)** | **530** |
| Recognition B (dynamic) | 561 |
| Recognition Index, Linear(16) | 662 |
| Sorted array + binary search | 674 |
| Rust BTreeSet | 686 |
| SQLite :memory: (through SQL) | 3,881 |
| SQLite on disk (through SQL) | 9,798 |

Bucket statistics: `Log(12)` → largest bucket 4,417 rows; `Linear(16)` → one bucket holds
3,638,389 rows (79%) because file sizes are very unevenly distributed. **The bucket function
decides whether recognition helps at all.**

## 5. Range queries `BETWEEN a AND b` (step 3)

`run_1792405529.txt`, 2,000 ranges per width class, `b = a + a·w`. Average hits: 454 / 4,001 / 37,405.

**COUNT (number of hits), ns per query**

| Structure | 0.1% | 1% | 10% |
|---|---|---|---|
| Recognition Index, Log(12) | 101 | 205 | 236 |
| Recognition B (dynamic) | 112 | 254 | 316 |
| Sorted array + binary search | 223 | 527 | 649 |
| Rust BTreeSet | 1,064 | 11,584 | 101,452 |
| SQLite :memory: | 12,394 | 100,609 | 936,821 |
| SQLite on disk | 21,369 | 130,304 | 1,163,135 |

Sorted structures count as a difference of two positions, independent of the number of hits;
`BTreeSet` and SQLite walk the range. The advantage over SQLite comes from the sorted layout
(any sorted array has it); the advantage over plain binary search comes from bucket recognition.

**FETCH (all rowids)**: all sorted structures are within a few percent (~6.9 ns per returned row);
SQLite ~58–65 ns per row through SQL.

## 6. Changes — CRUD (step 4)

`run_1792405529.txt`. 100,000 operations: 30% Create, 25% Delete, 25% Update (`size_bytes`
changes → delete + insert in the index), 20% Read. Every read checked against a reference model;
final state of every structure checked by fingerprint. µs per operation:

| Structure | batch 1 | batch 1,000 | batch 100,000 |
|---|---|---|---|
| Hash map (no ranges) | 0.34 | 0.35 | 0.35 |
| Rust BTreeSet | 0.59 | 0.61 | 0.59 |
| **Recognition B (dynamic)** | **0.74** | **0.72** | **0.73** |
| Recognition C (buffer + rebuild) | 1.31 | 1.30 | 1.31 |
| SQLite :memory: | 8.69 | 5.35 | 3.64 |
| SQLite on disk, synchronous=OFF | 59.46 | 45.48 | 29.20 |
| SQLite on disk, synchronous=FULL | 790.62 | 157.58 | 34.90 |
| Recognition A (one array) | 2,929.76 | 2,896.50 | 2,960.20 |

Limits stated in the output: A ran 3,000 operations (every change moves ~35 MB); SQLite on disk
with batch 1 ran 2,000 (FULL) / 10,000 (OFF) operations. SQLite on disk uses a separate table
`recidx_t(k)` with one index in the same file, so it is compared to single-index structures;
it is dropped afterwards. Rust structures have no persistence; SQLite with synchronous=FULL pays
for durability (fsync is ~96% of its time at batch 1, `run_1792404677.txt`).

## 7. The dynamic Recognition Index vs `BTreeSet` — summary

| | Recognition B | BTreeSet | |
|---|---|---|---|
| Build | 150 ms | 295 ms | B 2.0× faster |
| Memory | 76.5 MiB | 79.4 MiB | B 4% less |
| Equality lookup | 561 ns | 686 ns | B 1.22× faster |
| Range COUNT | 112–316 ns | 1.1–101 µs | B 9.5–321× faster |
| Range FETCH | ≈ | 8–14% slower | B slightly faster |
| CRUD | 0.72–0.74 µs | 0.59–0.61 µs | **BTreeSet 1.2× faster** |

## 8. Radix sort itself — what limited it, and what helped

Sorting 4,581,340 pairs of 16 bytes (`(u64 key, i64 rowid)`), in memory.

**Thread count** (`run_1792405810/5763/5262`): 9 and 10 threads give the same time (75.5 / 75.4 ms);
20 threads (hyper-threading) is ~11% slower (84.0 ms). No odd/even effect found on the 9/10 pair.
Default since v0.9: physical cores − 1.

**Variants** (`run_1792414722.txt`, v0.10):

| Variant | 1 thread | 9 threads |
|---|---|---|
| LENS radix (port of My Radix LENS) | 124.8 ms | 77.0 ms |
| + "third block" (write-combining buffer, W=16) | 133.0 ms | 75.8 ms |
| fused histogram, 8-bit digit (5 passes + copy) | **93.3 ms** | 60.5 ms |
| fused histogram, 11-bit digit (4 passes, no copy) | 107.0 ms | **52.1 ms** |
| fused histogram, 12-bit digit (3 passes + copy) | 106.3 ms | 56.3 ms |
| `std sort_unstable` / `rayon par_sort_unstable` | 143.3 ms | 40.4 ms |

- The "third block" (a small per-bucket buffer between source and destination) did not change
  the multi-thread behaviour measurably (≤ 5%) and costs 6–8% in one thread.
- Counting all digits in one pass and computing the next histogram *during* the scatter
  ("fused histogram") gave −21% (1 thread) / −19% (9 threads).
- Fewer, wider digits help with many threads (11 bits: 52.1 ms) but hurt in one thread
  (scattering into 2,048 buckets costs more than the saved pass). **The best digit width depends
  on the thread count.**
- On 9 threads LENS radix went 77.0 → 52.1 ms (−32%); `par_sort_unstable` (40.4 ms) is still
  1.29× faster. In one thread the fused 8-bit radix is 1.54× faster than `sort_unstable`.

## 9. Predictions written before measuring

Kept here including the misses (details in [`docs/IDEJA_treci_blok.md`](docs/IDEJA_treci_blok.md)).

| Prediction | Result | |
|---|---|---|
| v0.9: fused histogram on 9–10 threads → 45–55 ms | 60.5–62.5 ms | **missed** (direction right, size wrong; memory-traffic estimate ignored read-for-ownership on writes) |
| v0.9: saving smaller in 1 thread than on many | 1 thread saved more (−21% vs −19%) | **missed** |
| v0.10: 11-bit digit on 9 threads → 45–55 ms | 52.1 ms | hit |
| v0.10: 12-bit not better than 11-bit | 56.3 vs 52.1 ms | hit |
| v0.10: 11-bit digit in 1 thread → 70–85 ms | 107.0 ms (slower than 8-bit) | **missed** |

## 10. Limits — what this does *not* show

- One column, one integer key, one dataset, one machine. String keys (e.g. e-mail, subject) were
  not tested; they would need the MSD/string side of the recognition principle.
- The Rust structures live in memory without persistence or crash recovery (PoC phase 3,
  durability, not done).
- SQLite numbers in §3–§6 include SQL machinery. §11 measures the Recognition Index *through*
  SQL (virtual table) for read queries only; writes through the virtual table were not implemented.
- SQLite was used with default PRAGMAs except where stated; a tuned SQLite (larger cache) was
  measured only for `CREATE INDEX` in memory, where it was *slower* (2.9 s vs 1.8 s).
- The virtual table in §11 is read-only, in memory, built once from the data; it is not kept in
  sync with the SQLite table and has no persistence.
- No claim of O(1) is made: lookup is "compute bucket + binary search inside the bucket", and its
  cost depends on how evenly the bucket function spreads the data.

## 11. Used *from* SQLite — virtual table (v0.11)

`run_1792415582.txt`, 4,581,340 rows, 1 thread. The static Recognition Index (Log(12)) is exposed
as an SQLite virtual table `recog(k)` (constraints `=`, `>`, `>=`, `<`, `<=`, `BETWEEN` handled by the
index; `EXPLAIN QUERY PLAN` shows `SCAN recog VIRTUAL TABLE INDEX 1/10`) plus an SQL function
`recog_count(a, b)`. The **same SQL** runs on the same in-memory connection against `recog` and
against a normal table with an SQLite B-tree index. All results identical to the B-tree.

**Equality, 200,000 queries (ns per query)**

| Access | ns / query | vs B-tree through SQL |
|---|---|---|
| Recognition Index, direct from Rust (no SQL) | 556 | 0.14× |
| Recognition Index through virtual table | 2,746 | **0.67×** |
| SQLite B-tree through SQL | 4,072 | 1.00× |

**Range COUNT, 2,000 ranges per class (ns per query)**

| Access | 0.1% | 1% | 10% |
|---|---|---|---|
| `SELECT recog_count(a, b)` | 195 | 184 | 200 |
| `count(*)` through virtual table | 5,379 | 45,408 | 426,430 |
| `count(*)` on SQLite B-tree | 12,170 | 102,242 | 967,930 |

`recog_count` is 62× / 556× / 4,840× faster than `count(*)` on the B-tree, and does not depend on
the number of hits. `count(*)` through the virtual table still walks rows (SQLite asks for them one by
one) but is 2.3× faster than the B-tree walk.

**Range FETCH (all rowids)**: virtual table 0.69–0.73× of the B-tree time
(19,537 / 171,727 / 1,530,631 ns vs 26,682 / 239,664 / 2,228,147 ns).

**Prediction check** (written before measuring, [`docs/PREDVIDJANJE_vtab.md`](docs/PREDVIDJANJE_vtab.md)):

| Prediction | Result | |
|---|---|---|
| equality through virtual table 0.8–1.2× of B-tree | 0.67× | **missed** (faster than predicted) |
| `count(*)` through virtual table not faster, 1–3× slower | 0.44× (2.3× faster) | **missed** |
| `recog_count` 1–3 µs per query | 184–200 ns | **missed in value** (direction right) |
| FETCH through virtual table ±30% of B-tree | 0.69–0.73× | borderline |

The prediction overestimated the cost of SQLite calling into the virtual table per row and
underestimated what SQLite's own B-tree costs per row (page navigation, record decoding).

**What this answers.** The open question from §10 — does the advantage survive being called from
SQLite — has a measured answer for reads: SQL adds ~2.2 µs per equality query (556 → 2,746 ns), so
most of the raw advantage is absorbed, but the virtual table is still 1.48× faster than SQLite's
B-tree for equality, 1.4× for fetching ranges, and a dedicated SQL function keeps range counting at
~200 ns.


## 12. Adaptive Recognition Index — the matrix (v0.12 / v0.12b)

**Origin.** Another AI system, shown these results, proposed an *Adaptive Recognition Index*:
start from the fixed `Log(12)` buckets, and when one bucket grows past a threshold, refine *only
that bucket* by the next 8 bits of the key (with hysteresis: merge back below half the threshold).
It also proposed the tests. All of them were implemented as one matrix (`--matrix`); criteria
K1–K8 and Claude's predictions P1–P8 were written before the run
([`docs/ARI_KRITERIJUMI_I_PREDVIDJANJA.md`](docs/ARI_KRITERIJUMI_I_PREDVIDJANJA.md)).

**Matrix.** 13 variants × 5 datasets × 3 sizes (100k, 1M, 4.58M) × 3 workload mixes (insert-heavy,
balanced, delete-heavy), 3 repetitions, 100,000 changes per mix. Every cell is checked: structure
invariants, every read against a reference model, final fingerprint against the truth. **All cells
correct in both runs.**

| Variant | What it is |
|---|---|
| A_Log12 | dynamic Recognition Index from §6 (fixed buckets) |
| B_Log14 | the same with 4× finer fixed buckets — the control: is adaptivity better than simply finer? |
| C_ARI_{1024,4096,16384,65536} | ARI, refinement threshold |
| C_ARI_4096_efektivni / _lokalni_sort / _fenwick | effective (access-counted) trigger; local sort instead of distribution; Fenwick child counts |
| D_globalna_izgradnja | global rebuild with a finer bucket function when overloaded |
| E_BTreeSet, F_sortiran_niz | references (F limited to 3,000 changes above 100k rows) |
| G_posting | each distinct key stored once + list of rowids |

Datasets: *ravnomeran* (uniform), *grupisan* (90% of rows in 64 narrow clusters), *duplikati* (50% of
rows on 100 values), *promenljiv* (uniform start, changes go into 8 new narrow clusters — drift),
*m365* (the real `size_bytes`).

**Runs.** `matrix_1792483070` (v0.12) and `matrix_1792487363` (v0.12b), both in
[`results/matrix/`](results/matrix). In v0.12 the *promenljiv* dataset never overloaded a bucket
(largest bucket 1,312 rows), so it did not test what it was meant to test; v0.12b fixed the dataset
and added per-operation latency (p50/p95/p99 for insert/delete/update/read) and memory growth after
changes. Criteria and predictions were not changed between runs. Numbers below are from v0.12b,
4,581,340 rows, balanced mix.

### 12.1 Where ARI wins: changes on clustered data

| clustered | CRUD µs/op | insert ns (p99) | delete ns (p99) | eq ns | COUNT 1% ns | memory MiB | refinements |
|---|---|---|---|---|---|---|---|
| A_Log12 | 14.72 | 14,609 (58,500) | 13,036 (54,000) | 261 | 243 | 76.5 | — |
| B_Log14 | 6.42 | 6,011 (28,200) | 5,154 (22,500) | 270 | 176 | 95.4 | — |
| **C_ARI_4096** | **0.75** | 880 (3,700) | 492 (1,100) | 226 | 180 | 80.2 | 175 |
| C_ARI_65536 | 14.93 | 15,611 | 13,504 | 300 | 187 | 79.0 | 0 |
| D_globalna_izgradnja | 2.16 | 2,300 | 1,772 | 267 | 249 | 168 | 0 |
| E_BTreeSet | 0.56 | 600 (1,100) | 399 (900) | 286 | 113,128 | 79.4 | — |
| G_posting | 21.24 | 21,154 | 18,856 | 332 | 358 | 91.3 | — |

ARI is 19.7× faster than A and 8.6× faster than the finer fixed B on changes, for +5% memory.
The gain is in moving fewer elements per insert/delete, not in lookups: a lookup inside a large
bucket is a binary search, so bucket size barely matters (eq A/C 1.15×). Threshold 4096 was best;
65536 never triggered and equals A.

### 12.2 Where it does not

- **Uniform and real (M365) data:** ARI ≈ A (no or one refinement); differences are inside the
  noise (12.4).
- **Duplicates:** a key repeated ~23,000 times cannot be split by its bits. Delete costs 5.4 µs in A,
  6–8 µs in ARI, 0.5 µs in `BTreeSet`; posting lists (G) bring it to 2.4 µs and use 0.88× of A's memory.
- **Drift (*promenljiv*):** ARI_1024 is the fastest Recognition variant (0.45 µs vs A 0.72), but
  `BTreeSet` is 0.28 µs. Global rebuild (D) pays 290 ms of pauses in total (longest 104 ms) and
  doubles memory; ARI's local refinements pause 0.2 ms in total (longest 41 µs).
- **Effective trigger** (refine only after accesses ≥ 1/8 of the bucket size) never fired on the
  clustered data: a top bucket there holds a whole cluster (~64k rows → ~8k accesses needed), and
  100,000 changes spread over 64 clusters give ~1.5k per bucket. So it stayed equal to A (15.19 µs).
  This is the chosen threshold, not a defect; on drift (small, growing buckets) it fired like the
  structural trigger.

### 12.3 Maintenance cost — memory after changes

Balanced mix, MiB after build → after changes: A 76.5 → 126.1, ARI_4096 79.0 → 128.6,
`BTreeSet` 79.4 → 96.2 (uniform data). Per-bucket vectors double their capacity on the first insert
after an exact-size build; on drift, where changes go into few buckets, growth is ~1 MiB. A
real implementation would grow buckets in smaller steps or shrink them in the background — not
measured here.

### 12.4 Measurement noise

On uniform data all ARI configurations have *identical* structure (0 refinements), yet lookup
medians range 226–400 ns and COUNT 151–467 ns. Differences below ~1.5× in the sub-microsecond
columns are therefore not significant in this matrix; the CRUD differences in 12.1 (8–20×) are.

### 12.5 Criteria and predictions (v0.12b)

| # | criterion (from the proposal) | measured | |
|---|---|---|---|
| K1 | locality: a change touches only its bucket | 0 violations | pass |
| K2 | correctness | all checks correct | pass |
| K3 | uniform data: lookup ≤ 1.10× A | 0.90 / 1.03 / 0.84 | pass |
| K4 | ≥ 2× faster than A on the overloaded part (queries) | best 1.35× | **fail** — the gain is in changes (19.7×), the criterion measured queries |
| K5 | memory ≤ 1.25× A | max 1.30 (small sizes; 1.03–1.05 at 4.58M) | **fail** |
| K6 | ≤ 2 merges of the same bucket | max 1 | pass |
| K7 | CRUD incl. pauses ≤ A everywhere | A/C 0.90–22.5 | **fail** (only drift/delete-heavy, 0.90, inside noise) |
| K8 | control: ARI vs finer fixed B | clustered CRUD B/C 8.58; drift 1.11 | ARI better on changes |

| # | prediction (Claude, before measuring) | measured | |
|---|---|---|---|
| P1 | M365: ARI ≈ A ±10%, ≤ 10 refined nodes | eq 0.94, COUNT 0.89, CRUD 1.01; 1 node | missed (COUNT by 1 pt) |
| P2 | uniform: 0 refinements, ARI = A ±5% | 0 refinements; 0.84–1.03 | missed |
| P3 | clustered: A/ARI lookup ≥ 1.5; B gets ≥ 80% of the gain | 1.15; B −26% | missed |
| P4 | duplicates: no ARI gain; G ≤ 0.7× memory; G COUNT ±20% | ARI 1.05; G 0.88; G COUNT 1.74× | missed (first part right) |
| P5 | drift: A/ARI CRUD ≥ 1.5; D pause ≥ 100 ms | 1.35; 108 ms | missed (pause right) |
| P6 | longest refinement pause ≤ 2 ms | 0.20 ms | **hit** |
| P7 | Fenwick vs plain child counts < 5% | 0.65–1.16 | missed (noise) |
| P8 | distribution ≥ 2× faster than local sort in refinement, total < 5% | 1.15; total 0.96–1.03 | missed (total right) |

One hit of eight, as in v0.12. The largest real miss is P3: Claude expected the gain to show in
lookups and expected a finer fixed bucket to capture most of it; both were wrong.

**What this answers.** Adaptivity is worth it for one thing — changes on data that clusters —
where it is an order of magnitude better than any fixed bucket function and close to a B-tree,
while keeping the Recognition advantage for range counting. It does not help duplicates, and it
does not beat `BTreeSet` on changes anywhere.

## 13. Related work

Nearly every part of this work exists in the literature under another name; this repository makes
no claim of a new data structure.

- *Element finds its place from its value:* radix/trie indexes; learned indexes (Kraska et al.,
  "The Case for Learned Index Structures", 2018), RadixSpline, PGM-index.
- *`Log(S)` bucket function:* exponent + mantissa as in floating point; HDR Histogram bucketing.
- *ARI:* close to the Adaptive Radix Tree (Leis, Kemper, Neumann, 2013) — adaptive nodes by key
  bytes with path compression; also Judy arrays, and ALEX (2020) for adaptive node splitting.
- *Fenwick-tree range COUNT:* order-statistic trees and counted B-trees. SQLite's B-tree stores no
  subtree counts, which is why its `count(*)` walks rows (§5, §11).
- *Posting lists (G):* inverted indexes (Lucene).
- *Global rebuild (D):* periodic rebuilding; LSM-tree compaction.

What this repository adds is one framing (recognition for sorting, indexing and counting) and an
open measurement on a real 4.58M-row database, with predictions written before measuring and misses
reported as misses.
