# Matematyka regresji / Regression mathematics

**Animacja dla prowadzącego / Classroom animation:** [JAS-MIN Data Journey](journey.html)
— 13 stacji, 134 kroki obliczeń, PL/EN, lokalnie bez serwera. / 13 stations,
134 calculation steps, PL/EN, offline without a server.
[Opis i granice modelu / Guide and evidence limits](journey/README.md).

## Polski

Otwórz **[index.html](index.html#pl/start)** w przeglądarce. Plik zawiera cały kurs
i działa lokalnie, również bez internetu. Nie wymaga instalowania bibliotek,
Pythona, Node.js ani uruchamiania serwera. Przełącznik **Polski / English** zmienia
treści, instrukcje laboratoriów, podpowiedzi, rozwiązania i komunikaty.

Kurs zawiera 16 lekcji i 32 zadania. Prowadzi od obserwacji i iloczynu skalarnego,
przez standaryzację, macierze, pochodne, równania normalne, ręczną eliminację
Gaussa i uwarunkowanie, do Ridge, spadku gradientowego, Elastic Net, walidacji
chronologicznej, Hubera oraz Q95/ADMM. Zwieńczeniem jest odtworzenie rzeczywistego
czterowejściowego modelu Ridge i rozłożenie przewidywania na wkłady.

Kurs prowadzi jedną historię: Marta analizuje czas zajęcia stanowiska w browarze,
a Kuba przenosi jej rozumowanie na wydajność bazy. Najpierw pojawia się problem
i pomiar, potem rachunek z jednostkami, dopiero na końcu symbol i nazwa metody.
Współczynnik 0,6 min/L wynika z porównania 40 L / 120 min i 50 L / 126 min,
a nie z polecenia przyjęcia niezrozumiałej stałej. Trzeci pomiar pokazuje, dlaczego
trzeba dopasować cały dziennik zamiast wybrać jedną wygodną parę.

Dane browaru są **wymyślonymi przykładami matematycznymi**, nie recepturami,
poradami procesowymi ani źródłem pomiarów Oracle. Czas obejmuje zajęcie stanowiska
do zwolnienia po warzeniu, nie fermentację i dojrzewanie. Kurs rozróżnia odniesienie
do konkretnej warki, sąsiednie różnicowanie, centrowanie i skalowanie. Małe siatki
generowane do sprawdzania solverów są jawnie oddzielone od obserwacji w historii.
Równania 3×3 pozostają ćwiczeniem czysto algebraicznym.

Ta redakcja używa nowych zadań i osobnego zapisu postępu (wersja 3). Zaliczenia
starszych pytań nie zaliczają automatycznie nowych; poprzedni zapis nie jest usuwany.

W warsztacie Gaussa można przełączać układy, oglądać wszystkie etapy z wyborem
elementu głównego i podstawianiem wstecznym, wykonywać własne operacje na
wierszach, cofać je i sprawdzać rozwiązania w oryginalnych równaniach.

Zmiana języka zachowuje bieżącą lekcję i zaliczone zadania, lecz resetuje
laboratorium i usuwa jego importowane dane z aktywnego widoku. Postęp jest domyślnie
przechowywany tylko w karcie. Opcjonalny zapis w `localStorage` dotyczy wyłącznie
języka, lekcji i zaliczonych zadań; nie zapisuje plików ani wyników importu.

### Rzeczywiste pomiary i prywatność

Osadzony przykład zawiera zachowane wartości liczbowe: 1339 obserwacji,
1338 różnic i cztery miary oczekiwań Oracle. Usunięto nazwy organizacji,
instancji, daty, oryginalne identyfikatory raportów, ścieżki i skróty źródeł.
Pozostały tylko jawnie wybrane liczby, ogólne nazwy miar i parametry modeli.
Numery w interfejsie to lokalne pozycje wierszy.

Nie ukryto ograniczeń merytorycznych: zastępowania brakujących wpisów zerami,
różnych jednostek, braku normalizacji czasu raportów, zaokrągleń eksportu i braku
zbieżności historycznego Q95. To rekonstrukcja edukacyjna, nie aktualny pełny
ranking produkcyjny JAS-MIN. Aktualny kontrakt opisuje
[metodologia regresji](../gradient-methodology-v2.md).

Brak etykiet identyfikujących nie gwarantuje niepowiązywalności wzorców liczbowych.
Przed dalszą dystrybucją rzeczywistych pomiarów należy sprawdzić uprawnienia.
Kurs nie przesyła danych: brak usług zewnętrznych, analityki, zdalnych fontów
i bibliotek; polityka CSP blokuje połączenia i wysyłanie formularzy.

### Własny CSV, wyłącznie lokalnie

Ostatnia lekcja umożliwia import **tylko do Ridge**, bez walidacji holdout i bez
ponownego dopasowywania innych zapisanych modeli. Wymagania:

- Nagłówek dokładnie `y,x1,x2,x3,x4`, następnie 6–50 000 wierszy liczbowych.
- Separator przecinek, separator dziesiętny kropka. Bez cytowanych pól, pustych
  komórek, identyfikatorów i dat. Do 2 MiB; moduł wartości do 1 miliarda.
- Opcja różnicowania zamienia kolejne obserwacje w sąsiednie przyrosty; jej
  wyłączenie dopasowuje podane wiersze bez tego kroku. Zachowaj kolejność.
- Cel jest centrowany, wejścia skalowane odchyleniem próbkowym, λ=0,05.
  Stałe kolumny stają się zerowe i są oznaczone. Jednostka celu pochodzi z `y`.

Przykład syntetycznego CSV do ręcznego zapisania:

```csv
y,x1,x2,x3,x4
2,1,0,2,0
3,2,1,1,0
4,3,0,3,1
5,4,1,2,1
6,5,0,4,0
7,6,1,3,0
```

## English

Open **[index.html](index.html#en/start)** in a browser. It is a complete offline
file, requiring no server, dependencies or runtime installation. The language
selector translates all lessons, lab instructions, questions, hints, worked
answers and feedback. Language changes retain the lesson and checkpoint progress
but restart the current lab and clear its imported data from the active view.

The 16 lessons and 32 checkpoints cover observations, dot products,
standardization, matrices, derivatives, normal equations, guided and manual
Gaussian elimination, rank/conditioning, Ridge, gradient descent, Elastic Net,
chronological validation, Huber and Q95/ADMM. The capstone reconstructs a recorded
four-input Oracle Ridge fit and explains each prediction contribution.

One continuous story follows Marta investigating brewhouse station time and Kuba
transferring that reasoning to database performance. A question and measurements
come first, a calculation with units comes next, and symbols and method names
come last. The first coefficient, 0.6 min/L, is derived from 40 L / 120 min and
50 L / 126 min; a third observation motivates fitting the entire log.

Brewhouse numbers are **invented teaching examples**, not recipes, process advice
or the source of the Oracle measurements. Station time ends at release after
brewing, excluding fermentation and maturation. Each transformation distinguishes
reference-relative changes, adjacent differences, centering and scaling.
Constructed solver grids are explicitly labelled, not passed off as evidence for
the coefficients used to generate them. The 3×3 preset remains pure algebra.
The Gaussian workbench includes pivoting, elimination, back-substitution, manual
row operations, undo, hints and original-equation verification.

This revision has new questions and a separate version-3 progress record. Older
answers do not automatically pass the revised questions; the older record is not
deleted.

The recorded numerical example retains 1,339 observations, 1,338 differences,
four generic Oracle wait metrics and model parameters. Organization names,
instances, timestamps, original report IDs, paths and source hashes are excluded.
Displayed identifiers are local row ordinals. An explicit allowlist produces the
payload; arbitrary source strings are never copied. Removing identifiers does
not guarantee that numerical patterns cannot be linked to other records. Check
authorization before distributing real measurements.

Coverage limitations remain explicit: missing top-wait entries encoded as zero,
unlike units, unequal reporting durations, rounded export and a historical Q95
fit that did not converge. The course is not a new production run. Consult the
[current regression methodology](../gradient-methodology-v2.md) for production
contracts, including Q95 objective-gap certification.

The optional CSV importer fits **Ridge only**, with λ=0.05, a centered target and
sample-standardized inputs. It does not run holdout validation or refit the other
recorded models. Use header `y,x1,x2,x3,x4`, decimal points, commas, 6–50,000
numeric rows and at most 2 MiB; no quoted cells, missing values or identifiers.
Magnitude is limited to one billion. Optionally difference adjacent rows first,
preserving order. Constant columns become zero columns with an explicit label.
Target units are whatever the supplied `y` column measures.

No selected file is uploaded or persisted. There are no external scripts, fonts,
analytics or services. CSP blocks connections and form submission. Progress is
tab-local by default; opting in stores only language, lesson and solved
checkpoints in this browser. Disable the option to remove that saved record.

## Maintenance / Utrzymanie

Edit files under `src/`, then rebuild the standalone entrypoint:

```sh
python3 build_course.py
node test_course.cjs
python3 -B test_example.py
```

`index.html` is generated and maintained alongside its sources. `src/data.js`
contains only the permitted numeric teaching payload. `prepare_example.py`
accepts an explicitly supplied local trace path and exports an allowlist; it
never records that path. Replacing the example requires a separate privacy review
and corresponding numerical reference-test updates. Do not add raw source files,
archives or private provenance to this directory.

### Teaching continuity / Ciągłość dydaktyczna

The fictional numerical fixtures live in `CourseMath.brewing` in `src/math.js`.
The live labs and arithmetic tests share these fixtures. In particular:

- Pilot comparisons derive coefficients `[0.6, -2, 4]` with their declared units;
  batch E is predicted at 134 min against an invented observation of 136 min.
- The same three-batch table `[40,50,60] L` / `[120,126,129] min` feeds matrices,
  loss, normal equations and gradient descent, using `x=(volume-40)/10` and
  `y=time-120`. Fixing `a=0` gives `b=4.8`; fitting both gives `a=0.5,b=4.5`.
- Exact Gaussian comparisons solve volume/interruption multipliers `[6,4]`.
- Ridge and Elastic Net use declared constructed grids in ten-litre/two-degree
  units, not sample-standardized z-scores. The recorded Oracle example retains
  its own preprocessing, parameter values and limitations.
- Huber, quantile and ADMM labs explicitly restore the 120-minute reference
  when presenting total durations.

The teaching sequence uses worked examples, a small change to predict before
calculating, intermediate-state inspection and transfer back to an Oracle
question. It does not claim experimentally measured learning outcomes.

Browsers that implement the optional WebMCP interface can expose lesson
navigation, a read-only progress/matrix summary and manual row operations to an
agent. These reuse the visible controls and do not expose imported CSV contents.
Unsupported browsers simply skip registration. This optional interface has not
been runtime-verified in a supporting browser.

Validation covers mathematical fixtures, row-operation invariants, model
optimality conditions, recorded numerical reconstruction, CSV errors, bilingual
content parity, JavaScript syntax, privacy schema and offline bundling. It is
not browser visual/accessibility QA or a fresh production regression run.
