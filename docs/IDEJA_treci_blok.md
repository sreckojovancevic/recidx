# Ideja: treći blok (ternarni tok)

Zapisano 2026-10-19, u okviru Recognition Index PoC-a (recidx).

## Ideja (Srećkovim rečima)

> „treba mi treći blok koji je null, zašto ne znam ... jer pokušavam da imam trinarni flow,
> iako se ne računar razlog; mislim da je to deo gde pada brzina na više nitova“

Status: **intuicija, nije dokazana.**

## Šta je izmereno pre ideje (činjenice)

Iz recidx v0.2 (4.581.340 parova od 16 B, i9, D:\sqliteRecog):

| | 1 nit | 20 niti | ubrzanje |
|---|---|---|---|
| LENS radix | 133–140 ms | 85–93 ms | ~1,5× |
| rayon par_sort_unstable | 149–155 ms | 41–42 ms | ~3,6× |

Radix gotovo ne dobija od dodatnih niti, dok sort zasnovan na poređenju dobija.
To je mesto na koje ideja cilja.

Radix danas radi sa **dva** bloka: izvor → odredište (ping-pong), pa zamena uloga.
U fazi raspoređivanja svaka nit upisuje elemente direktno na 256 različitih,
međusobno udaljenih mesta u odredištu.

## Tumačenje (hipoteze — Claude, nije izmereno)

1. **Treći blok kao mali međubafer između izvora i odredišta.**
   Svaka nit ima mali lokalni blok po bucket-u (npr. 16 elemenata). Element ide prvo
   u taj blok; tek kad se blok napuni, ceo se prepiše u odredište odjednom.
   „Null“ stanje = prazan blok koji čeka da se napuni.
   Ovo je u literaturi poznato kao *software write-combining* kod radix sorta.
   Moguće objašnjenje pada brzine: 256 razbacanih upisa po niti × 20 niti preopterećuje
   keš procesora i memorijsku magistralu; grupisan upis to smanjuje.

2. **Treći blok kao trostruko baferovanje (triple buffering).**
   Jedan blok se čita, jedan se puni, treći je prazan („null“) i spreman,
   pa nijedna nit ne čeka drugu. Bliže je pipeline-u u My Radix LENS (priprema run-ova).

3. **Ternarni tok kao tri stanja elementa:** pročitan → u prolazu (null/neodređen) → smešten.
   Konceptualno, ne kao hardverska trinarnost.

## Kako se proverava

- Hipoteza 1 je implementirana kao eksperiment u recidx (radix sa trećim blokom) i meri se
  u 1 niti i u N niti, uz proveru da je rezultat identičan (otisak).
- Ako treći blok poboljša skaliranje na više niti → ideja ima merljivu potvrdu za taj oblik.
- Ako ne poboljša → taj oblik ideje ne objašnjava pad; ostaju hipoteze 2 i 3.
- I dobar i loš rezultat se zapisuju.

## Rezultat merenja — hipoteza 1 (recidx v0.8, 2026-10-19, results/run_1792405262.txt)

4.581.340 parova od 16 B, medijana od 5 ponavljanja, isti redosled (otisak) kod svih varijanti.

| | 1 nit | 20 niti | ubrzanje 1 → 20 |
|---|---|---|---|
| LENS radix | 118,4 ms | 84,0 ms (80,4–87,6) | 1,41× |
| + treći blok W=8 | — | 83,3 ms | — |
| + treći blok W=16 | 127,6 ms | 80,5 ms (79,0–84,3) | 1,59× |
| + treći blok W=32 | — | 80,0 ms (78,9–84,9) | — |
| rayon par_sort_unstable | 140,8 ms | 39,0 ms | 3,61× |

Čitanje:
- Na 20 niti treći blok je brži 4–5% (84,0 → 80,0–80,5 ms); rasponi se delimično preklapaju — mali efekat.
- U 1 niti treći blok je SPORIJI 8% (118,4 → 127,6 ms).
- Bolji odnos skaliranja (1,59× naspram 1,41×) dolazi većim delom od sporije jedne niti, a manjim od brže 20 niti.
- par_sort_unstable je i dalje 2× brži na 20 niti.

Zaključak za hipotezu 1: **grupisan upis (treći blok kao međubafer) ne objašnjava pad brzine na više niti.**
Efekat postoji, ali je mali. Uzrok je verovatno drugde — kandidati za sledeće merenje (Claude, hipoteze):
- 8 prolaza za histogram (i za preskočene bajtove) = dodatno čitanje 70 MB po prolazu;
  jedan zajednički prolaz za sve histograme bi to smanjio.
- 20 niti na 10 fizičkih jezgara (hyper-threading) — proveriti --threads 6 i 10.
- Oblik 2/3 ideje: jedan paralelni prolaz po najvišem bajtu, pa svaki bucket nezavisno u svojoj niti
  (nema deljenog razbacanog upisa) — klasičan način da radix bolje skalira.

## Merenje broja niti (2026-10-19, run_1792405763 i run_1792405810)

| | 9 niti | 10 niti | 20 niti |
|---|---|---|---|
| LENS radix | 75,5 ms | 75,4 ms | 84,0 ms |
| + treći blok W=16 | 75,8 ms | 76,8 ms | 80,5 ms |
| par_sort_unstable | 38,6 ms | 37,6 ms | 39,0 ms |

- 9 naspram 10: nema razlike (rasponi se preklapaju) — neparnost na ovom paru nema efekta.
- Preko 10 (broj fizičkih jezgara) radix se usporava ~11%.
- Odluka (Srećko): broj niti = broj fizičkih jezgara − 1.

## Hipoteza: radix je ograničen protokom memorije (Claude, procena — NIJE izmereno)

Saobraćaj kroz memoriju u postojećem LENS radixu (4,58M × 16 B = 70 MB):
- histogram u 8 posebnih prolaza (i za preskočene bajtove): 8 × 70 = 560 MB čitanja
- 5 prolaza raspoređivanja: 5 × (70 čitanje + 70 upis) = 700 MB
- neparan broj prolaza → završno kopiranje: 140 MB
- ukupno ≈ 1,4 GB za ~75 ms ≈ 19 GB/s — blizu realnog protoka RAM-a

## PREDVIĐANJE (zapisano PRE merenja v0.9)

v0.9: histogram za sledeći prolaz računa se TOKOM raspoređivanja (spojen prolaz);
samo prvi histogram je poseban prolaz. Saobraćaj pada na ≈ 70 + 700 + 140 ≈ 0,9 GB.

Ako je hipoteza tačna: LENS radix na 9–10 niti pada sa ~75 ms na **~45–55 ms**.
Ako ostane ~75 ms: hipoteza je pogrešna, usko grlo je drugde.
U 1 niti očekivana ušteda je manja (procesor je tada više ograničen računanjem nego memorijom).

## REZULTAT v0.9 — spojen histogram (2026-10-19, results/run_1792414113.txt, 9 niti, 10 fizičkih / 20 logičkih)

| | 1 nit | 9 niti |
|---|---|---|
| LENS radix | 127,3 ms | 77,4 ms (75,4–86,5) |
| LENS radix, spojen histogram v0.9 | **100,4 ms** | **62,5 ms (59,9–63,6)** |
| poboljšanje | −21% | −19% |
| rayon par_sort_unstable | — | 39,2 ms |
| std sort_unstable (1 nit) | 138,9 ms | — |

Provera predviđanja:
- Predviđeno: 9–10 niti ~45–55 ms. Izmereno: 62,5 ms → **kvantitativno predviđanje NIJE pogođeno.**
- Smer je potvrđen: manje prolaza kroz memoriju = brže, na 1 i na 9 niti.
- Pogrešan je bio i deo „u 1 niti ušteda manja“: ušteda u 1 niti je čak malo veća (−21%).
- Procena saobraćaja je bila nepotpuna: upis u odredište koje nije u kešu procesor obično plaća
  i dodatnim čitanjem te linije (read-for-ownership), pa je stvarni saobraćaj po prolazu veći od
  70 + 70 MB. Zato smanjenje broja prolaza za brojanje ne daje srazmeran dobitak.

Zaključak: radix je delimično ograničen protokom memorije; spojen histogram je merljivo bolji
(u 1 niti 1,38× brži od sort_unstable — najbolji radix rezultat u ovom projektu), ali
par_sort_unstable je na 9 niti i dalje 1,6× brži.

Sledeći kandidat (hipoteza, nije izmereno): manje prolaza raspoređivanja.
Ključ ima 35 bitova → sa cifrom od 8 bita treba 5 prolaza (neparno → još jedno kopiranje).
Sa cifrom od 11 ili 12 bita trebaju 3–4 prolaza (paran broj → bez završnog kopiranja).
Isto zapažanje kao u recognition_sort README: „baza je presudna jer određuje broj prolaza“.

## PREDVIĐANJE za v0.10 — veličina cifre (zapisano PRE merenja, 2026-10-19)

Jedna promenljiva: veličina cifre (8 / 11 / 12 bita), sve sa spojenim histogramom.
Ključ ima 35 bitova.

| cifra | prolaza | završno kopiranje | bucket-a |
|---|---|---|---|
| 8 (v0.9) | 5 | da | 256 |
| 11 | 4 | ne | 2.048 |
| 12 | 3 | da | 4.096 |

Gruba računica: ako se vreme meri „prolazima kroz niz“ (kopiranje ≈ 1 prolaz),
8 bita = 6 jedinica, 11 bita = 4, 12 bita = 4 → oba bi bila ~1/3 brža od 8 bita.
Suprotan efekat: sa 2.048 / 4.096 bucket-a svaka nit piše na mnogo više razbacanih mesta
(više promašaja keša i TLB-a), što može da pojede dobitak.

Predviđanje (Claude):
- 9 niti: 11 bita **45–55 ms** (v0.9 sa 8 bita: 62,5 ms).
- 12 bita **nije bolji od 11** (ima završno kopiranje i 2× više razbacanih upisa).
- 1 nit: 11 bita **70–85 ms** (v0.9: 100,4 ms).
- Opovrgavanje: ako je 11 bita ≥ ~62 ms na 9 niti, cena razbacanog upisa preteže i
  manje prolaza ne pomaže.

## REZULTAT v0.10 — veličina cifre (2026-10-19, results/run_1792414722.txt, 9 niti)

| | 1 nit | 9 niti | ubrzanje 1 → 9 |
|---|---|---|---|
| spojen histogram, 8 bita (5 prolaza + kopiranje) | **93,3 ms** | 60,5 ms | 1,54× |
| spojen histogram, 11 bita (4 prolaza, bez kopiranja) | 107,0 ms | **52,1 ms** | 2,05× |
| spojen histogram, 12 bita (3 prolaza + kopiranje) | 106,3 ms | 56,3 ms | 1,89× |
| rayon par_sort_unstable | — | 40,4 ms | — |
| std sort_unstable | 143,3 ms | — | — |

Provera predviđanja:
- 9 niti, 11 bita u 45–55 ms → **POGOĐENO** (52,1 ms).
- 12 bita nije bolji od 11 → **POGOĐENO** (56,3 > 52,1).
- 1 nit, 11 bita u 70–85 ms → **NIJE POGOĐENO**: 107,0 ms, čak SPORIJE od 8 bita (93,3 ms).

Čitanje:
- U 1 niti preteže cena razbacanog upisa u 2.048/4.096 bucket-a; manje prolaza ne pomaže.
- Na 9 niti preteže manje prolaza i izostanak završnog kopiranja; 11 bita je najbolji radix (52,1 ms).
- Najbolja veličina cifre ZAVISI od broja niti: 1 nit → 8 bita; 9 niti → 11 bita.
- par_sort_unstable je i dalje 1,29× brži od najboljeg radixa na 9 niti.

Ukupan put LENS radixa na 9 niti u ovom istraživanju: 77,0 → 60,5 (spojen histogram) → 52,1 ms (11 bita) = −32%.
Treći blok (write-combining) u tom putu nije doprineo merljivo.
