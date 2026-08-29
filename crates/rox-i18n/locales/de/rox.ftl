### Deutsch. Spiegelt en-CA/rox.ftl Schlüssel für Schlüssel; der
### Paritätstest in rox-i18n wacht darüber. Die Beschreibung einer
### Zeile ist ein Attribut der Nachricht ihres Labels.

## Shared widgets
tracking-title = Verfolgung
tracking-follow = Wiedergabe folgen
tracking-resume = Automatisch zurückkehren
tracking-smooth = Sanftes Scrollen
align-row = Ausrichtung
    .description = Wo der Inhalt steht, wenn das Panel Platz übrig hat
valign-row = Vertikale Ausrichtung
    .description = Wo der Inhalt steht, wenn das Panel Höhe übrig hat
valign-top = Oben
valign-middle = Mitte
valign-bottom = Unten
letter-rail-compact = Kompakte Leiste
    .description = Die Leiste auf eine Zeile begrenzen, die scrollt statt umzubrechen
letter-rail-side = Leistenposition
    .description = An welcher Kante der Wand die Leiste hängt

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
    .description = Worauf das Signal reagiert: Band folgt einem Frequenzbereich, Pegel der ganzen Mischung, Onset pulst bei jedem Schlag im Bereich, Trigger löst einen Impuls aus, wenn der Bereich seinen Schwellwert erreicht, Summe zählt ein anderes Signal über die Zeit zusammen
signal-kind-band = Band
signal-kind-level = Pegel
signal-kind-onset = Onset
signal-kind-trigger = Trigger
signal-kind-total = Summe
signal-response = Ansprechverhalten
signal-response-pulse = Wie lange jeder Impuls nachklingt
signal-response-drift = 0 folgt der Musik sofort, 100 zieht träge hinterher
signal-threshold = Schwellwert
signal-threshold-trigger = Der Pegel, den der Bereich erreichen muss, um den Impuls auszulösen; er feuert erst wieder, wenn der Pegel unter die Marke auf der Anzeige darüber fällt
signal-threshold-gate = Darunter steht das Signal auf null, darüber klettert die Ausgabe wieder von null hoch, damit leise Stellen den Regler nicht bewegen. Die Marke auf der Anzeige darüber zeigt, wo der Schwellwert liegt
signal-low-bound = Untere Grenze
signal-high-bound = Obere Grenze
signal-adds-up = Zählt zusammen
    .description = Welches Signal hier summiert wird; es klettert, solange das andere hoch steht, und stockt, solange es leise ist
signal-aggregate-nothing = Kein Signal zum Folgen
signal-aggregate-pick = Signal wählen
signal-aggregate-alone = Es gibt kein anderes Signal im Pool, das hier summiert werden könnte, also bleibt es bei null. Füge eines hinzu, und es taucht in der Liste auf.
signal-aggregate-unpicked = Nichts gewählt, also bleibt diese Summe bei null. Wähle oben ein Signal.
signal-rate = Rate
    .description = Umläufe pro Sekunde bei vollem Eingang; nach 1 springt es zurück auf 0 und klettert weiter, was ein Shader als Phase versteht
signal-reset-on-track = Bei Titelwechsel zurücksetzen
    .description = Auf null zurücklaufen, wenn ein neuer Titel beginnt, damit eine Phase nicht bei der Summe des letzten anfängt
signal-flush = Leeren
signal-routes-in-panel = { $count ->
    [one] { $count } Route in diesem Panel
   *[other] { $count } Routen in diesem Panel
}
    .description = Jetzt auf null zurücksetzen. Es läuft über einen Moment leer statt zu springen, damit nichts, was daran hängt, ruckt
route-header = Route
route-signal = Signal
    .description = Welchem gemeinsamen Signal diese Route folgt; was du hier einstellst, gilt für jede Route an diesem Signal
route-new-signal = Neues Signal
route-shared-note = Gilt für jede Route an diesem Signal
route-signal-gone = Das Signal dieser Route ist weg; der Regler behält seinen eingestellten Wert, bis oben ein anderes gewählt wird.
route-range-note = Bereich nur für diesen Parameter
route-quiet = Leise
    .description = Worauf der Regler bei Stille steht, als Anteil seiner eigenen Einstellung
route-loud = Laut
    .description = Worauf er bei vollem Signal steht; 100 % ist der eigene Wert des Schiebers, unter Leise moduliert nach unten
route-slot = Slot
    .description = Welchen der sechzehn Signal-Slots des Shaders diese Route füllt
route-slot-quiet-description = Worauf der Slot bei Stille steht
route-slot-loud-description = Worauf er bei vollem Signal steht; unter Leise läuft der Slot rückwärts
route-slot-signal-description = Welchem gemeinsamen Signal diese Route folgt
route-slot-signal-gone = Das Signal dieser Route ist weg; der Slot bleibt auf null, bis ein anderes gewählt wird.
route-add = Route hinzufügen
route-unrouted = Ohne Route
route-pick-slot = Slot wählen
route-pick-signal = Signal wählen
route-no-signal = kein Signal
route-no-signals-yet = Es gibt noch keine Signale, denen eine Route folgen könnte. Erstelle eines, und es taucht hier auf; bis dahin bleibt der Slot auf null.
route-open-signals = Signale öffnen
route-create-signal = Neues Signal erstellen

## Panel settings window
panel-settings = Panel-Einstellungen
panel-menu-label = Panel
panel-save-as-preset = Als Vorlage speichern
panel-rename = Umbenennen
panel-rename-name = Name
panel-rename-note = Wird als Reiter des Panels angezeigt; leer geht zurück zum eingebauten Namen
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
    .description = Das Panel an Ort und Stelle festheften; im Dock lässt es sich dann weder ziehen noch umsortieren
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
    .description = Diesem Panel eine eigene Deckkraft über dem Hintergrund geben statt der Deckkraft der App
panel-surface-opacity = Flächendeckkraft
panel-margin = Außenabstand
    .description = Das Panel in seiner Zelle einrücken; der Hintergrund scheint durch die Lücke
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
    .description = Einen WGSL-Shader über die Fläche dieses Panels laufen lassen, unter dem Bildschirm-Shader der App
panel-run-when-idle = Bei Stille weiterlaufen
    .description = Weiter Bilder zeichnen, solange der Ton still ist. Ausgeschaltet friert der Shader auf seinem letzten Bild ein, und das Panel kostet nichts
panel-shader-is-scene = Dieser Shader ist eine Szene, also deckt er die Fläche des Panels ab, statt darüber zu zeichnen. Er stammt aus einem Bundle oder einer älteren Konfiguration; die Liste oben bietet nur Shader an, die das Panel lesbar lassen.

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
preset-back-tail = in jedem Panel-Menü. Vorlagen gelten nur für diese Arbeitsfläche, eine andere hat sie nicht.

## Keyboard hints
hint-press = Drücke
hint-key-enter = Enter

## Settings: language
settings-language = Sprache
    .description = Die Sprache der Oberfläche; System gleicht mit der Liste des Betriebssystems ab und fällt auf Englisch zurück, wenn nichts passt
    .keywords = sprache uebersetzung lokalisierung landessprache
settings-language-system = (Systemsprache)
settings-language-search = Sprachen durchsuchen
picker-no-matches = Keine Treffer
settings-search-no-matches = Nichts passt zu "{ $text }"

## Embed dialog
bake-window-title = rox - Gespeicherte Metadaten einbetten
bake-title = Gespeicherte Metadaten einbetten
bake-intro = Schreibt gespeicherte Metadaten in die Dateien selbst, damit auch ein anderer Player sie liest. Nichts wird neu berechnet.
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
bake-nothing-to-embed = Nichts einzubetten: die Dateien enthalten bereits alles, was rox gespeichert hat
bake-rewrites = { $count ->
    [one] { $count } Datei wird neu geschrieben
   *[other] { $count } Dateien werden neu geschrieben
}
bake-hint-before = Zum Einbetten
bake-hint-key = Enter
bake-hint-after = drücken
bake-embed = Einbetten
bake-cancel = Abbrechen
bake-summary-files = { $count ->
    [one] 1 Datei
   *[other] { $count } Dateien
}
bake-summary-updated = { $files } aktualisiert
bake-summary-stopped = Gestoppt, { $files } aktualisiert
bake-summary-skipped = , { $count } übersprungen
bake-summary-failed = , { $count } fehlgeschlagen

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
panel-add-to-queue = Zur Warteschlange hinzufügen
panel-add-to-playlist = Zur Playlist hinzufügen
panel-favourite-add = Zu Favoriten hinzufügen
panel-favourite-remove = Aus Favoriten entfernen
shader-pick-missing = { $name } (fehlt)
shader-pick-custom = Eigen

## Shipped shader examples
shader-blurb-plasma = Treibende Farbe allein aus den eigenen Uniforms, also kostet es nur ein einfaches Quad.
shader-blurb-trails = Verschmiert das eigene letzte Bild, läuft also im Bildschirmdurchgang.
shader-blurb-sheen = Eine Vignette und ein wanderndes Glänzen, transparentes Overlay für ein Panel, das schon zeichnet.
shader-blurb-shadow = Ein Schlagschatten, den Text und Steuerelemente des Panels werfen, aus der Maskenaufnahme gewonnen.
shader-blurb-cover = Das Cover des laufenden Titels, im Letterbox-Format über einer Fläche in seiner eigenen Farbe.
shader-blurb-badge = Das Cover als kleine Karte in einer Ecke, mit einem Slot, um sie herumzuschieben.
shader-blurb-lamp = Ein Licht, das dem Zeiger folgt und auf Klicks reagiert, transparentes Overlay.
shader-blurb-cube = Ein Drahtgitterwürfel, der in falschem 3D taumelt, als additives Licht gezeichnet.
shader-blurb-bloom = Treibende Kugeln, durch einen halb so großen zweiten Durchgang gebloomt, die Kette im Kleinen.
shader-blurb-tube = Spielt das Panel darunter über eine gewölbte Röhrenfront ab, samt Scanlines.

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
shader-note-missing = { $name } ist nicht mehr in den Shadern dieser Arbeitsfläche, also wird nichts gezeichnet. Wähle hier etwas anderes, und dieses Panel bekommt eine eigene Quelle.
shader-note-shared = In dieser Arbeitsfläche geteilt. Ein Bearbeiten wirkt auf jede Fläche, die ihn nutzt.
shader-note-file = { $path }. Gespeicherte Änderungen laden neu, während der Shader zeichnet, und die Quelle steckt in Layouts und Bundles, funktioniert also auch auf einem Rechner, der die Datei nie hatte.
shader-note-custom = Diese Quelle reist in ihrem Layout oder Bundle mit, ohne Datei dahinter. Als Datei bearbeiten schreibt sie wieder heraus und übernimmt deine Änderungen.

## Panel pages and shared sides
panel-page-layout = Layout
panel-page-view = Ansicht
panel-page-content = Inhalt
panel-page-source = Quelle
panel-page-bindings = Bindungen
panel-page-emitters = Emitter
panel-page-forces = Kräfte
panel-page-scene = Szene
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
    .description = Gruppenumbrüche über der Liste; eine Sortierung hält zusammen, was zusammengehört, und die Suche zeigt die Liste flach
library-group-by = Gruppieren nach
    .description = Worauf die Kopfzeilen umbrechen; Genre und Jahr sortieren die Liste neu
library-header-row = Kopfzeile
    .description = Was die einzeilige Kopfzeile von links nach rechts anzeigt; ein Abstand oder Trenner teilt die Seiten
library-header-lines = Blockzeilen
    .description = Die Zeilen des Blocks von oben nach unten; eine leere Zeile fällt weg
library-follow-description = Zur laufenden Zeile scrollen, sobald der Titel wechselt
library-resume-description = Zur laufenden Zeile zurückscrollen, wenn du aufhörst zu stöbern
library-smooth-description = Zur Zeile gleiten statt zu springen
library-columns = Spalten
    .description = Welche Spalten erscheinen; zieh die Kopfzeilen im Panel, um sie umzuordnen und in der Breite anzupassen
library-column-headers = Spaltenköpfe
    .description = Die sortierbare Kopfzeile über der Liste; ausgeblendet behalten die Spalten Reihenfolge und Breite
library-compact-plays = Kompakte Wiedergaben
    .description = Die Wiedergabespalte als kleine Zahl mit einem Strich daneben
library-line-height = Zeilenhöhe
    .description = Eine Kopfzeile; Blöcke nehmen sich die Zeilen, die sie brauchen, unabhängig von den Titelzeilen
library-text-size = Textgröße
    .description = Der Text der Kopfzeilen, unabhängig von der Zeilenhöhe, sodass das Cover allein wächst
library-flush-background = Bündiger Hintergrund
    .description = Die Kopfzeilen auf den Listenhintergrund setzen statt auf die angehobene Tönung; Songfarben färben dann beide gemeinsam
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
    .description = Jede zweite Titelzeile tönen, damit sich eine lange Liste leichter überfliegen lässt
library-row-borders = Zeilenlinien
    .description = Die Haarlinie unter jeder Titelzeile
library-art-description = Die Kachel der ausgeklappten Kopfzeilen: das Cover, das Porträt des Interpreten oder das Genre-Bild
library-art-rounding = Cover-Rundung
    .description = Die Ecken des Covers abrunden
library-art-position = Cover-Position
    .description = Auf welcher Seite des Blocks die Kachel der ausgeklappten Kopfzeilen steht
library-art-margin = Cover-Abstand
    .description = Die Kachel im Block einrücken; sie schrumpft, um quadratisch zu bleiben
library-circular-portraits = Runde Porträts
    .description = Nach Interpret gruppiert, die Kacheln auf den vollen Kreis der Wand runden statt auf den Rundungsregler
library-genre-face = Genre-Bild
    .description = Nach Genre gruppiert, was die Kachel zeigt: die Cover, die Cover in der Farbe des Genres getönt, oder eine Farbkarte unter ihrer Geometrie

## Album grid panel
panel-title-album-grid = Albumraster
grid-menu-scroll = Scrollen
grid-menu-sort = Sortierung
grid-sort-artist = Interpret
grid-sort-album = Album
grid-sort-year = Jahr
grid-sort-added = Zuletzt hinzugefügt
grid-sort-plays = Meistgespielt
grid-letter-rail = Buchstabenleiste
    .description = Die Initialen am Rand der Wand; ein Klick springt zum ersten Album des Buchstabens
grid-vertical-scroll = Vertikal scrollen
grid-horizontal-scroll = Horizontal scrollen
grid-jump-to-playing = Zum laufenden Album springen
grid-library-empty = Die Bibliothek ist leer
grid-play-albums = { $count } Alben abspielen
grid-vertical-layout = Vertikales Layout
    .description = Die Wand hoch und runter scrollen, Zeilen füllen die Breite; ausgeschaltet scrollt sie nach links und rechts, Spalten füllen die Höhe
grid-follow-description = Zum laufenden Album scrollen, sobald der Titel wechselt
grid-resume-description = Zum laufenden Album zurückgleiten, wenn du aufhörst zu stöbern
grid-smooth-description = Zum Album gleiten statt zu springen
grid-section-dimming = Abdunkeln
grid-section-tiles = Kacheln
grid-dim-while-playing = Bei Wiedergabe abdunkeln
    .description = Jedes Cover außer dem des laufenden Albums verblassen lassen; beim Überfahren leuchtet eine Kachel wieder auf
grid-dim-amount = Stärke
    .description = Wie weit die anderen Cover verblassen; 100 % blendet sie ganz aus
grid-desaturate = Bei Wiedergabe entsättigen
    .description = Jedes Cover außer dem des laufenden Albums in Graustufen bringen; beim Überfahren kehrt die Farbe einer Kachel zurück
grid-always = Immer
    .description = Die Cover auch dann zurücknehmen, wenn nichts läuft; nur eine überfahrene Kachel zeigt sich ganz
grid-show-titles = Titel anzeigen
    .description = Album und Interpret unter jedes Cover setzen, wie in iTunes, statt nur beim Überfahren
grid-title-alignment = Titelausrichtung
    .description = Die Beschriftungen unter ihren Covern ausrichten
grid-tile-size = Kachelgröße
    .description = Die längste Kante der Cover-Kacheln; Spalten teilen die Panelbreite gleichmäßig
grid-gap = Abstand
    .description = Platz zwischen den Covern; null packt sie Kante an Kante
grid-art-rounding-description = Die Ecken jedes Covers abrunden; 100 % ist ein Kreis

## Settings: sidebar pages
settings-page-appearance = Aussehen
settings-page-application = Anwendung
settings-page-audio = Audio
settings-page-development = Entwicklung
settings-page-integrations = Integrationen
settings-page-keymap = Tastenbelegung
settings-page-library = Bibliothek
settings-page-mcp = MCP
settings-page-ml-models = ML-Modelle
settings-page-playback = Wiedergabe
settings-page-providers = Anbieter
settings-page-shader = Shader
settings-page-storage = Speicher
settings-page-workspace = Arbeitsfläche

## Settings: appearance
settings-appearance-backdrop-all-windows = Alle Fenster
    .description = Auch die Unterfenster hinterlegen: Einstellungen, Editoren, Dialoge, herausgelöste Panels. Ausgeschaltet bleiben Hintergrund und Transparenz bei den Arbeitsflächenfenstern
settings-appearance-backdrop-strength = Hintergrundstärke
    .description = Wie stark der Cover-Hintergrund dahinter durchscheint
settings-appearance-border = Rahmen
    .description = Eine Linie um die Kante jedes Panels, in der Farbe der Rolle Rahmen; eine Seite auf null zeichnet keine
settings-appearance-colors-locked-note = Songfarben sind an, also bestimmt der laufende Titel diese Farben, und der Export speichert sie. Schalte sie oben aus, um sie zu bearbeiten
settings-appearance-design-mode = Entwurfsmodus
    .description = Das Layout an Ort und Stelle bearbeiten: die Zeilen zum Hinzufügen, Umbenennen, Duplizieren, Herauslösen und Schließen in den Panel-Menüs, die Steuerelemente, die ein Container über seine Slots legt, und das Ziehen von Reitern. Ausgeschaltet ist all das verborgen; die Seite Arbeitsfläche bearbeitet den Baum weiterhin
    .keywords = bearbeiten umbauen anordnen sperren entwurf
settings-appearance-font = Schrift
    .description = Die appweite Schriftart; Panels können sie in ihren eigenen Einstellungen überschreiben
    .keywords = schrift schriftart schriftbild
settings-appearance-font-size = Schriftgröße
    .description = Die Basisgröße, von der aus der Text jedes Panels skaliert; Steuerelemente und Symbole behalten ihre Größe
settings-appearance-hide-menubar = Menüleiste ausblenden
    .description = Die Menüleiste ausgeblendet halten und über dem Dock einblenden, solange Alt gehalten wird. Zweimal Alt getippt bleibt sie eingeblendet, damit ihre Schaltflächen einen einfachen Klick annehmen
settings-appearance-icons-intro = Ein Paket ist ein Ordner voller SVGs, der die eingebauten Symbole ersetzt; ein Wechsel greift beim nächsten Start
settings-appearance-icons-open-folder = Ordner öffnen
settings-appearance-inverse-from-dark = Aus dunklem Farbschema invertieren
settings-appearance-inverse-from-light = Aus hellem Farbschema invertieren
settings-appearance-keep-theme = Farbschema halten
    .description = Das aktive Farbschema halten, auch wenn die Helligkeit eines Covers es umschlagen ließe; die Songfarben tönen die Farbe weiterhin
settings-appearance-margin = Außenabstand
    .description = Jedes Panel in seiner Zelle einrücken; ein Panel kann das in seinen eigenen Einstellungen überschreiben
settings-appearance-new-pack = Neues Paket
settings-appearance-os-decorations = Systemdekorationen
    .description = Titelleiste und Rahmen des Systems an den Hauptfenstern; ausgeschaltet übernehmen die Fenstersteuerung und Panels mit Ziehanker
settings-appearance-pack-name-placeholder = Paketname
settings-appearance-padding = Innenabstand
    .description = Platz innerhalb der Kante jedes Panels, im eigenen Hintergrund gehalten
settings-appearance-palette-export = Exportieren
settings-appearance-palette-import = Importieren
settings-appearance-panel-seams = Panel-Nähte
    .description = Die Haarlinie zwischen Panel-Kacheln; ausgeschaltet bleiben die Ziehgriffe unsichtbar, aber weiterhin ziehbar
settings-appearance-resize-border = Größenänderungsrand
    .description = Die Hauptfenster durch Ziehen an ihren Kanten in der Größe ändern; gilt nur bei ausgeschalteten Systemdekorationen, und ausgeschaltet bleiben Andocken und Win+Pfeil der Weg zur Größenänderung
settings-appearance-rounding = Rundung
    .description = Die Ecken jedes Panels in den Hintergrund abrunden
settings-appearance-section-colors = Farben
settings-appearance-section-frame = Rahmen
settings-appearance-section-icons = Symbole
settings-appearance-section-interface = Oberfläche
settings-appearance-section-theming = Farbgebung
settings-appearance-section-transparency = Transparenz
settings-appearance-section-typography = Typografie
settings-appearance-song-theming = Songfarben
    .description = Die Palette tönen und Fenster mit dem Cover des laufenden Titels hinterlegen
settings-appearance-surface-opacity = Flächendeckkraft
    .description = Wie deckend die Flächen der App über dem Hintergrund wirken
settings-appearance-theme = Farbschema
    .description = Die Palette, die die App zeichnet, und die, auf die der Farbeditor unten zielt; System folgt der Hell- oder Dunkel-Einstellung des Systems
settings-appearance-theme-dark = Dunkel
settings-appearance-theme-light = Hell
settings-appearance-theme-system = System

## Settings: application
settings-application-check-updates = Nach Updates suchen
    .description = Einmal am Tag beim Start von rox nach einer neueren Version schauen; das Über-Fenster prüft so oder so sofort
settings-application-download-updates = Updates herunterladen
    .description = Findet eine Prüfung eine neuere Version, wird sie im Hintergrund geladen und bereitgelegt; der nächste Start führt sie aus
settings-application-enable-ai = KI-Funktionen aktivieren
    .description = KI-Werkzeuge mit rox reden lassen: bringt MCP-Unterstützung und die ML-Modell-Downloads mit, samt ihren Seiten in der Seitenleiste.
settings-application-lock-panel-resize = Panelgrößen sperren
    .description = Panel-Teiler ändern ihre Größe nur bei eingeschaltetem Entwurfsmodus, damit ein Ziehen nahe einer Naht ein fertiges Layout nicht verrückt
settings-application-portable-copying = Daten werden kopiert...
settings-application-portable-mode = Portabler Modus
    .description = Einstellungen, Bibliothek und Caches in einem Ordner rox-data neben der ausführbaren Datei halten, damit der Player mit seinen Daten umzieht. Ausschalten geht zurück zum Systemordner und lässt rox-data liegen
settings-application-portable-not-writable = Der Ordner der App ist nicht beschreibbar
settings-application-portable-restart-note = Gilt ab dem nächsten Start; dieser Lauf bleibt bei seinem aktuellen Ordner
settings-application-remain-in-tray = Im Tray bleiben
    .description = Die Musik weiterlaufen lassen, wenn das letzte Fenster schließt, mit dem Tray-Symbol (unter macOS dem Dock) als Weg zurück
settings-application-section-ai = KI
settings-application-section-control-socket = Steuersocket
settings-application-section-data = Daten
settings-application-section-layout = Layout
settings-application-section-startup = Start
settings-application-section-window = Fenster
settings-application-socket-path = Socket-Pfad
    .description = Die Maschinenschnittstelle von rox im laufenden Betrieb: JSON-RPC über einen lokalen Socket, gebunden an diesen Datenordner. Der Proxy rox-mcp bedient darüber MCP-Clients

## Settings: audio
settings-audio-broadcast-bitrate = Bitrate
    .description = Was der MP3-Encoder pro Sekunde Stream ausgibt
settings-audio-broadcast-enable = Zu Icecast streamen
    .description = Was rox spielt, als Quell-Client an einen Icecast-Server schieben, kodiert als MP3. Der Mount, die Hörer und die Seite zum Netz hin gehören alle zu Icecast; rox verbindet sich nur nach draußen, und ein unerreichbarer Server rührt die lokale Wiedergabe nie an
settings-audio-broadcast-host-placeholder = Icecast-Host
settings-audio-broadcast-login = Quell-Anmeldung
    .description = Die Quell-Zugangsdaten von Icecast, Benutzer und Passwort, die seine Konfiguration nennt
settings-audio-broadcast-mount = Mount
    .description = Der Mount, den Hörer ansteuern, und der Streamname, den er ankündigt
settings-audio-broadcast-name-placeholder = Streamname
settings-audio-broadcast-password-placeholder = Quell-Passwort
settings-audio-broadcast-server = Server
    .description = Host und Port des Icecast-Servers; das Quellprotokoll läuft über einen einfachen Socket
settings-audio-broadcast-user-placeholder = source
settings-audio-crossfade = Überblenden
    .description = Wie lange ein Titel den folgenden überlappt. Die Blende ist für Zufall und Überspringen gedacht; die eigenen Grenzen eines Albums bleiben unberührt, solange die Zeile darunter nichts anderes sagt. Null schaltet sie aus
    .keywords = uebergang blende lueckenlos ueberblendung
settings-audio-equalizer-note = Zehn Oktavbänder über der Ausgabe. Er öffnet sich in einem eigenen Fenster, weil man ihn während der Musik bearbeitet, statt ihn einmal einzustellen
settings-audio-exclusive-mode = Exklusivmodus
    .description = Das Gerät für rox allein beanspruchen und mit der Rate der Datei fahren, wo die Hardware eine annimmt; ausgeschaltet teilt sich rox den Systemmixer mit allem anderen auf dem Desktop
settings-audio-fade-inside-albums = Innerhalb von Alben blenden
    .description = Auch Titel überlappen, die zur selben Platte gehören. Ausgeschaltet bleiben die eigenen Schnitte einer Platte genau so, wie sie gemastert wurden, und genau dort zählt lückenlose Wiedergabe am meisten
settings-audio-open-equalizer = Equalizer öffnen
settings-audio-output-buffer = Puffer
    .description = Wie viel Audio die Karte auf einmal hält. Kürzer reagiert schneller und knackt früher auf einem ausgelasteten Rechner; länger ist sicherer und träger
settings-audio-output-buffer-default = Standard (10 ms)
settings-audio-output-device = Gerät
    .description-default = Der Systemstandard folgt dem, worauf der Desktop eingestellt ist
    .description-linux = Exklusiv beansprucht eine Karte direkt vom Kernel, also listet sie Soundkarten statt der Ausgänge des Desktops. Bluetooth und andere Geräte am Soundserver haben keine Karte zum Beanspruchen und erscheinen nur bei ausgeschaltetem Exklusivmodus
    .description-other = Exklusiv nimmt das Gerät für rox allein, also kann nichts anderes auf dem Desktop hindurch klingen, bis der Modus aus ist
settings-audio-output-device-system-default = Systemstandard
settings-audio-output-experimental-badge = Experimentell
settings-audio-output-experimental-tooltip = Das Exklusiv-Backend dieser Plattform ist nach ihrer dokumentierten Audio-Schnittstelle geschrieben, lief aber nie auf echter Hardware der Entwickler. Es sollte das Gerät beanspruchen oder mit einer Begründung auf geteilt zurückfallen, niemals verstummen. Wenn es sich danebenbenimmt, schalte es aus und melde mit der Schaltfläche neben dieser Plakette, was passiert ist.
settings-audio-output-format = Format
    .description = Was rox der Karte übergibt. Eine Karte, die die Wahl nicht annimmt, fährt ihr breitestes Format, und der Status darunter zeigt, welches
settings-audio-output-format-f32 = 32-Bit-Gleitkomma
settings-audio-output-format-s16 = 16-Bit-Ganzzahl
settings-audio-output-format-s32 = 32-Bit-Ganzzahl
settings-audio-output-format-widest = So breit wie möglich
settings-audio-output-issue-tooltip = Melde, wie sich der Exklusivmodus auf diesem Rechner verhalten hat. Öffnet ein GitHub-Issue mit ausgefüllter Plattform und ausgehandeltem Stream.
settings-audio-output-mode-exclusive = Exklusiv
settings-audio-output-mode-shared = Geteilt
settings-audio-output-not-built = Für diese Plattform noch nicht gebaut
settings-audio-output-rate-follow = Der Datei folgen
settings-audio-output-sample-rate = Abtastrate
    .description = Folgen öffnet das Gerät bei jeder Datei mit deren eigener Rate neu, was an einer Grenze mit Ratenwechsel eine Lücke kostet; eine feste Rate zahlt das nie und tastet alles neu ab, was nicht passt
settings-audio-output-status-error-hint = Wähle ein anderes Gerät, oder schalte Exklusiv aus
settings-audio-output-status-error-title = Keine Ausgabe
settings-audio-output-status-idle-hint = Starte einen Titel, um zu sehen, welches Format das Gerät angenommen hat
settings-audio-output-status-idle-title = Nichts läuft
settings-audio-replaygain-level-by = Pegeln nach
    .description = Jeden Titel mit der Lautheit spielen, die seine ReplayGain-Tags gemessen haben, damit ein Zufallslauf nicht mehr zwischen Masterings springt. Titel pegelt jede Datei für sich; Album nimmt die Verstärkung der Platte über alle ihre Titel, was die leisen und lauten Stellen eines Albums dort lässt, wo sie hingelegt wurden
    .keywords = lautstaerke normalisierung pegel angleichen
settings-audio-replaygain-measure-missing-button = Fehlende messen
settings-audio-replaygain-measure-new = Neue Dateien messen
    .description = Messen, was die Ordnerüberwachung hereinholt, sobald es ankommt und der Abgleich zur Ruhe gekommen ist; so behält eine wachsende Bibliothek ihre Verstärkungen, ohne dass du hierher zurückmusst. Die Zahlen gehen dorthin, wohin Gemessene Verstärkungen speichern zeigt. Beim Einschalten wird angeboten, zuerst das bereits Fehlende zu messen; danach sieht es nur noch gerade erst hinzugekommene Dateien
settings-audio-replaygain-measuring-progress = Messe { $done } von { $total }
settings-audio-replaygain-measuring-start = Messung: ermittle, was fehlt...
settings-audio-replaygain-mode-album = Album
settings-audio-replaygain-mode-off = Aus
settings-audio-replaygain-mode-track = Titel
settings-audio-replaygain-preamp = Vorverstärkung
    .description = Wird zu jeder getaggten Verstärkung addiert. Die Referenz von ReplayGain liegt unter dem, worauf moderne Platten geschnitten werden, also spielt eine gepegelte Bibliothek leiser als dieselbe Bibliothek roh; hier kommt das zurück. Eine Anhebung übersteuert nie: der getaggte Spitzenwert deckelt sie
settings-audio-replaygain-save = Gemessene Verstärkungen speichern
    .description = Wohin der Messdurchgang seine Zahlen legt. Die Bibliotheksdatenbank lässt deine Dateien unberührt; Tags legen dieselben Werte dorthin, wo jeder andere Player sie liest, um den Preis, die Audiodateien neu zu schreiben
settings-audio-replaygain-status-measured = Alle { $total } eingelesenen Titel haben eine Verstärkung zum Pegeln, { $measured } davon von rox gemessen
settings-audio-replaygain-status-tagged = Alle { $total } eingelesenen Titel haben ReplayGain-Tags
settings-audio-replaygain-untagged = Ungetaggte Dateien
    .description = Mit welcher Verstärkung eine Datei ohne ReplayGain-Tags spielt. Nichts hat sie gemessen, das hier ist also nur eine Schätzung. Lass sie auf null, und ungetaggte Titel spielen wie eh und je
settings-audio-section-broadcast = Übertragung
settings-audio-section-equalizer = Equalizer
settings-audio-section-output = Ausgabe
settings-audio-section-playback = Wiedergabe
settings-audio-section-replaygain = ReplayGain
settings-audio-transport = Transport
    .description = Starten und stoppen, ohne diese Seite zu verlassen, denn jede Einstellung darunter wird nach Gehör beurteilt

## Settings: integrations
settings-integrations-discord-enable = Rich Presence aktivieren
    .description = rox-Aktivität auf Discord zeigen, während Musik läuft
settings-integrations-discord-show-lastfm = Last.fm-Schaltfläche zeigen
    .description = Eine klickbare Schaltfläche 'Auf Last.fm ansehen' im Discord-Status mitgeben
settings-integrations-discord-show-youtube = YouTube-Schaltfläche zeigen
    .description = Eine klickbare Schaltfläche 'Auf YouTube suchen' im Discord-Status mitgeben
settings-integrations-ffmpeg-binary = FFmpeg-Binärdatei
    .description = Welches ffmpeg die Umwandlungen übernimmt; leer lassen für das im PATH
settings-integrations-ffmpeg-fail-note = Umwandeln bleibt versteckt, bis ffmpeg auf eine lauffähige Binärdatei zeigt
settings-integrations-ffmpeg-fail-title = Dieses ffmpeg lief nicht
settings-integrations-ffmpeg-missing-note = Umwandeln bleibt versteckt; installiere ffmpeg oder zeige mit dem Pfad auf eine Binärdatei
settings-integrations-ffmpeg-missing-title = Kein lauffähiges ffmpeg gefunden
settings-integrations-ffmpeg-ok-note = ffmpeg läuft. Umwandeln steht bereit.
settings-integrations-ffmpeg-test = Testen
settings-integrations-lastfm-api-key-row = API-Schlüssel
settings-integrations-lastfm-connect = Verbinden
settings-integrations-lastfm-disconnect = Trennen
settings-integrations-lastfm-finish-connecting = Verbindung abschließen
settings-integrations-lastfm-hearts = { $n ->
    [one] { $n } Herz
   *[other] { $n } Herzen
}
settings-integrations-lastfm-import-loved = Geliebte Titel importieren
settings-integrations-lastfm-intro-builtin = Verbinde dein Last.fm-Konto: autorisiere rox im Browser, und gespielte Titel scrobbeln dorthin
settings-integrations-lastfm-intro-custom = Dieser Build bringt keine API-Identität mit, also braucht Scrobbeln dein eigenes API-Konto (Last.fm/api/account/create); füge Schlüssel und Shared Secret ein, dann verbinde
settings-integrations-lastfm-key-placeholder = API-Schlüssel
settings-integrations-lastfm-love-failed = Der letzte Versuch ist fehlgeschlagen: { $error }
settings-integrations-lastfm-love-pending = { $hearts } noch zu senden
settings-integrations-lastfm-love-pending-failed = { $hearts } noch zu senden, letzter Versuch: { $error }
settings-integrations-lastfm-reconnect = Neu verbinden
settings-integrations-lastfm-secret-placeholder = Shared Secret
settings-integrations-lastfm-secret-row = Shared Secret
settings-integrations-lastfm-status-confirming = Bestätige...
settings-integrations-lastfm-status-connected = Verbunden als { $username }
settings-integrations-lastfm-status-elsewhere = Auf einer anderen rox-Installation verbunden; jede autorisiert unter ihrer eigenen API-Identität, verbinde also auch diese
settings-integrations-lastfm-status-failed = Verbindung fehlgeschlagen: { $error }
settings-integrations-lastfm-status-not-connected = Nicht verbunden
settings-integrations-lastfm-status-rejected = Last.fm hat die Sitzung abgelehnt, und sie wurde verworfen. Verbinde erneut, um weiter zu scrobbeln
settings-integrations-lastfm-status-requesting = Fordere ein Token an...
settings-integrations-lastfm-status-waiting = Autorisiere rox im Browser, dann schließe die Verbindung ab
settings-integrations-lastfm-working = Arbeite...
settings-integrations-love-favourites = Favoriten lieben
    .description = Herzen als geliebte Titel zu Last.fm spiegeln; ein zurückgenommenes Herz nimmt es auch dort zurück
settings-integrations-scrobble-threshold = Scrobble-Schwelle
    .description = Wie viel von einem Titel laufen muss, bevor er scrobbelt; die Suchleiste und die Wellenform können die Schwelle markieren
settings-integrations-scrobble-tracks = Titel scrobbeln
    .description = Gespielte Titel an Last.fm senden, sobald sie die Schwelle überschreiten
settings-integrations-section-conversion = Umwandlung
settings-integrations-section-discord = Discord Rich Presence
settings-integrations-section-favourites = Favoriten
settings-integrations-section-lastfm = Last.fm
settings-integrations-section-scrobbling = Scrobbeln

## Settings: keymap
settings-keymap-clash = { $chord } ist auch { $other }; nur eines von beiden löst aus
settings-keymap-not-bound = Nicht belegt
settings-keymap-recording = Drücke die Tasten
settings-keymap-restore = Wiederherstellen
settings-keymap-restore-all = Alle Kürzel wiederherstellen
    .description = Jeden Befehl auf die Tasten zurücksetzen, mit denen er ausgeliefert wird, auch die, für die dieser Build keine Zeile mehr hat
settings-keymap-section-defaults = Standards
settings-keymap-undo = Rückgängig
settings-keymap-undo-last = Letztes Zurücksetzen rückgängig machen
    .description = Die Kürzel zurückholen, die das letzte Zurücksetzen verworfen hat, Zeile oder alle

## Settings: library
settings-library-acoustic-all-described = Alle { $total } eingelesenen Titel sind von { $label } beschrieben
settings-library-acoustic-auto = Neue Dateien beschreiben
    .description = Beschreiben, was die Ordnerüberwachung hereinholt, sobald es ankommt und der Abgleich zur Ruhe gekommen ist; so behält eine wachsende Bibliothek ihre Beschreibungen, ohne dass du hierher zurückmusst. Ausgeschaltet warten neue Dateien auf die Schaltfläche Fehlende analysieren. Beim Einschalten wird angeboten, zuerst das bereits Fehlende zu analysieren; danach sieht es nur noch gerade erst hinzugekommene Dateien
settings-library-acoustic-enable = Beschreiben, wie Titel klingen
    .description = Ermitteln, wie jeder Titel klingt, damit die Bibliothek Musik finden kann, die dem Laufenden ähnelt. Alles läuft auf diesem Rechner, und eine große Bibliothek zu beschreiben dauert eine Weile
    .keywords = aehnlich klang beschreiben analyse
settings-library-acoustic-extractor = Extraktor
settings-library-acoustic-extractor-model = Modell
settings-library-acoustic-fallback = Analyse
settings-library-acoustic-partial = { $label } beschreibt { $done } von { $total } eingelesenen Titeln. Fehlende analysieren arbeitet den Rest ab
settings-library-acoustic-progress = { $running } ist bei { $done } von { $total }
settings-library-acoustic-progress-start = { $running }: ermittelt, was fehlt...
settings-library-acoustic-save = Beschreibungen speichern
    .description = Wohin der Durchgang legt, was er ermittelt. Die Datenbank allein lässt deine Dateien unberührt; Tags legen zusätzlich eine Kopie in jede Datei, damit die Beschreibungen einen Neuaufbau der Bibliothek oder einen Umzug des Ordners auf einen anderen Rechner überleben, um den Preis, die Audiodateien neu zu schreiben. Tags erreichen nur MP3 und FLAC, jedes andere Format behält die Kopie in der Datenbank
settings-library-add-folder = Ordner hinzufügen
settings-library-duplicates = Duplikate...
settings-library-embed-button = Gespeicherte Metadaten einbetten...
settings-library-folder-col-albums = Alben
settings-library-folder-col-folder = Ordner
settings-library-folder-col-size = Größe
settings-library-folder-col-tracks = Titel
settings-library-folders-intro = In die Bibliothek eingelesene Ordner; einen zu entfernen wirft seine Titel aus dem Katalog und lässt die Dateien in Ruhe
settings-library-genre-separator-nudge = Trennzeichen geändert: das Stöbern folgt sofort. Genrelisten aus früheren Scans behalten ihre alte Form, bis du oben in der Kopfzeile Ordner auf Neu einlesen drückst
settings-library-merge-case = Schreibweisen zusammenführen
    .description = Werte, die sich nur in der Groß- und Kleinschreibung unterscheiden, als einen behandeln. Rock und rock werden dasselbe Genre, derselbe Interpret und dasselbe Album, gezeigt in der Schreibweise, die die meisten Titel tragen. Dateien behalten ihre Tags, wie sie geschrieben sind
settings-library-no-folders = Noch keine Ordner
settings-library-repair-tags = Tags reparieren...
settings-library-section-folders = Ordner
settings-library-section-stored-metadata = Gespeicherte Metadaten
settings-library-section-tempo = Tempoanalyse
settings-library-split-genres = Genres an Kommas und Schrägstrichen trennen
    .description = "Dubstep, Trap" und "Drum & Bass / Neurofunk" zählen jeden Wert als eigenes Genre; Semikolons trennen immer. Ausgeschaltet bleiben Namen mit Schrägstrich ganz, für Tags, in denen sie ein Genre meinen. Dateien behalten ihre Tags, wie sie geschrieben sind
settings-library-tempo-auto = Neue Dateien zählen
    .description = Die Beats in dem zählen, was die Ordnerüberwachung hereinholt, sobald es ankommt und der Abgleich zur Ruhe gekommen ist; so behält eine wachsende Bibliothek ihre Tempi, ohne dass du hierher zurückmusst. Ausgeschaltet warten neue Dateien auf die Schaltfläche Fehlende analysieren. Beim Einschalten wird angeboten, zuerst das bereits Fehlende zu zählen; danach sieht es nur noch gerade erst hinzugekommene Dateien
settings-library-tempo-enable = Ermitteln, wie schnell Titel laufen
    .description = Die Beats in Titeln zählen, deren Tags es nicht sagen, damit die Bibliothek nach Tempo zeigen und sortieren kann. Alles läuft auf diesem Rechner, die Zahlen gehen in die Bibliotheksdatenbank, und deine Dateien bleiben unberührt
settings-library-tempo-progress = Zähle { $done } von { $total }
settings-library-tempo-progress-start = Ermittle, was fehlt...
settings-library-tempo-status-measured = Alle { $total } eingelesenen Titel haben ein Tempo, { $measured } davon von rox ermittelt
settings-library-tempo-status-tagged = Alle { $total } eingelesenen Titel haben ein Tempo-Tag
settings-library-watch-folders = Ordner überwachen
    .description = Hinzugefügte, geänderte und gelöschte Dateien laufend in die Bibliothek übernehmen, ohne manuelles Neu-Einlesen
settings-library-write-stored = Gespeichertes in die Dateien schreiben
    .description = Die drei Speichereinstellungen gelten nur für den nächsten Schreibvorgang, also steckt alles, was vor einer Umstellung auf Tags gespeichert wurde, weiterhin allein in rox. Das hier schreibt die Songtexte, Verstärkungen und Beschreibungen, die rox bereits gespeichert hat, in die Dateien selbst, damit ein anderer Player, der den Ordner liest, sie sieht. Nichts wird neu berechnet

## Settings: MCP
settings-mcp-client-config = Client-Konfiguration
    .description = In die Serverliste eines MCP-Clients einfügen (Claude Code, Claude Desktop oder ein beliebiger anderer), damit er rox nach der Bibliothek, dem Laufenden und dem Transport fragen kann. rox muss laufen; die Werkzeuge laufen über seinen Steuersocket
settings-mcp-enable = MCP-Server aktivieren
    .description = Auf Werkzeugaufrufe verbundener MCP-Clients antworten. Der Proxy prüft das bei jedem Aufruf, also werden Clients mit der Begründung abgewiesen, solange es aus ist; die Konfiguration darunter lässt sich so oder so einrichten

## Settings: ML models
settings-mlmodels-checking = Prüfe...
settings-mlmodels-choose-file = Datei wählen
settings-mlmodels-custom-description-empty = Zeige rox einen eigenen PANNs-CNN10-Checkpoint, als safetensors. Er wird an Ort und Stelle gelesen und nach seinem Hash benannt, also beschreibt ein zweiter Checkpoint die Bibliothek getrennt, statt die Koordinaten des ersten wiederzuverwenden
settings-mlmodels-download-failed = { $label } konnte nicht heruntergeladen werden: { $reason }
settings-mlmodels-downloading = Lade { $label } herunter: { $done } von { $total }
settings-mlmodels-stopping = Stoppe den Download von { $label }...
settings-mlmodels-fallback-model = Modell
settings-mlmodels-fallback-the-model = Das Modell
settings-mlmodels-kind-custom = Eigen
settings-mlmodels-kind-recommended = Empfohlen
settings-mlmodels-pass-stopped = Der letzte Durchgang hat gestoppt: { $reason }
settings-mlmodels-weights-file = Gewichtsdatei

## Settings: playback
settings-playback-continuation-continue = Fortsetzen
    .description = Die Liste weiterspielen, aus der du gestartet hast, dann den Rest der Bibliothek dahinter. Spiel ein Album aus der Mitte einer Ansicht, und die Ansicht läuft weiter
settings-playback-continuation-off = Aus
    .description = Nichts füllt die Warteschlange nach; die Wiedergabe endet an ihrem Ende
settings-playback-continuation-weighted = Gewichtet
    .description = Aus der ganzen Bibliothek ziehen, nie Gespieltes zuerst und kürzlich Gehörtes zuletzt
settings-playback-keep-playing = Weiterspielen
    .description = Was läuft, wenn die Warteschlange leer wird. Was auch immer das wählt, wird als gewöhnlicher Kontext an die Zeitleiste gehängt, ist also sichtbar und entfernbar statt versteckter Zustand. Steht die Reihenfolge oben auf Ähnlich, findet es weiter Titel, die dem laufenden ähneln, egal welches davon gewählt ist
    .keywords = weiterspielen nachfuellen automatisch warteschlange
settings-playback-play-order = Wiedergabereihenfolge
    .description = Wie die bereits eingereihten Titel angeordnet werden, solange Zufall an ist. Die Zufallstaste im Transport schaltet ihn ein und aus; das hier ist, was er dann tut
settings-playback-rating-scale = Bewertungsskala
    .description = Sterne für schnelle Klicks, 0-10 in halben Schritten für feinere Rezensionsnoten
settings-playback-rating-scale-numeric = 0-10
settings-playback-rating-scale-stars = Sterne
settings-playback-restore-last-session = Letzte Sitzung wiederherstellen
    .description = Mit der Warteschlange starten, wie du sie verlassen hast, pausiert auf dem Titel, der lief, und an der Stelle, wo er stand. Eingereihte Titel außerhalb deiner Bibliotheksordner lassen sich nicht wiederherstellen und fallen aus der Reihenfolge
settings-playback-section-queue = Warteschlange
settings-playback-section-ratings = Bewertungen
settings-playback-section-startup = Start
settings-playback-shuffle-random = Zufällig
    .description = Der Zufall, den jeder mit dem Wort meint. Was kommt, läuft in keiner bestimmten Reihenfolge
settings-playback-shuffle-similar = Ähnlich
    .description = Das Nächste zuerst nach Klang. Was kommt, wird danach sortiert, wie sehr es dem Titel ähnelt, der lief, als du es eingeschaltet hast, und bei jedem Sprung neu sortiert. Die Bibliothek muss dafür auf der Seite Bibliothek beschrieben sein
settings-playback-unrated-dots = Punkte für Unbewertetes
    .description = Ungefüllte Sternplätze mit einem blassen Punkt markieren, statt sie leer zu lassen

## Settings: providers
settings-providers-artist = Last.fm
    .description = Interpretenbiografien, Statistiken und ähnliche Interpreten für das Biografie-Panel holen, mit einem Porträt von Deezer; alles landet im Datenordner und ist danach offline verfügbar
settings-providers-deezer = Deezer
    .description = Deezer nach Covern durchsuchen, bis zu 1000 Pixel
settings-providers-itunes = iTunes
    .description = iTunes nach Covern durchsuchen; die Suche im Cover-Editor zeigt Treffer zur Auswahl, bevor eines gesetzt wird
settings-providers-lastfm-art = Last.fm
    .description = Last.fm nach Covern durchsuchen
settings-providers-lrclib = LRCLIB
    .description = Fehlende Songtexte von lrclib.net holen, synchronisiert wenn vorhanden
settings-providers-lyrics-intro = Online-Abfragen laufen nur, wenn eine Panel-Aktion danach fragt; Wiedergabe und Stöbern rühren das Netz nie an
settings-providers-musicbrainz = MusicBrainz
    .description = Tags auf musicbrainz.org nachschlagen; die Suche im Metadaten-Panel zeigt Treffer, die Feld für Feld bestätigt werden, bevor geschrieben wird
settings-providers-save-lyrics = Geholte Songtexte speichern
    .description = Wo ein geholtes Blatt landet: im eigenen Datenordner von rox, was die Bibliothek sauber hält, in einer .lrc neben dem Titel, oder im eingebetteten Tag
settings-providers-save-lyrics-data-folder = Datenordner
settings-providers-save-lyrics-sidecar = Sidecar
settings-providers-save-lyrics-tag = Tag
settings-providers-section-artist = Interpret
settings-providers-section-cover-art = Cover
settings-providers-section-lyrics = Songtexte
settings-providers-section-metadata = Metadaten

## Settings: shader
settings-shader-backdrop-all-windows = Alle Fenster
    .description = Den Hintergrund jedes Fensters schattieren: Einstellungen, Editoren, Dialoge, herausgelöste Panels. Ausgeschaltet bleibt es bei den Arbeitsflächenfenstern
settings-shader-backdrop-enabled = Hintergrund-Shader
    .description = Einen musikreaktiven WGSL-Shader über den Cover-Hintergrund laufen lassen, unter allen Panels. Teil der Arbeitsfläche, reist also mit dem Erscheinungsbild
settings-shader-backdrop-fallback-name = Hintergrund
settings-shader-backdrop-run-idle = Bei Stille weiterlaufen
    .description = Weiter zeichnen, wenn nichts läuft. Die Animation bleibt so oder so eingefroren
settings-shader-compile-error-title = Dieser Shader ließ sich nicht kompilieren
settings-shader-legacy-note = Ohne Routen füllt der Pool die Slots in seiner eigenen Reihenfolge: das erste Signal in Slot 0, das zweite in Slot 1 und so weiter. Die erste Route, die du hinzufügst, übernimmt die ganze Zuordnung.
settings-shader-overlay-enabled = Overlay-Shader
    .description = Einen musikreaktiven WGSL-Shader über das ganze Fenster laufen lassen. Angeboten werden nur Shader, die die App darunter benutzbar lassen
settings-shader-scene-covers-window = Dieser Shader ist eine Szene, also deckt er das Fenster ab, statt darüber zu zeichnen. Er stammt aus einem Bundle oder einer älteren Konfiguration; die Liste oben bietet nur Shader an, die die App benutzbar lassen.
settings-shader-screen-all-windows = Alle Fenster
    .description = Auch die Unterfenster schattieren: Einstellungen, Statistiken, Equalizer, herausgelöste Panels. Der Countdown zum Zurücknehmen bleibt so oder so unschattiert
settings-shader-screen-fallback-name = Bildschirm
settings-shader-screen-run-idle = Bei Stille weiterlaufen
    .description = Weiter zeichnen, wenn nichts läuft. Die Animation bleibt so oder so eingefroren. Ein Shader, der die Maus ausliest, folgt dem Zeiger auch bei gestoppter Musik ohne das hier; er hört nur ein paar Sekunden nach dem Zeiger auf
settings-shader-section-backdrop = Hintergrund-Shader
settings-shader-section-overlay = Overlay-Shader
settings-shader-signals-block = Signale
    .description = Welches gemeinsame Signal jeder der sechzehn Slots des Shaders bezieht
settings-shader-slots-block = Slots
    .description = Jeder Slot, wie er den Shader erreicht; Slots ohne Route sind von Hand gesetzte Regler

## Settings: storage
settings-storage-artist-images = Interpretenbilder
    .description = Porträts, Banner und Biografien, die für die Interpretenansichten geholt wurden (artists/); geleerte werden beim nächsten Öffnen einer Ansicht neu geholt
settings-storage-catalog = Katalog
    .description = Der Titelindex, den Scans aufbauen: eine Zeile je Titel mit seinen Tags, seinen Dateiangaben und etwaigen CUE-Spannen, in library.db
settings-storage-cover-thumbnails = Cover-Miniaturen
    .description = Kleine Cover, die nach ihrer ersten Darstellung bleiben (thumbs.db); geleerte bauen sich neu auf, sobald sie ins Bild scrollen
settings-storage-logs = Protokolle
    .description = Was jeder Lauf für Fehlerberichte schreibt (logs/rox.log), bei einer Größengrenze rotiert, damit es nie groß wird
settings-storage-looks-layouts = Erscheinungsbilder und Layouts
    .description = Das Erscheinungsbild, das die App gerade verwendet (workspace.json), mit deinen gespeicherten Arbeitsflächen, ausgeworfenen Shader-Dateien und Symbolpaketen daneben. Klein, und jedes Byte davon ist etwas, das du eingerichtet hast
settings-storage-lyrics = Songtexte
    .description = Geholte und bearbeitete Blätter, im eigenen Speicher der App gehalten (lyrics/), damit die Bibliotheksordner sauber bleiben
settings-storage-measured-tempos = Gemessene Tempi
    .description = Die Tempi, die rox aus dem Audio gezählt hat, für Titel, deren Tags keins tragen; die eigenen Zahlen der Tags bleiben unberührt. Leeren setzt diese Titel zurück auf die Liste von Fehlende analysieren auf der Seite Bibliothek, sodass verbessertes Beatzählen die Zahlen ersetzen kann, die ein älterer Durchgang geschrieben hat
settings-storage-model-fallback-this = Dieses Modell
settings-storage-music-summary = { $tracks }, { $albums }, { $size }
settings-storage-model-weights = Modellgewichte
    .description = Die für die akustische Analyse heruntergeladenen Modelle (models/). Auf der Seite ML-Modelle werden sie geholt und gelöscht, eine Zeile je Modell
settings-storage-models-empty = Modelle
    .description = Noch hat nichts die Bibliothek beschrieben. Die akustische Analyse auf der Seite Bibliothek einzuschalten füllt das hier, und jedes Modell, das gelaufen ist, bekommt hier eine Zeile
settings-storage-music-files = Musikdateien
    .description = Was die eingelesenen Ordner halten; die Dateien bleiben, wo sie sind
settings-storage-none = Keine
settings-storage-playlists-history = Playlists und Verlauf
    .description = Deine Playlists und ihre Mitglieder, was du gespielt hast, und die Genre-Notizen der Bibliothek. Alles klein neben dem Rest von library.db
settings-storage-reclaimable = Wiederverwendbarer Platz
    .description = Seiten in library.db, die Löschungen hinterlassen haben. Neue Schreibvorgänge füllen sie wieder, also hört die Datei auf zu wachsen, bevor sie zu schrumpfen beginnt
    .keywords = aufraeumen verdichten schrumpfen datenbank
settings-storage-section-acoustic = Akustische Beschreibungen
settings-storage-section-app-data = App-Daten
settings-storage-section-caches = Caches
settings-storage-section-diagnostics = Diagnose
settings-storage-section-library = Bibliothek
settings-storage-section-tempo = Tempo
settings-storage-vectors = Vektoren
    .description = Was jede Beschreibung in library.db wiegt. In einer Bibliothek, durch die der Analysedurchgang gelaufen ist, ist das der größte Teil der Datei, ein paar Kilobyte je Titel gegen ein paar hundert Byte Tags
settings-storage-waveforms = Wellenformen
    .description = Die Spitzenleiste jedes Titels, nach dem ersten Spielen behalten; geleerte werden beim nächsten Spielen neu dekodiert

## Settings: workspace
settings-workspace-card-author = Autor
settings-workspace-card-author-placeholder = Wer es gemacht hat
settings-workspace-card-created = Erstellt { $date }
settings-workspace-card-created-updated = Erstellt { $created }, aktualisiert { $updated }
settings-workspace-card-description = Beschreibung
settings-workspace-card-description-placeholder = Worauf das Erscheinungsbild hinauswill
settings-workspace-card-empty = Diese Arbeitsfläche hat keine Karte
settings-workspace-card-hint = Die Karte steckt in der Datei, also sieht sie jeder, mit dem du dieses Erscheinungsbild teilst
settings-workspace-card-license = Lizenz
settings-workspace-card-license-placeholder = Die Bedingungen, unter denen du es teilst
settings-workspace-card-save = Karte speichern
settings-workspace-card-updated = Aktualisiert { $date }
settings-workspace-card-version = Version
settings-workspace-card-version-placeholder = Deine eigene Version, wie auch immer du zählst
settings-workspace-card-website = Website
settings-workspace-card-website-placeholder = Wo es zu finden ist
settings-workspace-composition-closed = Das Arbeitsflächenfenster ist geschlossen
settings-workspace-composition-hint = Die Panels des Fensters, wie sie in Teilern und Reitergruppen angeordnet sind; die Pfeile ordnen eine Zeile unter ihren Geschwistern um, das Schloss heftet ein Panel fest, und das Zahnrad öffnet seine Einstellungen
settings-workspace-empty = Noch keine Arbeitsflächen
settings-workspace-hint = Eine Arbeitsfläche ist ein ganzes Erscheinungsbild: Layouts, Palette, Aussehen. Eine anzuwenden ersetzt alle drei
settings-workspace-layout-name-placeholder = Layoutname
settings-workspace-layouts-empty = Noch keine Layouts
settings-workspace-layouts-hint = Primär und Mini sind die beiden, zwischen denen die Mini-Player-Taste der Menüleiste wechselt
settings-workspace-name-placeholder = Name der Arbeitsfläche
settings-workspace-panel-preset-unknown-kind = Unbekanntes Panel
settings-workspace-panel-presets-empty = Noch keine Panel-Vorlagen
settings-workspace-panel-presets-hint-after = in jedem Panel-Menü. Sie gelten nur für diese Arbeitsfläche, eine andere hat sie nicht.
settings-workspace-panel-presets-hint-before = Je ein eingerichtetes Panel, aus dem eigenen Menü eines Panels gespeichert und zurückgeholt über
settings-workspace-role-mini = Mini
settings-workspace-role-primary = Primär
settings-workspace-section-composition = Aufbau
settings-workspace-section-layouts = Layouts
settings-workspace-section-panel-presets = Panel-Vorlagen
settings-workspace-section-workspaces = Arbeitsflächen
settings-workspace-tree-empty-slot = Leerer Slot
settings-workspace-tree-split-column = Geteilt, gestapelt
settings-workspace-tree-split-row = Geteilt, nebeneinander
settings-workspace-tree-tabs = Reiter

## Settings: development
settings-development-experimental-panels = Experimentelle Panels
    .description = Die noch im Bau befindlichen Panels im Menü Panels und im Starter zeigen; sie ändern zwischen Releases ihre Form, und ein Layout, das schon eines hält, behält es auch, wenn das hier wieder aus ist
settings-development-section-features = Funktionen

## Settings: shared
settings-acoustic-analysis-heading = Akustische Analyse
settings-analyze-nothing-scanned = Noch nichts eingelesen zum Analysieren
settings-common-active = Aktiv
settings-common-analyze-missing = Fehlende analysieren
settings-common-built-in = Eingebaut
settings-common-clear = Leeren
settings-common-copy = Kopieren
settings-common-database = Datenbank
settings-common-delete = Löschen
settings-common-download = Herunterladen
settings-common-rescan = Neu einlesen
settings-common-reveal = Zeigen
settings-common-stop = Stopp
settings-common-stopping = Stoppe...
settings-common-tags = Tags
settings-common-tracks-count = { $count } Titel
settings-common-use = Verwenden
settings-confirm-apply-body = Das ersetzt deine Layouts, Palette und dein Aussehen durch die der Arbeitsfläche.
settings-confirm-apply-imported-body = Sie ist in deinen Arbeitsflächen gespeichert. Sie jetzt anzuwenden ersetzt deine Layouts, Palette und dein Aussehen durch die der Arbeitsfläche.
settings-confirm-clear = Leeren
settings-confirm-clear-embeddings-body = Die Beschreibungen gehen und der Platz kommt zurück. Sie wiederzuhaben heißt, den Analysedurchgang über jeden Titel der Bibliothek laufen zu lassen.
settings-confirm-clear-embeddings-title = Leeren, was "{ $model }" beschrieben hat?
settings-confirm-clear-measured-bpm-body = Jedes von rox ermittelte Tempo geht zurück auf ungemessen; Zahlen aus den eigenen Tags deiner Dateien bleiben. Sie wiederzuhaben heißt, den Tempodurchgang über jeden dieser Titel laufen zu lassen.
settings-confirm-clear-measured-bpm-title = Die gemessenen Tempi leeren?
settings-confirm-overwrite-workspace-body = Das ersetzt die gespeicherte Arbeitsfläche durch den aktuellen Zustand.
settings-confirm-overwrite-workspace-title = Arbeitsfläche "{ $name }" überschreiben?
settings-sidebar-data-folder = Datenordner
settings-sidebar-settings-file = Einstellungsdatei

## Menubar
menu-about = Über
menu-application = Anwendung
menu-apply-layout = Layout anwenden
menu-apply-workspace = Arbeitsfläche anwenden
menu-chat = Chat
menu-close = Schließen
menu-console = Konsole
menu-design-mode = Entwurfsmodus
menu-discussions = Diskussionen
menu-empty-window = Leeres Fenster
menu-equalizer = Equalizer
menu-exit = Beenden
menu-hide-menubar = Menüleiste ausblenden
menu-import-workspace = Arbeitsfläche importieren...
menu-new-ellipsis = Neu...
menu-new-window = Neues Fenster
menu-new-window-from-layout = Neues Fenster aus Layout
menu-new-window-from-panel = Neues Fenster aus Panel
menu-no-layouts = Keine Layouts
menu-no-presets = Keine Vorlagen
menu-no-workspaces = Keine Arbeitsflächen
menu-os-decorations = Systemdekorationen
menu-overlay-shader = Overlay-Shader
menu-panel-built-in = Eingebaut
menu-panel-new = Neu...
menu-panel-no-layouts = Keine Layouts
menu-panel-no-presets = Keine Vorlagen
menu-panel-no-workspaces = Keine Arbeitsflächen
menu-panel-title = Menü
menu-panels = Panels
menu-panels-presets = Vorlagen
menu-pause = Pause
menu-playback = Wiedergabe
menu-remain-in-tray = Im Tray bleiben
menu-report-issue = Problem melden
menu-save-layout = Layout speichern
menu-save-workspace = Arbeitsfläche speichern
menu-section-add = Hinzufügen
menu-section-app = App
menu-section-interface = Oberfläche
menu-section-layouts = Layouts
menu-section-library = Bibliothek
menu-section-session = Sitzung
menu-section-track = Titel
menu-section-tuning = Abstimmung
menu-settings = Einstellungen
menu-signals = Signale
menu-song-theming = Songfarben
menu-stats = Statistiken
menu-tasks = Aufgaben
menu-update-available = Update verfügbar
menu-welcome = Willkommen
menu-window = Fenster
menu-workspace = Arbeitsfläche
menu-workspace-builtin-tag = Eingebaut

## Workspaces
workspace-apply-body = Das ersetzt das ganze Erscheinungsbild: Layouts, Palette, Aussehen.
workspace-apply-imported-body = Sie ist in deinen Arbeitsflächen gespeichert. Sie jetzt anzuwenden ersetzt das ganze Erscheinungsbild: Layouts, Palette, Aussehen.
workspace-apply-imported-title = "{ $name }" importiert
workspace-apply-screen-shader-named = Legt den Overlay-Shader { $name } über das ganze Fenster.
workspace-apply-screen-shader-plain = Legt einen Overlay-Shader über das ganze Fenster.
workspace-apply-shader-count = { $count ->
   *[other] Enthält { $count } Shader: { $names }
}
workspace-apply-shaders-approve-body = Sie freizugeben lässt sie auf diesem Rechner laufen. Ohne sie bleibt das Erscheinungsbild kahl, die Shader liegen aber weiter in seinem Pool.
workspace-apply-shaders-plain-body = Ohne sie bleibt das Erscheinungsbild kahl, die Shader liegen aber weiter in seinem Pool.
workspace-byline-author = von { $author }
workspace-byline-version = Version { $version }
workspace-context-add-panel = Panel hinzufügen
workspace-dialog-apply = Anwenden
workspace-dialog-apply-title = "{ $name }" anwenden?
workspace-dialog-approve-apply = Freigeben und anwenden
workspace-dialog-cancel = Abbrechen
workspace-dialog-close = Schließen
workspace-dialog-close-title = "{ $name }" schließen?
workspace-dialog-export = Exportieren
workspace-dialog-layout-name-placeholder = Layoutname
workspace-dialog-not-now = Jetzt nicht
workspace-dialog-overwrite = Überschreiben
workspace-dialog-overwrite-title = "{ $name }" überschreiben?
workspace-dialog-save = Speichern
workspace-dialog-save-layout-title = Layout speichern
workspace-dialog-save-workspace-title = Arbeitsfläche speichern
workspace-dialog-with-shaders = Mit Shadern
workspace-dialog-without-shaders = Ohne Shader
workspace-dialog-workspace-name-placeholder = Name der Arbeitsfläche
workspace-drop-add-queue = Zur Warteschlange
workspace-drop-play-now = Jetzt abspielen
workspace-hint-or = oder
workspace-hint-then = dann
workspace-import = Importieren
workspace-launcher-hint = Füge dein erstes Panel hinzu, um loszulegen, oder wähle eine Vorlage unter Arbeitsfläche > Arbeitsfläche anwenden
workspace-launcher-need-help = Brauchst du Hilfe?
workspace-launcher-open-welcome = Das Willkommensfenster öffnen
workspace-launcher-title = Ein leeres Fenster
workspace-layout-apply-body = Das ersetzt das aktuelle Layout dieses Fensters.
workspace-layout-overwrite-body = Das ersetzt das gespeicherte Layout durch das aktuelle.
workspace-layout-preset-restore-failed = Die Layout-Vorlage dieses Fensters ließ sich nicht wiederherstellen, also startet es leer.
workspace-layout-restore-failed = Das gespeicherte Layout ließ sich nicht wiederherstellen, also startet dieses Fenster leer.
workspace-mini-tip-back = Zurück zum vollen Layout
workspace-mini-tip-shrink = Auf den Mini-Player schrumpfen
workspace-overwrite-body = Das ersetzt die gespeicherte Arbeitsfläche durch das aktuelle Erscheinungsbild.
workspace-panel-locked-close-body = Dieses Panel ist festgeheftet. Es zu schließen nimmt es aus dem Layout.
workspace-save-current = Aktuelles speichern
workspace-screen-shader-hint-before = Schalte ihn jederzeit aus mit
workspace-workspace-restore-failed = Das Layout der Arbeitsfläche ließ sich nicht wiederherstellen, also startet dieses Fenster leer.

## Tasks window
tasks-acoustic-all-described = Alle { $count } eingelesenen Titel sind von { $label } beschrieben
tasks-acoustic-off = Beschreiben, wie Titel klingen, ist in den Einstellungen unter Bibliothek ausgeschaltet
tasks-acoustic-partial = { $label } beschreibt { $embedded } von { $total } eingelesenen Titeln
tasks-analyzing = Analysiere { $progress }
tasks-bake-writing = Schreibe Tags...
tasks-chip-count = { $count } Aufgaben
tasks-convert-starting = Starte ffmpeg...
tasks-converting = Wandle { $progress } um
tasks-count-of-total = { $done } von { $total }
tasks-embedding = Bette { $progress } ein
tasks-estimate-at = { $estimate } bei { $workers }
tasks-import-failed = Der letzte Import ist fehlgeschlagen: { $error }
tasks-import-reading = Lese die Liste der geliebten Titel...
tasks-import-unmatched = Für { $count } gab es keine Entsprechung in dieser Bibliothek
tasks-importing = Importiere { $progress }
tasks-job-acoustic = Akustische Analyse
tasks-job-convert = Audio umwandeln
tasks-job-loved-import = Geliebte Titel auf Last.fm
tasks-job-replaygain = ReplayGain
tasks-job-scan = Bibliotheksscan
tasks-job-tempo = Tempoanalyse
tasks-last-pass-stopped = Der letzte Durchgang hat gestoppt: { $reason }
tasks-last-run-finished = Letzter Lauf fertig, { $count } erledigt
tasks-last-run-stopped = Letzter Lauf nach { $count } gestoppt
tasks-library-busy = Die Bibliothek ist beschäftigt
tasks-library-scanning = Die Bibliothek wird eingelesen
tasks-measuring = Messe { $progress }
tasks-model-downloading = Ein Modell lädt noch herunter
tasks-no-library-window = Kein Bibliotheksfenster ist offen, also lässt sich das hier nicht starten
tasks-nothing-to-measure = Noch nichts eingelesen zum Messen
tasks-rg-all-gain = Alle { $count } Titel haben eine Verstärkung zum Spielen
tasks-rg-partial = { $missing ->
    [one] { $missing } von { $total } Titeln hat keine Verstärkung
   *[other] { $missing } von { $total } Titeln haben keine Verstärkung
}
tasks-scan-folder-count = { $count ->
    [one] { $count } Ordner
   *[other] { $count } Ordner
}
tasks-scan-last-scanned = { $folders }, zuletzt vor { $ago } gescannt
tasks-scan-never-scanned = { $folders }, nie gescannt
tasks-scan-no-folders = Noch keine Ordner hinzugefügt. Füge einen in den Einstellungen unter Bibliothek hinzu
tasks-start-analyze-missing = Fehlende analysieren
tasks-start-measure-missing = Fehlende messen
tasks-start-rescan = Neu einlesen
tasks-stop = Stopp
tasks-stopping = Stoppe...
tasks-tempo-all = Alle { $count } Titel haben ein Tempo
tasks-tempo-off = Ermitteln, wie schnell Titel laufen, ist in den Einstellungen unter Bibliothek ausgeschaltet
tasks-tempo-partial = { $missing ->
    [one] { $missing } von { $total } Titeln hat kein Tempo
   *[other] { $missing } von { $total } Titeln haben kein Tempo
}
tasks-timing = Zähle { $progress }
tasks-tip = Bibliotheksaufgaben öffnen
tasks-window-title = rox - Aufgaben
tasks-working-out-missing = Ermittle, was fehlt...

## Stats window
stats-bucket-listens = { $count ->
    [one] { $count } Hörvorgang, { $ago }
   *[other] { $count } Hörvorgänge, { $ago }
}
stats-chart-start-all = Erstes Hören
stats-chart-start-month = Vor 30 Tagen
stats-chart-start-week = Vor 7 Tagen
stats-chart-start-year = Vor einem Jahr
stats-click-opens = Klick öffnet Statistik
stats-click-section = Klick
stats-count-menu = Anzahl
    .description = Über welchen zurückliegenden Zeitraum die Zahl Hörvorgänge zählt; die Liste beim Überfahren zeigt immer alle
stats-empty-all = Noch nichts gehört
stats-empty-range = In diesem Zeitraum nichts gehört
stats-now = Jetzt
stats-open = Statistik öffnen
stats-open-on-click = Bei Klick Statistik öffnen
    .description = Auf das Widget klicken, um das Statistikfenster zu öffnen, den vollständigen Hörverlauf
stats-play-these-tracks = Diese Titel abspielen
stats-play-this-track = Diesen Titel abspielen
stats-plays-count = { $count ->
    [one] { $count } Wiedergabe
   *[other] { $count } Wiedergaben
}
stats-range-all = Gesamtzeit
stats-range-all-short = Gesamt
stats-range-day-short = Tag
stats-range-label = Zeitraum
stats-range-month = Dieser Monat
stats-range-month-short = Monat
stats-range-today = Heute
stats-range-week = Diese Woche
stats-range-week-short = Woche
stats-range-year = Dieses Jahr
stats-range-year-short = Jahr
stats-readout-section = Anzeige
stats-section-listens = Hörvorgänge
stats-section-listens-over-time = Hörvorgänge über die Zeit
stats-section-recent-listens = Zuletzt gehört
stats-section-top-albums = Top-Alben
stats-section-top-artists = Top-Interpreten
stats-section-top-genres = Top-Genres
stats-show-change = Veränderung anzeigen
    .description = Ein Chip dafür, wie der Zeitraum gegen den davor steht, hoch oder runter; vor Gesamtzeit liegt nichts
stats-show-number = Zahl anzeigen
    .description = Die Anzahl neben das Symbol zeichnen; ausgeschaltet bleibt ein nacktes Symbol, die Zahlen kommen beim Überfahren
stats-title = Statistik-Widget
stats-tooltip-listens = Hörvorgänge
stats-window-title = rox - Statistik

## About window
about-check-failed = GitHub war nicht erreichbar
about-check-for-updates = Nach Updates suchen
about-checking = Suche...
about-download = Herunterladen
about-downloading = Lade herunter... { $percent } %
about-get-it = Holen
about-license-lead = rox ist freie Software unter der GNU AGPLv3. Der Quelltext liegt auf
about-notice-lead = Diesem Programm sollte eine Kopie der Lizenz beiliegen. Falls nicht, siehe
about-release-notes = Versionshinweise
about-restart-now = Jetzt neu starten
about-up-to-date = Du hast die neueste Version
about-update-failed = Das Update ist fehlgeschlagen: { $error }
about-version = Version { $version }
about-version-available = Version { $version } ist verfügbar
about-version-ready = Version { $version } ist bereit
about-window-title = rox - Über

## Welcome window
welcome-add-folder = Ordner hinzufügen
welcome-and = und
welcome-back = Zurück
welcome-card-menubar-title = Menüleiste
welcome-card-music-title = Musik
welcome-card-panels-title = Panels
welcome-card-playback-title = Wiedergabe
welcome-card-rearranging-title = Umräumen
welcome-card-settings-title = Einstellungen
welcome-close = Schließen
welcome-design-mode-note = Umräumen braucht den Entwurfsmodus, standardmäßig an, oben in diesem Menü. Ausgeschaltet sperrt er das Layout, damit ein fertiger Aufbau nicht verrutscht.
welcome-done = Fertig
welcome-drop-note = Lass es auf der Kante eines Panels fallen, um dort zu teilen, in der Mitte für eine gemeinsame Reitergruppe, oder außerhalb des Fensters für ein eigenes Fenster.
welcome-key-left-click = Linksklick
welcome-key-middle-mouse = Mittlere Maustaste
welcome-layout-note = Speichere eine Anordnung als Layout; eine Arbeitsfläche bündelt Layouts und Palette zu einem teilbaren Erscheinungsbild.
welcome-menubar-after = zweimal, um sie oben zu lassen.
welcome-menubar-before = Bei versteckter Menüleiste halte
welcome-menubar-mid = und sie schwebt zurück über das Dock, oder tippe
welcome-music-note = rox liest ihn in die Bibliothek ein und die Dateien bleiben, wo sie sind. Weitere Ordner fügst du in den Einstellungen unter Bibliothek hinzu.
welcome-next = Weiter
welcome-or = oder
welcome-panels-note = Jede Fläche ist ein Panel, und das Panels-Menü der Menüleiste öffnet weitere.
welcome-playback-after = spulen.
welcome-playback-before = schaltet die Wiedergabe um;
welcome-quickplay-after = und er läuft.
welcome-quickplay-before = öffnet die Schnellwiedergabe: tippe einen Titel, drücke
welcome-rearrange-after = irgendwo im Panel, um es zu bewegen.
welcome-rearrange-before = Zieh einen Reiter, oder halte
welcome-settings-hint-after = öffnet die Einstellungen: Palette, Transparenz und Verhalten.
welcome-shelf-caption = Eines zu wählen ersetzt das Erscheinungsbild des Hauptfensters und schließt die Tour. Dieses Fenster gibt es jederzeit unter Anwendung > Willkommen.
welcome-stage-lead-quick-start = Wähle eine Arbeitsfläche, und das Hauptfenster wechselt zu ihr: Layouts, Palette, das ganze Erscheinungsbild.
welcome-stage-lead-welcome = Foobar, wenn es in 20XX gebaut worden wäre.
welcome-stage-title-quick-start = Schnellstart
welcome-stage-title-welcome = Willkommen bei rox
welcome-step-hint-after = , oder den Tasten unten.
welcome-step-hint-before = Blättere durch mit
welcome-tile-by = von { $author }
welcome-tour-intro = Eine kurze Tour, wo die Musik hereinkommt und wo das Erscheinungsbild eingestellt wird. Sie endet am Regal der mitgelieferten Arbeitsflächen, je ein Klick.
welcome-window-title = rox - Willkommen

## Console window
console-clear = Leeren
console-copy = Kopieren
console-empty-filtered = Nichts auf diesen Stufen
console-empty-none = Noch nichts protokolliert
console-filter-error = Fehler
console-filter-info = Info
console-filter-warn = Warnung
console-follow = Folgen
console-line-count = { $count ->
    [one] { $count } Zeile
   *[other] { $count } Zeilen
}
console-open-button = Konsole öffnen
console-reveal = Zeigen
console-window-title = rox - Konsole

## Signals window
signals-about-toggle = Über Signale
signals-blurb-marked = Panels, die in den Menüs damit markiert sind, lassen die meisten ihrer Parameter binden: Rechtsklick auf einen Parameter in den Panel-Einstellungen und ein Signal wählen, oder dort eines hinzufügen.
signals-blurb-shared = Was hier eingestellt wird, ist geteilt: eine Änderung gilt für jeden Parameter, der auf dieses Signal geroutet ist, in jedem Panel und jedem Fenster.
signals-blurb-total = Eine Summe ist die vierte Art: sie zählt ein anderes Signal über die Zeit zusammen und läuft bei 1 um, also klettert sie, solange die Musik laut ist, und steht still, solange sie es nicht ist. Nimm sie, wenn ein Shader eine Phase braucht, die sich mit dem Song bewegt statt mit der Uhr.
signals-blurb-what = Ein Signal macht aus dem, was läuft, eine Zahl zwischen 0 und 1: die Energie in einem Frequenzband, den Pegel der ganzen Mischung, oder einen Impuls bei jedem Schlag in einem Band. Ansprechverhalten setzt, wie schnell es folgt, Schwellwert stellt es unter einem Pegel still, den du wählst.
signals-no-library = Es ist kein Bibliotheksfenster offen, also zeigen diese kein Audio. Änderungen werden trotzdem gespeichert.
signals-window-title = rox - Signale

## Equaliser
eq-analyzer-bars = Balken
eq-analyzer-off = Keine Analyse
eq-analyzer-wave = Welle
eq-band-badge = Band-Plakette
    .description = Anzeigen, wie viele Bänder nicht flach stehen, auf einer Plakette über dem Symbol
eq-band-label = Band { $number }
eq-click-nothing = Nichts
eq-click-open = Öffnen
eq-click-section = Klick
    .description = Was ein Klick tut: das Equalizer-Fenster öffnen, oder die ganze Kurve an Ort und Stelle ein- und ausschalten
eq-click-toggle = Umschalten
eq-flatten = Flach stellen
eq-freq-label = Freq
eq-gain-label = Gain
eq-heading = Equalizer
eq-help-text = Zieh ein Band, um es zu bewegen, scroll darüber, um es breiter oder schmaler zu machen. Die Verarbeitung läuft vor dem Puffer, der die Soundkarte versorgt, also braucht eine Änderung bis zu einer halben Sekunde bis zu den Lautsprechern.
eq-hint-off = Klick zum Ausschalten
eq-hint-on = Klick zum Einschalten
eq-hint-open = Klick, um den Equalizer zu öffnen
eq-open = Equalizer öffnen
eq-readout-curve = Kurve
eq-readout-icon = Symbol
eq-readout-section = Anzeige
    .description = Das Symbol, der Frequenzgang als Sparkline, oder beides. Die Kurve braucht etwa fünfzig Pixel Breite, um lesbar zu sein
eq-reset-bands = Bänder zurücksetzen
eq-shape-active = { $count ->
    [one] { $count } Band nicht flach, Spitze { $peak } dB
   *[other] { $count } Bänder nicht flach, Spitze { $peak } dB
}
eq-shape-flat = Flach, jedes Band auf 0 dB
eq-status-off = Equalizer aus
eq-status-on = Equalizer an
eq-title = EQ-Widget
eq-widget-section = Widget
eq-width-label = Breite
eq-window-title = rox - Equalizer

## Keymap
keymap-close-window = Fenster schließen
    .description = Das vorderste Fenster schließen. Überall belegt, herausgelöste Panels eingeschlossen
keymap-decrease-font-size = Textgröße verkleinern
    .description = Die appweite Textgröße eine Stufe runter
keymap-focus-search = Suche fokussieren
    .description = Den Cursor ins Suchfeld der Bibliothek setzen
keymap-group-browsing = Navigation
keymap-group-editing = Bearbeiten
keymap-group-playback = Wiedergabe
keymap-group-view = Ansicht
keymap-group-windows = Fenster
keymap-increase-font-size = Textgröße vergrößern
    .description = Die appweite Textgröße eine Stufe hoch
keymap-key-backspace = Rücktaste
keymap-key-delete = Entf
keymap-key-down = Runter
keymap-key-end = Ende
keymap-key-esc = Esc
keymap-key-home = Pos1
keymap-key-insert = Einfg
keymap-key-left = Links
keymap-key-page-down = Bild ab
keymap-key-page-up = Bild auf
keymap-key-right = Rechts
keymap-key-space = Leertaste
keymap-key-tab = Tab
keymap-key-up = Hoch
keymap-mod-alt = Alt
keymap-mod-cmd = Cmd
keymap-mod-ctrl = Strg
keymap-mod-fn = Fn
keymap-mod-option = Option
keymap-mod-shift = Umschalt
keymap-mod-super = Super
keymap-mod-win = Win
keymap-new-window = Neues Fenster
    .description = Ein weiteres Arbeitsfenster mit dem gespeicherten Layout öffnen
keymap-next-track = Nächster Titel
    .description = Zum nächsten Titel in der Warteschlange springen
keymap-open-about = Über
    .description = Version und Mitwirkende anzeigen
keymap-open-console = Konsole
    .description = Das Protokollfenster öffnen
keymap-open-equalizer = Equalizer
    .description = Das Equalizer-Fenster öffnen
keymap-open-quick-play = Schnellwiedergabe
    .description = Die Such- und Abspielleiste über dem Fenster einblenden
keymap-open-settings = Einstellungen öffnen
    .description = Dieses Fenster öffnen
keymap-open-stats = Statistik öffnen
    .description = Das Fenster mit der Hörstatistik öffnen
keymap-open-tasks = Aufgaben
    .description = Anzeigen, woran rox im Hintergrund arbeitet
keymap-open-welcome = Willkommen
    .description = Das Willkommensfenster wieder öffnen
keymap-previous-track = Vorheriger Titel
    .description = Zurück zum vorherigen Titel
keymap-quit = Beenden
    .description = rox verlassen. Überall belegt, denn es gibt kein Fenster, in dem es nicht gelten sollte
keymap-reset-font-size = Textgröße zurücksetzen
    .description = Die Textgröße zurück auf den Standard setzen
keymap-seek-backward = Zurückspulen
    .description = Schrittweise durch den laufenden Titel zurück
keymap-seek-forward = Vorspulen
    .description = Schrittweise durch den laufenden Titel vor
keymap-stamp-line = Songtextzeile stempeln
    .description = Die laufende Position auf die gerade bearbeitete Songtextzeile schreiben
keymap-stop-playback = Stopp
    .description = Die Wiedergabe beenden und den Titel freigeben
keymap-toggle-playback = Wiedergabe / Pause
    .description = Den aktuellen Titel starten, oder ihn dort pausieren, wo er ist
keymap-toggle-post-shader = Overlay-Shader umschalten
    .description = Den Bildschirm-Shader aus- und einschalten. Überall belegt, denn ein Shader kann genau die Steuerelemente verdecken, mit denen du ihn sonst ausschalten würdest
keymap-toggle-zoom = Panel-Gruppe zoomen
    .description = Das Dock mit der zuletzt angeklickten Panel-Gruppe füllen, oder wieder heraus
keymap-type-ahead-next = Nächster Treffer der Schnellsuche
    .description = Zur nächsten Zeile springen, die zum Getippten passt
keymap-type-ahead-prev = Vorheriger Treffer der Schnellsuche
    .description = Zurück zum vorherigen Treffer des Getippten

## Panel catalog
panel-catalog-album-carousel = Album-Karussell
panel-catalog-artist-grid = Interpretenraster
panel-catalog-biography = Biografie
panel-catalog-cover-art = Cover
panel-catalog-drawer = Schublade
panel-catalog-eq-widget = EQ-Widget
panel-catalog-filter = Filter
panel-catalog-folder-tree = Ordnerbaum
panel-catalog-genre-grid = Genre-Raster
panel-catalog-group-application = Anwendung
panel-catalog-group-arrangement = Anordnung
panel-catalog-group-catalogue = Katalog
panel-catalog-group-controls = Steuerung
panel-catalog-group-details = Details
panel-catalog-group-experimental = Experimentell
panel-catalog-group-visualizers = Visualisierungen
panel-catalog-history = Verlauf
panel-catalog-menu = Menü
panel-catalog-metadata = Metadaten
panel-catalog-mini-toggle = Mini-Umschalter
panel-catalog-oscilloscope = Oszilloskop
panel-catalog-overlay = Overlay
panel-catalog-particles = Partikel
panel-catalog-playlists = Playlists
panel-catalog-queue = Warteschlange
panel-catalog-queue-widget = Warteschlangen-Widget
panel-catalog-seek = Position
panel-catalog-slide = Folie
panel-catalog-spectrogram = Spektrogramm
panel-catalog-spectrum = Spektrum
panel-catalog-stats-widget = Statistik-Widget
panel-catalog-status = Status
panel-catalog-theme-toggle = Farbschema-Umschalter
panel-catalog-track-info = Titelinfo
panel-catalog-vu-meter = VU-Meter
panel-catalog-waveform = Wellenform
panel-catalog-window-controls = Fenstersteuerung

## Updater
updater-already-latest = bereits auf der neuesten Version
updater-checksum-mismatch = die Prüfsumme des Downloads ist { $digest }, nicht { $expected }, wie das Release angibt
updater-checksum-missing-entry = { $sums } hat keinen Eintrag für { $name }; ein nicht prüfbarer Download wird abgelehnt
updater-no-asset = das Release hat kein { $name }
updater-no-checksums = das Release hat kein { $sums }; ein nicht prüfbarer Download wird abgelehnt
updater-no-release-build = kein Release-Build für diese Plattform
updater-overran = der Download lief über die Größe hinaus, die das Release angibt
updater-short = der Download stoppte bei { $done } von { $bytes } Bytes
updater-size-mismatch = der Server bot { $claimed } Bytes an, das Release gibt { $bytes } an

## Last.fm
lastfm-import-matching = Abgleich mit der Bibliothek
lastfm-import-read = { $count ->
    [one] { $count } geliebten Titel gelesen
   *[other] { $count } geliebte Titel gelesen
}
lastfm-import-stopped = { $count ->
    [one] Nach { $count } geliebtem Titel gestoppt
   *[other] Nach { $count } geliebten Titeln gestoppt
}
lastfm-import-matched = , { $count } zugeordnet
lastfm-import-added = , { $count } zu den Favoriten hinzugefügt

## Tag tools
tags-editor-clear-all = alle leeren
tags-editor-form-view = Formular
tags-editor-format-unsupported-all = Tags für dieses Format lassen sich noch nicht lesen oder schreiben.
tags-editor-format-unsupported-some = Einige dieser Dateien haben ein Format, dessen Tags sich noch nicht lesen oder schreiben lassen.
tags-editor-guess-button = Raten
tags-editor-guess-folded = { $status }, { $count } nicht gezeigt
tags-editor-guess-help = { $placeholders }; / passt auf den Ordner darüber, %skip% verwirft
tags-editor-guess-match-count = { $hits ->
    [one] { $hits } von { $total } passt
   *[other] { $hits } von { $total } passen
}
tags-editor-guess-no-match = kein Treffer
tags-editor-guess-pattern-label = Muster
tags-editor-loading = Lade Tags...
tags-editor-look-up = Nachschlagen
tags-editor-multiple-values = Mehrere Werte
tags-editor-clear-on-save = Wird beim Speichern geleert
tags-editor-other-tags = Weitere Tags ({ $count })
tags-editor-remove = entfernen
tags-editor-reveal = Zeigen
tags-editor-save-errors = { $count ->
    [one] { $count } Datei fehlgeschlagen; { $error }
   *[other] { $count } Dateien fehlgeschlagen; { $error }
}
tags-editor-saving-progress = Speichere { $done }/{ $total }...
tags-editor-table-view = Tabelle
tags-editor-tags-section = Tags
tags-editor-unknown-partial = { $count } von { $total }
tags-editor-unread-count = Die Tags von { $failed } von { $total } Dateien ließen sich nicht lesen
tags-editor-will-clear = wird geleert
tags-editor-will-remove = wird entfernt
tags-editor-window-title = rox - Tag-Editor
tags-guess-empty-segment = Muster ergibt einen leeren Ordner- oder Dateinamen
tags-guess-no-placeholders = keine Platzhalter
tags-guess-skip-renders-nothing = %skip% hat nichts zu erzeugen
tags-guess-unclosed = nicht geschlossenes %
tags-guess-unknown-placeholder = unbekannter Platzhalter %{ $name }%
tags-matcher-blocked-arm = Aktiviere ein Feld, um es zu übernehmen
tags-matcher-blocked-no-match = Kein Treffer zum Übernehmen
tags-matcher-blocked-pick = Einen Treffer wählen
tags-matcher-blocked-writing = Schreibe die Tags...
tags-matcher-match-count = { $count ->
    [one] 1 Treffer
   *[other] { $count } Treffer
}
tags-matcher-no-matches = Keine Treffer gefunden
tags-matcher-pick-match = Einen Treffer wählen
tags-matcher-search-failed = Suche fehlgeschlagen: { $error }
tags-matcher-searching = Suche...
tags-matcher-tagging = Tagge { $track }
tags-matcher-window-title = rox - Metadaten finden
tags-rename-blocked-cue = CUE-Titel, keine eigene Datei
tags-rename-blocked-duplicate = zwei Titel landen auf diesem Namen
tags-rename-blocked-occupied = dort liegt schon eine Datei
tags-rename-blocked-outside-roots = außerhalb jeder Bibliothekswurzel
tags-rename-blocked-unresolved = noch nicht im Katalog
tags-rename-move-error = { $name }: { $error }
tags-rename-move-errors = { $count ->
    [one] { $count } Datei fehlgeschlagen; { $error }
   *[other] { $count } Dateien fehlgeschlagen; { $error }
}
tags-rename-moving = Verschiebe { $done }/{ $total }...
tags-rename-nothing-to-move = Nichts zu verschieben
tags-rename-pattern-help = { $placeholders }; / macht einen Ordner, die Endung folgt der Datei
tags-rename-pattern-section = Muster
tags-rename-preview-section = Vorschau
tags-rename-unchanged = unverändert
tags-rename-will-move = { $count ->
    [one] { $count } von { $total } wird verschoben
   *[other] { $count } von { $total } werden verschoben
}
tags-rename-window-title = rox - Dateien umbenennen
tags-repair-affected-files = Betroffene Dateien
tags-repair-section = Reparatur
tags-repair-check-to-repair = Eine Datei ankreuzen, um sie zu reparieren
tags-repair-count = { $count ->
    [one] 1 Datei
   *[other] { $count } Dateien
}
tags-repair-count-so-far = { $count } bisher
tags-repair-label-scope = Umfang
tags-repair-no-affected = Keine betroffenen Dateien gefunden.
tags-repair-no-folder = Kein Ordner zum Scannen; füge einen zur Bibliothek hinzu oder wähle einen.
tags-repair-pick-folder = Ordner wählen...
tags-repair-progress = Repariere { $done }/{ $total }...
tags-repair-repair-button = { $count ->
    [0] Reparieren
   *[other] Reparieren ({ $count })
}
tags-repair-result = { $count ->
    [one] 1 Datei repariert
   *[other] { $count } Dateien repariert
}
tags-repair-result-failed = { $count } repariert, { $failed } fehlgeschlagen
tags-repair-scan-first = Erst scannen
tags-repair-scan-hint = Scannen, um Dateien mit Tag-Schäden zu finden, die ein Neuschreiben repariert.
tags-repair-select-all = Alle auswählen
tags-repair-select-none = Keine auswählen
tags-repair-whole-library = Ganze Bibliothek
tags-repair-window-title = rox - Tag-Reparatur

## Convert
convert-arg-names-file = "{ $token }" nennt eine Datei; das Ziel kommt aus Ordner und Muster
convert-section-output = Ausgabe
convert-section-preview = Vorschau
convert-arg-not-flag-or-value = "{ $token }" ist weder ein Flag noch ein Wert dafür
convert-check-wrote-nothing = ffmpeg ist sauber beendet, hat aber nichts geschrieben
convert-custom-ext-empty = Die Endung wählt den Container, also braucht es eine
convert-custom-ext-invalid = "{ $ext }" ist kein Containername; Buchstaben und Ziffern, kein Punkt
convert-dialog-browse = Durchsuchen...
convert-dialog-check-passed = ffmpeg hat damit einen Moment Stille kodiert, also laufen sie
convert-dialog-check-waiting = Wird gegen ffmpeg geprüft, sobald du aufhörst zu tippen
convert-dialog-checking = Prüfe mit ffmpeg...
convert-dialog-choose-folder = Ordner zum Hineinschreiben auswählen
convert-dialog-convert-button = Umwandeln
convert-dialog-custom-label = Eigen
convert-dialog-custom-menu-item = Eigen...
convert-dialog-custom-note = Argumente trennen an Leerzeichen, also keine Anführungszeichen; eingebettetes Cover wird bei eigenen Formaten nicht mitkopiert
convert-dialog-format-not-ready = Das getippte Format hat ffmpeg noch nicht bestanden
convert-dialog-label-extension = Endung
convert-dialog-label-format = Format
convert-dialog-label-into = in
convert-dialog-label-named = benannt
convert-dialog-mirror = Die Ordner der Bibliothek spiegeln
convert-dialog-nothing-to-convert = Nichts umzuwandeln: jede Zeile wird übersprungen
convert-dialog-pattern-help = { $placeholders }; / macht einen Ordner, das Format setzt die Endung
convert-dialog-pick-folder = Ordner zum Hineinschreiben wählen
convert-dialog-span-note = { $count } aus einem CUE-Image herausgeschnitten und aus der Bibliothek getaggt
convert-dialog-will-convert = { $count ->
    [one] { $count } von { $total } wird umgewandelt
   *[other] { $count } von { $total } werden umgewandelt
}
convert-dialog-window-title = rox - Umwandeln
convert-ffmpeg-silent-failure = ffmpeg ist fehlgeschlagen, ohne zu sagen warum
convert-flag-attach = -attach liest eine eigene Datei, was hier nicht erlaubt ist
convert-flag-f = Die Endung wählt den Container, also setzt du -f nicht selbst
convert-flag-i = Der Eingang ist der Titel, den du gewählt hast, also setzt du -i nicht selbst
convert-flag-n = -n ist bei jedem Lauf schon dabei
convert-flag-y = Hier überschreibt nichts, also gibt es kein -y; ein Ziel, das es schon gibt, wird übersprungen
convert-preset-flac = FLAC
convert-preset-mp3-320 = MP3 320 kbps
convert-preset-mp3-v0 = MP3 V0
convert-preset-opus-192 = Opus 192 kbps
convert-preset-wav = WAV
convert-skip-duplicate = zwei Titel landen auf diesem Namen
convert-skip-exists = schon vorhanden
convert-summary-failed = , { $count } fehlgeschlagen
convert-summary-files = { $count ->
    [one] 1 Datei
   *[other] { $count } Dateien
}
convert-summary-line = { $files } nach { $dest }
convert-summary-skipped = , { $count } übersprungen
convert-summary-stopped = Gestoppt, { $files } nach { $dest }
convert-version-answered = { $binary } lief, meldete aber keine Version

## Duplicates
duplicates-auto-select = Automatisch auswählen
duplicates-check-to-trash = Kopien ankreuzen, um sie in den Papierkorb zu legen
duplicates-copy-count = { $count ->
    [one] 2 Kopien
   *[other] { $count } Kopien
}
duplicates-different-albums = verschiedene Alben
duplicates-filter-placeholder = Nach Titel, Interpret oder Ordner filtern
duplicates-groups-summary = { $groups ->
    [one] 1 Gruppe, { $extras ->
        [one] { $extras } zusätzliche Kopie
       *[other] { $extras } zusätzliche Kopien
    }
   *[other] { $groups } Gruppen, { $extras ->
        [one] { $extras } zusätzliche Kopie
       *[other] { $extras } zusätzliche Kopien
    }
}
duplicates-library-loading = Die Bibliothek lädt noch; versuch es gleich nochmal.
duplicates-no-duplicates = Keine Duplikate gefunden.
duplicates-no-filter-matches = Keine Gruppe passt zum Filter.
duplicates-policy-newest = Neueste behalten
duplicates-policy-oldest = Älteste behalten
duplicates-policy-quality = Beste Qualität behalten
duplicates-scan-hint = Die Bibliothek nach Titeln durchsuchen, die mehr als einmal vorkommen.
duplicates-select-none = Keine auswählen
duplicates-selected-count = { $count } ausgewählt
duplicates-trash-button = { $count ->
    [0] In den Papierkorb
   *[other] In den Papierkorb ({ $count })
}
duplicates-trash-error = { $name }: { $error }
duplicates-trash-result = { $count ->
    [one] 1 Datei in den Papierkorb verschoben
   *[other] { $count } Dateien in den Papierkorb verschoben
}
duplicates-trash-result-failed = { $count } in den Papierkorb verschoben, { $failed } fehlgeschlagen
duplicates-trashing = Verschiebe { $done }/{ $total } in den Papierkorb...
duplicates-window-title = rox - Duplikate

## Smart playlists
smart-playlist-descending = Absteigend
smart-playlist-edit-title = Intelligente Playlist bearbeiten
smart-playlist-limit-label = Limit
smart-playlist-limit-placeholder = Kein Limit
smart-playlist-match-count = { $count ->
    [one] 1 Titel passt
   *[other] { $count } Titel passen
}
smart-playlist-matched-tracks = Passende Titel
smart-playlist-new-title = Neue intelligente Playlist
smart-playlist-no-matches = Keine Titel passen
smart-playlist-query-label = Suche
smart-playlist-sort-default = Standardreihenfolge
smart-playlist-sort-added = Hinzugefügt
smart-playlist-sort-label = Sortierung
smart-playlist-unknown-field = "{ $field }:" ist kein Feld, also passt der Begriff als reiner Text
smart-playlist-window-title = rox - { $verb }

## Playlist creation
playlist-create-not-savable = Benenne die Playlist, um sie zu speichern
playlist-create-placeholder = Playlistname
playlist-create-rename-title = Playlist umbenennen
playlist-create-title = Neue Playlist
playlist-create-window-title = rox - { $verb }

## Cover tools
cover-art-back = Rückseite
cover-art-disc = CD
cover-art-front = Vorderseite
cover-artwork = Bild
    .description = Welches Bild gezeigt wird; ein Slot, den die Datei nicht hat, fällt auf die Vorderseite zurück
cover-disc-style = CD-Stil
    .description = Das Bild als CD oder als Etikett einer Schallplatte darstellen
cover-disc-off = Aus
cover-disc-cd = CD
cover-disc-vinyl = Schallplatte
cover-editor-choose-image = Bild wählen
cover-editor-multiple = Mehrere
cover-editor-none = Keins
cover-editor-not-an-image = Diese Datei ist kein Bild, das rox einbetten kann
cover-editor-not-decoded = Dieses Bild ließ sich nicht dekodieren
cover-editor-reading = Lese das aktuelle Cover...
cover-editor-remove = Entfernen
cover-editor-replace = Ersetzen
cover-editor-revert = Zurücknehmen
cover-editor-save-errors = { $count ->
    [one] { $count } Datei fehlgeschlagen; { $error }
   *[other] { $count } Dateien fehlgeschlagen; { $error }
}
cover-editor-saving-progress = Speichere { $done }/{ $total }...
cover-editor-search-online = Online suchen
cover-editor-section = Cover
cover-editor-slot-back = Rückseite
cover-editor-slot-front = Vorderseite
cover-editor-slot-media = Medium
cover-editor-will-remove = Wird entfernt
cover-editor-window-title = rox - Cover
cover-matcher-blocked-fetching = Hole das volle Bild...
cover-matcher-blocked-no-cover = Kein Cover zum Setzen
cover-matcher-blocked-pick = Ein Cover wählen, um es zu setzen
cover-matcher-cover-count = { $count ->
    [one] 1 Cover
   *[other] { $count } Cover
}
cover-matcher-editor-closed = Der Cover-Editor wurde geschlossen
cover-matcher-no-covers = Keine Cover gefunden
cover-matcher-search-failed = Suche fehlgeschlagen: { $error }
cover-matcher-set-cover = Cover setzen
cover-matcher-setting = Setze...
cover-matcher-tile-info = { $provider }  { $width }px
cover-matcher-unsupported-format = Nicht unterstütztes Bildformat
cover-matcher-window-title = rox - Cover finden
cover-spin = Drehen
    .description = Die CD drehen, während ein Titel läuft; gilt für den CD-Slot oder einen CD-Stil
cover-spin-disc = CD drehen
cover-spin-ramp = Anlauf
    .description = Wie lange die CD braucht, um auf volle Drehzahl zu kommen, und um wieder auszulaufen
cover-spin-speed = Drehzahl
    .description = Volle Drehzahl, in Umdrehungen pro Minute
cover-stretch = Strecken
    .description = Das Panel füllen, das Seitenverhältnis des Bildes ignorieren
cover-stretch-to-fill = Zum Füllen strecken
cover-title = Cover

## Lyrics
lyrics-always-centered = Immer zentriert
    .description = Die Enden auffüllen, damit auch erste und letzte Zeile mittig stehen können
lyrics-auto-search = Automatisch suchen
    .description = Bei einem Titel ohne Text online suchen und einen sicheren Treffer speichern, ohne Auswahl
lyrics-bold = Fett
lyrics-build-word-by-word = Wort für Wort aufbauen
    .description = Wörter zeigen, wie sie gesungen werden, im Karaoke-Stil; ungesungene Zeilen bleiben verborgen
lyrics-edge-bottom = Unten
lyrics-edge-top = Oben
lyrics-edit-hint-after-stamp = zum Stempeln
lyrics-edit-hint-or = oder
lyrics-edit-loading = Lade das Blatt...
lyrics-edit-lyrics = Songtext bearbeiten
lyrics-edit-saving = Speichere...
lyrics-edit-section = Songtext
lyrics-edit-stamp = Stempeln
lyrics-edit-stamp-time = { $time } stempeln
lyrics-edit-window-title = rox - Songtext bearbeiten
lyrics-fade-lines-in = Zeilen einblenden
    .description = Eine Zeile aus dem Dunklen hochblenden, sobald sie die aktive wird
lyrics-falloff-edge = Abdunkelseite
    .description = Welche Seite der aktiven Zeile abgedunkelt wird
lyrics-find-online = Songtext online finden...
lyrics-follow-playback = Wiedergabe folgen
    .description = Die aktive Zeile in die Mitte gleiten lassen, während ein synchrones Blatt läuft
lyrics-font = Schrift
    .description = Die Schrift des Songtexts; Standard folgt der App-Schrift
lyrics-gap-threshold = Lücken-Schwellwert
    .description = Wie lange ein Intro oder eine Lücke laufen muss, bevor es eine Pause gibt
lyrics-lead-in-rest = Pause vorm Einsatz
    .description = Vor einem langen Intro eine leere Pause zeigen, damit die erste Zeile einblendet, wenn sie kommt
lyrics-line-falloff = Abdunklung
    .description = Wie stark jede Zeile pro Schritt weg von der aktiven abdunkelt
lyrics-line-spacing = Zeilenabstand
    .description = Wie weit die synchronen Zeilen auseinanderliegen, als Vielfaches der Textgröße
lyrics-look-again = Erneut suchen
lyrics-mark-dots = Punkte
lyrics-mark-note = Note
lyrics-marked-notice = Als ohne Songtext markiert
lyrics-matcher-blocked-no-match = Kein Treffer zum Übernehmen
lyrics-matcher-blocked-pick = Einen Treffer zum Übernehmen wählen
lyrics-matcher-blocked-saving = Speichere den Text...
lyrics-matcher-match-count = { $count ->
    [one] 1 Treffer
   *[other] { $count } Treffer
}
lyrics-matcher-no-query = Dieser Titel hat keinen Interpreten und Titel zum Abgleichen
lyrics-matcher-pick-preview = Einen Treffer für die Vorschau wählen
lyrics-matcher-search-failed = Suche fehlgeschlagen: { $error }
lyrics-matcher-synced-tag = { $provider }  synchron
lyrics-matcher-window-title = rox - Songtext finden
lyrics-no-lyrics-notice = Kein Songtext
lyrics-no-lyrics-track = Kein Songtext für diesen Titel
lyrics-rest-in-gaps = Pause in Lücken
    .description = Bei einer langen Instrumentallücke auf eine leere Pause wechseln, statt die letzte Zeile zu halten
lyrics-rest-marker = Pausenzeichen
    .description = Was eine wortlose Zeile in einem synchronen Blatt zeigt, die Lücken und Leerzeilen
lyrics-search-button = Online-Suchtaste
    .description = Die Suchtaste auf der leeren Fläche zeigen; das Rechtsklick-Menü findet Songtexte trotzdem
lyrics-search-online = Online suchen
lyrics-show-song-name = Songnamen anzeigen
    .description = Den Namen des Titels auf der leeren Fläche zeigen, über der Zeile ohne Songtext
lyrics-text-size = Textgröße
    .description = Der Songtext; die Zeilenhöhe der synchronen Anzeige folgt ihm
lyrics-title = Songtext
lyrics-title-unsynced = Titel über unsynchronem Blatt
    .description = Den Titel des Stücks über ein unsynchrones Blatt heften, damit ein kurzes Panel ihn noch zeigt
lyrics-wipe-lyrics = Songtext löschen

## Analysis passes
pass-acoustic-body = { $model } ermittelt, wie jeder Titel klingt, damit die Bibliothek Musik finden kann, die dem Laufenden ähnelt. Alles läuft auf diesem Rechner, und schon Beschriebenes wird übersprungen. { $lands }
pass-acoustic-lands-database = Die Ergebnisse landen in der Bibliotheksdatenbank, deine Dateien bleiben unangetastet.
pass-acoustic-lands-tags = Die Ergebnisse landen in der Bibliotheksdatenbank und bei MP3 und FLAC zusätzlich in den Tags jeder Datei, damit sie einen Neuaufbau der Datenbank überstehen. Andere Formate behalten nur die Kopie in der Datenbank.
pass-acoustic-title = { $count ->
    [one] 1 Titel analysieren?
   *[other] { $count } Titel analysieren?
}
pass-analyze = Analysieren
pass-estimate-at = { $estimate } bei { $workers_phrase }.
pass-estimate-button = Schätzen
pass-estimating = Schätze...
pass-measure = Messen
pass-no-estimate = Auf diesem Rechner lief noch nichts, also gibt es keine Schätzung. Schätzen misst ein paar Titel und rechnet den Rest daraus hoch.
pass-replaygain-body = Jede Datei wird dekodiert und gemessen, damit sie in der Lautheit läuft, auf die sie gemastert wurde. Alben werden als Ganzes gemessen, wenn allen ihren Titeln eine Verstärkung fehlt. { $lands }
pass-replaygain-lands-database = Die Werte landen in der Bibliotheksdatenbank, deine Dateien bleiben unangetastet.
pass-replaygain-lands-tags = Die Werte werden zurück in die Tags jeder Datei geschrieben, wo jeder andere Player sie liest.
pass-replaygain-title = { $count ->
    [one] 1 Titel messen?
   *[other] { $count } Titel messen?
}
pass-tempo-body = Zwei halbminütige Fenster jeder Datei werden dekodiert und die Schläge gezählt, damit die Bibliothek zeigen kann, wie schnell ein Titel läuft. Am besten klappt das bei Musik, die zum Klick eingespielt wurde; was sich nicht messen lässt, wird übersprungen. Die Werte landen in der Bibliotheksdatenbank, deine Dateien bleiben unangetastet.
pass-tempo-title = { $count ->
    [one] Das Tempo von 1 Titel finden?
   *[other] Das Tempo von { $count } Titeln finden?
}
pass-timing = Messe ein paar Titel...
pass-timing-failed = Zeitmessung für diese Bibliothek fehlgeschlagen: { $error }
pass-workers = Arbeitsprozesse

## Quick play
quick-play-comfortable-rows = Luftige Zeilen
    .description = Jedem Treffer mehr Höhe geben
quick-play-cover = Cover
    .description = Ein Cover-Miniaturbild links von jedem Treffer zeigen
quick-play-duration = Dauer
    .description = Die Länge jedes Treffers rechts zeigen
quick-play-narrow-by = Eingrenzen nach
quick-play-search-placeholder = Bibliothek durchsuchen
quick-play-subtitle = Unterzeile
    .description = Interpret und Album unter jedem Treffer zeigen
quick-play-tag-album = Album
quick-play-tag-artist = Interpret

## Drawer panel
drawer-add-tooltip = Schubladen-Panel hinzufügen
drawer-answers = Reagiert auf
    .description = Welche Auswahl die Schublade öffnet: nur das eigene Hauptpanel, oder jedes Panel außerhalb
drawer-dim = Abdunkeln
    .description = Wie stark das Hauptpanel hinter der offenen Schublade abdunkelt
drawer-edge = Kante
    .description = Die Kante, an der die Schublade liegt und aus der sie herausfährt
drawer-edge-bottom = Unten
drawer-edge-top = Oben
drawer-handle = Griff
    .description = Den Griff an der Panelkante zeigen. Versteckt zeigt sich nichts von der Schublade, bis etwas gewählt wird, und der Griff bleibt dann, solange die Auswahl hält, damit eine zugeklappte Schublade wieder herausgezogen werden kann
drawer-open-on = Öffnen bei
    .description = Verweilen auf dem Griff öffnet die Schublade immer; Auswahl nimmt eine Wahl im Hauptpanel dazu
drawer-pin-open = Offen anheften
drawer-reveal = Auszug
    .description = Wie viel vom Panel die offene Schublade bedeckt
drawer-scope-elsewhere = Anderswo
drawer-scope-main = Hauptpanel
drawer-title = Schublade
drawer-trigger-hover = Überfahren
drawer-trigger-selection = Auswahl

## Mini player
mini-tip-back = Zurück zum vollen Layout
mini-tip-none = Kein Mini-Layout zugewiesen
mini-tip-shrink = Auf den Mini-Player schrumpfen
mini-title = Mini-Umschalter

## System tray
tray-open = Öffnen
tray-pause = Pause
tray-play = Abspielen
tray-quit = Beenden

## Window controls
window-controls-mini-toggle = Mini-Umschalter
    .description = Mit dem Mini-Layout-Umschalter beginnen; erscheint, sobald ein Mini-Layout zugewiesen ist
window-controls-minimize = Minimieren
window-controls-style = Stil
    .description = Flache Symbole, oder die macOS-Ampeln
window-controls-style-icons = Symbole
window-controls-title = Fenstersteuerung
window-controls-traffic-lights = Ampeln

## Section names the audio panels share. Each one heads the same kind of
## rows in the spectrum, VU, oscilloscope, spectrogram and particles
## settings, so they're defined once rather than per panel.
viz-section-analysis = Analyse
viz-section-color = Farbe
viz-section-peaks = Spitzen
viz-section-playback = Wiedergabe
viz-section-scale = Skala
viz-section-signal = Signal

## Particles panel
particles-add-emitter = Emitter hinzufügen
particles-aim = Zielrichtung
particles-aim-fixed = Fest
particles-aim-outward = Nach außen
particles-burst = Stoß
particles-color = Farbe
particles-cone = Kegel
particles-direction = Richtung
    .description = Wohin es zieht; 0 ist oben, 180 ist unten
particles-drag = Luftwiderstand
    .description = Wie viel Tempo die Luft pro Sekunde frisst; null ist Vakuum
particles-drift = Drift
    .description = Wie schnell sich das Feld selbst bewegt, damit die Wirbel nicht stillstehen
particles-edit-emitters = Emitter bearbeiten
particles-emitter-label = Emitter { $index }
particles-emitter-target = Emitter { $index } { $target }
particles-emitters-empty = Noch keine Emitter. Füge einen hinzu, um das Feld zu starten.
particles-glow = Leuchten
    .description = Einen weichen Schein hinter jedes Partikel legen
particles-gravity = Schwerkraft
particles-gravity-strength = Stärke
    .description = Konstanter Zug auf alles, was fliegt
particles-height = Höhe
particles-hold-on-pause = Bei Pause halten
    .description = Das Feld während der Pause einfrieren, statt es davontreiben zu lassen
particles-length = Länge
particles-lifetime = Lebensdauer
particles-position-x = Position X
particles-position-y = Position Y
particles-radius = Radius
particles-rate = Rate
particles-rotation = Drehung
particles-round-particles = Runde Partikel
    .description = Punkte statt Quadrate zeichnen
particles-scale = Maßstab
    .description = Wie weit ein Wirbel reicht; klein brodelt, groß wälzt
particles-section-emitters = Emitter
particles-section-medium = Medium
particles-section-particles = Partikel
particles-shape = Form
particles-shape-box = Rechteck
particles-shape-line = Linie
particles-shape-point = Punkt
particles-shape-ring = Ring
particles-size = Größe
particles-speed = Tempo
particles-trigger = Auslöser
particles-trigger-continuous = Dauerhaft
particles-turbulence = Turbulenz
particles-turbulence-drift = Turbulenzdrift
particles-turbulence-scale = Turbulenzmaßstab
particles-turbulence-strength = Stärke
    .description = Wie stark das Feld die Partikel herumschiebt; null ist aus
particles-width = Breite

## Spectrum panel
spectrum-axis-labels = Achsenbeschriftung
    .description = Den Bereich über das Panel markieren: Oktaven (C1, C2, ...) oder Frequenzen (100, 1k, 10k)
spectrum-bar-gap = Balkenabstand
    .description = Platz zwischen den Balken, größere Abstände lassen weniger Balken zu
spectrum-bar-width = Balkenbreite
    .description = Wie dick jeder Balken zeichnet, dünnere Balken lassen mehr Bänder zu
spectrum-block-gap = Blockabstand
    .description = Die Naht zwischen den Zellen eines Stapels
spectrum-block-height = Blockhöhe
    .description = Wie hoch jede Zelle eines Stapels zeichnet
spectrum-cap-gravity = Fallgeschwindigkeit
    .description = Wie schnell die Spitzenmarken fallen, sobald das Band abfällt
spectrum-fft-size = FFT-Größe
    .description = Analysefenster; kurz reagiert schnell, lang löst feiner auf
spectrum-gradient-base-color = Grundfarbe
    .description = Das leise Ende des eigenen Verlaufs
spectrum-gradient-cover = Cover
spectrum-gradient-mode = Verlauf
    .description = Die Bänder nach Lautstärke einfärben: der Verlauf des Farbschemas, die Farben des Covers bei Songfarben, oder ein eigenes Paar
spectrum-gradient-theme = Farbschema
spectrum-gradient-tip-color = Spitzenfarbe
    .description = Das laute Ende des eigenen Verlaufs
spectrum-high-bound-description = Höchste Frequenz, die die Balken auswerten
spectrum-high-fft-size = Hohe FFT-Größe
    .description = Analysefenster für die Bänder oberhalb der Teilung
spectrum-hold-on-pause = Bei Pause halten
    .description = Die Balken während der Pause einfrieren, statt sie in die Stille fallen zu lassen
spectrum-labels-frequency = Frequenz
spectrum-labels-pitch = Tonhöhe
spectrum-low-bound-description = Tiefste Frequenz, die die Balken auswerten
spectrum-orientation = Ausrichtung
    .description = Die Kante, aus der die Bänder wachsen
spectrum-outline-bars = Balken umreißen
    .description = Jeden Balken als hohle Kontur zeichnen statt als gefüllten Verlauf
spectrum-outline-width = Konturbreite
    .description = Strichstärke der hohlen Balken
spectrum-peak-caps = Spitzenmarken
    .description = Eine Marke an der letzten Spitze jedes Bandes halten
spectrum-section-bands = Bänder
spectrum-split-at = Teilen bei
    .description = Wo die Zonen sich treffen, auf den nächsten Balken gerastet
spectrum-split-zones = Zonen teilen
    .description = Unter- und oberhalb einer Teilfrequenz mit verschiedenen Fenstergrößen analysieren
spectrum-style = Stil
    .description = Klassische Balken, Blöcke im LED-Stil, oder eine durchgehende Linie
spectrum-style-bars = Balken
spectrum-style-blocks = Blöcke
spectrum-style-line = Linie
spectrum-symmetry = Symmetrie
    .description = Das Spektrum um die Mitte falten; vorwärts legt die Tiefen an die Ränder, rückwärts treffen sie sich in der Mitte
spectrum-symmetry-forward = Vorwärts
spectrum-symmetry-reverse = Rückwärts

## Waveform panel
waveform-bar-gap = Balkenabstand
    .description = Platz zwischen den Balken, null verschmilzt sie zu einer durchgehenden Form
waveform-bar-width = Balkenbreite
    .description = Wie dick jeder Balken zeichnet
waveform-outline = Kontur
    .description = Die Balken nachzeichnen statt sie zu füllen; verschmolzene Balken lesen sich als eine Form
waveform-scrobble-marker = Scrobble-Marke
    .description = Eine dünne Linie dort, wo der Titel als zu Last.fm gescrobbelt gilt
waveform-split-channels = Kanäle trennen
    .description = Eine Zeile je Kanal, links über rechts; Mono-Titel bleiben eine einzelne Zeile
waveform-unavailable = Für diesen Titel gibt es keine Wellenform

## VU panel
vu-ballistics = Ballistik
    .description = VU integriert die Lautheit langsam; Spitze springt hoch und sinkt sanft ab
vu-ballistics-peak = Spitze
vu-cap-gravity = Fallgeschwindigkeit
    .description = Wie schnell die Spitzenmarken fallen, sobald die Anzeige abfällt
vu-channels = Kanäle
    .description = Das Stereopaar trennen, oder auf eine Anzeige falten
vu-channels-mono = Mono
vu-channels-stereo = Stereo
vu-db-scale = dB-Skala
    .description = Beschriftete Gitterlinien an den dB-Marken hinter den Anzeigen zeichnen
vu-gradient-mode = Verlauf
    .description = Die Anzeigen nach Pegel einfärben: der Verlauf des Farbschemas, die Farben des Covers bei Songfarben, oder ein eigenes Paar
vu-hold-on-pause = Bei Pause halten
    .description = Die Anzeigen während der Pause einfrieren, statt sie in die Stille fallen zu lassen
vu-orientation = Ausrichtung
    .description = Die Kante, aus der die Anzeigen wachsen
vu-peak-caps = Spitzenmarken
    .description = Eine Marke an der letzten Spitze jeder Anzeige halten
vu-section-meter = Anzeige
vu-segment-gap = Segmentabstand
    .description = Die Naht zwischen den Zellen eines Stapels
vu-segment-height = Segmenthöhe
    .description = Wie hoch jede Zelle eines Stapels zeichnet
vu-style = Stil
    .description = Eine durchgehende Säule, oder Segmente im LED-Stil
vu-style-continuous = Durchgehend
vu-style-segments = Segmente

## Spectrogram panel
spectrogram-ceiling = Decke
    .description = Pegel, der auf das helle Ende der Farbskala abgebildet wird, sodass alles Lautere dort hängen bleibt
spectrogram-colormap = Farbskala
    .description = Wie Lautheit auf Farbe abgebildet wird
spectrogram-colormap-cover = Cover
spectrogram-colormap-grayscale = Graustufen
spectrogram-colormap-ice = Eis
spectrogram-colormap-magma = Magma
spectrogram-colormap-theme = Farbschema
spectrogram-colormap-viridis = Viridis
spectrogram-direction = Richtung
    .description = Kante, aus der neue Spalten eintreten, was auch bestimmt, ob die Frequenzachse das Panel hinauf oder quer darüber verläuft
spectrogram-fft-size = FFT-Größe
    .description = Fenstergröße, mit der die Analyse läuft, ein Abwägen zwischen schnellem Reagieren einer Spalte auf einen Transienten und sauberer Trennung zweier tiefer Töne
spectrogram-floor = Boden
    .description = Pegel, der auf das dunkle Ende der Farbskala abgebildet wird, sodass alles Leisere als Hintergrund erscheint
spectrogram-grid = Raster
    .description = Frequenzlinien über dem Bild
spectrogram-high-bound = Obere Grenze
    .description = Oberes Ende der Frequenzachse, gedeckelt unter Nyquist, um die fast stillen obersten Oktaven wegzulassen
spectrogram-history = Verlauf
    .description = Wie viele Spalten das Panel behält, bevor die älteste herausscrollt
spectrogram-hold-on-pause = Bei Pause halten
    .description = Das stehende Bild bei Pause halten, statt Stille hineinzuscrollen
spectrogram-labels = Beschriftungen
    .description = Frequenzzahlen entlang der Skala, wo das Panel Platz dafür hat
spectrogram-log-scale = Log-Skala
    .description = Jeder Oktave denselben Platz geben, die musikalische Lesart, statt der gleichmäßigen Hz-Abstände eines Messgeräts
spectrogram-low-bound = Untere Grenze
    .description = Unteres Ende der Frequenzachse
spectrogram-section-picture = Bild
spectrogram-speed = Geschwindigkeit
    .description = Wie schnell das Bild scrollt, in Spalten pro Sekunde

## Oscilloscope panel

oscilloscope-channels = Kanäle
    .description = Zu einer Kurve zusammenfalten, beide übereinander legen, oder je einen eigenen Rahmen stapeln
oscilloscope-channels-mono = Mono
oscilloscope-channels-overlay = Overlay
oscilloscope-channels-split = Geteilt
oscilloscope-fill = Füllung
    .description = Eine sanfte Füllung zwischen der Kurve und der Mittellinie
oscilloscope-gain = Gain
    .description = Vertikaler Maßstab, um einen leisen Titel auf eine lesbare Kurve anzuheben
oscilloscope-gradient-mode = Verlauf
    .description = Die Kurve nach Auslenkung einfärben: der Verlauf des Farbschemas, die Farben des Covers bei Songfarben, oder ein eigenes Paar
oscilloscope-grid = Raster
    .description = Das Raster hinter der Kurve zeichnen
oscilloscope-hold-on-pause = Bei Pause halten
    .description = Das stehende Bild bei Pause halten, statt die Kurve flach fallen zu lassen
oscilloscope-line-width = Linienbreite
    .description = Wie dick die Kurve gezeichnet wird
oscilloscope-persistence = Persistenz
    .description = Wie lange vorherige Bilder hinter der Kurve nachleuchten, der Phosphor-Nachleuchteffekt
oscilloscope-section-trace = Kurve
oscilloscope-trigger = Trigger
    .description = Jeden Frame dort beginnen, wo das Signal den Triggerpegel kreuzt, damit periodisches Material stillsteht
oscilloscope-trigger-falling = Fallend
oscilloscope-trigger-level = Triggerpegel
    .description = Der Pegel, an dem nach dem Übergang gesucht wird
oscilloscope-trigger-off = Aus
oscilloscope-trigger-rising = Steigend
oscilloscope-window = Fenster
    .description = Wie viel Zeit die Kurve über das Panel hinweg abdeckt

## Shader panel
shader-panel-compile-error = Dieser Shader ließ sich nicht kompilieren:
shader-panel-compile-title = Dieser Shader ließ sich nicht kompilieren
shader-panel-enable = Einschalten
shader-panel-inspect = Ansehen
shader-panel-note-empty-body = Wähle ein Beispiel, oder zeige dem Panel eine .wgsl-Datei, die fs_user(uv) definiert.
shader-panel-note-empty-title = Kein Shader geladen.
shader-panel-note-missing-body = Dieses Panel verweist auf einen Shader, den die Arbeitsfläche nicht hat, also gibt es nichts auszuführen.
shader-panel-note-missing-title = { $name } ist nicht in den Shadern dieser Arbeitsfläche.
shader-panel-note-off-body = Die Quelle und ihre Bindungen sind noch da, sie laufen nur nicht.
shader-panel-note-off-title = Dieser Shader ist aus.
shader-panel-note-pending-body = Er kam mit einem Layout oder einer Arbeitsfläche statt von diesem Rechner, also bleibt er aus, bis du ihn geprüft hast.
shader-panel-note-pending-title = Dieser Shader wurde noch nicht gelesen.
shader-pending-origin-file = Soll aus { $path } stammen
shader-pending-origin-inline = Keine Datei dahinter; die Quelle kam mit dem Layout
shader-pending-more-lines = { $count ->
    [one] ... { $count } weitere Zeile
   *[other] ... { $count } weitere Zeilen
}
shader-eject-name-taken = { $name } hat bereits { $count } nummerierte Kopien in den Shadern dieser Arbeitsfläche
shader-eject-not-in-pool = { $name } ist nicht in den Shadern dieser Arbeitsfläche
shader-eject-failed = Auswerfen: { $error }
shader-panel-pick = Shader wählen
shader-panel-run-shader = Shader ausführen
    .description = Ausgeschaltet bleiben Quelle, Lesezeichen und Bindungen an Ort und Stelle, und nichts wird gezeichnet
shader-panel-section-routes = Routen

## Genre grid panel
genre-grid-clear-picked = Gewählte Genres leeren
genre-grid-desaturate = Bei Wiedergabe entsättigen
    .description = Jede Kachel außer der des laufenden Genres in Graustufen bringen; beim Überfahren kehrt die Farbe einer Kachel zurück
genre-grid-dim-while-playing = Bei Wiedergabe abdunkeln
    .description = Jede Kachel außer der des laufenden Genres verblassen lassen; beim Überfahren leuchtet eine Kachel wieder auf
genre-grid-follow-description = Zum laufenden Genre scrollen, sobald der Titel wechselt
genre-grid-merge-many = { $count } Genres in "{ $target }" zusammenführen
genre-grid-merge-one = "{ $source }" in "{ $target }" zusammenführen
genre-grid-pick-filters = Auswahl filtert die Bibliothek
    .description = Ein Klick auf ein Genre grenzt jedes Panel, das der gemeinsamen Suche folgt, darauf ein; ausgeschaltet bleibt der Klick eine einfache Auswahl
genre-grid-play-genres = { $count } Genres abspielen
genre-grid-resume-description = Zum laufenden Genre zurückgleiten, wenn du aufhörst zu stöbern
genre-grid-show-names = Namen anzeigen
    .description = Das Genre unter jede Kachel setzen statt nur beim Überfahren
genre-grid-smooth-description = Zum Genre gleiten statt zu springen
genre-grid-tally = { $albums ->
    [one] { $albums } Album, { $tracks } Titel
   *[other] { $albums } Alben, { $tracks } Titel
}
genre-grid-tile-face = Kachelbild
    .description = Was eine Kachel zeigt: die Albumcover des Genres, die Cover in der eigenen Farbe des Genres getönt, oder eine flache Farbkarte mit dem Namen darauf
genre-grid-unmerge = { $count ->
    [one] { $count } Wert trennen
   *[other] { $count } Werte trennen
}

## Artist grid panel
artist-grid-clear-picked = Gewählte Interpreten leeren
artist-grid-desaturate = Bei Wiedergabe entsättigen
    .description = Jede Kachel außer der des laufenden Interpreten in Graustufen bringen; beim Überfahren kehrt die Farbe einer Kachel zurück
artist-grid-dim-while-playing = Bei Wiedergabe abdunkeln
    .description = Jede Kachel außer der des laufenden Interpreten verblassen lassen; beim Überfahren leuchtet eine Kachel wieder auf
artist-grid-follow-description = Zum laufenden Interpreten scrollen, sobald der Titel wechselt
artist-grid-group-mode = Eine Kachel je
    .description = Der angegebene Album-Interpret hält die Gäste einer Platte bei dem Act, der sie veröffentlicht hat; der Titel-Interpret gibt jedem Feature eine eigene Kachel
artist-grid-pick-filters = Auswahl filtert die Bibliothek
    .description = Ein Klick auf einen Interpreten grenzt jedes Panel, das der gemeinsamen Suche folgt, darauf ein; ausgeschaltet bleibt der Klick eine einfache Auswahl
artist-grid-play-artists = { $count } Interpreten abspielen
artist-grid-portraits = Interpreten-Porträts
    .description = Das eigene Bild jedes Interpreten zeigen, einmal je Name nachgeschlagen und lokal gespeichert; ausgeschaltet erscheint das Cover des ersten Albums
artist-grid-resume-description = Zum laufenden Interpreten zurückgleiten, wenn du aufhörst zu stöbern
artist-grid-section-grouping = Gruppierung
artist-grid-show-names = Namen anzeigen
    .description = Den Interpreten unter jede Kachel setzen statt nur beim Überfahren
artist-grid-smooth-description = Zum Interpreten gleiten statt zu springen
artist-grid-tally = { $albums ->
    [one] { $albums } Album, { $tracks } Titel
   *[other] { $albums } Alben, { $tracks } Titel
}
artist-grid-track-artist = Titel-Interpret

## Wall panels
wall-dim-always = Immer
    .description = Die Kacheln auch dann zurücknehmen, wenn nichts läuft; nur eine überfahrene Kachel zeigt sich ganz
wall-dim-amount = Stärke
    .description = Wie weit die anderen Kacheln verblassen; 100 % blendet sie aus
wall-gap = Abstand
    .description = Platz zwischen den Kacheln
wall-name-alignment = Namensausrichtung
    .description = Die Beschriftungen unter ihren Kacheln ausrichten
wall-rounding = Rundung
    .description = Die Ecken jeder Kachel abrunden; 100 % ist ein Kreis
wall-section-picking = Auswählen
wall-show-counts = Anzahlen anzeigen
    .description = Die Zahl der Alben und Titel unter jedem Namen
wall-tile-size = Kachelgröße
    .description = Die längste Kante der Kacheln; Spalten teilen die Panelbreite gleichmäßig auf

## Metadata panel
metadata-cover-background = Cover-Hintergrund
    .description = Das Cover des Titels hinter den Feldern
metadata-display = Anzeige
    .description = Das Blatt mit dem Titel voran, oder eine flache Tabelle aus Feld und Wert von oben
metadata-display-sheet = Blatt
metadata-display-table = Tabelle
metadata-edit-save = Speichern
metadata-field-bit-depth = Bittiefe
metadata-field-bitrate = Bitrate
metadata-field-codec = Codec
metadata-field-comment = Kommentar
metadata-field-disc = CD
metadata-field-file = Datei
metadata-field-sample-rate = Abtastrate
metadata-field-track = Titel
metadata-fields = Felder
    .description = Welche Felder das Blatt auflistet; ein Feld, das der Titel nicht trägt, bleibt verborgen
metadata-find-online = Metadaten online suchen...
metadata-no-library = Keine Bibliothek
metadata-row-borders-description = Die Haarlinie unter jeder Zeile der Tabelle
metadata-source = Quelle
    .description = Dem folgen, was läuft oder ausgewählt ist, oder die Bibliothek als Ganzes lesen
metadata-stripes-description = Jede zweite Zeile der Tabelle tönen

## History panel
history-column-last-played = Zuletzt gespielt
history-descending = Absteigend
    .description = Die Sortierung rückwärts laufen lassen
history-empty-never = Jeder Titel wurde schon gespielt
history-empty-recent = Noch nichts gehört
history-headings = Die Liste der letzten Titel in Albenblöcke unterteilen; Ausgeklappt nimmt Cover und Zahlen dazu
history-sort-browse = Stöberreihenfolge
history-sort-date-added = Hinzugefügt am
history-sort-menu = Sortieren
    .description = Wie die nie gespielten Titel geordnet sind
history-title = Verlauf
history-view-most = Meistgespielt
history-view-never = Nie gespielt
history-view-recent = Kürzlich gespielt
history-view-recent-short = Kürzlich
history-view-row = Ansicht
    .description = Welchen Ausschnitt des Hörverlaufs das Panel zeigt

## Folder tree panel
folder-tree-clear-scope = Ordnerbereich leeren
folder-tree-collapse-all = Alle einklappen
folder-tree-collapse-branch = Zweig einklappen
folder-tree-cover-art = Cover
    .description = Das Albumcover statt des Zeilensymbols zeigen, bei Ordnern oder Songs
folder-tree-cover-folders = Ordner
folder-tree-cover-songs = Songs
folder-tree-empty = Noch keine Ordner in der Bibliothek
folder-tree-expand-branch = Zweig ausklappen
folder-tree-follow-description = Den laufenden Titel aufdecken und dorthin scrollen, sobald er wechselt
folder-tree-nonmatch-folders = Ordner ohne Treffer
    .description = Die Ordner ohne Treffer ausblenden, oder sie gedimmt lassen
folder-tree-nonmatch-songs = Songs ohne Treffer
    .description = In einem Ordner mit Treffer die übrigen Songs dimmen oder ausblenden
folder-tree-play-folder = Ordner abspielen
folder-tree-play-songs = { $count ->
    [one] Abspielen
   *[other] { $count } Songs abspielen
}
folder-tree-resume-description = Zum laufenden Titel zurückscrollen, wenn du aufhörst zu stöbern
folder-tree-scope-to-folder = Filter auf Ordner eingrenzen
folder-tree-smooth-description = Zum Titel gleiten statt zu springen
folder-tree-title = Baum

## Art panel
art-always = Die Cover auch dann zurücknehmen, wenn nichts läuft; nur ein überfahrenes Cover zeigt sich ganz
art-convert = Umwandeln...
art-covers-section = Cover
matcher-section-matches = Treffer
art-desaturate = Jedes Cover außer dem des laufenden Albums in Graustufen bringen; beim Überfahren kehrt die Farbe eines Covers zurück
art-dim-while-playing = Jedes Cover außer dem des laufenden Albums verblassen lassen; beim Überfahren leuchtet ein Cover wieder auf
art-disc-style = CD-Stil
    .description = Jedes Cover als CD oder als Etikett einer Schallplatte darstellen
art-edit-tags = Tags bearbeiten...
art-fill-panel = Panel füllen
    .description = Das zentrierte Cover allein an der Höhe des Panels bemessen (an der Breite, wenn vertikal); die seitlichen Cover laufen über die Kante hinaus, statt es zu schrumpfen
art-follow-description = Das laufende Album zentrieren, sobald der Titel wechselt
art-glow = Leuchten
    .description = Die Akzentfarbe hinter dem zentrierten Cover sammeln; mit eingeschalteten Songfarben nimmt sie die Farbe des laufenden Albums an
art-label-position = Beschriftungsposition
    .description = Wo die Albumbeschriftung sitzt: oben, unter dem Cover, am unteren Rand oder ausgeblendet
art-letter-rail = Buchstabenleiste
    .description = Die Initialen der Interpreten am Rand des Regals; ein Klick springt zum ersten Album des Buchstabens
art-layout-section = Layout
art-perspective = Perspektive
    .description = Die seitlichen Cover in echtem 3D drehen statt sie flach zu quetschen
art-reflections = Spiegelungen
    .description = Jedes Cover in den Boden unter dem Regal spiegeln
art-resume-description = Das laufende Album wieder zentrieren, wenn du aufhörst zu stöbern
art-shadows = Schatten
    .description = Ein weicher Schatten unter jedem Cover
art-smooth-description = Zum Album gleiten statt zu springen
art-title = Album-Karussell
art-vertical-layout = Vertikales Layout
    .description = Das Regal als Spalte stapeln, die hoch und runter scrollt, statt als Zeile

## Playlists panel
playlists-columns = Welche Titelspalten neben dem Titel erscheinen
playlists-delete = Playlist löschen
playlists-edit-query = Suche bearbeiten...
playlists-empty = Noch keine Playlists, füge Titel hinzu oder nutze Neue Playlist
playlists-headings = Die Titel jeder Playlist in Albenblöcke unterteilen; Ausgeklappt nimmt Cover und Zahlen dazu
playlists-import-tooltip = Playlist importieren
playlists-imported-fallback = Importiert
playlists-new = Neue Playlist...
playlists-new-smart = Neue intelligente Playlist...
playlists-refuse-drag-out = Titel in einer intelligenten Playlist lassen sich nicht herausziehen
playlists-refuse-edit-query = Bearbeite die Suche, um zu ändern, was eine intelligente Playlist enthält
playlists-refuse-smart-source = Eine intelligente Playlist bezieht ihre Titel aus ihrer Suche
playlists-remove = { $count ->
    [one] Aus Playlist entfernen
   *[other] { $count } aus Playlist entfernen
}
playlists-rename = Umbenennen...
playlists-title = Playlists

## Queue panel
queue-clear = Warteschlange leeren
queue-empty = Die Warteschlange ist leer
queue-headings = Die Warteschlange in Albenblöcke unterteilen; Ausgeklappt nimmt Cover und Zahlen dazu
queue-play-now = Jetzt abspielen
queue-remove = { $count ->
    [one] Aus Warteschlange entfernen
   *[other] { $count } aus Warteschlange entfernen
}
queue-title = Warteschlange
queue-widget-always-modal = Immer als Dialog öffnen
    .description = Die Warteschlange jedes Mal in einem Dialog öffnen, statt zu einem schon offenen Warteschlangen-Panel zu springen
queue-widget-clear-queue = Warteschlange leeren
queue-widget-more = { $count ->
    [one] +{ $count } weiterer
   *[other] +{ $count } weitere
}
queue-widget-open-on-click = Warteschlange bei Klick öffnen
    .description = Auf das Widget klicken, um zu einem offenen Warteschlangen-Panel zu springen, oder die Warteschlange in einem Fenster öffnen, wenn keines offen ist
queue-widget-section-click = Klick
queue-widget-title = Warteschlangen-Widget
queue-widget-up-next = Als Nächstes

## Biography panel
biography-background = Hintergrund
    .description = Die Fanart des Interpreten hinter dem Text, gedimmt und nach unten auslaufend
biography-fill-width = Breite füllen
    .description = Ein hohes Kopfbild über die volle Breite laufen lassen, statt es gedeckelt und zentriert zu setzen
biography-from-lastfm = Von Last.fm
biography-header-image = Kopfbild
    .description = Das breite Interpretenbanner ganz oben, oder das Porträt, wenn es kein Banner gibt
biography-keep-aspect = Seitenverhältnis behalten
    .description = Das Kopfbild in seinen eigenen Proportionen zeigen, statt es auf ein Band zuzuschneiden
biography-listeners-count = { $count } Hörer
biography-looking-up = Schlage { $name } nach
biography-no-artist-tag = Kein Interpreten-Tag
biography-no-text = Keine Biografie hinterlegt
biography-not-found = Nichts gefunden für { $name }
biography-plays-count = { $count } Wiedergaben
biography-refresh = Aktualisieren
biography-similar-artists = Ähnliche Interpreten
    .description = Verwandte Interpreten nach Hördaten, ganz unten
biography-similar-heading = Ähnliche Interpreten
biography-stats = Zahlen
    .description = Hörer und Wiedergaben auf Last.fm, unter dem Namen
biography-tags = Tags
    .description = Die Genre-Tags als Chip-Reihe
biography-title = Biografie

## Status panel
status-count-albums = { $count ->
    [one] 1 Album
   *[other] { $count } Alben
}
status-count-artists = { $count ->
    [one] 1 Interpret
   *[other] { $count } Interpreten
}
status-count-plays = { $count ->
    [one] 1 Wiedergabe
   *[other] { $count } Wiedergaben
}
status-count-selected = { $count } ausgewählt
status-count-tracks = { $count ->
    [one] 1 Titel
   *[other] { $count } Titel
}
status-readouts = Anzeigen
    .description = Entlang der Leiste ziehen zum Umordnen; zwischen die Zeilen ziehen, oder x und Plus eines Chips nutzen, zum Aus- und Einblenden
status-scope-selection = Auswahl
status-title = Status

## Output panel
output-detail-badge = Plakette
output-detail-compact = Kompakt
output-detail-expanded = Ausgeklappt
output-detail-label = Detail
    .description = Plakette hält es bei einem Chip, der Rest kommt beim Überfahren; kompakt gibt der Hauptzeile eine eigene Zeile, für eine Leiste an einer Kante; ausgeklappt nimmt die Gründe daneben dazu, oder darunter, wenn das Panel zu schmal ist
output-device-name = Gerätename
    .description = Das laufende Gerät in der Hauptzeile nennen; ausgeschaltet bleibt die Zeile bei Modus, Rate und Format
output-file-rate = Dateirate
    .description = Die eigene Rate der laufenden Datei bestätigen, wenn nichts sie neu abtastet. Eine Neuabtastung wird so oder so gemeldet, denn genau darum geht es bei der Warnung
output-mode-exclusive = Exklusiv
output-mode-shared = Geteilt
output-no-output = Keine Ausgabe
output-nothing-playing = Nichts läuft
output-pick-another-device = Wähle ein anderes Gerät, oder schalte Exklusiv aus
output-headline-numbers = { $rate } Hz, { $channels } Kan., { $format }
output-headline = { $mode }, { output-headline-numbers }
output-headline-device = { $mode } auf { $device }, { output-headline-numbers }
output-fell-back-to-shared = Exklusiv ist auf Geteilt zurückgefallen: { $why }
output-replaygain-levelling = ReplayGain gleicht diese Datei um { $db } dB aus
output-replaygain-short = ReplayGain { $db } dB
output-rate-resampled = Diese Datei hat { $rate } Hz und wird neu abgetastet, um das Gerät zu erreichen
output-rate-resampled-short = { $rate } Hz Datei neu abgetastet
output-rate-native = Diese Datei hat { $rate } Hz, es wird also nichts neu abgetastet
output-rate-native-short = { $rate } Hz Datei, keine Neuabtastung
output-start-track-hint = Starte einen Titel, um zu sehen, welches Format das Gerät angenommen hat
output-title = Ausgabe

## Track columns
columns-bits = Bits
columns-bpm = BPM
columns-codec = Codec
columns-cover = Cover
columns-fav = Fav
columns-gain = Gain
columns-kbps = Kbps
columns-khz = kHz
columns-name = Name
columns-number = Nummer
columns-scanned = Erfasst
columns-similar = Ähnlich

## Filter panel
filter-add-column = Spalte hinzufügen
filter-add-column-tooltip = Spalte hinzufügen
filter-all = Alle
filter-clear-filters = Filter leeren
filter-clear-selection = Auswahl leeren
filter-empty = Wähle ein Feld, um zu filtern
filter-remove-column = Spalte entfernen

## Search panel
search-chips-below = Darunter
search-chips-inline = In der Zeile
search-filter-chips = Filter-Chips
search-placeholder = Bibliothek durchsuchen

## Playback panel
playback-buttons = Tasten
    .description = Entlang der Leiste ziehen zum Umordnen; zwischen die Zeilen ziehen, oder x und Plus eines Chips nutzen, zum Aus- und Einblenden
playback-continue-down-list = Weiterspielen, die Liste hinunter
playback-continue-off = Weiterspielen aus
playback-continue-weighted = Weiterspielen, nie Gespieltes zuerst
playback-crossfade-inside-albums = Innerhalb von Alben
playback-crossfade-off = Überblenden aus
playback-crossfade-tip = Überblenden { $length }
playback-highlight-circle = Kreis
playback-highlight-square = Quadrat
playback-hold-draw = { $tip }. Halten, um zu wählen, was gezogen wird
playback-hold-length = { $tip }. Halten, um eine Länge zu wählen
playback-hold-order = { $tip }. Halten, um eine Reihenfolge zu wählen
playback-loop-off = Wiederholen aus
playback-loop-queue = Die Warteschlange wiederholen
playback-loop-track = Diesen Titel wiederholen
playback-menu-continue = Fortsetzen-Taste
playback-menu-crossfade = Überblenden-Taste
playback-menu-favourite = Favorit-Taste
playback-menu-random = Zufällig-Taste
playback-menu-rating = Bewertungssterne
playback-menu-stop = Stopp-Taste
playback-menu-stop-after = Stopp-danach-Taste
playback-menu-volume = Lautstärke-Taste
playback-pause = Pause
playback-play-highlight = Wiedergabe-Hervorhebung
    .description = Die Akzentfüllung der Wiedergabetaste: ein Kreis, ein weiches Quadrat, oder keine
playback-random-tip-random = Einen zufälligen Titel abspielen
playback-random-tip-similar = Einen Titel wie diesen abspielen
playback-seek-back-tip = 10 Sekunden zurück
playback-seek-forward-tip = 10 Sekunden vor
playback-shuffle-off = Zufall aus
playback-shuffle-on = Zufall an, Reihenfolge { $order }
playback-stop-after-armed = Stopp nach diesem Titel, vorgemerkt
playback-stop-after-tip = Stopp nach diesem Titel
playback-stop-tip = Stoppen und den Titel entladen
playback-volume-tip-muted = Ton an, { $percent } %. Rechtsklick für den Schieber
playback-volume-tip-unmuted = Stumm, { $percent } %. Rechtsklick für den Schieber

## Track info panel
track-info-color-output-chip = Ausgabe-Chip einfärben
    .description = Den Chip Warnfarben annehmen lassen, wenn die Ausgabe zurückfällt oder neu abtastet. Ausgeschaltet bleibt er immer im selben gedämpften Ton, und der Hinweis beim Überfahren erklärt den Zustand weiterhin
track-info-cycle-every = Wechsel alle
    .description = Wie lange jede Zeile steht, bevor sie überblendet
track-info-cycle-rows = Zeilen wechseln
    .description = Die Zeilen der Anordnung nacheinander in einer einzigen Zeile zeigen, mit Überblendung dazwischen; eine einzelne Zeile steht einfach für sich
track-info-delay = Verzögerung
    .description = Wie lange die Zeile an jedem Ende ruht, bevor sie weiterläuft
track-info-marquee = Lauftext
    .description = Was eine für das Panel zu lange Zeile macht: kriechen und zurückkehren, oder endlos umlaufen
track-info-menu-overflow = Überlauf
track-info-next = Als Nächstes: { $line }
track-info-opening = wird geöffnet...
track-info-output-fallback = Das Gerät hat die exklusive Ausgabe abgelehnt, also läuft die Wiedergabe über den gemeinsamen Mixer. Das Gerät meldete: { $reason }
track-info-output-resample-exclusive = Diese Datei hat { $source } kHz und die Karte nahm { $device } kHz, also wird jedes Sample auf dem Weg nach draußen umgewandelt. Das Gerät wollte nicht mit der eigenen Rate der Datei laufen.
track-info-output-resample-mixer = Diese Datei hat { $source } kHz und der Mixer läuft mit { $device } kHz, also wird jedes Sample auf dem Weg nach draußen umgewandelt. Der Exklusivmodus würde der Karte stattdessen die eigene Rate der Datei geben.
track-info-overflow-loop = Umlaufen
track-info-overflow-scroll = Pendeln
track-info-overflow-truncate = Abschneiden
track-info-queued-count = { $count } in Warteschlange
track-info-row-size = Größe von Zeile { $number }
track-info-speed = Tempo
    .description = Wie schnell die Zeile kriecht
track-info-text-size = Textgröße

## Seek panel
seek-ending = Restzeit
    .description = Die verbleibende Zeit herunterzählen oder die volle Länge zeigen
seek-ending-remaining = Verbleibend
seek-ending-total = Gesamt
seek-playhead = Abspielkopf
    .description = Die volle Höhe der Leiste überspannen oder sich an die Linie schmiegen
seek-playhead-full = Voll
seek-playhead-line = Linie
seek-playhead-max-height = Maximalhöhe des Abspielkopfs
    .description = Den vollen Abspielkopf deckeln, auf der Linie zentriert; 0 füllt das Panel
seek-playhead-width = Breite des Abspielkopfs
    .description = Die Breite der wandernden Positionsmarke
seek-rounding = Rundung
    .description = Der Eckenradius der Linie, bis hin zur Pille bei halber Dicke
seek-scrobble-marker = Scrobble-Marke
    .description = Eine dünne Linie dort, wo der Titel als zu Last.fm gescrobbelt gilt
seek-show-timings = Zeiten anzeigen
seek-thickness = Dicke
    .description = Die Höhe der Titellinie

## Volume panel
volume-pieces = Teile
    .description = Entlang der Leiste ziehen zum Umordnen; zwischen die Zeilen ziehen, oder x und Plus eines Chips nutzen, zum Aus- und Einblenden. Bei ausgeblendetem Prozentwert zeigt ihn der Hinweis am Lautsprecher
volume-readout = Anzeige
    .description = Den Pegel als Prozent zeigen oder als die Dezibel-Verstärkung, die er anlegt
volume-readout-decibels = Dezibel
volume-readout-percent = Prozent
volume-stretch = Strecken
    .description = Den Schieber das Panel füllen lassen, statt seine Breite zu deckeln
volume-tip-mute = Stumm
volume-tip-mute-level = Stumm, { $level }
volume-tip-unmute = Ton an
volume-tip-unmute-level = Ton an, { $level }

## Shared panel content
content-filter = Filter
content-no-track = Kein Titel
content-total-genres = Genres
content-total-time = Gesamtdauer

## Shared panel chrome
panel-columns-description = Welche Titelspalten erscheinen
panel-headings = Überschriften
panel-jump-to-playing = Zum laufenden Titel springen
panel-menu-display = Anzeige
panel-title-artists = Interpreten
panel-title-genres = Genres
panel-title-oscilloscope = Oszilloskop
panel-title-particles = Partikel
panel-title-playback = Wiedergabe
panel-title-seek = Position
panel-title-shader = Shader
panel-title-spectrogram = Spektrogramm
panel-title-spectrum = Spektrum
panel-title-theme-toggle = Farbschema-Umschalter
panel-title-track-info = Titelinfo
panel-title-volume = Lautstärke
panel-title-vu = VU-Meter
panel-title-waveform = Wellenform

## Everything else
choice-both = Beides
choice-dim = Abblenden
choice-hide = Ausblenden
composite-add-panel = Panel hinzufügen
composite-host-settings = Einstellungen für { $host }
composite-move-left = Nach links
composite-move-right = Nach rechts
composite-remove = Entfernen
composite-replace = Ersetzen
group-panel-add-slot = Slot hinzufügen
group-panel-move-down = Nach unten
group-panel-move-up = Nach oben
group-panel-remove-slot = Slot entfernen
group-panel-split-side-by-side = Nebeneinander teilen
group-panel-split-stacked = Übereinander teilen
group-panel-swap-panels = Panels tauschen
group-panel-title = Gruppe
overlay-dim = Abdunkeln
    .description = Wie stark das Hauptpanel unter dem eingeblendeten Overlay abdunkelt
overlay-title = Overlay
overlay-toggle = Overlay umschalten
shader-confirm-hint-after = schaltet den Shader von überall um.
shader-confirm-hint-before = Ein Shader kann Fenster schwer bedienbar machen. Zurücknehmen oder dieses Fenster schließen bringt alles zurück, wie es war.
shader-confirm-keep = Behalten
shader-confirm-question = Diesen Bildschirm-Shader behalten?
shader-confirm-revert = Zurücknehmen
shader-confirm-window-title = rox - Overlay-Shader
slide-add = Folie hinzufügen
slide-next = Nächste Folie
slide-previous = Vorherige Folie
slide-title = Folie
theme-toggle-to-dark = Zum dunklen Farbschema wechseln
theme-toggle-to-light = Zum hellen Farbschema wechseln
transport-favourite-add = Zu Favoriten hinzufügen
transport-favourite-nothing = Nichts zum Favorisieren
transport-favourite-remove = Aus Favoriten entfernen
transport-pieces = Teile
    .description = Entlang einer Zeile ziehen zum Umordnen und zwischen Zeilen zum Verschieben; x und Plus eines Chips blenden aus und ein

## Stragglers picked up in the final sweep

duplicates-scanning = Suche läuft...
about-copyright = Copyright © 2026
signal-name-placeholder = Signalname
signals-empty = Noch keine Signale. Füge eines hinzu, oder klicke mit rechts auf einen bindbaren Regler.
signal-add = Signal hinzufügen
panel-approve = Freigeben
panel-turn-off = Ausschalten
shader-from-file = Aus Datei...
arrange-add-row = Zeile hinzufügen
smart-playlist-name-placeholder = Playlistname
smart-playlist-name-to-save = Benenne die Playlist, um sie zu speichern
panel-new-playlist = Neue Playlist...
panel-edit-tags = Tags bearbeiten...
panel-edit-cover = Cover bearbeiten...
panel-rename-files = Dateien umbenennen...
panel-convert = Umwandeln...
panel-catalog-drag-anchor = Ziehanker
panel-catalog-spacer = Abstandhalter

## Duration and worker phrasing

pace-under-a-minute = unter einer Minute
pace-minutes = { $count ->
    [one] etwa eine Minute
   *[other] etwa { $count } Minuten
}
pace-hours = { $count ->
    [one] etwa eine Stunde
   *[other] etwa { $count } Stunden
}
pace-half-hours = etwa { $value } Stunden
pace-days = { $count ->
    [one] etwa ein Tag
   *[other] etwa { $count } Tage
}
pace-workers = { $count ->
    [one] { $count } Arbeitsprozess
   *[other] { $count } Arbeitsprozessen
}
tasks-rest-takes = , der Rest dauert { $estimate }
tasks-measuring-takes = , das Messen dauert { $estimate }
tasks-working-out-takes = , das Berechnen dauert { $estimate }
tasks-time-left = , noch { $left }
tasks-failed-suffix = ({ $count } fehlgeschlagen)
tasks-file-suffix = - { $file }
tasks-no-beat-suffix = ({ $count } ohne eindeutiges Tempo)
tasks-estimate-at-workers = ({ tasks-estimate-at })

## Panel vanity names

panel-title-art-view = Cover-Ansicht
panel-title-artist-grid = Interpretenraster
panel-title-genre-grid = Genre-Raster
panel-title-biography = Biografie
panel-title-cover-art = Cover
panel-title-drag-anchor = Ziehanker
panel-title-drawer = Schublade
panel-title-eq-widget = EQ-Widget
panel-title-filter = Filter
panel-title-folder-tree = Ordnerbaum
panel-title-group = Gruppe
panel-title-history = Verlauf
panel-title-lyrics = Songtext
panel-title-menu = Menü
panel-title-metadata = Metadaten
panel-title-mini-toggle = Mini-Umschalter
panel-title-output = Ausgabe
panel-title-overlay = Overlay
panel-title-playlists = Playlists
panel-title-queue = Warteschlange
panel-title-queue-widget = Warteschlangen-Widget
panel-title-search = Suche
panel-title-slide = Folie
panel-title-spacer = Abstandhalter
panel-title-stats-widget = Statistik-Widget
panel-title-vu-meter = VU-Meter
panel-title-window-controls = Fenstersteuerung

## Relative time and the output headline

ago-just-now = gerade eben
ago-minutes = vor { $count } Min.
ago-hours = vor { $count } Std.
ago-days = vor { $count } Tg.
ago-weeks = vor { $count } Wo.
ago-years = vor { $count } J.

span-seconds = { $count ->
    [one] { $count } Sekunde
   *[other] { $count } Sekunden
}
span-minutes = { $count ->
    [one] { $count } Minute
   *[other] { $count } Minuten
}
span-hours = { $count ->
    [one] { $count } Stunde
   *[other] { $count } Stunden
}
span-days = { $count ->
    [one] { $count } Tag
   *[other] { $count } Tage
}
span-weeks = { $count ->
    [one] { $count } Woche
   *[other] { $count } Wochen
}
span-years = { $count ->
    [one] { $count } Jahr
   *[other] { $count } Jahre
}
span-pair = { $first }, { $second }
unit-percent = { $value } %

settings-audio-output-headline = { $mode }{ $note } auf { $device }, { $rate } Hz, { $channels } Kan., { $format }
settings-audio-output-experimental =  (experimentell)

## ML model catalog

settings-mlmodels-description = { $summary }. { $dim } Werte pro Titel. { $licence }
settings-mlmodels-on-disk = , { $size } auf der Festplatte
settings-mlmodels-to-download = , { $size } zum Herunterladen
model-summary-dsp-timbre-1 = Eingebaut, kein Download. Eine Zusammenfassung der logarithmischen Bandenergie, der spektralen Form und der Onset-Rate jedes Titels. Grob neben einem trainierten Netz, aber es braucht nichts und läuft überall
model-summary-panns-cnn10 = Ein auf AudioSet trainiertes Faltungsnetz, das erkennt, was ein Geräusch ist. Seine 512-Werte-Beschreibung eines Titels ist weit reicher als die eingebaute Skizze, um den Preis eines 24-MB-Downloads und eines langsameren Analysedurchgangs

## Shipped workspaces

workspace-shipped-default = (Standard)
workspace-shipped-default-blurb = Wie rox von Haus aus aussieht: durchscheinende Flächen über dem Desktop, kein Fensterrahmen, keine Songfarben. Der Ausgangspunkt, von dem jeder andere Look hier abweicht.
workspace-shipped-catrox-blurb = Das foobar2000-Skin, mit dem alles anfing, neu gebaut: eine runde CD-Darstellung des Covers, die Metadatenfelder links untereinander und nach Album gruppierte Titel mit Bewertungspunkten.
workspace-shipped-critters-blurb = Die ganze App als 1-Bit-Druck: ein geordnetes Dither über jeder Fläche, Töne, die mit dem Subbass zusammenbrechen, und eine Rauschwand, die sich mit dem Lied windet. Nach Critters for Sale.
workspace-shipped-diffuse-blurb = Nur das laufende Album: Cover und Wiedergabekarte als eine Gruppe, die das Fenster füllt, transparente Flächen über dem Hintergrund, ohne Naht. Bibliothek, Warteschlange und Songtext warten in einer Schublade am rechten Rand und fahren über die Musik heraus, wenn der Griff überfahren wird. Monochrom, damit die Farbe von den Covern kommt.
workspace-shipped-foobar-blurb = Das Layout, mit dem dieses ganze Projekt streitet. Deckende Panels, Filterspalten für Interpret und Album, eine dichte Titeltabelle und die Menüleiste genau da, wo sie immer war.
workspace-shipped-llama-winamp-blurb = Winamp so, wie du es in Erinnerung hast, nicht so, wie es war. Tahoma, dunkel, kein Rahmen, ein gepunktetes Spektrum über der Breite und ein Shade-Modus im Mini-Layout.
workspace-shipped-metro-blurb = Flache Panels und bequeme Zeilen in Segoe UI, mit eingeschalteten Songfarben, sodass die ganze Palette dem laufenden Cover folgt.
workspace-shipped-phosphor-blurb = Alles dicktengleich. Consolas, grün auf schwarz, kein Cover in der Schnellwiedergabe: ein Terminal, das zufällig Musik spielt.
