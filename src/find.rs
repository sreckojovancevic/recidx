//! Korak 2: izgradnja i pretraga (find) — Recognition Index naspram drugih struktura.
//!
//! Recognition Index = sortiran niz (ključ, rowid) dobijen LENS radixom
//!                   + tabela početaka bucket-a dobijena brojanjem i prefiksnim sumama
//!                     (isti korak kao u svakom radix prolazu).
//! Pretraga: ključ sam izračuna svoj bucket (bez poređenja), pa binarna pretraga
//! samo unutar tog bucket-a.
//!
//! Dve funkcije bucket-a:
//!   Linear(16): gornjih 16 bitova opsega ključeva — klasičan radix bucket.
//!   Log(S): broj bitova ključa + sledećih S bitova (logaritamska skala, monotona).

use crate::lens_radix::radix_sort_by_key;
use crate::{cur, fnv, key, med, mib, ms, FNV0};
use rusqlite::{params, Connection};
use std::collections::{BTreeSet, HashMap};
use std::hash::{BuildHasherDefault, Hasher};
use std::time::Instant;

// ---------- brz heš (FxHash stil), da hash kontrola ne bude oslabljena SipHash-om ----------
#[derive(Default)]
pub(crate) struct FxHasher(u64);
impl Hasher for FxHasher {
    fn finish(&self) -> u64 { self.0 }
    fn write(&mut self, b: &[u8]) { for &x in b { self.write_u64(x as u64); } }
    fn write_u64(&mut self, x: u64) { self.0 = (self.0.rotate_left(5) ^ x).wrapping_mul(0x517cc1b727220a95); }
}
pub(crate) type FxMap<K, V> = HashMap<K, V, BuildHasherDefault<FxHasher>>;

pub(crate) struct Rng(pub(crate) u64);
impl Rng {
    pub(crate) fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12; x ^= x << 25; x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545F4914F6CDD1D)
    }
    pub(crate) fn below(&mut self, n: u64) -> u64 { self.next() % n.max(1) }
}

// ---------- Recognition Index ----------
#[derive(Clone, Copy)]
pub enum Mode { Linear(u32), Log(u32) }

pub struct RecogIndex { pub(crate) mode: Mode, pub(crate) start: Vec<u32> }

impl RecogIndex {
    #[inline]
    pub(crate) fn bucket(mode: Mode, k: u64) -> usize {
        match mode {
            Mode::Linear(sh) => (k >> sh) as usize,
            Mode::Log(s) => {
                let l = 64 - k.leading_zeros();
                if l <= s + 1 { k as usize } else {
                    let sh = l - 1 - s;
                    ((sh as usize) << s) + (k >> sh) as usize
                }
            }
        }
    }
    /// `sorted` mora biti uređen po ključu. Jedan prolaz brojanja + prefiksne sume.
    pub fn build(sorted: &[(u64, i64)], mode: Mode) -> Self {
        assert!(sorted.len() < u32::MAX as usize);
        let max = sorted.last().map(|e| e.0).unwrap_or(0);
        let nb = Self::bucket(mode, max) + 1;
        let mut start = vec![0u32; nb + 1];
        for &(k, _) in sorted { start[Self::bucket(mode, k) + 1] += 1; }
        for i in 1..=nb { start[i] += start[i - 1]; }
        RecogIndex { mode, start }
    }
    #[inline]
    pub fn find(&self, data: &[(u64, i64)], k: u64, h: &mut u64) -> usize {
        let b = Self::bucket(self.mode, k);
        if b + 1 >= self.start.len() { return 0; }
        let s = &data[self.start[b] as usize..self.start[b + 1] as usize];
        let i = s.partition_point(|e| e.0 < k);
        let mut c = 0;
        for e in &s[i..] { if e.0 != k { break; } fnv(h, e.1 as u64); c += 1; }
        c
    }
    /// Prva pozicija sa ključem >= k (inclusive=false) ili > k (inclusive=true).
    /// Ispravno jer je bucket funkcija monotona: raniji bucket-i imaju manje ključeve.
    #[inline]
    pub fn pos(&self, data: &[(u64, i64)], k: u64, inclusive: bool) -> usize {
        let b = Self::bucket(self.mode, k);
        if b + 1 >= self.start.len() { return data.len(); }
        let lo = self.start[b] as usize;
        let s = &data[lo..self.start[b + 1] as usize];
        lo + if inclusive { s.partition_point(|e| e.0 <= k) } else { s.partition_point(|e| e.0 < k) }
    }
    /// (broj bucket-a, nepraznih, najveći bucket, prosek nepraznih)
    pub fn stats(&self) -> (usize, usize, u32, f64) {
        let nb = self.start.len() - 1;
        let sizes: Vec<u32> = (0..nb).map(|i| self.start[i + 1] - self.start[i]).collect();
        let ne = sizes.iter().filter(|&&x| x > 0).count();
        let mx = sizes.iter().copied().max().unwrap_or(0);
        let tot: u64 = sizes.iter().map(|&x| x as u64).sum();
        (nb, ne, mx, if ne > 0 { tot as f64 / ne as f64 } else { 0.0 })
    }
    pub fn bytes(&self) -> usize { self.start.len() * 4 }
}

fn find_binary(data: &[(u64, i64)], k: u64, h: &mut u64) -> usize {
    let i = data.partition_point(|e| e.0 < k);
    let mut c = 0;
    for e in &data[i..] { if e.0 != k { break; } fnv(h, e.1 as u64); c += 1; }
    c
}

/// Upiti: 1/2 vrednost nasumičnog reda (težinski po učestalosti), 1/4 nasumična različita
/// vrednost, 1/4 nasumičan broj u [min, max] (uglavnom nepostojeći).
pub(crate) fn make_queries(raw: &[(Option<i64>, i64)], sorted: &[(u64, i64)], root: i64, q: usize) -> Vec<i64> {
    let mut rng = Rng(0x9E3779B97F4A7C15 ^ 42);
    let mut distinct: Vec<u64> = sorted.iter().map(|e| e.0).filter(|&k| k != 0).collect();
    distinct.dedup();
    let maxv = sorted.last().map(|e| e.0).unwrap_or(1) as i64 - 1 + root;
    let mut out = Vec::with_capacity(q);
    while out.len() < q {
        match out.len() % 4 {
            0 | 1 => { if let Some(v) = raw[rng.below(raw.len() as u64) as usize].0 { out.push(v); } }
            2 => { let k = distinct[rng.below(distinct.len() as u64) as usize]; out.push((k - 1) as i64 + root); }
            _ => out.push(root + rng.below((maxv - root + 1) as u64) as i64),
        }
    }
    out
}

struct Row { name: String, build: Vec<f64>, mem: f64, q: Vec<f64>, fp: u64, rows: usize }

pub fn step2(o: &mut String, raw: &[(Option<i64>, i64)], keyed: &[(u64, i64)], root: i64,
             reps: usize, nq: usize, disk: bool, db: &str) {
    let pool1 = rayon::ThreadPoolBuilder::new().num_threads(1).build().unwrap();
    let n = keyed.len();
    out!(o, "\n## KORAK 2 — izgradnja i pretraga `WHERE size_bytes = ?` (sve u 1 niti)");

    // --- izgradnja: sort (LENS radix) je zajednički za sortirane strukture; meri se svaki put ---
    let mut sort_t = vec![];
    let mut sorted: Vec<(u64, i64)> = vec![];
    for _ in 0..reps {
        let mut v = keyed.to_vec();
        let t = Instant::now();
        pool1.install(|| { let mut s = Vec::new(); radix_sort_by_key(&mut v, &mut s, |e| e.0); });
        sort_t.push(ms(t.elapsed()));
        sorted = v;
    }
    let max_key = sorted.last().unwrap().0;
    let lin_sh = (64 - max_key.leading_zeros()).saturating_sub(16);
    let modes = [("Recognition Index, Log(10)", Mode::Log(10)), ("Recognition Index, Log(12)", Mode::Log(12)), ("Recognition Index, Linear(16 bita)", Mode::Linear(lin_sh))];

    let queries = make_queries(raw, &sorted, root, nq);
    let qkeys: Vec<u64> = queries.iter().map(|&v| key(Some(v), root)).collect();
    out!(o, "parova: {n} | upita: {nq} (½ postojeće vrednosti po učestalosti, ¼ nasumične različite, ¼ nasumične u opsegu) | ponavljanja: {reps}");

    let mut rows: Vec<Row> = vec![];

    // --- Recognition Index varijante ---
    for (name, mode) in modes {
        let mut bt = vec![];
        let mut idx = RecogIndex::build(&sorted, mode);
        for (i, _) in (0..reps).enumerate() {
            let t = Instant::now();
            idx = RecogIndex::build(&sorted, mode);
            bt.push(sort_t[i] + ms(t.elapsed()));
        }
        let (nb, ne, mx, avg) = idx.stats();
        out!(o, "{name}: bucket-a {nb}, nepraznih {ne}, najveći {mx}, prosek nepraznih {avg:.1} | tabela {:.2} MiB", mib(idx.bytes()));
        let mut qt = vec![]; let mut fp = 0; let mut tot = 0;
        for _ in 0..reps {
            let t = Instant::now();
            let mut h = FNV0; let mut c = 0;
            for &k in &qkeys { let r = idx.find(&sorted, k, &mut h); fnv(&mut h, r as u64); c += r; }
            qt.push(ms(t.elapsed())); fp = h; tot = c;
        }
        rows.push(Row { name: name.into(), build: bt, mem: mib(n * 16 + idx.bytes()), q: qt, fp, rows: tot });
    }

    // --- dinamički Recognition Index (B + Fenwick) ---
    {
        let mut bt = vec![]; let mut mem = 0usize;
        let mut d = crate::dynidx::DynIndex::new(&sorted);
        for i in 0..reps {
            drop(d);
            let base = cur();
            let t = Instant::now();
            d = crate::dynidx::DynIndex::new(&sorted);
            bt.push(sort_t[i] + ms(t.elapsed()));
            mem = cur() - base;
        }
        let mut qt = vec![]; let mut fp = 0; let mut tot = 0;
        for _ in 0..reps {
            let t = Instant::now();
            let mut h = FNV0; let mut c = 0;
            for &k in &qkeys { let r = d.find(k, &mut h); fnv(&mut h, r as u64); c += r; }
            qt.push(ms(t.elapsed())); fp = h; tot = c;
        }
        rows.push(Row { name: "Recognition B dinamički (bucket nizovi + Fenwick)".into(), build: bt, mem: mib(mem), q: qt, fp, rows: tot });
    }

    // --- sortiran niz + binarna pretraga po celom nizu ---
    {
        let mut qt = vec![]; let mut fp = 0; let mut tot = 0;
        for _ in 0..reps {
            let t = Instant::now();
            let mut h = FNV0; let mut c = 0;
            for &k in &qkeys { let r = find_binary(&sorted, k, &mut h); fnv(&mut h, r as u64); c += r; }
            qt.push(ms(t.elapsed())); fp = h; tot = c;
        }
        rows.push(Row { name: "sortiran niz + binarna pretraga".into(), build: sort_t.clone(), mem: mib(n * 16), q: qt, fp, rows: tot });
    }

    // --- heš mapa ključ -> rowid-ovi (kontrola za jednakost), gradi se iz ulaza u redosledu rowid-a ---
    {
        let mut bt = vec![]; let mut mem = 0usize;
        let mut map: FxMap<u64, Vec<i64>> = FxMap::default();
        for _ in 0..reps {
            drop(std::mem::take(&mut map));
            let base = cur();
            let t = Instant::now();
            let mut m: FxMap<u64, Vec<i64>> = FxMap::default();
            for &(k, r) in keyed { m.entry(k).or_default().push(r); }
            bt.push(ms(t.elapsed()));
            mem = cur() - base;
            map = m;
        }
        let mut qt = vec![]; let mut fp = 0; let mut tot = 0;
        for _ in 0..reps {
            let t = Instant::now();
            let mut h = FNV0; let mut c = 0;
            for &k in &qkeys {
                let r = match map.get(&k) { Some(v) => { for &x in v { fnv(&mut h, x as u64); } v.len() } None => 0 };
                fnv(&mut h, r as u64); c += r;
            }
            qt.push(ms(t.elapsed())); fp = h; tot = c;
        }
        rows.push(Row { name: "heš mapa (FxHash) ključ → rowid-ovi".into(), build: bt, mem: mib(mem), q: qt, fp, rows: tot });
    }

    // --- Rust BTreeSet<(ključ, rowid)> — B-tree u memoriji, gradi se iz ulaza u redosledu rowid-a ---
    {
        let mut bt = vec![]; let mut mem = 0usize;
        let mut set: BTreeSet<(u64, i64)> = BTreeSet::new();
        for _ in 0..reps {
            drop(std::mem::take(&mut set));
            let base = cur();
            let t = Instant::now();
            let st: BTreeSet<(u64, i64)> = keyed.iter().copied().collect();
            bt.push(ms(t.elapsed()));
            mem = cur() - base;
            set = st;
        }
        let mut qt = vec![]; let mut fp = 0; let mut tot = 0;
        for _ in 0..reps {
            let t = Instant::now();
            let mut h = FNV0; let mut c = 0;
            for &k in &qkeys {
                let mut r = 0usize;
                for e in set.range((k, i64::MIN)..=(k, i64::MAX)) { fnv(&mut h, e.1 as u64); r += 1; }
                fnv(&mut h, r as u64); c += r;
            }
            qt.push(ms(t.elapsed())); fp = h; tot = c;
        }
        rows.push(Row { name: "Rust BTreeSet (B-tree u memoriji)".into(), build: bt, mem: mib(mem), q: qt, fp, rows: tot });
    }

    // --- SQLite :memory: B-tree ---
    {
        let m = Connection::open_in_memory().unwrap();
        m.execute_batch("PRAGMA journal_mode=OFF; PRAGMA synchronous=OFF; CREATE TABLE t(k INTEGER);").unwrap();
        m.execute_batch("BEGIN").unwrap();
        { let mut st = m.prepare("INSERT INTO t(rowid,k) VALUES(?1,?2)").unwrap(); for &(s, id) in raw { st.execute(params![id, s]).unwrap(); } }
        m.execute_batch("COMMIT").unwrap();
        let mut bt = vec![]; let mut mem = 0i64;
        let pages = |m: &Connection| -> i64 { m.query_row("PRAGMA page_count", [], |r| r.get(0)).unwrap() };
        let psz: i64 = m.query_row("PRAGMA page_size", [], |r| r.get(0)).unwrap();
        for r in 0..reps {
            let base = pages(&m);
            let t = Instant::now();
            m.execute_batch("CREATE INDEX i ON t(k)").unwrap();
            bt.push(ms(t.elapsed()));
            if r == 0 { mem = (pages(&m) - base) * psz; } // posle DROP INDEX stranice se ponovo koriste
            if r + 1 < reps { m.execute_batch("DROP INDEX i").unwrap(); }
        }
        let (qt, fp, tot) = sqlite_queries(&m, "SELECT rowid FROM t INDEXED BY i WHERE k = ?1", &queries, reps);
        rows.push(Row { name: "SQLite B-tree, :memory: (upit kroz SQL; memorija = stranice indeksa)".into(), build: bt, mem: mib(mem.max(0) as usize), q: qt, fp, rows: tot });
    }

    // --- SQLite na disku ---
    if disk {
        let c = Connection::open(db).unwrap();
        c.execute_batch("DROP INDEX IF EXISTS idx_recidx_size").unwrap();
        let t = Instant::now();
        c.execute_batch("CREATE INDEX idx_recidx_size ON backup_index(size_bytes)").unwrap();
        let bt = vec![ms(t.elapsed())];
        let (qt, fp, tot) = sqlite_queries(&c, "SELECT rowid FROM backup_index INDEXED BY idx_recidx_size WHERE size_bytes = ?1", &queries, reps);
        c.execute_batch("DROP INDEX idx_recidx_size").unwrap();
        rows.push(Row { name: "SQLite B-tree, baza na disku (upit kroz SQL)".into(), build: bt, mem: f64::NAN, q: qt, fp, rows: tot });
    }

    // --- tabela ---
    let fp_ref = rows.iter().find(|r| r.name.starts_with("sortiran")).unwrap().fp;
    out!(o, "\n| struktura | izgradnja ms (medijana) | memorija MiB | {nq} upita ms (medijana) | ns po upitu | vraćeno redova | isti rezultat |");
    out!(o, "|---|---|---|---|---|---|---|");
    for r in &rows {
        let (b, _, _) = med(&r.build);
        let (q, _, _) = med(&r.q);
        let memtxt = if r.mem.is_nan() { "na disku".to_string() } else { format!("{:.1}", r.mem) };
        out!(o, "| {} | {b:.1} | {memtxt} | {q:.1} | {:.0} | {} | {} |", r.name, q * 1e6 / nq as f64, r.rows, if r.fp == fp_ref { "DA" } else { "NE" });
    }
    out!(o, "\nNAPOMENE:");
    out!(o, "- izgradnja Recognition Indexa = LENS radix sort + tabela bucket-a; sortiranog niza = samo sort; heš i BTreeSet = ubacivanje iz ulaza.");
    out!(o, "- memorija sortiranih struktura uključuje niz parova (16 B po redu); heš je izmeren alokatorom.");
    out!(o, "- SQLite upit uključuje SQL mašineriju (bind, VDBE, korak po red) — to je stvarna cena korišćenja kroz SQLite;");
    out!(o, "  Rust strukture bi kroz virtualnu tabelu takođe dobile deo te cene. Poređenje čiste strukture je među Rust redovima.");
    out!(o, "- heš podržava samo jednakost; sortirane strukture i B-tree podržavaju i opsege (nije mereno u ovom koraku).");
}

fn sqlite_queries(c: &Connection, sql: &str, queries: &[i64], reps: usize) -> (Vec<f64>, u64, usize) {
    let mut st = c.prepare(sql).unwrap();
    let mut qt = vec![]; let mut fp = 0; let mut tot = 0;
    for _ in 0..reps {
        let t = Instant::now();
        let mut h = FNV0; let mut c = 0;
        for &v in queries {
            let mut rows = st.query(params![v]).unwrap();
            let mut r = 0usize;
            while let Some(row) = rows.next().unwrap() { let id: i64 = row.get(0).unwrap(); fnv(&mut h, id as u64); r += 1; }
            fnv(&mut h, r as u64); c += r;
        }
        qt.push(ms(t.elapsed())); fp = h; tot = c;
    }
    (qt, fp, tot)
}

// ======================= KORAK 3: opsezi (BETWEEN) =======================

const CLASSES: [(&str, f64); 3] = [("0,1%", 0.001), ("1%", 0.01), ("10%", 0.1)];

/// Opseg [a, a + max(1, a·w)], a iz istog generatora kao kod jednakosti.
fn make_ranges(raw: &[(Option<i64>, i64)], sorted: &[(u64, i64)], root: i64, per_class: usize) -> Vec<Vec<(i64, i64)>> {
    let starts = make_queries(raw, sorted, root, per_class);
    CLASSES.iter().map(|&(_, w)| starts.iter().map(|&a| (a, a + ((a as f64 * w) as i64).max(1))).collect()).collect()
}

struct RRow { name: String, count: Vec<f64>, fetch: Vec<f64>, fp_c: Vec<u64>, fp_f: Vec<u64> }

fn run_sorted<P: Fn(u64, bool) -> usize>(sorted: &[(u64, i64)], ranges: &[(i64, i64)], root: i64, reps: usize, pos: P) -> (f64, f64, u64, u64) {
    let qk: Vec<(u64, u64)> = ranges.iter().map(|&(a, b)| (key(Some(a), root), key(Some(b), root))).collect();
    let (mut tc, mut tf) = (vec![], vec![]);
    let (mut fc, mut ff) = (0, 0);
    for _ in 0..reps {
        let t = Instant::now();
        let mut h = FNV0;
        for &(a, b) in &qk { let c = pos(b, true) - pos(a, false); fnv(&mut h, c as u64); }
        tc.push(ms(t.elapsed())); fc = h;
        let t = Instant::now();
        let mut h = FNV0;
        for &(a, b) in &qk {
            let (lo, hi) = (pos(a, false), pos(b, true));
            for e in &sorted[lo..hi] { fnv(&mut h, e.1 as u64); }
            fnv(&mut h, (hi - lo) as u64);
        }
        tf.push(ms(t.elapsed())); ff = h;
    }
    (med(&tc).0, med(&tf).0, fc, ff)
}

fn run_sqlite(c: &Connection, cnt_sql: &str, fetch_sql: &str, ranges: &[(i64, i64)], reps: usize) -> (f64, f64, u64, u64) {
    let mut sc = c.prepare(cnt_sql).unwrap();
    let mut sf = c.prepare(fetch_sql).unwrap();
    let (mut tc, mut tf) = (vec![], vec![]);
    let (mut fc, mut ff) = (0, 0);
    for _ in 0..reps {
        let t = Instant::now();
        let mut h = FNV0;
        for &(a, b) in ranges { let n: i64 = sc.query_row(params![a, b], |r| r.get(0)).unwrap(); fnv(&mut h, n as u64); }
        tc.push(ms(t.elapsed())); fc = h;
        let t = Instant::now();
        let mut h = FNV0;
        for &(a, b) in ranges {
            let mut rows = sf.query(params![a, b]).unwrap();
            let mut n = 0u64;
            while let Some(r) = rows.next().unwrap() { let id: i64 = r.get(0).unwrap(); fnv(&mut h, id as u64); n += 1; }
            fnv(&mut h, n);
        }
        tf.push(ms(t.elapsed())); ff = h;
    }
    (med(&tc).0, med(&tf).0, fc, ff)
}

pub fn step3(o: &mut String, raw: &[(Option<i64>, i64)], keyed: &[(u64, i64)], root: i64,
             reps: usize, per_class: usize, disk: bool, db: &str) {
    let pool1 = rayon::ThreadPoolBuilder::new().num_threads(1).build().unwrap();
    out!(o, "\n## KORAK 3 — opsezi `WHERE size_bytes BETWEEN a AND b` (sve u 1 niti)");
    let mut sorted = keyed.to_vec();
    pool1.install(|| { let mut s = Vec::new(); radix_sort_by_key(&mut sorted, &mut s, |e| e.0); });
    let ranges = make_ranges(raw, &sorted, root, per_class);
    let n = sorted.len();
    out!(o, "opsega po klasi: {per_class} | klase širine: b = a + a·w, w ∈ {{0,1%, 1%, 10%}} | ponavljanja: {reps}");
    out!(o, "COUNT = samo broj pogodaka; FETCH = svi rowid-ovi iz opsega (redosled ključ, rowid)");

    let mut rows: Vec<RRow> = vec![];
    let mut push = |name: &str, f: &dyn Fn(&[(i64, i64)]) -> (f64, f64, u64, u64)| {
        let mut r = RRow { name: name.into(), count: vec![], fetch: vec![], fp_c: vec![], fp_f: vec![] };
        for cl in &ranges { let (c, fe, a, b) = f(cl); r.count.push(c); r.fetch.push(fe); r.fp_c.push(a); r.fp_f.push(b); }
        rows.push(r);
    };
    for (name, mode) in [("Recognition Index, Log(12)", Mode::Log(12)), ("Recognition Index, Log(10)", Mode::Log(10))] {
        let idx = RecogIndex::build(&sorted, mode);
        push(name, &|cl| run_sorted(&sorted, cl, root, reps, |k, inc| idx.pos(&sorted, k, inc)));
    }
    {
        let d = crate::dynidx::DynIndex::new(&sorted);
        push("Recognition B dinamički (bucket nizovi + Fenwick)", &|cl| {
            let qk: Vec<(u64, u64)> = cl.iter().map(|&(a, b)| (key(Some(a), root), key(Some(b), root))).collect();
            let (mut tc, mut tf) = (vec![], vec![]);
            let (mut fc, mut ff) = (0, 0);
            for _ in 0..reps {
                let t = Instant::now();
                let mut h = FNV0;
                for &(a, b) in &qk { fnv(&mut h, d.count_range(a, b) as u64); }
                tc.push(ms(t.elapsed())); fc = h;
                let t = Instant::now();
                let mut h = FNV0;
                for &(a, b) in &qk { let c = d.fetch_range(a, b, &mut h); fnv(&mut h, c as u64); }
                tf.push(ms(t.elapsed())); ff = h;
            }
            (med(&tc).0, med(&tf).0, fc, ff)
        });
    }
    push("sortiran niz + binarna pretraga", &|cl| run_sorted(&sorted, cl, root, reps, |k, inc| {
        if inc { sorted.partition_point(|e| e.0 <= k) } else { sorted.partition_point(|e| e.0 < k) }
    }));

    {
        let set: BTreeSet<(u64, i64)> = keyed.iter().copied().collect();
        push("Rust BTreeSet (B-tree u memoriji)", &|cl| {
            let qk: Vec<(u64, u64)> = cl.iter().map(|&(a, b)| (key(Some(a), root), key(Some(b), root))).collect();
            let (mut tc, mut tf) = (vec![], vec![]);
            let (mut fc, mut ff) = (0, 0);
            for _ in 0..reps {
                let t = Instant::now();
                let mut h = FNV0;
                for &(a, b) in &qk { let c = set.range((a, i64::MIN)..=(b, i64::MAX)).count(); fnv(&mut h, c as u64); }
                tc.push(ms(t.elapsed())); fc = h;
                let t = Instant::now();
                let mut h = FNV0;
                for &(a, b) in &qk {
                    let mut c = 0u64;
                    for e in set.range((a, i64::MIN)..=(b, i64::MAX)) { fnv(&mut h, e.1 as u64); c += 1; }
                    fnv(&mut h, c);
                }
                tf.push(ms(t.elapsed())); ff = h;
            }
            (med(&tc).0, med(&tf).0, fc, ff)
        });
    }

    let m = Connection::open_in_memory().unwrap();
    m.execute_batch("PRAGMA journal_mode=OFF; PRAGMA synchronous=OFF; CREATE TABLE t(k INTEGER);").unwrap();
    m.execute_batch("BEGIN").unwrap();
    { let mut st = m.prepare("INSERT INTO t(rowid,k) VALUES(?1,?2)").unwrap(); for &(s, id) in raw { st.execute(params![id, s]).unwrap(); } }
    m.execute_batch("COMMIT; CREATE INDEX i ON t(k);").unwrap();
    push("SQLite B-tree, :memory: (kroz SQL)", &|cl| run_sqlite(&m,
        "SELECT count(*) FROM t INDEXED BY i WHERE k BETWEEN ?1 AND ?2",
        "SELECT rowid FROM t INDEXED BY i WHERE k BETWEEN ?1 AND ?2", cl, reps));
    drop(m);
    if disk {
        let c = Connection::open(db).unwrap();
        c.execute_batch("DROP INDEX IF EXISTS idx_recidx_size; CREATE INDEX idx_recidx_size ON backup_index(size_bytes);").unwrap();
        push("SQLite B-tree, disk (kroz SQL)", &|cl| run_sqlite(&c,
            "SELECT count(*) FROM backup_index INDEXED BY idx_recidx_size WHERE size_bytes BETWEEN ?1 AND ?2",
            "SELECT rowid FROM backup_index INDEXED BY idx_recidx_size WHERE size_bytes BETWEEN ?1 AND ?2", cl, reps));
        c.execute_batch("DROP INDEX idx_recidx_size").unwrap();
    }

    // prosečan broj pogodaka po klasi (iz sortiranog niza)
    let avg: Vec<f64> = ranges.iter().map(|cl| {
        let tot: usize = cl.iter().map(|&(a, b)| {
            let (ka, kb) = (key(Some(a), root), key(Some(b), root));
            sorted.partition_point(|e| e.0 <= kb) - sorted.partition_point(|e| e.0 < ka)
        }).sum();
        tot as f64 / cl.len() as f64
    }).collect();
    out!(o, "parova: {n} | prosečno pogodaka po opsegu: 0,1% → {:.0} | 1% → {:.0} | 10% → {:.0}", avg[0], avg[1], avg[2]);

    let reference = rows.iter().find(|r| r.name.starts_with("sortiran")).map(|r| (r.fp_c.clone(), r.fp_f.clone())).unwrap();
    for (label, which) in [("COUNT — ns po upitu", 0), ("FETCH — ns po upitu", 1)] {
        out!(o, "\n### {label}");
        out!(o, "| struktura | 0,1% | 1% | 10% | isti rezultat |");
        out!(o, "|---|---|---|---|---|");
        for r in &rows {
            let t = if which == 0 { &r.count } else { &r.fetch };
            let ok = if which == 0 { r.fp_c == reference.0 } else { r.fp_f == reference.1 };
            out!(o, "| {} | {:.0} | {:.0} | {:.0} | {} |", r.name,
                t[0] * 1e6 / per_class as f64, t[1] * 1e6 / per_class as f64, t[2] * 1e6 / per_class as f64, if ok { "DA" } else { "NE" });
        }
    }
    out!(o, "\nNAPOMENE:");
    out!(o, "- heš mapa nije u tabeli: ne podržava opsege bez punog prolaska kroz sve ključeve.");
    out!(o, "- COUNT kod sortiranih nizova = dve pretrage granica; kod B dinamičkog = dve pretrage + Fenwick zbir celih bucket-a; BTreeSet i SQLite broje prolaskom kroz opseg.");
    out!(o, "- SQLite vremena uključuju SQL mašineriju; nije čisto poređenje struktura.");
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn bucket_monotone() {
        for &m in &[Mode::Log(10), Mode::Log(12), Mode::Log(4), Mode::Linear(20)] {
            let mut prev = 0usize;
            let mut k = 0u64;
            while k < (1u64 << 40) {
                let b = RecogIndex::bucket(m, k);
                assert!(b >= prev, "nije monotono na {k}");
                prev = b;
                k = if k < 100_000 { k + 1 } else { k + k / 997 + 1 };
            }
        }
    }
    #[test]
    fn find_matches_binary() {
        let mut rng = Rng(7);
        let mut v: Vec<(u64, i64)> = (0..300_000).map(|i| ((rng.next() % 5000) * (rng.next() % 4000) + 1, i)).collect();
        v.sort_by_key(|e| e.0);
        for m in [Mode::Log(10), Mode::Log(3), Mode::Linear(8)] {
            let idx = RecogIndex::build(&v, m);
            for q in 0..30_000u64 {
                let k = q * 677;
                let (mut a, mut b) = (FNV0, FNV0);
                assert_eq!(idx.find(&v, k, &mut a), find_binary(&v, k, &mut b));
                assert_eq!(a, b);
                for inc in [false, true] {
                    let want = if inc { v.partition_point(|e| e.0 <= k) } else { v.partition_point(|e| e.0 < k) };
                    assert_eq!(idx.pos(&v, k, inc), want);
                }
            }
        }
    }
}
