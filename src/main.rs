//! recidx v0.10 — Recognition Index PoC, korak 1: size_bytes
//!
//! Meri izgradnju uređenog indeksa nad kolonom backup_index.size_bytes:
//!   * izvlačenje (size_bytes, rowid) iz baze (samo čitanje)
//!   * Rust: radix iz My Radix LENS (lens_radix.rs, port), jednostavni radix iz v0.1,
//!     std/rayon stabilni i nestabilni sort
//!   * SQLite: CREATE INDEX nad istim parovima u :memory: bazi (podrazumevano i podešeno)
//!   * SQLite: CREATE INDEX nad pravom bazom (samo uz --disk; menja fajl, pa briše indeks)
//! Svi rezultati moraju dati identičan redosled (ključ, rowid) — proverava se otiskom.
//! Meri se u 1 niti i u N niti (Rust: rayon pool; SQLite: PRAGMA threads).

mod lens_radix;

use rayon::prelude::*;
use rusqlite::types::ValueRef;
use rusqlite::{ffi, params, Connection, OpenFlags};
use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Instant;

// ---------- brojački alokator (isti pristup kao ALLOCATOR.md u recognition_sort) ----------
struct Counting;
static CUR: AtomicUsize = AtomicUsize::new(0);
static PEAK: AtomicUsize = AtomicUsize::new(0);
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, l: Layout) -> *mut u8 {
        let p = System.alloc(l);
        if !p.is_null() {
            let c = CUR.fetch_add(l.size(), Ordering::Relaxed) + l.size();
            PEAK.fetch_max(c, Ordering::Relaxed);
        }
        p
    }
    unsafe fn dealloc(&self, p: *mut u8, l: Layout) {
        System.dealloc(p, l);
        CUR.fetch_sub(l.size(), Ordering::Relaxed);
    }
    unsafe fn realloc(&self, p: *mut u8, l: Layout, n: usize) -> *mut u8 {
        let q = System.realloc(p, l, n);
        if !q.is_null() {
            if n > l.size() {
                let c = CUR.fetch_add(n - l.size(), Ordering::Relaxed) + (n - l.size());
                PEAK.fetch_max(c, Ordering::Relaxed);
            } else {
                CUR.fetch_sub(l.size() - n, Ordering::Relaxed);
            }
        }
        q
    }
}
#[global_allocator]
static ALLOC: Counting = Counting;
fn cur() -> usize { CUR.load(Ordering::Relaxed) }
fn reset_peak() { PEAK.store(cur(), Ordering::Relaxed); }
fn peak() -> usize { PEAK.load(Ordering::Relaxed) }

// ---------- pomoćno ----------
macro_rules! out {
    ($s:expr, $($a:tt)*) => {{ let l = format!($($a)*); println!("{}", l); $s.push_str(&l); $s.push('\n'); }};
}
mod find;
mod crud;
mod dynidx;

fn ms(d: std::time::Duration) -> f64 { d.as_secs_f64() * 1000.0 }
fn mib(b: usize) -> f64 { b as f64 / 1048576.0 }
fn med(v: &[f64]) -> (f64, f64, f64) {
    let mut s = v.to_vec();
    s.sort_by(|a, b| a.partial_cmp(b).unwrap());
    (s[(s.len() - 1) / 2], s[0], s[s.len() - 1])
}
fn fnv(h: &mut u64, x: u64) {
    for i in 0..8 {
        *h ^= (x >> (8 * i)) & 0xff;
        *h = h.wrapping_mul(0x100000001b3);
    }
}
const FNV0: u64 = 0xcbf29ce484222325;
fn fingerprint(v: &[(u64, i64)]) -> u64 {
    let mut h = FNV0;
    for &(k, r) in v { fnv(&mut h, k); fnv(&mut h, r as u64); }
    h
}
/// Ključ prepoznavanja: NULL -> 0, inače (vrednost - koren) + 1.
/// Koren = minimum, kao u sort_apsolutni.rs; čuva SQLite redosled (NULL pre svih brojeva).
fn key(s: Option<i64>, root: i64) -> u64 {
    match s { None => 0, Some(x) => ((x as i128 - root as i128) as u64) + 1 }
}

// ---------- radix: LSD, baza 256, brojanje -> prefiksne sume -> raspoređivanje ----------
/// Stabilan. Ulaz u redosledu rowid-a => izlaz uređen po (ključ, rowid), kao SQLite indeks.
/// skip=true preskače prolaz u kome svi elementi imaju isti bajt (taj prolaz ne menja redosled).
/// Jednostavna jednonitna verzija iz v0.1 (bez unsafe), zadržana radi poređenja.
fn radix_sort(v: &mut Vec<(u64, i64)>, skip: bool) -> usize {
    let n = v.len();
    if n < 2 { return 0; }
    let mut a = std::mem::take(v);
    let mut b = vec![(0u64, 0i64); n];
    let mut passes = 0;
    for p in 0..8 {
        let sh = 8 * p;
        let mut cnt = [0usize; 256];
        for &(k, _) in a.iter() { cnt[((k >> sh) & 0xff) as usize] += 1; }
        if skip && cnt.iter().any(|&c| c == n) { continue; }
        let mut pos = [0usize; 256];
        let mut s = 0;
        for i in 0..256 { pos[i] = s; s += cnt[i]; }
        for &e in a.iter() {
            let d = ((e.0 >> sh) & 0xff) as usize;
            b[pos[d]] = e;
            pos[d] += 1;
        }
        std::mem::swap(&mut a, &mut b);
        passes += 1;
    }
    *v = a;
    passes
}

// ---------- SQLite :memory: ----------
#[allow(dead_code)]
struct MemRes { insert_ms: f64, times: Vec<f64>, highwater: i64, fp: u64 }
fn sqlite_mem(raw: &[(Option<i64>, i64)], tuned: bool, threads: usize, reps: usize, root: i64) -> MemRes {
    let m = Connection::open_in_memory().unwrap();
    m.execute_batch("PRAGMA journal_mode=OFF; PRAGMA synchronous=OFF; CREATE TABLE t(k INTEGER);").unwrap();
    if tuned {
        m.execute_batch("PRAGMA cache_size=-2097152; PRAGMA temp_store=MEMORY;").unwrap();
    }
    m.execute_batch(&format!("PRAGMA threads={threads};")).unwrap();
    let t = Instant::now();
    m.execute_batch("BEGIN").unwrap();
    {
        let mut st = m.prepare("INSERT INTO t(rowid,k) VALUES(?1,?2)").unwrap();
        for &(s, id) in raw { st.execute(params![id, s]).unwrap(); }
    }
    m.execute_batch("COMMIT").unwrap();
    let insert_ms = ms(t.elapsed());
    let mut times = vec![];
    let mut hw = 0i64;
    for r in 0..reps {
        let base = unsafe { ffi::sqlite3_memory_used() };
        unsafe { ffi::sqlite3_memory_highwater(1); }
        let t = Instant::now();
        m.execute_batch("CREATE INDEX i ON t(k)").unwrap();
        times.push(ms(t.elapsed()));
        hw = hw.max(unsafe { ffi::sqlite3_memory_highwater(0) } - base);
        if r + 1 < reps { m.execute_batch("DROP INDEX i").unwrap(); }
    }
    let mut h = FNV0;
    {
        let mut st = m.prepare("SELECT k, rowid FROM t INDEXED BY i").unwrap();
        let mut rows = st.query([]).unwrap();
        while let Some(r) = rows.next().unwrap() {
            let k: Option<i64> = r.get(0).unwrap();
            let id: i64 = r.get(1).unwrap();
            fnv(&mut h, key(k, root));
            fnv(&mut h, id as u64);
        }
    }
    MemRes { insert_ms, times, highwater: hw, fp: h }
}

// ---------- SQLite na disku (menja bazu; indeks se briše posle merenja) ----------
fn sqlite_disk(db: &str, reps: usize, root: i64) -> (Vec<f64>, u64) {
    let c = Connection::open(db).unwrap();
    c.execute_batch("DROP INDEX IF EXISTS idx_recidx_size").unwrap();
    let mut times = vec![];
    let mut h = FNV0;
    for r in 0..reps {
        let t = Instant::now();
        c.execute_batch("CREATE INDEX idx_recidx_size ON backup_index(size_bytes)").unwrap();
        times.push(ms(t.elapsed()));
        if r == 0 {
            let mut st = c.prepare("SELECT size_bytes, rowid FROM backup_index INDEXED BY idx_recidx_size").unwrap();
            let mut rows = st.query([]).unwrap();
            while let Some(row) = rows.next().unwrap() {
                let k: Option<i64> = row.get(0).unwrap();
                let id: i64 = row.get(1).unwrap();
                fnv(&mut h, key(k, root));
                fnv(&mut h, id as u64);
            }
        }
        c.execute_batch("DROP INDEX idx_recidx_size").unwrap();
    }
    (times, h)
}


fn save(o: &str) {
    let _ = std::fs::create_dir_all("results");
    let ts = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs();
    let p = format!("results/run_{ts}.txt");
    if std::fs::write(&p, o).is_ok() { println!("\n[sačuvano u {p}]"); }
}

// ---------- keš izvučenih parova (da se ne čeka puno skeniranje svaki put) ----------
const CACHE: &str = "results/pairs_cache.bin";
fn cache_save(raw: &[(Option<i64>, i64)]) {
    use std::io::Write;
    let _ = std::fs::create_dir_all("results");
    let mut w = std::io::BufWriter::new(std::fs::File::create(CACHE).unwrap());
    for &(s, id) in raw {
        w.write_all(&[s.is_some() as u8]).unwrap();
        w.write_all(&s.unwrap_or(0).to_le_bytes()).unwrap();
        w.write_all(&id.to_le_bytes()).unwrap();
    }
}
fn cache_load() -> Option<Vec<(Option<i64>, i64)>> {
    let b = std::fs::read(CACHE).ok()?;
    if b.len() % 17 != 0 { return None; }
    Some(b.chunks_exact(17).map(|c| {
        let s = i64::from_le_bytes(c[1..9].try_into().unwrap());
        let id = i64::from_le_bytes(c[9..17].try_into().unwrap());
        (if c[0] == 1 { Some(s) } else { None }, id)
    }).collect())
}

type Pairs = Vec<(u64, i64)>;
type SortFn = Box<dyn Fn(&mut Pairs) -> u32 + Sync>;
struct BenchRes { times: Vec<f64>, peak: usize, ok: bool, passes: u32 }

/// Svaka metoda dobija svežu kopiju ulaza (kopiranje nije u vremenu); redosled metoda se smenjuje.
fn bench(pool: &rayon::ThreadPool, keyed: &[(u64, i64)], fns: &[SortFn], reps: usize, reference: &[(u64, i64)]) -> Vec<BenchRes> {
    let k = fns.len();
    let mut res: Vec<BenchRes> = (0..k).map(|_| BenchRes { times: vec![], peak: 0, ok: true, passes: 0 }).collect();
    for r in 0..reps {
        for j in 0..k {
            let m = (j + r) % k;
            let mut v = keyed.to_vec();
            let base = cur();
            reset_peak();
            let t = Instant::now();
            let p = pool.install(|| fns[m](&mut v));
            let el = ms(t.elapsed());
            let pk = peak() - base;
            res[m].times.push(el);
            res[m].peak = res[m].peak.max(pk);
            res[m].passes = p;
            if r == 0 { res[m].ok = v.as_slice() == reference; }
        }
    }
    res
}

fn table(o: &mut String, names: &[&str], res: &[BenchRes], base_idx: usize) {
    let (bm, _, _) = med(&res[base_idx].times);
    out!(o, "| metoda | medijana ms | min | max | odnos prema {} | dodatna vršna mem. MiB | radix prolaza | isti redosled |", names[base_idx]);
    out!(o, "|---|---|---|---|---|---|---|---|");
    for (i, r) in res.iter().enumerate() {
        let (md, mn, mx) = med(&r.times);
        let pas = if r.passes > 0 { r.passes.to_string() } else { "-".into() };
        out!(o, "| {} | {md:.1} | {mn:.1} | {mx:.1} | {:.2}× | {:.1} | {pas} | {} |", names[i], bm / md, mib(r.peak), if r.ok { "DA" } else { "NE" });
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let mut db = "../db/index.db".to_string();
    let mut reps = 5usize;
    let mut disk = false;
    let mut disk_reps = 1usize;
    let mut use_cache = false;
    let mut do_find = false;
    let mut do_range = false;
    let mut do_crud = false;
    let mut crud_ops = 100_000usize;
    let mut crud_thr = 65_536usize;
    let mut nr = 2_000usize;
    let mut nq = 200_000usize;
    let phys = num_cpus::get_physical();
    let logical = num_cpus::get();
    // podrazumevano: broj fizičkih jezgara − 1 (odluka iz merenja 9/10/20 niti)
    let mut threads = phys.saturating_sub(1).max(1);
    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--db" => { i += 1; db = args[i].clone(); }
            "--reps" => { i += 1; reps = args[i].parse().unwrap(); }
            "--threads" => { i += 1; threads = args[i].parse().unwrap(); }
            "--disk" => disk = true,
            "--disk-reps" => { i += 1; disk_reps = args[i].parse().unwrap(); }
            "--cache" => use_cache = true,
            "--find" => do_find = true,
            "--range" => do_range = true,
            "--crud" => do_crud = true,
            "--crud-ops" => { i += 1; crud_ops = args[i].parse().unwrap(); }
            "--crud-thr" => { i += 1; crud_thr = args[i].parse().unwrap(); }
            "--ranges" => { i += 1; nr = args[i].parse().unwrap(); }
            "--queries" => { i += 1; nq = args[i].parse().unwrap(); }
            x => { eprintln!("nepoznat argument: {x}\nupotreba: recidx [--db PUTANJA] [--reps N] [--threads N] [--cache] [--find] [--queries N] [--range] [--ranges N_PO_KLASI] [--crud] [--crud-ops N] [--crud-thr N] [--disk] [--disk-reps N]"); std::process::exit(2); }
        }
        i += 1;
    }
    let mut o = String::new();
    out!(o, "# recidx v0.10 — Recognition Index, korak 1: backup_index.size_bytes");
    out!(o, "SQLite (bundled): {} | baza: {db}", rusqlite::version());
    out!(o, "procesor: {phys} fizičkih jezgara, {logical} logičkih | podrazumevano niti = fizička − 1");
    out!(o, "ponavljanja: {reps} | niti za paralelni deo: {threads} | keš parova: {use_cache} | disk test: {disk} ({disk_reps}x)");

    // 1) izvlačenje
    let mut t_extract: Option<f64> = None;
    let cached = if use_cache { cache_load() } else { None };
    let mut raw = match cached {
        Some(v) => { out!(o, "\n## 1. Izvlačenje: učitano iz {CACHE} (vreme izvlačenja nije mereno u ovom pokretanju)"); v }
        None => {
            let t = Instant::now();
            let c = Connection::open_with_flags(&db, OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX).expect("ne mogu da otvorim bazu");
            let mut raw: Vec<(Option<i64>, i64)> = Vec::with_capacity(5_000_000);
            let mut other = 0usize;
            {
                let mut st = c.prepare("SELECT rowid, size_bytes FROM backup_index").unwrap();
                let mut rows = st.query([]).unwrap();
                while let Some(r) = rows.next().unwrap() {
                    let id: i64 = r.get(0).unwrap();
                    let s = match r.get_ref(1).unwrap() {
                        ValueRef::Null => None,
                        ValueRef::Integer(x) => Some(x),
                        _ => { other += 1; None }
                    };
                    raw.push((s, id));
                }
            }
            drop(c);
            let te = ms(t.elapsed());
            t_extract = Some(te);
            out!(o, "\n## 1. Izvlačenje (size_bytes, rowid) — puno skeniranje tabele");
            out!(o, "vreme: {te:.0} ms");
            if other > 0 {
                out!(o, "GREŠKA: {other} redova ima size_bytes koji nije INTEGER ni NULL. Prekid.");
                save(&o);
                std::process::exit(1);
            }
            if use_cache { cache_save(&raw); out!(o, "parovi sačuvani u {CACHE}"); }
            raw
        }
    };

    // 2) inventar
    let n = raw.len();
    let rowid_sorted = raw.windows(2).all(|w| w[0].1 < w[1].1);
    if !rowid_sorted { raw.sort_by_key(|e| e.1); }
    let nulls = raw.iter().filter(|e| e.0.is_none()).count();
    let min = raw.iter().filter_map(|e| e.0).min();
    let max = raw.iter().filter_map(|e| e.0).max();
    let big = raw.iter().filter(|e| matches!(e.0, Some(x) if x > u32::MAX as i64)).count();
    let root = min.unwrap_or(0);
    let keyed: Pairs = raw.iter().map(|&(s, id)| (key(s, root), id)).collect();
    let max_key = keyed.iter().map(|e| e.0).max().unwrap_or(0);
    let key_bytes = ((64 - max_key.leading_zeros()) + 7) / 8;
    out!(o, "\n## 2. Inventar");
    out!(o, "redova: {n} | ulaz u redosledu rowid-a: {} | NULL: {nulls} | > u32::MAX: {big}", if rowid_sorted { "da" } else { "NE (sortirano pre merenja)" });
    out!(o, "min: {:?} | max: {:?} | najveći ključ: {max_key} → potrebno bajtova: {key_bytes}", min, max);

    // referenca: stabilan sort po ključu (nije u vremenu)
    let mut reference = keyed.clone();
    reference.sort_by_key(|e| e.0);
    let fp_ref = fingerprint(&reference);
    let distinct = 1 + reference.windows(2).filter(|w| w[0].0 != w[1].0).count();
    out!(o, "različitih vrednosti: {distinct} | otisak reference (ključ, rowid): {fp_ref:016x}");

    if do_find || do_range || do_crud {
        if do_find { find::step2(&mut o, &raw, &keyed, root, reps, nq, disk, &db); }
        if do_range { find::step3(&mut o, &raw, &keyed, root, reps, nr, disk, &db); }
        if do_crud { crud::step4(&mut o, &raw, &keyed, root, crud_ops, crud_thr, disk, &db); }
        save(&o);
        return;
    }

    // 3a) jedna nit
    let pool1 = rayon::ThreadPoolBuilder::new().num_threads(1).build().unwrap();
    let names1 = ["LENS radix (1 nit)", "LENS radix + treći blok W=16 (1 nit)", "LENS radix, spojen histogram v0.9 (1 nit)", "spojen histogram, cifra 11 bita (1 nit)", "spojen histogram, cifra 12 bita (1 nit)", "radix v0.1 (bez unsafe)", "std sort_by_key (stabilan)", "std sort_unstable"];
    let fns1: Vec<SortFn> = vec![
        Box::new(|v: &mut Pairs| { let mut s = Vec::new(); lens_radix::radix_sort_by_key(v, &mut s, |e| e.0) }),
        Box::new(|v: &mut Pairs| { let mut s = Vec::new(); lens_radix::radix_sort_by_key_wc::<_, _, 16>(v, &mut s, |e| e.0) }),
        Box::new(|v: &mut Pairs| { let mut s = Vec::new(); lens_radix::radix_sort_by_key_fused(v, &mut s, |e| e.0) }),
        Box::new(|v: &mut Pairs| { let mut s = Vec::new(); lens_radix::radix_sort_by_key_fused_bits(v, &mut s, 11, |e| e.0) }),
        Box::new(|v: &mut Pairs| { let mut s = Vec::new(); lens_radix::radix_sort_by_key_fused_bits(v, &mut s, 12, |e| e.0) }),
        Box::new(|v: &mut Pairs| radix_sort(v, true) as u32),
        Box::new(|v: &mut Pairs| { v.sort_by_key(|e| e.0); 0 }),
        Box::new(|v: &mut Pairs| { v.sort_unstable(); 0 }),
    ];
    out!(o, "\n## 3a. Sortiranje u memoriji, 1 nit — {n} parova = {:.1} MiB (alokacija pomoćnog bafera je u vremenu)", mib(n * 16));
    let r1 = bench(&pool1, &keyed, &fns1, reps, &reference);
    table(&mut o, &names1, &r1, 7);

    // 3b) N niti
    let pooln = rayon::ThreadPoolBuilder::new().num_threads(threads).build().unwrap();
    let names_n = ["LENS radix", "LENS radix + treći blok W=8", "LENS radix + treći blok W=16", "LENS radix + treći blok W=32", "LENS radix, spojen histogram v0.9", "spojen histogram, cifra 11 bita", "spojen histogram, cifra 12 bita", "rayon par_sort_by_key (stabilan)", "rayon par_sort_unstable"];
    let fns_n: Vec<SortFn> = vec![
        Box::new(|v: &mut Pairs| { let mut s = Vec::new(); lens_radix::radix_sort_by_key(v, &mut s, |e| e.0) }),
        Box::new(|v: &mut Pairs| { let mut s = Vec::new(); lens_radix::radix_sort_by_key_wc::<_, _, 8>(v, &mut s, |e| e.0) }),
        Box::new(|v: &mut Pairs| { let mut s = Vec::new(); lens_radix::radix_sort_by_key_wc::<_, _, 16>(v, &mut s, |e| e.0) }),
        Box::new(|v: &mut Pairs| { let mut s = Vec::new(); lens_radix::radix_sort_by_key_wc::<_, _, 32>(v, &mut s, |e| e.0) }),
        Box::new(|v: &mut Pairs| { let mut s = Vec::new(); lens_radix::radix_sort_by_key_fused(v, &mut s, |e| e.0) }),
        Box::new(|v: &mut Pairs| { let mut s = Vec::new(); lens_radix::radix_sort_by_key_fused_bits(v, &mut s, 11, |e| e.0) }),
        Box::new(|v: &mut Pairs| { let mut s = Vec::new(); lens_radix::radix_sort_by_key_fused_bits(v, &mut s, 12, |e| e.0) }),
        Box::new(|v: &mut Pairs| { v.par_sort_by_key(|e| e.0); 0 }),
        Box::new(|v: &mut Pairs| { v.par_sort_unstable(); 0 }),
    ];
    out!(o, "\n## 3b. Sortiranje u memoriji, {threads} niti");
    let rn = bench(&pooln, &keyed, &fns_n, reps, &reference);
    table(&mut o, &names_n, &rn, 8);

    // 4) SQLite :memory:
    out!(o, "\n## 4. SQLite CREATE INDEX nad istim parovima u :memory: bazi (sort + izgradnja B-tree u memoriji)");
    out!(o, "| konfiguracija | medijana ms | min | max | highwater MiB | isti redosled |");
    out!(o, "|---|---|---|---|---|---|");
    let mut sqlite_best = f64::MAX;
    for &(tuned, th) in &[(false, 0usize), (false, threads), (true, 0usize)] {
        let r = sqlite_mem(&raw, tuned, th, reps, root);
        let (md, mn, mx) = med(&r.times);
        sqlite_best = sqlite_best.min(md);
        let label = format!("{}, PRAGMA threads={th}", if tuned { "cache 2 GiB + temp_store=MEMORY" } else { "podrazumevano" });
        out!(o, "| {label} | {md:.1} | {mn:.1} | {mx:.1} | {:.1} | {} |", mib(r.highwater.max(0) as usize), if r.fp == fp_ref { "DA" } else { "NE" });
    }
    out!(o, "\n### Skaliranje na niti (1 nit → {threads} niti), isti algoritam");
    for (a, b, label) in [(0usize, 0usize, "LENS radix"), (1, 2, "LENS radix + treći blok W=16"), (2, 4, "LENS radix, spojen histogram v0.9"), (3, 5, "spojen histogram, cifra 11 bita"), (4, 6, "spojen histogram, cifra 12 bita")] {
        let (t1, _, _) = med(&r1[a].times);
        let (tn, _, _) = med(&rn[b].times);
        out!(o, "{label}: {t1:.1} ms → {tn:.1} ms = {:.2}× ubrzanje", t1 / tn);
    }
    out!(o, "(treći blok = mali lokalni bafer po bucket-u između izvora i odredišta; vidi D:\\sqliteRecog\\IDEJA_treci_blok.md)");
    {
        let (a, _, _) = med(&rn[0].times);
        let (b, _, _) = med(&rn[4].times);
        out!(o, "PROVERA PREDVIĐANJA (zapisano pre merenja): spojen histogram na {threads} niti {b:.1} ms naspram LENS radix {a:.1} ms; predviđeno ~45–55 ms ako je radix ograničen protokom memorije");
        let (c11, _, _) = med(&rn[5].times);
        let (c12, _, _) = med(&rn[6].times);
        let (s11, _, _) = med(&r1[3].times);
        out!(o, "PROVERA PREDVIĐANJA v0.10 (zapisano pre merenja): {threads} niti — 8 bita {b:.1} | 11 bita {c11:.1} (predviđeno 45–55) | 12 bita {c12:.1} (predviđeno: ne bolje od 11); 1 nit — 11 bita {s11:.1} ms (predviđeno 70–85)");
    }
    let (l1, _, _) = med(&r1[0].times);
    let (ln, _, _) = med(&rn[0].times);
    out!(o, "najbolji SQLite {sqlite_best:.1} ms naspram LENS radix {l1:.1} ms (1 nit) / {ln:.1} ms ({threads} niti)");
    out!(o, "NAPOMENA: SQLite pored sortiranja kodira zapise i gradi B-tree; radix samo uređuje niz. Odnos nije \"isti posao, brže\".");

    // 5) disk
    if disk {
        out!(o, "\n## 5. SQLite CREATE INDEX nad pravom bazom (skeniranje tabele + sort + B-tree + upis)");
        let (t, fp) = sqlite_disk(&db, disk_reps, root);
        let (md, mn, mx) = med(&t);
        out!(o, "CREATE INDEX: medijana {md:.0} ms (min {mn:.0}, max {mx:.0}), ponavljanja {disk_reps} | isti redosled: {}", if fp == fp_ref { "DA" } else { "NE" });
        if let Some(te) = t_extract {
            out!(o, "za orijentaciju: izvlačenje {te:.0} ms + LENS radix {ln:.0} ms = {:.0} ms (bez B-tree i upisa — nije ista količina posla)", te + ln);
        }
    } else {
        out!(o, "\n(disk test preskočen; pokreni sa --disk)");
    }
    save(&o);
}
