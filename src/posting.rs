#![allow(dead_code)]
//! Varijanta G: dinamički Recognition Index (Log(12) + Fenwick) sa posting listama.
//!
//! U svakom bucket-u svaki različit ključ čuva se JEDNOM:
//!   keys[i]  — i-ti različit ključ (sortirano)
//!   ends[i]  — kraj njegove grupe u `rowids` (kumulativno, isključivo)
//!   rowids   — rowid-ovi grupisani po ključu, sortirani unutar grupe
//! Memorija: 12 B po različitom ključu + 8 B po redu (naspram 16 B po redu u A).
//! Broj redova ključa je težina za COUNT; Fenwick nad brojem redova po bucket-u.

use crate::dynidx::{Fenwick, MODE};
use crate::find::RecogIndex;
use crate::fnv;

#[derive(Default)]
struct PB { keys: Vec<u64>, ends: Vec<u32>, rowids: Vec<i64> }
impl PB {
    #[inline] fn start(&self, i: usize) -> usize { if i == 0 { 0 } else { self.ends[i - 1] as usize } }
    /// broj redova sa ključem < k (incl=false) ili ≤ k (incl=true)
    fn rank(&self, k: u64, incl: bool) -> usize {
        let i = if incl { self.keys.partition_point(|&x| x <= k) } else { self.keys.partition_point(|&x| x < k) };
        self.start(i)
    }
}

pub struct Posting { b: Vec<Option<Box<PB>>>, fen: Fenwick }

static EMPTY: PB = PB { keys: Vec::new(), ends: Vec::new(), rowids: Vec::new() };

impl Posting {
    fn cap() -> usize { RecogIndex::bucket(MODE, u64::MAX) + 1 }
    fn pb(&self, x: usize) -> &PB { self.b[x].as_deref().unwrap_or(&EMPTY) }
    pub fn new(sorted: &[(u64, i64)]) -> Self {
        let nb = Self::cap();
        let mut b: Vec<Option<Box<PB>>> = (0..nb).map(|_| None).collect();
        for &(k, r) in sorted {
            let p = b[RecogIndex::bucket(MODE, k)].get_or_insert_with(Default::default);
            if p.keys.last() != Some(&k) { p.keys.push(k); p.ends.push(p.rowids.len() as u32); }
            p.rowids.push(r);
            *p.ends.last_mut().unwrap() += 1;
        }
        // posle izgradnje: kapacitet = stvarna veličina (kao to_vec u A), da poređenje memorije bude fer
        for p in b.iter_mut().flatten() { p.keys.shrink_to_fit(); p.ends.shrink_to_fit(); p.rowids.shrink_to_fit(); }
        let sizes: Vec<i64> = b.iter().map(|p| p.as_ref().map_or(0, |p| p.rowids.len()) as i64).collect();
        Posting { b, fen: Fenwick::from_sizes(&sizes) }
    }
    pub fn ins(&mut self, k: u64, r: i64) {
        let x = RecogIndex::bucket(MODE, k);
        let p = self.b[x].get_or_insert_with(Default::default);
        let i = p.keys.partition_point(|&y| y < k);
        if i == p.keys.len() || p.keys[i] != k {
            let s = p.start(i);
            p.keys.insert(i, k);
            p.ends.insert(i, s as u32);
        }
        let (s, e) = (p.start(i), p.ends[i] as usize);
        let at = s + p.rowids[s..e].partition_point(|&y| y < r);
        p.rowids.insert(at, r);
        for v in &mut p.ends[i..] { *v += 1; }
        self.fen.add(x, 1);
    }
    pub fn del(&mut self, k: u64, r: i64) {
        let x = RecogIndex::bucket(MODE, k);
        let p = self.b[x].as_mut().expect("Posting: prazan bucket");
        let i = p.keys.partition_point(|&y| y < k);
        assert!(i < p.keys.len() && p.keys[i] == k, "Posting: nema ključa");
        let (s, e) = (p.start(i), p.ends[i] as usize);
        let at = s + p.rowids[s..e].partition_point(|&y| y < r);
        assert!(at < e && p.rowids[at] == r, "Posting: nema rowid-a");
        p.rowids.remove(at);
        for v in &mut p.ends[i..] { *v -= 1; }
        if p.start(i) == p.ends[i] as usize { p.keys.remove(i); p.ends.remove(i); }
        if p.rowids.is_empty() { self.b[x] = None; }
        self.fen.add(x, -1);
    }
    pub fn find(&self, k: u64, h: &mut u64) -> usize {
        let p = self.pb(RecogIndex::bucket(MODE, k));
        let i = p.keys.partition_point(|&y| y < k);
        if i == p.keys.len() || p.keys[i] != k { return 0; }
        let (s, e) = (p.start(i), p.ends[i] as usize);
        for &r in &p.rowids[s..e] { fnv(h, r as u64); }
        e - s
    }
    fn rank(&self, k: u64, incl: bool) -> usize {
        let x = RecogIndex::bucket(MODE, k);
        self.fen.prefix(x) as usize + self.pb(x).rank(k, incl)
    }
    pub fn count_range(&self, ka: u64, kb: u64) -> usize { if ka > kb { 0 } else { self.rank(kb, true) - self.rank(ka, false) } }
    pub fn fetch_range(&self, ka: u64, kb: u64, h: &mut u64) -> usize {
        if ka > kb { return 0; }
        let (ba, bb) = (RecogIndex::bucket(MODE, ka), RecogIndex::bucket(MODE, kb));
        let mut c = 0;
        for x in ba..=bb {
            let Some(p) = self.b[x].as_deref() else { continue };
            let lo = if x == ba { p.rank(ka, false) } else { 0 };
            let hi = if x == bb { p.rank(kb, true) } else { p.rowids.len() };
            for &r in &p.rowids[lo..hi] { fnv(h, r as u64); c += 1; }
        }
        c
    }
    pub fn fingerprint(&self) -> u64 {
        let mut h = crate::FNV0;
        for p in self.b.iter().flatten() { for i in 0..p.keys.len() { for &r in &p.rowids[p.start(i)..p.ends[i] as usize] { fnv(&mut h, p.keys[i]); fnv(&mut h, r as u64); } } }
        h
    }
    pub fn len(&self) -> usize { self.fen.prefix(self.b.len()) as usize }
    pub fn check(&self) -> Result<(), String> {
        for (x, p) in self.b.iter().enumerate() {
            let p = p.as_deref().unwrap_or(&EMPTY);
            if p.keys.len() != p.ends.len() { return Err(format!("bucket {x}: keys/ends dužine")); }
            for w in p.keys.windows(2) { if w[0] >= w[1] { return Err(format!("bucket {x}: ključevi nisu strogo sortirani")); } }
            for (i, &k) in p.keys.iter().enumerate() {
                if RecogIndex::bucket(MODE, k) != x { return Err(format!("ključ {k} u pogrešnom bucket-u")); }
                let (s, e) = (p.start(i), p.ends[i] as usize);
                if s >= e { return Err(format!("prazna grupa ključa {k}")); }
                for w in p.rowids[s..e].windows(2) { if w[0] >= w[1] { return Err(format!("rowid-ovi ključa {k} nisu sortirani")); } }
            }
            if p.ends.last().map(|&e| e as usize).unwrap_or(0) != p.rowids.len() { return Err(format!("bucket {x}: kraj ≠ broj redova")); }
            if (self.fen.prefix(x + 1) - self.fen.prefix(x)) as usize != p.rowids.len() { return Err(format!("Fenwick ≠ bucket {x}")); }
        }
        Ok(())
    }
    pub fn bytes(&self) -> usize {
        self.b.len() * 8 + self.fen.bytes()
            + self.b.iter().flatten().map(|p| std::mem::size_of::<PB>() + p.keys.capacity() * 8 + p.ends.capacity() * 4 + p.rowids.capacity() * 8).sum::<usize>()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::find::Rng;
    use std::collections::BTreeSet;
    #[test]
    fn posting_matches_model() {
        let mut rng = Rng(3);
        let gen = |rng: &mut Rng| if rng.below(2) == 0 { 1 + rng.below(50) * 1_000_003 } else { 1 + rng.below(1 << 30) };
        let mut v: Vec<(u64, i64)> = (0..50_000).map(|i| (gen(&mut rng), i)).collect();
        v.sort();
        let mut p = Posting::new(&v);
        let mut m: BTreeSet<(u64, i64)> = v.iter().copied().collect();
        let mut next = 1_000_000;
        for _ in 0..40_000 {
            match rng.below(4) {
                0 => { let k = gen(&mut rng); p.ins(k, next); m.insert((k, next)); next += 1; }
                1 => { if let Some(&e) = m.iter().nth(rng.below(100) as usize) { p.del(e.0, e.1); m.remove(&e); } }
                2 => { let k = gen(&mut rng); let (mut a, mut b) = (crate::FNV0, crate::FNV0);
                       let c = p.find(k, &mut a); let mut c2 = 0; for e in m.range((k, i64::MIN)..=(k, i64::MAX)) { fnv(&mut b, e.1 as u64); c2 += 1; }
                       assert_eq!((c, a), (c2, b)); }
                _ => { let ka = gen(&mut rng); let kb = ka + rng.below(1 << 24);
                       let want = m.range((ka, i64::MIN)..=(kb, i64::MAX)).count();
                       assert_eq!(p.count_range(ka, kb), want);
                       let (mut a, mut b) = (crate::FNV0, crate::FNV0);
                       assert_eq!(p.fetch_range(ka, kb, &mut a), want);
                       for e in m.range((ka, i64::MIN)..=(kb, i64::MAX)) { fnv(&mut b, e.1 as u64); }
                       assert_eq!(a, b); }
            }
        }
        p.check().unwrap();
        let mut h = crate::FNV0; for &(k, r) in &m { fnv(&mut h, k); fnv(&mut h, r as u64); }
        assert_eq!(p.fingerprint(), h);
    }
}
