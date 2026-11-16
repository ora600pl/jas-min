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
