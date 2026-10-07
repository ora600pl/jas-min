# JAS-MIN Companion

Lokalna wtyczka Codex: zakapturzony awatar, wykresy i tabele z JAS-MIN MCP,
sterowane w rozmowie tekstowej lub natywnym głosem Codex. Bez klucza OpenAI API,
osobnego modelu, Realtime API, nagrywania mikrofonu i syntezy mowy w panelu.
Natywny głos i praca Codex zużywają limity planu użytkownika.

## Instalacja

Wymagania: lokalny Codex desktop, CLI `codex`, Node.js 22+ (sprawdzone na 24.18.0).
Nie trzeba uruchamiać `npm install` — paczka nie ma zależności.

Z głównego katalogu repozytorium:

```sh
node plugins/jasmin-companion/install.mjs
```

Instalator rejestruje repo jako marketplace `jasmin-local`, instaluje i włącza
`jasmin-companion@jasmin-local`. Format przenośny `mcp.json` uruchamia zawarty
w paczce `./launch.sh`, który znajduje serwer niezależnie od katalogu projektu.
Node 22+ musi być dostępny w PATH procesu desktop. Dla starszego CLI instalator
generuje także ignorowany przez Git `.mcp.json` z lokalnymi ścieżkami Node i serwera.
Po zmianie kodu uruchom instalator ponownie. Na macOS domyślnie używa silnika
z aplikacji ChatGPT desktop; `CODEX_CLI` pozwala wskazać inny plik wykonywalny.

**Przeładuj Codex, aby załadować nowy skill i narzędzia MCP.** Następnie wpisz:

> Użyj JAS-MIN Companion. Otwórz demo, pokaż DBTime i opowiedz, co się dzieje.

Możesz też wybrać skill `jasmin-companion` z selektora `$`.
Włącz natywną rozmowę głosową w tym samym czacie. Powiedz np.:

> Pokaż waity w szczycie obciążenia. A teraz najdroższe SQL-e z tego snapshotu.

Codex otwiera panel w swojej przeglądarce i wywołuje narzędzia wtyczki. Każde
zapytanie dodaje dowód do historii. Kliknij punkt lub SQL, a następnie powiedz
„przeanalizuj ten snapshot / ten SQL”. Skill odczytuje wybór przez
`companion_context`. Samo kliknięcie nie wysyła wiadomości do modelu.

## Rzeczywiste dane

Uruchom istniejący JAS-MIN z wybranym plikiem kolekcji, np.:

```sh
./target/release/jas-min -j "/pełna/ścieżka/kolekcja.json" --security-level 2 --mcp 127.0.0.1:4242/mcp
```

W Codex:

> Otwórz JAS-MIN Companion w trybie live. MCP jest pod http://127.0.0.1:4242/mcp.
> Pokaż dostępne projekty i obciążenie wybranej bazy.

Wtyczka pobiera listę projektów i schematy narzędzi z serwera. Tworzy własną sesję
analizy, automatycznie dopisuje `analysis_id` i `project_id`, zachowuje identyfikatory
dowodów. Obsługuje 13 narzędzi do odczytu; nie wystawia narzędzi zapisu raportów.
Adres MCP musi być lokalny; dla zdalnego serwera można użyć tunelu SSH.
Tryb live nie przełącza się sam na dane syntetyczne po błędzie połączenia.
Skill pobiera dokładne nazwy metryk przez `list_available_metrics`: starszy serwer
DNV zwraca serię dla `DB Time(s)`, ale pustą serię dla skrótu `DBTime`.

## Architektura i granice

```text
Natywny głos / tekst Codex
          │
Skill → MCP stdio wtyczki → JAS-MIN MCP przez HTTP
          │
          └─ prywatny panel HTTP na 127.0.0.1, losowy port i URL sesji
```

Każdy proces MCP ma własny panel i sesję. Po zakończeniu procesu panel przestaje
działać; ponownie otwórz go przez `companion_open`. Historia do 20 widoków jest
przechowywana w pamięci. URL panelu daje dostęp do danych tej sesji — nie publikuj go.
Dane zwrócone narzędziami trafiają do rozmowy Codex zgodnie z jej ustawieniami;
„lokalny panel” nie oznacza, że analiza przez Codex jest offline.

Awatar oddycha w stanie bezczynności i rozświetla się podczas pracy narzędzi.
Nie korzysta ze zdarzeń mikrofonu ani natywnego głosu. Panel działa jako lokalna
strona w przeglądarce Codex, bez zależności od obsługi osadzonych MCP Apps.

Nowy format `plugin.json` ma także manifest zgodności `.codex-plugin/plugin.json`.
Starszy CLI 0.144.5 używa manifestu zgodności i wygenerowanego `.mcp.json`.
Desktop 0.160.1 używa formatu przenośnego i odrzuca absolutne ścieżki w polu
`mcp.json` → `command`. Z tego powodu konfiguracje transportu są celowo różne.
Obie uruchamiają ten sam serwer w zainstalowanej kopii paczki. To lokalna paczka
developerska, nie publikacja w publicznym katalogu.

Grafika awatara została wygenerowana w tej rozmowie; to oryginalna zakapturzona
postać inspirowana ogólnym kierunkiem wizualnym, nie kopia logotypu POUG.

## Podgląd i testy

```sh
cd plugins/jasmin-companion
npm test
npm run preview
```

Podgląd wypisuje lokalny URL, pokazuje fikcyjne dane i działa do Ctrl+C.
Jest osobnym panelem testowym; rozmowa z zainstalowaną wtyczką otworzy swoją sesję.

Sprawdzone 2026-10-07:

- 4 testy integracyjne: uruchomienie przez przenośny launcher z obcego katalogu
  i rzeczywisty transport stdio; cykl HTTP/SSE i DELETE
  z kontrolowanym serwerem MCP; routing projektu, izolacja sesji, wybór SQL/snapshotu,
  limity, ochrona panelu i zachowanie zer/braków danych.
- Instalacja przez CLI; `plugin/read` widzi skill i MCP; `mcpServerStatus/list`
  w silniku desktop 0.160.1 uruchomionym z katalogu DNV odczytuje wszystkie
  8 narzędzi zainstalowanej wtyczki.
- Test live z katalogu DNV: zainstalowany launcher → MCP stdio wtyczki → lokalny
  JAS-MIN → projekt CEBSOFPR1 → dokładna nazwa `DB Time(s)` → 1339 pomiarów
  i widok dowodu w stanie panelu. Bez zapisu raportu i bez zapytań do Oracle.
- Panel obejrzany w przeglądarce Codex; wykres, historia i wybór punktu działają.

Rozmowa głosowa z mikrofonem użytkownika nie była jeszcze testowana.
Automatyczne testy upstream używają serwera kontrolnego; osobno wykonano opisany
wyżej test z działającym JAS-MIN na rzeczywistej kolekcji DNV.

Usunięcie instalacji (zachowuje kod w repo):

```sh
codex plugin remove jasmin-companion@jasmin-local
```

Źródła formatu i natywnego głosu:
[OpenAI — pakowanie wtyczek](https://developers.openai.com/plugins/build/plugins),
[OpenAI — Voice](https://learn.chatgpt.com/docs/features/voice).
