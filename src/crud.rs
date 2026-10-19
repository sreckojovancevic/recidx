//! Korak 4: izmene (CRUD) — Recognition Index u tri dinamičke varijante naspram
//! BTreeSet-a, heš mape i SQLite B-tree-a (u memoriji i na disku).
//!
//! Svi dobijaju ISTI deterministički niz operacija (Create / Delete / Update / Read).
//! Tačnost: (1) svako čitanje mora vratiti isto što i referentni model,
//!          (2) krajnje stanje indeksa mora imati isti otisak kao referentni model.
//!
//! Varijante Recognition Indexa (bucket funkcija Log(12) iz koraka 2):
//!   A — jedan sortiran niz + tabela početaka; svaka izmena odmah (pomeranje niza + ažuriranje tabele)
//!   B — niz po bucket-u; ključ prepozna svoj bucket, izmena samo u njemu
//!   C — statičan sortiran niz + tabela, izmene u baferu (dodati/obrisani);
//!       kad bafer dostigne prag: spajanje u novi niz + nova tabela bucket-a (brojanje + prefiksne sume)

use crate::find::{FxMap, Mode, RecogIndex, Rng};
use crate::lens_radix::radix_sort_by_key;
use crate::{cur, fingerprint, fnv, mib, ms, FNV0};
use rusqlite::{params, Connection};
use std::collections::BTreeSet;
use std::time::Instant;

const MODE: Mode = Mode::Log(12);

#[derive(Clone, Copy)]
pub enum Op { Ins(i64, u64), Del(i64, u64), Upd(i64, u64, u64), Read(u64) }

fn read_hash<I: Iterator<Item = i64>>(it: I) -> u64 {
    let mut h = FNV0;
    let mut c = 0u64;
    for r in it { fnv(&mut h, r as u64); c += 1; }
    fnv(&mut h, c);
    h
}

// ======================= opterećenje + referentni model =======================

struct Workload { ops: Vec<Op>, expect: Vec<u64>, counts: [usize; 4] }

fn generate(keyed: &[(u64, i64)], n_ops: usize) -> Workload {
    let mut rng = Rng(0xC0FFEE_u64 ^ 0x9E3779B97F4A7C15);
    let max_r = keyed.iter().map(|e| e.1).max().unwrap();
    let max_k = keyed.iter().map(|e| e.0).max().unwrap();
    let size = max_r as usize + n_ops + 2;
    let mut keyof = vec![u64::MAX; size];
    let mut pos = vec![u32::MAX; size];
    let mut live: Vec<i64> = Vec::with_capacity(keyed.len() + n_ops);
    for &(k, r) in keyed {
        keyof[r as usize] = k;
        pos[r as usize] = live.len() as u32;
        live.push(r);
    }
    let mut truth: BTreeSet<(u64, i64)> = keyed.iter().copied().collect();
    let mut next = max_r + 1;
    let mut ops = Vec::with_capacity(n_ops);
    let mut expect = vec![];
    let mut counts = [0usize; 4];
    fn sample(rng: &mut Rng, live: &[i64], keyof: &[u64]) -> u64 {
        keyof[live[rng.below(live.len() as u64) as usize] as usize]
    }
    for _ in 0..n_ops {
        match rng.below(100) {
            0..=29 => {
                let k = sample(&mut rng, &live, &keyof);
                let r = next; next += 1;
                keyof[r as usize] = k; pos[r as usize] = live.len() as u32; live.push(r);
                truth.insert((k, r));
                ops.push(Op::Ins(r, k)); counts[0] += 1;
            }
            30..=54 => {
                let i = rng.below(live.len() as u64) as usize;
                let r = live[i];
                let k = keyof[r as usize];
                let last = *live.last().unwrap();
                live.swap_remove(i);
                if last != r { pos[last as usize] = i as u32; }
                pos[r as usize] = u32::MAX; keyof[r as usize] = u64::MAX;
                truth.remove(&(k, r));
                ops.push(Op::Del(r, k)); counts[1] += 1;
            }
            55..=79 => {
                let r = live[rng.below(live.len() as u64) as usize];
                let old = keyof[r as usize];
                let new = sample(&mut rng, &live, &keyof);
                keyof[r as usize] = new;
                truth.remove(&(old, r)); truth.insert((new, r));
                ops.push(Op::Upd(r, old, new)); counts[2] += 1;
            }
            _ => {
                let k = if rng.below(2) == 0 { sample(&mut rng, &live, &keyof) } else { 1 + rng.below(max_k) };
                expect.push(read_hash(truth.range((k, i64::MIN)..=(k, i64::MAX)).map(|e| e.1)));
                ops.push(Op::Read(k)); counts[3] += 1;
            }
        }
    }
    Workload { ops, expect, counts }
}

/// Otisak stanja posle prvih `prefix` operacija (ponavljanje nad modelom; nije u vremenu).
fn truth_fp(keyed: &[(u64, i64)], ops: &[Op]) -> u64 {
    let mut set: FxMap<i64, u64> = FxMap::default();
    set.reserve(keyed.len() + ops.len());
    for &(k, r) in keyed { set.insert(r, k); }
    for op in ops {
        match *op {
            Op::Ins(r, k) => { set.insert(r, k); }
            Op::Del(r, _) => { set.remove(&r); }
            Op::Upd(r, _, n) => { set.insert(r, n); }
            Op::Read(_) => {}
        }
    }
    let mut v: Vec<(u64, i64)> = set.into_iter().map(|(r, k)| (k, r)).collect();
    v.sort_unstable();
    fingerprint(&v)
}

// ======================= zajednički interfejs =======================

trait Dyn {
    fn ins(&mut self, k: u64, r: i64);
    fn del(&mut self, k: u64, r: i64);
    fn upd(&mut self, r: i64, old: u64, new: u64) { self.del(old, r); self.ins(new, r); }
    fn find(&mut self, k: u64) -> u64;
    fn begin(&mut self) {}
    fn commit(&mut self) {}
    fn finish(&mut self) {}
    fn fp(&mut self) -> u64;
    fn note(&self) -> String { String::new() }
}

// ---------- A: jedan sortiran niz + tabela ----------
struct VarA { v: Vec<(u64, i64)>, start: Vec<u32> }
impl VarA {
    fn new(sorted: Vec<(u64, i64)>) -> Self { let idx = RecogIndex::build(&sorted, MODE); VarA { v: sorted, start: idx.start } }
    fn ensure(&mut self, b: usize) { if b + 1 >= self.start.len() { let n = self.v.len() as u32; self.start.resize(b + 2, n); } }
}
impl Dyn for VarA {
    fn ins(&mut self, k: u64, r: i64) {
        let b = RecogIndex::bucket(MODE, k);
        self.ensure(b);
        let (lo, hi) = (self.start[b] as usize, self.start[b + 1] as usize);
        let p = lo + self.v[lo..hi].partition_point(|e| *e < (k, r));
        self.v.insert(p, (k, r));
        for x in &mut self.start[b + 1..] { *x += 1; }
    }
    fn del(&mut self, k: u64, r: i64) {
        let b = RecogIndex::bucket(MODE, k);
        let (lo, hi) = (self.start[b] as usize, self.start[b + 1] as usize);
        let p = lo + self.v[lo..hi].partition_point(|e| *e < (k, r));
        assert_eq!(self.v[p], (k, r), "A: brisanje nepostojećeg para");
        self.v.remove(p);
        for x in &mut self.start[b + 1..] { *x -= 1; }
    }
    fn find(&mut self, k: u64) -> u64 {
        let b = RecogIndex::bucket(MODE, k);
        if b + 1 >= self.start.len() { return read_hash(std::iter::empty()); }
        let s = &self.v[self.start[b] as usize..self.start[b + 1] as usize];
        let i = s.partition_point(|e| e.0 < k);
        read_hash(s[i..].iter().take_while(|e| e.0 == k).map(|e| e.1))
    }
    fn fp(&mut self) -> u64 { fingerprint(&self.v) }
}

// ---------- B: niz po bucket-u + Fenwick (crate::dynidx) ----------
struct VarB(crate::dynidx::DynIndex);
impl VarB { fn new(sorted: &[(u64, i64)]) -> Self { VarB(crate::dynidx::DynIndex::new(sorted)) } }
impl Dyn for VarB {
    fn ins(&mut self, k: u64, r: i64) { self.0.ins(k, r); }
    fn del(&mut self, k: u64, r: i64) { self.0.del(k, r); }
    fn find(&mut self, k: u64) -> u64 { let mut h = FNV0; let c = self.0.find(k, &mut h); fnv(&mut h, c as u64); h }
    fn fp(&mut self) -> u64 { self.0.fingerprint() }
}

// ---------- C: statičan niz + bafer + ponovna izgradnja ----------
struct VarC { main: Vec<(u64, i64)>, idx: RecogIndex, add: BTreeSet<(u64, i64)>, del: BTreeSet<(u64, i64)>, thr: usize, rebuilds: usize, rebuild_ms: f64 }
impl VarC {
    fn new(sorted: Vec<(u64, i64)>, thr: usize) -> Self {
        let idx = RecogIndex::build(&sorted, MODE);
        VarC { main: sorted, idx, add: BTreeSet::new(), del: BTreeSet::new(), thr, rebuilds: 0, rebuild_ms: 0.0 }
    }
    fn rebuild(&mut self) {
        let t = Instant::now();
        let mut out = Vec::with_capacity(self.main.len() + self.add.len());
        let mut di = self.del.iter().peekable();
        let mut ai = self.add.iter().peekable();
        for &e in &self.main {
            if let Some(&&d) = di.peek() { if d == e { di.next(); continue; } }
            while let Some(&&a) = ai.peek() { if a < e { out.push(a); ai.next(); } else { break; } }
            out.push(e);
        }
        assert!(di.next().is_none(), "C: obrisan par koji nije u glavnom nizu");
        out.extend(ai.copied());
        self.idx = RecogIndex::build(&out, MODE);
        self.main = out;
        self.add.clear();
        self.del.clear();
        self.rebuilds += 1;
        self.rebuild_ms += ms(t.elapsed());
    }
    fn maybe(&mut self) { if self.add.len() + self.del.len() >= self.thr { self.rebuild(); } }
}
impl Dyn for VarC {
    fn ins(&mut self, k: u64, r: i64) { if !self.del.remove(&(k, r)) { self.add.insert((k, r)); } self.maybe(); }
    fn del(&mut self, k: u64, r: i64) { if !self.add.remove(&(k, r)) { self.del.insert((k, r)); } self.maybe(); }
    fn find(&mut self, k: u64) -> u64 {
        let (lo, hi) = (self.idx.pos(&self.main, k, false), self.idx.pos(&self.main, k, true));
        let mut v: Vec<i64> = if self.del.is_empty() {
            self.main[lo..hi].iter().map(|e| e.1).collect()
        } else {
            self.main[lo..hi].iter().filter(|e| !self.del.contains(e)).map(|e| e.1).collect()
        };
        let before = v.len();
        v.extend(self.add.range((k, i64::MIN)..=(k, i64::MAX)).map(|e| e.1));
        if v.len() != before { v.sort_unstable(); }
        read_hash(v.into_iter())
    }
    fn finish(&mut self) { if !self.add.is_empty() || !self.del.is_empty() { self.rebuild(); } }
    fn fp(&mut self) -> u64 { fingerprint(&self.main) }
    fn note(&self) -> String { format!("prag {}, ponovnih izgradnji {}, ukupno {:.0} ms", self.thr, self.rebuilds, self.rebuild_ms) }
}

// ---------- BTreeSet ----------
struct Bt(BTreeSet<(u64, i64)>);
impl Dyn for Bt {
    fn ins(&mut self, k: u64, r: i64) { self.0.insert((k, r)); }
    fn del(&mut self, k: u64, r: i64) { assert!(self.0.remove(&(k, r))); }
    fn find(&mut self, k: u64) -> u64 { read_hash(self.0.range((k, i64::MIN)..=(k, i64::MAX)).map(|e| e.1)) }
    fn fp(&mut self) -> u64 { let mut h = FNV0; for &(k, r) in &self.0 { fnv(&mut h, k); fnv(&mut h, r as u64); } h }
}

// ---------- heš mapa ----------
struct Hs(FxMap<u64, Vec<i64>>);
impl Dyn for Hs {
    fn ins(&mut self, k: u64, r: i64) { let v = self.0.entry(k).or_default(); let p = v.partition_point(|&x| x < r); v.insert(p, r); }
    fn del(&mut self, k: u64, r: i64) {
        let v = self.0.get_mut(&k).expect("Hs: nema ključa");
        let p = v.partition_point(|&x| x < r);
        assert_eq!(v[p], r);
        v.remove(p);
        if v.is_empty() { self.0.remove(&k); }
    }
    fn find(&mut self, k: u64) -> u64 { match self.0.get(&k) { Some(v) => read_hash(v.iter().copied()), None => read_hash(std::iter::empty()) } }
    fn fp(&mut self) -> u64 {
        let mut ks: Vec<u64> = self.0.keys().copied().collect();
        ks.sort_unstable();
        let mut h = FNV0;
        for k in ks { for &r in &self.0[&k] { fnv(&mut h, k); fnv(&mut h, r as u64); } }
        h
    }
}

// ---------- SQLite ----------
struct Sq { c: Connection, root: i64, ins: String, del: String, upd: String, sel: String, sel_null: String, all: String }
impl Sq {
    fn new(c: Connection, root: i64, t: &str, i: &str) -> Self {
        Sq {
            c, root,
            ins: format!("INSERT INTO {t}(rowid,k) VALUES(?1,?2)"),
            del: format!("DELETE FROM {t} WHERE rowid=?1"),
            upd: format!("UPDATE {t} SET k=?2 WHERE rowid=?1"),
            sel: format!("SELECT rowid FROM {t} INDEXED BY {i} WHERE k=?1"),
            sel_null: format!("SELECT rowid FROM {t} INDEXED BY {i} WHERE k IS NULL AND ?1 IS NULL"),
            all: format!("SELECT k, rowid FROM {t} INDEXED BY {i}"),
        }
    }
    fn val(&self, k: u64) -> Option<i64> { if k == 0 { None } else { Some(((k - 1) as i128 + self.root as i128) as i64) } }
}
impl Dyn for Sq {
    fn ins(&mut self, k: u64, r: i64) { let v = self.val(k); self.c.prepare_cached(&self.ins).unwrap().execute(params![r, v]).unwrap(); }
    fn del(&mut self, _k: u64, r: i64) { let n = self.c.prepare_cached(&self.del).unwrap().execute(params![r]).unwrap(); assert_eq!(n, 1); }
    fn upd(&mut self, r: i64, _old: u64, new: u64) { let v = self.val(new); let n = self.c.prepare_cached(&self.upd).unwrap().execute(params![r, v]).unwrap(); assert_eq!(n, 1); }
    fn find(&mut self, k: u64) -> u64 {
        let v = self.val(k);
        let sql = if k == 0 { &self.sel_null } else { &self.sel };
        let mut st = self.c.prepare_cached(sql).unwrap();
        let mut rows = st.query(params![v]).unwrap();
        let mut h = FNV0; let mut c = 0u64;
        while let Some(row) = rows.next().unwrap() { let id: i64 = row.get(0).unwrap(); fnv(&mut h, id as u64); c += 1; }
        fnv(&mut h, c);
        h
    }
    fn begin(&mut self) { self.c.execute_batch("BEGIN").unwrap(); }
    fn commit(&mut self) { self.c.execute_batch("COMMIT").unwrap(); }
    fn fp(&mut self) -> u64 {
        let root = self.root;
        let mut st = self.c.prepare(&self.all).unwrap();
        let mut rows = st.query([]).unwrap();
        let mut h = FNV0;
        while let Some(row) = rows.next().unwrap() {
            let v: Option<i64> = row.get(0).unwrap();
            let id: i64 = row.get(1).unwrap();
            fnv(&mut h, crate::key(v, root)); fnv(&mut h, id as u64);
        }
        h
    }
}

// ======================= pokretanje =======================

#[derive(Clone, Copy, PartialEq)]
enum Kind { A, B, C, Bt, Hs, SqMem, SqDisk, SqDiskOff }
impl Kind {
    fn name(self) -> &'static str {
        match self {
            Kind::A => "Recognition A — jedan niz, izmena odmah",
            Kind::B => "Recognition B — niz po bucket-u + Fenwick",
            Kind::C => "Recognition C — bafer + ponovna izgradnja",
            Kind::Bt => "Rust BTreeSet (B-tree u memoriji)",
            Kind::Hs => "heš mapa (FxHash), bez opsega",
            Kind::SqMem => "SQLite :memory: (tabela + 1 indeks)",
            Kind::SqDisk => "SQLite disk, synchronous=FULL (sa fsync)",
            Kind::SqDiskOff => "SQLite disk, synchronous=OFF (bez fsync)",
        }
    }
}

fn build(kind: Kind, sorted: &[(u64, i64)], raw: &[(Option<i64>, i64)], root: i64, thr: usize, db: &str) -> Box<dyn Dyn> {
    match kind {
        Kind::A => Box::new(VarA::new(sorted.to_vec())),
        Kind::B => Box::new(VarB::new(sorted)),
        Kind::C => Box::new(VarC::new(sorted.to_vec(), thr)),
        Kind::Bt => Box::new(Bt(sorted.iter().copied().collect())),
        Kind::Hs => {
            let mut m: FxMap<u64, Vec<i64>> = FxMap::default();
            for &(k, r) in sorted { m.entry(k).or_default().push(r); }
            Box::new(Hs(m))
        }
        Kind::SqMem => {
            let c = Connection::open_in_memory().unwrap();
            c.execute_batch("PRAGMA journal_mode=MEMORY; PRAGMA synchronous=OFF; CREATE TABLE t(k INTEGER);").unwrap();
            c.execute_batch("BEGIN").unwrap();
            { let mut st = c.prepare("INSERT INTO t(rowid,k) VALUES(?1,?2)").unwrap(); for &(s, id) in raw { st.execute(params![id, s]).unwrap(); } }
            c.execute_batch("COMMIT; CREATE INDEX i ON t(k);").unwrap();
            Box::new(Sq::new(c, root, "t", "i"))
        }
        Kind::SqDisk | Kind::SqDiskOff => {
            let c = Connection::open(db).unwrap();
            if kind == Kind::SqDiskOff { c.execute_batch("PRAGMA synchronous=OFF;").unwrap(); }
            c.execute_batch("DROP TABLE IF EXISTS recidx_t; CREATE TABLE recidx_t(k INTEGER);
                             INSERT INTO recidx_t(rowid,k) SELECT rowid, size_bytes FROM backup_index;
                             CREATE INDEX recidx_t_i ON recidx_t(k);").unwrap();
            Box::new(Sq::new(c, root, "recidx_t", "recidx_t_i"))
        }
    }
}

fn cap(kind: Kind, bs: usize, n: usize, a_cap: usize, disk1_cap: usize) -> usize {
    match kind {
        Kind::A => n.min(a_cap),
        Kind::SqDisk if bs == 1 => n.min(disk1_cap),
        Kind::SqDiskOff if bs == 1 => n.min(disk1_cap * 5),
        _ => n,
    }
}

pub fn step4(o: &mut String, raw: &[(Option<i64>, i64)], keyed: &[(u64, i64)], root: i64,
             n_ops: usize, thr: usize, disk: bool, db: &str) {
    const A_CAP: usize = 3_000;
    const DISK1_CAP: usize = 2_000;
    out!(o, "\n## KORAK 4 — izmene (CRUD), sve u 1 niti, bucket funkcija Log(12)");
    let pool1 = rayon::ThreadPoolBuilder::new().num_threads(1).build().unwrap();
    let mut sorted = keyed.to_vec();
    pool1.install(|| { let mut s = Vec::new(); radix_sort_by_key(&mut sorted, &mut s, |e| e.0); });

    let t = Instant::now();
    let w = generate(keyed, n_ops);
    out!(o, "operacija: {n_ops} (Create {} | Delete {} | Update {} | Read {}) | generisanje + referentni model: {:.0} ms",
        w.counts[0], w.counts[1], w.counts[2], w.counts[3], ms(t.elapsed()));
    out!(o, "ograničenja broja operacija: A najviše {A_CAP} (svaka izmena pomera ceo niz); SQLite disk sa serijom 1 najviše {DISK1_CAP} uz FULL (svaka operacija = commit sa fsync) i {} uz OFF", DISK1_CAP * 5);
    out!(o, "SQLite disk: posebna tabela recidx_t(k) sa jednim indeksom u istom fajlu (fer prema strukturama sa jednim indeksom)");
    out!(o, "SQLite disk u dve varijante: synchronous=FULL (podrazumevano; commit čeka fsync = trajnost) i synchronous=OFF (isti upis, bez čekanja fsync-a)");
    out!(o, "Rust strukture nemaju upis na disk ni trajnost; najbliže im je SQLite :memory: i SQLite disk OFF");

    let mut fps: Vec<(usize, u64)> = vec![];
    let mut truth = |len: usize| -> u64 {
        if let Some(&(_, f)) = fps.iter().find(|e| e.0 == len) { return f; }
        let f = truth_fp(keyed, &w.ops[..len]);
        fps.push((len, f));
        f
    };

    let mut kinds = vec![Kind::A, Kind::B, Kind::C, Kind::Bt, Kind::Hs, Kind::SqMem];
    if disk { kinds.push(Kind::SqDisk); kinds.push(Kind::SqDiskOff); }

    for &bs in &[1usize, 1_000, 100_000] {
        let bs = bs.min(n_ops.max(1));
        out!(o, "\n### Serija od {bs} operacija (commit po seriji; za Rust strukture serija nema efekta osim kod C na kraju)");
        out!(o, "| struktura | izvršeno operacija | ukupno ms | µs po operaciji | priprema ms | memorija MiB | čitanja tačna | krajnje stanje = model | napomena |");
        out!(o, "|---|---|---|---|---|---|---|---|---|");
        for &kind in &kinds {
            let n = cap(kind, bs, n_ops, A_CAP, DISK1_CAP);
            let ops = &w.ops[..n];
            let reads_n = ops.iter().filter(|op| matches!(op, Op::Read(_))).count();
            let base = cur();
            let tb = Instant::now();
            let mut s = build(kind, &sorted, raw, root, thr, db);
            let prep = ms(tb.elapsed());
            let mem = if matches!(kind, Kind::SqMem | Kind::SqDisk | Kind::SqDiskOff) { "—".to_string() } else { format!("{:.1}", mib(cur().saturating_sub(base))) };

            let t = Instant::now();
            let mut ri = 0;
            let mut ok_reads = true;
            for (i, op) in ops.iter().enumerate() {
                if i % bs == 0 { s.begin(); }
                match *op {
                    Op::Ins(r, k) => s.ins(k, r),
                    Op::Del(r, k) => s.del(k, r),
                    Op::Upd(r, a, b) => s.upd(r, a, b),
                    Op::Read(k) => { if s.find(k) != w.expect[ri] { ok_reads = false; } ri += 1; }
                }
                if (i + 1) % bs == 0 || i + 1 == n { s.commit(); }
            }
            s.finish();
            let el = ms(t.elapsed());
            debug_assert_eq!(ri, reads_n);
            let ok_fp = s.fp() == truth(n);
            out!(o, "| {} | {n} | {el:.1} | {:.2} | {prep:.0} | {mem} | {} | {} | {} |",
                kind.name(), el * 1000.0 / n as f64, if ok_reads { "DA" } else { "NE" }, if ok_fp { "DA" } else { "NE" }, s.note());
            drop(s);
            if matches!(kind, Kind::SqDisk | Kind::SqDiskOff) {
                let c = Connection::open(db).unwrap();
                c.execute_batch("DROP TABLE IF EXISTS recidx_t;").unwrap();
            }
        }
    }
    out!(o, "\nNAPOMENE:");
    out!(o, "- µs po operaciji = ukupno vreme / broj operacija (mešavina Create/Delete/Update/Read; Update = brisanje + ubacivanje u indeksu).");
    out!(o, "- priprema = izgradnja početne strukture od svih redova (nije u ukupnom vremenu); memorija = alokator, posle pripreme.");
    out!(o, "- C uključuje i završnu ponovnu izgradnju (ispražnjen bafer) u ukupnom vremenu.");
    out!(o, "- SQLite vremena uključuju SQL mašineriju; SQLite disk FULL uključuje commit + fsync po seriji, OFF samo commit bez čekanja fsync-a.");
}
