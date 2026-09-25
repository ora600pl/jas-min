# ONE MORE QUERY — The Evidence Distillery

Pięcioetapowy, interaktywny kurs PL/EN do prelekcji **“Stop being DBA. Stop being DEV. Performance tuning is math!”**.

**[Otwórz kurs / Open the course](https://ora600pl.github.io/jas-min/)**

Jeden samowystarczalny plik HTML, działający także offline. Bez kont, kluczy API, CDN, instalacji i przesyłania danych do AI. Język można zmieniać bez utraty etapu, wyborów ani ustawień. Ruch respektuje ustawienia dostępności i można go ograniczyć ręcznie.

## Pięć etapów

1. **Hałas:** wszystkie 1 339 obserwacji na wykresach; wspólna i osobne osie, wprowadzenie do AAS i wybór pierwszego tropu.
2. **Kontekst:** ilustracja ograniczeń wielkich załączników, edytowalne założenia budżetu tokenów i porównanie z małym, celowo wybranym zbiorem.
3. **Destylacja:** sześć zaworów — jakość danych, różnice, skala, modele, zbieżność i etykieta wyniku. Cztery piwne dymki objaśniają Ridge, Elastic Net, Huber i Q95.
4. **Sygnał:** dwa osobne wybory — model i wielkość zmiany P90/P99/MAX; rzeczywiste lokalne przeliczenie Ridge; sześć rozwijanych kroków pochodzenia wyniku; sześć eliminacji Gaussa i cztery podstawienia wsteczne. Wyjaśnienie wybranego modelu można otworzyć bez opuszczania rankingu.
5. **Jeszcze jedno:** sprawdzalna hipoteza i szablon żądania MCP, quiz o brakach, podgląd dokładnego JSON-u i eksporty.

### Wnioski operacyjne: wielkość wyniku przed miejscem w rankingu

W „04 Sygnał” przycisk pod porównaniem P90/P99 prowadzi do lekcji **„Złoty medal za mały problem?”**:

- Przy badaniu dużych incydentów zacznij od dużych wartości P99, sprawdź też MAX, a P90 wykorzystaj jako kontekst mniej skrajnych zmian. P90 nie oznacza „niegroźne”; trzy miary jednego dopasowania nie są niezależnymi potwierdzeniami.
- Suwak roboczego progu AAS wyróżnia pasujące tropy w wybranym modelu i rankingu. Początkowe 10 AAS to przykład dydaktyczny, nie domyślny alarm JAS-MIN ani próg uniwersalny. Cztery karty pokazują równocześnie P90, P99, MAX, pozycje, Share % i Typical Impact. Q95 pozostaje niedopuszczony.
- **Share %** dzieli wynik P90 przez sumę dodatnich wyników P90 całego dopasowania. To udział w punktacji, nie DB Time. Mianownik i przykład udziału cursor pin są obliczane z bieżącego modelu.
- **Typical Impact** to wielkość współczynnika w jednostkach źródłowych razy surowy MAD różnic z zachowanym znakiem. Rachunek na pięciu wymyślonych zmianach poprzedza przykład z prawdziwych danych. MAD mierzy rozrzut wokół mediany, nie średni rozmiar zmiany ani regularność.

Próg nie zmienia dopasowań, kwalifikacji modeli ani pakietu JSON. Zachowane pomiary pozostają niezmienione. Kolejnym krokiem diagnostycznym jest weryfikacja konkretnych okien, kierunku zmian, braków, SQL i sesji. Liczby tej lekcji pochodzą wyłącznie z czterocechowego zbioru kursu, a nie z innego raportu czy zrzutu ekranu.

Wyjaśnienia zaczynają się od problemu, który można samemu zobaczyć:

- **Ridge:** dwa przepisy identycznie pasują do historii. Rozsuń dwa oczekiwania i porównaj nowe przewidywania.
- **Elastic Net:** pomiar +10,6 AAS pozostaje stały. Zwiększ λ i obserwuj prognozę, błąd oraz dokładne zero mnożnika. Tabela oddziela pomyłkę w AAS od punktów za tę pomyłkę i od dodatków L1/L2. Wszystkie różnice oraz liczby 0,2 i 2,1 są wyprowadzone. Oba suwaki tego samego ćwiczenia są zsynchronizowane.
- **Huber:** wszystkie pięć pomiarów pozostaje stałych. Zmieniaj próg błędu i obserwuj przesunięcie prognozy oraz kompromis między błędami przy czterech zwykłych pomiarach i przy dużym skoku.
- **Q95:** 100 widocznych wyników przy takich samych wejściach. Przesuwaj prognozę i licz, ile wyników zostaje nad nią; porównaj średnią, poziom Q95 i maksimum. Osobne rozwinięcie pokazuje znaczenie częstości skoku, a „Sygnał” wyjaśnia różnicę między Q95 a Ridge × P99.

Dopiero rozwinięcie „Policz to krok po kroku” wprowadza wzory. Wszystkie miniatury są jawnie ilustracyjne i nie zmieniają autentycznych danych. Pochodzenie rzeczywistego progu Hubera, współczynników i rankingów nadal można prześledzić. Przy tej samej λ **niższa ocena dopasowania jest preferowana**; dobór λ wymaga innego porównania — błędów na późniejszych danych, bez dodatków regularyzacyjnych.

### Skąd parametry w JAS-MIN?

Każdy dymek ma widoczną sekcję o ustawieniach, sprawdzoną względem `src/cli.rs`, `src/analysis/gradient.rs` i `src/analysis/quantile.rs`:

| Model | Ustawione | Wyliczane z danych |
| --- | --- | --- |
| Ridge | λ = 0,05 domyślnie; `--ridge-lambda` zmienia wartość. Brak automatycznego doboru λ. | Mnożniki β przy wybranej λ. |
| Elastic Net | α = 0,2 domyślnie (`--en-alpha`). Opcjonalne `--en-lambda` narzuca stałą λ. | Domyślnie dobór λ przez sprawdziany na późniejszych oknach; następnie β. λ i α są stałe podczas końcowego dopasowania. |
| Huber | Reguła 1,345 × surowy MAD, minimum 0,000001; dodatek używa λ Ridge. | Próg δ raz przed dopasowaniem; potem β i wagi. Zachowany δ = 3,228 AAS. |
| Q95 | τ = 0,95 oraz λ = 0,0005 są stałymi implementacji. | β i wyraz wolny; niezbieżny wynik nie jest dopuszczony. |

Automatyczny Elastic Net sprawdza 40 proporcji λ względem maksimum wyliczanego z danych treningowych każdego podziału. Do pięciu chronologicznych sprawdzianów używa skalowania wyznaczonego wyłącznie na wcześniejszych danych. Porównuje średni kwadrat błędu przeskalowanego celu. Reguła one-standard-error wybiera najsilniejsze upraszczanie w granicy najlepszego średniego błędu plus jeden błąd standardowy tej średniej. Końcowa λ to wybrana proporcja razy maksimum policzone na całym zbiorze. Brak wystarczających sprawdzianów uruchamia wartość zastępczą 0,05 × maksimum; stały cel lub zerowy sygnał wszystkich wejść daje λ = 0.

Kurs **nie powtarza tego doboru**: zachowany ślad zawiera pięć podziałów, λ ≈ 0,870544 i α = 0,2, ale nie komplet ocen kandydatów. Początkowa λ Ridge = 0,05 pozostaje wartością domyślną, nie potwierdzonym optimum. Suwak „Sygnału” przelicza tylko Ridge; zachowane Huber/EN/Q95 nie zmieniają parametrów ani współczynników.

To kurs wprowadzający i interaktywny materiał po prelekcji, nie pełny podręcznik statystyki ani diagnoza trwającej awarii. Pubowa historia jest fikcyjna. Nie wykonujemy operacji Oracle, wywołań LLM ani MCP.

## Uruchomienie i budowanie

Otwórz `index.html` dwuklikiem. Opcjonalnie, z katalogu głównego repozytorium:

```sh
node docs/one-more-query/build.cjs
node docs/one-more-query/test.cjs
python3 -m http.server 8768 --bind 127.0.0.1 --directory docs/one-more-query
```

Build wymaga tylko Node.js (CI używa wersji 24). Korzysta wyłącznie z zatwierdzonego `sample.json` oraz bieżących źródeł. Nie potrzebuje żadnego poprzedniego kursu ani zewnętrznych raportów.

Aby przygotować paczkę WWW i katalog GitHub Pages:

```sh
node docs/one-more-query/package.cjs
```

Polecenie buduje i testuje kurs, tworzy ZIP, sprawdza jego zawartość i zgodność HTML oraz zapisuje SHA-256 w `dist/manifest.json`. Pakowanie używa standardowych programów `zip` i `unzip` dostępnych w macOS i w runnerze Ubuntu.

| Wynik | Zastosowanie |
| --- | --- |
| `dist/jas-min-upload.zip` | Rozpakuj i wgraj folder `jas-min` pod `https://www.ora-600.pl/jas-min/`. |
| `dist/site/` | Dokładnie dwa pliki dla GitHub Pages: `index.html` i `.nojekyll`. |
| `dist/manifest.json` | Sumy kontrolne paczki i strony oraz jawna lista publikowanych plików. |

Instrukcja wgrywania PL/EN znajduje się także w ZIP-ie i w [deploy/UPLOAD.md](deploy/UPLOAD.md). Lokalny `.htaccess` należy tylko do podkatalogu `jas-min`; nigdy nie zastępuj nim konfiguracji głównego WordPressa. Paczka nie jest automatycznie wgrywana na stronę firmową.

## GitHub Pages

Workflow [course-pages.yml](../../.github/workflows/course-pages.yml) publikuje kurs po zmianach jego źródeł na `main` lub po ręcznym uruchomieniu. Źródło witryny w ustawieniach Pages: **GitHub Actions**. Publikowany jest wyłącznie `dist/site`, a nie całe `docs` czy repozytorium.

Adresy:
- polski: https://ora600pl.github.io/jas-min/#pl/0
- English: https://ora600pl.github.io/jas-min/#en/0

Nie ustawiamy CNAME ani DNS dla `www.ora-600.pl`: firmowy hosting i GitHub Pages są niezależnymi kopiami tego samego pliku. Konfiguracja przepływu jest oparta na [oficjalnej dokumentacji GitHub Pages](https://docs.github.com/en/pages/getting-started-with-github-pages/using-custom-workflows-with-github-pages).

## Dane i granice interpretacji

Autor zatwierdził publikację nazw zdarzeń i liczb. Dane nie zawierają nazw organizacji, baz, hostów, SQL_ID ani dat pomiarów. Oryginalnych raportów nie dołączamy.

- Cel: historyczny, zaokrąglony Load Profile / AAS.
- Cztery cechy: sumy sekund oczekiwania w oknach. Długości okien nie zachowały się; nie udajemy ich normalizacji do AAS.
- Z 1 339 obserwacji powstaje 1 338 kolejnych różnic. Cechy są standaryzowane odchyleniem próby (N−1); cel jest centrowany.
- Braki uzupełnione zerami: 0, 1, 0, 963 w kolejności cech. Maska wierszy nie zachowała się. Nie można ustalić, które konkretne zera są pomiarami, a które wypełnieniem.
- Ridge jest liczony w przeglądarce; EN, Huber i Q95 korzystają z zachowanych dopasowań. Q95 nie spełnił kryteriów zakończenia po 20 000 iteracji i nie jest dopuszczony do kwalifikacji.
- Dodatni współczynnik w jednostkach źródłowych × P90/P99/MAX(|Δx|) to odpowiedź modelu na umowną zmianę jednej cechy przy pozostałych stałych. To nie zmierzony udział w DB Time, przyczyna incydentu ani odzyskiwalny czas.
- Finał, paragon i pakiet zawsze dotyczą **bieżącego Ridge × P99**. Ustawienia, brakujące wpisy i ograniczenia są eksportowane razem z wynikiem.
- Szablon MCP zawiera znaczniki `analysis_id` i `project_id`, które należy uzupełnić po wyborze projektu i `start_performance_analysis`. Nie udajemy istniejącej sesji ani odpowiedzi z SQL/blockerami.
- Licznik kontekstu to ilustracja, nie cennik, tokenizacja AWR ani specyfikacja konkretnego LLM. Porównanie bajtów JSON nie jest benchmarkiem kompresji całych raportów.

Przy bazowym Ridge λ = 0,05 liderem P90 jest PX (4,004093 AAS), a P99 — cursor pin (29,237083 AAS). Zmiana pytania zmienia kolejność, nie dowodzi przyczynowości.

## Testy i recenzja

[VALIDATION.md](VALIDATION.md) odróżnia wykonane testy od ograniczeń. Najnowszy wpis w [REVIEW.md](REVIEW.md) dokumentuje osobne recenzje każdego modelu i „04 Sygnał” przez Claude Opus 5.5, bez historii rozmowy z autorem, oraz końcową akceptację poprawek. Starsze wpisy zachowują historię wcześniejszych wersji.

Opcjonalny samodzielny test przeglądarkowy wymaga zainstalowanego Playwright oraz Chromium/Chrome:

```sh
node docs/one-more-query/browser-test.cjs
```

`PREVIEW_URL` zmienia adres serwera, `BROWSER_PATH` ścieżkę do przeglądarki, a `NODE_PATH` może wskazywać istniejące biblioteki. Test używa osobnego profilu i sprawdza także uruchomienie z pliku. Nie należy mylić historycznych wyników runnera z późniejszymi kontrolami w przeglądarce aplikacji.

## English

A five-chapter PL/EN introduction to regression-based Oracle performance analysis, with authentic anonymized measurements, worked calculations, interactive mini-labs, percentile comparisons and full Gaussian elimination/back substitution.

Open `index.html` directly, or visit [GitHub Pages](https://ora600pl.github.io/jas-min/#en/0). No installation, accounts, external assets, data uploads or live Oracle/LLM/MCP calls. Only Ridge is refitted live; the other models use retained numerical results. Q95 remains ineligible. Fitted association is not causal attribution.

Run `node docs/one-more-query/package.cjs` to build, test and create the upload ZIP. Upload its `jas-min` directory to your web root; do not overwrite WordPress's root `.htaccess`. The independent GitHub Actions workflow publishes only the two-file `dist/site` directory. No custom-domain or DNS changes are needed.
