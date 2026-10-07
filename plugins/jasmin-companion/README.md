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
`jasmin-companion@jasmin-local`. Zapisuje lokalne ścieżki Node i serwera w
ignorowanych przez Git plikach `mcp.json` i `.mcp.json`. Dzięki temu działa również,
gdy desktop ma inny PATH niż terminal. Ścieżki wskazują zainstalowaną kopię w cache,
nie pliki serwera w bieżącym branchu. Po zmianie kodu uruchom instalator ponownie.
Opcjonalnie `CODEX_CLI` wskazuje konkretny plik wykonywalny CLI.

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
Na zainstalowanym CLI 0.144.5 dopiero manifest zgodności wykrywa metadane/MCP.
Ta wersja nie rozwija `${CLAUDE_PLUGIN_ROOT}` w argumentach MCP, więc instalator
generuje dokładne ścieżki. Pliki źródłowe bez uruchomienia instalatora nie są jeszcze
gotowym archiwum instalacyjnym. To lokalna paczka developerska, nie publikacja
w publicznym katalogu.

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

- 4 testy integracyjne: rzeczywisty transport stdio; cykl HTTP/SSE i DELETE
  z kontrolowanym serwerem MCP; routing projektu, izolacja sesji, wybór SQL/snapshotu,
  limity, ochrona panelu i zachowanie zer/braków danych.
- Instalacja przez CLI; `plugin/read` widzi skill i MCP; `mcpServerStatus/list`
  uruchamia zainstalowany serwer i odczytuje wszystkie 8 narzędzi.
- Panel obejrzany w przeglądarce Codex; wykres, historia i wybór punktu działają.

Rozmowa głosowa z mikrofonem użytkownika oraz połączenie tej wersji wtyczki
z rzeczywistym zestawem danych JAS-MIN nie były jeszcze testowane. Test integracji
upstream używa serwera kontrolnego, nie żywej bazy Oracle.

Usunięcie instalacji (zachowuje kod w repo):

```sh
codex plugin remove jasmin-companion@jasmin-local
```

Źródła formatu i natywnego głosu:
[OpenAI — pakowanie wtyczek](https://developers.openai.com/plugins/build/plugins),
[OpenAI — Voice](https://learn.chatgpt.com/docs/features/voice).
