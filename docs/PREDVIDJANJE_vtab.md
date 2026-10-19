# Predviđanje — Recognition Index kao SQLite virtualna tabela (v0.11)

Zapisano 2026-10-19, PRE merenja (Claude).

Pitanje: koliko ostaje od prednosti Recognition Indexa kad se koristi IZ SQLite-a,
istim SQL upitom kao B-tree?

Polazne vrednosti (v0.8, run_1792405529): direktno iz Rust-a 530 ns po upitu jednakosti;
isti upit kroz SQL nad SQLite B-tree-om (:memory:) 3.881 ns. Prosečno ~54 reda po upitu.

Predviđanja:
1. Jednakost (`WHERE k = ?`) kroz virtualnu tabelu: 0,8–1,2× vremena B-tree-a kroz SQL.
   Razlog: trošak SQL-a po upitu i po redu (bind, step, poziv po redu iz SQLite-a u Rust)
   je veći od razlike među strukturama → prednost od ~530 ns skoro nestaje.
2. `SELECT count(*) ... BETWEEN` kroz virtualnu tabelu: NIJE brže od B-tree-a; za široke
   opsege 1–3× sporije (SQLite broji tako što traži red po red od virtualne tabele).
3. SQL funkcija `recog_count(a, b)` (struktura sama vrati broj): 1–3 µs po upitu,
   praktično nezavisno od širine opsega → za opseg 10% 100–500× brže od B-tree count-a.
4. FETCH (svi rowid-ovi iz opsega) kroz virtualnu tabelu: ±30% od B-tree-a.

Opovrgavanje: ako je virtualna tabela za jednakost < 0,8× (jasno brža) ili recog_count
> 3 µs, predviđanje je pogrešno.

## REZULTAT (v0.11, results/run_1792415582.txt, 4.581.340 redova, 1 nit)

| | izmereno | predviđeno | |
|---|---|---|---|
| 1. jednakost kroz virtualnu tabelu / B-tree kroz SQL | **0,67×** (2.746 / 4.072 ns) | 0,8–1,2× | **PROMAŠENO** — virtualna tabela je jasno brža (1,48×) |
| 2. count(*) kroz virtualnu tabelu / B-tree | **0,44×** (sve tri širine) | nije brže, 1–3× sporije | **PROMAŠENO** — 2,3× brže |
| 3. recog_count(a, b) | **184–200 ns**, nezavisno od širine | 1–3 µs | **PROMAŠENO u vrednosti** (10× brže od predviđenog); smer pogođen: 62× / 556× / 4.840× brže od B-tree count-a |
| 4. FETCH kroz virtualnu tabelu / B-tree | **0,69–0,73×** | ±30% (0,7–1,3) | **na granici** (0,72 i 0,73 unutar, 0,69 malo ispod) |

Zašto je predviđanje bilo pogrešno (Claude): precenio sam trošak poziva iz SQLite-a u
virtualnu tabelu po redu, a potcenio koliko SQLite-ov B-tree sam troši po redu
(kretanje kroz stranice, dekodiranje zapisa). Virtualna tabela samo vraća sledeći element
iz niza u memoriji.

Šta ostaje od direktnog poziva kroz SQL: jednakost 556 → 2.746 ns (SQL dodaje ~2,2 µs po upitu).
