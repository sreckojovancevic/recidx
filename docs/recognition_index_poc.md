# Recognition Index — istraživački okvir (PoC)

## 1. Ideja
Ispitati da li principi projekta **Recognition Sort / LZFILM** mogu da se primene na organizaciju i pronalaženje ključeva u bazi podataka, uz potencijalno smanjenje vremena izgradnje indeksa i/ili pretrage u odnosu na postojeće mehanizme.

**Hipoteza, ne potvrđen rezultat:** prepoznavanje klase ili pozicije ključa, uz direktno adresiranje gde je primenljivo, može dati prednost za određene distribucije podataka. Brzina sortiranja sama po sebi ne dokazuje bržu indeksnu pretragu.

## 2. Polazna osnova
- **Izvor podataka:** postojeća SQLite baza M365 backup rešenja, sa više od 3 miliona evidencionih zapisa.
- **Primeri polja:** ime, prezime, email adresa, naslov poruke ili dokumenta, identifikator zapisa (tačnu šemu tek proveriti).
- **Razvojni projekti:** Recognition Sort i LZFILM / LZFILM-Win.
- **Model obrade:** filmska traka sa okvirima (*frame*); okvir od približno 1 MB predstavlja jedinicu obrade u relevantnom režimu, a ne ukupno ograničenje veličine ulaza.
- **Dosadašnja zapažanja autora:** bolji rezultati na većim skupovima u određenim testovima; veća, ali kontrolisana potrošnja memorije. Potrebna je nezavisna reprodukcija merenja i jasno razlikovanje memorije po okviru od ukupne memorije strukture.

## 3. Cilj i granice
**Primarni cilj:** razviti eksperimentalni **Recognition Index** i proveriti njegovu upotrebljivost za `WHERE key = ?` nad stvarnim podacima.

**Sekundarni ciljevi:** uporediti brzinu izgradnje indeksa, traženja svih duplikata, ažuriranja, veličinu strukture i vršnu memoriju.

**Van opsega prve faze:** zamena SQLite B-tree mehanizma, izmene originalne produkcione baze, obećavanje univerzalne prednosti ili podrška svim SQL operatorima.

## 4. Konceptualna arhitektura

```text
               KOPIJA SQLite BAZE (3M+)
                         |
              izdvajanje (key, rowid)
                         |
               +---------+---------+
               |                   |
               v                   v
       SQLite B-tree         Recognition Index
       referentni test       eksperimentalni modul
               |                   |
               +---------+---------+
                         |
                isti skup upita
                         |
             tačnost + vreme + memorija
```

Recognition modul treba da mapira ključ na jedan ili više identifikatora zapisa. Konkretan mehanizam mapiranja, kolizija i duplikata nije unapred pretpostavljen.

## 5. Minimalni interfejs

```text
build(entries: [(key, rowid)]) -> index
find(key) -> [rowid]
insert(key, rowid)
delete(key, rowid)
serialize(index) / load(index)   # druga faza, ako PoC uspe
```

Kriterijum ispravnosti: `find(key)` mora vratiti isti skup identifikatora kao referentni SQLite upit, uključujući duplikate. Definisati postupanje sa `NULL`, kolacijama, Unicode normalizacijom i velikim ključevima.

## 6. Eksperimentalne faze i kriterijumi prolaza

### Faza 0 — inventar i zaštita podataka
- Raditi isključivo nad kopijom ili anonimizovanim izvozom; ne menjati produkcionu bazu.
- Zabeležiti SQLite verziju, šemu, postojeće indekse, broj redova, tipove i raspodelu duplikata.
- Odabrati konkretne postojeće upite; `EXPLAIN QUERY PLAN` za referentni pristup.

**Gate 0:** potvrđena kopija i reproduktibilan opis podataka bez izlaganja ličnih podataka.

### Faza 1 — korektnost algoritma
- Implementirati `build` i `find` kao zaseban modul, bez izmene SQLite izvornog koda.
- Testirati postojeće/nepostojeće ključeve, duplikate, prazne vrednosti, dugačke stringove, Unicode i kolizije.

**Gate 1:** 100% identični rezultati u determinističkom testnom skupu i nasumičnim proverama naspram SQLite-a.

### Faza 2 — uporedni benchmark
- Uporediti: SQLite bez odgovarajućeg indeksa, SQLite B-tree, Recognition Index; opciono hash strukturu za jednakost.
- Odvojeno meriti: izgradnju, pojedinačnu pretragu, pretragu duplikata, insert/delete, veličinu na disku i vršnu RAM potrošnju.
- Koristiti isti hardver, isti skup ključeva i upita, ponavljanja, medijanu i p95; odvojiti hladan i topao keš.
- Testirati raspone od 100 hiljada do najmanje 3 miliona zapisa, a zatim veće sintetičke skupove ako ima resursa.

**Gate 2:** kompletni, ponovljivi rezultati bez grešaka i sa dokumentovanim uslovima; prednost se tvrdi samo za metrike i opterećenja u kojima je izmerena.

### Faza 3 — dinamičke izmene i trajnost
- Izmeriti mešovite upite i upise, brisanja, promene ključa i ponovnu izgradnju.
- Ako je potreban trajni indeks, definisati format, oporavak posle prekida i sinhronizaciju sa SQLite transakcijama.

**Gate 3:** nema izgubljenih ili pogrešno mapiranih zapisa posle izmena; dokumentovane granice trajnosti.

### Faza 4 — odluka o integraciji
- Ako rezultati opravdavaju nastavak, odabrati integraciju (npr. SQLite ekstenzija/virtualna tabela ili fork). Obična SQLite funkcija ne predstavlja zamenu za planer i indeksni pristup.

**Gate 4:** tehnički dizajn sa jasno definisanim API-jem, ograničenjima i merljivim ciljevima.

## 7. Ključna istraživačka pitanja
1. Može li Recognition iz vrednosti ključa odrediti lokaciju ili dovoljno malu grupu kandidata bez punog skeniranja?
2. Kako se rešavaju različiti ključevi koji vode u istu klasu/poziciju?
3. Kako se čuvaju svi zapisi sa istim ključem?
4. Da li obrada u okvirima ograničava **radnu** memoriju bez narušavanja globalne pretrage?
5. Koliki je trošak održavanja strukture pri promenama podataka?
6. Da li prednost postoji za jednakost (`=`), opsege (`BETWEEN`), prefikse (`LIKE 'abc%'`) ili samo za izgradnju indeksa?

## 8. Očekivani rezultat PoC-a
- Izvorni kod prototipa i skripte za reprodukciju.
- Dokumentovan skup testova i potvrđena korektnost.
- Tabela i grafikoni vremena, memorije i veličine indeksa.
- Zaključak: **gde Recognition Index ima merljivu prednost, gde nema i zašto**.

## 9. Pravilo zaključivanja
Ne poistovećivati sortiranje, indeksiranje i pretragu. Ne zaključivati da je algoritam `O(1)` ili da nema mogućnosti prekoračenja memorije bez dokaza za odgovarajući model ulaza. Svaka tvrdnja o performansama mora biti vezana za konkretnu implementaciju, skup podataka, hardver i metod merenja.

## Reference
- https://github.com/sreckojovancevic/lzfilm
- https://github.com/sreckojovancevic/lzfilm-win
- https://www.sqlite.org/queryplanner.html
