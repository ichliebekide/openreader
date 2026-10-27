# Changelog

Alle nennenswerten Änderungen an OpenReader. Die Versionen folgen
[Semantic Versioning](https://semver.org/lang/de/). Der Abschnitt einer
Version wird beim Release automatisch als Release-Text übernommen; enthält er
das Wort „Sicherheit“, zeigt OpenReader das Update als Sicherheitsupdate an.

## [0.1.1] – 2026-10-27

Macht die installierten Pakete vollständig nutzbar.

### Behoben
- Das KWin-Skript fehlte in den Paketen: Ohne es erschien die Vorlese-Blase
  nicht neben dem Mauszeiger und „Terminals ignorieren“ griff nicht.
  OpenReader richtet das Skript jetzt beim Start selbst ein und hält es nach
  Updates aktuell.
- Die Piper-Stimmen fehlten in den Paketen, damit gab es keine Ausweichstimme,
  wenn Seraphina nicht erreichbar ist. Beim ersten Start wird jetzt Thorsten
  (etwa 60 MB) im Hintergrund geladen; alle Piper-Stimmen lassen sich in den
  Einstellungen unter „Stimme“ einzeln laden, mit Fortschrittsanzeige.

### Dokumentation
- README mit Installationsanleitung; Voraussetzung ist KDE Plasma 6
  (Ubuntu/Kubuntu ab 25.04).

## [0.1.0] – 2026-10-27

Erste veröffentlichte Version.

### Vorlesen
- Markierten Text aus jeder Anwendung vorlesen: Text markieren, auf die runde
  Vorlese-Blase klicken. Die Blase erscheint nur bei echter Textauswahl, nie
  beim bloßen Drücken der Maustaste.
- Stimmen: Microsoft Seraphina (online, mit Wiederholung bei Aussetzern) und
  Piper (offline, sechs deutsche Stimmen). Fällt Seraphina aus, springt Piper
  ein und die App zeigt das an.
- Das gerade gesprochene Wort wird markiert, die Fortschrittsleiste läuft mit;
  bei Piper werden die Zeiten pro Satz geschätzt.
- Ein neuer Text ersetzt die laufende Wiedergabe, ohne vorher zu stoppen.
- PDF, EPUB, HTML und Textdateien öffnen, Bilder per Texterkennung (OCR).
- Export als WAV-Datei.

### Desktop
- Für KDE Plasma unter Wayland gebaut; erkennt die Auswahl über die
  primäre Auswahl, ohne Mittelklick-Einfügen über die Barrierefreiheit (AT-SPI)
  und notfalls über Strg+C.
- Oberfläche „Fokus“ in Rust (Leptos), hell, dunkel oder nach System,
  anpassbare Akzent- und Blasenfarbe, einstellbare Blasengröße.
- Rahmenloses Fenster mit eigenen Knöpfen (Minimieren, Maximieren,
  Schließen in die Leiste), Doppelklick auf die Kopfzeile, Größe an den
  Rändern ziehbar; Symbol in der Systemleiste.
- Auswahlen in Terminals werden ignoriert (abschaltbar), u. a. Konsole,
  Ptyxis, GNOME Console, Kitty, Alacritty, Ghostty, WezTerm, Foot, Tilix.
- Optional: Markierung nach dem Erfassen leeren, gegen versehentliches
  Einfügen mit der mittleren Maustaste.
- Hinweis auf neue Versionen (einmal täglich, abschaltbar).

### Sicherheit
- Die lokale Schnittstelle nimmt nur Anfragen von der eigenen Oberfläche an
  (Schutz gegen fremde Webseiten und DNS-Rebinding); der Export schreibt nur
  WAV-Dateien.
