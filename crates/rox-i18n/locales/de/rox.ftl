### Deutsch. Spiegelt en-CA/rox.ftl Schlüssel für Schlüssel; der
### Paritätstest in rox-i18n wacht darüber.

## Shared widgets

tracking-title = Verfolgung
tracking-follow = Wiedergabe folgen
tracking-resume = Bei Untätigkeit zurückkehren
tracking-smooth = Sanftes Scrollen
align-row = Ausrichtung
    .description = Wo der Inhalt sitzt, wenn das Panel Platz übrig hat
valign-row = Vertikale Ausrichtung
    .description = Wo der Inhalt sitzt, wenn das Panel Höhe übrig hat
valign-top = Oben
valign-middle = Mitte
valign-bottom = Unten

## Panel source and search rows

source-track = Titel
    .description = Folge dem, was läuft, oder dem, was in der Bibliothek ausgewählt ist
source-follow-playing = Wiedergabe folgen
source-follow-selection = Auswahl folgen
source-playing = Wiedergabe
source-selected = Auswahl
query-search = Suche
query-search-box = Suchfeld
    .description = Das Suchfeld anzeigen; die Suchanfrage gilt nur, solange es sichtbar ist
query-source = Suchquelle
    .description = Der gemeinsamen Suchanfrage folgen, nach dem eigenen Feld dieses Panels filtern oder zeigen, was ein anderes Panel ausgewählt hat
query-source-shared = Gemeinsam
query-source-own = Eigen
query-source-selection = Auswahl

## Signals and routes

signal-source = Quelle
    .description = Worauf das Signal hört: Band folgt einem Frequenzbereich, Pegel der ganzen Mischung, Onset pulst bei jedem Schlag im Bereich, Trigger löst einen Impuls aus, wenn der Bereich seinen Schwellwert erreicht, Summe zählt ein anderes Signal über die Zeit zusammen
signal-kind-band = Band
signal-kind-level = Pegel
signal-kind-onset = Onset
signal-kind-trigger = Trigger
signal-kind-total = Summe
signal-response = Ansprechen
signal-response-pulse = Wie lange jeder Impuls nachklingt, bevor er abklingt
signal-response-drift = 0 folgt der Musik sofort, 100 zieht ihr nach
signal-threshold = Schwellwert
signal-threshold-trigger = Der Pegel, den der Bereich erreichen muss, um den Impuls auszulösen; er feuert erst wieder, wenn der Pegel unter die Marke im Meter darüber fällt
signal-threshold-gate = Darunter liest sich das Signal als nichts, darüber klettert die Ausgabe wieder von null, sodass die leisen Stellen den Regler in Ruhe lassen; die Marke im Meter darüber zeigt, wo er sitzt
signal-low-bound = Untere Grenze
signal-high-bound = Obere Grenze
signal-adds-up = Zählt zusammen
    .description = Welches Signal hier summiert wird; es klettert, solange jenes hoch liest, und stockt, solange es leise ist
signal-aggregate-nothing = Nichts zu folgen
signal-aggregate-pick = Signal wählen
signal-aggregate-alone = Es gibt kein anderes Signal im Pool, das hier summiert werden könnte, also bleibt es bei null. Füge eines hinzu, und es taucht in der Liste auf.
signal-aggregate-unpicked = Nichts gewählt, also bleibt diese Summe bei null. Wähle oben ein Signal.
signal-rate = Rate
    .description = Umläufe pro Sekunde bei vollem Eingang; nach 1 springt es zurück auf 0 und klettert weiter, was ein Shader als Phase liest
signal-reset-on-track = Bei Titelwechsel zurücksetzen
    .description = Auf null zurückfahren, wenn ein neues Lied beginnt, damit eine Phase die Summe des letzten nicht mitnimmt
signal-flush = Leeren
    .description = Jetzt auf null zurücksetzen; es fährt über einen Moment herunter statt zu springen, damit nichts, was darauf reitet, hüpft
route-header = Route
route-signal = Signal
    .description = Welches gemeinsame Signal diese Route reitet; hier stimmen stimmt jede Route darauf
route-new-signal = Neues Signal
route-shared-note = Von jeder Route auf diesem Signal geteilt
route-signal-gone = Das Signal dieser Route ist weg; der Regler hält seinen Schiebewert, bis oben ein anderes gewählt wird.
route-range-note = Bereich nur für diesen Parameter
route-quiet = Leise
    .description = Was der Regler bei Stille erreicht, als Anteil seiner eigenen Einstellung
route-loud = Laut
    .description = Was er bei vollem Signal erreicht; 100% ist der eigene Wert des Schiebers, unter Leise moduliert nach unten
route-slot = Slot
    .description = Welchen der sechzehn Signal-Slots des Shaders diese Route füllt
route-slot-quiet-description = Was der Slot bei Stille liest
route-slot-loud-description = Was er bei vollem Signal liest; unter Leise läuft der Slot rückwärts
route-slot-signal-description = Welches gemeinsame Signal diese Route reitet
route-slot-signal-gone = Das Signal dieser Route ist weg; der Slot liest null, bis ein anderes gewählt wird.
route-add = Route hinzufügen
route-unrouted = Ohne Route
route-pick-slot = Slot wählen
route-pick-signal = Signal wählen
route-no-signal = kein Signal
route-no-signals-yet = Es gibt noch keine Signale zum Reiten. Erstelle eines, und es taucht hier auf; bis dahin liest der Slot null.
route-open-signals = Signale öffnen
route-create-signal = Neues Signal erstellen

## Panel settings window

panel-settings = Panel-Einstellungen
panel-menu-label = Panel
panel-save-as-preset = Als Vorlage speichern
panel-rename = Umbenennen
panel-rename-name = Name
panel-rename-note = Wird als Reiter des Panels angezeigt; leer heißt zurück zum eingebauten Namen
panel-rename-hint-after = zum Umbenennen
panel-was-closed = Das Panel wurde geschlossen
panel-reset = Zurücksetzen
panel-inverse = Invertieren
panel-apply-song-theme = Songfarben übernehmen
panel-page-appearance = Aussehen
panel-page-behavior = Verhalten
panel-page-shader = Shader
panel-section-placement = Platzierung
panel-section-size = Größe
panel-section-opacity = Deckkraft
panel-section-frame = Rahmen
panel-section-colors = Farben
panel-section-font = Schrift
panel-section-shader = Shader
panel-section-signals = Signale
panel-section-slots = Slots
panel-awaiting-approval = Wartet auf Freigabe
panel-size-off = Aus
panel-locked = Gesperrt
    .description = Das Panel an Ort und Stelle festhalten; das Dock lässt es nicht ziehen oder umsortieren
panel-drag-anchor = Ziehanker
    .description = Ein Ziehen irgendwo auf dem Panel bewegt das Fenster, während einfache Klicks weiterhin auf seinen Steuerelementen landen; für Layouts ohne Fensterrahmen
panel-slot-controls = Slot-Steuerung
    .description = Die Ecktasten zum Tauschen und Entfernen der hier untergebrachten Panels anzeigen. Ausgeblendet wird das Layout weiterhin über den Baum auf der Arbeitsflächen-Seite in den Einstellungen bearbeitet
panel-min-width = Mindestbreite
    .description = Wo eine Größenänderung aufhört, das Panel schmaler zu quetschen. Wird so genommen, wie es dasteht, auch unter der eigenen Untergrenze des Panels, sodass ein kompakter Streifen enger gehen kann als vorgesehen; leer lässt die Untergrenze in Ruhe
panel-max-width = Maximalbreite
    .description = Die Breite des Panels deckeln, damit es sich nicht dehnt, wenn das Fenster breiter wird
panel-min-height = Mindesthöhe
    .description = Wo eine Größenänderung aufhört, das Panel kürzer zu quetschen. Wird so genommen, wie es dasteht, auch unter der eigenen Untergrenze des Panels, sodass ein kompakter Streifen enger gehen kann als vorgesehen; leer lässt die Untergrenze in Ruhe
panel-max-height = Maximalhöhe
    .description = Die Höhe des Panels deckeln, damit es sich nicht dehnt, wenn das Fenster höher wird
panel-own-opacity = Eigene Flächendeckkraft
    .description = Diesem Panel eine eigene Deckkraft über dem Hintergrund geben statt der der App
panel-surface-opacity = Flächendeckkraft
panel-margin = Außenabstand
    .description = Das Panel aus seiner Zelle hereinziehen, wobei der Hintergrund durch die Lücke scheint
panel-padding = Innenabstand
    .description = Platz innerhalb der Panel-Kante, im eigenen Hintergrund gehalten
panel-rounding = Rundung
    .description = Die Ecken des Panels in den Hintergrund abrunden
panel-border = Rahmen
    .description = Eine Linie um die Kante des Panels, in der Farbe der Rolle Rahmen; eine Seite auf null zeichnet keine
panel-font = Schrift
    .description = Die Schriftart des Panels; Standard folgt der App-Schrift
panel-font-size = Schriftgröße
    .description = Die Textgröße des Panels relativ zur App-Schrift; Zeilen skalieren mit
panel-surface-shader = Flächen-Shader
    .description = Einen WGSL-Shader über den Körper dieses Panels laufen lassen, unter dem Bildschirm-Shader der App
panel-run-when-idle = Bei Stille weiterlaufen
    .description = Weiter Bilder zeichnen, solange der Ton still ist. Aus parkt der Shader, wo er steht, und das Panel kostet nichts
panel-shader-is-scene = Dieser Shader ist eine Szene, also deckt er den Körper des Panels ab, statt darüber zu zeichnen. Er kam aus einem Bundle oder einer älteren Konfiguration; die Liste oben bietet nur Shader an, die das Panel lesbar lassen.

## Shader picker and saving

shader-source = Quelle
shader-pick-none = Keiner
shader-reload = Neu laden
shader-edit-as-file = Als Datei bearbeiten
shader-make-private-copy = Private Kopie anlegen
shader-save-replace = Ersetzen
shader-save-to-workspace = In Arbeitsfläche speichern
shader-save-replaces = Ersetzt den Shader, den diese Arbeitsfläche bereits { $name } nennt. Jedes Panel, das diesen Namen nutzt, ändert sich mit
shader-save-adds = Fügt ihn den Shadern dieser Arbeitsfläche unter { $name } hinzu. Jedes Panel kann ihn nutzen, und ein Bearbeiten wirkt auf alle
shader-group-examples = Beispiele
shader-group-this-workspace = Diese Arbeitsfläche
shader-group-scenes = Szenen
shader-group-workspace-scenes = Szenen der Arbeitsfläche
shader-group-overlays = Overlays
shader-group-workspace-overlays = Overlays der Arbeitsfläche

## Saving a panel preset

preset-save = Vorlage speichern
preset-save-name = Vorlagenname
preset-save-replaces = Ersetzt die Vorlage, die diese Arbeitsfläche bereits { $name } nennt
preset-save-hint-after = zum Speichern
preset-back-from = Hol es zurück über
preset-back-add-panel = Panel hinzufügen
preset-back-then = dann
preset-back-presets = Vorlagen
preset-back-tail = in jedem Panel-Menü. Vorlagen gelten nur für diese Arbeitsfläche, eine andere trägt sie also nicht mit.

## Keyboard hints

hint-press = Drücke
hint-key-enter = Enter

## Settings: language

settings-language = Sprache
    .description = Die Sprache der Oberfläche; System verhandelt gegen die Liste des Betriebssystems und landet bei Englisch, wenn nichts passt
settings-language-system = (Systemsprache)
settings-language-search = Sprachen durchsuchen
picker-no-matches = Keine Treffer

## Embed dialog

bake-window-title = rox - Gespeicherte Metadaten einbetten
bake-title = Gespeicherte Metadaten einbetten
bake-intro = Schreibt, was rox bereits vorliegt, in die Dateien selbst, damit auch ein anderer Player es liest. Nichts wird neu berechnet.
bake-formats = Nur MP3 und FLAC; andere Formate und CUE-Titel werden übersprungen
bake-source-lyrics = Songtexte
bake-source-gain = ReplayGain
bake-source-acoustic = Akustische Beschreibungen
bake-detail-nothing = nichts Gespeichertes zum Einbetten
bake-detail-only-skipped = nichts zu schreiben, { $skipped } übersprungen
bake-detail-writes = { $count ->
    [one] { $count } Datei zu schreiben
   *[other] { $count } Dateien zu schreiben
}
bake-detail-writes-skipped = { $count ->
    [one] { $count } Datei zu schreiben, { $skipped } übersprungen
   *[other] { $count } Dateien zu schreiben, { $skipped } übersprungen
}
bake-error-read = Die Bibliothek konnte nicht gelesen werden: { $error }
bake-survey-counting = Durchsuche die Bibliothek...
bake-survey-progress = Lese Tags, { $done } von { $total }
bake-nothing-to-embed = Nichts einzubetten: die Dateien tragen bereits alles, was rox vorliegt
bake-rewrites = { $count ->
    [one] { $count } Datei wird neu geschrieben
   *[other] { $count } Dateien werden neu geschrieben
}
bake-hint-before = Zum Einbetten
bake-hint-key = Enter
bake-hint-after = drücken
bake-embed = Einbetten
bake-cancel = Abbrechen

## Arrange editors and header pieces

arrange-shown = Sichtbar
arrange-hidden = Ausgeblendet
tile-face-mosaic = Cover-Mosaik
tile-face-tinted = Getöntes Mosaik
tile-face-gradient = Verlaufskarte
tile-face-color = Farbkarte
head-piece-artist = Interpret
head-piece-album = Album
head-piece-year = Jahr
head-piece-genre = Genre
head-piece-quality = Qualität
head-piece-tracks = Titel
head-piece-time = Dauer
head-piece-spacer = Abstand
head-piece-divider = Trenner
head-piece-art = Cover
head-unknown = Unbekannt
status-item-count = Anzahl
status-item-time = Dauer
status-item-albums = Alben
status-item-artists = Interpreten
status-item-plays = Wiedergaben
volume-item-icon = Symbol
volume-item-slider = Schieber
volume-item-percent = Prozent

## Filter chips and search menus

filter-field-artist = Interpret
filter-field-album-artist = Album-Interpret
filter-field-album = Album
filter-field-genre = Genre
filter-field-year = Jahr
filter-field-folder = Ordner
filter-unknown = Unbekannt
filter-clear = Leeren
query-show-search-box = Suchfeld anzeigen
query-own-query = Eigene Suche
query-shared-query = Gemeinsame Suche
headers-off = Aus
headers-compact = Kompakt
headers-expanded = Ausgeklappt

## Panel context menu

panel-dock-back = Zurück andocken
panel-pop-out = Herauslösen
panel-close = Schließen
panel-duplicate = Duplizieren
panel-reveal-in-browser = Im Dateimanager zeigen
panel-play-next = Als Nächstes spielen
panel-add-to-queue = Zur Warteschlange
panel-add-to-playlist = Zur Playlist hinzufügen
shader-pick-missing = { $name } (fehlt)
shader-pick-custom = Eigen

## Shipped shader examples

shader-blurb-plasma = Treibende Farbe allein aus den eigenen Uniforms, also kostet es nur ein schlichtes Quad.
shader-blurb-trails = Verschmiert das eigene letzte Bild, was ihn auf den Bildschirmdurchgang legt.
shader-blurb-sheen = Eine Vignette und ein wanderndes Glänzen, transparentes Overlay für ein Panel, das schon zeichnet.
shader-blurb-shadow = Ein Schlagschatten, den Text und Steuerelemente des Panels werfen, von der Maskenaufnahme gelesen.
shader-blurb-cover = Das Cover des laufenden Titels, im Letterbox über einem Waschgang seiner eigenen Farbe.
shader-blurb-badge = Das Cover als kleine Karte in einer Ecke, mit einem Slot, um sie herumzuschieben.
shader-blurb-lamp = Ein Licht, das dem Zeiger folgt und auf die Tasten reagiert, transparentes Overlay.
shader-blurb-cube = Ein Drahtgitterwürfel, der in falschem 3D taumelt, als zugesetztes Licht gezeichnet.
shader-blurb-bloom = Treibende Kugeln, durch einen halb so großen zweiten Durchgang gebloomt, die Kette im Kleinen.
shader-blurb-tube = Spielt das Panel darunter über eine gewölbte Röhrenfront ab, samt Zeilen.

## Transport strip pieces

seek-item-elapsed = Verstrichen
seek-item-strip = Leiste
seek-item-ending = Restzeit
seek-item-duration = Dauer
info-item-track-no = Titelnr.
info-item-title = Titel
info-item-duration = Dauer
info-item-next = Als Nächstes
info-item-queued = In Warteschlange
info-item-output = Ausgabe
info-item-favourite = Favorit
info-item-rating = Bewertung
playback-item-previous = Zurück
playback-item-seek-back = Zurückspulen
playback-item-play = Wiedergabe
playback-item-seek-forward = Vorspulen
playback-item-next = Weiter
playback-item-stop = Stopp
playback-item-volume = Lautstärke
playback-item-loop = Wiederholen
playback-item-shuffle = Zufall
playback-item-continue = Fortsetzen
playback-item-crossfade = Überblenden
playback-item-random = Zufällig
playback-item-stop-after = Stopp danach
playback-item-favourite = Favorit
playback-item-rating = Bewertung

## Dock chrome

dock-empty-tab = Leerer Reiter
dock-unnamed = Unbenannt
dock-tiles = Kacheln
dock-zoom-in = Vergrößern
dock-zoom-out = Verkleinern
dock-collapse = Einklappen
dock-expand = Ausklappen

## Shader picker notes

shader-note-empty = Wähle ein Beispiel zum Anfangen, oder zeige rox eine .wgsl-Datei mit einer Fragment-Stufe, die fs_user(uv) definiert
shader-note-missing = { $name } ist nicht mehr in den Shadern dieser Arbeitsfläche, also malt nichts. Wähle hier etwas anderes, und dieses Panel bekommt eine eigene Quelle.
shader-note-shared = In dieser Arbeitsfläche geteilt. Ein Bearbeiten wirkt auf jede Fläche, die ihn nutzt.
shader-note-file = { $path }. Deine Speicherungen laden neu, während der Shader zeichnet, und die Quelle reist in Layouts und Bundles mit, also übersteht sie eine Maschine, die die Datei nie hatte.
shader-note-custom = Diese Quelle reist in ihrem Layout oder Bundle mit, ohne Datei dahinter. Als Datei bearbeiten schreibt sie heraus und nimmt deine Speicherungen auf.

## Panel pages and shared sides

panel-page-layout = Layout
panel-page-view = Ansicht
panel-page-content = Inhalt
panel-page-source = Quelle
panel-page-bindings = Bindungen
panel-page-emitters = Emitter
panel-page-forces = Kräfte
side-left = Links
side-right = Rechts
genre-face-mosaic = Mosaik
genre-face-tinted = Getönt
genre-face-gradient = Verlauf
genre-face-color = Farbe

## Library panel

panel-title-library = Bibliothek
library-play = Abspielen
library-play-album = Album abspielen
library-play-group = Gruppe abspielen
library-play-tracks = { $count } Titel abspielen
library-play-similar = Ähnliches abspielen
library-filter-by-album = Nach Album filtern
library-filter-by-artist = Nach Interpret filtern
library-jump-to-playing = Zum laufenden Titel springen
library-menu-display = Anzeige
library-disc = CD { $number }
library-empty-title = Musikordner öffnen
library-empty-note = Er wird in die Bibliothek eingelesen (flac, mp3, wav)
library-headers = Kopfzeilen
    .description = Gruppenumbrüche über der Liste; eine Sortierung hält zusammen, was zusammengehört, die Suche stellt flach dar
library-group-by = Gruppieren nach
    .description = Worauf die Kopfzeilen umbrechen; Genre und Jahr sortieren die Liste neu
library-header-row = Kopfzeile
    .description = Was die einzeiligen Kopfzeilen von links nach rechts packen; ein Abstand oder Trenner teilt die Seiten
library-header-lines = Kopfzeilen-Zeilen
    .description = Die Zeilen des Blocks von oben nach unten; eine leere Zeile fällt weg
library-follow-description = Zur laufenden Zeile scrollen, sobald der Titel wechselt
library-resume-description = Zur laufenden Zeile zurückscrollen, wenn du aufhörst zu stöbern
library-smooth-description = Zur Zeile gleiten statt zu springen
library-columns = Spalten
    .description = Welche Spalten erscheinen; zieh die Kopfzeilen im Panel, um sie zu ordnen und zu bemessen
library-column-headers = Spaltenköpfe
    .description = Die sortierbare Kopfzeile über der Liste; ausgeblendet behalten die Spalten Reihenfolge und Breite
library-compact-plays = Kompakte Wiedergaben
    .description = Die Wiedergabespalte als kleine Zahl mit einem Strich daneben
library-line-height = Zeilenhöhe
    .description = Eine Kopfzeile; Blöcke nehmen sich die Zeilen, die sie brauchen, unabhängig von den Titelzeilen
library-text-size = Textgröße
    .description = Der Text der Kopfzeilen, unabhängig von der Zeilenhöhe, sodass das Cover allein wächst
library-flush-background = Bündiger Hintergrund
    .description = Die Kopfzeilen auf den Listenhintergrund setzen statt auf die angehobene Tönung; Songfarben bewegen beide zusammen
library-gap-above = Abstand oben
    .description = Vom oberen Rand des Blocks abgeschnitten; die Liste scheint durch, und die Zeilen rücken zusammen
library-gap-below = Abstand unten
    .description = Dasselbe unter dem Block, vor seinen Titeln
library-section-rows = Zeilen
library-row-height = Zeilenhöhe
    .description = Die Titelzeilen; der Text folgt, und beide skalieren mit der App-Schrift
library-row-spacing = Zeilenabstand
    .description = Zusätzliche Höhe je Zeile; Luft, ohne den Text zu vergrößern
library-stripes = Abwechselnde Hervorhebung
    .description = Jede zweite Titelzeile tönen, damit eine lange Liste scannbar bleibt
library-row-borders = Zeilenlinien
    .description = Die Haarlinie unter jeder Titelzeile
library-art-description = Die Kachel der ausgeklappten Kopfzeilen: das Cover, das Porträt des Interpreten oder das Genre-Bild
library-art-rounding = Cover-Rundung
    .description = Die Ecken des Covers abrunden
library-art-position = Cover-Position
    .description = Auf welcher Seite des Blocks die Kachel der ausgeklappten Kopfzeilen sitzt
library-art-margin = Cover-Abstand
    .description = Die Kachel im Block einrücken; sie schrumpft, um quadratisch zu bleiben
library-circular-portraits = Runde Porträts
    .description = Nach Interpret gruppiert, die Kacheln auf den vollen Kreis der Wand runden statt auf den Rundungsregler
library-genre-face = Genre-Bild
    .description = Nach Genre gruppiert, was die Kachel trägt: die Cover, die Cover in der Farbe des Genres gewaschen, oder eine Farbkarte unter ihrer Geometrie

## Album grid panel

panel-title-album-grid = Albumraster
grid-menu-scroll = Scrollen
grid-vertical-scroll = Vertikal scrollen
grid-horizontal-scroll = Horizontal scrollen
grid-jump-to-playing = Zum laufenden Album springen
grid-library-empty = Die Bibliothek ist leer
grid-play-albums = { $count } Alben abspielen
grid-vertical-layout = Vertikales Layout
    .description = Die Wand hoch und runter scrollen, Zeilen füllen die Breite; aus scrollt sie nach links und rechts, Spalten füllen die Höhe
grid-follow-description = Zum laufenden Album scrollen, sobald der Titel wechselt
grid-resume-description = Zum laufenden Album zurückgleiten, wenn du aufhörst zu stöbern
grid-smooth-description = Zum Album gleiten statt zu springen
grid-section-dimming = Abdunkeln
grid-section-tiles = Kacheln
grid-dim-while-playing = Bei Wiedergabe abdunkeln
    .description = Jedes Cover außer dem des laufenden Albums ausblenden; ein Zeiger darauf hellt die Kachel wieder auf
grid-dim-amount = Stärke
    .description = Wie weit die anderen Cover verblassen; 100% blendet sie ganz aus
grid-desaturate = Bei Wiedergabe entsättigen
    .description = Jedes Cover außer dem des laufenden Albums in Graustufen ziehen; ein Zeiger darauf holt die Farbe zurück
grid-always = Immer
    .description = Die Cover auch dann zurücknehmen, wenn nichts läuft; nur die Kachel unter dem Zeiger zeigt sich voll
grid-show-titles = Titel anzeigen
    .description = Album und Interpret unter jedes Cover drucken, iTunes-Stil, statt nur beim Darüberfahren
grid-title-alignment = Titelausrichtung
    .description = Die Beschriftungen unter ihren Covern ausrichten
grid-tile-size = Kachelgröße
    .description = Die längste Kante der Cover-Kacheln; Spalten teilen die Panelbreite gleichmäßig
grid-gap = Abstand
    .description = Platz zwischen den Covern; null packt sie Kante an Kante
grid-art-rounding-description = Die Ecken jedes Covers abrunden; 100% ist ein Kreis
