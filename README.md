# pixel-editor

Nativer Pixelart- und Animationseditor in Rust. Man kann ihn von Hand bedienen (Bedienkonzept wie Aseprite), und Claude kann ihn über MCP steuern. Beide arbeiten auf denselben Dateien (`.pxe.json` oder `.aseprite`, Karten als `.tilemap.json`). Ist eine Datei im Editor offen, sieht man Claudes Änderungen live, weil der Editor sie bei Änderungen auf der Platte neu lädt.

- `core/`: Dokumentmodell (Ebenen × Frames, verknüpfte Cels, Tags), Laden/Speichern (`.pxe.json`, `.aseprite`), Zeichenfunktionen, Rendering, Undo, Exporter. Keine GUI-Abhängigkeiten.
- `app/`: das Programm `pixel-editor` (GUI mit egui/eframe) und der MCP-Server (`pixel-editor mcp <ordner>`).
- Dateiformat: [FORMAT.md](FORMAT.md)
- Beispiel: `examples/schleim.pxe.json` (16×16, 4 Frames, Frame 2 mit Frame 0 verknüpft, Tag `idle` 0..3), komplett per MCP erstellt.

Verhältnis zu `pixelart-studio` (Spiel-Repo mit dem Generator `pxa`): Der Editor ist die Zentrale zum Zeichnen, Ansehen, Animieren und Exportieren. `pxa` bleibt Claudes prozeduraler Generator. Palettentasten und Sheet-JSON sind kompatibel; ein PNG(-Sheet) aus `pxa` lässt sich mit `import_png` übernehmen.

## Pixelgenauigkeit

Nirgends wird geglättet:
- Zoom nur in ganzen Stufen, gezählt in physischen Bildschirmpixeln. Die Leinwand liegt auf dem Pixelraster des Bildschirms, auch bei gebrochener UI-Skalierung (getestet bei 1,0 und 1,5).
- Texturen werden mit Nearest Neighbour gezeichnet. Formvorschauen (Linie/Rechteck/Ellipse beim Ziehen) werden in die Leinwand-Textur gerechnet. Raster und Rahmen sind Meshes ohne Kantenglättung.
- Exporte und Vorschaubilder skalieren nur ganzzahlig, und Farben bleiben exakt (GIF mit eigener Palette, `.aseprite` indiziert). Die Vorschau-Beschriftung nutzt eine eingebaute Pixelschrift.

## Konsistenz: benennen, gezielt ändern, sperren, nachvollziehen

Mensch und Claude sollen jedes Element genau benennen können, und Claude ändert nur das Genannte. Nichts wird „neu generiert“.

- **Namen überall**: Ebenen, Frames (eindeutiger Name, zusätzlich als `<tag>/<n>` adressierbar, z. B. `walk_down/2`), Tags, Paletteneinträge (Taste + Name, z. B. `9 harness`) und Regionen. Die GUI zeigt sie in der Zeitleiste (Frame-Nummer + Name, Tag-Balken, Ebenennamen), in den Paletten-Tooltips, in den Überschriften der Eigenschaften und in der **Statuszeile**: `Frame walk_down/1 (f2) · Ebene body · x 4, y 4 · Farbe 9 harness · Region left_shoe`. Diese Zeile kann man ablesen und Claude so sagen.
- **Regionen**: benannte Rechtecke (je Frame änderbar, optional auf eine Ebene beschränkt), als beschriftete Umrisse auf der Leinwand (abschaltbar) und in der Vorschau. In der GUI entstehen sie aus einer Auswahl („Region aus Auswahl“), per MCP mit `set_region`.
- **Marker** (`kind: "marker"`): benannte Punkte wie `origin` oder ein Handanker, gezeigt als Fadenkreuz mit Beschriftung (der markierte Pixel bleibt sichtbar). Gesperrt schützt ein Marker nur seine Position und seinen Namen, nie die Pixel darunter. `move_region` auf einen gesperrten Marker schlägt fehl, Pixel-Schreiben dort funktioniert. In `render_animation` dient ein Marker als `track`-Anker: sein Punkt je Schritt wird verbunden. In der GUI: „als Marker“ beim Anlegen aus der Auswahl.
- **Gezielte Änderungen (MCP)**: Jeder Schreibbefehl braucht ein ausdrückliches Ziel, also Ebene und Frame, optional eine Region oder `clip`-Rechteck. Was außerhalb läge, wird nicht geändert und gemeldet. Eine Schlussprüfung stellt sicher, dass keine andere Cel, Ebene, Palette, kein Tag und keine Region verändert wurde. Verknüpfte Cels werden nur mit `edit_linked: true` beschrieben. Jede Antwort enthält einen kompakten Diff: Anzahl, umschließendes Rechteck, Zeilen vorher/nachher. Mit `dry_run: true` wird nur gezeigt, was passieren würde.
- **Sperren**: Ebenen, Frames und Regionen lassen sich sperren (GUI: Kästchen in der Zeitleiste bzw. Rechtsklick auf die Frame-Nummer, Häkchen bei Regionen; MCP: `locked`). Schreibversuche scheitern mit einer klaren Meldung; in der GUI malen Werkzeuge nicht in gesperrte Regionen.
- **Verlauf**: Jede Änderung ist ein rückgängig machbarer Schritt mit Beschreibung, sichtbar im Panel „Verlauf“ (Klick auf einen älteren Schritt geht dorthin zurück). MCP-Änderungen haben eine Notiz (`note`, sonst z. B. `Claude: set_rows walk_down/2 body [left_shoe] (3x2)`). Der Editor übernimmt sie beim Neuladen als Schritt. Alles landet auch im Journal neben der Datei (`.pxe-history/`, siehe FORMAT.md): MCP `history`, `undo` (auch über GUI-Speicherungen hinweg), benannte Schnappschüsse (`snapshot`, `list_snapshots`, `diff_snapshot`, `restore_snapshot`; in der GUI „Schnappschuss … Sichern / wiederherstellen“).

## Der KI live zusehen („KI folgen“)

Der Editor prüft Datei und Journal etwa alle 100 ms und spielt jede neue Änderung von außen der Reihe nach ab. Schnell aufeinanderfolgende Schreibvorgänge gehen nicht verloren; jeder wird ein eigener Verlaufsschritt mit Claudes Notiz.

- **Folgen** (Standard an; Schalter im Menü Ansicht und als Badge „Folgen: an/aus“ in der Statuszeile):
  - Bei einer Änderung springt der Editor zu Frame und Ebene der Änderung.
  - Er verschiebt die Ansicht und zoomt nur heraus, wenn nötig, damit der geänderte Bereich sichtbar ist.
  - Die geänderten Pixel leuchten etwa 600 ms gelb auf, mit einem verblassenden Rahmen.
  - Claudes Notiz erscheint 3 s als Banner oben auf der Leinwand und in der Statuszeile.
- **Wer war das?** Der MCP-Server nimmt den Namen aus `clientInfo.name` beim Verbinden: `claude-code`/`claude…` → „Claude“, `codex…` → „Codex“, sonst der Name mit großem Anfangsbuchstaben, ohne Angabe „KI“. So können Claude und eine Codex-Sitzung am selben Arbeitsordner arbeiten. Der Name steht als Autor im Journal, vor den Standardnotizen („Codex: set_rows …“) und in `focus`-Bannern. Der Editor zeigt ihn im Banner, im Verlauf und im Badge („Folgen: an · Codex“).
- **Pause**: Jede eigene Eingabe (Klick, Taste) pausiert das Folgen 5 s („Folgen: pausiert 4 s“). Der Inhalt wird trotzdem aktualisiert, nur die Ansicht springt nicht.
- **Ungespeicherte eigene Änderungen**: Nichts wird überschrieben, wie bisher erscheint der Konflikt-Balken.
- **MCP**:
  - `focus(path, frame, layer?, region? | rect?, note?)` lenkt nur die Ansicht („Claude: arbeitet an fist_left“), ohne etwas zu ändern. Ist die Datei im Fenster als Hintergrund-Tab offen, wechselt das Fenster zu diesem Tab (nur bei „Folgen: an“, nicht während der Pause). Eine Datei ohne Tab öffnet `focus` nicht, dafür gibt es `open_editor`.
  - `pace_ms` an jedem Schreibwerkzeug zeichnet die Änderung Zeile für Zeile mit dieser Pause (höchstens 48 Striche, höchstens 30 s). Man sieht sie entstehen, und am Ende ist sie ein einziger Verlaufsschritt.
  - `open_editor(path)` öffnet die Datei als Tab im laufenden Editorfenster des Arbeitsordners („opened … as tab in the running editor (pid …)“); ist sie dort schon offen, wird ihr Tab gewählt. Nur wenn kein Fenster läuft, startet ein neues, losgelöst auf dem Bildschirm des Nutzers (siehe „Tabs“). Fehlen dem MCP-Server die Anzeige-Variablen (Claude Code startet ihn ohne), leitet der Editor sie ab: `XDG_RUNTIME_DIR` (sonst `/run/user/<uid>`), der neueste `wayland-*`-Socket darin, `HYPRLAND_INSTANCE_SIGNATURE` bei genau einer Hyprland-Instanz, `DISPLAY=:0` nur als Notlösung. Danach wartet er 1,5 s: Beendet sich der Editor in der Zeit, kommt ein Fehler mit seiner Ausgabe statt „opened“. Die Ausgabe steht in `.pxe-history/<datei>/gui.log`.

## Tabs: mehrere Dateien in einem Fenster

- Jede offene Datei ist ein Tab in der Menüzeile, benannt relativ zum Arbeitsordner des Fensters (`toast_warrior/walk_side`). `•` davor heißt ungespeicherte Änderungen, ein gelber Punkt dahinter: von außen geändert (z. B. von Claude) und noch nicht angesehen; der Tooltip nennt die Änderung. `×` (oder mittlere Maustaste) schließt den Tab, `+` öffnet einen leeren.
- **Strg+Tab / Strg+Umschalt+Tab** wechseln, **Strg+W** schließt (mit ungespeicherten Änderungen kommt die bekannte Frage Speichern/Verwerfen/Abbrechen). Der letzte Tab wird beim Schließen durch ein leeres Dokument ersetzt. Beenden fragt einmal für alle Tabs mit ungespeicherten Änderungen.
- Öffnen (Strg+O, „Öffnen über Pfad…“) und Neu (Strg+N) legen einen neuen Tab an; ein unberührter leerer Tab wird dafür wiederverwendet.
- **Je Tab** bleiben: Frame, Ebene, Farbe, Auswahl (auch schwebend), Wiedergabe, Zoom/Ausschnitt, Verlauf mit Rückgängig, ungespeicherte Änderungen, Konflikt-Balken, Banner und Aufleuchten, Statusmeldung und die rechte Leiste samt ihrem stehenden Frame beim Abspielen. **Fürs ganze Fenster** gelten: Werkzeug, Raster/Onion Skin/Regionen/Saubere Ansicht, die Vorschau, die Zwischenablage (Kopieren zwischen Dokumenten geht), Ablegen vorne/hinten, „alle gleichfarbigen“ beim Zauberstab und „KI folgen“.
- **Live**: Alle Tabs werden wie bisher alle 100 ms geprüft und neu geladen. Eine Änderung in einem Hintergrund-Tab holt den Tab nicht nach vorn, sie markiert ihn nur; der Tab springt dabei schon zur Stelle der Änderung, so dass man sie beim Hinsehen gleich sieht. Nur ein ausdrückliches `focus` der KI wechselt den Tab.
- **„KI folgen“ gilt fürs ganze Fenster**, nicht je Tab: Es ist die Entscheidung, ob die Ansicht der KI hinterherläuft, und genau diese Entscheidung steuert auch den Tab-Wechsel bei `focus`. Je Tab wäre sie unsichtbar, sobald der Tab im Hintergrund liegt, und ein `focus` könnte in einen Tab wechseln, in dem das Folgen aus ist. Auch die 5-s-Pause nach eigener Eingabe gilt fürs Fenster: Wer gerade arbeitet, dem wird weder die Ansicht noch der Tab weggezogen.
- **Ein Fenster je Arbeitsordner**: Das Fenster lauscht auf einem Unix-Socket `$XDG_RUNTIME_DIR/pixel-editor/<hash des Arbeitsordners>.sock` (sonst `/run/user/<uid>/…`). `pixel-editor open <datei>` und MCP `open_editor` suchen das Fenster vom Ordner der Datei aus nach außen (beim MCP-Server zuerst dessen Arbeitsordner) und schicken ihm die Datei; nur ohne Fenster startet ein neues mit `--workdir` (MCP: dessen Arbeitsordner; CLI: das aktuelle Verzeichnis, wenn die Datei darin liegt, sonst ihr Ordner). Ein Socket, dessen Fenster abgestürzt ist, nimmt keine Verbindung an; er wird übergangen und vom nächsten Fenster ersetzt. Ein Fenster, das gerade nicht gezeichnet wird (anderer Arbeitsbereich), antwortet nach 3 s mit „queued“ und öffnet den Tab beim nächsten Bild.
- **Wiederherstellen**: Die offenen Tabs merkt sich der Editor je Arbeitsordner in `~/.config/pixel-editor/settings.json` und öffnet sie beim nächsten Start für denselben Ordner wieder (fehlende Dateien entfallen); die angeforderte Datei bleibt vorn.
- `get_user_view` sieht den Tab, der vorn ist. Ein Hintergrund-Tab hat keine Ansichtsdatei.

## Werkzeugleiste

Links steht eine Symbolleiste mit Werkzeuggruppen wie in Photoshop: **Stift** (Stift, Schattieren), Radierer, Füllen, Linie, **Formen** (Rechteck, Ellipse), Pipette, **Auswahl-Werkzeuge** (Auswahl, Auswahl-Pinsel, Zauberstab, Lasso) und KI zeigen. Jeder Knopf zeigt das zuletzt benutzte Werkzeug seiner Gruppe; ein kleines Dreieck unten rechts heißt, dass die Gruppe mehr enthält. Klick wählt das gezeigte Werkzeug. **Rechtsklick, langes Drücken oder Klick auf das Dreieck** klappt die Gruppe auf (Symbol, Name, Kürzel); ein Klick daneben oder Esc schließt sie. Der Tooltip nennt Name und Kürzel. Darunter stehen Name und Kürzel des aktiven Werkzeugs, seine Hinweise und Einstellungen (gefüllt, Symmetrie mit Achsen, Ablegen vorne/hinten, Zauberstab-Modus, Markierungsknöpfe). Das Menü **Werkzeuge** ist genauso gruppiert (Untermenüs „Formen“ und „Auswahl-Werkzeuge“).

## Auswählen und Verschieben

Eine Auswahl ist eine Pixelmaske: genau die gewählten Pixel, nicht ihr umgebendes Rechteck. Das Rechteck (M) ist nur eine Art, sie zu bilden. Dazu kommen **Auswahl-Pinsel** (Umschalt+M), **Zauberstab** (W) und **Lasso** (Q), in der Werkzeugpalette und im Menü Werkzeuge. Mit Umschalt kommt eine Geste zur Auswahl dazu, mit Alt oder Strg wird sie abgezogen, ohne Taste ersetzt sie die Auswahl. Der Umriss läuft an den Kanten der Maske entlang (weiß innen, schwarz außen); die Statuszeile nennt Rechteck und Pixelzahl („Auswahl x 1..6 y 0..6 (6x7), 12 Pixel“).

**Auswahl-Befehle** im Menü **Auswahl** und per Rechtsklick auf die Leinwand mit einem Auswahl-Werkzeug: Auswahl aufheben (Strg+D), Alles auswählen (Strg+A), Auswahl umkehren (Strg+Umschalt+I), Ausschneiden, Kopieren, Einfügen, Löschen und, solange Pixel schweben, „Hier vorne ablegen“ / „Hier hinten ablegen“ (einmalig, der eingestellte Modus bleibt). Beim Auswahl-Pinsel öffnet nur ein Rechtsklick ohne Ziehen das Menü; Ziehen mit der rechten Taste nimmt Pixel heraus, einzelne Pixel nimmst du mit Alt+Klick (oder Strg+Klick) heraus.

Verschieben (in der Auswahl ziehen oder Pfeiltasten), Strg+C/X/V, Entf und Enter wirken nur auf die gewählten Pixel; alles andere im Rechteck bleibt, wo es ist. Beim Kopieren wandert die Form mit, eingefügt wird wieder genau diese Form.

**Ablegen: Vorne / Hinten** (Werkzeugfeld bei den Auswahl-Werkzeugen und beim Verschieben, Menü Werkzeuge, **Umschalt+H**):
- **Vorne** (Standard): die verschobenen Pixel überdecken, was dort ist; transparente verschobene Pixel lassen das Darunterliegende stehen.
- **Hinten**: die verschobenen Pixel füllen nur Stellen, die auf der Ebene leer sind. Sie landen also hinter dem Bestehenden.

Die Vorschau beim Verschieben zeigt schon das Ergebnis im gewählten Modus. Esc legt die Pixel unverändert an ihren Ursprung zurück. Im Verlauf steht beim hinten Abgelegten „· hinten“.

## Was du gerade ansiehst: `get_user_view`

Die KI (Claude, Codex …) sieht deinen Bildschirm nicht. Damit „das hier“ oder „die Auswahl“ trotzdem eindeutig ist, schreibt der Editor laufend auf, was er zeigt, und die KI liest das mit dem MCP-Werkzeug `get_user_view`.

- **Ansichtsdatei**: Jeder laufende Editor schreibt `.pxe-history/<datei>/views/<pid>.json` neben das offene Dokument. Eine Datei je Prozess, mehrere offene Editoren stören sich also nicht. Geschrieben wird bei jeder Änderung, höchstens 5-mal pro Sekunde, und sonst einmal pro Minute als Lebenszeichen. Beim Schließen oder beim Wechsel zu einem anderen Dokument verschwindet die Datei.
- **Inhalt**: Dokument, Frame (Index, Name, `tag/n`-Adresse), Ebene (Index, Name), Auswahl (Rechteck der Auswahl und, bei einer geformten Auswahl, welche Pixel darin gewählt sind), das Pixel unter dem Mauszeiger (x, y, Taste, Farbname, sichtbare Farbe, Regionen dort), Zoom, Werkzeug, ob „KI folgen“ an ist, ob es ungespeicherte Änderungen gibt, Zeitstempel und pid.
- **Werkzeug „KI zeigen“ (K)**: in der Werkzeugleiste (blaues Symbol) und im Menü **Werkzeuge**. Damit setzt du beliebig viele nummerierte Markierungen, auf jedem Frame, jeweils mit einer Notiz für die KI:
  - **Ziehen** auf freier Fläche markiert einen Bereich, ein **Klick** ein Pixel. Jede Markierung kommt dazu (#1, #2, … je Dokument), keine ersetzt eine andere. Pixel und Auswahl bleiben, wie sie sind.
  - Danach öffnet sich unter der Markierung ein kleines Textfeld: Notiz tippen („brauner Block weg“), **Enter** oder ein Klick daneben speichert sie. Leer lassen geht auch. Ein Klick daneben schließt nur das Feld und setzt keine neue Markierung.
  - **Klick in eine Markierung** wählt sie aus und öffnet ihre Notiz zum Ändern. **Ziehen im Inneren** verschiebt sie, **Ziehen an Ecken und Kanten** (weiße Griffe der ausgewählten Markierung) ändert die Größe. Esc hebt die Auswahl auf.
  - **Rechtsklick** auf eine Markierung entfernt nur diese.
- **KI zeigen ohne Werkzeug** (Knopf in der Statuszeile, Menü Werkzeuge oder Ansicht, **Strg+Umschalt+K**, auch während ein Textfeld den Cursor hat): fügt eine Markierung für die aktuelle Auswahl oder, ohne Auswahl, für den ganzen Frame hinzu.
- **Anzeige**: Die Markierungen des aktuellen Frames sind hellblau umrandet und mit Nummer und gekürzter Notiz beschriftet („#1 brauner Block weg“). Markierungen anderer Frames erscheinen nicht auf der Leinwand; dafür zeigt die Zeitleiste über jedem Frame mit Markierungen einen blauen Punkt mit ihrer Anzahl (Tooltip: „2 KI-Markierungen“). Die Saubere Ansicht blendet alles aus.
- **Entfernen**: Rechtsklick (eine), „KI-Markierungen dieses Frames entfernen“ und „Alle KI-Markierungen entfernen“ in den Menüs Werkzeuge/Ansicht, „Frame: N entfernen“ / „Alle N entfernen“ unter dem Werkzeug, „KI-Markierungen (N) ×“ in der Statuszeile (alle). Sind alle weg, beginnt die Nummerierung wieder bei 1.
- **Speicherort**: je Dokument (also je Tab) in `.pxe-history/<datei>/marks.json`. Sie bleiben, auch wenn der Editor geschlossen wird, und der Editor lädt sie beim Öffnen wieder. Ändert die KI sie (`clear_marks`), aktualisiert sich der offene Editor sofort. Eine alte `pointer.json` (früher: ein einzelner Zeiger) wird beim Lesen als Markierung #1 übernommen.
- Kommt eine Taste nicht an: `PIXEL_EDITOR_DEBUG_KEYS=1 pixel-editor …` schreibt jede empfangene Taste mit Modifikatoren nach stderr (bei `open` in `gui.log`).
- **`get_user_view`** (ohne Argumente) liefert:
  - für jeden lebenden Editor auf einem Dokument im Arbeitsordner die Ansicht, neueste zuerst. Ansichtsdateien von beendeten Prozessen oder ohne Lebenszeichen seit 10 Minuten werden ignoriert und gelöscht.
  - bei einer Auswahl zusätzlich die ausgewählten Pixel, genau wie `get_rows` für ihr Rechteck auf diesem Frame und dieser Ebene. Bei einer geformten Auswahl (Zauberstab, Auswahl-Pinsel, Lasso, zusammengesetzt) steht `·` für nicht ausgewählte Pixel im Rechteck, dazu die Zahl der gewählten Pixel;
  - alle „KI zeigen“-Markierungen jedes Dokuments im Arbeitsordner (auch wenn kein Editor mehr darauf offen ist), nach Frame gruppiert: Nummer, Frame (Adresse, Index, Name), Rechteck oder „the whole frame“, Ebene, Alter, Notiz und die Pixel des Bereichs. Hat die KI sie erledigt, entfernt sie sie mit `clear_marks`;
  - einen deutlichen Hinweis, wenn kein Editor offen ist.
- **Ungespeicherte Änderungen**: Die KI liest und schreibt die Datei. Hast du ungespeicherte Änderungen, sagt `get_user_view` das ausdrücklich, weil die Pixel aus der Datei dann von deinem Bildschirm abweichen können.
- Ein neues, noch nie gespeichertes Dokument hat keine Datei, also auch keine Ansicht für die KI („KI zeigen“ bittet dann ums Speichern). Dokumente außerhalb des Arbeitsordners des MCP-Servers sieht die KI ebenfalls nicht.

## Bauen

Rust ist per rustup im Benutzerverzeichnis installiert (`~/.cargo/bin`).

```sh
cd pixel-editor
cargo build --release          # Binary: target/release/pixel-editor
cargo test                     # Kern-, MCP- und GUI-Tests (GUI offscreen über wgpu)
```

Zusätzliche Systempakete waren nicht nötig. Gerendert wird mit wgpu (Vulkan). Das OpenGL-Backend (glow) scheitert auf diesem Rechner unter nativem Wayland mit `glutin error: [3003]` (EGL_BAD_ALLOC, NVIDIA). Die GUI-Tests laufen nacheinander (eine Sperre in den Tests), weil mehrere parallel erzeugte wgpu-Geräte hier hängen blieben.

## Starten

```sh
target/release/pixel-editor                        # leeres 64×64-Dokument
target/release/pixel-editor figur.pxe.json         # öffnen (oder neu anlegen, wenn sie fehlt)
target/release/pixel-editor figur.aseprite         # Aseprite-Datei öffnen; Speichern schreibt wieder .aseprite
target/release/pixel-editor open figur.pxe.json    # als Tab ins laufende Fenster des Arbeitsordners, sonst neues Fenster (losgelöst)
target/release/pixel-editor --workdir art art/held/walk.pxe.json   # Fenster mit Arbeitsordner (Tab-Namen, `open`)
target/release/pixel-editor mcp ~/pixel-art   # MCP-Server für diesen Ordner
target/release/pixel-editor export figur.pxe.json godot [ausgabeordner] [--scale N] [--godot-res-dir res://…]
target/release/pixel-editor preview figur.pxe.json vorschau.png [--zoom 8] [--frame f0]
target/release/pixel-editor render-animation figur.pxe.json walk_down anim.png [--region NAME|x,y,w,h] [--track NAME|x,y,w,h] [--zoom N] [--jump 2]
target/release/pixel-editor import-world world.txt karten/world.tilemap.json --tileset art/tiles32.pxe.json [--force]
target/release/pixel-editor export-world karten/world.tilemap.json world.txt
target/release/pixel-editor export karten/world.tilemap.json godot [ausgabeordner]   # oder world_txt
target/release/pixel-editor export art/tiles32.pxe.json godot_tileset [ausgabeordner]
target/release/pixel-editor preview karten/world.tilemap.json welt.png [--zoom N] [--no-grid]
```

## Spiegeln, Drehen, Symmetrie

**Menü „Bild“**: Horizontal spiegeln (**Alt+H**), Vertikal spiegeln (**Alt+V**), 90° rechts drehen (**Alt+R**), 90° links drehen (**Alt+L**), 180° drehen (**Alt+Umschalt+R**). Die Kürzel und die obersten Einträge wirken auf die Auswahl, ohne Auswahl auf den aktuellen Cel. Die Untermenüs wählen den Bereich ausdrücklich:
- **Auswahl**: genau die gewählten Pixel (auch Zauberstab-, Lasso- und Pinsel-Formen; die Maske dreht sich mit), gedreht um die Mitte ihres Rechtecks. Schwebende Pixel (verschoben oder eingefügt) drehen sich, ohne den Cel zu ändern, bis sie abgelegt werden. Dieselben fünf Einträge stehen im Rechtsklick-Menü der Auswahl-Werkzeuge.
- **Cel**: der aktuelle Cel um die Leinwandmitte; Regionen, die nur für diese Ebene gelten, wandern mit.
- **Frame (alle Ebenen)**, **Frames des Tags** (des abgespielten Tags oder des ersten, der den Frame enthält), **Alle Frames**: um die Leinwandmitte, damit Ebenen und Frames deckungsgleich bleiben. Jede verknüpfte Cel wird genau einmal gedreht. Regionen und Marker auf diesen Frames wandern mit.

Ein nicht quadratischer Bereich, um 90° gedreht, belegt das gedrehte Rechteck auf derselben Mitte. Was dabei über die Leinwand hinausgeht, wird abgeschnitten; die Statuszeile nennt die Zahl („12 Pixel an der Leinwand abgeschnitten“). Gesperrte Ebenen und Frames werden übersprungen und genannt, Pixel in gesperrten Regionen bleiben, gesperrte Regionen und Marker behalten ihr Rechteck. Jede Drehung ist ein Rückgängig-Schritt mit Beschreibung.

**„Animation gespiegelt duplizieren…“** (Menü Bild) schreibt eine horizontal gespiegelte Kopie des Dokuments in eine neue Datei und öffnet sie im neuen Tab. Vorgeschlagen wird der Name mit `right`↔`left` getauscht, `walk_side` wird zu `walk_left`, sonst `<name>_mirrored`. Tags, Dauern, Ebenen, Sperren, verknüpfte Cels und Regionen bleiben erhalten; Regionen-Rechtecke und Marker werden mitgespiegelt. Das Original bleibt unverändert.

**Symmetrisch zeichnen** (Werkzeugfeld bei Stift, Radierer, Füllen, Linie, Rechteck, Ellipse; Menü Bild; **Alt+S** schaltet Aus → Vertikal → Horizontal → Beide): jeder Strich, jede Form und jede Füllung wird an der vertikalen Achse (links/rechts), der horizontalen (oben/unten) oder an beiden gespiegelt, auch in der Vorschau beim Ziehen. Die Achsen liegen zuerst in der Leinwandmitte und erscheinen als gestrichelte Linien. Verschieben: am dreieckigen Griff über (vertikale Achse) bzw. links neben der Leinwand (horizontale Achse) ziehen, oder die Zahl im Werkzeugfeld eingeben (in halben Pixeln: `12` liegt auf einer Pixelkante, `12.5` geht durch die Pixelmitte). „Mitte“ setzt sie zurück. Die Einstellung gilt je Tab.

## Saubere Linien und Schattieren

**Pixel-Perfect** (Werkzeugfeld bei Stift und Radierer, standardmäßig an, gemerkt in `~/.config/pixel-editor/settings.json`): Wie in Aseprite entfernt der Editor beim Freihandzeichnen die doppelten Eckpixel. Wo der Strich ein „L“ macht (ein Pixel berührt seinen Vorgänger und Nachfolger gerade, diese beiden einander aber nur schräg), bleibt die Ecke weg; ein Pixel, das die Ecke schon gemalt hatte, bekommt seine Farbe von vor dem Strich zurück. Linien und Kurven bleiben so 1 Pixel dünn. Mit Symmetrie und im Kachel-Modus gilt das für alle Kopien, weil erst der Strich bereinigt und dann gespiegelt bzw. gewrappt wird. Linie, Rechteck und Ellipse betrifft die Option nicht (die sind ohnehin sauber).

**Farbrampen**: geordnete Reihen von Palettenfarben, von dunkel nach hell (z. B. Kruste `b 8 9 a`). Sie stehen im Dokument (siehe FORMAT.md) und als Reihen über der Palette:
- **Anlegen**: Farben in der Palette mit **Strg+Klick** markieren, in der Reihenfolge dunkel → hell (eine kleine Nummer zeigt die Reihenfolge, nochmal Strg+Klick nimmt eine heraus), dann „Rampe anlegen (n Farben)“. Sie heißt erst „Rampe 1“; **Doppelklick auf den Namen** benennt sie um (Enter).
- **Umsortieren**: Klick auf eine Farbe der Reihe wählt sie; `<` / `>` schieben die aktuelle Farbe eine Stelle dunkler / heller, „hell“ sortiert die ganze Rampe nach Helligkeit. `+` hängt die aktuelle Farbe hell an, `-` nimmt sie heraus.
- **Löschen**: `×` (die Farben bleiben in der Palette). Wird eine Palettenfarbe entfernt, verschwindet sie auch aus ihren Rampen. Alles ist ein Schritt im Verlauf (Strg+Z).

**Schattieren** (Werkzeug in der Stift-Gruppe, Taste **D**): Klick oder Ziehen ersetzt jedes Pixel durch die nächsthellere Farbe seiner Rampe, die **rechte Maustaste** durch die nächstdunklere. Am Ende der Rampe bleibt die Farbe stehen. Pixel, deren Farbe in keiner Rampe liegt (z. B. die Kontur oder Transparenz), bleiben unverändert. In einem Strich ändert sich jedes Pixel nur einmal, auch wenn man mehrmals darüberfährt; für eine zweite Stufe setzt man neu an. Symmetrie, Kachel-Modus, Auswahl und gesperrte Regionen wirken wie beim Stift. Liegt eine Farbe in mehreren Rampen, gilt die erste.

Für Claude: `set_ramps` legt die Rampen fest, `get_document` zeigt sie, `shade` schattiert Pixel, ein Rechteck oder eine Region (siehe Werkzeuge).

## Farbe ersetzen, Dither, Kontur

**Farbe ersetzen…** (Menü Bild, oder **Rechtsklick auf eine Palettenfarbe** > „Diese Farbe ersetzen durch …“): Farbe A wird durch Farbe B ersetzt (beide auch transparent; „Tauschen“ dreht sie um). Der Dialog wählt den Bereich wie die Untermenüs von Spiegeln/Drehen: **Auswahl** (genau die gewählten Pixel, auch Masken-Formen), **Cel**, **Frame (alle Ebenen)**, **Frames des Tags**, **Alle Frames**; bei den drei letzten optional „nur Ebene …“. Die **Vorschau** zählt die betroffenen Pixel und Cels, bevor etwas geschrieben wird. Gesperrte Ebenen und Frames werden übersprungen und genannt, Pixel in gesperrten Regionen bleiben und werden gezählt. Verknüpfte Cels werden einmal geändert (und damit in allen verknüpften Frames). Ein Rückgängig-Schritt.

**Dither** (Werkzeugfeld bei **Füllen** und bei **Rechteck/Ellipse „gefüllt“**): statt einer Farbe ein Muster aus der aktuellen Farbe und einer **2. Farbe** (aus der Palette, transparent, oder per „Rampe dunkler“/„Rampe heller“ die Nachbarstufe der Farbrampe). Muster: Schachbrett 50 %, 25 %, 75 %, Bayer 4×4 in den Stufen 2/16, 6/16, 10/16, 14/16, Linien horizontal und diagonal. Das Muster ist **an der Leinwand ausgerichtet** (Pixel x, y bekommt immer dieselbe der beiden Farben) und wiederholt sich alle 2 oder 4 Pixel: Füllungen nebeneinander gehen nahtlos ineinander über, und Tiles mit einer Kantenlänge von 4·n Pixeln (32×32) kacheln nahtlos. Auswahl (auch Masken), Symmetrie, Kachel-Modus und gesperrte Regionen wirken wie beim normalen Füllen; die Formvorschau beim Ziehen zeigt das Muster. Rechtsklick füllt wie immer transparent, ohne Muster.

**Kontur (Outline)…** (Menü Bild): zieht eine 1-Pixel-Kontur in einer gewählten Farbe um die nicht-transparenten Pixel, je Cel des Bereichs (Auswahl, Cel, Frame, Tag, alle Frames, optional nur eine Ebene, wie oben):
- **außen**: auf die Pixel rund um die Form (bei einer Auswahl die gewählten Pixel; die Kontur darf dann 1 Pixel über die Auswahl hinausgehen), **innen**: auf die Randpixel der Form selbst (der Leinwandrand zählt als außen).
- **4er-Nachbarschaft**: nur gerade Nachbarn, die Ecken bleiben frei (typische Pixelart-Kontur); **8er**: auch diagonal, geschlossene Ecken.
- **Nur transparente Pixel füllen** (nur bei außen): vorhandene Farbe bleibt stehen, z. B. wo die Kontur einer Auswahl über anderes Gezeichnetes läuft.

Für Claude: `replace_color`, `outline` (Bereiche wie `transform`) und `dither` an `fill`/`draw_rect` (siehe Werkzeuge).

## Frames umsortieren, Referenz, Ebenen-Deckkraft

**Frames wählen**: Klick auf eine Frame-Nummer oder ein Vorschaubild wählt den Frame, **Umschalt+Klick** einen Bereich, **Strg+Klick** nimmt einzelne Frames dazu oder heraus. Gewählte Frames haben einen blauen Kopf. Auf die Auswahl wirken: **Löschen** (Knopf und Alt+C; gesperrte Frames verhindern es, ein Frame bleibt immer), **„n Frames duplizieren“** (Kopien hinter dem letzten gewählten, danach gewählt), die Knöpfe **`<` / `>`** (die Auswahl als Block einen Platz weiter), **„für Auswahl (n)“** neben der Dauer in den Eigenschaften und das **Rechtsklick-Menü** einer Frame-Nummer (duplizieren, löschen, Dauer für die Auswahl).

**Frames per Ziehen sortieren**: ein **Vorschaubild ziehen**, oder **Alt gedrückt halten und eine Frame-Nummer (oder Cel) ziehen**. Eine gelbe Linie zeigt, wo die Frames landen; beim Loslassen wandern alle gewählten Frames als Block dorthin (in ihrer Reihenfolge, ein Rückgängig-Schritt). Ziehen auf den Nummern oder Cels **ohne** Alt bleibt Scrubben. Mit dem Frame wandern: seine Cels auf allen Ebenen (verknüpfte bleiben verknüpft), Name, Dauer, Sperre, die Rechtecke der Regionen auf diesem Frame und die „KI zeigen“-Markierungen (sie hängen am Frame-Namen, auch nach Rückgängig). **Tags behalten ihre Frames**: ein Tag folgt seinen Frames, die liegen bleiben; ein Frame, der mitten in einen Tag fällt, gehört danach dazu; einer, der herausgezogen wird, nicht mehr; Umsortieren innerhalb eines Tags lässt ihn, wie er ist; zieht man alle Frames eines Tags, wandert der Tag mit. Für Claude: `move_frames(frames, to)`.

**Referenz** (Knopf „Referenz…“ neben „sichtbar“/„gesperrt“ der Ebene oder Menü Ansicht): ein Bild (PNG/JPG) oder ein anderes Dokument liegt als halbtransparente Vorlage **unter** oder **über** den Pixeln, zum Nachzeichnen oder Vergleichen (z. B. `walk_up` über `walk_side`). Ein Dokument als Referenz zeigt den Frame mit derselben Nummer wie der aktuelle (bei weniger Frames den letzten). Im Fenster: Datei (relativ zum Dokument) und „Laden“ oder „…“, zeigen, sperren, über den Pixeln, **Verschieben** (dann verschiebt Ziehen auf der Leinwand die Vorlage in ganzen Pixeln statt zu malen), X/Y, Größe (×0,01 bis ×64, Nearest Neighbour), Deckkraft, entfernen. Eine gesperrte Referenz lässt sich nicht verschieben oder skalieren. Die Referenz wird **nie exportiert, nie in Pixel geschrieben** und erscheint nicht in `render_preview`, Vorschau-Fenster, Vorschaubildern oder `get_rows`.

Sie wird **im Dokument** gespeichert (Feld `reference` in `.pxe.json`, siehe FORMAT.md), nicht in `.pxe-history`. Gründe: Sie gehört zu genau diesem Dokument und soll mit ihm umziehen (Kopie, Git); Änderungen an ihr laufen damit durch denselben Verlauf wie alles andere (Rückgängig in GUI und MCP, Journal, Diff, Schnappschüsse); und Claude sieht sie in `get_document`. `.pxe-history` ist löschbar und gehört nicht ins Repository, eine dort abgelegte Vorlage ginge damit verloren. Ohne Referenz schreibt der Editor das Feld nicht, ältere Dateien bleiben byte-gleich. `.aseprite` hat keinen Platz dafür: beim Speichern als `.aseprite` geht sie verloren (die Statuszeile sagt es), `set_reference` lehnt `.aseprite` ab.

**Ebenen-Deckkraft** (Regler in den Eigenschaften, MCP `set_layer opacity`) wirkt überall, wo Pixel zusammengesetzt werden: Leinwand, Formvorschau beim Ziehen (zeigt die Form so, wie sie gezeichnet wird), Vorschau-Fenster, Vorschaubilder, Onion Skin, `render_preview`, `render_animation`, `check_seams`, Karten und alle Exporte (PNG-Sheet, Godot, Godot-TileSet, Unity, GIF; `.aseprite` speichert die Deckkraft als Ebenen-Eigenschaft). Getestet für PNG-Sheet, Godot und Unity (halbe Deckkraft über Weiß ergibt die Mischfarbe, über nichts Alpha 128) und GIF. GIF kennt keine Halbtransparenz: Mischfarben über deckenden Pixeln stimmen exakt, eine halb durchsichtige Ebene über nichts erscheint ab 50 % deckend, darunter gar nicht.

**Ebenengruppen gibt es bewusst nicht.** Ebenen sind im Format eine flache Liste, und Cels, MCP-Werkzeuge, Regionen (`layer`), Markierungen und Diffs sprechen Ebenen über ihren Index an. Eine Gruppe wäre eine Ebene ohne Pixel in dieser Liste (oder ein zweites Verschachtelungsfeld) und würde jede Ebenen-Nummer mehrdeutig machen; ein sauberer Einbau hieße Formatversion 2. Was Gruppen bringen sollen, deckt bisher ab: mehrere Ebenen nacheinander per Punkt aus-/einblenden. Aseprite-Gruppen werden beim Import weiter aufgelöst (Sichtbarkeit vererbt).

## Kachel-Modus (Tiles nahtlos zeichnen)

Wie der „Tiled Mode“ von Aseprite: Die Leinwand zeigt das Bild wiederholt, damit man beim Zeichnen sieht, ob ein Tile nahtlos kachelt.

- **Einschalten**: Menü **Ansicht → Kachel-Modus** (Aus / Horizontal / Vertikal / Beide), Zeile „Kacheln“ im Werkzeugfeld oder **Alt+T** (Aus → Beide → Horizontal → Vertikal → Aus). Horizontal legt Kopien links und rechts daneben, Vertikal darüber und darunter, Beide zeigt 3×3. Die Kopien sind leicht abgedunkelt, das echte Bild in der Mitte hat einen gelben Rahmen; „Einpassen“ (0) passt alle Kopien ein. Das Pixelraster läuft über die Kopien weiter.
- **In allen Kopien zeichnen**: Stift, Radierer, Linie, Rechteck, Ellipse, Füllen und die Auswahl-Werkzeuge (Rechteck, Pinsel, Zauberstab, Lasso) wirken überall in den Kopien und schreiben ins echte Bild (Wrap per Modulo). Ein Strich, der rechts hinausläuft, kommt links wieder herein, auch schon in der Vorschau beim Ziehen. Füllen und Zauberstab laufen über den Rand hinweg weiter. Die Symmetrie spielt mit: gespiegelte Punkte, die über den Rand fallen, wrappen ebenfalls. Pipette und Statuszeile nennen über einer Kopie das echte Pixel.
- **Vorschau** (P): Knopf „Kacheln“ (auch im Untermenü Ansicht → Kachel-Modus: „Vorschau auch gekachelt“) zeigt das Bild so oft wie der Kachel-Modus, bei ausgeschaltetem Modus 3×3.
- **Je Dokument gemerkt** (in `~/.config/pixel-editor/settings.json`, `tile_modes`). Dokumente, in deren Namen oder Ordner „tile“ vorkommt (`tiles32.pxe.json`, `tiles/gras.pxe.json`), starten im Modus „Beide“, solange nichts anderes gemerkt ist. **Neu** schaltet ihn mit den Vorlagen „Tile 32×32“/„Tile 16×16“ ein (Schalter „Kachel-Modus“ im Dialog); in einem Tile-Ordner ist die Vorlage Tile 32×32 vorausgewählt.
- **Für Claude**: `render_preview` mit `tiled: true` zeigt jeden Frame 3×3 (Lineale zählen innerhalb des Tiles, rosa Striche markieren die Nahtstellen, ohne Raster), `check_seams` vergleicht die rechte Spalte mit der linken und die unterste Zeile mit der obersten und meldet harte Kanten (siehe Werkzeuge).

## Karten (Tilemaps): Umgebungen aus Tiles zusammensetzen

Eine Karte (`<name>.tilemap.json`, Format in [FORMAT.md](FORMAT.md#karten-tilemapjson)) stempelt Tiles aus einem Tileset-Dokument (z. B. `tiles32.pxe.json`, ein Tile je Frame) auf ein Raster: Wiese, Weg, Wasser, Mauern, Bäume. Sie ist wie `world.txt` aufgebaut, ein Zeichen je Zelle, und die Legende sagt, welches Tile (per Frame-Name) und welches Objekt (ein größeres Dokument wie `tree64.pxe.json`, das auf der Zelle steht) ein Zeichen zeigt.

**Anlegen und öffnen**: Datei → **Neue Karte…** (Tileset, Dateiname, Größe; gefüllt mit dem ersten Tile) oder **world.txt importieren…**, oder eine `.tilemap.json` öffnen wie jedes Dokument (Strg+O, `pixel-editor karte.tilemap.json`, MCP `open_editor`). Eine Karte ist ein eigener Tab.

**Bedienung im Karten-Tab**:

| Taste / Maus | Funktion |
|---|---|
| Klick auf ein Tile in der Palette (links, „Tileset“) | dieses Tile als Pinsel; hat es noch keine Taste, bekommt es beim ersten Setzen die nächste freie (Teil desselben Rückgängig-Schritts) |
| Klick auf einen Legenden-Eintrag (links, „Legende“) | diese Taste als Pinsel, auch Objekte (`T` Baum) und Platzhalter (`@`) |
| B / E / U / G / I | Stift / Radierer / Rechteck / Füllen (zusammenhängende gleiche Zellen) / Pipette |
| Linksklick, ziehen | ganze Tiles setzen (Rechteck: aufziehen, Vorschau als weißer Rahmen) |
| Rechtsklick, ziehen | Tiles entfernen (bei jedem Werkzeug) |
| Alt + Klick | Taste der Zelle aufnehmen |
| Mausrad / + / − / 0 | zoomen (0,25× bis 12×) / einpassen |
| Mittlere Maustaste / Leertaste + ziehen | verschieben |
| Strg+' | Kachelraster an/aus |
| Strg+Z / Strg+Umschalt+Z | Rückgängig / Wiederholen (Verlauf rechts) |
| Strg+S | speichern (mit Journal-Eintrag wie bei Dokumenten) |

Rechts: Datei, Tileset, **Tileset im Tab öffnen**, Kachelgröße, **Kartengröße** (Breite/Höhe + „Größe übernehmen“, die linke obere Ecke bleibt), Raster, Zoom, Hinweise (fehlendes Tileset, unbekannter Tile-Name, falsche Kachelgröße) und der Verlauf. Die Statuszeile nennt Zelle, Taste mit Name und Tile sowie den Pinsel: `Zelle x 17, y 3 · ~ Wasser (water0) · Pinsel ~ Wasser (water0) · 46×28 Kacheln · Zoom 0.5×`.

**Live**: Ändert sich ein Tile im Tileset, ändert es sich überall auf der Karte. Ist das Tileset in einem anderen Tab offen, zeigt die Karte dessen Pixel schon während des Zeichnens, auch ungespeichert („live aus dem offenen Tab“). Gespeicherte Änderungen am Tileset oder an Objekt-Dokumenten (auch von der KI) und Änderungen an der Karte selbst (MCP `set_tiles` …) lädt der Karten-Tab wie Dokumente nach; geänderte Zellen leuchten kurz gelb, die Notiz steht in der Statuszeile, und der Schritt ist rückgängig zu machen. Bei ungespeicherten eigenen Änderungen erscheint der bekannte Konflikt-Balken.

**world.txt der Sandbox** (`world.txt` im Repo `pixel-sandbox`): Import über Datei → „world.txt importieren…“, CLI `import-world` oder MCP `import_world`; Export über Export → „world.txt“, CLI `export-world` oder MCP `export_world`. Die Zeichen bleiben dieselben, die Kommentarzeilen samt Legende werden unverändert an ihrer Stelle zurückgeschrieben: Import → Export ist byte-gleich (getestet mit einer Kopie der echten Datei). Die Legenden-Namen kommen aus den Kommentaren, die Tiles aus dieser Tabelle (`SANDBOX_TILES` in `core/src/tilemap.rs`, die einzige Stelle dafür):

| Zeichen | Tile in tiles32 | Objekt |
|---|---|---|
| `.` | grass | |
| `,` | grass2 | |
| `*` | flowers | |
| `=` | path | |
| `g` | tallgrass0 | |
| `~` | water0 | |
| `#` | wall_top | |
| `b` | bush | |
| `T` | grass | tree64.pxe.json (neben dem Tileset) |
| `^` | grass_high | |
| `H` | stairs | |
| `@` | grass | |
| `s` | grass | |

Zeichen, die dort fehlen, bleiben als Platzhalter (lila Kachel mit dem Zeichen) in der Legende und werden beim Import gemeldet; der Export schreibt sie unverändert zurück und warnt, wenn ein Zeichen in den Kommentaren der Datei nicht erklärt ist. **Der Editor zeigt nur die Basiskacheln.** Weg- und Wasserränder (Dual-Grid-Übergänge), Felswände vor Anhöhen, Mauerfronten, Deko (Blumen, Steine, Pilze), Schmetterlinge, Spieler und Schleime ergänzt das Spiel beim Laden selbst; das baut der Editor bewusst nicht nach.

**Godot-Export** (Menü Export → „Godot“, CLI `export karte.tilemap.json godot`, MCP `export` mit `target: "godot"`), nach `<Kartenordner>/export/<name>/`:
- `<tileset>_atlas.png`: alle Tiles des Tilesets, 8 je Reihe (Tile i bei Spalte i % 8, Reihe i / 8).
- `<tileset>_tileset.tres`: Godot-4-`TileSet` mit einer `TileSetAtlasSource`; jedes Tile trägt seinen Frame-Namen in der Custom-Data-Ebene `name` (`tile_data.get_custom_data("name")`).
- `<name>.tscn`: `Node2D` (Texturfilter Nearest) mit einer `TileMapLayer` „Tiles“ und, falls Objekte vorkommen, „Objects“ (y-sortiert) mit einem `Sprite2D` je Objekt-Zelle (`T_<x>_<y>`, Position = Fußpunkt der Zelle) und dessen PNG.
- Pfade stehen relativ zur jeweiligen Datei, mit `godot_res_dir` (MCP) bzw. `--godot-res-dir` (CLI) als feste `res://`-Pfade.
- Nur das Tileset: `export tiles32.pxe.json godot_tileset` (CLI/MCP).

Wohin die Dateien im Spiel gehören, entscheidest du; der Export schreibt nichts in die Sandbox.

## Bedienung

Aufbau wie in Aseprite: links Werkzeuge und Palette, in der Mitte die Leinwand, rechts die Eigenschaften (Ebene, Frame/Cel, Tag), unten die **Zeitleiste**:

- ein Raster aus Ebenen (Zeilen, oberste Ebene oben; links Sichtbarkeit, Sperre, Name) × Frames (Spalten; Nummer und Name, gesperrte Frames rot). Ein Punkt heißt: die Cel hat Pixel; ein Kreis: sie ist leer. Ein Ring markiert verknüpfte Cels, eine Linie verbindet benachbarte verknüpfte Cels.
- darüber die Tags als farbige Bereiche mit Richtungszeichen (`>` vorwärts, `<` rückwärts, `<>` Ping-Pong, `><` Ping-Pong rückwärts); sich überlappende Tags liegen in eigenen Spuren.
- **Scrubben**: linke Maustaste auf einer Frame-Nummer (oder einer Cel) drücken und nach links/rechts ziehen: der aktuelle Frame folgt der Spalte unter dem Mauszeiger, live auf Leinwand und rechter Leiste. Läuft die Wiedergabe, hält sie an und bleibt danach angehalten. Das Mausrad über den Frame-Nummern (und Tags/Vorschaubildern) geht je Raste einen Frame vor oder zurück; über den Cels scrollt es wie bisher.
- **Frames wählen und sortieren**: Umschalt+Klick (Bereich) und Strg+Klick (einzeln) auf Nummern oder Vorschaubilder; Vorschaubild ziehen oder Alt+Ziehen auf einer Nummer verschiebt die gewählten Frames (siehe „Frames umsortieren, Referenz, Ebenen-Deckkraft“).
- Frame-Nummern und Vorschaubilder (abschaltbar). Klick auf eine Cel wählt Ebene und Frame, auf einen Tag wählt ihn für Wiedergabe und Bearbeitung, auf den Punkt links schaltet die Ebene sichtbar/unsichtbar. Rechtsklick auf eine Cel: Verknüpfung lösen, mit vorherigem Frame verknüpfen, leeren.

Tastenkürzel (soweit sinnvoll wie in Aseprite):

| Taste / Maus | Funktion |
|---|---|
| B / D / E / G / I / L / U / Umschalt+U / M / Umschalt+M / W / Q / K | Stift / Schattieren / Radierer / Füllen / Pipette / Linie / Rechteck / Ellipse / Auswahl / Auswahl-Pinsel / Zauberstab / Lasso / KI zeigen (auch im Menü Werkzeuge) |
| Auswahl (M): ziehen | Rechteck auswählen; in der Auswahl ziehen oder Pfeiltasten (Umschalt = 8 px) verschieben die Pixel; Enter legt ab, Esc bricht ab |
| Auswahl-Pinsel (Umschalt+M) | Klicken oder Ziehen malt einzelne Pixel in die Auswahl; Alt/Strg+Klick oder Ziehen mit der rechten Taste nimmt sie heraus; Rechtsklick ohne Ziehen öffnet das Auswahl-Menü |
| Zauberstab (W) | Klick wählt die zusammenhängende Fläche gleicher Farbe auf der Ebene (4 Nachbarn); Schalter „alle gleichfarbigen“ im Werkzeugfeld und im Menü Werkzeuge; in der Auswahl ziehen verschiebt sie |
| Lasso (Q) | Umriss frei ziehen; gewählt wird alles innerhalb und auf dem Umriss |
| Umschalt / Alt oder Strg beim Auswählen | bei Rechteck, Zauberstab und Lasso: zur Auswahl hinzufügen / von ihr abziehen; ohne Taste ersetzt die Geste die Auswahl |
| Alt+H / Alt+V / Alt+R / Alt+L / Alt+Umschalt+R | Auswahl (sonst Cel) horizontal / vertikal spiegeln, 90° rechts / 90° links / 180° drehen (Menü Bild, siehe oben) |
| Alt+S | Symmetrisch zeichnen: Aus → Vertikal → Horizontal → Beide |
| Alt+T | Kachel-Modus: Aus → Beide → Horizontal → Vertikal (siehe oben) |
| Umschalt+H | Ablegen „Vorne“/„Hinten“ umschalten (auch im Werkzeugfeld und im Menü Werkzeuge), siehe unten |
| Strg+C / Strg+X / Strg+V | Auswahl kopieren / ausschneiden / einfügen (schwebend, verschiebbar, Enter legt ab) |
| Strg+A / Strg+D / Strg+Umschalt+I / Entf | alles auswählen / Auswahl aufheben / Auswahl umkehren / Auswahl löschen (auch im Menü Auswahl und per Rechtsklick auf die Leinwand) |
| Linksklick | mit der gewählten Farbe zeichnen |
| Rechtsklick | mit Transparenz zeichnen (bei jedem Malwerkzeug); beim Schattieren eine Stufe dunkler |
| Strg+Klick auf eine Palettenfarbe | für eine neue Farbrampe markieren (siehe „Saubere Linien und Schattieren“) |
| Rechtsklick auf eine Palettenfarbe | „Diese Farbe ersetzen durch …“ (siehe „Farbe ersetzen, Dither, Kontur“) |
| Alt + Klick | Farbe aufnehmen |
| Mausrad | zoomen in ganzen Stufen, um den Mauszeiger herum |
| Mittlere Maustaste / Leertaste + ziehen | verschieben |
| + / − / 0 | zoomen / einpassen |
| Strg+' | Pixelraster an/aus (hellere Linie alle 8 px) |
| F3 | Onion Skin (vorheriger Frame rot, nächster blau, hinter dem aktuellen) |
| H | Saubere Ansicht: Raster, Regionen samt Namen, Marker, Auswahlrahmen, Onion Skin, Aufleuchten und Banner von Claude, Mauszeiger-Kästchen und Leinwandrahmen auf einmal aus und wieder an. Die einzelnen Schalter (z. B. Raster) bleiben, wie du sie gesetzt hast. Auch per Knopf in der Statuszeile („Saubere Ansicht: an“) und im Menü Ansicht |
| P | Vorschau ein/aus: kleines Fenster oben rechts über der Leinwand mit dem aktuellen Frame, ohne jede Einblendung, in 1×/2×/3× (echte Bildschirmpixel), Hintergrund Schachbrett/Dunkel/Hell; „Tag abspielen“ spielt den Tag des aktuellen Frames in Echtzeit als Schleife ab. Verschiebbar (an der Titelleiste) und einklappbar; Lage, Maßstab, Hintergrund, eingeklappt und an/aus merkt sich der Editor in `~/.config/pixel-editor/settings.json`. Zeigt Claudes Änderungen sofort mit |
| Enter | Abspielen/Pause (gewählter Tag oder alle Frames, mit Richtung und Loop); bei schwebender Auswahl: ablegen |
| Alt+, / Alt+. (auch , / . und ← / →) | vorheriger / nächster Frame |
| ↑ / ↓ | Ebene darüber / darunter |
| Alt+N / Alt+B / Alt+C | neuer Frame (Kopie) / neuer leerer Frame / Frame löschen (bei mehreren gewählten: alle gewählten) |
| Umschalt+Klick / Strg+Klick auf eine Frame-Nummer | Frames als Bereich / einzeln wählen |
| Vorschaubild ziehen / Alt + Frame-Nummer ziehen | gewählte Frames verschieben (ohne Alt: scrubben) |
| Umschalt+N | neue Ebene |
| Strg+Z / Strg+Umschalt+Z (oder Strg+Y) | Rückgängig / Wiederholen |
| Strg+N / Strg+O / Strg+S / Strg+Umschalt+S | Neu / Öffnen (beide im neuen Tab) / Speichern / Speichern unter |
| Strg+Tab / Strg+Umschalt+Tab / Strg+W | nächster / vorheriger Tab / Tab schließen |
| Strg+Umschalt+K | KI zeigen: Auswahl oder Frame als weitere Markierung für die KI hinzufügen (`get_user_view`) |

- **Neu** bietet die Vorlagen Figur 48×48, Tile 32×32 und 16×16 (diese mit Kachel-Modus) sowie eine eingebaute Palette.
- **Zeitleisten-Leiste**: Abspielen, Animation (Tag) wählen, Onion Skin, Vorschaubilder; Frame: `+ Neu` (Kopie), `+ Leer`, `+ Verknüpft` (neuer Frame mit verknüpften Cels), „n Frames duplizieren“ (bei mehreren gewählten), Löschen, `<`/`>` verschieben (wirken auf die gewählten Frames); Ebene: Neu, Löschen, Hoch, Runter.
- **Eigenschaften**: Ebene (Name, sichtbar, gesperrt, „Referenz…“, Deckkraft), Frame (Name, Dauer, „für alle“, „für Auswahl“, gesperrt, Status der Cel: leer, eigenständig oder verknüpft mit Frames …, „Verknüpfung lösen“, „Mit vorherigem Frame verknüpfen“), Tag (Name, Bereich, Richtung, Loop, Farbe; „Übernehmen“, „Löschen“, „Neu“; **Folge** für eine beliebige Reihenfolge wie `0 1 0 2`, angelegt als verknüpfte Frames, damit der Tag ein Bereich bleibt wie in Aseprite), Regionen (Liste mit Rechteck im aktuellen Frame, Sperre, Löschen; Klick übernimmt sie als Auswahl; neue Region aus der Auswahl, auf Wunsch nur für diesen Frame), Verlauf (Schritte mit Beschreibung, Rückgängig/Wiederholen, Schnappschüsse).
- **Palette**: darüber die Farbrampen (siehe oben). Klick wählt, Strg+Klick markiert für eine Rampe, Name der Farbe, Farbe ändern (Farbwähler oder Hex-Feld), „+ Farbe“, „Entfernen“ (betroffene Pixel werden transparent). Tooltips zeigen Taste, Name und Farbwert.
- **Öffnen/Speichern** nutzt den Dateidialog des Systems (xdg-portal) für `.pxe.json` und `.aseprite`. Ohne Portal oder bei Abbruch erscheint ein eigener Pfad-Dialog.
- **Live-Neuladen**: etwa alle 0,3 s. Ein Neuladen lässt sich mit Strg+Z zurücknehmen. Bei ungespeicherten eigenen Änderungen wird nichts überschrieben, ein Hinweisbalken fragt nach.
- **Export** (Menü): PNG-Sheet, Godot, Unity, GIF, Aseprite nach `<Ordner des Dokuments>/export/<name>/`.

## MCP-Server

`pixel-editor mcp <arbeitsordner>` spricht MCP (JSON-RPC 2.0, eine Nachricht pro Zeile über stdin/stdout). Pfade sind relativ zum Arbeitsordner, Pfade außerhalb werden abgelehnt. Die Endung darf man weglassen (`"path": "held"` findet `held.pxe.json` oder `held.aseprite`). Ebenen und Frames gehen per Index oder Name, Standard ist jeweils 0.

Konfiguration für Claude Code (`.mcp.json` im Projekt):

```json
{
  "mcpServers": {
    "pixel-editor": {
      "command": "/pfad/zu/pixel-editor/target/release/pixel-editor",
      "args": ["mcp", "/pfad/zu/pixel-art"]
    }
  }
}
```

oder `claude mcp add pixel-editor -- /pfad/zu/pixel-editor/target/release/pixel-editor mcp /pfad/zu/pixel-art` (der Arbeitsordner muss existieren; `pixel-art` ist nur ein Beispiel).

### Für Claude gedacht

- **Lesen in Tastenzeilen**: `get_document` liefert eine Übersicht: Palette mit Namen, Ebenen mit Sperren, Frames mit ihren `tag/n`-Adressen, Tags samt Abspielreihenfolge und Regionen. Dazu je Cel die Pixel als Zeilen von Palettentasten, auf den Inhalt zugeschnitten, mit Lineal und Zeilennummern in absoluten Koordinaten. `get_rows` liest ein Rechteck oder eine Region einer Ebene oder, mit `composite: true`, des sichtbaren Gesamtbilds, samt Legende der benutzten Farben.
- **Schreiben mit Ziel**: `layer` und `frame` sind Pflicht, `region` oder `clip` grenzen weiter ein. `set_rows` beginnt ohne x/y an der Ecke der Region. Farben gehen per Taste oder Name (`"key": "harness"`).
- **Genaue Fehler**: Unbekannte Argumente werden mit der Liste der gültigen abgelehnt (z. B. `colour`). Fehlende Pflichtargumente werden benannt. Ungültige Tasten werden mit Position gemeldet (`rows[1][1] (x=2, y=3)`, `pixels[4]`). Unbekannte Ebenen, Frames, Tags und Regionen werden mit der Liste der vorhandenen gemeldet. Gesperrte Ziele und verknüpfte Cels werden mit dem Weg zur Lösung gemeldet. Bei einem Fehler wird nichts geschrieben.
- **Diff statt Vertrauen**: Jede Schreibantwort enthält die geänderten Pixel, das Rechteck und die Zeilen vorher/nachher sowie den Verlaufseintrag; `dry_run` zeigt das, ohne zu schreiben.
- **`render_preview`**: wie die Vorschau von pixelart-studio. Je Frame gibt es eine Beschriftung (Index, Name, ms, Tags, LINK), Koordinatenlineale, das gezoomte Bild mit Raster (heller alle 8 px), Regionen als beschriftete Umrisse (gesperrte gestrichelt) sowie 1×/2×-Vorschauen; unten die Palette mit Tasten. Das PNG wird nach `preview/` geschrieben und direkt als Bild zurückgegeben.

### Werkzeuge

| Werkzeug | Zweck |
|---|---|
| `list_documents` | `.pxe.json`/`.aseprite` im Arbeitsordner mit Tags und Regionen, Karten (`.tilemap.json`) mit Größe und Tileset |
| `create_document(name, width, height, palette?, layer_name?, …)` | neues Dokument; Palette als Name oder Liste `{key, color, name?}` |
| `get_document(path, layer?, frame?, pixels?, crop?)` | Übersicht + Tastenzeilen |
| `get_rows(path, layer?, frame?, region? \| x/y/w/h, composite?)` | Ausschnitt lesen |
| `set_pixels`, `set_rows`, `draw_line`, `draw_rect`, `draw_ellipse`, `fill`, `clear` | Pixel schreiben; immer `layer` + `frame`, optional `region`/`clip`, `edit_linked`, `dry_run`, `note` |
| `copy_region`, `move_region(region \| clip, dx?, dy?, to_frame?, to_layer?, mode?)` | Bereich kopieren/verschieben, auch in andere Frames/Ebenen; `move_region` verschiebt das Rechteck der Region auf diesem Frame mit |
| `transform(path, op, scope, layer?, frame?, region? \| rect?, frames? \| from/to?, tag?)` | spiegeln/drehen: `op` `flip_h`, `flip_v`, `rot90` (rechts), `rot180`, `rot270` (links); `scope` `rect`/`region`/`cel` (eine Ebene + Frame) oder `frame`/`frames`/`tag`/`all` (ganze Frames um die Leinwandmitte, alle Ebenen oder nur `layer`; Regionen wandern mit). Abgeschnittene Pixel werden gezählt. Diff, `dry_run`, `note`, `pace_ms`, Verlauf wie die anderen Schreibwerkzeuge; Cels, die mit Frames außerhalb verknüpft sind, brauchen `edit_linked` |
| `replace_color(path, from, to, scope?, layer?, frame?, region? \| rect?, frames? \| from/to?, tag?)` | Farbe `from` überall im Bereich durch `to` ersetzen (Tasten oder Farbnamen, `.` = transparent). `scope` wie bei `transform`; ohne `scope` gilt `region`/`rect`, wenn angegeben, sonst `cel`. Pixel in gesperrten Regionen bleiben und werden gezählt. Diff, `dry_run`, `note`, Verlauf |
| `outline(path, color, mode?, connectivity?, only_transparent?, scope?, …)` | 1-px-Kontur um die nicht-transparenten Pixel jedes Cels im Bereich: `mode` `outside` (Standard) / `inside`, `connectivity` 4 (Standard) / 8, `only_transparent` (nur außen) lässt farbige Pixel stehen. Bereiche wie `replace_color`; bei `rect`/`region` darf die Kontur außen 1 px darüber hinaus |
| `fill(…, dither?)`, `draw_rect(…, filled: true, dither?)` | `dither: {pattern, colors: [a, b]}` füllt mit einem Muster: `checker` (50 %), `25`, `75`, `bayer1`…`bayer15` (Bayer 4×4, n/16 erste Farbe), `lines_h`, `lines_d`; `colors` Tasten oder Farbnamen, z. B. zwei Rampenstufen (zweite Standard `.`, erste Standard `key`). An der Leinwand ausgerichtet, Periode 2 bzw. 4 |
| `mirror_document(src, dst, op?, keep_layers?)` | gespiegelte Kopie (z. B. `walk_left` aus `walk_right`): alle Frames, Tags, Dauern, verknüpfte Cels und Regionen (Rechtecke mitgespiegelt); das Original bleibt. `keep_layers` (Indizes oder Namen) übernimmt diese Ebenen ungespiegelt, z. B. ein Schwert, das auf seiner Seite bleibt, samt den auf sie beschränkten Regionen und Markern; Regionen aller Ebenen (`origin`) werden mit dem Körper gespiegelt |
| `check_rigid(path, layer, region? \| rect?, frames? \| from/to? \| tag?, reference?, max_shift?, max_pixels?)` | prüft, ob ein starres Teil (Schwert, Schild, Helm) über die Frames pixelgleich bleibt und sich nur verschiebt. Das Teil sind die Pixel der Ebene in `region`/`rect` auf dem Referenzframe (Standard: erster gewählter Frame), ohne Bereich das deckende Rechteck des ganzen Cels (dann zählt jedes weitere Pixel der Ebene als Abweichung). Je Frame: beste Verschiebung `dx`/`dy` (bis `max_shift`, Standard 4), Zahl der abweichenden Pixel (0 = starr) und ihre Koordinaten (erwartet -> gefunden); eine Region, deren Rechteck auf einem Frame wandert, setzt dort den Suchmittelpunkt. Am Ende z. B. `rigid in 7/8 frames; offsets dy: 0 0 0 -1 0 0 0 -1` (relativ zur Referenz). Nur lesend |
| `shift_frames(path, layer, offsets, frames? \| from/to? \| tag?, region? \| rect?, update_region?, edit_linked?)` | verschiebt die Pixel einer Ebene Frame für Frame in einem Schreibvorgang (ein Diff, ein Verlaufseintrag), z. B. das Schwert mit dem Körper-Bob: `offsets` je gewähltem Frame ein `dy` (`[1, 1, 1, 0]`) oder `[dx, dy]`. Ganzes Cel oder nur `region` (ihr Rechteck je Frame) / `rect`, wie `move_region`; über den Leinwandrand geschobene Pixel werden abgeschnitten und gezählt. `update_region` (Standard an): die Region bzw. bei ganzem Cel die auf die Ebene beschränkten Regionen wandern mit (gesperrte bleiben). Ein zwischen gewählten Frames verknüpftes Cel wird bei gleichem Versatz einmal verschoben, bei verschiedenem abgelehnt (erst entknüpfen); Sperren, `dry_run`, `note` wie überall |
| `list_regions`, `set_region(name, kind?, x/y/w/h, frame? \| from/to \| tag, layer?, locked?, color?, hide?, rename?)`, `delete_region` | Regionen und Marker (`kind: "marker"`, nur x/y) |
| `add_layer`, `set_layer(…, opacity?, locked?)`, `delete_layer` | Ebenen; `opacity` 0..1 gilt für Leinwand, Vorschau und alle Exporte |
| `add_frame(copy_from?, linked?, …)`, `delete_frame`, `set_frame_duration`, `set_frame(name?, duration_ms?, locked?, move_to?)`, `link_cel` | Frames und verknüpfte Cels |
| `move_frames(path, frames[], to)` | Frames (Index, Name oder `tag/n`) als Block so verschieben, dass der erste bei `to` der neuen Reihenfolge landet. Cels, Links, Regions-Rechtecke und „KI zeigen“-Markierungen gehen mit, Tags behalten ihre Frames (Regeln oben). Antwort: wer wohin, neue Reihenfolge, Diff; auch auf gesperrten Ebenen erlaubt (kein Pixel eines Frames ändert sich) |
| `set_reference(path, image \| document, x?, y?, scale?, opacity?, above?, visible?, locked?, remove?)` | Referenz setzen, einzelne Felder ändern oder entfernen; Pfade relativ zum Dokument. Prüft, dass die Datei lädt, und nennt ihre Größe. Gesperrt: nur `locked: false` geht. Nur `.pxe.json`. Mit Diff, `dry_run`, `note`, Verlauf |
| `set_tag(name, from/to \| frames, direction?, loop?, color?, delete?)` | Tags (Bereich + Richtung; `frames` in beliebiger Reihenfolge über verknüpfte Frames) |
| `set_palette_color(key, color?, name?, remove?)` | Palette mit Namen |
| `set_ramps(path, ramps[{name, keys}])` | Farbrampen setzen (ersetzt alle, `[]` löscht sie): `keys` von dunkel nach hell als Tastenfolge `"b89a"` oder Liste von Tasten/Farbnamen. Nur in `.pxe.json` (bei `.aseprite` abgelehnt). `get_document` listet sie |
| `shade(path, layer, frame, pixels \| rect \| region, steps, dry_run?, note?)` | entlang der Rampen schattieren: `steps` +n = n Stufen heller, −n = dunkler, am Ende der Rampe bleibt die Farbe. Farben ohne Rampe und Transparenz bleiben, jedes Pixel ändert sich höchstens einmal. Diff, Verlauf, `undo` wie die anderen Schreibwerkzeuge |
| `render_preview(path, frame? \| frames?, layer?, zoom?, grid?, regions?, tiled?, view?, inline?)` | Vorschau ansehen; `tiled: true` zeigt jeden Frame 3×3 wie gekachelt (Datei `…_tiled.png`, Raster und Regionen dann nur auf Wunsch). `view`: `normal` (Standard), `silhouette` (alle deckenden Pixel dunkel auf hell: liest sich die Form?), `grayscale` (nur Helligkeit: tragen die Werte ohne Farbton?), `mirrored` (links-rechts gespiegelt: zeigt Proportionsfehler; Lineale und Regionen nennen weiter die echten Pixel); eine Liste zeigt die Ansichten je Frame nebeneinander. Nur im Vorschaubild, nie in Pixeln oder Exporten |
| `lint(path, frame? \| frames? \| tag?, layer?, region? \| rect?, checks?, max?)` | Handwerks-Prüfung, nur lesend, auf dem Gesamtbild oder einer Ebene: verwaiste Pixel, Doppelte (L-Ecken auf 1-px-Linien), Treppen-Unregelmäßigkeiten (ungleichmäßige Stufen wie 2-1-3) und Farben außerhalb aller Rampen, je mit Koordinaten, gruppiert und gekürzt; erste Zeile fasst zusammen (Regeln unten) |
| `save_part(path, layer, frame, region \| rect, name, library?, overwrite?)`, `place_part(path, layer, frame \| frames, name, x, y, mode?, flip_h?, flip_v?, map?, library?)`, `list_parts(library?)` | Teile-Bibliothek: ein abgenommenes Stück (z. B. das Schwert in einem Winkel) unter einem Namen speichern und später wieder einsetzen statt neu zu zeichnen (siehe unten) |
| `check_seams(path, frame?, layer?, threshold?)` | Nahtstellen eines Tiles prüfen: rechte Spalte gegen linke, unterste Zeile gegen oberste, wie sie beim Kacheln aneinanderstoßen. Je Naht: wie viele Paare sich unterscheiden, welche harte Kanten sind (Farbabstand ≥ `threshold`, Standard 0,25; als Läufe mit beiden Tasten, z. B. `y 9..14: E cliff earth dark \| r stone`) und ob die Naht mehr Kontrast hat als das Innere des Tiles („VISIBLE SEAM“). Nur lesend |
| `render_animation(path, tag, region?, track?, trace?, trace_threshold?, zoom?, jump_threshold?, inline?)` | Animation als Standbild prüfen (siehe unten) |
| `history`, `undo(steps?)`, `snapshot(name, note?)`, `list_snapshots`, `diff_snapshot(name, layer?, frame?)`, `restore_snapshot(name)` | Verlauf und Schnappschüsse |
| `focus(path, frame, layer?, region? \| rect?, note?)`, `open_editor(path)`; `pace_ms` an Schreibwerkzeugen | live zusehen (siehe „KI folgen“) |
| `get_user_view` | was im Editor gerade offen und ausgewählt ist, samt Pixeln der Auswahl und allen „KI zeigen“-Markierungen mit Notizen (siehe „Was du gerade ansiehst“) |
| `get_guide(topic?)` | Pixelart-Leitfäden aus drei Ebenen (siehe „Pixelart-Wissen“): ohne `topic` die Themenliste mit Ebenen, mit `topic` eingebauter Text, dann persönliche und Projekt-Ergänzungen |
| `add_learning(topic, text, layer?, source?, dry_run?)` | bestätigte Erkenntnis datiert anhängen (`- JJJJ-MM-TT: Text (Quelle)`), Ebene `personal` (Standard) oder `project` |
| `hook_status(path?)` | der `on_save`-Befehl aus `.pxe-project.json`: vertraut oder nicht, läuft gerade, letztes Ergebnis (Exit-Code, Dauer, Ausgabe) |
| `clear_marks(path, frame?, numbers?)` | „KI zeigen“-Markierungen entfernen: alle des Dokuments, nur die eines Frames oder bestimmte Nummern |
| `export(path, target, …)` | `png_sheet`, `godot`, `unity`, `gif`, `aseprite`, `godot_tileset` (Tileset als Atlas + TileSet); auf einer Karte `godot` (Atlas, TileSet, Szene) oder `world_txt` |
| `create_tilemap(name, tileset, width, height, fill?)` | neue Karte; nennt alle Tiles des Tilesets |
| `get_tilemap(path, x?, y?, w?, h?)` | Größe, Tileset, Legende (Taste, Name, Tile, Objekt, Anzahl), noch unbenutzte Tiles, Zellen als Tastenzeilen mit Lineal |
| `set_tiles(path, cells? \| rows? + x/y \| rect? + tile, dry_run?, note?)` | Zellen setzen: `cells [{x, y, tile}]`, Zeilen aus Tasten (`-` = Zelle lassen, Leerzeichen = leeren) oder ein Rechteck. `tile` ist Taste, Legenden-Name oder Frame-Name des Tilesets (bekommt dann eine neue Taste). Diff (Anzahl, Rechteck, Zeilen vorher/nachher), Verlauf, `undo` |
| `set_legend(path, key, tile?, object?, name?, remove?)` | Legenden-Eintrag anlegen/ändern/entfernen (nur unbenutzte) |
| `resize_tilemap(path, width, height, fill?)` | Kartengröße; linke obere Ecke bleibt, abgeschnittene Zellen werden gezählt |
| `import_world(src, name, tileset)`, `export_world(path, to)` | world.txt der Sandbox hin und zurück (byte-gleich) |
| `render_preview(path)` auf einer Karte | die ganze Karte mit Objekten, Kachelraster und Zellnummern alle 5 (`preview/<name>_map.png`, Zoom Standard 1) |
| `history`, `undo` auf einer Karte | wie bei Dokumenten (Schnappschüsse gibt es für Karten nicht) |
| `save_as(path, to)`, `import_png(…)` | Umwandeln / PNG übernehmen |

Strukturänderungen (Ebenen, Frames, Tags, Palette, Regionen) haben ebenfalls `dry_run` und `note`, werden protokolliert und antworten mit einer Zusammenfassung der Änderungen.

### Animationen prüfen: `render_animation`

Claude sieht nur Standbilder. `render_animation` macht deshalb aus einem Tag ein Prüfbild und eine Analyse (MCP-Werkzeug und CLI `render-animation`).

- **Filmstreifen** in echter Abspielreihenfolge (Richtung und Ping-Pong ausgeschrieben). Jede Zelle ist beschriftet mit Schritt, Frame-Name, ms und geänderten Pixeln. Bei Loops folgt nach dem letzten Schritt noch einmal Schritt 0 („loop -> 0“), damit Sprünge an der Nahtstelle sichtbar werden.
- Eine **rot gestrichelte Linie** zeigt die unterste deckende Zeile des ersten Schritts (Bodenlinie).
- **Onion-Zeile**: vorheriger Schritt kühl getönt mit etwa 35 %, aktueller warm getönt darüber.
- **Differenz-Zeile**: geänderte Pixel magenta über dem abgedunkelten Bild. Passen die meisten Pixel zum vorherigen Schritt, wenn man ihn um (dx, dy) verschiebt (z. B. das Wippen des Körpers), wird gegen den verschobenen Schritt verglichen. Die Zelle zeigt dann „moved dx,dy chg N“, und es bleibt nur die echte Änderung sichtbar (etwa nur der Fuß, der sich bewegt).
- **`region`** (Name, „x,y,w,h“ oder {x,y,w,h}) zeigt nur diesen Bereich, vergrößert. **`track`** markiert per Schritt das Rechteck der deckenden Pixel im Bereich (cyan) und verbindet die Mittelpunkte (gelb), z. B. Füße, Kopf oder Schwertspitze.
- **Textauswertung**: je Schritt geänderte Pixel, Verschiebung der Silhouette (oben/unten/links/rechts) und die Ausrichtung; dazu die Gesamtdauer, die Track-Mittelpunkte und Warnungen:
  - Bodenlinie ändert sich
  - Silhouette springt um mehr als `jump_threshold` px (Standard 2)
  - isolierte Pixel ohne deckenden Nachbarn
  - neue verwaiste Pixel ohne 4-Nachbarn gleicher Taste (verglichen mit dem vorherigen Schritt an der verschobenen Position)
  - Schritte, die mit dem vorherigen identisch sind
  - Bahnen aus `trace`, die ihren Bogen verlassen (siehe unten)
- **`trace`** verfolgt Punkte, damit Hände, Füße und Kopf auf Bögen laufen: ein Markername (sein Punkt je Schritt), ein anderer Regionsname (oberster deckender Pixel darin) oder `{key, area?}` (oberster Pixel dieser Taste, wahlweise nur in `area`; bei flacher Oberkante die Mitte der Zeile). Die Antwort nennt je Schritt Position und Bewegung (`trace hand: 0:(3,4) | 1:(5,3) +2,-1 …`), im Filmstreifen ist die ganze Bahn gepunktet eingezeichnet (rosa, grün, orange, violett je Spur), der Punkt des Schritts als Ring. Warnungen: ein einzelner Schritt gegen die Bewegung auf einer Achse („goes back and forth“), mehr als zwei Richtungsumkehrungen (aufeinanderfolgende Bewegungen mehr als 90° auseinander; ein Pendel kehrt zweimal je Loop um, ein Bogen oder Kreis nie) und Sprünge über `trace_threshold` px (Standard 3). Stillstand zählt nicht als Bewegung, und Wackler nur aus 1-px-Schritten (Kopfwippen) werden nicht beurteilt: ein Pixel ist der kleinste Schritt.

Das PNG landet in `preview/<name>_<tag>_anim.png` und wird direkt als Bild zurückgegeben.

### Handwerk prüfen: `lint`

`lint` sucht typische Pixelart-Fehler und nennt sie mit Koordinaten (je Art höchstens `max`, Standard 10). Auswahl über `checks`: `orphans`, `doubles`, `jaggies`, `ramps`.

- **orphans**: Pixel ohne 4-Nachbarn derselben Taste, außer solchen mit einem diagonalen Nachbarn ihrer Taste (Stufen einer Diagonale). Einzelne Lichter (Augen, Glanzpunkte) sind oft Absicht.
- **doubles**: L-Ecken auf 1-px-Linien. Geprüft werden nur „dünne“ Formen: 8-verbundene Flächen einer Taste mit mindestens 4 Pixeln und ohne 2×2-Block, gefüllte Flächen und dicke Striche also nie. Ein Pixel ist doppelt, wenn genau zwei seiner 4-Nachbarn seine Taste haben, diese im rechten Winkel liegen, jeder andere Nachbar derselben Taste einen der beiden berührt (ohne ihn bleibt die Linie verbunden) und nicht beide Arme gerade weiterlaufen (das ist eine echte Ecke, etwa eines Kastens). Eine Treppenstufe hat oft zwei solche Ecken nebeneinander; sie gelten als ein Fund, einer der beiden Pixel kann weg.
- **jaggies**: ungleichmäßige Stufen. Die Linie wird an Abzweigungen in einfache Ketten zerlegt (nach Entfernen eines Pixels je Doppelten) und jede Kette in Abschnitte mit Bewegungen in einem Oktanten. Die Pixelläufe entlang der geraden Richtung sind die Stufen; erster und letzter Lauf sind angeschnitten und zählen nicht, nötig sind mindestens 3 innere Läufe. In Ordnung ist ein Abschnitt, wenn seine inneren Läufe höchstens zwei benachbarte Werte haben (gerade Linie jeder Steigung, z. B. 2-1-2-1 oder 3-3-2), nur wachsen oder nur schrumpfen (Kurve) oder sich von Lauf zu Lauf um höchstens 1 ändern; alles andere (2-1-3, 3-1-3, 1-4-1) wird gemeldet. Grenzen: Linien aus mehreren Tasten (Anti-Aliasing, selektive Kontur) zerfallen in mehrere Linien, kurze Linien werden nicht beurteilt, ein gewollter Knick innerhalb eines Oktanten wird ebenfalls gemeldet.
- **ramps**: Farben, die in keiner Rampe des Dokuments stehen (nur wenn es Rampen hat); Kontur- und Akzentfarben sind das oft mit Absicht.
- Ein Banding-Check fehlt bewusst: eine billige Heuristik (parallele Schattenbänder entlang der Kontur) lieferte auf kleinen Sprites zu viele Fehlalarme.

### Teile-Bibliothek: `save_part`, `place_part`, `list_parts`

Ein abgenommenes Teil (das Schwert in jedem Winkel, eine Hand) wird einmal gespeichert und danach eingesetzt statt neu gezeichnet. `save_part` nimmt die Pixel einer Ebene und eines Frames in `region` oder `rect`; transparente Pixel bleiben transparent. Die Datei `<name>.json` enthält die Tastenzeilen und zu jeder benutzten Taste Farbe und Name. Namen: Kleinbuchstaben, Ziffern, `-`, `_`.

- **Projekt-Bibliothek** (Standard): `<Arbeitsordner>/.pxe-parts/`.
- **Persönliche Bibliothek** (für alle Projekte): `$PXE_PARTS_DIR`, sonst `$XDG_DATA_HOME/pixel-editor/parts`, sonst `~/.local/share/pixel-editor/parts`.

`place_part` setzt die linke obere Ecke auf (`x`, `y`) in einen oder mehrere Frames (`frame` oder `frames`) und ist ein normales Schreibwerkzeug: Sperren verweigern, Diff, `dry_run`, `note`, Verlauf, `undo`. Farben werden über den Wert zugeordnet: dieselbe Taste, wenn sie die Farbe des Teils hat, sonst irgendeine Taste mit dieser Farbe. Fehlt eine Farbe in der Zielpalette, schreibt das Werkzeug nichts und nennt sie (`map: {Teil-Taste: Ziel-Taste}` legt die Zuordnung selbst fest). `mode` `over` (Standard) lässt den Cel unter transparenten Teil-Pixeln stehen, `replace` leert ihn dort; `flip_h`/`flip_v` spiegeln das Teil. Ohne `library` sucht es erst im Projekt, dann persönlich. `list_parts` zeigt Name, Größe, Tasten, Herkunft und die Zeilen.

## Exporte

Sheets enthalten die Frames in Reihenfolge (höchstens 16 pro Reihe, über `columns` einstellbar). Frames mit identischem Bild (z. B. verknüpfte) teilen sich einen Bereich im Sheet. Unsichtbare Ebenen werden nicht exportiert. Ohne Tags entsteht eine Animation `default` über alle Frames. Tag-Richtungen werden in die Frame-Folge ausgeschrieben (Ping-Pong 0 1 2 1, rückwärts 2 1 0).

- **png_sheet**: `<name>.png` + `<name>.json` in der Form der `*_sheet.json` von pixelart-studio.
- **godot**: `<name>.png` + `<name>.tres` (SpriteFrames, Godot-4-Textformat), eine Animation je Tag mit Loop-Flag. Die Zeiten bleiben exakt: `speed` = 1000 / Basisdauer, `duration` je Frame = ms / Basisdauer. Standardmäßig verweist die `.tres` relativ auf die PNG; mit `godot_res_dir` auf einen festen `res://`-Pfad. In Godot für Pixelart die Texturfilterung auf *Nearest* stellen.
- **unity**: `<name>.png` + `.png.meta` (Sprite Mode *Multiple*, Filter *Point*, Kompression *None*, keine Mipmaps, 32 Pixels per Unit) + `<name>.anim.json` mit Sprite-Namen (`<tag>_<n>`) und Zeiten. GUIDs und IDs sind aus den Namen berechnet und bleiben bei erneutem Export gleich.
- **gif**: `<name>_<animation>.gif` je Animation, Standard 4×, echte Transparenz, exakte Farben (bis 255), Dauer auf 10 ms gerundet (GIF-Grenze).
- **aseprite**: `<name>.aseprite` (indiziert, oder `color_mode: "rgba"`) mit Ebenen, Frames, Dauern, verknüpften Cels, Tags (Richtung, Loop, Farbe) und Palette (Tasten als Eintragsnamen). Ebenso direkt über „Speichern unter“ mit Endung `.aseprite`.

### Was geprüft ist und was nicht

- **Godot-Karten**: mit Godot 4.7.2 headless in einem Wegwerf-Projekt mit der importierten Kopie der Sandbox-Welt geprüft: `--import` ohne Fehler, Szene lädt, `TileMapLayer` mit 1288 Zellen, Atlas-Koordinaten und Tile-Namen (`get_custom_data("name")`: grass, water0, path an den erwarteten Zellen), 40 Bäume als `Sprite2D`.
- **Godot**: mit Godot 4.7.2 headless in einem Wegwerf-Projekt geprüft. Import ohne Fehler, `load()` der `.tres` mit relativem und mit `res://`-Pfad, Regionen, Loop-Flags, Ping-Pong-Folge, Millisekunden je Frame, Pixelfarben. Godot 4.6 selbst ist nicht installiert; das Format (`format=3`) ist dasselbe.
- **Aseprite**: Aseprite ist nicht installiert. Geprüft wurden Hin- und Rückweg mit dem eigenen Leser (Pixel identisch, samt Ebenensperre, Palettennamen und Regionen ↔ Slices) und das Lesen der Exporte mit einem unabhängigen Parser, dem Rust-Crate `asefile` (MIT, nur Test-Abhängigkeit). Für indizierte und RGBA-Dateien stimmen dort Größe, Ebenen, Sichtbarkeit, Dauern, Tags, aufgelöste Link-Cels und exakte Pixelfarben. `asefile` 0.3 kennt die Richtung „Ping-Pong rückwärts“ (Aseprite 1.3) noch nicht; das ist dort nicht geprüft. Echte, mit Aseprite erzeugte Dateien lagen nicht vor. Der Import ist nur gegen eigene Dateien und die Beschreibung getestet.
- **Unity**: **nicht geprüft**, Unity ist nicht installiert. Die `.meta` folgt dem Aufbau von Unity 2022/6000.
- **GIF / PNG-Sheet**: Dauern, Transparenz, Farben und JSON-Aufbau geprüft.
- **GUI**: per egui_kittest offscreen geprüft, mit echten Maus- und Tastaturereignissen. Getestet sind:
  - Stift, Rückgängig/Wiederholen, Rechteck, Füllen, Aseprite-Kürzel
  - verknüpfter Frame, Wiedergabe, Live-Neuladen samt Journal-Beschreibung, Öffnen/Speichern von `.aseprite`
  - Pixelgenauigkeit der Leinwand
  - Namen in der Statuszeile und im Verlauf, Sperren (Ebene, gesperrte Region)
  - Auswahl verschieben/Pfeiltasten/Kopieren/Einfügen in anderen Frame, Region aus Auswahl, Tag als Folge
  - Werkzeugleiste: Gruppenknöpfe, Aufklappen per Rechtsklick, Pfeil und langem Drücken, Gruppe merkt sich das letzte Werkzeug, Untermenüs im Menü Werkzeuge; Bild in `target/toolbar-screenshot.png`
  - Strg+A, Strg+D, Strg+Umschalt+I (hin und zurück), Kontextmenü der Leinwand und Menü Auswahl, „Hier hinten ablegen“
  - Pixel-Auswahl: Zauberstab (auch „alle gleichfarbigen“), Auswahl-Pinsel (Ziehen mit rechts, Alt, Strg nehmen heraus; Rechtsklick öffnet das Menü), Rechteck und Lasso mit Umschalt/Alt/Strg, Umriss entlang der Maske, Verschieben einer geformten Auswahl (Pixel außerhalb bleiben), Ablegen vorne und hinten samt Vorschau, Pfeile und Esc, Kopieren/Einfügen/Ausschneiden/Entf mit Maske; `get_user_view` mit `·` für nicht gewählte Pixel
  - Tabs: öffnen, wechseln (Strg+Tab, Klick), schließen mit Rückfrage, eigener Zustand je Tab, Markierung bei Änderung im Hintergrund ohne Tab-Wechsel, `focus` wechselt den Tab, Öffnen aus einem anderen Prozess über den Socket (`open`/`open_editor`), Wiederherstellen; Bild der Tab-Leiste in `target/tabs-screenshot.png`
  - Zeitleiste scrubben (Ziehen über die Nummern und Cels, Pause, Klemmen am Rand, Klick wählt weiter), Mausrad über den Nummern
  - Kachel-Modus: Stift über den Rand, Klick in eine Kopie, Rechteck über den unteren Rand, Füllen um den Rand herum, Symmetrie mit Wrap, Rechteck-Auswahl über den Rand, Alt+T, Merken je Dokument und Wiederherstellen beim Öffnen, Start im Modus bei Tile-Namen, Vorlage Tile 32×32, Menü Ansicht, gekachelte Vorschau; Bild mit einer Kopie von `tiles32` (Frame `path`, ein Strich über den rechten Rand) in `target/tiled-screenshot.png`
  - Karten: Öffnen als Karten-Tab, Legenden-Eintrag per Klick als Pinsel, Stift mit neuem Tile (Taste entsteht), Strich über mehrere Zellen, Rechtsklick entfernt, Rechteck (U), Füllen (G), Pipette (I), Strg+Z/Strg+Umschalt+Z, Kartengröße, Strg+S mit Journal; Tileset per „Tileset im Tab öffnen“, Zeichnen dort ändert die Karte live (ungespeichert), MCP-`set_tiles` wird nachgeladen (Notiz, Rückgängig); Bild mit Kopien von `tiles32`/`tree64` und der importierten Kopie von `world.txt` in `target/tilemap-screenshot.png`, `render_preview` der ganzen Welt in `target/tilemap-world.png`
  - Farbe ersetzen: Dialog aus dem Menü Bild, Vorschau-Zahl vor dem Schreiben, Cel, Tag mit und ohne „nur Ebene“, gesperrter Frame übersprungen, Masken-Auswahl, ein Rückgängig-Schritt; Bild in `target/replace-screenshot.png`. Der Eintrag im Rechtsklick-Menü der Palette ist nur über `open_replace` geprüft (kittest findet die Farbfelder nicht per Name)
  - Dither: Füllen (Schachbrett, Leinwand-ausgerichtet), Rechteck gefüllt mit 25 % und 2. Farbe per „Rampe dunkler“, Rechteck-Umriss ohne Muster, Masken-Auswahl, Symmetrie, Kachel-Modus (Klick in eine Kopie); Bild aller Muster im Kachel-Modus in `target/dither-screenshot.png`
  - Kontur: außen 4er/8er mit Vorschau-Zahl, innen auf allen Ebenen eines Frames; Vergleich vorher/nachher an einer Kopie von `slime/idle` (Auswahl der rechten Hälfte, außen, 8er) in `target/outline-compare.png`
  - Frames: Klick, Umschalt+Klick, Strg+Klick (auch auf Vorschaubildern), Ziehen eines Vorschaubilds verschiebt drei verstreute Frames als Block (Linie an der richtigen Stelle, Pixel, Link, Regionen, Tags, Markierung in `marks.json`, ein Rückgängig-Schritt, Markierung wandert zurück), Alt+Ziehen auf einer Nummer, Ziehen ohne Alt scrubbt weiter und verschiebt nichts, Dauer/Duplizieren/`>`/Alt+C auf der Auswahl; Bild in `target/frames-drag-screenshot.png`
  - Referenz: Laden im Fenster, halbtransparent unter den Pixeln (Bildschirmpixel geprüft), ausblenden, Verschieben per Ziehen in ganzen Pixeln mit einem Rückgängig-Schritt, ohne ein Pixel zu malen, Sperre schaltet Verschieben ab, über den Pixeln, Speichern im Dokument; Bild einer Kopie von `toast_warrior/walk_side` mit `walk_up` (2×, angezeigt mit ×0,5) halbtransparent darüber in `target/reference-screenshot.png`
  - Formvorschau auf einer Ebene mit Deckkraft 0,5: sieht beim Ziehen genau aus wie das Ergebnis
  - KI-Markierungen: mehrere per Ziehen und Klick, Notiz tippen und mit Enter speichern, Klick daneben schließt nur das Notizfeld, Klick hinein wählt aus und ändert die Notiz, verschieben, Größe an Kante und Ecke, Rechtsklick entfernt eine, Menü „dieses Frames“, Statuszeile „alle“, andere Frames nicht auf der Leinwand aber in der Zeitleiste, eigene Markierungen je Tab, Strg+Umschalt+K auch mit Fokus in einem Textfeld, Übernahme einer alten `pointer.json`, Neuladen nach `clear_marks`; Bild in `target/marks-screenshot.png`. Am echten Bildschirm wurde die Oberfläche bisher nicht bedient.

Zum Prüfen der GUI ohne Bildschirm: `cargo test screenshot` rendert offscreen nach `target/gui-screenshot.png`. Mit sichtbarem Fenster: `PIXEL_EDITOR_SCREENSHOT=/pfad/bild.png pixel-editor datei.pxe.json` speichert nach 2 s (`PIXEL_EDITOR_SCREENSHOT_DELAY`) ein Bild des eigenen Fensters und beendet das Programm. Einige Screenshot-Tests nehmen echte Grafik, wenn `PIXEL_EDITOR_TEST_ART` auf einen Art-Ordner zeigt (mit `tiles32.pxe.json`, `toast_warrior/`, `slime/`) und `PIXEL_EDITOR_TEST_WORLD` auf eine `world.txt`; ohne die Variablen zeichnen sie einen Ersatz.

## Für Claude: Arbeitsweise

1. `create_document` in den Maßen und Ordnern des Projekts (üblich: ein Dokument je Animation, ein Ordner je Figur), Farben und Ebenen benennen, Teile als Regionen anlegen (`set_region`). Die Vorgaben eines Projekts (Leinwandgrößen, Ordner, Marker wie `origin`, Viewport) stehen in `.pxe-instructions.md` im Arbeitsordner: Der MCP-Server liest die Datei beim Start und hängt sie unter „Project instructions“ an seine Anweisungen an. Sie ist freies Markdown, fehlt sie, gelten nur die eingebauten, neutralen Anweisungen; über 16 KB wird sie mit Hinweis abgeschnitten. Änderungen wirken nach einem Neustart des Servers.
2. Flächen mit `set_rows`, Formen mit `draw_*`/`fill`, Details mit `set_pixels`, immer mit Ziel (Ebene, Frame, Region).
3. Den zurückgegebenen Diff prüfen, `render_preview` ansehen, mit `get_rows` Details prüfen. Bei Korrekturwünschen nur die genannte Region ändern; vor größeren Umbauten `snapshot`. Bei Tiles zusätzlich `render_preview` mit `tiled: true` und `check_seams`: das Tile muss an seinen eigenen Rändern weiterlaufen.
4. Animation: `add_frame(copy_from)` + kleine Änderungen. Bezieht sich der Nutzer auf „das hier“, erst `get_user_view` aufrufen. Wiederkehrende Frames mit `linked: true` anlegen, dann `set_frame_duration` und `set_tag` (Bereich + Richtung).
5. `export` (für das Testspiel `godot`), für Aseprite-Nutzer `aseprite`.
6. Umgebungen: Karte mit `create_tilemap` oder `import_world`, ansehen mit `get_tilemap` und `render_preview`, ändern mit `set_tiles` (immer den Diff prüfen). Tiles selbst bearbeitet man im Tileset-Dokument; jede Karte zeigt die Änderung sofort.

## Pixelart-Wissen: eingebaut, persönlich, Projekt

Der Editor bringt Handwerksregeln für Pixelart mit, damit eine KI sie vor dem Zeichnen liest (`get_guide`). Drei Ebenen, beim Lesen zusammengeführt; bei Widerspruch gilt Projekt vor persönlich vor eingebaut:

1. **Eingebaut**: `knowledge/*.md` im Repo (englisch), per `include_str!` im Programm, zur Laufzeit nur lesbar. Themen: `workflow` (Arbeitsweise einer KI: künstlerische Rolle, vor dem Zeichnen, erst Pose dann Animation, Sichtkontrolle, sichere Entwürfe), `fundamentals` (Linien, Cluster, Banding, Licht, Rampen, Konturen, AA/Dither, Lesbarkeit), `animation-principles` (die 12 Prinzipien in Pixeln), `pixel-animation` (Walk, Timing, Bob, Ausrüstung, Spiegeln, Subpixel, Idle, Smear, Flimmern), `checklist` (vor der Abgabe). Kennzeichen `[consensus]`/`[style]`/`[practice]` und Quellen-URLs stehen dabei.
2. **Persönlich**: gehört zur Installation, nicht zu einem Projekt: `$XDG_DATA_HOME/pixel-editor/knowledge/<thema>.md` (sonst `~/.local/share/pixel-editor/knowledge/`), umlenkbar nur mit `PXE_KNOWLEDGE_DIR`.
3. **Projekt**: `<arbeitsordner>/.pxe-knowledge/<thema>.md`, dazu wie bisher `.pxe-instructions.md` (hängt an den Server-Anweisungen).

`get_guide` ohne Thema listet alle Themen mit ihren Ebenen und der ersten Überschrift, mit Thema kommen die Texte nacheinander unter je einer Überschrift mit Ebene und Datei. `add_learning` hängt eine datierte Zeile an die Datei der gewählten Ebene an (legt Ordner und Datei mit `# <thema>` an; Themennamen aus Kleinbuchstaben, Ziffern und `-`, neue Themen erlaubt) und schreibt nie die eingebaute Ebene. Gedacht nur für Erkenntnisse, die der Nutzer bestätigt hat: Projekt-Konventionen in die Projekt-Ebene, eigene projektübergreifende Vorlieben in die persönliche. Beide Dateien sind normales Markdown und dürfen von Hand bearbeitet werden; Änderungen wirken sofort, ohne Neustart.

## Nach dem Speichern automatisch exportieren

Liegt im Arbeitsordner eine `.pxe-project.json` mit `on_save`, führt der Editor diesen Befehl nach jedem Speichern im Hintergrund aus, z. B. den Export des Projekts in eine Engine:

```json
{"on_save": ["python3", "../tools/export.py", "--only", "{file}"], "label": "Export"}
```

`{file}` ist das gespeicherte Dokument relativ zum Arbeitsordner, dazu gibt es `{abs}` und `{workdir}`. Gilt für Strg+S in der GUI und für jedes MCP-Werkzeug, das ein Dokument schreibt; eine Serie von Schreibvorgängen gibt nach 300 ms Ruhe einen Lauf. Beim ersten Mal fragt das Fenster „Diesem Ordner vertrauen?“, denn ein fremdes Repo soll nicht schon beim Öffnen Befehle ausführen. Die Zustimmung gilt für genau diesen Befehl und liegt außerhalb des Ordners (`~/.config/pixel-editor/trusted.json`); der MCP-Server läuft nur in vertrauten Ordnern. Die Statusleiste zeigt „Export läuft … / ok (0,4 s) / Fehler“, ein Klick die letzte Ausgabe; per MCP liefert `hook_status` dasselbe. Einzelheiten in [FORMAT.md](FORMAT.md#projekt-datei-pxe-projectjson).

## Bekannte Grenzen

- Drehen nur in 90°-Schritten (keine freien Winkel, kein Skalieren). Symmetrie wirkt nicht auf Auswahl-Werkzeuge und Pipette.
- Kachel-Modus: Schwebende (verschobene oder eingefügte) Pixel wrappen nicht, sie werden am Bildrand abgeschnitten wie ohne Kachel-Modus. „KI zeigen“-Markierungen, Regionen und der Auswahlumriss erscheinen nur auf dem echten Bild in der Mitte.
- Karten: eine Ebene (wie world.txt); Objekte nur über Legenden-Einträge, nicht frei platziert; Objekte zeigen Frame 0 (keine Animation in der Karte, auch nicht `water0..7`). Keine Auswahl/Kopieren, keine Schnappschüsse, kein „KI zeigen“ und kein `get_user_view` für Karten-Tabs. Legenden-Namen ändert man per MCP `set_legend` oder in der Datei, nicht in der GUI. Autotiling (Ränder, Übergänge) macht der Editor nicht, das bleibt beim Spiel.
- Zeitleiste: Ebenen werden per Knopf verschoben, nicht per Ziehen (Frames per Ziehen, siehe oben). Die Ebenennamen-Spalte scrollt horizontal mit. Die Frame-Auswahl ist eine Auswahl der Zeitleiste: Pixel-Werkzeuge, Spiegeln/Drehen „Frames“ und Farbe ersetzen nutzen sie nicht (dort gibt es eigene Bereiche); Rückgängig hebt sie auf.
- Referenz: eine je Dokument; nicht drehbar; ein Dokument als Referenz zeigt immer den Frame mit derselben Nummer (kein fester Frame wählbar); sie wird auf die Leinwand beschnitten und im Kachel-Modus nicht in den Kopien gezeigt. `render_preview` zeigt sie nicht (Absicht: Vorschauen zeigen nur, was exportiert wird).
- Keine Ebenengruppen (Begründung oben), Blend-Modi, Cel-Deckkraft; Ebenen-Deckkraft gibt es und sie gilt überall, im GIF aber nur mit 1-Bit-Transparenz; Aseprite-Tilemap-Ebenen werden beim Import übersprungen und gemeldet (Karten sind ein eigenes Format, siehe oben). Frame-Namen und -Sperren gehen beim Umweg über `.aseprite` verloren (Aseprite kennt sie nicht).
- Tags sind Bereiche wie in Aseprite. Eine beliebige Reihenfolge entsteht über angehängte verknüpfte Frames (GUI „Folge“, MCP `set_tag frames`).
- Palette: Tasten lassen sich in der GUI nicht umbenennen oder umsortieren (Farbnamen schon).
- Farbrampen werden nur in `.pxe.json` gespeichert; `.aseprite` hat keinen Platz dafür (die GUI weist beim Speichern darauf hin, MCP `save_as` ebenfalls). Rampen-Farben verschiebt man per Knopf, nicht per Ziehen. Schattieren nutzt kein Pixel-Perfect (es soll jedes überstrichene Pixel treffen).
- Dither gibt es beim Füllen und bei gefüllten Formen, nicht als Stift-Muster; die Muster sind fest (kein eigener Musterentwurf). Die Dither-Einstellung wird nicht in `settings.json` gemerkt.
- Kein direkter Import von `.pxs` (Umweg: `pxa build` → PNG-Sheet → `import_png` mit Tile-Größe).
- Fehlermeldungen des Kerns (auch in der GUI-Statuszeile) sind englisch, weil sie mit dem MCP-Server geteilt werden.

## Lizenz

Lizenziert nach deiner Wahl unter

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE) oder <http://www.apache.org/licenses/LICENSE-2.0>)
- MIT-Lizenz ([LICENSE-MIT](LICENSE-MIT) oder <http://opensource.org/licenses/MIT>)

### Beiträge

Sofern du nicht ausdrücklich etwas anderes angibst, steht jeder Beitrag, den du absichtlich zur Aufnahme in dieses Werk einreichst, wie in der Apache-2.0-Lizenz definiert, unter der obigen doppelten Lizenz, ohne zusätzliche Bedingungen.
