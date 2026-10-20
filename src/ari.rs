#![allow(dead_code)]
//! ARI — Adaptive Recognition Index (predlog drugog AI-ja, implementacija Claude).
//!
//! Polazi od dinamičkog Recognition Indexa (Log(12) bucket-i + Fenwick nad veličinama).
//! Novo: bucket koji postane preopterećen (više od `high` elemenata) dobija DODATNI nivo
//! prepoznavanja samo za sebe — sledećih `digit` bitova ključa određuje dete. Ostali bucket-i
//! se ne diraju (lokalnost). Kad čvor padne ispod `low`, spaja se nazad u jedan niz.
//!
//! Čvor je:
//!   Flat — sortiran niz (ključ, rowid)
//!   Ref  — `prefix` (zajednički bitovi iznad `pos`), deca po cifri (k >> (pos − d)) & maska,
//!          veličine dece (obična tabela ili Fenwick) i ukupan broj.
//! Ako svi ključevi u čvoru dele i sledeće bitove, cifra se spušta do prvog bita po kome se
//! razlikuju (sažimanje puta), pa svaka podela stvarno deli bar na dve grupe.
//! Čvor u kome su svi ključevi isti (duplikati) ostaje Flat — bitovi ključa ga ne mogu podeliti.

use crate::dynidx::Fenwick;
use crate::find::{Mode, RecogIndex};
use crate::fnv;
use std::time::Instant;

pub const TOP: Mode = Mode::Log(12);

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Trigger { Structural, Effective }
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Method { Distribute, Sort }
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Counts { Plain, Fenwick }

#[derive(Clone, Copy, Debug)]
pub struct AriCfg { pub high: usize, pub low: usize, pub digit: u32, pub trigger: Trigger, pub method: Method, pub counts: Counts }
impl AriCfg {
    pub fn new(high: usize) -> Self { AriCfg { high, low: high / 2, digit: 8, trigger: Trigger::Structural, method: Method::Distribute, counts: Counts::Plain } }
}

enum Sizes { Plain(Vec<u32>), Fen(Fenwick, Vec<u32>) }
impl Sizes {
    fn new(v: Vec<u32>, c: Counts) -> Self {
        match c {
            Counts::Plain => Sizes::Plain(v),
            Counts::Fenwick => { let s: Vec<i64> = v.iter().map(|&x| x as i64).collect(); Sizes::Fen(Fenwick::from_sizes(&s), v) }
        }
    }
    fn add(&mut self, i: usize, d: i64) {
        match self {
            Sizes::Plain(v) => v[i] = (v[i] as i64 + d) as u32,
            Sizes::Fen(f, v) => { f.add(i, d); v[i] = (v[i] as i64 + d) as u32; }
        }
    }
    /// zbir veličina dece [0, i)
    fn prefix(&self, i: usize) -> usize {
        match self {
            Sizes::Plain(v) => v[..i].iter().map(|&x| x as usize).sum(),
            Sizes::Fen(f, _) => f.prefix(i) as usize,
        }
    }
    fn get(&self, i: usize) -> usize { match self { Sizes::Plain(v) | Sizes::Fen(_, v) => v[i] as usize } }
    fn bytes(&self) -> usize { match self { Sizes::Plain(v) => v.len() * 4, Sizes::Fen(f, v) => f.bytes() + v.len() * 4 } }
}

struct Ref { pos: u32, d: u32, prefix: u64, children: Vec<Node>, sizes: Sizes, total: usize }
enum Node { Flat(Vec<(u64, i64)>), Ref(Box<Ref>) }

impl Ref {
    #[inline] fn digit(&self, k: u64) -> usize { ((k >> (self.pos - self.d)) & ((1u64 << self.d) - 1)) as usize }
    #[inline] fn child_pos(&self) -> u32 { self.pos - self.d }
}

#[derive(Default, Clone, Debug)]
pub struct AriStats {
    pub refines: u64,       // usavršavanja posle izgradnje (pauze)
    pub build_refines: u64, // usavršavanja tokom izgradnje
    pub merges: u64,
    pub moved: u64,         // elemenata premeštenih u usavršavanjima i spajanjima posle izgradnje
    pub locality_violations: u64,
    pub pauses_ns: Vec<u64>,
    pub max_flips: u32,     // najviše spajanja jednog vršnog bucket-a (oscilacija)
}

struct Meta { pos: u32, touches: u32, merges: u32 }

pub struct Ari { top: Vec<Node>, meta: Vec<Meta>, fen: Fenwick, cfg: AriCfg, pub st: AriStats, building: bool, depth_guard: u32 }

fn top_pos(k: u64) -> u32 { let l = 64 - k.leading_zeros(); if l <= 13 { 0 } else { l - 13 } }

impl Ari {
    fn cap() -> usize { RecogIndex::bucket(TOP, u64::MAX) + 1 }

    pub fn new(sorted: &[(u64, i64)], cfg: AriCfg) -> Self {
        let nb = Self::cap();
        let mut top: Vec<Node> = (0..nb).map(|_| Node::Flat(Vec::new())).collect();
        let mut meta: Vec<Meta> = (0..nb).map(|_| Meta { pos: 0, touches: 0, merges: 0 }).collect();
        let mut i = 0;
        while i < sorted.len() {
            let x = RecogIndex::bucket(TOP, sorted[i].0);
            let mut j = i + 1;
            while j < sorted.len() && RecogIndex::bucket(TOP, sorted[j].0) == x { j += 1; }
            top[x] = Node::Flat(sorted[i..j].to_vec());
            meta[x].pos = top_pos(sorted[i].0);
            i = j;
        }
        let sizes: Vec<i64> = top.iter().map(|t| Self::node_len(t) as i64).collect();
        let mut a = Ari { top, meta, fen: Fenwick::from_sizes(&sizes), cfg, st: AriStats::default(), building: true, depth_guard: 0 };
        if cfg.trigger == Trigger::Structural {
            for x in 0..a.top.len() {
                if Self::node_len(&a.top[x]) > cfg.high { a.refine_top(x); }
            }
        }
        a.building = false;
        a
    }

    #[inline(always)]
    fn node_len(n: &Node) -> usize { match n { Node::Flat(v) => v.len(), Node::Ref(r) => r.total } }

    // ---------- usavršavanje i spajanje ----------
    fn build_node(&mut self, v: Vec<(u64, i64)>, pos: u32) -> Node {
        if v.len() <= self.cfg.high || pos == 0 { return Node::Flat(v); }
        let (first, last) = (v[0].0, v[v.len() - 1].0);
        if first == last { return Node::Flat(v); } // svi isti ključ — nedeljivo
        let diff = 64 - (first ^ last).leading_zeros(); // najviši bit po kome se razlikuju + 1
        let pos = pos.min(diff);
        let d = self.cfg.digit.min(pos);
        let shift = pos - d;
        let nchild = 1usize << d;
        let mask = (nchild - 1) as u64;
        let mut cnt = vec![0u32; nchild];
        for e in &v { cnt[((e.0 >> shift) & mask) as usize] += 1; }
        let mut kids: Vec<Vec<(u64, i64)>> = cnt.iter().map(|&c| Vec::with_capacity(c as usize)).collect();
        for e in &v { kids[((e.0 >> shift) & mask) as usize].push(*e); }
        if self.cfg.method == Method::Sort { for k in kids.iter_mut() { k.sort_unstable(); } }
        let total = v.len();
        drop(v);
        if self.building { self.st.build_refines += 1; } else if self.depth_guard == 0 { self.st.moved += total as u64; }
        self.depth_guard += 1;
        let children: Vec<Node> = kids.into_iter().map(|k| self.build_node(k, shift)).collect();
        self.depth_guard -= 1;
        Node::Ref(Box::new(Ref { pos, d, prefix: first >> pos, children, sizes: Sizes::new(cnt, self.cfg.counts), total }))
    }

    fn collapse(n: Node, out: &mut Vec<(u64, i64)>) {
        match n {
            Node::Flat(v) => out.extend(v),
            Node::Ref(r) => { for c in r.children { Self::collapse(c, out); } }
        }
    }

    fn refine_top(&mut self, x: usize) {
        let t0 = Instant::now();
        let node = std::mem::replace(&mut self.top[x], Node::Flat(Vec::new()));
        let len = Self::node_len(&node);
        let mut v = Vec::with_capacity(len);
        Self::collapse(node, &mut v);
        let moved_before = self.st.moved;
        let pos = self.meta[x].pos;
        let n = self.build_node(v, pos);
        let was_ref = matches!(n, Node::Ref(_));
        self.top[x] = n;
        if !self.building && was_ref {
            self.st.refines += 1;
            self.st.pauses_ns.push(t0.elapsed().as_nanos() as u64);
            if self.st.moved - moved_before > len as u64 { self.st.locality_violations += 1; }
        }
    }

    /// Posle izmene u vršnom bucket-u x: usavrši ako je potrebno, spoji pod-čvorove ispod `low`.
    fn maintain(&mut self, x: usize, k: u64) {
        // spajanje: ide niz putanju ključa k i spaja prvi Ref čiji je total < low
        let low = self.cfg.low;
        let mut merged = false;
        {
            let mut cur: *mut Node = &mut self.top[x];
            loop {
                // SAFETY: cur uvek pokazuje na čvor unutar self.top[x], jedini pokazivač u upotrebi
                let n = unsafe { &mut *cur };
                match n {
                    Node::Flat(_) => break,
                    Node::Ref(r) => {
                        if r.total < low {
                            let total = r.total;
                            let old = std::mem::replace(n, Node::Flat(Vec::new()));
                            let mut v = Vec::with_capacity(total);
                            Self::collapse(old, &mut v);
                            *n = Node::Flat(v);
                            self.st.merges += 1;
                            self.st.moved += total as u64;
                            merged = true;
                            break;
                        }
                        let c = r.digit(k);
                        if (k >> r.pos) != r.prefix { break; }
                        cur = &mut r.children[c];
                    }
                }
            }
        }
        if merged {
            self.meta[x].merges += 1;
            self.st.max_flips = self.st.max_flips.max(self.meta[x].merges);
        }
        // usavršavanje: list na putanji ključa k veći od high
        let need = {
            let mut n = &self.top[x];
            loop {
                match n {
                    Node::Flat(v) => break v.len() > self.cfg.high && v.len() > 1 && v[0].0 != v[v.len() - 1].0,
                    Node::Ref(r) => { if (k >> r.pos) != r.prefix { break false; } n = &r.children[r.digit(k)]; }
                }
            }
        };
        if need {
            let ok = match self.cfg.trigger {
                Trigger::Structural => true,
                Trigger::Effective => self.meta[x].touches as usize >= Self::node_len(&self.top[x]) / 8,
            };
            if ok { self.refine_leaf(x, k); self.meta[x].touches = 0; }
        }
    }

    /// Usavršava SAMO list na putanji ključa k (ostatak vršnog bucket-a i ostali bucket-i netaknuti).
    fn refine_leaf(&mut self, x: usize, k: u64) {
        let t0 = Instant::now();
        let mut pos = self.meta[x].pos;
        let mut cur: *mut Node = &mut self.top[x];
        loop {
            // SAFETY: cur pokazuje na čvor unutar self.top[x]; nema drugih aktivnih referenci
            let n = unsafe { &mut *cur };
            match n {
                Node::Flat(_) => break,
                Node::Ref(r) => { pos = r.child_pos(); let c = r.digit(k); cur = &mut r.children[c]; }
            }
        }
        let leaf = unsafe { &mut *cur };
        let v = match std::mem::replace(leaf, Node::Flat(Vec::new())) { Node::Flat(v) => v, Node::Ref(_) => unreachable!() };
        let len = v.len();
        let moved_before = self.st.moved;
        let built = self.build_node(v, pos);
        let was_ref = matches!(built, Node::Ref(_));
        *leaf = built;
        if was_ref {
            self.st.refines += 1;
            self.st.pauses_ns.push(t0.elapsed().as_nanos() as u64);
            if self.st.moved - moved_before > len as u64 { self.st.locality_violations += 1; }
        }
    }

    // ---------- izmene ----------
    pub fn ins(&mut self, k: u64, r: i64) {
        let x = RecogIndex::bucket(TOP, k);
        if Self::node_len(&self.top[x]) == 0 { self.meta[x].pos = top_pos(k); }
        self.meta[x].touches = self.meta[x].touches.saturating_add(1);
        self.fen.add(x, 1);
        // spust; ako se prefiks ne poklapa (sažet put), ceo vršni bucket se ponovo gradi
        let mut conflict = false;
        {
            let mut n = &mut self.top[x];
            loop {
                match n {
                    Node::Flat(v) => { let p = v.partition_point(|e| *e < (k, r)); v.insert(p, (k, r)); break; }
                    Node::Ref(rf) => {
                        if (k >> rf.pos) != rf.prefix { conflict = true; break; }
                        let c = rf.digit(k);
                        rf.sizes.add(c, 1);
                        rf.total += 1;
                        n = &mut rf.children[c];
                    }
                }
            }
        }
        if conflict {
            // vrati brojače koji su već povećani na putanji, pa obnovi ceo vršni bucket sa novim ključem
            let node = std::mem::replace(&mut self.top[x], Node::Flat(Vec::new()));
            let mut v = Vec::with_capacity(Self::node_len(&node) + 1);
            Self::collapse(node, &mut v);
            let p = v.partition_point(|e| *e < (k, r));
            v.insert(p, (k, r));
            self.st.moved += v.len() as u64;
            self.st.merges += 1;
            self.top[x] = Node::Flat(v);
            self.meta[x].merges += 1;
            self.st.max_flips = self.st.max_flips.max(self.meta[x].merges);
        }
        self.maintain(x, k);
    }

    pub fn del(&mut self, k: u64, r: i64) {
        let x = RecogIndex::bucket(TOP, k);
        self.fen.add(x, -1);
        self.meta[x].touches = self.meta[x].touches.saturating_add(1);
        let mut n = &mut self.top[x];
        loop {
            match n {
                Node::Flat(v) => {
                    let p = v.partition_point(|e| *e < (k, r));
                    assert!(p < v.len() && v[p] == (k, r), "ARI: brisanje nepostojećeg para");
                    v.remove(p);
                    break;
                }
                Node::Ref(rf) => {
                    assert!((k >> rf.pos) == rf.prefix, "ARI: brisanje van prefiksa");
                    let c = rf.digit(k);
                    rf.sizes.add(c, -1);
                    rf.total -= 1;
                    n = &mut rf.children[c];
                }
            }
        }
        self.maintain(x, k);
    }

    // ---------- čitanje ----------
    pub fn find(&mut self, k: u64, h: &mut u64) -> usize {
        if self.cfg.trigger == Trigger::Effective { self.touch_on_read(k); }
        self.lookup(k, h)
    }

    /// Samo za efektivni okidač: čitanje se broji kao pristup, i može pokrenuti usavršavanje.
    #[cold]
    #[inline(never)]
    fn touch_on_read(&mut self, k: u64) {
        let x = RecogIndex::bucket(TOP, k);
        self.meta[x].touches = self.meta[x].touches.saturating_add(1);
        let len = Self::node_len(&self.top[x]);
        if len > self.cfg.high && self.meta[x].touches as usize >= len / 8 { self.maintain(x, k); }
    }

    #[inline(always)]
    fn scan(s: &[(u64, i64)], k: u64, h: &mut u64) -> usize {
        let i = s.partition_point(|e| e.0 < k);
        let mut c = 0;
        for e in &s[i..] { if e.0 != k { break; } fnv(h, e.1 as u64); c += 1; }
        c
    }

    #[inline]
    fn lookup(&self, k: u64, h: &mut u64) -> usize {
        let mut n = &self.top[RecogIndex::bucket(TOP, k)];
        // brza putanja: neusavršen bucket je običan niz, kao u A
        if let Node::Flat(s) = n { return Self::scan(s, k, h); }
        loop {
            match n {
                Node::Flat(s) => return Self::scan(s, k, h),
                Node::Ref(r) => { if (k >> r.pos) != r.prefix { return 0; } n = &r.children[r.digit(k)]; }
            }
        }
    }

    /// broj elemenata sa ključem < k (incl = false) ili ≤ k (incl = true)
    fn rank(&self, k: u64, incl: bool) -> usize {
        let x = RecogIndex::bucket(TOP, k);
        if x >= self.top.len() { return self.fen.prefix(self.top.len()) as usize; }
        let mut acc = self.fen.prefix(x) as usize;
        let mut n = &self.top[x];
        loop {
            match n {
                Node::Flat(s) => { acc += if incl { s.partition_point(|e| e.0 <= k) } else { s.partition_point(|e| e.0 < k) }; return acc; }
                Node::Ref(r) => {
                    let kp = k >> r.pos;
                    if kp < r.prefix { return acc; }
                    if kp > r.prefix { return acc + r.total; }
                    let c = r.digit(k);
                    acc += r.sizes.prefix(c);
                    n = &r.children[c];
                }
            }
        }
    }
    /// broj elemenata čvora sa ključem < k (incl = false) ili ≤ k (incl = true)
    #[inline(always)]
    fn node_rank(mut n: &Node, k: u64, incl: bool) -> usize {
        let mut acc = 0;
        loop {
            match n {
                Node::Flat(s) => return acc + if incl { s.partition_point(|e| e.0 <= k) } else { s.partition_point(|e| e.0 < k) },
                Node::Ref(r) => {
                    let kp = k >> r.pos;
                    if kp < r.prefix { return acc; }
                    if kp > r.prefix { return acc + r.total; }
                    let c = r.digit(k);
                    acc += r.sizes.prefix(c);
                    n = &r.children[c];
                }
            }
        }
    }
    /// Kao u DynIndex-u: Fenwick samo za cele vršne bucket-e između granica.
    #[inline]
    pub fn count_range(&self, ka: u64, kb: u64) -> usize {
        if ka > kb { return 0; }
        let (ba, bb) = (RecogIndex::bucket(TOP, ka), RecogIndex::bucket(TOP, kb));
        if ba == bb { return Self::node_rank(&self.top[ba], kb, true) - Self::node_rank(&self.top[ba], ka, false); }
        let first = Self::node_len(&self.top[ba]) - Self::node_rank(&self.top[ba], ka, false);
        let middle = (self.fen.prefix(bb) - self.fen.prefix(ba + 1)) as usize;
        let last = Self::node_rank(&self.top[bb], kb, true);
        first + middle + last
    }

    fn fetch_node(n: &Node, ka: u64, kb: u64, h: &mut u64) -> usize {
        match n {
            Node::Flat(s) => {
                let lo = s.partition_point(|e| e.0 < ka);
                let hi = s.partition_point(|e| e.0 <= kb);
                let mut c = 0;
                if lo < hi { for e in &s[lo..hi] { fnv(h, e.1 as u64); c += 1; } }
                c
            }
            Node::Ref(r) => {
                let (pa, pb) = (ka >> r.pos, kb >> r.pos);
                if pb < r.prefix || pa > r.prefix { return 0; }
                let clo = if pa < r.prefix { 0 } else { r.digit(ka) };
                let chi = if pb > r.prefix { r.children.len() - 1 } else { r.digit(kb) };
                let mut c = 0;
                for i in clo..=chi { if r.sizes.get(i) > 0 { c += Self::fetch_node(&r.children[i], ka, kb, h); } }
                c
            }
        }
    }
    pub fn fetch_range(&self, ka: u64, kb: u64, h: &mut u64) -> usize {
        if ka > kb { return 0; }
        let (ba, bb) = (RecogIndex::bucket(TOP, ka), RecogIndex::bucket(TOP, kb).min(self.top.len() - 1));
        let mut c = 0;
        for x in ba..=bb { if Self::node_len(&self.top[x]) > 0 { c += Self::fetch_node(&self.top[x], ka, kb, h); } }
        c
    }

    // ---------- pomoćno ----------
    fn walk<'a>(n: &'a Node, f: &mut dyn FnMut(&'a [(u64, i64)])) {
        match n { Node::Flat(v) => f(v), Node::Ref(r) => for c in &r.children { Self::walk(c, f) } }
    }
    pub fn fingerprint(&self) -> u64 {
        let mut h = crate::FNV0;
        for t in &self.top { Self::walk(t, &mut |s| for &(k, r) in s { fnv(&mut h, k); fnv(&mut h, r as u64); }); }
        h
    }
    pub fn len(&self) -> usize { self.fen.prefix(self.top.len()) as usize }
    pub fn max_leaf(&self) -> usize {
        let mut m = 0;
        for t in &self.top { Self::walk(t, &mut |s| m = m.max(s.len())); }
        m
    }
    fn depth_of(n: &Node) -> usize { match n { Node::Flat(_) => 0, Node::Ref(r) => 1 + r.children.iter().map(Self::depth_of).max().unwrap_or(0) } }
    pub fn depth(&self) -> usize { self.top.iter().map(|t| Self::depth_of(t)).max().unwrap_or(0) }
    pub fn refined_nodes(&self) -> usize {
        fn cnt(n: &Node) -> usize { match n { Node::Flat(_) => 0, Node::Ref(r) => 1 + r.children.iter().map(cnt).sum::<usize>() } }
        self.top.iter().map(|t| cnt(t)).sum()
    }
    fn node_bytes(n: &Node) -> usize {
        match n {
            Node::Flat(v) => v.capacity() * 16,
            Node::Ref(r) => std::mem::size_of::<Ref>() + r.children.len() * std::mem::size_of::<Node>() + r.sizes.bytes()
                + r.children.iter().map(Self::node_bytes).sum::<usize>(),
        }
    }
    pub fn bytes(&self) -> usize { self.top.len() * (std::mem::size_of::<Node>() + std::mem::size_of::<Meta>()) + self.fen.bytes() + self.top.iter().map(Self::node_bytes).sum::<usize>() }

    /// Invarijante: svaki list sortiran; svaki ključ u pravom vršnom bucket-u i pravom detetu;
    /// zbir dece = roditelj; Fenwick = veličina vršnog bucket-a.
    pub fn check(&self) -> Result<(), String> {
        fn chk(n: &Node, x: usize, path_ok: &dyn Fn(u64) -> bool) -> Result<usize, String> {
            match n {
                Node::Flat(v) => {
                    for w in v.windows(2) { if w[0] >= w[1] { return Err(format!("list u bucket-u {x} nije strogo sortiran")); } }
                    for e in v { if !path_ok(e.0) { return Err(format!("ključ {} na pogrešnom putu u bucket-u {x}", e.0)); } }
                    Ok(v.len())
                }
                Node::Ref(r) => {
                    let mut sum = 0;
                    for (i, c) in r.children.iter().enumerate() {
                        let (pos, d, prefix) = (r.pos, r.d, r.prefix);
                        let ok = move |k: u64| (k >> pos) == prefix && (((k >> (pos - d)) & ((1u64 << d) - 1)) as usize) == i && path_ok(k);
                        let s = chk(c, x, &ok)?;
                        if s != r.sizes.get(i) { return Err(format!("veličina deteta {i} ({}) ≠ stvarna ({s}) u bucket-u {x}", r.sizes.get(i))); }
                        if r.sizes.prefix(i + 1) - r.sizes.prefix(i) != s { return Err(format!("prefiksni zbir dece neispravan u bucket-u {x}")); }
                        sum += s;
                    }
                    if sum != r.total { return Err(format!("zbir dece {sum} ≠ roditelj {} u bucket-u {x}", r.total)); }
                    Ok(sum)
                }
            }
        }
        for (x, t) in self.top.iter().enumerate() {
            let s = chk(t, x, &|k| RecogIndex::bucket(TOP, k) == x)?;
            let f = (self.fen.prefix(x + 1) - self.fen.prefix(x)) as usize;
            if f != s { return Err(format!("Fenwick {f} ≠ veličina {s} vršnog bucket-a {x}")); }
        }
        Ok(())
    }

    /// granice listova (najmanji i najveći ključ svakog lista u usavršenim čvorovima) — za COUNT test na granicama
    pub fn boundary_keys(&self, limit: usize) -> Vec<u64> {
        let mut out = vec![];
        for t in &self.top {
            if let Node::Ref(_) = t {
                Self::walk(t, &mut |s| if !s.is_empty() && out.len() < limit { out.push(s[0].0); out.push(s[s.len() - 1].0); });
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::find::Rng;
    use std::collections::BTreeSet;

    fn model_fp(m: &BTreeSet<(u64, i64)>) -> u64 { let mut h = crate::FNV0; for &(k, r) in m { fnv(&mut h, k); fnv(&mut h, r as u64); } h }

    #[test]
    fn ari_matches_model_all_configs() {
        for &(trig, meth, cnt, high) in &[
            (Trigger::Structural, Method::Distribute, Counts::Plain, 64usize),
            (Trigger::Structural, Method::Sort, Counts::Fenwick, 64),
            (Trigger::Effective, Method::Distribute, Counts::Plain, 64),
            (Trigger::Structural, Method::Distribute, Counts::Plain, 1024),
        ] {
            let mut rng = Rng(5);
            // grupisani ključevi + duplikati + mali ključevi
            let gen = |rng: &mut Rng| -> u64 {
                match rng.below(10) {
                    0..=5 => (1u64 << 30) + rng.below(1 << 14),      // gust grozd
                    6 => 777_777,                                   // mnogo istih
                    7 => 1 + rng.below(5000),                       // mali (nedeljivi bucket-i)
                    _ => 1 + rng.below(1u64 << 34),
                }
            };
            let mut v: Vec<(u64, i64)> = (0..40_000).map(|i| (gen(&mut rng), i)).collect();
            v.sort();
            let mut cfg = AriCfg::new(high); cfg.trigger = trig; cfg.method = meth; cfg.counts = cnt;
            let mut a = Ari::new(&v, cfg);
            let mut m: BTreeSet<(u64, i64)> = v.iter().copied().collect();
            a.check().unwrap();
            let mut next = 1_000_000i64;
            for step in 0..40_000 {
                match rng.below(5) {
                    0 | 1 => { let k = gen(&mut rng); a.ins(k, next); m.insert((k, next)); next += 1; }
                    2 => { let i = rng.below(m.len() as u64) as usize; if let Some(&e) = m.iter().nth(i % 64) { a.del(e.0, e.1); m.remove(&e); } }
                    3 => { let k = gen(&mut rng); let mut h1 = crate::FNV0; let mut h2 = crate::FNV0;
                           let c = a.find(k, &mut h1);
                           let mut c2 = 0; for e in m.range((k, i64::MIN)..=(k, i64::MAX)) { fnv(&mut h2, e.1 as u64); c2 += 1; }
                           assert_eq!((c, h1), (c2, h2), "find korak {step}"); }
                    _ => { let ka = gen(&mut rng); let kb = ka.saturating_add(rng.below(1 << 16));
                           let want = m.range((ka, i64::MIN)..=(kb, i64::MAX)).count();
                           assert_eq!(a.count_range(ka, kb), want, "count korak {step}");
                           let (mut h1, mut h2) = (crate::FNV0, crate::FNV0);
                           assert_eq!(a.fetch_range(ka, kb, &mut h1), want);
                           for e in m.range((ka, i64::MIN)..=(kb, i64::MAX)) { fnv(&mut h2, e.1 as u64); }
                           assert_eq!(h1, h2, "fetch korak {step}"); }
                }
                if step % 5000 == 0 { a.check().unwrap(); }
            }
            a.check().unwrap();
            assert_eq!(a.fingerprint(), model_fp(&m));
            assert_eq!(a.len(), m.len());
            // granice listova
            for &b in &a.boundary_keys(2000) {
                for (ka, kb) in [(b, b), (b.saturating_sub(1), b), (b, b + 1), (b.saturating_sub(5), b + 5)] {
                    assert_eq!(a.count_range(ka, kb), m.range((ka, i64::MIN)..=(kb, i64::MAX)).count(), "granica {ka}..{kb}");
                }
            }
            if high == 64 { assert!(a.refined_nodes() > 0, "test mora da izazove usavršavanje ({trig:?})"); }
            assert_eq!(a.st.locality_violations, 0);
        }
    }
}

#[cfg(test)]
mod perf_probe {
    use super::*;
    use crate::find::Rng;
    #[test]
    #[ignore]
    fn probe() {
        println!("size_of Node = {}, Vec = {}", std::mem::size_of::<Node>(), std::mem::size_of::<Vec<(u64, i64)>>());
        let mut r = Rng(1);
        let mut v: Vec<(u64, i64)> = (0..300_000).map(|i| (1 + r.below(1u64 << 34), i)).collect();
        v.sort();
        let q: Vec<u64> = (0..200_000).map(|i| if i % 2 == 0 { v[r.below(v.len() as u64) as usize].0 } else { 1 + r.below(1u64 << 34) }).collect();
        let d = crate::dynidx::DynIndex::new(&v);
        let mut a = Ari::new(&v, AriCfg::new(4096));
        for round in 0..4 {
            let t = Instant::now(); let mut h = crate::FNV0; for &k in &q { d.find(k, &mut h); } let td = t.elapsed();
            let t = Instant::now(); let mut h2 = crate::FNV0; for &k in &q { a.find(k, &mut h2); } let ta = t.elapsed();
            let (mut s1, mut s2) = (0, 0);
            let (tdc, tac);
            if round % 2 == 0 {
                let t = Instant::now(); for &k in &q { s1 += d.count_range(k, k + (k >> 7)); } tdc = t.elapsed();
                let t = Instant::now(); for &k in &q { s2 += a.count_range(k, k + (k >> 7)); } tac = t.elapsed();
            } else {
                let t = Instant::now(); for &k in &q { s2 += a.count_range(k, k + (k >> 7)); } tac = t.elapsed();
                let t = Instant::now(); for &k in &q { s1 += d.count_range(k, k + (k >> 7)); } tdc = t.elapsed();
            }
            let t = Instant::now(); let mut s3 = 0usize; for &k in &q { let x = RecogIndex::bucket(TOP, k); s3 += Ari::node_len(&a.top[x]); } let tl = t.elapsed();
            let t = Instant::now(); let mut s4 = 0i64; for &k in &q { let x = RecogIndex::bucket(TOP, k); s4 += a.fen.prefix(x); } let tf = t.elapsed();
            println!("  node_len {:?} fen.prefix {:?} ({s3} {s4})", tl / 200, tf / 200);
            assert_eq!(h, h2); assert_eq!(s1, s2);
            println!("runda {round}: find A {:?} ARI {:?} | count A {:?} ARI {:?}", td / 200, ta / 200, tdc / 200, tac / 200);
        }
    }
}
