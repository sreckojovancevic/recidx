//! Matrični test (v0.12b): sve varijante × svi skupovi × sve veličine × svi odnosi izmena.
//!
//! Plan i kriterijumi: D:\sqliteRecog\ARI_KRITERIJUMI_I_PREDVIDJANJA.md (zapisano pre merenja).
//! Svaki rezultat se odmah upisuje u CSV (results/matrix_<ts>.csv); `--resume FAJL` preskače
//! već završene kombinacije. Na kraju se pravi izveštaj sa ocenom kriterijuma i predviđanja.

use crate::ari::{Ari, AriCfg, Counts, Method, Trigger};
use crate::dynidx::DynIndex;
use crate::find::{FxMap, Mode, Rng};
use crate::lens_radix::radix_sort_by_key;
use crate::posting::Posting;
use crate::{cur, fingerprint, fnv, med, mib, ms, FNV0};
use std::collections::{BTreeSet, HashSet};
use std::io::Write;
use std::time::Instant;

// ======================= zajednički interfejs =======================

trait Idx {
    fn ins(&mut self, k: u64, r: i64);
    fn del(&mut self, k: u64, r: i64);
    fn upd(&mut self, r: i64, old: u64, new: u64) { self.del(old, r); self.ins(new, r); }
    fn find(&mut self, k: u64, h: &mut u64) -> usize;
    fn count(&self, a: u64, b: u64) -> usize;
    fn fetch(&self, a: u64, b: u64, h: &mut u64) -> usize;
    fn fp(&self) -> u64;
    fn check(&self) -> Result<(), String> { Ok(()) }
    fn extra(&self) -> Vec<(String, f64)> { vec![] }
}

impl Idx for DynIndex {
    fn ins(&mut self, k: u64, r: i64) { DynIndex::ins(self, k, r) }
    fn del(&mut self, k: u64, r: i64) { DynIndex::del(self, k, r) }
    fn find(&mut self, k: u64, h: &mut u64) -> usize { DynIndex::find(self, k, h) }
    fn count(&self, a: u64, b: u64) -> usize { self.count_range(a, b) }
    fn fetch(&self, a: u64, b: u64, h: &mut u64) -> usize { self.fetch_range(a, b, h) }
    fn fp(&self) -> u64 { self.fingerprint() }
    fn check(&self) -> Result<(), String> { DynIndex::check(self) }
    fn extra(&self) -> Vec<(String, f64)> { vec![("max_bucket".into(), self.max_bucket() as f64)] }
}

impl Idx for Ari {
    fn ins(&mut self, k: u64, r: i64) { Ari::ins(self, k, r) }
    fn del(&mut self, k: u64, r: i64) { Ari::del(self, k, r) }
    fn find(&mut self, k: u64, h: &mut u64) -> usize { Ari::find(self, k, h) }
    fn count(&self, a: u64, b: u64) -> usize { self.count_range(a, b) }
    fn fetch(&self, a: u64, b: u64, h: &mut u64) -> usize { self.fetch_range(a, b, h) }
    fn fp(&self) -> u64 { self.fingerprint() }
    fn check(&self) -> Result<(), String> { Ari::check(self) }
    fn extra(&self) -> Vec<(String, f64)> {
        let mut p = self.st.pauses_ns.clone();
        p.sort_unstable();
        let pct = |q: f64| if p.is_empty() { 0.0 } else { p[((p.len() - 1) as f64 * q) as usize] as f64 / 1000.0 };
        vec![
            ("refines".into(), self.st.refines as f64),
            ("build_refines".into(), self.st.build_refines as f64),
            ("merges".into(), self.st.merges as f64),
            ("moved".into(), self.st.moved as f64),
            ("pause_total_ms".into(), p.iter().sum::<u64>() as f64 / 1e6),
            ("pause_max_us".into(), p.last().copied().unwrap_or(0) as f64 / 1000.0),
            ("pause_p99_us".into(), pct(0.99)),
            ("flips".into(), self.st.max_flips as f64),
            ("locality_viol".into(), self.st.locality_violations as f64),
            ("depth".into(), self.depth() as f64),
            ("max_leaf".into(), self.max_leaf() as f64),
            ("refined_nodes".into(), self.refined_nodes() as f64),
        ]
    }
}

/// D: globalna ponovna izgradnja — kad deljiv bucket pređe prag, ceo indeks se gradi sa Log(S+1).
struct Global { d: DynIndex, s: u32, high: usize, rebuilds: u64, pauses: Vec<u64> }
impl Global {
    fn new(sorted: &[(u64, i64)], high: usize) -> Self {
        let mut s = 12;
        let mut d = DynIndex::with_mode(sorted, Mode::Log(s));
        while d.max_splittable() > high && s < 16 { s += 1; d = DynIndex::with_mode(sorted, Mode::Log(s)); }
        Global { d, s, high, rebuilds: 0, pauses: vec![] }
    }
}
impl Idx for Global {
    fn ins(&mut self, k: u64, r: i64) {
        self.d.ins(k, r);
        let (len, split) = self.d.bucket_info(k);
        if len > self.high && split && self.s < 16 {
            let t = Instant::now();
            let v = self.d.to_sorted();
            self.s += 1;
            self.d = DynIndex::with_mode(&v, Mode::Log(self.s));
            self.rebuilds += 1;
            self.pauses.push(t.elapsed().as_nanos() as u64);
        }
    }
    fn del(&mut self, k: u64, r: i64) { self.d.del(k, r) }
    fn find(&mut self, k: u64, h: &mut u64) -> usize { self.d.find(k, h) }
    fn count(&self, a: u64, b: u64) -> usize { self.d.count_range(a, b) }
    fn fetch(&self, a: u64, b: u64, h: &mut u64) -> usize { self.d.fetch_range(a, b, h) }
    fn fp(&self) -> u64 { self.d.fingerprint() }
    fn check(&self) -> Result<(), String> { self.d.check() }
    fn extra(&self) -> Vec<(String, f64)> {
        vec![
            ("rebuilds".into(), self.rebuilds as f64),
            ("log_s".into(), self.s as f64),
            ("pause_max_us".into(), self.pauses.iter().copied().max().unwrap_or(0) as f64 / 1000.0),
            ("pause_total_ms".into(), self.pauses.iter().sum::<u64>() as f64 / 1e6),
        ]
    }
}

struct Bt(BTreeSet<(u64, i64)>);
impl Idx for Bt {
    fn ins(&mut self, k: u64, r: i64) { self.0.insert((k, r)); }
    fn del(&mut self, k: u64, r: i64) { assert!(self.0.remove(&(k, r))); }
    fn find(&mut self, k: u64, h: &mut u64) -> usize { let mut c = 0; for e in self.0.range((k, i64::MIN)..=(k, i64::MAX)) { fnv(h, e.1 as u64); c += 1; } c }
    fn count(&self, a: u64, b: u64) -> usize { if a > b { 0 } else { self.0.range((a, i64::MIN)..=(b, i64::MAX)).count() } }
    fn fetch(&self, a: u64, b: u64, h: &mut u64) -> usize { if a > b { return 0; } let mut c = 0; for e in self.0.range((a, i64::MIN)..=(b, i64::MAX)) { fnv(h, e.1 as u64); c += 1; } c }
    fn fp(&self) -> u64 { let mut h = FNV0; for &(k, r) in &self.0 { fnv(&mut h, k); fnv(&mut h, r as u64); } h }
}

struct Sv(Vec<(u64, i64)>);
impl Idx for Sv {
    fn ins(&mut self, k: u64, r: i64) { let p = self.0.partition_point(|e| *e < (k, r)); self.0.insert(p, (k, r)); }
    fn del(&mut self, k: u64, r: i64) { let p = self.0.partition_point(|e| *e < (k, r)); assert!(self.0[p] == (k, r)); self.0.remove(p); }
    fn find(&mut self, k: u64, h: &mut u64) -> usize { let i = self.0.partition_point(|e| e.0 < k); let mut c = 0; for e in &self.0[i..] { if e.0 != k { break; } fnv(h, e.1 as u64); c += 1; } c }
    fn count(&self, a: u64, b: u64) -> usize { if a > b { 0 } else { self.0.partition_point(|e| e.0 <= b) - self.0.partition_point(|e| e.0 < a) } }
    fn fetch(&self, a: u64, b: u64, h: &mut u64) -> usize { if a > b { return 0; } let (l, u) = (self.0.partition_point(|e| e.0 < a), self.0.partition_point(|e| e.0 <= b)); for e in &self.0[l..u] { fnv(h, e.1 as u64); } u - l }
    fn fp(&self) -> u64 { fingerprint(&self.0) }
}

impl Idx for Posting {
    fn ins(&mut self, k: u64, r: i64) { Posting::ins(self, k, r) }
    fn del(&mut self, k: u64, r: i64) { Posting::del(self, k, r) }
    fn find(&mut self, k: u64, h: &mut u64) -> usize { Posting::find(self, k, h) }
    fn count(&self, a: u64, b: u64) -> usize { self.count_range(a, b) }
    fn fetch(&self, a: u64, b: u64, h: &mut u64) -> usize { self.fetch_range(a, b, h) }
    fn fp(&self) -> u64 { self.fingerprint() }
    fn check(&self) -> Result<(), String> { Posting::check(self) }
}

// ======================= varijante =======================

#[derive(Clone, Copy)]
enum V { A, B, C(AriCfg), D, E, F, G }

fn variants() -> Vec<(String, V)> {
    let mut v: Vec<(String, V)> = vec![("A_Log12".into(), V::A), ("B_Log14".into(), V::B)];
    for h in [1024usize, 4096, 16384, 65536] { v.push((format!("C_ARI_{h}"), V::C(AriCfg::new(h)))); }
    let mut e = AriCfg::new(4096); e.trigger = Trigger::Effective; v.push(("C_ARI_4096_efektivni".into(), V::C(e)));
    let mut s = AriCfg::new(4096); s.method = Method::Sort; v.push(("C_ARI_4096_lokalni_sort".into(), V::C(s)));
    let mut f = AriCfg::new(4096); f.counts = Counts::Fenwick; v.push(("C_ARI_4096_fenwick".into(), V::C(f)));
    v.push(("D_globalna_izgradnja".into(), V::D));
    v.push(("E_BTreeSet".into(), V::E));
    v.push(("F_sortiran_niz".into(), V::F));
    v.push(("G_posting".into(), V::G));
    v
}

fn build(v: V, sorted: &[(u64, i64)]) -> Box<dyn Idx> {
    match v {
        V::A => Box::new(DynIndex::with_mode(sorted, Mode::Log(12))),
        V::B => Box::new(DynIndex::with_mode(sorted, Mode::Log(14))),
        V::C(cfg) => Box::new(Ari::new(sorted, cfg)),
        V::D => Box::new(Global::new(sorted, 4096)),
        V::E => Box::new(Bt(sorted.iter().copied().collect())),
        V::F => Box::new(Sv(sorted.to_vec())),
        V::G => Box::new(Posting::new(sorted)),
    }
}

// ======================= skupovi podataka =======================

const DATASETS: [&str; 5] = ["ravnomeran", "grupisan", "duplikati", "promenljiv", "m365"];

/// generator novih ključeva za dati skup (za promenljiv: NOVI grozdovi = pomeranje raspodele)
struct Gen { kind: &'static str, centers: Vec<u64>, drift: Vec<u64>, dups: Vec<u64>, existing: Vec<u64> }
impl Gen {
    fn new(kind: &'static str, seed: u64, m365: &[(u64, i64)]) -> Self {
        let mut r = Rng(seed ^ 0xA5A5_5A5A_1234_4321);
        let centers = (0..64).map(|_| (1u64 << 28) + r.below((1u64 << 33) - (1u64 << 28))).collect();
        // v0.12b: „promenljiv“ — 8 NOVIH uskih grozdova (širina 2^14) u koje idu umetanja i izmene,
        // tako da se pojedini bucket-i stvarno prepune tokom rada (v0.12 je imao preširoke grozdove).
        let drift = (0..8).map(|_| (1u64 << 29) + r.below((1u64 << 33) - (1u64 << 29))).collect();
        let dups = (0..100).map(|_| 1 + r.below(1u64 << 34)).collect();
        let existing = if kind == "m365" { m365.iter().map(|e| e.0).collect() } else { vec![] };
        Gen { kind, centers, drift, dups, existing }
    }
    fn uniform(r: &mut Rng) -> u64 { 1 + r.below(1u64 << 34) }
    fn clustered(&self, r: &mut Rng) -> u64 {
        if r.below(10) < 9 { self.centers[r.below(64) as usize] + r.below(1 << 20) } else { Self::uniform(r) }
    }
    /// ključ za početni skup
    fn initial(&self, r: &mut Rng) -> u64 {
        match self.kind {
            "ravnomeran" | "promenljiv" => Self::uniform(r),
            "grupisan" => self.clustered(r),
            "duplikati" => if r.below(2) == 0 { self.dups[r.below(100) as usize] } else { Self::uniform(r) },
            _ => self.existing[r.below(self.existing.len() as u64) as usize],
        }
    }
    /// ključ za umetanje / izmenu tokom CRUD-a
    fn new_key(&self, r: &mut Rng) -> u64 {
        match self.kind { "promenljiv" => self.drift[r.below(8) as usize] + r.below(1 << 14), _ => self.initial(r) }
    }
}

fn dataset(kind: &'static str, n: usize, m365: &[(u64, i64)]) -> (Vec<(u64, i64)>, Gen) {
    let g = Gen::new(kind, 1 + n as u64, m365);
    let v: Vec<(u64, i64)> = if kind == "m365" {
        if n >= m365.len() { m365.to_vec() } else { (0..n).map(|i| m365[i * m365.len() / n]).collect() }
    } else {
        let mut r = Rng(0x5EED ^ (n as u64) ^ (kind.len() as u64 * 7919));
        (0..n).map(|i| (g.initial(&mut r), i as i64 + 1)).collect()
    };
    (v, g)
}

// ======================= opterećenja =======================

#[derive(Clone, Copy)]
enum Op { Ins(i64, u64), Del(i64, u64), Upd(i64, u64, u64), Read(u64) }

const MIXES: [(&str, [u64; 4]); 3] = [("umetanje", [60, 15, 15, 10]), ("uravnotezeno", [30, 25, 25, 20]), ("brisanje", [10, 50, 20, 20])];

fn read_hash<I: Iterator<Item = i64>>(it: I) -> u64 { let mut h = FNV0; let mut c = 0u64; for r in it { fnv(&mut h, r as u64); c += 1; } fnv(&mut h, c); h }

struct Work { ops: Vec<Op>, expect: Vec<u64>, truth_full: u64 }

fn workload(sorted: &[(u64, i64)], g: &Gen, mix: [u64; 4], n_ops: usize, seed: u64) -> Work {
    let mut rng = Rng(seed);
    let max_r = sorted.iter().map(|e| e.1).max().unwrap_or(0);
    let max_k = sorted.iter().map(|e| e.0).max().unwrap_or(1);
    let size = max_r as usize + n_ops + 2;
    let mut keyof = vec![u64::MAX; size];
    let mut pos = vec![u32::MAX; size];
    let mut live: Vec<i64> = Vec::with_capacity(sorted.len() + n_ops);
    for &(k, r) in sorted { keyof[r as usize] = k; pos[r as usize] = live.len() as u32; live.push(r); }
    let mut truth: BTreeSet<(u64, i64)> = sorted.iter().copied().collect();
    let mut next = max_r + 1;
    let (mut ops, mut expect) = (Vec::with_capacity(n_ops), vec![]);
    let (ci, cd, cu) = (mix[0], mix[0] + mix[1], mix[0] + mix[1] + mix[2]);
    for _ in 0..n_ops {
        let x = rng.below(100);
        if x < ci || live.len() < 2 {
            let k = g.new_key(&mut rng);
            let r = next; next += 1;
            keyof[r as usize] = k; pos[r as usize] = live.len() as u32; live.push(r);
            truth.insert((k, r));
            ops.push(Op::Ins(r, k));
        } else if x < cd {
            let i = rng.below(live.len() as u64) as usize;
            let r = live[i]; let k = keyof[r as usize];
            let last = *live.last().unwrap();
            live.swap_remove(i);
            if last != r { pos[last as usize] = i as u32; }
            keyof[r as usize] = u64::MAX;
            truth.remove(&(k, r));
            ops.push(Op::Del(r, k));
        } else if x < cu {
            let r = live[rng.below(live.len() as u64) as usize];
            let old = keyof[r as usize];
            let new = g.new_key(&mut rng);
            keyof[r as usize] = new;
            truth.remove(&(old, r)); truth.insert((new, r));
            ops.push(Op::Upd(r, old, new));
        } else {
            let k = if rng.below(2) == 0 && !live.is_empty() { keyof[live[rng.below(live.len() as u64) as usize] as usize] } else { 1 + rng.below(max_k) };
            expect.push(read_hash(truth.range((k, i64::MIN)..=(k, i64::MAX)).map(|e| e.1)));
            ops.push(Op::Read(k));
        }
    }
    let mut h = FNV0;
    for &(k, r) in &truth { fnv(&mut h, k); fnv(&mut h, r as u64); }
    Work { ops, expect, truth_full: h }
}

fn truth_prefix(sorted: &[(u64, i64)], ops: &[Op]) -> u64 {
    let mut m: FxMap<i64, u64> = FxMap::default();
    m.reserve(sorted.len() + ops.len());
    for &(k, r) in sorted { m.insert(r, k); }
    for op in ops { match *op { Op::Ins(r, k) => { m.insert(r, k); } Op::Del(r, _) => { m.remove(&r); } Op::Upd(r, _, n) => { m.insert(r, n); } Op::Read(_) => {} } }
    let mut v: Vec<(u64, i64)> = m.into_iter().map(|(r, k)| (k, r)).collect();
    v.sort_unstable();
    fingerprint(&v)
}

// ======================= upiti i referentni odgovori =======================

const RANGE_W: [f64; 5] = [0.0001, 0.001, 0.01, 0.1, 0.5];
const RANGE_LBL: [&str; 5] = ["0.01pct", "0.1pct", "1pct", "10pct", "50pct"];

struct Queries { eq: Vec<u64>, ranges: Vec<Vec<(u64, u64)>> }

fn queries(sorted: &[(u64, i64)], nq: usize, nr: usize) -> Queries {
    let mut rng = Rng(0x0BEE_F00D);
    let max_k = sorted.last().map(|e| e.0).unwrap_or(1);
    let mut distinct: Vec<u64> = sorted.iter().map(|e| e.0).collect();
    distinct.dedup();
    let pick = |rng: &mut Rng, i: usize| -> u64 {
        match i % 4 {
            0 | 1 => sorted[rng.below(sorted.len() as u64) as usize].0,
            2 => distinct[rng.below(distinct.len() as u64) as usize],
            _ => 1 + rng.below(max_k),
        }
    };
    let eq: Vec<u64> = (0..nq).map(|i| pick(&mut rng, i)).collect();
    let starts: Vec<u64> = (0..nr).map(|i| pick(&mut rng, i)).collect();
    let ranges = RANGE_W.iter().map(|&w| starts.iter().map(|&a| (a, a.saturating_add(((a as f64 * w) as u64).max(1)))).collect()).collect();
    Queries { eq, ranges }
}

// ======================= CSV i nastavak =======================

struct Csv { f: std::fs::File, pub rows: Vec<(usize, String, String, String, f64)> }
impl Csv {
    fn put(&mut self, n: usize, ds: &str, var: &str, metric: &str, v: f64) {
        let _ = writeln!(self.f, "{n},{ds},{var},{metric},{v}");
        let _ = self.f.flush();
        self.rows.push((n, ds.into(), var.into(), metric.into(), v));
    }
    fn get(&self, n: usize, ds: &str, var: &str, metric: &str) -> Option<f64> {
        self.rows.iter().rev().find(|r| r.0 == n && r.1 == ds && r.2 == var && r.3 == metric).map(|r| r.4)
    }
}

fn pctl(v: &mut [u64], q: f64) -> f64 { if v.is_empty() { return 0.0; } v.sort_unstable(); v[((v.len() - 1) as f64 * q).round() as usize] as f64 }

// ======================= pokretanje jedne kombinacije =======================

#[allow(clippy::too_many_arguments)]
fn run_one(csv: &mut Csv, n: usize, ds: &str, vname: &str, v: V, sorted: &[(u64, i64)], q: &Queries,
           refs: &(u64, Vec<u64>, Vec<u64>), works: &[(String, Work)], reps: usize, prefix_fp: &mut FxMap<(usize, usize), u64>) {
    let mut ok_all = true;
    // --- izgradnja ---
    let mut bt = vec![];
    let mut mem = 0usize;
    let mut slot: Option<Box<dyn Idx>> = None;
    for rep in 0..reps {
        drop(slot.take());
        let base = cur();
        let t = Instant::now();
        let i = build(v, sorted);
        bt.push(ms(t.elapsed()));
        if rep == 0 { mem = cur().saturating_sub(base); }
        slot = Some(i);
    }
    let mut idx = slot.unwrap();
    csv.put(n, ds, vname, "build_ms", med(&bt).0);
    csv.put(n, ds, vname, "mem_mib", mib(mem));
    let ok_b = idx.check().is_ok();
    ok_all &= ok_b;
    csv.put(n, ds, vname, "ok_invarijante_izgradnja", ok_b as u8 as f64);
    for (k, val) in idx.extra() { csv.put(n, ds, vname, &format!("{k}_izgradnja"), val); }

    // --- jednakost ---
    let mut et = vec![]; let mut efp = 0;
    for _ in 0..reps {
        let t = Instant::now(); let mut h = FNV0;
        for &k in &q.eq { let c = idx.find(k, &mut h); fnv(&mut h, c as u64); }
        et.push(ms(t.elapsed())); efp = h;
    }
    csv.put(n, ds, vname, "eq_ns", med(&et).0 * 1e6 / q.eq.len() as f64);
    let ok_e = efp == refs.0; ok_all &= ok_e;
    csv.put(n, ds, vname, "ok_eq", ok_e as u8 as f64);
    {
        let mut lat = Vec::with_capacity(q.eq.len());
        let mut h = FNV0;
        for &k in &q.eq { let t = Instant::now(); idx.find(k, &mut h); lat.push(t.elapsed().as_nanos() as u64); }
        for (lbl, p) in [("eq_p50", 0.5), ("eq_p95", 0.95), ("eq_p99", 0.99)] { csv.put(n, ds, vname, lbl, pctl(&mut lat, p)); }
    }

    // --- opsezi ---
    for (ci, cl) in q.ranges.iter().enumerate() {
        let mut ct = vec![]; let mut cfp = 0;
        for _ in 0..reps { let t = Instant::now(); let mut h = FNV0; for &(a, b) in cl { fnv(&mut h, idx.count(a, b) as u64); } ct.push(ms(t.elapsed())); cfp = h; }
        csv.put(n, ds, vname, &format!("count_ns_{}", RANGE_LBL[ci]), med(&ct).0 * 1e6 / cl.len() as f64);
        let ok_c = cfp == refs.1[ci]; ok_all &= ok_c;
        csv.put(n, ds, vname, &format!("ok_count_{}", RANGE_LBL[ci]), ok_c as u8 as f64);
        if ci < 4 {
            let mut ft = vec![]; let mut ffp = 0;
            for _ in 0..reps { let t = Instant::now(); let mut h = FNV0; for &(a, b) in cl { let c = idx.fetch(a, b, &mut h); fnv(&mut h, c as u64); } ft.push(ms(t.elapsed())); ffp = h; }
            csv.put(n, ds, vname, &format!("fetch_ns_{}", RANGE_LBL[ci]), med(&ft).0 * 1e6 / cl.len() as f64);
            let ok_f = ffp == refs.2[ci]; ok_all &= ok_f;
            csv.put(n, ds, vname, &format!("ok_fetch_{}", RANGE_LBL[ci]), ok_f as u8 as f64);
        }
    }
    drop(idx);

    // --- izmene (CRUD) ---
    for (wi, (mname, w)) in works.iter().enumerate() {
        let cap = if matches!(v, V::F) && n > 100_000 { w.ops.len().min(3000) } else { w.ops.len() };
        let ops = &w.ops[..cap];
        let tfp = if cap == w.ops.len() { w.truth_full } else {
            *prefix_fp.entry((wi, cap)).or_insert_with(|| truth_prefix(sorted, ops))
        };
        let mut tt = vec![];
        let mut ok_r = true; let mut ok_f = true; let mut ok_i = true;
        let mut extra = vec![];
        for rep in 0..reps {
            let mut s = build(v, sorted);
            let t = Instant::now();
            let mut ri = 0;
            for op in ops {
                match *op {
                    Op::Ins(r, k) => s.ins(k, r),
                    Op::Del(r, k) => s.del(k, r),
                    Op::Upd(r, a, b) => s.upd(r, a, b),
                    Op::Read(k) => { let mut h = FNV0; let c = s.find(k, &mut h); fnv(&mut h, c as u64); if h != w.expect[ri] { ok_r = false; } ri += 1; }
                }
            }
            tt.push(ms(t.elapsed()));
            if rep == 0 { ok_f &= s.fp() == tfp; ok_i &= s.check().is_ok(); extra = s.extra(); }
        }
        csv.put(n, ds, vname, &format!("crud_us_{mname}"), med(&tt).0 * 1000.0 / cap as f64);
        csv.put(n, ds, vname, &format!("ops_{mname}"), cap as f64);
        ok_all &= ok_r && ok_f && ok_i;
        csv.put(n, ds, vname, &format!("ok_crud_{mname}"), (ok_r && ok_f && ok_i) as u8 as f64);
        for (k, val) in extra { csv.put(n, ds, vname, &format!("{k}_{mname}"), val); }
        // latencije po operaciji (poseban prolaz) — ukupno i po tipu; memorija pre i posle izmena
        // merni nizovi se alociraju PRE početnog stanja memorije, da ne ulaze u rast strukture
        let mut lat: Vec<u64> = Vec::with_capacity(cap);
        let mut by: [Vec<u64>; 4] = [Vec::with_capacity(cap), Vec::with_capacity(cap), Vec::with_capacity(cap), Vec::with_capacity(cap)];
        let base = cur();
        let mut s = build(v, sorted);
        let mem_before = cur().saturating_sub(base);
        for op in ops {
            let t = Instant::now();
            let ty = match *op {
                Op::Ins(r, k) => { s.ins(k, r); 0 }
                Op::Del(r, k) => { s.del(k, r); 1 }
                Op::Upd(r, a, b) => { s.upd(r, a, b); 2 }
                Op::Read(k) => { let mut h = FNV0; s.find(k, &mut h); 3 }
            };
            let ns = t.elapsed().as_nanos() as u64;
            lat.push(ns);
            by[ty].push(ns);
        }
        let mem_after = cur().saturating_sub(base);
        drop(s);
        for (lbl, p) in [("p50", 0.5), ("p95", 0.95), ("p99", 0.99)] { csv.put(n, ds, vname, &format!("crud_{lbl}_ns_{mname}"), pctl(&mut lat, p)); }
        for (ti, tname) in ["umetanje", "brisanje", "azuriranje", "citanje"].iter().enumerate() {
            let v = &mut by[ti];
            if v.is_empty() { continue; }
            let mean = v.iter().sum::<u64>() as f64 / v.len() as f64;
            csv.put(n, ds, vname, &format!("op_{tname}_prosek_ns_{mname}"), mean);
            for (lbl, p) in [("p50", 0.5), ("p95", 0.95), ("p99", 0.99)] { csv.put(n, ds, vname, &format!("op_{tname}_{lbl}_ns_{mname}"), pctl(v, p)); }
        }
        csv.put(n, ds, vname, &format!("mem_pre_izmena_mib_{mname}"), mib(mem_before));
        csv.put(n, ds, vname, &format!("mem_posle_izmena_mib_{mname}"), mib(mem_after));
        csv.put(n, ds, vname, &format!("mem_rast_mib_{mname}"), mib(mem_after) - mib(mem_before));
    }
    csv.put(n, ds, vname, "ok_sve", ok_all as u8 as f64);
    csv.put(n, ds, vname, "zavrseno", 1.0);
}

// ======================= glavna petlja =======================

pub fn run(o: &mut String, m365: &[(u64, i64)], reps: usize, resume: Option<String>, sizes: &[usize], nq: usize, nr: usize, n_ops: usize) {
    let _ = std::fs::create_dir_all("results");
    let path = resume.clone().unwrap_or_else(|| {
        let ts = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs();
        format!("results/matrix_{ts}.csv")
    });
    let mut rows = vec![];
    let mut done: HashSet<(usize, String, String)> = HashSet::new();
    if let Ok(txt) = std::fs::read_to_string(&path) {
        for line in txt.lines().skip(1) {
            let p: Vec<&str> = line.split(',').collect();
            if p.len() != 5 { continue; }
            if let (Ok(n), Ok(v)) = (p[0].parse::<usize>(), p[4].parse::<f64>()) {
                if p[3] == "zavrseno" { done.insert((n, p[1].into(), p[2].into())); }
                rows.push((n, p[1].to_string(), p[2].to_string(), p[3].to_string(), v));
            }
        }
    }
    let new_file = rows.is_empty();
    let f = std::fs::OpenOptions::new().create(true).append(true).open(&path).expect("ne mogu da otvorim CSV");
    let mut csv = Csv { f, rows };
    if new_file { let _ = writeln!(csv.f, "redova,skup,varijanta,metrika,vrednost"); }
    // nedovršene kombinacije se ponavljaju; njihovi delimični redovi se zanemaruju u izveštaju
    csv.rows.retain(|r| done.contains(&(r.0, r.1.clone(), r.2.clone())));

    let vars = variants();
    let total_w: f64 = sizes.iter().map(|&n| n as f64 * (DATASETS.len() * vars.len()) as f64).sum();
    let mut done_w = 0.0;
    let mut work_w = 0.0;
    let t0 = Instant::now();
    out!(o, "\n## MATRIČNI TEST — ARI i kontrole (v0.12b: + cena po tipu operacije, memorija posle izmena, ispravljen skup „promenljiv“)");
    out!(o, "CSV: {path} | ponavljanja {reps} | veličine {:?} | skupovi {:?} | varijanti {} | upita {nq} | opsega po širini {nr} | izmena {n_ops} × {} odnosa",
        sizes, DATASETS, vars.len(), MIXES.len());
    out!(o, "kriterijumi i predviđanja (pre merenja): D:\\sqliteRecog\\ARI_KRITERIJUMI_I_PREDVIDJANJA.md");
    if !done.is_empty() { out!(o, "nastavak: {} kombinacija već završeno, preskaču se", done.len()); }

    let pool1 = rayon::ThreadPoolBuilder::new().num_threads(1).build().unwrap();
    for &n0 in sizes {
        for &ds in DATASETS.iter() {
            let n = if ds == "m365" { n0.min(m365.len()) } else { n0 };
            let todo: Vec<&(String, V)> = vars.iter().filter(|(vn, _)| !done.contains(&(n, ds.to_string(), vn.clone()))).collect();
            if todo.is_empty() { done_w += (n0 * vars.len()) as f64; continue; }
            if ds == "m365" && m365.is_empty() { out!(o, "m365: nema podataka (pokreni sa --cache), preskačem"); continue; }
            let tp = Instant::now();
            let (mut data, g) = dataset(ds, n, m365);
            pool1.install(|| { let mut s = Vec::new(); radix_sort_by_key(&mut data, &mut s, |e| e.0); });
            let q = queries(&data, nq, nr);
            let sv = Sv(data.clone());
            let refs = {
                let mut h = FNV0;
                let mut s2 = Sv(data.clone());
                for &k in &q.eq { let c = s2.find(k, &mut h); fnv(&mut h, c as u64); }
                let cnt: Vec<u64> = q.ranges.iter().map(|cl| { let mut h = FNV0; for &(a, b) in cl { fnv(&mut h, sv.count(a, b) as u64); } h }).collect();
                let fet: Vec<u64> = q.ranges.iter().map(|cl| { let mut h = FNV0; for &(a, b) in cl { let c = sv.fetch(a, b, &mut h); fnv(&mut h, c as u64); } h }).collect();
                (h, cnt, fet)
            };
            drop(sv);
            let works: Vec<(String, Work)> = MIXES.iter().enumerate().map(|(i, (name, mix))| (name.to_string(), workload(&data, &g, *mix, n_ops, 0xC0FFEE + i as u64 * 31 + n as u64))).collect();
            let max_bucket = DynIndex::with_mode(&data, Mode::Log(12)).max_bucket();
            out!(o, "\n### {n} redova — skup „{ds}“ (priprema {:.1} s; najveći Log(12) bucket {max_bucket})", tp.elapsed().as_secs_f64());
            let mut prefix_fp: FxMap<(usize, usize), u64> = FxMap::default();
            for (vname, v) in vars.iter() {
                if done.contains(&(n, ds.to_string(), vname.clone())) { done_w += n0 as f64; continue; }
                let tr = Instant::now();
                run_one(&mut csv, n, ds, vname, *v, &data, &q, &refs, &works, reps, &mut prefix_fp);
                done_w += n0 as f64; work_w += n0 as f64;
                let el = t0.elapsed().as_secs_f64();
                let eta = if work_w > 0.0 { el / work_w * (total_w - done_w) } else { 0.0 };
                let ok = csv.get(n, ds, vname, "ok_sve").unwrap_or(0.0) == 1.0;
                out!(o, "[{:>5.1}%] {n:>8} {ds:<11} {vname:<26} {:>6.1} s  tačno: {}  | proteklo {:.0} min, preostalo ~{:.0} min",
                    100.0 * done_w / total_w, tr.elapsed().as_secs_f64(), if ok { "DA" } else { "NE" }, el / 60.0, eta / 60.0);
            }
        }
    }
    report(o, &csv, sizes, m365.len());
    let rp = path.replace(".csv", "_izvestaj.md");
    let _ = std::fs::write(&rp, o.split("## IZVEŠTAJ").nth(1).map(|s| format!("## IZVEŠTAJ{s}")).unwrap_or_default());
    out!(o, "\n[CSV: {path}]\n[izveštaj: {rp}]");
}

// ======================= izveštaj: kriterijumi i predviđanja =======================

fn report(o: &mut String, csv: &Csv, sizes: &[usize], m365_len: usize) {
    let big = *sizes.iter().max().unwrap();
    let m365_big = big.min(m365_len.max(1));
    let g = |n: usize, ds: &str, v: &str, m: &str| csv.get(n, ds, v, m);
    let r = |a: Option<f64>, b: Option<f64>| match (a, b) { (Some(a), Some(b)) if b > 0.0 => Some(a / b), _ => None };
    let f2 = |x: Option<f64>| x.map(|v| format!("{v:.2}")).unwrap_or("—".into());
    let verdict = |b: Option<bool>| match b { Some(true) => "PROLAZI", Some(false) => "NE PROLAZI", None => "nema podataka" };
    let c = "C_ARI_4096";
    let a = "A_Log12";
    let all_ok = csv.rows.iter().filter(|r| r.3.starts_with("ok_")).all(|r| r.4 == 1.0);
    let bad: Vec<String> = csv.rows.iter().filter(|r| r.3.starts_with("ok_") && r.4 != 1.0).map(|r| format!("{} {} {} {}", r.0, r.1, r.2, r.3)).take(20).collect();

    out!(o, "\n## IZVEŠTAJ — ocena kriterijuma (iz predloga) i predviđanja (Claude), zapisanih pre merenja");
    out!(o, "Najveća veličina: {big} (m365: {m365_big}). Odnosi > 1 znače „brže od“ ili „manje od“ kako je navedeno.");

    // --- kriterijumi ---
    out!(o, "\n### Kriterijumi uspeha za C (ARI)");
    out!(o, "| # | kriterijum | merenje | ocena |");
    out!(o, "|---|---|---|---|");
    let loc: f64 = csv.rows.iter().filter(|r| r.2.starts_with("C_") && r.3.starts_with("locality_viol")).map(|r| r.4).sum();
    out!(o, "| K1 | lokalnost | kršenja lokalnosti u svim C pokretanjima: {loc} | {} |", verdict(Some(loc == 0.0)));
    out!(o, "| K2 | tačnost | sve provere (invarijante, jednakost, opsezi, CRUD): {} {} | {} |", if all_ok { "sve DA" } else { "ima NE:" }, bad.join("; "), verdict(Some(all_ok)));
    let mut k3 = vec![]; let mut k3ok = Some(true);
    for &n in sizes { let x = r(g(n, "ravnomeran", c, "eq_ns"), g(n, "ravnomeran", a, "eq_ns")); k3.push(format!("{n}: {}", f2(x))); if let Some(x) = x { if x > 1.10 { k3ok = Some(false); } } else { k3ok = None; } }
    out!(o, "| K3 | jednakost na ravnomernim ≤ 1,10× A | C/A vreme: {} | {} |", k3.join(", "), verdict(k3ok));
    let mut best: Option<(f64, String)> = None;
    for m in ["eq_ns", "count_ns_0.01pct", "count_ns_0.1pct", "count_ns_1pct", "count_ns_10pct", "count_ns_50pct"] {
        if let Some(x) = r(g(big, "grupisan", a, m), g(big, "grupisan", c, m)) { if best.as_ref().map(|b| x > b.0).unwrap_or(true) { best = Some((x, m.to_string())); } }
    }
    out!(o, "| K4 | ≥ 2× brže od A u preopterećenom delu (grupisan, {big}) | najbolji A/C: {} ({}) | {} |",
        best.as_ref().map(|b| format!("{:.2}×", b.0)).unwrap_or("—".into()), best.as_ref().map(|b| b.1.clone()).unwrap_or_default(), verdict(best.as_ref().map(|b| b.0 >= 2.0)));
    let mut k5 = None::<f64>;
    for &n in sizes { for ds in DATASETS { if let Some(x) = r(g(n.min(if ds == "m365" { m365_len } else { n }), ds, c, "mem_mib"), g(n.min(if ds == "m365" { m365_len } else { n }), ds, a, "mem_mib")) { k5 = Some(k5.map_or(x, |m| m.max(x))); } } }
    out!(o, "| K5 | memorija ≤ 1,25× A | najveći odnos C/A: {} | {} |", f2(k5), verdict(k5.map(|x| x <= 1.25)));
    let flips = csv.rows.iter().filter(|r| r.2.starts_with("C_") && r.3.starts_with("flips")).map(|r| r.4).fold(0.0, f64::max);
    out!(o, "| K6 | stabilnost: ≤ 2 spajanja istog bucket-a | najviše: {flips} | {} |", verdict(Some(flips <= 2.0)));
    let mut k7 = vec![]; let mut k7ok = Some(true);
    for ds in ["grupisan", "promenljiv"] { for (mn, _) in MIXES { let x = r(g(big, ds, a, &format!("crud_us_{mn}")), g(big, ds, c, &format!("crud_us_{mn}"))); k7.push(format!("{ds}/{mn}: {}", f2(x))); match x { Some(x) if x < 1.0 => k7ok = Some(false), None => k7ok = None, _ => {} } } }
    out!(o, "| K7 | isplativost: CRUD C (sa pauzama) ≤ A | A/C: {} | {} |", k7.join(", "), verdict(k7ok));
    let mut k8 = vec![];
    for ds in ["grupisan", "promenljiv"] {
        k8.push(format!("{ds}: eq B/C {} · count1% B/C {} · CRUD B/C {}", f2(r(g(big, ds, "B_Log14", "eq_ns"), g(big, ds, c, "eq_ns"))),
            f2(r(g(big, ds, "B_Log14", "count_ns_1pct"), g(big, ds, c, "count_ns_1pct"))), f2(r(g(big, ds, "B_Log14", "crud_us_uravnotezeno"), g(big, ds, c, "crud_us_uravnotezeno")))));
    }
    out!(o, "| K8 | kontrola B (Log(14)): da li je C bolji od finijeg fiksnog bucket-a? (> 1 = C brži) | {} | tumači se |", k8.join(" ‖ "));

    // --- predviđanja ---
    out!(o, "\n### Predviđanja (Claude)");
    out!(o, "| # | predviđanje | izmereno | ocena |");
    out!(o, "|---|---|---|---|");
    let n = m365_big;
    let p1 = [("eq", r(g(n, "m365", c, "eq_ns"), g(n, "m365", a, "eq_ns"))), ("count1%", r(g(n, "m365", c, "count_ns_1pct"), g(n, "m365", a, "count_ns_1pct"))), ("CRUD", r(g(n, "m365", c, "crud_us_uravnotezeno"), g(n, "m365", a, "crud_us_uravnotezeno")))];
    let nodes = g(n, "m365", c, "refined_nodes_izgradnja").unwrap_or(0.0) + g(n, "m365", c, "refines_uravnotezeno").unwrap_or(0.0);
    let p1ok = p1.iter().all(|x| x.1.map(|v| (0.9..=1.1).contains(&v)).unwrap_or(false)) && nodes <= 10.0;
    out!(o, "| P1 | m365: C ≈ A (±10%), ≤ 10 usavršenih čvorova | C/A: {} · čvorova: {nodes} | {} |", p1.iter().map(|x| format!("{} {}", x.0, f2(x.1))).collect::<Vec<_>>().join(", "), if p1ok { "POGOĐENO" } else { "PROMAŠENO" });
    let mut p2 = vec![]; let mut p2ok = true;
    for &n in sizes { let br = g(n, "ravnomeran", "C_ARI_1024", "build_refines_izgradnja").unwrap_or(-1.0); let x = r(g(n, "ravnomeran", c, "eq_ns"), g(n, "ravnomeran", a, "eq_ns")); p2.push(format!("{n}: usavršavanja(1024) {br}, C/A {}", f2(x))); if br != 0.0 || !x.map(|v| (0.95..=1.05).contains(&v)).unwrap_or(false) { p2ok = false; } }
    out!(o, "| P2 | ravnomeran: 0 usavršavanja, C = A ±5% | {} | {} |", p2.join("; "), if p2ok { "POGOĐENO" } else { "PROMAŠENO" });
    let (ae, ce, be) = (g(big, "grupisan", a, "eq_ns"), g(big, "grupisan", c, "eq_ns"), g(big, "grupisan", "B_Log14", "eq_ns"));
    let p3a = r(ae, ce);
    let p3b = match (ae, ce, be) { (Some(a), Some(c), Some(b)) if a > c => Some((a - b) / (a - c)), _ => None };
    out!(o, "| P3 | grupisan: A/C eq ≥ 1,5; B postiže ≥ 80% dobitka | A/C {} · udeo dobitka B {} | {} |", f2(p3a), f2(p3b), if p3a.map(|x| x >= 1.5).unwrap_or(false) && p3b.map(|x| x >= 0.8).unwrap_or(false) { "POGOĐENO" } else { "PROMAŠENO" });
    let p4a = r(g(big, "duplikati", c, "eq_ns"), g(big, "duplikati", a, "eq_ns"));
    let p4m = r(g(big, "duplikati", "G_posting", "mem_mib"), g(big, "duplikati", a, "mem_mib"));
    let p4c = r(g(big, "duplikati", "G_posting", "count_ns_1pct"), g(big, "duplikati", a, "count_ns_1pct"));
    let p4ok = p4a.map(|x| (0.9..=1.1).contains(&x)).unwrap_or(false) && p4m.map(|x| x <= 0.7).unwrap_or(false) && p4c.map(|x| (0.8..=1.2).contains(&x)).unwrap_or(false);
    out!(o, "| P4 | duplikati: C bez dobitka; G ≤ 0,7× memorije A; G COUNT ±20% | C/A eq {} · G/A mem {} · G/A count {} | {} |", f2(p4a), f2(p4m), f2(p4c), if p4ok { "POGOĐENO" } else { "PROMAŠENO" });
    let p5 = r(g(big, "promenljiv", a, "crud_us_uravnotezeno"), g(big, "promenljiv", c, "crud_us_uravnotezeno"));
    let p5d = MIXES.iter().filter_map(|(m, _)| g(big, "promenljiv", "D_globalna_izgradnja", &format!("pause_max_us_{m}"))).fold(0.0, f64::max) / 1000.0;
    out!(o, "| P5 | promenljiv: A/C CRUD ≥ 1,5; D pauza ≥ 100 ms | A/C {} · D najveća pauza {p5d:.1} ms | {} |", f2(p5), if p5.map(|x| x >= 1.5).unwrap_or(false) && p5d >= 100.0 { "POGOĐENO" } else { "PROMAŠENO" });
    let p6 = csv.rows.iter().filter(|r| r.2.starts_with("C_") && r.3.starts_with("pause_max_us")).map(|r| r.4).fold(0.0, f64::max) / 1000.0;
    out!(o, "| P6 | najduža pauza usavršavanja ≤ 2 ms | {p6:.2} ms | {} |", if p6 <= 2.0 { "POGOĐENO" } else { "PROMAŠENO" });
    let mut p7 = vec![]; let mut p7ok = true;
    for ds in DATASETS { let nn = if ds == "m365" { m365_big } else { big }; for m in ["eq_ns", "count_ns_1pct", "crud_us_uravnotezeno"] { if let Some(x) = r(g(nn, ds, "C_ARI_4096_fenwick", m), g(nn, ds, c, m)) { if !(0.95..=1.05).contains(&x) { p7ok = false; } p7.push(format!("{ds} {m} {x:.2}")); } } }
    out!(o, "| P7 | Fenwick ≈ obična tabela (< 5%) | {} | {} |", p7.join(", "), if p7ok && !p7.is_empty() { "POGOĐENO" } else { "PROMAŠENO" });
    let mut p8 = vec![]; let mut p8ok = true;
    for ds in ["grupisan", "promenljiv"] {
        let rt = r(g(big, ds, "C_ARI_4096_lokalni_sort", "pause_total_ms_uravnotezeno"), g(big, ds, c, "pause_total_ms_uravnotezeno"));
        let tot = r(g(big, ds, "C_ARI_4096_lokalni_sort", "crud_us_uravnotezeno"), g(big, ds, c, "crud_us_uravnotezeno"));
        p8.push(format!("{ds}: vreme usavršavanja sort/raspodela {} · ukupno {}", f2(rt), f2(tot)));
        if !(rt.map(|x| x >= 2.0).unwrap_or(false) && tot.map(|x| (0.95..=1.05).contains(&x)).unwrap_or(false)) { p8ok = false; }
    }
    out!(o, "| P8 | lokalna raspodela ≥ 2× brža u usavršavanju, ukupno < 5% razlike | {} | {} |", p8.join(" ‖ "), if p8ok { "POGOĐENO" } else { "PROMAŠENO" });

    // --- pregled po skupu (najveća veličina) ---
    out!(o, "\n### Pregled — najveća veličina, odnos „uravnoteženo“ (ns po upitu / µs po izmeni)");
    for ds in DATASETS {
        let nn = if ds == "m365" { m365_big } else { big };
        out!(o, "\n**{ds}** ({nn} redova)");
        out!(o, "| varijanta | izgradnja ms | mem MiB | eq ns | eq p99 | COUNT 1% | COUNT 50% | FETCH 1% | CRUD µs | CRUD p99 ns | usavršavanja | najveća pauza µs | tačno |");
        out!(o, "|---|---|---|---|---|---|---|---|---|---|---|---|---|");
        for (vn, _) in variants() {
            let x = |m: &str| g(nn, ds, &vn, m).map(|v| if v >= 100.0 { format!("{v:.0}") } else { format!("{v:.2}") }).unwrap_or("—".into());
            let refs = g(nn, ds, &vn, "refines_uravnotezeno").map(|v| v + g(nn, ds, &vn, "build_refines_izgradnja").unwrap_or(0.0)).or(g(nn, ds, &vn, "rebuilds_uravnotezeno"));
            out!(o, "| {vn} | {} | {} | {} | {} | {} | {} | {} | {} | {} | {} | {} | {} |", x("build_ms"), x("mem_mib"), x("eq_ns"), x("eq_p99"), x("count_ns_1pct"), x("count_ns_50pct"), x("fetch_ns_1pct"),
                x("crud_us_uravnotezeno"), x("crud_p99_ns_uravnotezeno"), refs.map(|v| format!("{v:.0}")).unwrap_or("—".into()), x("pause_max_us_uravnotezeno"),
                if g(nn, ds, &vn, "ok_sve") == Some(1.0) { "DA" } else { "NE" });
        }
    }
    out!(o, "\n### Cena izmena po tipu i cena održavanja — najveća veličina, odnos „uravnoteženo“");
    out!(o, "Vremena po jednoj operaciji iz prolaza sa merenjem svake operacije (uključuju ~20–30 ns za samo merenje). Rast memorije = posle izmena − posle izgradnje.");
    for ds in DATASETS {
        let nn = if ds == "m365" { m365_big } else { big };
        out!(o, "\n**{ds}** ({nn} redova)");
        out!(o, "| varijanta | umetanje ns (p99) | brisanje ns (p99) | ažuriranje ns (p99) | čitanje ns (p99) | mem posle izgradnje MiB | mem posle izmena MiB | rast MiB | pauze ukupno ms | premešteno elemenata |");
        out!(o, "|---|---|---|---|---|---|---|---|---|---|");
        for (vn, _) in variants() {
            let x = |m: &str| g(nn, ds, &vn, m);
            let pair = |t: &str| match (x(&format!("op_{t}_prosek_ns_uravnotezeno")), x(&format!("op_{t}_p99_ns_uravnotezeno"))) {
                (Some(a), Some(b)) => format!("{a:.0} ({b:.0})"), _ => "—".into() };
            let f = |m: &str| x(m).map(|v| format!("{v:.1}")).unwrap_or("—".into());
            out!(o, "| {vn} | {} | {} | {} | {} | {} | {} | {} | {} | {} |", pair("umetanje"), pair("brisanje"), pair("azuriranje"), pair("citanje"),
                f("mem_pre_izmena_mib_uravnotezeno"), f("mem_posle_izmena_mib_uravnotezeno"), f("mem_rast_mib_uravnotezeno"),
                f("pause_total_ms_uravnotezeno"), x("moved_uravnotezeno").map(|v| format!("{v:.0}")).unwrap_or("—".into()));
        }
    }
    out!(o, "\nSvi brojevi su u CSV-u; ovde je samo pregled. Napomena: F (sortiran niz) radi najviše 3.000 izmena kad je n > 100.000.");
}

#[cfg(test)]
mod probe {
    use super::*;
    #[test]
    #[ignore]
    fn eq_through_trait() {
        let (mut data, _g) = dataset("ravnomeran", 300_000, &[]);
        data.sort();
        let mut q = queries(&data, 200_000, 10);
        if std::env::var("ARIQ").is_ok() {
            let mut r = Rng(1);
            q.eq = (0..200_000).map(|i| if i % 2 == 0 { data[r.below(data.len() as u64) as usize].0 } else { 1 + r.below(1u64 << 34) }).collect();
            println!("ARI-probe upiti");
        }
        if std::env::var("ONLYHIT").is_ok() { q.eq.retain(|&k| data.binary_search_by(|e| e.0.cmp(&k)).is_ok()); println!("samo pogoci: {}", q.eq.len()); }
        if std::env::var("ONLYMISS").is_ok() { q.eq.retain(|&k| data.binary_search_by(|e| e.0.cmp(&k)).is_err()); println!("samo promašaji: {}", q.eq.len()); }
        let mut a = build(V::A, &data);
        let mut c = build(V::C(AriCfg::new(4096)), &data);
        let rounds: usize = std::env::var("ROUNDS").ok().and_then(|x| x.parse().ok()).unwrap_or(6);
        for round in 0..rounds {
            let mut run = |x: &mut Box<dyn Idx>| { let t = Instant::now(); let mut h = FNV0; for &k in &q.eq { let n = x.find(k, &mut h); fnv(&mut h, n as u64); } (t.elapsed().as_nanos() as f64 / q.eq.len() as f64, h) };
            let (ta, tc, ha, hc);
            if round % 2 == 0 { (ta, ha) = run(&mut a); (tc, hc) = run(&mut c); } else { (tc, hc) = run(&mut c); (ta, ha) = run(&mut a); }
            assert_eq!(ha, hc);
            // direktno, bez trait objekta
            let (mut ad, mut cd);
            if round % 2 == 0 { ad = DynIndex::with_mode(&data, Mode::Log(12)); cd = Ari::new(&data, AriCfg::new(4096)); }
            else { cd = Ari::new(&data, AriCfg::new(4096)); ad = DynIndex::with_mode(&data, Mode::Log(12)); }
            let ml = cd.max_leaf(); let rn = cd.refined_nodes();
            let t = Instant::now(); let mut h = FNV0; for &k in &q.eq { let n = DynIndex::find(&mut ad, k, &mut h); fnv(&mut h, n as u64); } let tad = t.elapsed().as_nanos() as f64 / q.eq.len() as f64;
            let t = Instant::now(); let mut h2 = FNV0; for &k in &q.eq { let n = Ari::find(&mut cd, k, &mut h2); fnv(&mut h2, n as u64); } let tcd = t.elapsed().as_nanos() as f64 / q.eq.len() as f64;
            assert_eq!(h, h2);
            println!("runda {round}: kroz trait A {ta:.0} ns, C {tc:.0} ns | direktno A {tad:.0} ns, C {tcd:.0} ns | najveći list {ml}, usavršenih {rn}, A najveći bucket {}", ad.max_bucket());
        }
    }
}
