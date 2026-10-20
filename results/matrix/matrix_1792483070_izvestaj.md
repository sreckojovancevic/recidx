## IZVEŠTAJ — ocena kriterijuma (iz predloga) i predviđanja (Claude), zapisanih pre merenja
Najveća veličina: 4581340 (m365: 4581340). Odnosi > 1 znače „brže od“ ili „manje od“ kako je navedeno.

### Kriterijumi uspeha za C (ARI)
| # | kriterijum | merenje | ocena |
|---|---|---|---|
| K1 | lokalnost | kršenja lokalnosti u svim C pokretanjima: 0 | PROLAZI |
| K2 | tačnost | sve provere (invarijante, jednakost, opsezi, CRUD): sve DA  | PROLAZI |
| K3 | jednakost na ravnomernim ≤ 1,10× A | C/A vreme: 100000: 0.96, 1000000: 1.06, 4581340: 0.82 | PROLAZI |
| K4 | ≥ 2× brže od A u preopterećenom delu (grupisan, 4581340) | najbolji A/C: 1.53× (count_ns_0.1pct) | NE PROLAZI |
| K5 | memorija ≤ 1,25× A | najveći odnos C/A: 1.30 | NE PROLAZI |
| K6 | stabilnost: ≤ 2 spajanja istog bucket-a | najviše: 1 | PROLAZI |
| K7 | isplativost: CRUD C (sa pauzama) ≤ A | A/C: grupisan/umetanje: 21.59, grupisan/uravnotezeno: 22.52, grupisan/brisanje: 28.69, promenljiv/umetanje: 0.94, promenljiv/uravnotezeno: 0.90, promenljiv/brisanje: 0.90 | NE PROLAZI |
| K8 | kontrola B (Log(14)): da li je C bolji od finijeg fiksnog bucket-a? (> 1 = C brži) | grupisan: eq B/C 1.40 · count1% B/C 1.18 · CRUD B/C 7.91 ‖ promenljiv: eq B/C 0.80 · count1% B/C 0.94 · CRUD B/C 0.65 | tumači se |

### Predviđanja (Claude)
| # | predviđanje | izmereno | ocena |
|---|---|---|---|
| P1 | m365: C ≈ A (±10%), ≤ 10 usavršenih čvorova | C/A: eq 1.00, count1% 1.42, CRUD 1.09 · čvorova: 1 | PROMAŠENO |
| P2 | ravnomeran: 0 usavršavanja, C = A ±5% | 100000: usavršavanja(1024) 0, C/A 0.96; 1000000: usavršavanja(1024) 0, C/A 1.06; 4581340: usavršavanja(1024) 0, C/A 0.82 | PROMAŠENO |
| P3 | grupisan: A/C eq ≥ 1,5; B postiže ≥ 80% dobitka | A/C 1.45 · udeo dobitka B 0.13 | PROMAŠENO |
| P4 | duplikati: C bez dobitka; G ≤ 0,7× memorije A; G COUNT ±20% | C/A eq 1.00 · G/A mem 0.88 · G/A count 1.19 | PROMAŠENO |
| P5 | promenljiv: A/C CRUD ≥ 1,5; D pauza ≥ 100 ms | A/C 0.90 · D najveća pauza 0.0 ms | PROMAŠENO |
| P6 | najduža pauza usavršavanja ≤ 2 ms | 0.22 ms | POGOĐENO |
| P7 | Fenwick ≈ obična tabela (< 5%) | ravnomeran eq_ns 1.07, ravnomeran count_ns_1pct 1.08, ravnomeran crud_us_uravnotezeno 1.04, grupisan eq_ns 1.03, grupisan count_ns_1pct 0.78, grupisan crud_us_uravnotezeno 0.98, duplikati eq_ns 1.01, duplikati count_ns_1pct 1.06, duplikati crud_us_uravnotezeno 1.04, promenljiv eq_ns 1.01, promenljiv count_ns_1pct 1.12, promenljiv crud_us_uravnotezeno 0.96, m365 eq_ns 0.93, m365 count_ns_1pct 0.67, m365 crud_us_uravnotezeno 0.88 | PROMAŠENO |
| P8 | lokalna raspodela ≥ 2× brža u usavršavanju, ukupno < 5% razlike | grupisan: vreme usavršavanja sort/raspodela — · ukupno 0.97 ‖ promenljiv: vreme usavršavanja sort/raspodela — · ukupno 0.93 | PROMAŠENO |

### Pregled — najveća veličina, odnos „uravnoteženo“ (ns po upitu / µs po izmeni)

**ravnomeran** (4581340 redova)
| varijanta | izgradnja ms | mem MiB | eq ns | eq p99 | COUNT 1% | COUNT 50% | FETCH 1% | CRUD µs | CRUD p99 ns | usavršavanja | najveća pauza µs | tačno |
|---|---|---|---|---|---|---|---|---|---|---|---|---|
| A_Log12 | 27.47 | 76.53 | 244 | 900 | 177 | 147 | 158236 | 0.91 | 7200 | — | — | DA |
| B_Log14 | 40.28 | 95.41 | 166 | 600 | 134 | 113 | 161394 | 0.65 | 2400 | — | — | DA |
| C_ARI_1024 | 23.83 | 79.02 | 197 | 600 | 160 | 405 | 166829 | 0.86 | 7200 | 0 | 0.00 | DA |
| C_ARI_4096 | 26.63 | 79.02 | 201 | 700 | 159 | 143 | 167099 | 0.88 | 7300 | 0 | 0.00 | DA |
| C_ARI_16384 | 28.24 | 79.02 | 201 | 700 | 247 | 155 | 172262 | 0.90 | 7100 | 0 | 0.00 | DA |
| C_ARI_65536 | 28.36 | 79.02 | 206 | 600 | 206 | 140 | 173241 | 0.90 | 7200 | 0 | 0.00 | DA |
| C_ARI_4096_efektivni | 25.90 | 79.02 | 226 | 700 | 309 | 139 | 173794 | 0.92 | 7100 | 0 | 0.00 | DA |
| C_ARI_4096_lokalni_sort | 27.19 | 79.02 | 201 | 600 | 179 | 133 | 170143 | 0.97 | 7300 | 0 | 0.00 | DA |
| C_ARI_4096_fenwick | 34.43 | 79.02 | 215 | 600 | 171 | 137 | 168003 | 0.91 | 7600 | 0 | 0.00 | DA |
| D_globalna_izgradnja | 28.79 | 76.53 | 194 | 600 | 148 | 161 | 157150 | 0.88 | 7000 | 0 | 0.00 | DA |
| E_BTreeSet | 61.38 | 79.44 | 295 | 700 | 67763 | 1986557 | 186207 | 0.50 | 1200 | — | — | DA |
| F_sortiran_niz | 12.38 | 69.91 | 333 | 700 | 296 | 328 | 154690 | 2902 | 9816400 | — | — | DA |
| G_posting | 126 | 93.64 | 254 | 800 | 473 | 286 | 159016 | 1.13 | 4900 | — | — | DA |

**grupisan** (4581340 redova)
| varijanta | izgradnja ms | mem MiB | eq ns | eq p99 | COUNT 1% | COUNT 50% | FETCH 1% | CRUD µs | CRUD p99 ns | usavršavanja | najveća pauza µs | tačno |
|---|---|---|---|---|---|---|---|---|---|---|---|---|
| A_Log12 | 19.15 | 76.53 | 258 | 700 | 201 | 172 | 271728 | 15.07 | 74000 | — | — | DA |
| B_Log14 | 25.34 | 95.41 | 248 | 600 | 188 | 174 | 268028 | 5.30 | 25700 | — | — | DA |
| C_ARI_1024 | 40.19 | 80.43 | 174 | 600 | 156 | 234 | 316032 | 0.67 | 2700 | 204 | 0.00 | DA |
| C_ARI_4096 | 35.83 | 80.23 | 178 | 600 | 158 | 128 | 324404 | 0.67 | 2800 | 175 | 0.00 | DA |
| C_ARI_16384 | 46.01 | 79.65 | 198 | 600 | 146 | 166 | 300414 | 1.39 | 9900 | 92 | 0.00 | DA |
| C_ARI_65536 | 17.76 | 79.02 | 256 | 700 | 211 | 174 | 270363 | 14.26 | 64400 | 0 | 0.00 | DA |
| C_ARI_4096_efektivni | 16.34 | 79.02 | 264 | 700 | 149 | 142 | 310921 | 14.28 | 65800 | 0 | 0.00 | DA |
| C_ARI_4096_lokalni_sort | 40.72 | 80.23 | 172 | 600 | 144 | 151 | 304097 | 0.65 | 2400 | 175 | 0.00 | DA |
| C_ARI_4096_fenwick | 36.66 | 80.57 | 182 | 600 | 124 | 130 | 304461 | 0.66 | 2700 | 175 | 0.00 | DA |
| D_globalna_izgradnja | 229 | 168 | 243 | 700 | 175 | 174 | 267471 | 1.92 | 9300 | 0 | 0.00 | DA |
| E_BTreeSet | 60.31 | 79.44 | 281 | 700 | 91144 | 1918719 | 292708 | 0.50 | 1100 | — | — | DA |
| F_sortiran_niz | 11.63 | 69.91 | 338 | 800 | 420 | 321 | 262179 | 2854 | 9416300 | — | — | DA |
| G_posting | 61.72 | 91.27 | 300 | 800 | 305 | 290 | 261790 | 19.35 | 91100 | — | — | DA |

**duplikati** (4581340 redova)
| varijanta | izgradnja ms | mem MiB | eq ns | eq p99 | COUNT 1% | COUNT 50% | FETCH 1% | CRUD µs | CRUD p99 ns | usavršavanja | najveća pauza µs | tačno |
|---|---|---|---|---|---|---|---|---|---|---|---|---|
| A_Log12 | 18.51 | 76.53 | 39295 | 172600 | 173 | 133 | 189909 | 12.19 | 157000 | — | — | DA |
| B_Log14 | 27.50 | 95.41 | 39225 | 197800 | 161 | 125 | 196080 | 11.69 | 157100 | — | — | DA |
| C_ARI_1024 | 30.07 | 79.99 | 39212 | 165200 | 199 | 162 | 199751 | 12.16 | 156800 | 143 | 0.00 | DA |
| C_ARI_4096 | 31.33 | 79.99 | 39184 | 169600 | 183 | 285 | 202061 | 11.85 | 157900 | 143 | 0.00 | DA |
| C_ARI_16384 | 30.09 | 79.99 | 39033 | 167100 | 207 | 158 | 202630 | 11.72 | 156500 | 143 | 0.00 | DA |
| C_ARI_65536 | 17.02 | 79.02 | 39146 | 166800 | 180 | 139 | 200367 | 12.01 | 157200 | 0 | 0.00 | DA |
| C_ARI_4096_efektivni | 16.63 | 79.02 | 39151 | 171500 | 178 | 145 | 199302 | 12.20 | 157000 | 0 | 0.00 | DA |
| C_ARI_4096_lokalni_sort | 33.15 | 79.99 | 38996 | 172800 | 215 | 166 | 204536 | 11.75 | 157700 | 143 | 0.00 | DA |
| C_ARI_4096_fenwick | 30.44 | 80.27 | 39505 | 167600 | 194 | 149 | 202413 | 12.28 | 164200 | 143 | 0.00 | DA |
| D_globalna_izgradnja | 217 | 168 | 39032 | 172400 | 144 | 123 | 203526 | 11.78 | 156600 | 0 | 0.00 | DA |
| E_BTreeSet | 49.37 | 79.44 | 43217 | 202200 | 74068 | 2358635 | 223687 | 10.32 | 204600 | — | — | DA |
| F_sortiran_niz | 11.41 | 69.91 | 40488 | 170400 | 377 | 342 | 195911 | 2836 | 9386700 | — | — | DA |
| G_posting | 81.34 | 67.15 | 38920 | 171400 | 206 | 196 | 190563 | 9.82 | 153600 | — | — | DA |

**promenljiv** (4581340 redova)
| varijanta | izgradnja ms | mem MiB | eq ns | eq p99 | COUNT 1% | COUNT 50% | FETCH 1% | CRUD µs | CRUD p99 ns | usavršavanja | najveća pauza µs | tačno |
|---|---|---|---|---|---|---|---|---|---|---|---|---|
| A_Log12 | 17.49 | 76.53 | 206 | 600 | 157 | 154 | 156315 | 0.45 | 1700 | — | — | DA |
| B_Log14 | 26.39 | 95.41 | 167 | 500 | 152 | 123 | 160819 | 0.32 | 1200 | — | — | DA |
| C_ARI_1024 | 38.00 | 79.02 | 207 | 600 | 167 | 166 | 172427 | 0.48 | 1700 | 0 | 0.00 | DA |
| C_ARI_4096 | 16.66 | 79.02 | 210 | 600 | 162 | 163 | 176569 | 0.50 | 1800 | 0 | 0.00 | DA |
| C_ARI_16384 | 39.37 | 79.02 | 248 | 700 | 216 | 162 | 177137 | 0.46 | 1700 | 0 | 0.00 | DA |
| C_ARI_65536 | 16.93 | 79.02 | 209 | 600 | 195 | 152 | 170443 | 0.49 | 1700 | 0 | 0.00 | DA |
| C_ARI_4096_efektivni | 16.60 | 79.02 | 254 | 700 | 566 | 153 | 170136 | 0.46 | 1700 | 0 | 0.00 | DA |
| C_ARI_4096_lokalni_sort | 17.43 | 79.02 | 212 | 700 | 220 | 162 | 172904 | 0.46 | 1700 | 0 | 0.00 | DA |
| C_ARI_4096_fenwick | 17.79 | 79.02 | 213 | 600 | 181 | 169 | 172995 | 0.48 | 1900 | 0 | 0.00 | DA |
| D_globalna_izgradnja | 18.31 | 76.53 | 195 | 600 | 174 | 196 | 158714 | 0.45 | 1700 | 0 | 0.00 | DA |
| E_BTreeSet | 56.53 | 79.44 | 301 | 600 | 58692 | 2253772 | 181960 | 0.33 | 1100 | — | — | DA |
| F_sortiran_niz | 11.94 | 69.91 | 393 | 900 | 408 | 541 | 159699 | 3741 | 10769400 | — | — | DA |
| G_posting | 110 | 93.64 | 295 | 900 | 297 | 335 | 159561 | 0.70 | 2500 | — | — | DA |

**m365** (4581340 redova)
| varijanta | izgradnja ms | mem MiB | eq ns | eq p99 | COUNT 1% | COUNT 50% | FETCH 1% | CRUD µs | CRUD p99 ns | usavršavanja | najveća pauza µs | tačno |
|---|---|---|---|---|---|---|---|---|---|---|---|---|
| A_Log12 | 40.68 | 76.53 | 618 | 8600 | 117 | 131 | 31476 | 0.60 | 3300 | — | — | DA |
| B_Log14 | 31.08 | 95.41 | 578 | 9100 | 160 | 181 | 36428 | 0.62 | 2900 | — | — | DA |
| C_ARI_1024 | 20.99 | 79.04 | 560 | 8100 | 97.45 | 214 | 36177 | 0.74 | 3800 | 73 | 6.90 | DA |
| C_ARI_4096 | 22.07 | 79.02 | 619 | 9400 | 167 | 163 | 38334 | 0.65 | 4600 | 1 | 0.00 | DA |
| C_ARI_16384 | 19.66 | 79.02 | 585 | 8400 | 133 | 348 | 37006 | 0.65 | 3200 | 0 | 0.00 | DA |
| C_ARI_65536 | 20.35 | 79.02 | 616 | 8600 | 98.15 | 183 | 37264 | 0.66 | 5200 | 0 | 0.00 | DA |
| C_ARI_4096_efektivni | 19.30 | 79.02 | 606 | 8400 | 134 | 156 | 38185 | 0.62 | 3100 | 0 | 0.00 | DA |
| C_ARI_4096_lokalni_sort | 21.04 | 79.02 | 564 | 8200 | 123 | 178 | 35718 | 0.65 | 4600 | 1 | 0.00 | DA |
| C_ARI_4096_fenwick | 20.65 | 79.02 | 576 | 8500 | 111 | 133 | 36252 | 0.58 | 3000 | 1 | 0.00 | DA |
| D_globalna_izgradnja | 162 | 120 | 547 | 8000 | 105 | 144 | 34460 | 0.49 | 2700 | 0 | 0.00 | DA |
| E_BTreeSet | 54.18 | 79.44 | 717 | 8900 | 11039 | 427201 | 32313 | 0.56 | 3300 | — | — | DA |
| F_sortiran_niz | 11.24 | 69.91 | 695 | 8200 | 302 | 263 | 28286 | 3163 | 10074500 | — | — | DA |
| G_posting | 93.05 | 53.43 | 549 | 7800 | 102 | 153 | 29638 | 0.58 | 2300 | — | — | DA |

Svi brojevi su u CSV-u; ovde je samo pregled. Napomena: F (sortiran niz) radi najviše 3.000 izmena kad je n > 100.000.
