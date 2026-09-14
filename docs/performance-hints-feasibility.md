# HINTS: ocena pierwszej reguły — możliwa fragmentacja tabeli

Stan: **wykonalne; propozycja implementacji, nie gotowy detektor**. Badanie 2026-09-13,
checkout JAS-MIN `3f6114b15d334cf7bd711ac359a44793e5a71d4f`.
W tym etapie nie zmieniono silnika ani aktywnego `reasonings.txt`.

## Wniosek

Pierwszy hint powinien wykrywać **wzrost liczby odwiedzanych bloków na skan wraz
ze wzrostem kosztu pracy**. Rozrzedzenie tabeli/puste bloki poniżej HWM są możliwym
wyjaśnieniem. Sam wzrost skanowanych bloków nie rozstrzyga między rozrzedzeniem,
wzrostem użytecznych danych i zmianą miksu skanowanych obiektów.

Tytuł `Possible table fragmentation detected` jest odpowiedni dla hipotezy.
Pole `Prove` powinno zawierać mierzalne przesłanki, a nie deklarować udowodnioną
przyczynę fizyczną. Brak danych o segmentach nie blokuje hinta na poziomie instancji.

Podstawa mechanizmu: full scan odwiedza sformatowane bloki pod HWM, także takie,
które wcześniej zawierały dane. Szczegóły przebiegu zależą od ASSM/MSSM.
[Oracle 19c: Logical Storage Structures](https://docs.oracle.com/en/database/oracle/oracle-database/19/cncpt/logical-storage-structures.html).

## Sprawdzenie na zachowanych danych

Ponownie policzono wartości z nieuzupełnionego
`oracle-EmptyCalories/hourly_lab/data/awr_native.json`:
SHA-256 `f8447b60ce6d189834fa69971ed15eddd2d54b29cfa51bfb245a7f6764be84ed`.
W pliku jest 61 okien i **zero `access_path_observations`**. Scope: DBID 1203622871,
INST_ID 1, Oracle 19.32 AArch64. Baza: begin SNAP 192–200, okres porównania: 214–215.
Są to jawnie wybrane okna do oceny możliwości, nie automatycznie wyznaczona baza produkcyjna.

Stawki to `sum(delta) / sum(seconds)`. Ilorazy to `sum(numerator) / sum(denominator)`.
Obie domeny SQL mają pokrycie 9/9 i 2/2 w swoich okresach; każda używa własnych
obserwowanych wykonań. DB Time/CPU obliczono z sekund Time Model.

| Przesłanka | Baza | Porównanie | Znaczenie |
| --- | ---: | ---: | --- |
| Bloki/skan | 152,29 | 3331,82 | **21,88×** większa praca na skan |
| Inicjalizacje skanów/s | 41,81 | 40,43 | −3,30%; podobna aktywność opisowo |
| Execute count/s | 661,45 | 643,10 | −2,77%; dodatkowa kontrola wolumenu |
| DB Time, s/s | 0,22771 | 0,35256 | +54,83%, absolutnie +0,12485 s/s |
| DB CPU, s/s | 0,22028 | 0,34248 | +55,48%, absolutnie +0,12220 s/s |
| SQL `9paxwp1pabugh`, gets/execute | 189,75 | 5535,64 | Rosnący koszt logicznego odczytu |
| Ten SQL, elapsed/execute | 2,774 ms | 9,696 ms | Rosnący zmierzony koszt czasu |
| ERP_SPARSE, logical reads/s | 3790,70 | 128692,23 | Kandydat OBJ# 76100, DATAOBJ# 76100 |
| ERP_DENSE, logical reads/s | 2732,98 | 2730,18 | Kontrola pozostaje stabilna opisowo |

Dwa końcowe okna wystarczają do tego porównania opisowego, ale nie do kalibracji
niezawodnego testu równoważności aktywności ani progów produkcyjnych. Wzrost względny
CPU nie jest dowodem nasycenia CPU ani oszczędności możliwych do odzyskania.
Dokładne wartości zachowano w
`test_runs/performance-hints-feasibility-2026-09-13/evidence.json`.

Sprawdzono również kontrprzykłady w zapisanym `jasmin_only/evidence/calibration.json`:

- Nested loops, 2 → 5 iteracji: liczba bloków rośnie, ale pozostaje **134 bloki/skan**
  w każdym z trzech powtórzeń. Nie powinien powstać hint inflacji bloków/skan.
- 8000 żywych wierszy oraz 2000 żywych + 6000 usuniętych: **identyczne 23 liczniki**
  w trzech powtórzeniach, po 534 bloki/skan. Oba stany muszą dostać tę samą ostrożną
  interpretację tych liczników. Czasy wykonania nie są identyczne; nie dowodzi to
  identyczności wszystkich możliwych obserwacji AWR.
- Pobranie szerokiej i wąskiej projekcji tej samej tabeli zmienia continued-row
  activity bez zmiany struktury. Kontynuacje proponuję obsłużyć kolejną, odrębną regułą.

## Proponowana reguła v1

Identyfikator: `possible_table_fragmentation`; mechanizm: `scan_work_inflation`.

1. Zbudować chronologiczne, zgodne zakresowo obserwacje. Obowiązkowe wejście:
   `table scan blocks gotten`, `table scans (short tables)`, `table scans (long tables)`
   i poprawna ekspozycja. Używać delt zapisanych w AWR, bez ponownego różnicowania.
2. Obliczać `blocks_per_scan = scan_blocks / (short_scans + long_scans)`.
   Mianownik zerowy/brakujący daje brak oceny z powodem. `table scan rows gotten`
   pozostaje kontekstem; nie jest mianownikiem użytecznej pracy ani liczbą żywych wierszy.
3. Wykrywać utrzymujący się wzrost względem zapisanej bazy, uwzględniając wzrost
   względny, absolutną dodatkową pracę oraz liczbę i ekspozycję obserwacji.
   Ocena wydajności powinna wymagać towarzyszącego kosztu: DB CPU/DB Time albo
   obserwowanego kosztu SQL. Sam duży, ale stale efektywny skan nie wystarcza.
4. Pokazywać kontrolę scans/s, execute/s i scans/execute. Nie nazywać aktywności
   statystycznie równoważną na podstawie samego braku trendu lub średnich bliskich sobie.
   Docelowy test równoważności wymaga jawnej tolerancji i przedziału niepewności;
   przy krótkiej próbie status pozostaje `inconclusive`. Jeśli pokazujemy wtedy hint,
   porównywalność pracy musi pozostać jego jawnym ograniczeniem.
5. Sprawdzić konkurencyjne wyjaśnienia: wzrost danych, miks SQL/obiektów, plan,
   projekcja, selektywność i praca CR/undo. Brak planów nie blokuje hipotezy,
   ale nie pozwala uznać planu za niezmieniony.
6. Przypisać status epizodu i zachować jego okres. Nie uzależniać całej reguły od
   `db_time_degradation_report.is_degradation_detected`: detector ostatniej ćwiartki
   może pominąć wcześniejszą degradację, gdy końcowe okna zawierają spadek kosztu.

Suma short+long jest propozycją dla porównywalnego, zwykłego skanowania. Oracle
osobno opisuje direct scans, skany IM, cache partitions i ROWID ranges w PX.
Nie wolno dopisywać wszystkich tych liczników do wspólnego mianownika bez potwierdzenia
ich relacji. Dla istotnej aktywności tych ścieżek lub nieznanego pokrycia v1 powinna
obniżyć zakres interpretacji albo zwrócić `not_assessable` dla tej reguły.
[Oracle 19c: Statistics Descriptions](https://docs.oracle.com/en/database/oracle/oracle-database/19/refrn/statistics-descriptions-2.html).

Nie wymagać wzrostu physical reads, waitów I/O, latch sleeps ani pogorszenia cache hit ratio:
ten zapisany przypadek ujawnił koszt CPU/logical I/O przy danych w cache.
Nie liczyć skorelowanych `consistent gets`, `session logical reads` i pin/fastpath
jako niezależnych głosów. Nie interpretować `no work - consistent read gets` jako pustych bloków.

### Baza i progi

W pierwszej implementacji zalecam porównania chronologiczne z zamrożoną bazą oraz
zachowanie wykrytych epizodów. Baza jawnie wskazana ma pierwszeństwo; automatyczny wybór
musi być deterministyczny, opisywać wybrany zakres i odmawiać oceny, gdy nie ma
porównywalnego okresu. Nie włączać narastającej degradacji do aktualizowanej normy.
Przerwy/restarty i zmiany scope wyznaczają granice analizy. Spadek aktywności pod koniec
obciążenia nie potwierdza naprawy. Wykryty historyczny epizod pozostaje na stronie HINTS.

Istniejący demonstrator w EmptyCalories zgłasza pierwszy sygnał od begin SNAP 203,
używając pierwszych 10 okien, +25% i trzech kolejnych przekroczeń. To wynik demonstratora,
nie przetestowana reguła JAS-MIN. Jego tolerancje wolumenu są pasmami, nie formalnym
testem równoważności. Parametry wzrostu, ekspozycji i trwałości muszą być wersjonowane
i sprawdzone na niezależnych przypadkach przed przyjęciem wartości domyślnych.
Do v1 nie potrzeba równoczesnego wdrożenia PELT, CUSUM i kolejnej rodziny regresji.

## Wskazywanie segmentów

Hint może istnieć bez segmentu. Kandydat wymaga dodatkowych przesłanek w tych samych
okresach: obserwowanego wzrostu segment Logical Reads i/lub kosztu powiązanego SQL,
z podaniem sposobu powiązania. SQL text wskazuje używane obiekty, nie dowodzi rzeczywistego
planu ani udziału obiektu w kosztach. Samo aktualne TOP Logical Reads nie wystarcza.
Ranking powinien preferować zmierzony przyrost i pokrycie, nie nazwę ani pojedynczy szczyt.

Nieobecność w TOP to obserwacja ocenzurowana, nie zero. Porównania robić na obserwowanych
oknach, osobno dla każdej domeny. Nie kopiować instancyjnych scan blocks na segmenty.
Nie sumować kosztów nakładających się obiektów i SQL jako odzyskiwalnego DB Time.

Obecny `SegmentStats` zachowuje OBJ#, DATAOBJ#, nazwę i typ, lecz pomija owner,
PDB/CON_ID i subobject. Oba parsery należy uzupełnić o dostępne w raporcie identyfikatory
przy implementacji bezpiecznego rankingu wielokontenerowego. Starsze dane pozostają
obsługiwane z jawnym niepełnym scope; nie łączyć niejednoznacznych nazw/ID. Zmiana DATAOBJ#
oznacza nową epokę fizyczną obiektu, nie dowód konkretnego DDL.

Fallback w karcie: **`No conclusive data to identify affected segments.`**
Krótki powód może rozróżniać brak zbioru segmentów, brak wspólnych obserwacji i niejednoznaczną tożsamość.

## Przykład karty HINTS

To przykład treści na podstawie wybranych okresów powyżej, nie wygenerowany wynik nowego modułu.

**Hint:** Possible table fragmentation detected

**Prove:** Average scan work increased from 152.29 to 3331.82 blocks/scan (21.88×),
while scan starts/s changed by −3.30%. DB CPU increased by 55.48% (+0.1222 s/s).
SQL `9paxwp1pabugh` increased from 189.75 to 5535.64 gets/execution and from
2.774 to 9.696 ms/execution. Baseline: SNAP 192–200; comparison: SNAP 214–215.

**Possibly affected segments:** ERP_SPARSE, OBJ# 76100, DATAOBJ# 76100 — observed
logical reads grew from 3790.70 to 128692.23/s; SQL text names this table.
The JSON lacks owner/PDB identity. ERP_DENSE remains near 2730 logical reads/s.

**Limitation:** Scan-work inflation is observed. Sparse blocks are a possible cause;
data growth and changed scan mix remain alternatives. Two recent observations do not
establish statistical workload equivalence. No direct segment-space measurement is supplied.

**Next check:** Verify the candidate SQL's actual scan plan and compare useful data
with space below HWM before selecting a remedy. No automatic MOVE/SHRINK/REBUILD.

## Moduł i dostęp dla AI

Zalecana architektura: deterministyczny `src/performance_hints.rs` z rejestrem reguł,
funkcją `build(collection, report, range, policy)` i rendererem HTML korzystającym
z tego samego serializowalnego wyniku. Każda reguła ma ID, wersję, wymagane dane,
parametry, warunki oraz testy kontrprzykładów. Brak zależności od dostępności LLM.

- Wynik: `PerformanceHintsReport { hints, rule_evaluations, policy }`.
- Hint: ID reguły/wersji, tytuł, scope i okna, status epizodu, obserwacje z jednostkami
  i źródłami, kandydaci segmentów, alternatywy, ograniczenia i krok weryfikacji.
  Bez niekalibrowanego procentowego prawdopodobieństwa fragmentacji.
- `rule_evaluations` odróżnia `not_triggered` od `not_assessable`. Pusta lista nie
  stwierdza zdrowia bazy. W UI krótka informacja o pokryciu i rozwijane przyczyny.
- `analyze.rs`: obliczyć po dostępnych analizach, zapisać np. `stats/performance_hints.html`,
  podpiąć stały przycisk **HINTS** w menu. Karty krótkie; szczegóły i źródła rozwijane.
- `ReportForAI.performance_hints`: ten sam wynik dla klasycznej analizy i zapisu JSON/TOON.
- Local agent/MCP: krótki indeks w seedzie, pełne dane przez
  `get_precomputed_analysis(section="performance_hints")` z filtrami i paginacją.
  Zachować standardową rejestrację dowodów MCP. AI ma sprawdzać alternatywy;
  hint nie zastępuje ustaleń końcowej analizy i nie wymaga kolejnego rozdziału raportu AI.

Można wykorzystać `measurements.rs` do ekspozycji, dostępności i dokładnych celów.
Koszty SQL w `access_path.rs` wymagają wydzielenia wspólnej funkcji i zachowania
oddzielnych masek obserwacji/wykonań gets, elapsed i CPU. Nie przenosić jego wymogu
`source == attributed` na natywne hinty: obecna gałąź celowo nie tworzy kandydatów
segmentów ze zwykłych TOP SQL, co blokowałoby pokazany przypadek.

## Co można przenieść z reasonings.txt

Aktywny plik wskazany przez `JASMIN_HOME` sprawdzono; jego §3.1 CHAINED ROWS
opisuje kontynuacje, nie gotowy detektor wzrostu blocks/scan. §1.4 USER I/O podaje
ograniczenia interpretacji skanów i I/O. Reguła pierwszego hinta wynika więc głównie
z eksperymentu i jego kontrprzykładów; nie jest prostym przepisaniem istniejącego paragrafu.

Kolejne reguły można implementować na podstawie `TRIGGER`, `REQUIRED EVIDENCE`,
`DO NOT INFER` i `ACTION`, ale każda wymaga doprecyzowania mierzalnego predykatu.
Nie wykonywać tekstu `reasonings.txt` jako reguł ani nie uzależniać działania menu od
automatycznej interpretacji tego pliku przez LLM. Zachować referencję do metodologii
oddzielnie od referencji do pomiarów.

## Minimalna akceptacja implementacji

1. Natywne AWR z pustymi `access_path_observations`: wykryty epizod, właściwe przesłanki,
   ERP_SPARSE jako kandydat, brak twierdzenia o udowodnionych pustych blokach.
2. Nested loops 2→5: brak inflacji bloków/skan przy większej liczbie skanów.
3. Live 8000 vs live 2000/deleted 6000: identyczna interpretacja identycznych liczników;
   zachowana alternatywa wzrostu danych.
4. Brak/zero mianownika, częściowe TOP, mała ekspozycja, scope i ścieżki PX/IM/direct:
   jawne ograniczenia, bez sztucznych zer i nieskończonych wzrostów procentowych.
5. Wcześniejszy epizod zachowany mimo późniejszego spadku kosztu/aktywności.
6. Te same wartości, identyfikatory i ograniczenia w HTML, klasycznym API i MCP;
   brak segmentów daje dokładny fallback w karcie; test układu, linków i escapingu nazw.
7. Osobny zestaw walidacyjny dla progów: rzeczywisty wzrost danych, zmiana planu/miksu,
   cykle obciążenia, CR/undo, cache, krótkie okna i różne wersje Oracle. Obecne laboratorium
   potwierdza wykonalność i ujawnia granice dowodu; nie mierzy trafności produkcyjnej.

Materiały źródłowe projektu: `oracle-EmptyCalories/jasmin_only/IMPLEMENTATION_SPEC.md`,
`REPORT_PL.md`, `native_analysis.json`, `analyze_native.py` oraz zapisane `calibration.json`.
