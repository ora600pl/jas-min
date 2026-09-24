# Recenzje dydaktyczne ONE MORE QUERY

## Aktualizacja 25 września 2026: problem przed regułą

Po uwagach autora wykonano **trzy kolejne rzeczywiste rundy konsultacji z Claude Opus 5.5**. Metryki każdego wywołania potwierdziły `claude-opus-5-5`. Recenzent otrzymał wyłącznie publiczne źródła i opis zmian, bez raportów źródłowych ani narzędzi. Recenzował tekst i matematykę; nie uruchamiał przeglądarki. Testy oraz publikację wykonano osobno.

### Uzgodniona konstrukcja

Każdy model zaczyna od sytuacji w bazie i widocznej konsekwencji. Czytelnik zmienia liczby, obserwuje prognozę w AAS, otrzymuje krótkie podsumowanie i wraca do zachowanych pomiarów. Wzory znajdują się w domyślnie zamkniętej sekcji. AAS i różnica między zmianą a końcowym poziomem są zdefiniowane na początku każdego dymka. Piwna anegdota wprowadza temat, ale go nie zastępuje.

| Model | Nowe doświadczenie |
| --- | --- |
| Ridge | Dwa przepisy idealnie pasują do historii współzmiennych oczekiwań. Rozsunięcie wejść ujawnia różnicę prognoz. Brak nowego pomiaru oznacza brak rozstrzygnięcia, kto ma rację. Dopiero potem pojawiają się najmniejsza suma kwadratów, lambda oraz sprawdzian na późniejszych oknach. |
| Elastic Net | Inne oczekiwania przewidują +10 AAS, pomiar to +10,6. Suwak zmienia niewyjaśnioną różnicę, przełącznik L1 pokazuje powstanie i znikanie przedziału zerowego mnożnika. Osobny wykres różnicy uwidacznia mały efekt. |
| Huber | Cztery zmiany +10 i jedna +30 AAS. Przesunięcie ostatniej do +40 zmienia średnią z 14 na 16, podczas gdy Huber z umownym progiem 3 pozostaje przy 10,75. Pokazano zarówno poprawę przy czterech pomiarach, jak i większy błąd przy piątym. |
| Q95 | Te same sytuacje, ale pytanie o wysoki wzrost. Częstość 1/5 kontra 1/21 zmienia odpowiedź z 30 na 10 przy tym samym maksimum. Średnia, Huber i Q95 są pokazane obok siebie jako odpowiedzi na różne pytania. |

### Krytyczne ustalenia i granice

- Wycofano jednoobserwacyjny pokaz Ridge `β = 2 / (1 + λ)`. Pokazywał mechanizm dodatku, nie powód jego użycia. Nowy rachunek porównuje dwóch kandydatów i nie nazywa żadnego dokładnym minimum całego Ridge. Nie dodano kolejnego, wymagającego osobnego założenia o wyrazie wolnym wzoru na współczynnik 0,495; Opus zaakceptował tę decyzję.
- Nie wdrożono doboru lambdy przez walidację. Opisano ocenianie błędów na późniejszych oknach, oddzielnie od funkcji dopasowania. Bazowe 0,05 nie jest przedstawiane jako potwierdzone optimum.
- Jedno okno w Elastic Net wyjaśnia rachunek, ale samo nie uzasadnia odrzucenia wejścia. Potrzeba sprawdzenia na dalszych danych jest jawna. Zachowano oznaczenie `c` dla brakującej zmiany w tej miniaturze, zamiast mieszać je z resztą `r` Hubera.
- Huber nie ocenia ważności incydentu. Wybrano porównanie błędów przy zwykłych i odległym pomiarze zamiast sumowania zmian AAS jako rzekomego całkowitego obciążenia. Próg 3 jest ilustracyjny: reguła MAD zastosowana do pięciu wartości dałaby dolne ograniczenie. Rzeczywisty próg 3,228 i końcowa waga około 0,761 są nadal odtwarzane.
- Główne Q95 ma dwa warianty częstości; graniczny płaski wynik 1/20 pozostaje w rachunku. Warstwy ćwiczeń mają osobne, oznaczone ustawienia. Nie przeliczają niezbieżnego Q95.
- Usunięto polemiki z prywatną rozmową, m.in. „magicznie daje zero”, i ujednolicono słownictwo oczekiwań. Poprawiono ujemne słupki, tekst Hubera po obu stronach progu oraz ułamkową średnią Q95.
- Ostatnia uwaga Opusa dotyczyła `-0` w Elastic Net. Wynik normalizuje teraz dokładne zero, a osobny test sprawdza `Object.is` i oba języki. Potwierdzono także dokładne zero zachowanego współczynnika i rzeczywisty błąd Hubera mieszczący się w zakresie suwaka.

Po usunięciu ostatniej uwagi Opus nie wskazał dalszych przeszkód do publikacji. To uzgodnienie redakcyjne i matematyczne, nie badanie skuteczności nauczania z kursantami. Liczby źródłowe, parametry prawdziwych modeli i wykluczenie Q95 nie zostały zmienione. Wyniki sprawdzeń: [VALIDATION.md](VALIDATION.md).

## Archiwum: wersja do recenzji 1

24 września 2026. Zakres: cała pięcioetapowa próbka ONE MORE QUERY, oba języki, opowieść, modele, mini-laboratoria, pochodzenie liczb, Gauss, finał i eksporty. To gotowa wersja do oceny przez autora, nie deklaracja ukończenia wielogodzinnego kursu ani diagnoza rzeczywistej awarii.

## Jak pracowaliśmy

Przeprowadzono dwie rzeczywiste rundy krytycznej konsultacji przez Claude Code. Metryki obu wykonań potwierdziły **claude-opus-5-5**. Opus otrzymał kompletny kod aplikacji i objaśnień oraz anonimowe metadane matematyczne potrzebne do sprawdzenia rachunków. Nie otrzymał źródłowych raportów, identyfikatorów organizacji ani danych dostępowych. Nie miał narzędzi, MCP ani możliwości edycji plików. Drugą rundę poprzedziły konkretne kontrargumenty i zmieniony kod.

Opus przeprowadził recenzję tekstu i matematyki, nie test przeglądarki. Testy aplikacji, kontrolę źródeł algorytmów i wdrożenie poprawek wykonał Codex. Konsultacja nie jest niezależnym certyfikatem poprawności ani badaniem skuteczności nauczania.

## Co zmieniliśmy w całej historii

| Etap | Decyzja po recenzji |
| --- | --- |
| Hałas | Krótka misja i wprowadzenie do okien, sekund oczekiwania oraz AAS. Trzy sesje przez dziesięć sekund pokazują różnicę między czasem sesji a czasem zegarowym. DBA i DEV mogą zarówno pomóc, jak i pochopnie wnioskować. Pierwszy wybór zachowuje fokus klawiatury. |
| Kontekst | Wyraźne rozdzielenie ilustracyjnego scenariusza 100 MB raportów od rzeczywistej macierzy około 25 kB. Tę małą próbkę można przekazać AI wprost; wartością modeli jest także powtarzalny rachunek i lepsze pytanie. Wybór strategii jest widocznie zaznaczony. |
| Destylacja | Rozróżnienie zmiany oczekiwania od AAS, ostrzeżenie przy skrajnym z, prostszy opis braków, poprawione liczebniki. Ridge jest liczony lokalnie; trzy inne dopasowania są zachowane. Q95 pozostaje niedopuszczony. Wyłączenie ruchu daje nieruchomy schemat z opisem i nie przebudowuje strony. |
| Sygnał | Pierwszy typ wraca obok wyników Ridge dla P90 i P99. Sześć kroków pochodzenia wyniku prowadzi do rzeczywistego układu równań. Gauss pokazuje sześć eliminacji, obie strony działania i cztery podstawienia od dołu. Usunięto mylące „−0” w macierzy. |
| Jeszcze jedno | Finał, paragon, proponowane żądanie MCP i JSON stosują jeden jawny kontrakt: **bieżący Ridge × P99**. Inne modele i miary są porównaniami. Quiz korzysta z rzeczywistego porównania P99/MAX dla resmgr i dynamicznej zgodności modeli. Finał łączy DBA i DEV wspólnym pytaniem oraz hasłem prelekcji. |

W dymkach pozostają wcześniejsze, szczegółowo sprawdzone mini-laboratoria: koszt błędu i L2, próg zerowania L1, rzeczywiste pochodzenie δ Hubera, asymetria kosztów Q95 i przykład, w którym Q95 nie wybiera maksimum. Ilustracyjne suwaki nie zmieniają prawdziwych obserwacji.

## O czym się spieraliśmy

**Wynik modelu nie jest zmierzonym udziałem w awarii.** Odrzucono zaproponowane w pierwszej recenzji zdanie sugerujące, że duża część konkretnego wzrostu AAS „była tym oczekiwaniem”. Wynik to dodatni współczynnik w jednostkach źródłowych pomnożony przez wybraną wielkość zmiany cechy. Opisuje reakcję dopasowanego modelu przy pozostałych trzech cechach stałych, nie dekompozycję incydentu. Opus wycofał pierwotne sformułowanie.

**Huber i skrajna wartość wejściowa to dwa różne tematy.** Dla pary 447 → 448 Huber rzeczywiście zmniejsza wagę do około 0,761. Nie przyjęto twierdzenia, że tej pary nie osłabia. Zachowano właściwe ograniczenie: kontrola dużego błędu przewidywania nie daje pełnej ochrony przed skrajnym wejściem. Nie wykonano leave-one-out i nie przypisano tej parze udowodnionego wpływu na cały model.

**Bieżące ustawienia, nie zamrożony paragon.** Opus początkowo proponował eksport stałego wyniku bazowego. Wybrano spójny eksport bieżącego Ridge, z λ na paragonie i w metadanych. Żądanie jest wyprowadzane z lidera rankingu, nie wpisane na stałe. Test obejmuje także kontrolowany kontrprzykład, w którym lider się zmienia. Suwak ma zatrzask przy bazowym λ = 0,05.

**Proweniencję trzeba sprawdzać w kodzie.** Opus słusznie zażądał dowodu, że zachowany Huber używa kary 0,05. Potwierdzono to w kodzie rekonstrukcji przed jej wycofaniem (pętla ważonych równań) oraz w `src/gradient.rs` (przekazanie ridge_lambda do Hubera). Dodatkowy test sprawdza ważone równania dla zachowanych współczynników. Metadane Elastic Net opisują α jako udział L1 i ujawniają skalowanie celu podczas dopasowania.

**Braków nie można odtworzyć z efektownej historii.** Nie zgadywano pozycji wypełnień zerem, długości okien ani klas konkretnych oczekiwań. Eksport macierzy zawiera teraz własne ostrzeżenia o brakach, nieznanych oknach, zaokrąglonym celu i pominiętych predyktorach. Nie dodano obietnic o odzyskaniu CPU czy AAS.

**Szablon MCP musi ujawniać brak kontekstu sesji.** Końcowa kontrola aktualnego serwera wykazała, że samo event_name/limit nie wystarcza: narzędzie wymaga analysis_id, a przy wielu projektach również project_id. Finał i pakiet pokazują więc oznaczone znaczniki do uzupełnienia po wyborze projektu i start_performance_analysis. Nie sugerują gotowego do wykonania żądania ani istniejącej sesji.

## Proponowana ścieżka Twojej recenzji

1. Na początku wybierz PX i przełącz wspólną skalę na osobne osie.
2. Porównaj scenariusz dużych raportów z rozmiarem naszej małej macierzy.
3. Przejdź sześć zaworów. W każdym kuflu otwórz matematykę i zmień jeden suwak.
4. Zobacz powrót pierwszego typu. Porównaj P90, P99 i MAX, a potem przejdź Gaussa od początku do ostatniego podstawienia.
5. Zmień λ Ridge. W finale porównaj paragon z podglądem JSON-u, wybierz żądanie dowodu i odpowiedz na quiz o brakach.
6. Przełącz język. Sprawdź, czy w którymś miejscu nadal trzeba „wiedzieć wcześniej”, aby zrozumieć wyjaśnienie.

Najważniejsze pytanie redakcyjne: **czy odbiorca umie własnymi słowami uzasadnić następne zapytanie do bazy, zamiast tylko wskazać największy słupek?** Nie mierzyliśmy tego jeszcze z kursantami.

Wyniki i granice testów: [VALIDATION.md](VALIDATION.md). Uruchomienie: [README.md](README.md). W chwili tej recenzji nie wykonano publikacji. Późniejszy proces udostępnienia opisuje README.md.

## English summary

Review edition 1 covers the complete five-chapter bilingual prototype. Two actual Claude Opus 5.5 review rounds challenged the narrative, numerical explanations and export contract. Adopted changes include an AAS primer, payoff for the initial guess, full Gaussian row arithmetic and back substitution, a real MAX-based quality quiz, and a consistent current-Ridge/P99 handoff.

We rejected interpreting regression scores as observed DB Time shares, corrected a claim about Huber downweighting, and verified retained model settings against the reconstruction and JAS-MIN source. The result is a review candidate, not a complete multi-hour course, causal diagnosis or validated learning intervention. No Oracle/LLM/MCP calls are performed by the course. Subsequent static publication is described in README.md.
