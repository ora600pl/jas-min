# Przegląd reasonings.txt i propozycje rozszerzenia

Data: 2026-09-08. Przejrzano cały [reasonings.txt](</Users/inter/Library/Mobile Documents/com~apple~CloudDocs/Documents/ORA-600/scripts/oracle/audit_tools/performance/jasmin_home/reasonings.txt>): 1308 linii, 51 sekcji szczegółowych. Plik jest użytecznym katalogiem hipotez, ale w obecnej postaci potrafi skłaniać AI do nieuzasadnionego rozpoznania przyczyny i zbyt szerokiego strojenia. Przed rozszerzeniem należy poprawić przede wszystkim interpretację metryk, atrybucję SQL oraz reguły zamieniające samą korelację w diagnozę.

Gotowe **14 nowych wpisów po angielsku** znajduje się w [reasonings-additions-2026-09-08.txt](</Users/inter/Library/Mobile Documents/com~apple~CloudDocs/Documents/ORA-600/scripts/oracle/audit_tools/performance/jas-min/docs/reasonings-additions-2026-09-08.txt>). Jest to propozycja do przeglądu, nie zmiana aktywnej biblioteki. Samo dopisanie jej na końcu pozostawiłoby sprzeczności opisane poniżej.

Podstawa przeglądu: bieżący kod JAS-MIN, lokalne opracowania laboratoryjne i wybrane surowe wyniki oraz oficjalna dokumentacja Oracle 19c. W tej sesji nie wykonywano nowych eksperymentów na Oracle ani ponownej dekompilacji. Wyniki laboratoryjne zachowują zakres wersji/RU i platformy. Ustalenia CR/RPI pochodzą z archiwum wcześniejszych badań; nie są świeżym potwierdzeniem zachowania dowolnego środowiska.

SHA-256 oryginału: `0e0d41ec97e2209b02534be63a74be826700eaac9df3419ed1e595baaf3cc6a0`.
Bazowy HEAD JAS-MIN podczas przeglądu: `a345532`; analizowano także odczytany stan plików roboczych. W trakcie pracy pojawiły się zmiany innych prac w `src/`; niniejsza propozycja nie modyfikuje tych plików.

## Korekty o najwyższym priorytecie

Numery linii odnoszą się do wskazanego wyżej oryginału. P1 oznacza ryzyko błędnej diagnozy lub zalecenia, P2 — ograniczenie precyzji i użyteczności reguły.

| ID | Miejsce | Ocena i proponowana korekta |
|---|---|---|
| R01 · P1 | §10.1, 1146–1164 | `impact=0.35` **nie oznacza 35% wariancji**. Kod wylicza moduł współczynnika w skali surowej pomnożony przez MAD zmian predyktora. `impact_active` używa P90, a `impact_peak` P99. Wprowadzić dokładne definicje i rozróżnienie wyniku modelu od zmierzonego czasu; gotowy tekst w §12.2. Dowód: [build_ranking](</Users/inter/Library/Mobile Documents/com~apple~CloudDocs/Documents/ORA-600/scripts/oracle/audit_tools/performance/jas-min/src/gradient.rs:1517>). |
| R02 · P1 | §10.4–10.5, 1212–1238 | Q95 nie wybiera po prostu najgorszych 5% snapshotów; modeluje kwantyl warunkowy celu, tutaj jego zmian. Brak w top-N nie dowodzi wyzerowania przez Elastic Net. Etykieta `CONFIRMED_BOTTLENECK_EN_COLLINEAR` wynika z obecności w rankingach, bez obowiązkowego testu konkretnej pary współliniowej. Zachować etykietę serwera, lecz ograniczyć wniosek; §12.2. Dowód: [cross_model_classify](</Users/inter/Library/Mobile Documents/com~apple~CloudDocs/Documents/ORA-600/scripts/oracle/audit_tools/performance/jas-min/src/gradient.rs:1583>). |
| R03 · P1 | §1.2, §2.2, §4.3, §10.2–10.3, §11.1 | Zgodność modeli, korelacja SQL z waitem i wspólny klaster są tropami. Nie rozstrzygają, kto blokował, czy SQL był ofiarą, ani który mechanizm wykonał się w foregroundzie. Usunąć automatyczne `CONFIRMED ... root cause`; wymagać atrybucji, wielkości kosztu i zgodnego okna. Same korelacje statystyk nie identyfikują „true causal chain”. Uzasadnienie: konstrukcja modeli JAS-MIN oraz rozdzielenie dowodów w LAB-MUTEX/LAB-ESS. |
| R04 · P1 | §7.1, 1012–1013 | `wait_time_weighted_avg_s > 1` **nie dowodzi pojedynczych waitów >1 s**. Kod liczy `sum(get_requests_i * wait_time_i) / sum(get_requests_i)`, gdzie wejściowy czas pochodzi z wiersza aktywności latcha. To agregacja czasów snapshotów, nie `sum(wait_time)/sum(waits)`. Zmienić opis i wymagać osobnych danych o liczbie waitów/histogramie. Dowód: [agregacja](</Users/inter/Library/Mobile Documents/com~apple~CloudDocs/Documents/ORA-600/scripts/oracle/audit_tools/performance/jas-min/src/analyze.rs:2080>), [parser AWR](</Users/inter/Library/Mobile Documents/com~apple~CloudDocs/Documents/ORA-600/scripts/oracle/audit_tools/performance/jas-min/src/awr.rs:1138>). |
| R05 · P1 | §3.1, §5.3–5.4 i inne odwołania do segmentów | `top_10_segments_by_physical_reads` nie istnieje w aktualnym `ReportForAI`. Jest `top_10_segments_by_physical_read_requests`, a osobno `top_10_segments_by_direct_physical_reads`. Nie zamieniać nazwy bez uwzględnienia jednostki: requests nie są liczbą bloków ani bajtów. Sprawdzić też ogólne nazwy `elapsed_time_by_exec`/`number_of_executions` względem faktycznych pól `avg_*`/`stddev_*`. Dowód: [ReportForAI](</Users/inter/Library/Mobile Documents/com~apple~CloudDocs/Documents/ORA-600/scripts/oracle/audit_tools/performance/jas-min/src/reasonings.rs:883>). |
| R06 · P1 | §3.4–3.5, 597–629 | Sekcja `instance_stats_pearson_correlation` zawiera nazwę i współczynnik, **bez wartości licznika**. Nie można z niej policzyć hard-parse ratio ani hit ratio. Trzeba pobrać rzeczywiste szeregi/delty. Dla cache użyć `1 - Δphysical reads cache / (Δconsistent gets from cache + Δdb block gets from cache)`; nie traktować direct reads jako misses cache. Progi 90/95% nie wystarczają do rozpoznania zbyt małej pamięci. [Oracle — buffer cache](https://docs.oracle.com/en/database/oracle/oracle-database/19/tgdba/tuning-database-buffer-cache.html). |
| R07 · P1 | §1.3, 127–145; §2.4 | Skorelowany SELECT nie dowodzi posiadania blokady TX. Zwykły SELECT nie uruchamia „triggera na SELECT”; możliwe są wywołane funkcje, rekursja lub błędna atrybucja. Rozdzielić `FOR UPDATE`, czekającą transakcję i blokera. ITL wymaga m.in. właściwego eventu/trybu i danych o bloku, nie tylko TX+buffer busy na segmencie. TX mode 4 może dotyczyć również unikalności lub fragmentu bitmapy. [Oracle — TX/ITL](https://docs.oracle.com/en/database/oracle/oracle-database/19/tgdba/instance-tuning-using-performance-views.html). |
| R08 · P1 | §1.6, 205–228 | Unindexed FK to częsty kandydat dla TM, ale nie diagnoza każdego TM. Najpierw zidentyfikować obiekt, tryby blokad i operację. Obecny SQL kontrolny nie łączy po OWNER i nie weryfikuje poprawnie całego FK złożonego oraz jego pokrycia w początkowych kolumnach używalnego indeksu. Usunąć automatyczne „index ALL foreign keys”; ocenić konkretne relacje i koszt DML. [Oracle — TM](https://docs.oracle.com/en/database/oracle/oracle-database/19/tgdba/instance-tuning-using-performance-views.html). |
| R09 · P1 | §1.7, 236–250 | `gc ... grant 2-way` nie oznacza transferu bloku: przekazano uprawnienie do dostępu. `gc ... block busy` nie sprowadza się wyłącznie do flush redo; możliwe są pinning i współbieżność. Usunąć „>1 ms → interconnect problem”; porównać rodzaj żądania, kolejki i obsługę na obu instancjach. [Oracle — message/block/contention waits](https://docs.oracle.com/en/database/oracle/oracle-database/19/racad/monitoring-performance.html). |
| R10 · P1 | §1.1, 29–71; §6.1, 878 | Rozdzielić wolny zapis, częstotliwość synchronizacji, ilość redo i opóźnienia CPU/powiadomienia. `log file sync` nie jest samym czasem dysku. Nie zakładać jednego waita na COMMIT ani braku synchronizacji przy `user commits=0`. NOWAIT zmienia gwarancję trwałości, więc nie jest rutynowym tuningiem; rozważanie go wymaga świadomej zmiany kontraktu aplikacji. [Oracle — COMMIT](https://docs.oracle.com/en/database/oracle/oracle-database/19/sqlrf/COMMIT.html), LAB-SELECT-LGWR, LAB-ESS. |
| R11 · P1 | §1.2, 94–100 vs §6.3, 972–973 | Plik jednocześnie sugeruje obniżenie `_cursor_obsolete_threshold` do 1024 i ostrzega przed obniżeniem wartości 8192. To sprzeczna instrukcja, bez warunków RU i potwierdzenia konkretnego problemu. Zachować jako wskazówkę do sprawdzenia właściwej poprawki/MOS, usunąć domyślną zmianę. Treści KB147694 ani uniwersalnego defaultu nie potwierdzono tutaj w MOS. |
| R12 · P1 | §5.1, 789–809 | Aktywność słownika nie dowodzi, że aplikacja wykonuje CREATE/DROP. Nasz czysty SELECT uruchamiał rekursywny zapis ESS do `EXP_HEAD$`. Przed przypisaniem winy architekturze aplikacji wymagać rekursywnego SQL, callera i lifecycle obiektu; §12.7–12.8. Dowód: LAB-ESS. |

## Pozostałe korekty zwiększające precyzję

| ID | Miejsce | Zalecenie |
|---|---|---|
| R13 · P2 | §1.1, 32–34 | `FILESYSTEMIO_OPTIONS` dotyczy plików filesystemu i jest zależny od platformy. `ASYNCH` również włącza async I/O; wymaganie zawsze `SETALL` jest zbyt mocne. Nie stosować tej reguły automatycznie do ASM. [Oracle — FILESYSTEMIO_OPTIONS](https://docs.oracle.com/en/database/oracle/oracle-database/19/refrn/FILESYSTEMIO_OPTIONS.html). |
| R14 · P2 | §1.4, §8.1–8.2 | Wysoki czas Oracle I/O uzasadnia badanie ścieżki I/O, lecz progi 1/5/10 ms nie identyfikują awarii sprzętu. Dodać request size, throughput, kolejki, tail latency i baseline. Single-block read nie dowodzi konkretnego indeksu, a niskie latency przy dużej liczbie odczytów nie dowodzi brakującego indeksu. Zmienić kategoryczne rozpoznania na hipotezy do potwierdzenia planem i pomiarami. |
| R15 · P2 | §1.5; §4.3, 746 | RECYCLEBIN nie powinien być jedyną przyczyną `enq: CR - block range reuse ckpt`. Wymagać operacji ponownego użycia zakresu, obiektu i aktywności checkpointu. Z samej koegzystencji cache dictionary nie zalecać globalnego PURGE/RECYCLEBIN=OFF. Ten trop zostawić jako przypadek zależny od wersji, a nie uniwersalną receptę. |
| R16 · P2 | §1.8, 267–270; §3.7; §7.2 | Rozdzielić gorący blok tabeli i indeksu — tabela w rankingu nie dowodzi right-hand index contention. Splity podczas wzrostu indeksu mogą być oczekiwane. Przed REVERSE KEY/partycjonowaniem ocenić wzorzec dostępu i wymaganą obsługę range scans; nazwa latcha sama nie rozstrzyga wielkości cache. |
| R17 · P2 | §2.1–2.3; §5.2; §9.3 | Zmienny czas przy stałej liczbie wykonań nie dowodzi plan flip; sprawdzić blokady, CPU, bindy, fetch i zmianę danych. Obecność w 3–4 rankingach mierzy widoczność, nie dominację kosztu. Stosować rzeczywisty koszt i SLA. Agregat per-execution miesza różne SQL-e; najpierw porównać ten sam SQL i workload. |
| R18 · P2 | §2.2; §10.3 | CPU/elapsed wymaga spójnej jednostki i zakresu. Dla PX i PL/SQL rozdzielić coordinator/workers, czas skumulowany i wall time oraz koszty nadrzędne/wewnętrzne. Brak SQL w rankingu DB CPU nie dowodzi wait-bound. Dodać wymiar kosztu funkcji, który może zmienić CPU przy stałym LIO; LAB-ORE i §12.13. |
| R19 · P2 | §3.2–3.3 | Nie traktować `user logons cumulative`/`user logouts cumulative` jako gwarantowanych nazw Oracle. Zweryfikować dostępne nazwy, np. `logons cumulative` i `logons current`, oraz ewentualne etykiety własne źródła. Rosnąca liczba sesji może oznaczać rozgrzewanie puli; leak wymaga dłuższego przebiegu i danych aplikacji. „ASH misses ~99%” pozostawić jako wynik cytowanego przypadku, nie współczynnik korekcji każdego logon storm. [Oracle — statystyki](https://docs.oracle.com/en/database/oracle/oracle-database/19/refrn/statistics-descriptions-2.html). |
| R20 · P2 | §3.1; §3.8 | `table fetch continued row` nie lokalizuje samodzielnie tabeli i nie rozdziela każdej odmiany chaining/migration. MOVE nie gwarantuje usunięcia problemu szerokiego wiersza; obietnica „no downtime” pomija ograniczenia operacji. Przy ORA-01555 nie wystarcza porównanie parametru UNDO_RETENTION z długością zapytania — potrzebne są retencja faktyczna, przestrzeń i przebieg wykorzystania undo. Zastąpić obietnice automatyczne procedurą weryfikacji na konkretnym obiekcie. |
| R21 · P2 | §6.2, 894–908 | `SGA_MAX_SIZE > SGA_TARGET` to możliwość konfiguracji, nie dowód niekontrolowanego wzrostu. Parametry i HugePages oceniać z realnym przydziałem i polityką OS. Oddzielić stan konfiguracji od potwierdzonego wpływu wydajnościowego. |
| R22 · P1 | §6.2, 949–950 | `AUDIT_TRAIL=NONE` nie dowodzi braku audytu: po migracji do unified auditing parametr nie steruje tym mechanizmem. Sprawdzić tryb i aktywne polityki; nie wydawać diagnozy compliance z samej wartości parametru. [Oracle — AUDIT_TRAIL](https://docs.oracle.com/en/database/oracle/oracle-database/19/refrn/AUDIT_TRAIL.html). |
| R23 · P2 | §6.2, 960–964; §6.3 | Brak jawnego `PARALLEL_DEGREE_LIMIT` nie oznacza braku limitu — w 19c domyślnie `CPU`. Nie proponować stałych mnożników PX bez polityki, DOP i współbieżności. Wersje wprowadzenia/usunięcia parametrów i hidden defaults w §6.3 wymagają sprawdzenia dla konkretnego RU; obecnego katalogu nie traktować jako zweryfikowanej macierzy wersji. [Oracle — PARALLEL_DEGREE_LIMIT](https://docs.oracle.com/en/database/oracle/oracle-database/19/refrn/PARALLEL_DEGREE_LIMIT.html). |
| R24 · P2 | §1.11; §2.5 | Rozmiar fetch/arraysize i SDU to różne elementy. Czekanie na wysyłkę nie dowodzi nadmiaru danych bez rozmiaru wyniku i danych klienta. `JDBC Thin Client` nie identyfikuje pracy ad hoc — potrzebne są MODULE/ACTION/service i kontekst aplikacji. Idle wait nie wchodzi do standardowego DB Time; jego „% DB Time” nie należy odczytywać jako udziału w tym czasie. |
| R25 · P2 | §4.4; §9.1; §11.2 | Ciągłe lub wysokie MAD nie dowodzi problemu wymagającego architektury; może opisywać oczekiwany workload lub mały rozrzut bazowy. `found_in_pct_of_probes` to pokrycie/widoczność w źródle, nie zawsze częstotliwość fizycznego zjawiska. Usunąć automatyczny iloczyn gradient × pokrycie jako rzekomo obiektywny priorytet. Priorytet uzasadniać kosztem bezwzględnym, dotkniętym procesem i SLA. |

Powyższa lista łączy potwierdzone błędy semantyki/kodu z nadmiernie kategorycznymi heurystykami. Nie stanowi potwierdzenia każdego pozostałego defaultu, komendy DDL czy odwołania MOS. Takie szczegóły powinny być sprawdzane przy zastosowaniu reguły do wskazanej wersji i obiektu.

## Co wnoszą nowe wpisy

| Sekcja | Nowa zdolność diagnostyczna | Podstawa |
|---|---|---|
| §12.1 | Rozdzielenie obserwacji, hipotezy i przyczyny; delty, zakres, SQL/parent/recursive attribution | Kod i metodologia labów |
| §12.2 | Poprawna interpretacja impact/peak/share i etykiet modeli | Bieżący kod |
| §12.3 | Soft-parse storm mimo niskiego hard-parse ratio; wspólny parent/bucket między PDB | Runtime + statyczna analiza 19.25 |
| §12.4 | Producent checkpoint queue; fallback do partnera latcha i warunki faktycznego blokowania | Runtime + statyczna analiza 19.25 |
| §12.5 | Koszt odczytu CR i undo; reuse/trim zamiast założenia „get = clone” | Archiwum badania 19.32 |
| §12.6 | Cleanout generujący redo, ale bez sync w przebadanej ścieżce | Runtime + statyczna analiza 19.25 |
| §12.7 | SELECT czekający na LGWR przez autonomiczną transakcję, sekwencję lub ESS | Runtime + statyczna analiza 19.25 |
| §12.8 | Rozpoznanie ESS, tożsamości ekspresji/obiektu i granic ponownego tworzenia metadanych | Kontrole lifecycle 19.25 |
| §12.9 | Poprawna interpretacja `TOP_LEVEL_RPI_CURSOR` jako odmowy współdzielenia | Archiwum odwróconych prób 19.32 |
| §12.10 | Skalowanie CPU, rzeczywiste PX, nowe childy mimo fixed DOP/SPM | Kontrole runtime 19.32 |
| §12.11 | Rozróżnienie rozważanej transformacji od wybranego i wykonanego planu | Referencja 10053 + lab ORE |
| §12.12 | Asymetryczne kosztowanie podzapytania przed/po ORE; warunkowa rola fix 35365062 | 10053, dekompilacja, kontrola 0/1/0/1/0 |
| §12.13 | Koszt wywołań PL/SQL przy niemal stałych buffer gets | HPROF, plan i benchmark |
| §12.14 | Różnice NLS/bind metadata mimo identycznego SQL i pozornie tych samych ustawień | Historyczny case study 19c/Linux |

Szczególnie mocny nowy przykład to §12.13: lokalny [wynik analizy kosztów](</Users/inter/Documents/Oracle-SQLParsing/labs/1qz1t07s2ymbb/cbo-cost-summary.json>) zapisuje spadek liczby wywołań funkcji z 482 740 do 36 337 przy niezmienionych 36 116 startach podzapytania. Osobny [log kontroli przyczynowej](</Users/inter/Documents/Oracle-SQLParsing/labs/1qz1t07s2ymbb/97_cost_causal_recheck.log>) zawiera 25 pomiarów i potwierdzenie równości multizbioru wyników. To uzasadnia rozszerzenie reguły „CPU → mniej logical I/O”: koszt ewaluacji wyrażeń i funkcji jest osobnym wymiarem. Wynik dotyczy syntetycznego labu i nie stanowi obietnicy przyspieszenia produkcyjnych pakietów.

## Jak przygotować bibliotekę do użycia

1. Poprawić istniejące sekcje wskazane w R01–R12 i R22. Krytyczne korekty muszą znajdować się również w tych sekcjach, ponieważ retrieval może zwrócić np. samo §1.3, bez §12.1.
2. Dodać nowe sekcje z pliku propozycji. Zachować stabilne identyfikatory `§x.y`; tytuły zawierają nazwy waitów, statystyk i funkcji ułatwiające wyszukiwanie.
3. Dla każdej reguły stosować `TRIGGER → DIAGNOSIS → REQUIRED EVIDENCE → DO NOT INFER → ACTION → SCOPE/SOURCE`. Zwięzła sekcja powinna samodzielnie zawierać warunek rozstrzygający i granicę wniosku.
4. Pobrać katalog aktualnego MCP/schema przed stosowaniem reguły. Nie udawać, że HPROF, X$BH, `V$EXP_STATS`, ASH czy szczegóły REASON są zawsze dostępne w `ReportForAI`. Jeśli brakuje pomiaru, zapisać konkretną lukę i hipotezę, a nie przyczynę.
5. Poprawić odpowiadające im opisy w głównych instrukcjach AI. W odczytanym [src/reasonings.rs](</Users/inter/Library/Mobile Documents/com~apple~CloudDocs/Documents/ORA-600/scripts/oracle/audit_tools/performance/jas-min/src/reasonings.rs:995>) występują także uproszczenia CPU/wait-bound, Q95 jako „worst 5%” oraz przykład udziału `impact_share` jako procentu wariancji. Korekta wyłącznie zewnętrznego pliku pozostawiłaby konflikt. Również automatyczne opisy klasyfikacji w `src/gradient.rs` wymagają łagodniejszego języka przyczynowego.

Przykładowe brzmienia zastępujące najbardziej problematyczne zdania:

```text
§10.1: An impact score is a model-derived sensitivity magnitude in target
units. It is not a percentage of explained DB Time variance or a measured
reduction achievable by removing the predictor.

§7.1: wait_time_weighted_avg_s summarizes source-interval latch wait times
weighted by get requests. It is not mean latency per individual latch wait.

§1.3: A correlated SELECT is an investigation candidate, not evidence that
it acquired the blocking TX lock. Establish waiter, blocker, transaction
and current versus top-level SQL before identifying the locking mechanism.

§1.7: A gc grant wait records a global grant, not a received block. Diagnose
message latency, block transfer and busy-block serialization separately.

§5.1: Dictionary activity can originate from application DDL or recursive
Oracle work such as ESS registration. Identify the recursive SQL and its
caller before attributing the hotspot to application object creation.
```

## Źródła wpisów i granice dowodu

| Identyfikator | Źródło |
|---|---|
| JASMIN-CODE | [GuidanceLibrary](</Users/inter/Library/Mobile Documents/com~apple~CloudDocs/Documents/ORA-600/scripts/oracle/audit_tools/performance/jas-min/src/local_agent.rs:160>), [gradient.rs](</Users/inter/Library/Mobile Documents/com~apple~CloudDocs/Documents/ORA-600/scripts/oracle/audit_tools/performance/jas-min/src/gradient.rs:1517>), [analyze.rs](</Users/inter/Library/Mobile Documents/com~apple~CloudDocs/Documents/ORA-600/scripts/oracle/audit_tools/performance/jas-min/src/analyze.rs:2080>), [schemat](</Users/inter/Library/Mobile Documents/com~apple~CloudDocs/Documents/ORA-600/scripts/oracle/audit_tools/performance/jas-min/src/reasonings.rs:883>) — odczytane 2026-09-08. |
| LAB-MUTEX | [Cross-PDB library-cache mutex contention](</Users/inter/Documents/Oracle-SQLParsing/library-cache-mutex-noisy-neighbors.md>) — 19.25 AArch64; lab zawiera osobne childy, wspólne mutexy i pomiar soft-parse storm. |
| LAB-CHECKPOINT | [Checkpoint queues revisited](</Users/inter/Documents/Oracle-CheckpointQ/checkpoint-queue-revisited.md>) — 19.25 AArch64; ścieżki DML/COMMIT, preferowany latch, partner i blocking fallback. |
| LAB-SELECT-LGWR | [SELECT, log file sync i cleanout](</Users/inter/Documents/Oracle-CheckpointQ/select-log-file-sync.md>) — 19.25 AArch64; pozytywne próby autonomous/NOCACHE i kontrola cleanoutu. |
| LAB-ESS | [Expression Statistics Store i log file sync](</Users/inter/Documents/Oracle-CheckpointQ/optimizer-expression-store-log-file-sync.md>) — 19.25 AArch64; 10046, delty sesji, BPFtrace, Ghidra i kontrole lifecycle. |
| ARCHIVE-CR | [Archiwum badania CR z 2026-08-23](</Users/inter/.codex/memories/rollout_summaries/2026-08-23T16-23-34-L6jc-oracle_cr_clones_limit_and_reuse.md>) — 19.32 AArch64; wtórny zapis wcześniejszych dowodów, bez ponownego runtime w tej sesji. |
| ARCHIVE-RPI | [Archiwum badania RPI z 2026-08-23, Task 2](</Users/inter/.codex/memories/rollout_summaries/2026-08-23T12-33-24-IQlE-jasmin_rac_audit_top_level_rpi_cursor_semantics.md>) — 19.32 AArch64; istotna jest odwrócona kolejność direct/dynamic SQL, nie samo Y w widoku. |
| LAB-CPU-PX | [CPU_COUNT i child cursors](</Users/inter/Documents/Oracle-SQLParsing/cpu-count-child-cursors.md>) — 19.32 AArch64; kontrole serial/PX/fixed DOP/SPM/hotplug. |
| LAB-10053 | [Referencja transformacji 10053](</Users/inter/Documents/Oracle-SQLParsing/oracle-10053-query-transformations-en.md>) — 19.32 AArch64; klasy dowodu i kontroli, część przykładów runtime pozostała niezweryfikowana. |
| LAB-ORE | [Przyczyna kosztowa ORE](</Users/inter/Documents/Oracle-SQLParsing/labs/1qz1t07s2ymbb/cbo-cost-root-cause-pl.md>), [JSON wyników](</Users/inter/Documents/Oracle-SQLParsing/labs/1qz1t07s2ymbb/cbo-cost-summary.json>), [kontrola przyczynowa](</Users/inter/Documents/Oracle-SQLParsing/labs/1qz1t07s2ymbb/97_cost_causal_recheck.log>) — 2026-09-08, 19.32 AArch64, syntetyczny rozkład i funkcje labowe. |
| LAB-NLS | [NLS child cursor GDB case study](</Users/inter/Documents/Oracle-SQLParsing/nls_child_cursor_gdb_guide.md>) — historyczny lab 19c/Linux; nie wyprowadzono z niego uniwersalnego defaultu RU. |
| ORACLE-STATS | [Statistics Descriptions, 19c](https://docs.oracle.com/en/database/oracle/oracle-database/19/refrn/statistics-descriptions-2.html) — publiczne znaczenie nazw statystyk, nie dowód przebiegu wewnętrznej ścieżki. |
| ORACLE-ESS | [Query Optimizer Concepts — ESS](https://docs.oracle.com/en/database/oracle/oracle-database/19/tgsql/query-optimizer-concepts.html) i [In-Memory architecture — ESS](https://docs.oracle.com/en/database/oracle/oracle-database/19/inmem/in-memory-column-store-architecture.html) — semantyka ESS i niezależność od IM column store. |
| ORACLE-CURSORS | [V$SQL_SHARED_CURSOR](https://docs.oracle.com/en/database/oracle/oracle-database/19/refrn/V-SQL_SHARED_CURSOR.html) — publiczny kontrakt widoku; dokładne znaczenie RPI w propozycji ma dodatkową podstawę laboratoryjną. |
| ORACLE-CPU | [CPU_COUNT](https://docs.oracle.com/en/database/oracle/oracle-database/19/refrn/CPU_COUNT.html) — publiczna semantyka monitorowania CPU; szczegóły tworzenia childów pochodzą z LAB-CPU-PX. |

## Walidacja propozycji

- Sprawdzono kompletność struktury nowych wpisów, brak kolizji z 51 istniejącymi identyfikatorami oraz zachowanie numeracji `§12.1`–`§12.14`. Uruchomiono aktualne funkcje Rust `GuidanceLibrary::from_text` i `parse_guidance_heading`, wyodrębnione bez zmian do tymczasowego programu: oryginał 51 sekcji, dodatki 14, połączenie 65 unikalnych sekcji. Nie był to test rankingu wyszukiwania ani pełnego serwera MCP.
- Sekcje mają maksymalnie około 2,2 tys. znaków, aby można je było pobierać osobno. Parser indeksuje tylko nagłówki szczegółowe `§x.y`; wstęp i sam nagłówek `§12` nie zastępują warunków wpisanych w każdą regułę.
- Porównano formuły gradientów, agregację latchy i wskazane nazwy pól z kodem; nie wykonywano end-to-end analizy AI ani nowych testów Oracle.
- Aktywny `jasmin_home/reasonings.txt` zachowuje powyższy SHA-256. Zmiany dostarczone przez ten przegląd to wyłącznie dwa pliki propozycji w `docs/`.
