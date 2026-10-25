# Wayland Overlay und Selection Konzept

## Grundsatz

OpenReader benutzt keine X11-Hooks, kein `XTest`, kein globales Keylogging und kein Auslesen fremder Fensterinhalte. Unter Wayland wäre das nicht nur instabil, sondern bewusst durch das Sicherheitsmodell verhindert.

## Text-Erkennung

OpenReader wählt die Quelle selbst, ohne Einstellung und ohne Einrichtung
(`clipboard_monitor.rs`). Die aktive Quelle steht in `GET /api/health`
(`selection_input`) und im Log; jeder Wechsel und bei jeder Auswahl die Quelle
(nie der Text) werden protokolliert.

1. **PRIMARY** (Standard, beste Abdeckung): KDE Plasma 6.4+ und
   wlroots-Compositoren über `ext-data-control-v1` bzw. `wlr-data-control`,
   per wl-clipboard-rs im Helper-Prozess `<exe> --selection-helper`, Text
   NUL-gerahmt auf stdout.
2. **Ohne PRIMARY** (KDE: „Mittelklick fügt markierten Text ein“ ist aus,
   `kwinrc [Wayland] EnablePrimarySelection=false`): Barrierefreiheit (AT-SPI2)
   für markierten Text, dazu die normale Zwischenablage (Strg+C) als letzte
   Rückfallebene.
3. Älteres KDE ohne Data-Control: Klipper über D-Bus.

Es laufen kein `wl-paste` und kein Polling. Debounce (mindestens 550 ms) und
Mindestlänge gelten für alle Quellen, ebenso der `SelectionGuard`
(Terminal-Ausschluss, Secret-Filter).

### Erkennung „kein PRIMARY“

KWin mit abgeschaltetem Mittelklick-Einfügen entfernt das globale Objekt
`zwp_primary_selection_device_manager_v1`, über das Programme PRIMARY setzen.
`ext-data-control-v1` nimmt einen PRIMARY-Beobachter trotzdem an, er sieht dann
aber nie eine Auswahl. Der Helper prüft deshalb zuerst, ob ein
Primary-Selection-Global angeboten wird, und beendet sich sonst mit Exit-Code
3 („PRIMARY nicht unterstützt“) statt in einer Neustartschleife zu landen.
Der Monitor startet dann AT-SPI und `--selection-helper --clipboard`. Dieser
Zwischenablage-Helper prüft jede Minute, ob PRIMARY wieder da ist, und endet in
dem Fall mit Exit-Code 4; der Monitor wechselt zurück zu PRIMARY und schaltet
die Barrierefreiheit wieder ab, falls er sie eingeschaltet hatte. Wird die
KDE-Option wieder aktiviert (KWin übernimmt sie in der Regel nach einer neuen
Anmeldung), funktioniert also alles ohne Zutun.

### AT-SPI (`selection_atspi.rs`)

- Adresse des Barrierefreiheits-Busses über `org.a11y.Bus.GetAddress`, eigene
  zbus-Verbindung dorthin.
- Genau eine Match-Regel (`org.a11y.atspi.Event.Object.TextSelectionChanged`)
  und eine Registrierung `RegisterEvent("object:text-selection-changed")` an
  der Registry, damit Toolkits nur dieses Ereignis senden. Keine
  Tastaturereignisse, keine Fokus- oder Caret-Ereignisse, keine Baumdurchläufe.
- Ereignisse werden pro Ruhephase (Debounce) zusammengefasst; eine
  Drag-Markierung erzeugt eine Abfrage. Danach wird nur die Ereignisquelle
  befragt: Rolle und Zustand (Passwortfeld `ROLE_PASSWORD_TEXT`, abgebaute
  Objekte), Auswahlbereiche (`GetNSelections`/`GetSelection`), der Zustand der
  obersten Fenster der Anwendung (`STATE_ACTIVE`, also nur das aktive Fenster)
  und erst dann `GetText(start, ende)` der Auswahl. Passwortfelder werden nie
  gelesen, Auswahlen im OpenReader-Prozess selbst werden ignoriert.
- Jeder D-Bus-Aufruf hat 800 ms Timeout, eine Abfrage insgesamt höchstens 2 s,
  und sie läuft außerhalb der Ereignisschleife. Ein neues Ereignis bricht eine
  laufende Abfrage ab; eine hängende Anwendung blockiert also weder spätere
  Auswahlen noch andere Anwendungen (wie beim früheren Fix für hängende
  PRIMARY-Quellen).
- Verschwindet eine Anwendung, schlägt nur ihre Abfrage fehl. Endet die
  Busverbindung (Neustart von at-spi), verbindet sich OpenReader mit Backoff
  neu.
- Dieselbe Auswahl desselben Objekts wird nicht zweimal angeboten (etwa wenn
  ein Toolkit sie beim Fokuswechsel erneut meldet), erst nachdem sie
  aufgehoben wurde. Markiert man und drückt dann Strg+C, erscheint die Blase
  nur einmal: derselbe Text aus einer anderen Quelle wird 10 s lang
  verworfen.
- `OPENREADER_ATSPI=0` schaltet AT-SPI ab (dann nur Zwischenablage).

#### `org.a11y.Status.IsEnabled`

Qt- und KDE-Programme bauen ihren Barrierefreiheitsbaum nur auf, solange
`org.a11y.Status.IsEnabled` wahr ist (sie reagieren darauf auch im laufenden
Betrieb). Ist der Schalter aus, setzt OpenReader ihn auf `true`, sobald es
AT-SPI braucht. `ScreenReaderEnabled` wird nie angefasst.

Wichtig: at-spi2-core bis einschließlich 2.60 speichert diesen Schalter im
GSettings-Schlüssel `org.gnome.desktop.interface toolkit-accessibility`
(dconf). Er ist damit nicht sitzungsgebunden, sondern würde eine Abmeldung
überdauern. OpenReader legt deshalb beim Einschalten die Markierungsdatei
`~/.cache/openreader/a11y-enabled-by-openreader` an und schaltet den Schalter
wieder aus, wenn es AT-SPI nicht mehr braucht (PRIMARY ist zurück) oder
beendet wird. Wird OpenReader hart beendet (z. B. beim Abmelden), bleibt der
Schalter an; die Markierungsdatei sagt dem nächsten Start, dass OpenReader ihn
gesetzt hat, und er wird beim nächsten Wechsel bzw. Beenden zurückgesetzt.
Läuft ein Screenreader, bleibt er an. Neuere at-spi2-core-Versionen schalten
`IsEnabled` beim Registrieren eines Ereignis-Listeners selbst sitzungsweise ein;
dann muss OpenReader nichts setzen.

Kosten: Mit `IsEnabled` bauen Toolkits ihre Barrierefreiheitsobjekte (meist bei
Bedarf) auf und senden registrierte Ereignisse. OpenReader selbst registriert
nur das eine Auswahl-Ereignis und fragt nur bei Auswahlen nach.

#### Abdeckung

Live geprüft mit `cargo run -p openreader-backend --example
check_atspi_selection` (eigene Testfenster, Auswahl per AT-SPI gesetzt):

| Toolkit | Auswahl kommt an | Passwortfeld ignoriert | braucht `IsEnabled` |
| --- | --- | --- | --- |
| GTK 3 (GtkTextView, GtkEntry) | ja | ja | nein |
| GTK 4 (GtkTextView, GtkPasswordEntry) | ja | ja | nein |
| Qt 6 / KDE (QLineEdit über kdialog) | ja, auch wenn das Programm vor dem Einschalten lief | – | ja |

Weitere Programme:

- Firefox und Chromium lesen `IsEnabled` nur beim Start. Laufen sie schon,
  bevor OpenReader den Schalter setzt, liefern sie erst nach einem Neustart
  Barrierefreiheitsdaten; Firefox meldet dann Textauswahlen. Eine Auswahl, die
  über mehrere Absätze geht, liefert per AT-SPI nur den Teil im meldenden
  Objekt.
- Chromium und Electron (VS Code, Discord, Element, Chrome) aktivieren den
  Barrierefreiheitsbaum ihrer Webinhalte erst, wenn ein Hilfsmittel wie Orca
  läuft oder den Baum aktiv abfragt. OpenReader tut das bewusst nicht: es würde
  diese Programme spürbar verlangsamen, und VS Code würde in den
  Screenreader-Modus wechseln. Der Monaco-Editor von VS Code meldet Auswahlen
  ohnehin nur in diesem Modus. Für diese Programme bleibt Strg+C.
- Terminals sind wie bisher ausgeschlossen.

### Zwischenablage als Rückfallebene

`--selection-helper --clipboard` beobachtet die normale Zwischenablage über
`ext-data-control-v1`. Data-Control meldet jede Änderung, auch solche von
Programmen; deshalb gelten dieselben Filter wie für Auswahlen, und zusätzlich:

- Einträge mit dem MIME-Typ `x-kde-passwordManagerHint` (KeePassXC, KWallet
  usw.) werden nicht gelesen.
- Der Inhalt beim Start wird nur gemerkt, nicht angeboten; ein erneutes Angebot
  mit identischem Inhalt (Klipper übernimmt die Zwischenablage eines
  geschlossenen Programms) gilt nicht als neue Kopie. Dieselbe Kopie zweimal
  hintereinander bringt daher keine zweite Blase.
- OpenReader verändert die Zwischenablage nie.

### Geplant

- Desktop-Portal-basierte Shortcuts für explizites "Auswahl vorlesen"

### Markierung nach dem Erfassen leeren

Optional (Einstellungen -> Auswahl, `clear_primary_after_capture`, Standard aus):
Nachdem eine PRIMARY-Auswahl angenommen und als Blase angeboten wurde, leert
OpenReader die PRIMARY-Auswahl ueber data-control (`wl_clipboard_rs::copy::clear`).
Ein versehentlicher Mittelklick fuegt dann nichts ein, ohne dass man
Mittelklick-Einfuegen in KDE abschalten muss (was PRIMARY ganz entfernt). Abgelehnte
Auswahlen (z. B. in Terminals) bleiben unangetastet. Manche Programme, etwa
Konsole, heben ihre sichtbare Markierung auf, sobald sie die Auswahl verlieren.

Ein neuer Vorlese-Auftrag waehrend einer laufenden Wiedergabe ersetzt diese
(wie bei MWS Reader); derselbe Text erneut laesst sie weiterlaufen.

## Overlay-Position

Normale Wayland-Clients dürfen die globale Cursorposition nicht frei abfragen. Deshalb gibt es drei Stufen:

1. KDE/KWin Context Bridge
   - KWin Script liest `workspace.cursorPos`
   - meldet `resourceClass/resourceName`
   - positioniert das Tauri Overlay-Fenster über `frameGeometry`, weil Wayland-Clients ihre globale Fensterposition nicht zuverlässig selbst setzen können
   - sendet Daten über D-Bus an `org.openreader.Desktop`

   Die Position gilt unabhängig von der Quelle: auch Auswahlen über AT-SPI
   oder die Zwischenablage bekommen die zuletzt gemeldete Zeigerposition.

2. Fallback ohne KWin Script
   - Overlay erscheint an einer festen, unaufdringlichen Position
   - Selection funktioniert weiterhin

3. Zukünftiger Layer-Shell Helper
   - separater nativer Overlay-Prozess
   - nutzt `wlr-layer-shell`/KDE-kompatible Layer-Shell-Unterstützung, wo verfügbar

## Fokusverhalten

Das Overlay ist:

- klein
- transparent
- rahmenlos
- nicht in der Taskleiste
- nicht im Window-Switcher
- nur kurz sichtbar

Tauri setzt `decorations=false`, `transparent=true`, `skipTaskbar=true` und
`alwaysOnTop=true`. Das KWin-Skript ergänzt `skipSwitcher`, ordnet bei mehreren
Monitoren den passenden Output zu und setzt die logische `frameGeometry`.
Rust blendet das native Overlay-Fenster nach 4,8 Sekunden vollständig aus.
Ein Layer-Shell Helper bleibt eine mögliche spätere Alternative.

## Sicherheitsfilter

OpenReader zeigt kein Overlay, wenn:

- der Text zu kurz ist
- aktive Fensterklasse als Terminal bekannt ist
- der Text wie ein einzelnes Token/Secret aussieht

Passwortfelder sollen unter Wayland in normalen Toolkits keine PRIMARY Selection liefern. Über AT-SPI werden Objekte mit der Rolle `ROLE_PASSWORD_TEXT` nie gelesen, in der Zwischenablage Einträge mit `x-kde-passwordManagerHint` übersprungen. Zusätzlich filtert OpenReader secret-artige Tokens und bietet App-Ausschlusslisten.

## Globale Hotkeys

Globale Shortcuts gehören nicht in eigene Keyboard-Hooks. Dafür ist der XDG Desktop Portal GlobalShortcuts-Flow vorgesehen:

- Session erstellen
- Shortcut binden
- Aktivierungs-Signal empfangen
- dann aktuelle Selection lesen und vorlesen

Der MVP enthält die Architektur; die Portal-Implementierung ist ein Roadmap-Punkt.
