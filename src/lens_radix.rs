//! Port radix sorta iz My Radix LENS (suit/src/radix_sort.rs, `radix_sort_u32`).
//!
//! Algoritam je isti, korak po korak: paralelni histogram po delu niza, prefiksne
//! sume tako da deo c dobija svoj opseg u svakom bucket-u, paralelno raspoređivanje
//! u pomoćni bafer kroz disjunktne pozicije, preskakanje prolaza u kome su sve
//! vrednosti u istom bucket-u, i vraćanje rezultata u ulazni niz na kraju.
//!
//! Jedina razlika: element nije `u32`, nego proizvoljan `T` sa ključem `u64`
//! (ovde par (ključ, rowid)), pa je broj prolaza 8 umesto 4 — kao što bi bilo
//! za u64 u originalu. Prolazi u kojima je ceo bajt isti preskaču se istom logikom.

use rayon::prelude::*;

const RADIX_BITS: u32 = 8;
const BUCKETS: usize = 1 << RADIX_BITS;
const PASSES: u32 = 64 / RADIX_BITS;

#[derive(Clone, Copy)]
struct SendPtr<T>(*mut T);
unsafe impl<T> Send for SendPtr<T> {}
unsafe impl<T> Sync for SendPtr<T> {}

/// Vraća broj stvarno izvršenih (nepreskočenih) prolaza.
pub fn radix_sort_by_key<T, F>(data: &mut [T], scratch: &mut Vec<T>, key: F) -> u32
where
    T: Copy + Send + Sync + Default,
    F: Fn(&T) -> u64 + Sync,
{
    let n = data.len();
    if n < 2 {
        return 0;
    }
    if scratch.len() < n {
        scratch.resize(n, T::default());
    }
    let scratch = &mut scratch[..n];
    let key = &key;

    let threads = rayon::current_num_threads().max(1);
    let chunk = ((n + threads - 1) / threads).max(1 << 16);

    let mut src_is_data = true;
    let mut done = 0;
    for pass in 0..PASSES {
        let shift = pass * RADIX_BITS;
        let (src, dst): (&[T], &mut [T]) = if src_is_data {
            (&*data, &mut *scratch)
        } else {
            (&*scratch, &mut *data)
        };

        let hists: Vec<[usize; BUCKETS]> = src
            .par_chunks(chunk)
            .map(|c| {
                let mut h = [0usize; BUCKETS];
                for v in c {
                    h[((key(v) >> shift) as usize) & (BUCKETS - 1)] += 1;
                }
                h
            })
            .collect();

        let mut totals = [0usize; BUCKETS];
        for h in &hists {
            for b in 0..BUCKETS {
                totals[b] += h[b];
            }
        }
        if totals.iter().any(|&t| t == n) {
            continue;
        }

        let mut offsets: Vec<[usize; BUCKETS]> = vec![[0usize; BUCKETS]; hists.len()];
        let mut running = 0usize;
        for b in 0..BUCKETS {
            for (c, h) in hists.iter().enumerate() {
                offsets[c][b] = running;
                running += h[b];
            }
        }
        debug_assert_eq!(running, n);

        let out = SendPtr(dst.as_mut_ptr());
        src.par_chunks(chunk)
            .zip(offsets.into_par_iter())
            .for_each(move |(c, mut off)| {
                let out = out;
                for v in c {
                    let b = ((key(v) >> shift) as usize) & (BUCKETS - 1);
                    // SAFETY: opsezi su disjunktni i ukupno pokrivaju 0..n (kao u originalu)
                    unsafe { *out.0.add(off[b]) = *v; }
                    off[b] += 1;
                }
            });

        src_is_data = !src_is_data;
        done += 1;
    }

    if !src_is_data {
        data.copy_from_slice(scratch);
    }
    done
}

/// EKSPERIMENT „treći blok“ (IDEJA_treci_blok.md, hipoteza 1).
///
/// Isto kao `radix_sort_by_key`, osim faze raspoređivanja: između izvora i odredišta
/// svaka nit ima mali lokalni blok od W elemenata po bucket-u. Element ide prvo u blok;
/// kad se blok napuni, ceo se prepiše u odredište odjednom (grupisan upis).
/// Na kraju dela niza preostali delimični blokovi se isprazne. Redosled unutar bucket-a
/// ostaje isti (blok je FIFO), pa je sort i dalje stabilan.
pub fn radix_sort_by_key_wc<T, F, const W: usize>(data: &mut [T], scratch: &mut Vec<T>, key: F) -> u32
where
    T: Copy + Send + Sync + Default,
    F: Fn(&T) -> u64 + Sync,
{
    let n = data.len();
    if n < 2 {
        return 0;
    }
    if scratch.len() < n {
        scratch.resize(n, T::default());
    }
    let scratch = &mut scratch[..n];
    let key = &key;

    let threads = rayon::current_num_threads().max(1);
    let chunk = ((n + threads - 1) / threads).max(1 << 16);

    let mut src_is_data = true;
    let mut done = 0;
    for pass in 0..PASSES {
        let shift = pass * RADIX_BITS;
        let (src, dst): (&[T], &mut [T]) = if src_is_data {
            (&*data, &mut *scratch)
        } else {
            (&*scratch, &mut *data)
        };

        let hists: Vec<[usize; BUCKETS]> = src
            .par_chunks(chunk)
            .map(|c| {
                let mut h = [0usize; BUCKETS];
                for v in c {
                    h[((key(v) >> shift) as usize) & (BUCKETS - 1)] += 1;
                }
                h
            })
            .collect();

        let mut totals = [0usize; BUCKETS];
        for h in &hists {
            for b in 0..BUCKETS {
                totals[b] += h[b];
            }
        }
        if totals.iter().any(|&t| t == n) {
            continue;
        }

        let mut offsets: Vec<[usize; BUCKETS]> = vec![[0usize; BUCKETS]; hists.len()];
        let mut running = 0usize;
        for b in 0..BUCKETS {
            for (c, h) in hists.iter().enumerate() {
                offsets[c][b] = running;
                running += h[b];
            }
        }

        let out = SendPtr(dst.as_mut_ptr());
        src.par_chunks(chunk)
            .zip(offsets.into_par_iter())
            .for_each(move |(c, mut off)| {
                let out = out;
                // treći blok: W elemenata po bucket-u, lokalno za ovu nit
                let mut buf: Vec<T> = vec![T::default(); BUCKETS * W];
                let mut fill = [0usize; BUCKETS];
                for v in c {
                    let b = ((key(v) >> shift) as usize) & (BUCKETS - 1);
                    let f = fill[b];
                    buf[b * W + f] = *v;
                    if f + 1 == W {
                        // SAFETY: [off[b], off[b]+W) je unutar opsega ovog dela za bucket b
                        unsafe { std::ptr::copy_nonoverlapping(buf.as_ptr().add(b * W), out.0.add(off[b]), W); }
                        off[b] += W;
                        fill[b] = 0;
                    } else {
                        fill[b] = f + 1;
                    }
                }
                for b in 0..BUCKETS {
                    let f = fill[b];
                    if f > 0 {
                        unsafe { std::ptr::copy_nonoverlapping(buf.as_ptr().add(b * W), out.0.add(off[b]), f); }
                        off[b] += f;
                    }
                }
            });

        src_is_data = !src_is_data;
        done += 1;
    }

    if !src_is_data {
        data.copy_from_slice(scratch);
    }
    done
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn wc_matches_plain() {
        let mut x = 0x1234_5678_9abc_def0u64;
        let mut v: Vec<(u64, i64)> = (0..700_000).map(|i| { x ^= x << 13; x ^= x >> 7; x ^= x << 17; (x % 50_000_000, i) }).collect();
        v[5].0 = u64::MAX;
        let mut a = v.clone();
        let mut b = v.clone();
        let mut c = v.clone();
        v.sort_by_key(|e| e.0);
        let pool = rayon::ThreadPoolBuilder::new().num_threads(4).build().unwrap();
        pool.install(|| {
            radix_sort_by_key(&mut a, &mut Vec::new(), |e| e.0);
            radix_sort_by_key_wc::<_, _, 16>(&mut b, &mut Vec::new(), |e| e.0);
            radix_sort_by_key_wc::<_, _, 3>(&mut c, &mut Vec::new(), |e| e.0);
        });
        assert_eq!(a, v);
        assert_eq!(b, v);
        assert_eq!(c, v);
    }
}

/// v0.9 — „spojen histogram“ (vidi IDEJA_treci_blok.md, hipoteza o protoku memorije).
///
/// Isti LSD algoritam kao `radix_sort_by_key`, sa dve razlike u tome KADA se broji:
///  1. Jedan početni paralelni prolaz broji SVIH 8 bajtova odjednom (umesto 8 prolaza),
///     pa se odmah zna koji se prolazi preskaču.
///  2. Histogram za SLEDEĆI aktivni prolaz računa se tokom raspoređivanja: kad nit upiše
///     element na poziciju p odredišta, poveća brojač dela p / chunk za bajt sledećeg prolaza.
///     Tako za prolaze 2..k nema posebnog čitanja niza radi brojanja.
/// Delovi su veličine stepena dvojke (p >> shift umesto deljenja).
pub fn radix_sort_by_key_fused<T, F>(data: &mut [T], scratch: &mut Vec<T>, key: F) -> u32
where
    T: Copy + Send + Sync + Default,
    F: Fn(&T) -> u64 + Sync,
{
    let n = data.len();
    if n < 2 {
        return 0;
    }
    if scratch.len() < n {
        scratch.resize(n, T::default());
    }
    let scratch = &mut scratch[..n];
    let key = &key;

    let threads = rayon::current_num_threads().max(1);
    let chunk = ((n + threads - 1) / threads).max(1 << 16).next_power_of_two();
    let cshift = chunk.trailing_zeros();
    let nchunks = (n + chunk - 1) / chunk;

    // 1) jedan prolaz: histogram svih 8 bajtova po delu
    let all: Vec<Vec<[usize; BUCKETS]>> = data
        .par_chunks(chunk)
        .map(|c| {
            let mut h = vec![[0usize; BUCKETS]; PASSES as usize];
            for v in c {
                let k = key(v);
                for p in 0..PASSES as usize {
                    h[p][((k >> (p as u32 * RADIX_BITS)) as usize) & (BUCKETS - 1)] += 1;
                }
            }
            h
        })
        .collect();
    let active: Vec<u32> = (0..PASSES)
        .filter(|&p| {
            let mut tot = [0usize; BUCKETS];
            for h in &all { for b in 0..BUCKETS { tot[b] += h[p as usize][b]; } }
            !tot.iter().any(|&t| t == n)
        })
        .collect();
    if active.is_empty() {
        return 0;
    }
    let mut cur: Vec<[usize; BUCKETS]> = all.iter().map(|h| h[active[0] as usize]).collect();
    drop(all);

    let mut src_is_data = true;
    for (ai, &pass) in active.iter().enumerate() {
        let shift = pass * RADIX_BITS;
        let next_shift = active.get(ai + 1).map(|&p| p * RADIX_BITS);
        let (src, dst): (&[T], &mut [T]) = if src_is_data {
            (&*data, &mut *scratch)
        } else {
            (&*scratch, &mut *data)
        };

        let mut offsets: Vec<[usize; BUCKETS]> = vec![[0usize; BUCKETS]; cur.len()];
        let mut running = 0usize;
        for b in 0..BUCKETS {
            for (c, h) in cur.iter().enumerate() {
                offsets[c][b] = running;
                running += h[b];
            }
        }
        debug_assert_eq!(running, n);

        let out = SendPtr(dst.as_mut_ptr());
        let partial: Vec<Vec<[usize; BUCKETS]>> = src
            .par_chunks(chunk)
            .zip(offsets.into_par_iter())
            .map(move |(c, mut off)| {
                let out = out;
                let mut nh: Vec<[usize; BUCKETS]> = if next_shift.is_some() { vec![[0usize; BUCKETS]; nchunks] } else { Vec::new() };
                match next_shift {
                    Some(ns) => {
                        for v in c {
                            let k = key(v);
                            let b = ((k >> shift) as usize) & (BUCKETS - 1);
                            let p = off[b];
                            // SAFETY: disjunktni opsezi, kao u originalu
                            unsafe { *out.0.add(p) = *v; }
                            off[b] = p + 1;
                            nh[p >> cshift][((k >> ns) as usize) & (BUCKETS - 1)] += 1;
                        }
                    }
                    None => {
                        for v in c {
                            let b = ((key(v) >> shift) as usize) & (BUCKETS - 1);
                            unsafe { *out.0.add(off[b]) = *v; }
                            off[b] += 1;
                        }
                    }
                }
                nh
            })
            .collect();

        if next_shift.is_some() {
            let mut sum = vec![[0usize; BUCKETS]; nchunks];
            for part in &partial {
                for c in 0..nchunks { for b in 0..BUCKETS { sum[c][b] += part[c][b]; } }
            }
            cur = sum;
        }
        src_is_data = !src_is_data;
    }

    if !src_is_data {
        data.copy_from_slice(scratch);
    }
    active.len() as u32
}

#[cfg(test)]
mod tests_fused {
    use super::*;
    #[test]
    fn fused_matches_sort() {
        let mut x = 0x9e37_79b9_7f4a_7c15u64;
        for &n in &[0usize, 1, 2, 3, 1000, 65_535, 65_537, 300_001, 1_200_000] {
            let mut v: Vec<(u64, i64)> = (0..n).map(|i| { x ^= x << 13; x ^= x >> 7; x ^= x << 17; (x % 40_000_000_000, i as i64) }).collect();
            if n > 10 { v[3].0 = u64::MAX; v[7].0 = 0; }
            let mut e = v.clone();
            e.sort_by_key(|p| p.0);
            for t in [1usize, 3, 9] {
                let pool = rayon::ThreadPoolBuilder::new().num_threads(t).build().unwrap();
                let mut a = v.clone();
                pool.install(|| { radix_sort_by_key_fused(&mut a, &mut Vec::new(), |p| p.0); });
                assert_eq!(a, e, "n={n} t={t}");
            }
        }
        // svi isti ključevi -> nijedan aktivan prolaz
        let mut same: Vec<(u64, i64)> = (0..100_000).map(|i| (42, i)).collect();
        let e = same.clone();
        radix_sort_by_key_fused(&mut same, &mut Vec::new(), |p| p.0);
        assert_eq!(same, e);
    }
}

/// v0.10 — spojen histogram sa promenljivom veličinom cifre (`bits`, 1..=16).
/// Isti postupak kao `radix_sort_by_key_fused`; jedina promenljiva je širina cifre,
/// pa i broj bucket-a (2^bits) i broj prolaza (ceil(64 / bits), aktivni samo oni koji menjaju redosled).
pub fn radix_sort_by_key_fused_bits<T, F>(data: &mut [T], scratch: &mut Vec<T>, bits: u32, key: F) -> u32
where
    T: Copy + Send + Sync + Default,
    F: Fn(&T) -> u64 + Sync,
{
    assert!((1..=16).contains(&bits));
    let n = data.len();
    if n < 2 {
        return 0;
    }
    if scratch.len() < n {
        scratch.resize(n, T::default());
    }
    let scratch = &mut scratch[..n];
    let key = &key;
    let nb = 1usize << bits;
    let mask = (nb - 1) as u64;
    let digits = (64 + bits - 1) / bits;

    let threads = rayon::current_num_threads().max(1);
    let chunk = ((n + threads - 1) / threads).max(1 << 16).next_power_of_two();
    let cshift = chunk.trailing_zeros();
    let nchunks = (n + chunk - 1) / chunk;

    // 1) jedan prolaz: histogram svih cifara po delu
    let all: Vec<Vec<Vec<usize>>> = data
        .par_chunks(chunk)
        .map(|c| {
            let mut h = vec![vec![0usize; nb]; digits as usize];
            for v in c {
                let k = key(v);
                for d in 0..digits as usize {
                    h[d][((k >> (d as u32 * bits)) & mask) as usize] += 1;
                }
            }
            h
        })
        .collect();
    let active: Vec<u32> = (0..digits)
        .filter(|&d| {
            let mut tot = vec![0usize; nb];
            for h in &all { for b in 0..nb { tot[b] += h[d as usize][b]; } }
            !tot.iter().any(|&t| t == n)
        })
        .collect();
    if active.is_empty() {
        return 0;
    }
    let mut cur: Vec<Vec<usize>> = all.iter().map(|h| h[active[0] as usize].clone()).collect();
    drop(all);

    let mut src_is_data = true;
    for (ai, &d) in active.iter().enumerate() {
        let shift = d * bits;
        let next_shift = active.get(ai + 1).map(|&p| p * bits);
        let (src, dst): (&[T], &mut [T]) = if src_is_data {
            (&*data, &mut *scratch)
        } else {
            (&*scratch, &mut *data)
        };

        let mut offsets: Vec<Vec<usize>> = vec![vec![0usize; nb]; cur.len()];
        let mut running = 0usize;
        for b in 0..nb {
            for (c, h) in cur.iter().enumerate() {
                offsets[c][b] = running;
                running += h[b];
            }
        }
        debug_assert_eq!(running, n);

        let out = SendPtr(dst.as_mut_ptr());
        let partial: Vec<Vec<Vec<usize>>> = src
            .par_chunks(chunk)
            .zip(offsets.into_par_iter())
            .map(move |(c, mut off)| {
                let out = out;
                match next_shift {
                    Some(ns) => {
                        let mut nh = vec![vec![0usize; nb]; nchunks];
                        for v in c {
                            let k = key(v);
                            let b = ((k >> shift) & mask) as usize;
                            let p = off[b];
                            // SAFETY: disjunktni opsezi, kao u originalu
                            unsafe { *out.0.add(p) = *v; }
                            off[b] = p + 1;
                            nh[p >> cshift][((k >> ns) & mask) as usize] += 1;
                        }
                        nh
                    }
                    None => {
                        for v in c {
                            let b = ((key(v) >> shift) & mask) as usize;
                            unsafe { *out.0.add(off[b]) = *v; }
                            off[b] += 1;
                        }
                        Vec::new()
                    }
                }
            })
            .collect();

        if next_shift.is_some() {
            let mut sum = vec![vec![0usize; nb]; nchunks];
            for part in &partial {
                for c in 0..nchunks { for b in 0..nb { sum[c][b] += part[c][b]; } }
            }
            cur = sum;
        }
        src_is_data = !src_is_data;
    }

    if !src_is_data {
        data.copy_from_slice(scratch);
    }
    active.len() as u32
}

#[cfg(test)]
mod tests_bits {
    use super::*;
    #[test]
    fn bits_match_sort() {
        let mut x = 0x0123_4567_89ab_cdefu64;
        for &n in &[0usize, 1, 2, 5000, 65_537, 700_001] {
            let mut v: Vec<(u64, i64)> = (0..n).map(|i| { x ^= x << 13; x ^= x >> 7; x ^= x << 17; (x % 34_000_000_000, i as i64) }).collect();
            if n > 10 { v[1].0 = u64::MAX; v[2].0 = 0; }
            let mut e = v.clone();
            e.sort_by_key(|p| p.0);
            for bits in [4u32, 8, 11, 12, 16] {
                for t in [1usize, 3, 9] {
                    let pool = rayon::ThreadPoolBuilder::new().num_threads(t).build().unwrap();
                    let mut a = v.clone();
                    pool.install(|| { radix_sort_by_key_fused_bits(&mut a, &mut Vec::new(), bits, |p| p.0); });
                    assert_eq!(a, e, "n={n} bits={bits} t={t}");
                }
            }
        }
    }
}
