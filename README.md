# Akku-Monitor

Akku-Stand von Funk-Headsets und -Mäusen direkt in der Windows-Taskleiste – `🎧 91 %  🎧 55 %  🖱 96 %` neben der Uhr.
Ein Rust-Port von [BatteryHub](https://github.com/krisgfx9/Wireless-Device-Battery-Monitor) (krisgfx9), reines Win32, ~750 KB, kein Hintergrunddienst, keine Hersteller-Software nötig.

## Getestete Geräte

Mit echter Hardware getestet:

| Gerät | Anschluss | USB-ID |
|---|---|---|
| **ASTRO A50 Gen 5** | USB-Basisstation | `046D:0B1C` |
| **ASTRO A50 Gen 4** | USB-Basisstation | `9886:002C` |
| **AJAZZ AJ179P** (Maus) | 2,4-GHz-Empfänger | `249A:5C2F` |

Gelesen werden Akku in Prozent und – bei den Headsets – der Ladezustand (Gen 4 zusätzlich Dock-Status und „Headset aus“).

## Funktionen

- **Taskleisten-Widget** neben der Uhr, auf allen Monitoren; weicht dem Windows-Widgets-Knopf aus, folgt hell/dunkel und DPI
- **Tray-Modus** als Alternative: ein Symbol pro Gerät mit der Zahl darin
- Zwei Geräte gleicher Art bekommen eine kleine Beschriftung (z. B. „4“ / „5“ bei den beiden A50)
- Menü: Geräte ein-/ausblenden, Reihenfolge ändern, **umbenennen**, **Warnschwelle pro Gerät** (5–40 %), Markenfarben, Abfrage-Intervall (30 s – 60 min), Autostart
- **Akku-Verlauf** (24 h / 7 / 30 Tage) mit Diagramm und vorsichtiger Restlaufzeit-Schätzung, nur lokal gespeichert
- **19 Sprachen**, folgt der Windows-Sprache
- Benachrichtigung bei schwachem Akku

## Installation

`Akku-Monitor-Setup-x.y.z.exe` aus den [Releases](../../releases) herunterladen und ausführen (keine Admin-Rechte nötig).
Das Programm ist nicht signiert, SmartScreen kann deshalb warnen („Weitere Informationen“ → „Trotzdem ausführen“).

## Bauen

```powershell
cargo build --release
```

Installer: Inno Setup 6, `installer\akku-monitor.iss` kompilieren → `dist\`.

`akku-monitor.exe --list` schreibt die gefundenen Geräte nach `%TEMP%\akku-monitor-list.txt`.

## Lizenz und Herkunft

GPL-3.0, wie das Original. Oberfläche, Widget, Menüs, Verlauf und Übersetzungen sind aus
[BatteryHub](https://github.com/krisgfx9/Wireless-Device-Battery-Monitor) nach Rust übertragen.
Geräteprotokolle: [HeadsetControl](https://github.com/Sapd/HeadsetControl) (A50 Gen 5),
[eh-fifty](https://github.com/tdryer/eh-fifty) (A50 Gen 4), [Ajazzy](https://github.com/Geovane2dd/Ajazzy) (AJAZZ).

---

# Akku-Monitor (English)

Battery levels of wireless headsets and mice right on the Windows taskbar – `🎧 91%  🎧 55%  🖱 96%` next to the clock.
A Rust port of [BatteryHub](https://github.com/krisgfx9/Wireless-Device-Battery-Monitor) by krisgfx9: plain Win32, ~750 KB, no background service, no vendor software required.

## Tested devices

Tested on real hardware:

| Device | Connection | USB ID |
|---|---|---|
| **ASTRO A50 Gen 5** | USB base station | `046D:0B1C` |
| **ASTRO A50 Gen 4** | USB base station | `9886:002C` |
| **AJAZZ AJ179P** (mouse) | 2.4 GHz receiver | `249A:5C2F` |

## Features

- **Taskbar widget** next to the clock on every monitor; steps past the Windows Widgets button, follows light/dark mode and DPI
- **Tray icon mode** as an alternative: one icon per device with the number drawn in it
- Two devices of one kind get a tiny caption (e.g. "4" / "5" for the two A50s)
- Menu: show/hide and reorder devices, **rename**, **per-device low-battery threshold** (5–40%), brand colors, poll interval (30 s – 60 min), start with Windows
- **Battery history** (24 h / 7 / 30 days) with chart and a conservative time-to-empty estimate, stored locally only
- **19 languages**, follows the Windows display language
- Low-battery notification

## Install

Download `Akku-Monitor-Setup-x.y.z.exe` from [Releases](../../releases) and run it (no admin rights needed).
The app is unsigned, so SmartScreen may warn ("More info" → "Run anyway").

## License

GPL-3.0, like the original. UI, widget, menus, history and translations are ported from
[BatteryHub](https://github.com/krisgfx9/Wireless-Device-Battery-Monitor). Device protocols:
[HeadsetControl](https://github.com/Sapd/HeadsetControl), [eh-fifty](https://github.com/tdryer/eh-fifty), [Ajazzy](https://github.com/Geovane2dd/Ajazzy).
