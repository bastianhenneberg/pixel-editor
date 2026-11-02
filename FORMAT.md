# Das `.pxe.json`-Format

Ein Dokument ist eine JSON-Datei mit der Endung `.pxe.json`. Sie ist so aufgebaut, dass Menschen und Claude sie lesen und von Hand ändern können und dass Git-Diffs klein bleiben: jeder Paletteneintrag, jede Ebene, jeder Frame und jeder Tag steht auf einer Zeile, jede Pixelreihe auf einer eigenen Zeile.

Koordinaten: `(0, 0)` ist oben links, x nach rechts, y nach unten.

Der Editor und der MCP-Server lesen und schreiben außerdem `.aseprite`/`.ase` (siehe unten). Welches Format gilt, entscheidet die Dateiendung.

## Beispiel

```json
{
  "format": "pixel-editor",
  "version": 1,
  "width": 4,
  "height": 3,
  "palette": [
    {"key": "0", "color": "#1a1c2c"},
    {"key": "6", "color": "#38b764", "name": "slime"},
    {"key": "g", "color": "#ffffff80"}
  ],
  "layers": [
    {"name": "Körper", "visible": true, "opacity": 1.0, "locked": true},
    {"name": "Glanz", "visible": true, "opacity": 0.5}
  ],
  "frames": [
    {"name": "f0", "duration_ms": 120},
    {"name": "f1", "duration_ms": 200, "locked": true},
    {"name": "f2", "duration_ms": 120}
  ],
  "tags": [
    {"name": "idle", "from": 0, "to": 2, "direction": "pingpong", "loop": true, "color": "#36b5ff"}
  ],
  "regions": [
    {"name": "face", "layer": "Körper", "locked": true, "keys": [{"frame": 0, "x": 1, "y": 0, "w": 2, "h": 2}, {"frame": 2, "x": 1, "y": 1, "w": 2, "h": 2}]},
    {"name": "spark", "keys": [{"frame": 0, "x": 0, "y": 0, "w": 1, "h": 1}, {"frame": 1, "hidden": true}]},
    {"name": "origin", "kind": "marker", "locked": true, "keys": [{"frame": 0, "x": 2, "y": 2, "w": 1, "h": 1}]}
  ],
  "cels": [
    {"layer": 0, "frame": 0, "rows": [
      ".00.",
      "0660",
      ".00."
    ]},
    {"layer": 0, "frame": 2, "link": 0},
    {"layer": 1, "frame": 0, "rows": [
      "....",
      ".g..",
      "...."
    ]}
  ]
}
```

## Felder

| Feld | Bedeutung |
|---|---|
| `format` | immer `"pixel-editor"` (optional beim Lesen) |
| `version` | Formatversion, aktuell `1`. Neuere Versionen werden abgelehnt. |
| `width`, `height` | Größe in Pixeln (1–1024). Alle Ebenen und Frames haben diese Größe. |
| `palette` | geordnete Liste `{key, color, name?}`. `key` ist genau ein ASCII-Zeichen, `color` ist `#rrggbb` oder `#rrggbbaa` (auch `#rgb`), `name` ein lesbarer Name (z. B. `"crust_dark"`, mindestens 2 Zeichen), über den die Farbe auch angesprochen werden kann. |
| `layers` | Ebenen von **unten nach oben** (Index 0 = unterste). `visible` (Standard `true`), `opacity` 0–1 (Standard `1.0`), `locked` (Standard `false`: gesperrte Ebenen nehmen keine Pixeländerungen an). |
| `frames` | Frames in Reihenfolge, `duration_ms` = Anzeigedauer (Standard 100), `locked` (Standard `false`). Namen müssen eindeutig sein und dürfen kein `/` enthalten; Frames sind außerdem als `<tag>/<n>` adressierbar (n-ter Frame im Bereich des Tags, ab 0, z. B. `walk_down/2`). |
| `tags` | Animationen wie in Aseprite: ein Frame-Bereich `from`..`to` (Indizes, inklusive), `direction` = `forward` (Standard), `reverse`, `pingpong` (0 1 2 1) oder `pingpong_reverse` (2 1 0 1), `loop` (Standard `true`, sonst einmal abspielen), optional `color` für die Zeitleiste. Tags dürfen sich überlappen. |
| `regions` | Benannte Bereiche (siehe unten). |
| `cels` | Pixel je (Ebene, Frame): `layer` und `frame` sind Indizes. Entweder `rows` (genau `height` Zeichenketten mit je `width` Palettentasten) oder `link`: der Index eines anderen Frames, dessen Cel auf dieser Ebene geteilt wird. Fehlende Cels sind komplett transparent. |

## Verknüpfte Cels

Wie in Aseprite können sich Frames einer Ebene dieselbe Cel teilen (`"link": 0`): Eine Änderung in einem verknüpften Frame ändert alle Frames der Gruppe. So wiederholt man einen Frame innerhalb eines Tag-Bereichs, ohne die Pixel doppelt zu pflegen (z. B. `idle`: Frames 0, 1, 2 = Link auf 0, 3). Der Writer verweist immer auf den ersten Frame der Gruppe; dieser steht mit `rows` in der Datei (oder fehlt, wenn er leer ist).

## Regionen

Eine Region ist ein benanntes Rechteck („left_shoe“, „face“), über das Mensch und Claude einen Teil des Sprites eindeutig benennen und gezielt ändern. Modell wie Aseprite-Slices:

- `keys`: nach Frame sortiert; ein Schlüssel gilt ab seinem Frame bis zum nächsten Schlüssel. `{"frame": 2, "x":…, "y":…, "w":…, "h":…}` setzt das Rechteck, `{"frame": 3, "hidden": true}` blendet die Region ab Frame 3 aus. So kann eine Region auf einen Frame, einen Tag-Bereich oder alle Frames beschränkt sein und sich über die Animation bewegen.
- Kurzform beim Schreiben von Hand: `{"name": "face", "x": 1, "y": 0, "w": 2, "h": 2}` = auf allen Frames.
- `layer` (optional, Ebenenname): Region gilt nur für diese Ebene.
- `kind` (optional): `"area"` (Standard, auch für ältere Dateien ohne das Feld) oder `"marker"`. Ein Marker ist ein benannter Punkt (z. B. Ursprung, Handposition), gespeichert als 1×1-Rechteck; der Punkt ist dessen linke obere Ecke.
- `locked`: bei einem Bereich (`area`) sind die Pixel darin und die Definition vor jeder Änderung geschützt (GUI und MCP). Bei einem Marker schützt `locked` nur die Definition (kein Verschieben, Umbenennen, Löschen, kein anderer `kind`); Pixel am Marker bleiben bearbeitbar.
- `color` (optional): Farbe der Umrandung.

Beim Einfügen und Löschen von Frames wandern die Schlüssel mit.

## Palettentasten

- `.` ist immer transparent und steht nicht in der Palette.
- Erlaubt sind druckbare ASCII-Zeichen außer `.` `-` `;` `"` `\` und Leerzeichen. Neue Farben bekommen der Reihe nach `0–9`, `a–z`, `A–Z`, dann Sonderzeichen.
- Die Tasten sind dieselben wie in den `.pxs`-Rastern von pixelart-studio; die eingebauten Paletten (`sweetie16`, `pico8`, `gameboy`, `mono`) haben dieselbe Belegung.
- `-` hat im Dateiformat keine Bedeutung. Nur beim MCP-Werkzeug `set_rows` heißt `-` „Pixel nicht ändern“ (wie in `.pxs`).

## Darstellung

Ebenen werden von unten nach oben mit ihrer Deckkraft übereinandergelegt (normale Alpha-Überblendung); unsichtbare Ebenen zählen nicht, auch nicht in PNG/GIF/Godot/Unity-Exporten. Skaliert wird überall nur ganzzahlig per Nearest Neighbour.

## Regeln beim Laden

Abgelehnt werden Dateien, bei denen Zeilenlänge oder Zeilenzahl nicht zur Größe passen, eine Taste nicht in der Palette steht, eine Taste doppelt definiert ist, ein Link auf einen fehlenden Frame zeigt oder ein Tag-Bereich nicht passt. Die Fehlermeldung nennt Ebene, Frame und Zeile.

Ältere Dateien dieses Editors werden weiter gelesen: `"pingpong": true` wird zu `direction: "pingpong"`, eine Tag-Liste `"frames": [0, 1, 2]` wird zum Bereich 0..2 (nur zusammenhängende Listen; für Wiederholungen verknüpfte Frames nutzen).

Gespeichert wird atomar (temporäre Datei, dann umbenennen), damit ein offener Editor nie eine halb geschriebene Datei liest.

## `.aseprite` / `.ase`

Umgesetzt nach der öffentlichen Formatbeschreibung (`docs/ase-file-specs.md` im Aseprite-Repository), ohne Code aus Aseprite.

**Schreiben** (Standard: indizierte Farben; RGBA wählbar beim Export): Header, Farbprofil sRGB, Palette (Chunk 0x2019 und, wenn keine Farbe Alpha hat, zusätzlich 0x0004), eine Ebene je Layer mit Name, Sichtbarkeit, Deckkraft und Sperre (Aseprite-Flag „editierbar“), Regionen als Slices (Chunk 0x2022, ein Schlüssel je Region-Schlüssel, ausgeblendet = Breite 0; Marker als Slice mit Pivot, Flag 2, der Punkt liegt am Pivot; Ebene, Sperre und `kind` stehen im Text der User-Data der Slice als `pixel-editor:{"layer":…,"locked":…,"kind":…}`, die Farbe in der User-Data-Farbe), je Frame die Dauer, zlib-komprimierte Cels auf ihren Inhalt zugeschnitten, verknüpfte Cels als Link-Cels, Tags mit Bereich, Richtung, Wiederholung (Loop = 0, einmal = 1) und Farbe (User-Data-Chunks). Bei indizierten Dateien ist Index 0 transparent, die Palette folgt ab Index 1. Die Palettentasten stehen als Namen der Paletteneinträge in der Datei (`"9"` oder mit Farbnamen `"9 harness"`); so bleiben Tasten und Farbnamen bei einem Hin- und Rückweg erhalten.

**Lesen**: RGBA, Graustufen und indiziert; Slices werden zu Regionen (9-Patch- und Pivot-Daten werden mit Hinweis ignoriert); normale Ebenen (Gruppen werden aufgelöst, ihre Sichtbarkeit vererbt), Frame-Dauern, rohe und komprimierte Cels, Link-Cels, Tags (Richtung, Loop, Farbe), Palette (0x2019, sonst 0x0004/0x0011), Hintergrund-Ebenen (Transparenz-Index gilt dort nicht). Farben werden exakt übernommen; tragen die Paletteneinträge keine gültigen Tasten als Namen, werden Tasten in Palettenreihenfolge vergeben (bei mehr als 89 Farben nur die benutzten).

Nicht abbildbar, beim Lesen als Hinweis gemeldet: Blend-Modi (als normal dargestellt), Cel-Deckkraft und Z-Index (ignoriert), Tilemap-Ebenen (übersprungen), Tag-Wiederholung > 1 (wird zu „einmal“). Frame-Namen und Frame-Sperren gibt es in Aseprite nicht; Frames heißen beim Lesen `f0`, `f1`, … und sind entsperrt. Pixel, Ebenen (samt Sperre), Dauern, verknüpfte Cels, Tags, Palette (samt Tasten und Namen) und Regionen bleiben bei `.pxe.json → .aseprite → .pxe.json` erhalten (getestet, Pixel identisch).

## Verlauf (`.pxe-history/`)

Neben jedem Dokument legen MCP-Server und Editor `.pxe-history/<dateiname>/` an: `journal.json` (Einträge mit Zeit, Autor: Name des MCP-Clients wie `Claude`/`Codex` (ältere Journale `claude`), `user` für GUI-Speicherungen, Beschreibung, Art `edit`/`progress`/`focus` und Ort der Änderung `focus: {frame, layer, bbox, region}`, damit der Editor folgen kann; `gui.pid` vom laufenden Editor, `gui.log` mit seiner Fehlerausgabe, wenn er über `open`/`open_editor` gestartet wurde), `views/<pid>.json` (was jeder laufende Editor gerade zeigt, für `get_user_view`), `marks.json` (die „KI zeigen“-Markierungen des Dokuments: `{"next": 3, "marks": [{"n": 1, "frame": {index, name, address}, "layer": {index, name}, "rect": {x, y, w, h} oder null für den ganzen Frame, "note": "…", "time_ms", "pid", "dirty"}]}`; ohne Markierungen fehlt die Datei; eine ältere `pointer.json` mit einem einzelnen Zeiger wird beim Lesen als #1 übernommen und dann ersetzt) und `states/` (Zustand nach jedem Eintrag, höchstens 300) sowie `snapshots/` mit benannten Schnappschüssen. Der Ordner kann gelöscht werden; dann beginnt der Verlauf neu. Für Git: in `.gitignore` aufnehmen.
