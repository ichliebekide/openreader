# UI (Rust/Leptos)

Die Oberflaeche ist eine Leptos-App (0.8, client-side rendering) im
Repository-Wurzelverzeichnis (`Cargo.toml`, `src/`, `styles/app.css`,
`index.html`, `Trunk.toml`). Trunk baut sie nach WebAssembly (`dist/`); Tauri
laedt dasselbe Bundle in beide Fenster:

| Fenster | URL | Inhalt |
| --- | --- | --- |
| `main` | `index.html` | Lesefenster "Fokus" |
| `overlay` | `index.html#/overlay` | transparenter Vorlese-Knopf |

React, Vite, TypeScript, npm und ts-rs sind entfernt.

## Module

| Datei | Zweck |
| --- | --- |
| `src/main.rs` | waehlt anhand von `#/overlay` Hauptfenster oder Overlay |
| `src/api.rs` | HTTP + WebSocket (`gloo-net`) gegen `127.0.0.1:8765`, Reconnect nach 1,2 s |
| `src/tauri.rs` | `window.__TAURI__` (`app.withGlobalTauri`): `invoke`, `listen`, `startDragging`; ausserhalb von Tauri No-ops |
| `src/state.rs` | reaktiver Zustand, Polling (1,6 s), Wiedergabe-Befehle, entprelltes Speichern der Einstellungen |
| `src/logic.rs` | reine Logik ohne DOM, nativ getestet (`cargo test`) |
| `src/reader.rs` | Kopfzeile, Lesetext mit Wortmarkierung, Meldungszeile, Player-Leiste |
| `src/settings_drawer.rs` | Einstellungen als Seitenleiste |
| `src/overlay.rs` | Overlay-Knopf |
| `src/icons.rs` | Tabler-Icons (MIT) als Inline-SVG |

## API-Typen

Alle Modelle an der API-Grenze liegen in der wasm-faehigen Crate
`openreader-api` (`src-tauri/api-types/`). Backend (`models.rs` re-exportiert
sie) und UI nutzen dieselben serde-Typen; JSON-Feldnamen und das Format von
`settings.json` sind unveraendert. Es gibt keine generierten Dateien und keinen
Drift-Check mehr: eine Aenderung am Typ bricht den Build beider Seiten.

## API-Adresse

Standard ist `http://127.0.0.1:8765`. Abweichend:

- beim Bauen: `OPENREADER_API=http://127.0.0.1:18765 trunk build`
- zur Laufzeit (nur Browser/Tests): `?api=http://127.0.0.1:18765`; erlaubt sind
  nur Loopback-Adressen.

## Design "Fokus"

- Eine Spalte; der Text steht im Mittelpunkt (Serif 19,5 px, Zeilenhoehe 1,75,
  max. 65 Zeichen, 56 px Seitenabstand). Das gesprochene Wort hat einen
  bernsteinfarbenen Textmarker mit 2 px Unterstrich.
- Kopfzeile (Ziehbereich, `data-tauri-drag-region` + `startDragging`): Status-
  punkt (Wayland/Fallback/getrennt), "N Zeichen · Stimme", rechts Datei oeffnen
  und Einstellungen.
- Player-Leiste: runder Play/Pause-Knopf, 3-px-Fortschrittslinie (Wortende /
  Textlaenge), Tempo-Anzeige (Klick schaltet 0,8x ... 1,4x; waehrend des
  Vorlesens gesperrt, weil Speichern die Wiedergabe beendet), Stopp.
- Fehler (rot) und Fallback-Hinweis (gedaempft) als eine ruhige Zeile ueber der
  Player-Leiste.
- Einstellungen in einer Seitenleiste (X, Escape, Klick daneben): Stimme
  (Engine, Stimmprofil, Engine-Status, TTS testen), Wiedergabe (Tempo,
  Lautstaerke), Auswahl (minimale Textlaenge, Terminals ignorieren), Dokumente
  (OCR: Tesseract oder ocrs), Darstellung (Hell/Dunkel/System), Export (WAV).
- Farben als CSS-Variablen; folgt `prefers-color-scheme`, Auswahl
  Hell/Dunkel wird pro WebView in `localStorage` (`openreader.theme`) gemerkt.
  Einziger Akzent: Bernstein `#BA7517` (dunkel `#E3A44F`). Keine Webfonts:
  Noto Sans/Noto Serif aus dem System.
- Tempo wird als Geschwindigkeit angezeigt (`1 / length_scale`); gespeichert
  wird weiterhin `length_scale`.

## Overlay

Fenstergroesse (56 x 56), Fokusregel, Eingaberegion (56-px-Kreis in
`overlay_input.rs`) und KWin-Positionierung sind unveraendert. Der Knopf fuellt
den 56-px-Kreis, sichtbar ist eine 34-px-Flaeche in Textfarbe mit invertiertem
Play-Symbol (Spinner waehrend der Anfrage) und leichtem Schatten. Ablauf wie
bisher: `overlay-visibility` -> zwei Animation-Frames -> `set_overlay_input`,
Ausblenden nach 4,8 s (neue Auswahl startet den Timer neu), Klick ->
`/api/speak` -> `hide_overlay`.

## Bauen und Starten

```bash
./scripts/ensure-ui-toolchain.sh        # wasm32-Target, trunk, tauri-cli pruefen
OPENREADER_INSTALL_TOOLS=1 ./scripts/ensure-ui-toolchain.sh   # ... und installieren
./scripts/dev.sh                        # cargo tauri dev (trunk serve auf :1420)
trunk build --release                   # nur die UI nach dist/
cargo test                              # UI-Logik (im Wurzelverzeichnis)
cargo tauri build --debug --no-bundle   # App mit eingebettetem dist/
```

Trunk laedt beim ersten Lauf `wasm-bindgen` und `wasm-opt` nach
`~/.cache/trunk`.

## UI-Check (Ersatz fuer `check-reader-ui.mjs`)

`src-tauri/ui-check` startet das echte Backend-Routing mit einer stillen
Test-Stimme (keine Audioausgabe, kein Selection-Monitor, kein D-Bus, eigener
Port, temporaere `settings.json`) und steuert Chrome/Chromium headless:

```bash
trunk build --release
cd src-tauri
cargo run -p openreader-ui-check                                   # Check
cargo run -p openreader-ui-check -- --screenshots ../docs/screenshots
cargo run -p openreader-ui-check -- --serve 18765                  # Vorschau im Browser
```

Geprueft werden: Einstellungen und Engine-Status, Auswahl-Event mit
Zeichenzahl, Datei-Import, zeitgesteuerte Wortmarkierung, eingefrorene
Markierung bei Pause, Wiederherstellung nach Reload (Reading-Snapshot), Stopp,
Importfehler (UTF-8), Fallback-Hinweis, Seitenleiste (Escape, Klick daneben),
OCR-Auswahl, Hell/Dunkel/Override, Tempo-Anzeige, Layouts 1240x800, 1040x660,
390x844 und der Overlay-Knopf (transparente Seite, 56/34 px). Die Overlay-
Positionstests liegen seit Phase 2 in `src-tauri/tests/kwin_overlay_position.rs`,
der Live-Fokustest in `src-tauri/examples/check_overlay_focus.rs`.

Nicht im Browser pruefbar: echte Tauri-IPC (`show_overlay`, Eingaberegion,
Fensterziehen). Das wurde einmalig in einer isolierten virtuellen KWin-Sitzung
mit der echten App geprueft (`docs/screenshots/tauri-webkitgtk-main-overlay.png`).

## Screenshots

`docs/screenshots/`: `fokus-light-*`, `fokus-dark-*`, `overlay-light/dark.png`
(vom UI-Check erzeugt) und `tauri-webkitgtk-main-overlay.png` (echte App in
WebKitGTK).
