//! Korak 5: Recognition Index korišćen IZ SQLite-a.
//!
//! Isti SQL upiti nad:
//!   (a) virtualnom tabelom `recog` (eponimna; statični Recognition Index Log(12) ispod),
//!   (b) običnom tabelom `t(k)` sa SQLite B-tree indeksom `i` (u memoriji),
//! uz (c) direktan poziv strukture iz Rust-a kao referencu bez SQL-a.
//! Plus SQL funkcija `recog_count(a, b)` — struktura sama vraća broj pogodaka u opsegu.
//!
//! Virtualna tabela prihvata ograničenja nad kolonom k: =, >=, >, <=, < (BETWEEN = >= i <=),
//! vraća redove u redosledu (k, rowid) i javlja SQLite-u da ih sam ne proverava ponovo.

use crate::find::{make_queries, make_ranges, Mode, RecogIndex, CLASSES};
use crate::lens_radix::radix_sort_by_key;
use crate::{fnv, key, med, ms, FNV0};
use rusqlite::functions::FunctionFlags;
use rusqlite::vtab::{eponymous_only_module, Context, IndexConstraintOp, IndexInfo, VTab, VTabConnection, VTabCursor, Values};
use rusqlite::{ffi, params, Connection};
use std::os::raw::c_int;
use std::sync::Arc;
use std::time::Instant;

const MODE: Mode = Mode::Log(12);

pub struct VData { sorted: Vec<(u64, i64)>, idx: RecogIndex, root: i64 }

impl VData {
    /// Granice opsega ključeva [lo, hi) u sortiranom nizu za uključive ključeve [ka, kb].
    fn span(&self, ka: u64, kb: u64) -> (usize, usize) {
        if ka > kb { return (0, 0); }
        (self.idx.pos(&self.sorted, ka, false), self.idx.pos(&self.sorted, kb, true))
    }
    /// SQL vrednost → ključ, sa odsecanjem: vrednosti ispod korena daju ključ 0 (iza NULL-a).
    fn vkey(&self, v: i64) -> i128 { v as i128 - self.root as i128 + 1 }
}

const B_EQ: c_int = 1;
const B_GE: c_int = 2;
const B_GT: c_int = 4;
const B_LE: c_int = 8;
const B_LT: c_int = 16;

#[repr(C)]
struct RecogTab { base: ffi::sqlite3_vtab, data: Arc<VData> }

unsafe impl<'vtab> VTab<'vtab> for RecogTab {
    type Aux = Arc<VData>;
    type Cursor = RecogCursor<'vtab>;

    fn connect(_db: &mut VTabConnection, aux: Option<&Arc<VData>>, _args: &[&[u8]]) -> rusqlite::Result<(String, Self)> {
        let data = aux.expect("recog: nedostaju podaci").clone();
        Ok(("CREATE TABLE x(k INTEGER)".to_owned(), RecogTab { base: ffi::sqlite3_vtab::default(), data }))
    }

    fn best_index(&self, info: &mut IndexInfo) -> rusqlite::Result<()> {
        // prvi upotrebljiv uslov za svaku operaciju nad kolonom 0
        let order = [
            (IndexConstraintOp::SQLITE_INDEX_CONSTRAINT_EQ, B_EQ),
            (IndexConstraintOp::SQLITE_INDEX_CONSTRAINT_GE, B_GE),
            (IndexConstraintOp::SQLITE_INDEX_CONSTRAINT_GT, B_GT),
            (IndexConstraintOp::SQLITE_INDEX_CONSTRAINT_LE, B_LE),
            (IndexConstraintOp::SQLITE_INDEX_CONSTRAINT_LT, B_LT),
        ];
        let mut found: [Option<usize>; 5] = [None; 5];
        for (i, c) in info.constraints().enumerate() {
            if c.column() != 0 || !c.is_usable() { continue; }
            for (j, (op, _)) in order.iter().enumerate() {
                if c.operator() == *op && found[j].is_none() { found[j] = Some(i); }
            }
        }
        let mut idx_num = 0;
        let mut argv = 1;
        for (j, f) in found.iter().enumerate() {
            if let Some(ci) = f {
                let mut u = info.constraint_usage(*ci);
                u.set_argv_index(argv);
                u.set_omit(true);
                argv += 1;
                idx_num |= order[j].1;
            }
        }
        info.set_idx_num(idx_num);
        let cost = if idx_num & B_EQ != 0 { 10.0 } else if idx_num != 0 { 100.0 } else { 1e7 };
        info.set_estimated_cost(cost);
        let ob: Vec<(c_int, bool)> = info.order_bys().map(|o| (o.column(), o.is_order_by_desc())).collect();
        if ob.len() == 1 && ob[0] == (0, false) { info.set_order_by_consumed(true); }
        Ok(())
    }

    fn open(&'vtab mut self) -> rusqlite::Result<RecogCursor<'vtab>> {
        Ok(RecogCursor { base: ffi::sqlite3_vtab_cursor::default(), data: &self.data, pos: 0, end: 0 })
    }
}

#[repr(C)]
struct RecogCursor<'vtab> { base: ffi::sqlite3_vtab_cursor, data: &'vtab VData, pos: usize, end: usize }

unsafe impl VTabCursor for RecogCursor<'_> {
    fn filter(&mut self, idx_num: c_int, _idx_str: Option<&str>, args: &Values<'_>) -> rusqlite::Result<()> {
        let d = self.data;
        let maxk = d.sorted.last().map(|e| e.0).unwrap_or(0) as i128;
        let mut lo: i128 = 1; // NULL (ključ 0) nikad ne zadovoljava poređenje
        let mut hi: i128 = maxk;
        let mut a = 0;
        let mut next = || { let v: Option<i64> = args.get(a).ok().flatten(); a += 1; v };
        if idx_num & B_EQ != 0 { match next() { Some(v) => { let k = d.vkey(v); lo = lo.max(k); hi = hi.min(k); } None => { lo = 1; hi = 0; } } }
        if idx_num & B_GE != 0 { match next() { Some(v) => lo = lo.max(d.vkey(v)), None => { lo = 1; hi = 0; } } }
        if idx_num & B_GT != 0 { match next() { Some(v) => lo = lo.max(d.vkey(v) + 1), None => { lo = 1; hi = 0; } } }
        if idx_num & B_LE != 0 { match next() { Some(v) => hi = hi.min(d.vkey(v)), None => { lo = 1; hi = 0; } } }
        if idx_num & B_LT != 0 { match next() { Some(v) => hi = hi.min(d.vkey(v) - 1), None => { lo = 1; hi = 0; } } }
        if idx_num == 0 {
            // bez uslova: ceo niz, uključujući NULL-ove
            self.pos = 0; self.end = d.sorted.len();
            return Ok(());
        }
        if lo > hi || hi < 1 { self.pos = 0; self.end = 0; return Ok(()); }
        let (p, e) = d.span(lo.max(1) as u64, hi.min(u64::MAX as i128) as u64);
        self.pos = p; self.end = e;
        Ok(())
    }
    fn next(&mut self) -> rusqlite::Result<()> { self.pos += 1; Ok(()) }
    fn eof(&self) -> bool { self.pos >= self.end }
    fn column(&self, ctx: &mut Context, i: c_int) -> rusqlite::Result<()> {
        if i != 0 { return ctx.set_result(&rusqlite::types::Null); }
        let k = self.data.sorted[self.pos].0;
        if k == 0 { ctx.set_result(&rusqlite::types::Null) } else { ctx.set_result(&(((k - 1) as i128 + self.data.root as i128) as i64)) }
    }
    fn rowid(&self) -> rusqlite::Result<i64> { Ok(self.data.sorted[self.pos].1) }
}

/// Registruje virtualnu tabelu `recog` i funkciju `recog_count(a, b)` na konekciji.
pub fn register(c: &Connection, data: Arc<VData>) -> rusqlite::Result<()> {
    c.create_module("recog", eponymous_only_module::<RecogTab>(), Some(data.clone()))?;
    c.create_scalar_function("recog_count", 2, FunctionFlags::SQLITE_UTF8 | FunctionFlags::SQLITE_DETERMINISTIC, move |ctx| {
        let a: Option<i64> = ctx.get(0)?;
        let b: Option<i64> = ctx.get(1)?;
        let (Some(a), Some(b)) = (a, b) else { return Ok(0i64) };
        let maxk = data.sorted.last().map(|e| e.0).unwrap_or(0) as i128;
        let lo = data.vkey(a).max(1);
        let hi = data.vkey(b).min(maxk);
        if lo > hi { return Ok(0); }
        let (p, e) = data.span(lo as u64, hi as u64);
        Ok((e - p) as i64)
    })
}

pub fn build_data(keyed: &[(u64, i64)], root: i64) -> Arc<VData> {
    let pool1 = rayon::ThreadPoolBuilder::new().num_threads(1).build().unwrap();
    let mut sorted = keyed.to_vec();
    pool1.install(|| { let mut s = Vec::new(); radix_sort_by_key(&mut sorted, &mut s, |e| e.0); });
    let idx = RecogIndex::build(&sorted, MODE);
    Arc::new(VData { sorted, idx, root })
}

/// Isti obrazac otiska svuda: fnv nad rowid-ovima jednog upita, pa fnv nad brojem.
fn sql_rows(c: &Connection, sql: &str, args: &[(i64, Option<i64>)], reps: usize) -> (f64, u64) {
    let mut st = c.prepare(sql).unwrap();
    let mut t = vec![]; let mut fp = 0;
    for _ in 0..reps {
        let t0 = Instant::now();
        let mut h = FNV0;
        for &(a, b) in args {
            let mut rows = match b { None => st.query(params![a]).unwrap(), Some(b) => st.query(params![a, b]).unwrap() };
            let mut n = 0u64;
            while let Some(r) = rows.next().unwrap() { let id: i64 = r.get(0).unwrap(); fnv(&mut h, id as u64); n += 1; }
            fnv(&mut h, n);
        }
        t.push(ms(t0.elapsed())); fp = h;
    }
    (med(&t).0, fp)
}
fn sql_count(c: &Connection, sql: &str, args: &[(i64, i64)], reps: usize) -> (f64, u64) {
    let mut st = c.prepare(sql).unwrap();
    let mut t = vec![]; let mut fp = 0;
    for _ in 0..reps {
        let t0 = Instant::now();
        let mut h = FNV0;
        for &(a, b) in args { let n: i64 = st.query_row(params![a, b], |r| r.get(0)).unwrap(); fnv(&mut h, n as u64); }
        t.push(ms(t0.elapsed())); fp = h;
    }
    (med(&t).0, fp)
}

pub fn step5(o: &mut String, raw: &[(Option<i64>, i64)], keyed: &[(u64, i64)], root: i64, reps: usize, nq: usize, per_class: usize) {
    out!(o, "\n## KORAK 5 — Recognition Index korišćen IZ SQLite-a (virtualna tabela), sve u 1 niti");
    let data = build_data(keyed, root);
    let c = Connection::open_in_memory().unwrap();
    c.execute_batch("PRAGMA journal_mode=OFF; PRAGMA synchronous=OFF; CREATE TABLE t(k INTEGER);").unwrap();
    c.execute_batch("BEGIN").unwrap();
    { let mut st = c.prepare("INSERT INTO t(rowid,k) VALUES(?1,?2)").unwrap(); for &(s, id) in raw { st.execute(params![id, s]).unwrap(); } }
    c.execute_batch("COMMIT; CREATE INDEX i ON t(k);").unwrap();
    register(&c, data.clone()).unwrap();

    for sql in ["SELECT rowid FROM recog WHERE k = ?1", "SELECT rowid FROM recog WHERE k BETWEEN ?1 AND ?2", "SELECT count(*) FROM recog WHERE k BETWEEN ?1 AND ?2"] {
        let lit = sql.replace("?1", "1000").replace("?2", "2000");
        let plan: Vec<String> = c.prepare(&format!("EXPLAIN QUERY PLAN {lit}")).unwrap()
            .query_map([], |r| r.get::<_, String>(3)).unwrap()
            .map(|x| x.unwrap()).collect();
        out!(o, "plan: {sql}  →  {}", plan.join(" | "));
    }

    // --- jednakost ---
    let queries = make_queries(raw, &data.sorted, root, nq);
    let qargs: Vec<(i64, Option<i64>)> = queries.iter().map(|&v| (v, None)).collect();
    let mut dt = vec![]; let mut dfp = 0;
    for _ in 0..reps {
        let t0 = Instant::now();
        let mut h = FNV0;
        for &v in &queries {
            let k = key(Some(v), root);
            let (p, e) = data.span(k, k);
            for x in &data.sorted[p..e] { fnv(&mut h, x.1 as u64); }
            fnv(&mut h, (e - p) as u64);
        }
        dt.push(ms(t0.elapsed())); dfp = h;
    }
    let (d_eq, _) = (med(&dt).0, 0);
    let (v_eq, vfp) = sql_rows(&c, "SELECT rowid FROM recog WHERE k = ?1", &qargs, reps);
    let (b_eq, bfp) = sql_rows(&c, "SELECT rowid FROM t INDEXED BY i WHERE k = ?1", &qargs, reps);
    let ns = |m: f64, n: usize| m * 1e6 / n as f64;
    out!(o, "\n### Jednakost `WHERE k = ?`, {nq} upita — ns po upitu");
    out!(o, "| način | ns po upitu | odnos prema B-tree kroz SQL | isti rezultat |");
    out!(o, "|---|---|---|---|");
    out!(o, "| Recognition Index direktno iz Rust-a (bez SQL-a) | {:.0} | {:.2}× | {} |", ns(d_eq, nq), d_eq / b_eq, if dfp == bfp { "DA" } else { "NE" });
    out!(o, "| Recognition Index kroz virtualnu tabelu (SQL) | {:.0} | {:.2}× | {} |", ns(v_eq, nq), v_eq / b_eq, if vfp == bfp { "DA" } else { "NE" });
    out!(o, "| SQLite B-tree kroz SQL | {:.0} | 1.00× | — |", ns(b_eq, nq));

    // --- opsezi ---
    let ranges = make_ranges(raw, &data.sorted, root, per_class);
    out!(o, "\n### Opsezi `BETWEEN`, {per_class} po klasi — ns po upitu (odnos prema B-tree kroz SQL)");
    let mut base_c = vec![]; let mut base_f = vec![];
    let mut cnt_rows: Vec<(String, Vec<(f64, u64)>)> = vec![];
    let mut fet_rows: Vec<(String, Vec<(f64, u64)>)> = vec![];
    for cl in &ranges {
        let a2: Vec<(i64, i64)> = cl.clone();
        let a2o: Vec<(i64, Option<i64>)> = cl.iter().map(|&(a, b)| (a, Some(b))).collect();
        // direktno
        let mut tc = vec![]; let mut tf = vec![]; let (mut fc, mut ff) = (0, 0);
        for _ in 0..reps {
            let t0 = Instant::now(); let mut h = FNV0;
            for &(a, b) in &a2 { let (p, e) = data.span(key(Some(a), root), key(Some(b), root)); fnv(&mut h, (e - p) as u64); }
            tc.push(ms(t0.elapsed())); fc = h;
            let t0 = Instant::now(); let mut h = FNV0;
            for &(a, b) in &a2 { let (p, e) = data.span(key(Some(a), root), key(Some(b), root)); for x in &data.sorted[p..e] { fnv(&mut h, x.1 as u64); } fnv(&mut h, (e - p) as u64); }
            tf.push(ms(t0.elapsed())); ff = h;
        }
        let bc = sql_count(&c, "SELECT count(*) FROM t INDEXED BY i WHERE k BETWEEN ?1 AND ?2", &a2, reps);
        let bf = sql_rows(&c, "SELECT rowid FROM t INDEXED BY i WHERE k BETWEEN ?1 AND ?2", &a2o, reps);
        base_c.push(bc); base_f.push(bf);
        let entries_c = [
            ("Recognition direktno (bez SQL-a)", (med(&tc).0, fc)),
            ("SQL funkcija recog_count(a, b)", sql_count(&c, "SELECT recog_count(?1, ?2)", &a2, reps)),
            ("count(*) kroz virtualnu tabelu", sql_count(&c, "SELECT count(*) FROM recog WHERE k BETWEEN ?1 AND ?2", &a2, reps)),
            ("count(*) nad SQLite B-tree", bc),
        ];
        for (n, v) in entries_c {
            match cnt_rows.iter_mut().find(|r| r.0 == n) { Some(r) => r.1.push(v), None => cnt_rows.push((n.to_string(), vec![v])) }
        }
        let entries_f = [
            ("Recognition direktno (bez SQL-a)", (med(&tf).0, ff)),
            ("rowid-ovi kroz virtualnu tabelu", sql_rows(&c, "SELECT rowid FROM recog WHERE k BETWEEN ?1 AND ?2", &a2o, reps)),
            ("rowid-ovi nad SQLite B-tree", bf),
        ];
        for (n, v) in entries_f {
            match fet_rows.iter_mut().find(|r| r.0 == n) { Some(r) => r.1.push(v), None => fet_rows.push((n.to_string(), vec![v])) }
        }
    }
    out!(o, "**COUNT**");
    out!(o, "| način | {} | {} | {} | isti rezultat |", CLASSES[0].0, CLASSES[1].0, CLASSES[2].0);
    out!(o, "|---|---|---|---|---|");
    for (n, v) in &cnt_rows {
        let ok = v.iter().zip(&base_c).all(|(a, b)| a.1 == b.1);
        let cells: Vec<String> = v.iter().zip(&base_c).map(|(a, b)| format!("{:.0} ({:.3}×)", ns(a.0, per_class), a.0 / b.0)).collect();
        out!(o, "| {n} | {} | {} | {} | {} |", cells[0], cells[1], cells[2], if ok { "DA" } else { "NE" });
    }
    out!(o, "\n**FETCH (svi rowid-ovi)**");
    out!(o, "| način | {} | {} | {} | isti rezultat |", CLASSES[0].0, CLASSES[1].0, CLASSES[2].0);
    out!(o, "|---|---|---|---|---|");
    for (n, v) in &fet_rows {
        let ok = v.iter().zip(&base_f).all(|(a, b)| a.1 == b.1);
        let cells: Vec<String> = v.iter().zip(&base_f).map(|(a, b)| format!("{:.0} ({:.2}×)", ns(a.0, per_class), a.0 / b.0)).collect();
        out!(o, "| {n} | {} | {} | {} | {} |", cells[0], cells[1], cells[2], if ok { "DA" } else { "NE" });
    }
    out!(o, "\nNAPOMENE:");
    out!(o, "- virtualna tabela i B-tree koriste isti SQL i istu konekciju (:memory:); razlika je samo struktura ispod.");
    out!(o, "- count(*) kroz virtualnu tabelu: SQLite traži red po red (xNext) i sam broji; recog_count vraća broj iz strukture.");
    out!(o, "- predviđanje zapisano pre merenja: D:\\sqliteRecog\\PREDVIDJANJE_vtab.md");
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn vtab_matches_btree() {
        let mut x = 7u64;
        let raw: Vec<(Option<i64>, i64)> = (1..=60_000).map(|i| { x ^= x << 13; x ^= x >> 7; x ^= x << 17; (if i % 997 == 0 { None } else { Some((x % 5_000) as i64 * (x % 7) as i64 + 3) }, i) }).collect();
        let root = raw.iter().filter_map(|e| e.0).min().unwrap();
        let keyed: Vec<(u64, i64)> = raw.iter().map(|&(s, r)| (key(s, root), r)).collect();
        let data = build_data(&keyed, root);
        let c = Connection::open_in_memory().unwrap();
        c.execute_batch("CREATE TABLE t(k INTEGER);").unwrap();
        { let mut st = c.prepare("INSERT INTO t(rowid,k) VALUES(?1,?2)").unwrap(); for &(s, id) in &raw { st.execute(params![id, s]).unwrap(); } }
        c.execute_batch("CREATE INDEX i ON t(k);").unwrap();
        register(&c, data).unwrap();
        let q = |sql: &str| -> Vec<(Option<i64>, i64)> { c.prepare(sql).unwrap().query_map([], |r| Ok((r.get(0)?, r.get(1)?))).unwrap().map(|x| x.unwrap()).collect() };
        for w in ["k = 9", "k = -5", "k BETWEEN 100 AND 900", "k > 30000", "k >= 30000", "k < 10", "k <= 10", "k > 100 AND k < 105", "k BETWEEN 900 AND 100", "k = 3", "k IS NULL"] {
            let a = q(&format!("SELECT k, rowid FROM recog WHERE {w} ORDER BY k, rowid"));
            let b = q(&format!("SELECT k, rowid FROM t WHERE {w} ORDER BY k, rowid"));
            assert_eq!(a, b, "uslov {w}");
        }
        let a = q("SELECT k, rowid FROM recog ORDER BY k, rowid");
        let b = q("SELECT k, rowid FROM t ORDER BY k, rowid");
        assert_eq!(a, b);
        for (lo, hi) in [(100i64, 900i64), (-10, 5), (30000, 40000), (900, 100), (3, 3)] {
            let a: i64 = c.query_row("SELECT recog_count(?1, ?2)", params![lo, hi], |r| r.get(0)).unwrap();
            let b: i64 = c.query_row("SELECT count(*) FROM t WHERE k BETWEEN ?1 AND ?2", params![lo, hi], |r| r.get(0)).unwrap();
            assert_eq!(a, b, "recog_count({lo},{hi})");
        }
    }
}
