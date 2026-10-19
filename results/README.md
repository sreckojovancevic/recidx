# Results index

Every file in `runs/` is the unedited text output of one `recidx` run on the author's machine
(Intel i9, 10 physical / 20 logical cores, 32 GB RAM; database on a SATA SSD, drive `D:`).
The program output is in Serbian. File names are `run_<unix timestamp>.txt`.

**Important:** runs were produced by the version of the code that existed at that moment
(v0.1 … v0.11). Only the final code (v0.11) is in `src/`. Earlier versions measured fewer
structures; their numbers are kept as-is, not re-run.

| File | Version | Command (`cargo run --release -- …`) | What it measures |
|---|---|---|---|
| run_1792401552.txt | v0.1 | *(none)* | step 1: simple radix vs std sorts vs SQLite `CREATE INDEX`; first cold extraction (60 s) |
| run_1792401979.txt | v0.2 | `--cache` | step 1 with the My Radix LENS port, 1 and 20 threads |
| run_1792402026.txt | v0.2 | `--cache --disk` | + on-disk `CREATE INDEX` (5.0 s) |
| run_1792402862.txt | v0.3 | `--cache --find` | step 2: build + `WHERE size_bytes = ?` |
| run_1792402900.txt | v0.3 | `--cache --find --disk` | + SQLite on disk |
| run_1792403268.txt | v0.4 | `--cache --range` | step 3: `BETWEEN` COUNT / FETCH |
| run_1792403398.txt | v0.4 | `--cache --range --disk` | + SQLite on disk |
| run_1792404076.txt | v0.5 | `--cache --crud` | step 4: CRUD, variants A/B/C, BTreeSet, hash, SQLite :memory: |
| run_1792404169.txt | v0.5 | `--cache --crud --disk` | + SQLite on disk (synchronous=FULL) |
| run_1792404677.txt | v0.7 | `--cache --find --range --crud --disk` | full run; BTreeSet added to steps 2–3; SQLite disk FULL and OFF |
| run_1792405262.txt | v0.8 | `--cache` | step 1 with the "third block" (write-combining) experiment, 20 threads |
| run_1792405529.txt | v0.8 | `--cache --find --range --crud --disk` | **full run with the dynamic Recognition Index (B + Fenwick)** |
| run_1792405763.txt | v0.8 | `--cache --threads 10` | thread-count check |
| run_1792405810.txt | v0.8 | `--cache --threads 9` | thread-count check |
| run_1792414113.txt | v0.9 | `--cache` | fused histogram; default threads = physical cores − 1 |
| run_1792414722.txt | v0.10 | `--cache` | **digit width 8 / 11 / 12 bits** |
| run_1792415582.txt | v0.11 | `--cache --vtab` | **Recognition Index used from SQLite: virtual table + `recog_count`** |

`spreadsheets/` holds the Excel summaries made during the work (in Serbian); every number in
them comes from the run files above, and ratios are formulas.
