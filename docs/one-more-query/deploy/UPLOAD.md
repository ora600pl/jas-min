# ONE MORE QUERY — upload / wgrywanie

## Polski

1. Rozpakuj archiwum. Folder `jas-min` zawiera gotowy kurs PL/EN.
2. Wgraj **folder `jas-min`** do katalogu publicznego domeny `www.ora-600.pl`, obok jej głównego pliku index, nie do katalogu wtyczek WordPressa.
3. Jeżeli katalog `/jas-min/` już istnieje, wgraj do niego zawartość folderu z paczki — nie twórz `/jas-min/jas-min/`.
4. Włącz pokazywanie ukrytych plików w kliencie FTP/SFTP, aby przesłać także `.htaccess`. Plik dotyczy wyłącznie tego podkatalogu. **Nie zastępuj głównego `.htaccess` WordPressa.**
5. Otwórz `https://www.ora-600.pl/jas-min/#pl/0`. Wersja angielska: `https://www.ora-600.pl/jas-min/#en/0`.

Nie trzeba instalować Node.js, Pythona, PHP, biblioteki wykresów ani bazy danych na serwerze. Wszystkie style, skrypty i zatwierdzone anonimowe liczby są osadzone w `index.html`. Interaktywne obliczenia działają w przeglądarce. Aplikacja nie wysyła pomiarów do AI/MCP ani innych serwisów. Samo udostępnienie przez WWW oczywiście wymaga pobrania strony z serwera.

Na serwerach innych niż Apache wystarczy `index.html` jako domyślny dokument katalogu; `.htaccess` nie jest tam używany. Jeżeli hosting Apache odrzuca lokalne dyrektywy i zgłasza HTTP 500, usuń wyłącznie `jas-min/.htaccess` i poproś administratora hostingu o domyślny dokument `index.html`. Nie zmieniaj konfiguracji całego WordPressa.

Kurs można także uruchomić lokalnie dwuklikiem w `jas-min/index.html`. Linki używają fragmentów `#pl/…` i `#en/…`, więc nie wymagają przepisywania adresów ani dodatkowych tras.

## English

Unzip and upload the `jas-min` folder to your domain's public document root. If `/jas-min/` already exists, upload the folder's **contents** there. Include hidden `.htaccess`, but never replace WordPress's root `.htaccess`. Open `/jas-min/#en/0` or `/jas-min/#pl/0`.

No server-side runtime or dependencies are required. `index.html` is self-contained and also runs offline. The app makes no AI/MCP requests or data uploads. On non-Apache servers, configure `index.html` as the directory index; `.htaccess` is not used. If Apache rejects local directives with HTTP 500, remove only `jas-min/.htaccess` and ask the hosting administrator to configure the index document for that directory.
