# ARI (Adaptive Recognition Index) — kriterijumi uspeha i predviđanja

Zapisano 2026-10-20, PRE implementacije i merenja.
Kriterijumi su iz predloga drugog AI-ja (preneti u merljiv oblik); predviđanja su Claude-ova.

## Varijante

| Oznaka | Struktura |
|---|---|
| A | postojeći DynIndex, Log(12) — osnovna kontrola |
| B | fiksni finiji bucket, DynIndex Log(14) |
| C | ARI — lokalno usavršavanje preopterećenih bucket-a (pragovi 1.024 / 4.096 / 16.384 / 65.536) |
| D | globalna ponovna izgradnja: kad bucket pređe prag, ceo indeks se gradi sa finijim Log(S+1) |
| E | Rust BTreeSet |
| F | sortiran niz + binarna pretraga (izmene ograničene, pomeranje niza) |
| G | DynIndex Log(12) + posting liste (ključ jednom, lista rowid-ova) |

ARI dodatne kontrole (pri pragu 4.096): efektivni okidač umesto strukturnog;
lokalni sort umesto lokalne raspodele; Fenwick umesto obične tabele veličina u čvoru.

## Kriterijumi uspeha za C (iz predloga; pragovi preneti u brojeve)

| # | Kriterijum | Merljivo kao |
|---|---|---|
| K1 | Lokalnost | pri usavršavanju se premeštaju samo elementi jednog bucket-a (broj premeštenih ≤ veličina tog bucket-a) |
| K2 | Tačnost | sve invarijante i svi odgovori (COUNT, FETCH, CRUD) = referentni model |
| K3 | Jednakost | C nije više od 10% sporiji od A na ravnomernim podacima |
| K4 | Range COUNT / pretraga u preopterećenom delu | C ≥ 2× brži od A na grupisanom skupu (pretraga ili COUNT) |
| K5 | Memorija | C ≤ 1,25× memorije A (prag 1,25 je Claude-ov izbor; predlog kaže „ograničeno“) |
| K6 | Stabilnost | nijedan čvor nije usavršen pa spojen više od 2 puta u jednom pokretanju |
| K7 | Isplativost | ukupno CRUD vreme C (uključujući pauze usavršavanja) < A |
| K8 | Kontrola B | ako C ne bude bolji od B (Log(14)), adaptacija ne opravdava složenost |

## Predviđanja (Claude)

| # | Predviđanje |
|---|---|
| P1 | M365 podaci: C ≈ A (±10%) za jednakost, COUNT i CRUD; na pragu 4.096 usavrši se ≤ 10 čvorova — najveći bucket-i su jedan ključ ponovljen hiljadama puta (36.864 B × 4.318) i ne mogu se podeliti |
| P2 | Ravnomeran skup: ni jedan bucket ne pređe 1.024 → C = A (±5%), nula usavršavanja |
| P3 | Grupisan skup (4,58M): C brži od A za pretragu ≥ 1,5×; B (Log(14)) postiže ≥ 80% tog dobitka bez adaptacije |
| P4 | Mnogo identičnih ključeva: C bez dobitka; G troši ≥ 30% manje memorije od A, COUNT u granicama ±20% |
| P5 | Promenljiva raspodela (4,58M): C ≥ 1,5× brži od A u CRUD-u; D ima pauze ≥ 100 ms po globalnoj izgradnji |
| P6 | Najduža pauza usavršavanja ≤ 2 ms čak i pri pragu 65.536 |
| P7 | Fenwick naspram obične tabele u čvoru: razlika < 5% |
| P8 | Lokalni sort naspram lokalne raspodele: raspodela ≥ 2× brža u samom usavršavanju, ukupna razlika < 5% |

Merenje: medijana od 3 ponavljanja za ukupno vreme; p95/p99 iz posebnog prolaza sa merenjem svake operacije.
I dobri i loši rezultati se zapisuju.

## Dopuna posle prvog pokretanja (2026-10-20, 10:5x) — kriterijumi i predviđanja NISU menjani

Prvo pokretanje: results/matrix_1792483070.csv (v0.12), svih 195 kombinacija, sve provere tačnosti DA.

Greška u postavci (Claude): skup „promenljiv“ nije izazvao preopterećenje — novi grozdovi su bili
preširoki (najveći bucket A narastao samo do 1.312), pa se nigde nije pokrenulo usavršavanje ni
globalna izgradnja. P5 i K7 na tom skupu zato nisu bili stvarno testirani.

v0.12b:
- „promenljiv“: umetanja i izmene idu u 8 NOVIH uskih grozdova (širina 2^14).
- dodato po tipu operacije (umetanje, brisanje, ažuriranje, čitanje): prosek, p50, p95, p99;
- dodata memorija posle izmena i rast memorije (cena održavanja).

Transparentnost: pre drugog pokretanja Claude je video kratku proveru ispravnosti na sintetičkoj
bazi u svom kontejneru (100.000 redova, 1 ponavljanje): na „promenljiv“ A/C CRUD 1,43, D pauza 83 ms,
ARI 8 usavršavanja. To NIJE merenje na i9 i ne menja zapisana predviđanja.
