//! Localization is applied only to generated labels, never to source evidence.
#[derive(Clone, Copy)]
pub(crate) struct ReportLanguage(bool);

impl ReportLanguage {
    pub(crate) fn new(language: &str) -> Self {
        Self(matches!(
            language.to_ascii_lowercase().as_str(),
            "pl" | "pl-pl" | "polish" | "polski"
        ))
    }

    pub(crate) fn text<'a>(self, english: &'a str) -> &'a str {
        if !self.0 {
            return english;
        }
        match english {
            "Oracle Performance Analysis" => "Analiza wydajności Oracle",
            "Executive Summary" => "Najważniejsze wnioski",
            "Gradients, Anomalies and Workload Profile" => {
                "Gradienty, anomalie i profil obciążenia"
            }
            "Hypothesis Review and Statistical Synthesis" => {
                "Weryfikacja hipotez i synteza statystyczna"
            }
            "Overall Performance Profile and DB Time Degradation" => {
                "Profil obciążenia i pogorszenie DB Time"
            }
            "Wait Events" => "Zdarzenia oczekiwania",
            "SQL-Level Analysis" => "Analiza zapytań SQL",
            "Segments and Objects" => "Segmenty i obiekty",
            "Latches and Internal Contention" => "Zatrzaski i współbieżny dostęp",
            "I/O and Disk Assessment" => "Ocena operacji I/O i dysków",
            "UNDO, Redo and Load Profile" => "UNDO, redo i profil obciążenia",
            "Gradient and Anomaly Synthesis" => "Wnioski z modeli i anomalii",
            "Relevant Initialization Parameters" => "Istotne parametry bazy",
            "Prioritized Actions and Mandatory Assessments" => {
                "Działania według priorytetu i wymagane oceny"
            }
            "Next action" => "Następny krok",
            "Evidence capture" => "Zebranie pomiarów",
            "Mitigation" => "Ograniczenie skutków",
            "Durable fix" => "Trwała zmiana",
            "Unclassified (legacy)" => "Bez klasyfikacji (starszy format)",
            "technical" => "techniczni",
            "management" => "zarządzający",
            "mixed" => "techniczni i zarządzający",
            "Developer" => "Programista",
            "Management" => "Osoba zarządzająca",
            "IMMEDIATE" => "PILNY",
            "HIGH" => "WYSOKI",
            "MEDIUM" => "ŚREDNI",
            "LOW" => "NISKI",
            "critical" => "krytyczny",
            "high" => "wysoka",
            "medium" => "średnia",
            "low" => "niska",
            "unknown" => "nieznana",
            "informational" => "informacyjny",
            "Decision boundary" => "Czego jeszcze nie wiemy",
            "Scope" => "Zakres",
            "Confidence" => "Pewność wniosku",
            "Severity" => "Znaczenie",
            "Issue" => "Temat",
            "Canonical finding" => "Główny opis problemu",
            "Finding and evidence" => "Opis i dowody",
            "Complete action register" => "Pełna lista działań",
            "Affected workload" => "Obciążenie objęte analizą",
            "Temporal pattern" => "Kiedy występuje problem",
            "Evidence basis" => "Podstawa pomiarowa",
            "Diagnostic mechanism" => "Wyjaśnienie mechanizmu",
            "Original diagnostic conclusion" => "Pełny wniosek diagnostyczny",
            "Measurements, mechanism and supporting evidence" => "Pomiary, mechanizm i dowody",
            "Additional diagnostic context" => "Dodatkowy kontekst diagnostyczny",
            "Technical provenance" => "Źródła dowodów",
            "What we observed" => "Co zaobserwowaliśmy",
            "What this means" => "Co to oznacza",
            "Why" => "Dlaczego",
            "Success criterion" => "Kryterium powodzenia",
            "Decision rule" => "Jak wynik zmieni decyzję",
            "Procedure" => "Szczegóły działania",
            "Steps" => "Kroki",
            "Supports" => "Powiązane ustalenia",
            "Why grouped" => "Dlaczego to jeden temat",
            "Supporting findings" => "Ustalenia pomocnicze",
            "Issue register — scope and supporting findings" => {
                "Lista tematów — zakres i ustalenia pomocnicze"
            }
            "Structured technical evidence — complete tables" => {
                "Dowody techniczne — kompletne zestawienia"
            }
            "Structured technical evidence" => "Dowody techniczne",
            "Evidence coverage and limitations" => "Zakres dowodów i ograniczenia",
            "Mandatory Assessments" => "Wymagane oceny",
            "Methodology" => "Metodyka",
            "Why this SQL is here" => "Dlaczego analizujemy to zapytanie",
            "Measured workload impact" => "Zmierzony udział w obciążeniu",
            "Coverage and timing" => "Zakres i okres pomiarów",
            "Comparison context" => "Kontekst porównania",
            "Key operations" => "Najważniejsze operacje",
            "Access paths / joins" => "Ścieżki dostępu i złączenia",
            "Cardinality / row-source evidence" => "Liczba wierszy i pomiary operacji",
            "Partitioning / PX" => "Partycjonowanie i przetwarzanie równoległe",
            "Show complete plan" => "Pokaż pełny plan",
            "Show flagged paths only" => "Pokaż tylko wskazane ścieżki",
            "Plan-specific finding" => "Wniosek z planu wykonania",
            "PL/SQL-specific finding" => "Wniosek dotyczący PL/SQL",
            "Why this recommendation follows" => "Uzasadnienie zalecenia",
            "What this evidence cannot prove" => "Czego te dowody nie rozstrzygają",
            "Concrete action" => "Konkretne działanie",
            "Artifact classification" => "Rodzaj materiału",
            "Attachment status" => "Stan załącznika",
            "No evidence-backed findings were recorded for this section." => {
                "W tej części nie zapisano ustaleń popartych dowodami."
            }
            "No evidence-backed findings have been recorded." => {
                "Nie zapisano jeszcze ustaleń popartych dowodami."
            }
            "No prioritized actions were recorded." => "Nie zapisano działań z priorytetem.",
            "No action recorded; review the evidence boundary before intervening." => {
                "Nie zapisano działania; przed zmianą należy sprawdzić ograniczenia dowodów."
            }
            "No action recorded; review the decision boundary before intervening." => {
                "Nie zapisano działania; przed zmianą należy sprawdzić, czego jeszcze nie wiemy."
            }
            "Not supplied" => "Brak pomiaru",
            "No numeric active/peak pair was supplied for this fit. Coverage and actions remain below." => "Dostępne pomiary nie tworzą pary P90 i P99 dla tego modelu. Zakres danych i sposób weryfikacji opisano poniżej.",
            "active and peak model impact" => "wpływ modelowany przy typowym obciążeniu i w szczycie",
            "Each bubble is a contributor in one fit. Both axes use log(1+x); labels show original values. Bubble area represents the count of selected models, not saved CPU. Numbered points link to the table and exact source data." => "Każde koło oznacza jeden sygnał. Obie osie mają skalę log(1+x), a etykiety podają wartości źródłowe. Powierzchnia koła odpowiada liczbie modeli, które wskazały sygnał. Numer prowadzi do pomiaru w tabeli.",
            "Peak impact · P99" => "Wpływ w szczycie · P99",
            "Active impact · P90" => "Typowy wpływ · P90",
            "; active " => "; typowo ",
            "; peak " => "; w szczycie ",
            "known selections; " => "wskazań modeli; ",
            "of 4 flags supplied" => "z 4 znanych wyników selekcji",
            "Open evidence." => "Otwórz pomiar.",
            "Active:" => "Typowo:",
            "Peak:" => "W szczycie:",
            "Model selection and impact comparison" => "Porównanie wskazań modeli i skali wpływu",
            "Signal / source" => "Sygnał i źródło",
            "Peak ÷ active" => "Szczyt / typowo",
            "Next test" => "Kolejny pomiar",
            "Open source report" => "Otwórz raport źródłowy",
            "Exact model label:" => "Klasyfikacja źródłowa:",
            "Source:" => "Źródło:",
            "PEAK SIGNAL" => "SYGNAŁ W SZCZYCIE",
            "ACTIVE LOAD" => "TYPOWE OBCIĄŻENIE",
            "Largest supplied " => "Największy zaobserwowany wpływ ",
            " impact in this fit" => " dla tego modelu",
            "Gradient and cross-signal analysis" => "Analiza gradientów i współwystępowania sygnałów",
            "GRADIENTS × ANOMALIES × RUNTIME EVIDENCE" => "GRADIENTY, ANOMALIE I POMIARY",
            "Find the work. Isolate the peaks." => "Co obciążało bazę i kiedy pojawiały się szczyty?",
            "Read the model pattern, compare the evidence, then choose the test." => "Wskaźniki modeli pomagają wybrać godziny i operacje do sprawdzenia w pomiarach.",
            " scoped instances" => " — instancje",
            " separate fits" => " — dopasowania modeli",
            " supplied signals" => " — uwzględnione sygnały",
            "SHARED WORKING HYPOTHESIS" => "WSPÓLNA HIPOTEZA ROBOCZA",
            "TEST THAT CHANGES THE DECISION" => "POMIAR ROZSTRZYGAJĄCY",
            " Peak " => " W szczycie ",
            "Confidence: " => "Pewność wniosku: ",
            "limits and counterevidence" => "ograniczenia i dowody przeciwne",
            "Supporting evidence: " => "Pomiary źródłowe: ",
            "EXPLORE ONE FIT AT A TIME" => "WYBÓR MODELU",
            "Active load versus peak impact" => "Obciążenie typowe i wpływ w szczycie",
            "Signal family → target" => "Rodzina sygnałów → badana miara",
            "Combined model-impact indices; compare contributors within the selected fit. These are neither DB Time percentages nor recoverable CPU. Filled bubble area shows selected-model count; hollow rings indicate incomplete or no selection. Colour shows the selection pattern." => "Wskaźniki opisują względny wpływ sygnałów w wybranym modelu. Powierzchnia wypełnionego koła odpowiada liczbie wskazań. Pusty okrąg oznacza brak wskazań lub niepełne dane, a kolor pokazuje zgodność modeli. Oszacowanie oszczędności CPU wymaga odrębnego pomiaru.",
            "Model agreement" => "Zgodność modeli",
            "Tail / outlier" => "Szczyty i obserwacje odstające",
            "Explore all " => "Wszystkie sygnały: ",
            " supplied contributors · " => " · poza pierwszym zestawieniem: ",
            "Numbered shortlist: union of the three largest active and three largest peak impacts. All supplied numeric pairs remain on the plot. Missing values are never plotted as zero." => "Numery wyróżniają trzy największe wartości typowe oraz trzy największe wartości szczytowe. Wykres obejmuje wszystkie dostępne pary pomiarów.",
            "WHEN TO LOOK" => "PRZEDZIAŁY CZASOWE",
            "Observed windows to investigate" => "Godziny wybrane do dalszego sprawdzenia",
            "Selected from the supplied evidence subset. Co-occurrence is a capture target, not proof of one common cause." => "Godziny wybrano na podstawie dostępnych pomiarów. Gdy kilka sygnałów występuje jednocześnie, dane z tej samej godziny pomogą sprawdzić, czy mają wspólną przyczynę.",
            "How to read this analysis" => "Jak czytać tę analizę",
            "Active / peak:" => "Typowo i w szczycie:",
            "source combined P90 / P99 impact across eligible model outputs. Percentiles include zeros: zero active impact does not rule out a peak, and P99 is not the maximum. Different target and predictor families are separate fits. Do not add their scores or treat a high score as saved seconds." => "Wskaźniki łączą wyniki kwalifikujących się modeli przy P90 i P99. Percentyle uwzględniają zera. Sygnał o typowej wartości zero może pojawić się w szczycie. P99 oraz maksimum opisują różne obserwacje. Każde dopasowanie opisuje odrębną rodzinę predyktorów i badaną miarę.",
            "Model selection:" => "Wskazania modeli:",
            "a filled marker reproduces source top-selection membership. Gradient v2 combines independently ranked active, peak and extreme selections; consult fit diagnostics and observation coverage. A dash is not a zero coefficient or proof of collinearity. A question mark means no selection evidence was supplied. Models share the same data; agreement is not independent causal proof." => "Pełny znacznik oznacza sygnał wybrany przez model. Gradient v2 uwzględnia oddzielne rankingi dla obciążenia typowego, szczytowego i maksymalnego. Kreska oznacza pozycję poza wyborem modelu, a znak zapytania brak informacji o wyborze. Modele korzystają ze wspólnych danych. Przyczynę należy sprawdzić w pomiarach.",
            "Precision and scope:" => "Dokładność i zakres:",
            "displayed values are rounded; hover a number for the original value. Peak ÷ active is undefined at zero active impact and can be unstable near zero. Raw tables, exact values, method settings and provenance remain in the complete technical evidence below." => "Wartości na wykresie zaokrąglono. Po wskazaniu liczby pojawia się dokładny wynik. Iloraz ma sens przy dodatnim mianowniku i jest wrażliwy na bardzo małe wartości typowe. Tabele źródłowe zawierają pełne wartości, ustawienia modeli i pochodzenie pomiarów.",
            "Active" => "Typowo",
            "Peak" => "W szczycie",
            "Active " => "Typowo ",
            "Instance" => "Instancja",
            "Recurring" => "Sygnał powtarzalny",
            "Sparse" => "Selekcja rzadka",
            " more" => "",
            "Four-model selection" => "Wskazanie czterech modeli",
            "Recurring contributor" => "Powtarzalny sygnał",
            "Tail-sensitive selection" => "Wskazanie w szczytach",
            "Outlier-sensitive selection" => "Wskazanie zależne od obserwacji odstających",
            "Sparse-model selection" => "Wskazanie modelu rzadkiego",
            "Robust-model selection" => "Wskazanie modelu odpornego",
            "Partial / unavailable selection" => "Wybór częściowy lub brak wyniku",
            "selected in source classification" => "model wskazał sygnał",
            "not selected in source classification; not zero cost" => "sygnał poza wyborem modelu",
            "selection not supplied" => "brak danych o wskazaniu",
            "Main JAS-MIN dashboard" => "Główny panel JAS-MIN",
            "Load profile" => "Profil obciążenia",
            "Secondary load profile" => "Uzupełniający profil obciążenia",
            "Interactive source reports" => "Interaktywne raporty źródłowe",
            _ => english,
        }
    }

    pub(crate) fn severity<'a>(self, value: &'a str) -> &'a str {
        if !self.0 {
            return value;
        }
        match value {
            "critical" => "krytyczne",
            "high" => "wysokie",
            "medium" => "średnie",
            "low" => "niskie",
            "informational" => "informacyjne",
            _ => value,
        }
    }

    pub(crate) fn choose<'a>(self, english: &'a str, polish: &'a str) -> &'a str {
        if self.0 {
            polish
        } else {
            english
        }
    }
}
