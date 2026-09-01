# Wayland Overlay und Selection Konzept

## Grundsatz

OpenReader benutzt keine X11-Hooks, kein `XTest`, kein globales Keylogging und kein Auslesen fremder Fensterinhalte. Unter Wayland wäre das nicht nur instabil, sondern bewusst durch das Sicherheitsmodell verhindert.

## Text-Erkennung

MVP:

- KDE Plasma 6.4+: nativer Rust-Adapter ueber `ext-data-control-v1`
- Aelteres KDE: Klipper D-Bus als eingeschraenkter Fallback
- wlroots-Compositoren: derselbe passive Watch-Adapter, wenn ein kompatibles
  Data-Control-Protokoll vorhanden ist
- Debounce: kurze Wartezeit, damit Drag-Markierungen nicht sofort feuern
- Mindestlänge: konfigurierbar

Unter KDE darf kein `wl-paste`-Polling laufen. KWin 6.4+ implementiert den
standardisierten `ext-data-control-v1`-Pfad fuer Clipboard-Manager. OpenReader
bindet ihn in einem nativen Rust-Helper und uebertraegt Text NUL-gerahmt an das
Python-Backend. Der alte rekursive Aufruf und schnelle Fallback-Abrufe bleiben
entfernt, weil sie Fokus, Menues und Drag-Markierungen gestoert hatten.

Nicht-MVP, aber geplant:

- Desktop-Portal-basierte Shortcuts für explizites "Auswahl vorlesen"
- App-spezifische Accessibility APIs, falls DEs stabile Schnittstellen anbieten

## Overlay-Position

Normale Wayland-Clients dürfen die globale Cursorposition nicht frei abfragen. Deshalb gibt es drei Stufen:

1. KDE/KWin Context Bridge
   - KWin Script liest `workspace.cursorPos`
   - meldet `resourceClass/resourceName`
   - positioniert das Tauri Overlay-Fenster über `frameGeometry`, weil Wayland-Clients ihre globale Fensterposition nicht zuverlässig selbst setzen können
   - sendet Daten über D-Bus an `org.openreader.Desktop`

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

Passwortfelder sollen unter Wayland in normalen Toolkits keine PRIMARY Selection liefern. Trotzdem filtert OpenReader zusätzlich secret-artige Tokens und bietet App-Ausschlusslisten.

## Globale Hotkeys

Globale Shortcuts gehören nicht in eigene Keyboard-Hooks. Dafür ist der XDG Desktop Portal GlobalShortcuts-Flow vorgesehen:

- Session erstellen
- Shortcut binden
- Aktivierungs-Signal empfangen
- dann aktuelle Selection lesen und vorlesen

Der MVP enthält die Architektur; die Portal-Implementierung ist ein Roadmap-Punkt.
