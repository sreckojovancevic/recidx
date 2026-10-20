# Koncept: rang, autocount i dodatni identiteti

Zapisano 2026-10-20 iz razgovora Srećka Jovančevića i Claude-a, tokom rada na ARI testu.
Ideje su Srećkove; formulacija i veza sa implementacijom su zajedničke.

## 1. Identitet je relacija poretka

Element prepoznaje svoje mesto iz sopstvene vrednosti: 2 < 3 < 4.
Bucket nije unapred zadat blok, nego mesto koje ključ sam izračuna (broj bitova + sledećih 12 bitova).
U bucket idu pojedinačni identiteti: parovi (ključ, rowid).

## 2. Autocount — rang kao IZVEDENO svojstvo, ne upisani identitet

`((2 < 3 < 4) autocount#)`

Broj elemenata ispred datog mesta nije podatak upisan u element, nego posledica poretka,
izračunata automatski kad zatreba.

| | upisani rang | izvedeni rang (autocount) |
|---|---|---|
| šta se čuva | broj uz svaki element | samo poredak i zbirovi po grupama |
| posle umetanja/brisanja | menjaju se deskriptori svih elemenata iza | menjaju se samo zbirovi na putu (~17) |
| tačnost | tek posle osvežavanja | uvek |

**Autocount je pojam; Fenwick je mehanizam** koji ga izračunava (u recidx-u, varijanta B, `src/dynidx.rs`).
Isti autocount mogu da daju i drugi mehanizmi — prefiksna tabela (statičan indeks), dvonivojski
zbirovi, lenja baza — pojam ostaje, menja se cena. Izbor mehanizma je inženjerska odluka koja se meri.

COUNT(a..b) = autocount(b) − autocount(a) — ne zavisi od broja pogodaka
(izmereno: 112–316 ns na 4,58M redova, `results/runs/run_1792405529.txt`).

Poštena napomena: čuvanje broja elemenata kao automatski izvedenog svojstva strukture postoji
u literaturi (augmented / order-statistic stabla, gde čvor nosi veličinu podstabla). Ovde je drugačije
odakle poredak dolazi: iz prepoznavanja ključa, ne iz poređenja kroz stablo.

## 3. Dodatni identiteti — složeni ključ

Autocount je dodatak samom rangu, pa se može koristiti i sa dodatnim identitetima:

```
identitet  = poredak po (kolona1 ⧺ kolona2)     — spojen ključ
autocount  = izveden iz tog poretka              — za ceo ključ ili za njegov prefiks
```

Sa ključem `(korisnik, veličina)` isti autocount daje:
- koliko redova ima u opsegu,
- koliko fajlova ima jedan korisnik (opseg po prefiksu),
- koliko fajlova jedan korisnik ima između dve veličine.

Različiti indeksi nad istim podacima, svaki za drugu vrstu pitanja — isto što baze rade
(`backup_index` ima 7 indeksa nad istim redovima).

### Poreklo ideje

Srećko je ovo radio ranije u FileMaker-u, pre nego što je poznavao njegove mogućnosti:
računska polja (formule) kao izvedeni opisi, i dve kolone spojene (concatenate) da bi dobio
ono što baza realno ima kao indeks nad redom — i više takvih indeksa nad istim podacima
za različite potrebe.

## 4. Otvoreno (nije izmereno)

- Složen ključ (`user` kao broj ⧺ `size_bytes`) + autocount po prefiksu, naspram SQLite složenog indeksa.
- Lenja baza umesto Fenwick-a pri mnogo izmena i malo brojanja.
- Dvonivojski zbirovi (grupe od 256 bucket-a) kao jednostavniji mehanizam za autocount.
