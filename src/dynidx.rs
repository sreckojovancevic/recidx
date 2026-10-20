#![allow(dead_code)]
//! Dinamički Recognition Index (varijanta B iz koraka 4 + Fenwick).
//!
//! Struktura: za svaki bucket (podrazumevano Log(12)) poseban sortiran niz (ključ, rowid).
//! Ključ sam izračuna svoj bucket (bez poređenja); izmena i pretraga rade samo u njemu.
//! Fenwick stablo nad veličinama bucket-a daje broj elemenata u nizu celih bucket-a
//! u O(log B), pa COUNT nad opsegom ne prolazi kroz redove:
//!   COUNT(a..=b) = ostatak prvog bucket-a + zbir celih bucket-a između (Fenwick) + početak poslednjeg.
//! Svaka izmena ažurira i Fenwick (O(log B)).
//!
//! v0.12: funkcija bucket-a je parametar (`with_mode`), da bi finiji fiksni Log(14) bio kontrola za ARI.

use crate::find::{Mode, RecogIndex};
use crate::fnv;

pub const MODE: Mode = Mode::Log(12);

pub struct Fenwick { t: Vec<i64> }
impl Fenwick {
    pub fn new(n: usize) -> Self { Fenwick { t: vec![0; n + 1] } }
    pub fn from_sizes(sizes: &[i64]) -> Self {
        let n = sizes.len();
        let mut t = vec![0i64; n + 1];
        for i in 0..n {
            t[i + 1] += sizes[i];
            let j = (i + 1) + ((i + 1) & (!(i + 1) + 1));
            if j <= n { let v = t[i + 1]; t[j] += v; }
        }
        Fenwick { t }
    }
    pub fn add(&mut self, i: usize, d: i64) { let mut i = i + 1; while i < self.t.len() { self.t[i] += d; i += i & i.wrapping_neg(); } }
    /// zbir [0, i)
    pub fn prefix(&self, i: usize) -> i64 { let mut i = i.min(self.t.len() - 1); let mut s = 0; while i > 0 { s += self.t[i]; i &= i - 1; } s }
    pub fn bytes(&self) -> usize { self.t.len() * 8 }
}

pub struct DynIndex { b: Vec<Vec<(u64, i64)>>, fen: Fenwick, mode: Mode }

impl DynIndex {
    /// Broj bucket-a pokriva ceo opseg u64, pa ubacivanje nikad ne traži proširenje.
    fn cap(mode: Mode) -> usize { RecogIndex::bucket(mode, u64::MAX) + 1 }

    /// `sorted` uređen po (ključ, rowid).
    pub fn new(sorted: &[(u64, i64)]) -> Self { Self::with_mode(sorted, MODE) }

    pub fn with_mode(sorted: &[(u64, i64)], mode: Mode) -> Self {
        let nb = Self::cap(mode);
        let mut b: Vec<Vec<(u64, i64)>> = (0..nb).map(|_| Vec::new()).collect();
        let mut i = 0;
        while i < sorted.len() {
            let x = RecogIndex::bucket(mode, sorted[i].0);
            let mut j = i + 1;
            while j < sorted.len() && RecogIndex::bucket(mode, sorted[j].0) == x { j += 1; }
            b[x] = sorted[i..j].to_vec();
            i = j;
        }
        let sizes: Vec<i64> = b.iter().map(|v| v.len() as i64).collect();
        DynIndex { b, fen: Fenwick::from_sizes(&sizes), mode }
    }
    #[allow(dead_code)]
    pub fn empty() -> Self { DynIndex { b: (0..Self::cap(MODE)).map(|_| Vec::new()).collect(), fen: Fenwick::new(Self::cap(MODE)), mode: MODE } }

    pub fn mode(&self) -> Mode { self.mode }

    pub fn ins(&mut self, k: u64, r: i64) {
        let x = RecogIndex::bucket(self.mode, k);
        let v = &mut self.b[x];
        let p = v.partition_point(|e| *e < (k, r));
        v.insert(p, (k, r));
        self.fen.add(x, 1);
    }
    pub fn del(&mut self, k: u64, r: i64) {
        let x = RecogIndex::bucket(self.mode, k);
        let v = &mut self.b[x];
        let p = v.partition_point(|e| *e < (k, r));
        assert!(p < v.len() && v[p] == (k, r), "DynIndex: brisanje nepostojećeg para");
        v.remove(p);
        self.fen.add(x, -1);
    }
    /// fnv nad rowid-ovima ključa k; vraća broj pogodaka.
    pub fn find(&self, k: u64, h: &mut u64) -> usize {
        let s = &self.b[RecogIndex::bucket(self.mode, k)];
        let i = s.partition_point(|e| e.0 < k);
        let mut c = 0;
        for e in &s[i..] { if e.0 != k { break; } fnv(h, e.1 as u64); c += 1; }
        c
    }
    /// broj elemenata sa ključem u [ka, kb]
    pub fn count_range(&self, ka: u64, kb: u64) -> usize {
        if ka > kb { return 0; }
        let (ba, bb) = (RecogIndex::bucket(self.mode, ka), RecogIndex::bucket(self.mode, kb));
        let sa = &self.b[ba];
        if ba == bb {
            return sa.partition_point(|e| e.0 <= kb) - sa.partition_point(|e| e.0 < ka);
        }
        let first = sa.len() - sa.partition_point(|e| e.0 < ka);
        let middle = (self.fen.prefix(bb) - self.fen.prefix(ba + 1)) as usize;
        let last = self.b[bb].partition_point(|e| e.0 <= kb);
        first + middle + last
    }
    /// fnv nad rowid-ovima iz opsega u redosledu (ključ, rowid); vraća broj pogodaka.
    pub fn fetch_range(&self, ka: u64, kb: u64, h: &mut u64) -> usize {
        if ka > kb { return 0; }
        let (ba, bb) = (RecogIndex::bucket(self.mode, ka), RecogIndex::bucket(self.mode, kb));
        let mut c = 0;
        for x in ba..=bb {
            let s = &self.b[x];
            if s.is_empty() { continue; }
            let lo = if x == ba { s.partition_point(|e| e.0 < ka) } else { 0 };
            let hi = if x == bb { s.partition_point(|e| e.0 <= kb) } else { s.len() };
            for e in &s[lo..hi] { fnv(h, e.1 as u64); c += 1; }
        }
        c
    }
    pub fn fingerprint(&self) -> u64 {
        let mut h = crate::FNV0;
        for v in &self.b { for &(k, r) in v { fnv(&mut h, k); fnv(&mut h, r as u64); } }
        h
    }
    pub fn len(&self) -> usize { self.fen.prefix(self.b.len()) as usize }
    /// veličina bucket-a u kome bi bio ključ k, i da li u njemu ima različitih ključeva
    pub fn bucket_info(&self, k: u64) -> (usize, bool) {
        let s = &self.b[RecogIndex::bucket(self.mode, k)];
        (s.len(), s.len() > 1 && s[0].0 != s[s.len() - 1].0)
    }
    /// najveći bucket koji ima bar dva različita ključa (deljiv)
    pub fn max_splittable(&self) -> usize {
        self.b.iter().filter(|s| s.len() > 1 && s[0].0 != s[s.len() - 1].0).map(|s| s.len()).max().unwrap_or(0)
    }
    pub fn max_bucket(&self) -> usize { self.b.iter().map(|s| s.len()).max().unwrap_or(0) }
    /// svi parovi, uređeni po (ključ, rowid)
    pub fn to_sorted(&self) -> Vec<(u64, i64)> {
        let mut out = Vec::with_capacity(self.len());
        for v in &self.b { out.extend_from_slice(v); }
        out
    }
    /// provera invarijanti: sortiranost, pravi bucket, Fenwick = veličine
    pub fn check(&self) -> Result<(), String> {
        for (x, v) in self.b.iter().enumerate() {
            for w in v.windows(2) { if w[0] >= w[1] { return Err(format!("bucket {x} nije strogo sortiran")); } }
            for e in v { if RecogIndex::bucket(self.mode, e.0) != x { return Err(format!("ključ {} u pogrešnom bucket-u {x}", e.0)); } }
            let f = self.fen.prefix(x + 1) - self.fen.prefix(x);
            if f != v.len() as i64 { return Err(format!("Fenwick {f} ≠ veličina {} u bucket-u {x}", v.len())); }
        }
        Ok(())
    }
    pub fn bytes(&self) -> usize {
        self.b.len() * std::mem::size_of::<Vec<(u64, i64)>>() + self.b.iter().map(|v| v.capacity() * 16).sum::<usize>() + self.fen.bytes()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::find::Rng;
    #[test]
    fn dyn_matches_model() {
        for mode in [Mode::Log(12), Mode::Log(14)] {
            let mut rng = Rng(11);
            let mut v: Vec<(u64, i64)> = (0..200_000).map(|i| ((rng.next() % 3000) * (rng.next() % 3000) + 1, i)).collect();
            v.sort();
            let mut d = DynIndex::with_mode(&v, mode);
            let mut model: std::collections::BTreeSet<(u64, i64)> = v.iter().copied().collect();
            let mut next = 1_000_000i64;
            for step in 0..30_000 {
                match rng.below(3) {
                    0 => { let k = (rng.next() % 3000) * (rng.next() % 3000) + 1; d.ins(k, next); model.insert((k, next)); next += 1; }
                    1 => { if let Some(&e) = model.iter().nth((rng.below(50)) as usize) { d.del(e.0, e.1); model.remove(&e); } }
                    _ => {
                        let a = rng.next() % 9_000_000; let b = a + rng.next() % 2_000_000;
                        let want = model.range((a, i64::MIN)..=(b, i64::MAX)).count();
                        assert_eq!(d.count_range(a, b), want, "count na koraku {step}");
                        let (mut h1, mut h2) = (crate::FNV0, crate::FNV0);
                        let c = d.fetch_range(a, b, &mut h1);
                        for e in model.range((a, i64::MIN)..=(b, i64::MAX)) { fnv(&mut h2, e.1 as u64); }
                        assert_eq!(c, want); assert_eq!(h1, h2);
                    }
                }
            }
            let mut h = crate::FNV0;
            for &(k, r) in &model { fnv(&mut h, k); fnv(&mut h, r as u64); }
            assert_eq!(d.fingerprint(), h);
            assert!(d.check().is_ok());
            assert_eq!(d.len(), model.len());
        }
    }
}
