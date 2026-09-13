# JAS-MIN Data Journey / Podróż danych

## Uruchomienie

Otwórz **[journey.html](../journey.html)** w przeglądarce. Ten pojedynczy plik
działa lokalnie, bez internetu, serwera i instalacji. Wybierz Polski lub English.
Odtwórz/Pauza prowadzi przez całość; strzałki i suwak pozwalają przejść o krok,
a przyciski 13 stacji przeskoczyć do konkretnego etapu. Pełny ekran jest przydatny
na projektorze. Tempo 1× daje 6,5 sekundy na krok; 0,5× daje czas na omówienie.

134 kroki obejmują odczyt i wyrównanie danych, ograniczenia pokrycia, różnicowanie,
centrowanie, standaryzację, równania i eliminację Gaussa dla Ridge, walidację
chronologiczną i aktualizacje Elastic Net, MAD/wagi/IRLS Hubera, skalowanie celu
i ADMM Q95, kontrolę zbieżności, VIF/grupy, przywrócenie jednostek, wkłady do
przewidywania, MAD/P90/P99/max, unię TOP i interpretację zgodności modeli.

„Wejdź do obliczeń” rozwija dokładniejsze tabele i założenia. Pięć wierszy do
śledzenia zmienia **objaśnianą obserwację**, nie trening: wszystkie modele używają
1338 zmian z 1339 obserwacji i czterech wejść. Układ przestrzenny jest inspirowany
podróżami OraCity; stacje i wysokości pakietów są umowne. Wykresy, tabele i wyniki
pochodzą z policzonych wartości. Wybór innego modelu pokazuje osobną gałąź ze
wspólnymi wejściami, nie przekazuje współczynników jednego modelu do następnego.

## Granice dowodowe

- To rekonstrukcja edukacyjna równań z `src/gradient.rs`, `src/quantile.rs` i
  `src/tools.rs`, sprawdzonych 2026-09-13; nie instrumentacja programu Rust.
- Odczyt oryginalnych raportów jest pokazany schematycznie. Wejście liczbowe jest
  już sparsowane. Pozostałe rodziny modeli, pełna lista predyktorów, raportowanie
  wszystkich modułów oraz rzeczywista optymalizacja bazy nie są wykonywane tutaj.
- Zachowany przykład nie zawiera identyfikatorów organizacji ani źródeł. Nie
  zmieniono jego liczb. Brak identyfikatorów nie gwarantuje niepowiązywalności
  wzorców liczbowych; przed dystrybucją trzeba sprawdzić uprawnienia.
- Braki wpisów TOP były kodowane jako zera. Znane są sumy braków, nie ich maska
  wierszowa. Nie twierdzimy, że konkretne zero jest brakującym pomiarem.
- Różne długości raportów nie są normalizowane przez to różnicowanie.
- Q95 na tych danych osiąga limit **20 000 iteracji bez zbieżności**. Współczynniki
  są jawnie wstępne i wyłączone z TOP oraz zgodności. Nie zmieniamy historycznego
  Q95 w głównym kursie. Kontrola obejmuje normy reszt i lukę celu.
- Obliczono wszystkie iteracje. Odtwarzacz pokazuje wybrane, jawnie numerowane
  iteracje ADMM i IRLS; nie symuluje ich tempa ani rzeczywistej równoległości.
- TOP N=2 jest ustawieniem dydaktycznym. Wszystkie współczynniki pozostają
  dostępne; zgoda modeli i wielkości impact nie są dowodem przyczyny/oszczędności.

## English

Open **[journey.html](../journey.html)** directly in a browser. No server, internet,
installation, or upload is required. Choose English; use Play/Pause, previous/next,
the timeline, or any of 13 stations. Fullscreen supports classroom presentation.
1× gives 6.5 seconds per step; 0.5× leaves more time for explanation. Expand
“Inside: numbers and assumptions” for numerical tables. The five tracked rows
change the observation being explained, not the full 1,338-row training set.

The 134 steps cover preprocessing, all four regression branches and their
calculations, solver checks, predictor diagnostics, restored units, individual
predictions, ranking magnitudes, independent TOP unions, and evidence limits.
The spatial model follows the OraCity journey idea but is not a running process
monitor. Chart values are computed; packet heights are explicitly symbolic.

The engine reconstructs current local JAS-MIN equations for the retained
four-input example. It does not parse original reports, rerun the production
binary, fit all other report families, or validate future forecasts. It retains
missing-TOP-as-zero and unequal-interval limitations. Source-identifying metadata
is absent, but numeric patterns may still be linkable; check rights before
redistribution. Browser CSP blocks network connections and form submissions.

Q95 reaches 20,000 iterations **without convergence** and is excluded from TOP
and agreement. Historical course results are untouched. Every iteration was
computed; selected ADMM/IRLS iterations are displayed with their actual numbers.
TOP N=2 is a teaching setting, not a production-default claim. Model agreement
and impact magnitudes are neither causal proof nor savings estimates.

## Maintenance

From this directory:

```sh
node build.cjs
node test.cjs
```

`engine.cjs` is a pure numerical reconstruction. `build.cjs` reads only the
course's permitted `src/data.js`, writes `trace.js`, and bundles `../journey.html`.
`scenes.js` contains bilingual narration and numerical views; `player.js` draws
the map/charts and manages playback. No third-party runtime dependency is used.
The source page `index.html` is usable with a local static server as well.

Tests cover retained Ridge/EN/Huber values, all CV grid scores, a Q95 independent
reference copied from the existing Rust test expectations, ADMM identities,
stopping/gating, Gaussian invariants, units/rankings, all bilingual scene/row
combinations, script syntax, payload preservation and offline bundling. This is
not a claim of bitwise equivalence with Rust or a new database audit.

Browser checks on the standalone bundle covered PL/EN switching, station
navigation, next/last-step behavior, tracked-row selection, expandable numerical
details, play/pause advancement and fullscreen entry/exit. The two-column view
was inspected at 1440×1000; at 390px, the page reflowed to one column with no
document-level horizontal overflow. Browser error/warning logs were empty in
these checks. This is targeted UI verification, not a full accessibility audit.
