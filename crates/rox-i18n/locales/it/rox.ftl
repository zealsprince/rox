### Italiano. Rispecchia en-CA/rox.ftl chiave per chiave; il test di
### parità in rox-i18n lo garantisce. Le chiavi sono in kebab-case con
### il prefisso della superficie; la descrizione di una riga è un
### attributo del messaggio dell'etichetta.

## Shared widgets
tracking-title = Tracciamento
tracking-follow = Segui la riproduzione
tracking-resume = Riprendi quando inattivo
tracking-smooth = Scorrimento fluido
align-row = Allineamento
    .description = Dove va il contenuto quando il pannello ha spazio in più
valign-row = Allineamento verticale
    .description = Dove va il contenuto quando il pannello ha altezza in più
valign-top = Alto
valign-middle = Centro
valign-bottom = Basso
letter-rail-compact = Barra compatta
    .description = Limita la barra a una sola riga che scorre invece di andare a capo

## Panel source and search rows
source-track = Traccia
    .description = Segui ciò che è in riproduzione, o ciò che è selezionato nella libreria
source-follow-playing = Segui la riproduzione
source-follow-selection = Segui la selezione
source-playing = In riproduzione
source-selected = Selezionato
query-search = Ricerca
query-search-box = Campo di ricerca
    .description = Mostra il campo di ricerca; la query vale solo finché è visibile
query-source = Sorgente di ricerca
    .description = Segui la query condivisa, filtra con il campo di questo pannello, oppure mostra ciò che un altro pannello ha selezionato
query-source-shared = Condivisa
query-source-own = Propria
query-source-selection = Selezione

## Signals and routes
signal-source = Sorgente
    .description = Cosa ascolta il segnale: Banda segue un intervallo di frequenze, Livello tutto il mix, Attacco pulsa a ogni colpo nell'intervallo, Trigger lancia un impulso quando l'intervallo raggiunge la soglia, Totale somma un altro segnale nel tempo
signal-kind-band = Banda
signal-kind-level = Livello
signal-kind-onset = Attacco
signal-kind-trigger = Trigger
signal-kind-total = Totale
signal-response = Risposta
signal-response-pulse = Quanto a lungo ogni impulso risuona prima di spegnersi
signal-response-drift = 0 scatta con la musica, 100 le va dietro
signal-threshold = Soglia
signal-threshold-trigger = Il livello che l'intervallo deve raggiungere per lanciare l'impulso; non riparte finché il livello non ricade sotto il segno sul misuratore sopra
signal-threshold-gate = Sotto questa soglia il segnale conta come zero; sopra, l'uscita risale da zero, così le parti silenziose non muovono la manopola. Il segno sul misuratore qui sopra indica dov'è
signal-low-bound = Limite inferiore
signal-high-bound = Limite superiore
signal-adds-up = Somma
    .description = Quale segnale somma questo totale; sale mentre quello sta alto e si ferma quando tace
signal-aggregate-nothing = Niente da seguire
signal-aggregate-pick = Scegli un segnale
signal-aggregate-alone = Non c'è nessun altro segnale nel pool da sommare, quindi resta a zero. Aggiungine uno e comparirà nella lista.
signal-aggregate-unpicked = Niente scelto, quindi questo totale resta a zero. Scegli un segnale qui sopra.
signal-rate = Frequenza
    .description = Giri al secondo a pieno ingresso; superato 1 riparte da 0 e continua a salire, e per uno shader è una fase
signal-reset-on-track = Azzera al cambio traccia
    .description = Torna a zero quando parte un nuovo brano, così una fase non riparte dal totale del brano precedente
signal-flush = Svuota
signal-routes-in-panel = { $count ->
    [one] { $count } route in questo pannello
   *[other] { $count } route in questo pannello
}
    .description = Riportalo a zero adesso; scende in un attimo invece che di colpo, così quello che lo segue non fa salti
route-header = Route
route-signal = Segnale
    .description = Quale segnale condiviso segue questa route; regolarlo qui regola ogni route che lo segue
route-new-signal = Nuovo segnale
route-shared-note = Condiviso da ogni route su questo segnale
route-signal-gone = Il segnale di questa route non c'è più; la manopola resta sul valore del cursore finché non ne scegli un altro qui sopra.
route-range-note = Intervallo solo per questo parametro
route-quiet = Silenzio
    .description = Quanto segna la manopola nel silenzio, come quota della sua impostazione
route-loud = Pieno
    .description = Quanto segna a pieno segnale; 100% è il valore del cursore stesso, sotto Silenzio modula verso il basso
route-slot = Slot
    .description = Quale dei sedici slot di segnale dello shader viene riempito da questa route
route-slot-quiet-description = Cosa segna lo slot nel silenzio
route-slot-loud-description = Cosa segna a pieno segnale; sotto Silenzio lo slot va al contrario
route-slot-signal-description = Quale segnale condiviso segue questa route
route-slot-signal-gone = Il segnale di questa route non c'è più; lo slot resta a zero finché non ne scegli un altro.
route-add = Aggiungi route
route-unrouted = Senza route
route-pick-slot = Scegli uno slot
route-pick-signal = Scegli un segnale
route-no-signal = nessun segnale
route-no-signals-yet = Non ci sono ancora segnali da seguire. Creane uno e comparirà qui; fino ad allora lo slot resta a zero.
route-open-signals = Apri i segnali
route-create-signal = Crea nuovo segnale

## Panel settings window
panel-settings = Impostazioni pannello
panel-menu-label = Pannello
panel-save-as-preset = Salva come preimpostazione
panel-rename = Rinomina
panel-rename-name = Nome
panel-rename-note = Mostrato come scheda del pannello; vuoto torna al nome integrato
panel-rename-hint-after = per rinominare
panel-was-closed = Il pannello è stato chiuso
panel-reset = Reimposta
panel-inverse = Inverti
panel-apply-song-theme = Applica i colori del brano
panel-page-appearance = Aspetto
panel-page-behavior = Comportamento
panel-page-shader = Shader
panel-section-placement = Posizionamento
panel-section-size = Dimensione
panel-section-opacity = Opacità
panel-section-frame = Cornice
panel-section-colors = Colori
panel-section-font = Carattere
panel-section-shader = Shader
panel-section-signals = Segnali
panel-section-slots = Slot
panel-awaiting-approval = In attesa di approvazione
panel-size-off = Off
panel-locked = Bloccato
    .description = Fissa il pannello al suo posto; non si può più trascinare né riordinare nel dock
panel-drag-anchor = Ancora di trascinamento
    .description = Un trascinamento ovunque sul pannello sposta la finestra, mentre i clic semplici arrivano comunque ai suoi controlli; per layout senza decorazioni
panel-slot-controls = Controlli degli slot
    .description = Mostra i pulsanti d'angolo per scambiare e rimuovere i pannelli ospitati qui. Nascosti, il layout si modifica comunque dall'albero nella pagina Spazio di lavoro nelle impostazioni
panel-min-width = Larghezza minima
    .description = Dove un ridimensionamento smette di stringere il pannello. Vale alla lettera, anche sotto il minimo del pannello stesso, così una striscia compatta può scendere più stretta che di serie; vuoto lascia il minimo com'è
panel-max-width = Larghezza massima
    .description = Limita la larghezza del pannello perché non si allunghi quando la finestra si allarga
panel-min-height = Altezza minima
    .description = Dove un ridimensionamento smette di schiacciare il pannello. Vale alla lettera, anche sotto il minimo del pannello stesso, così una striscia compatta può scendere più stretta che di serie; vuoto lascia il minimo com'è
panel-max-height = Altezza massima
    .description = Limita l'altezza del pannello perché non si allunghi quando la finestra si alza
panel-own-opacity = Opacità di superficie propria
    .description = Dai a questo pannello un'opacità sua sullo sfondo invece di quella dell'app
panel-surface-opacity = Opacità di superficie
panel-margin = Margine
    .description = Tira il pannello dentro dalla sua cella, con lo sfondo che traspare nello spazio
panel-padding = Spaziatura interna
    .description = Spazio dentro il bordo del pannello, tenuto nel suo stesso sfondo
panel-rounding = Arrotondamento
    .description = Arrotonda gli angoli del pannello verso lo sfondo
panel-border = Bordo
    .description = Una linea attorno al bordo del pannello, nel colore del ruolo Bordo; un lato a zero non ne disegna
panel-font = Carattere
    .description = Il carattere del pannello; il predefinito segue quello dell'app
panel-font-size = Dimensione carattere
    .description = La dimensione del testo del pannello rispetto al carattere dell'app; le righe si scalano con essa
panel-surface-shader = Shader di superficie
    .description = Fa girare uno shader WGSL sul corpo di questo pannello, sotto lo shader di schermo dell'app
panel-run-when-idle = Continua da fermo
    .description = Continua a disegnare fotogrammi mentre l'audio è muto. Off, lo shader si congela sull'ultimo fotogramma e il pannello non costa nulla
panel-shader-is-scene = Questo shader è una scena, quindi copre il corpo del pannello invece di disegnarci sopra. Viene da un bundle o da una configurazione più vecchia; la lista qui sopra offre solo shader che lasciano leggibile il pannello.

## Shader picker and saving
shader-source = Sorgente
shader-pick-none = Nessuno
shader-reload = Ricarica
shader-edit-as-file = Modifica come file
shader-make-private-copy = Crea copia privata
shader-save-replace = Sostituisci
shader-save-to-workspace = Salva nello spazio di lavoro
shader-save-replaces = Sostituisce lo shader che questo spazio di lavoro chiama già { $name }. Ogni pannello che usa quel nome cambia con lui
shader-save-adds = Lo aggiunge agli shader di questo spazio di lavoro sotto { $name }. Qualsiasi pannello può usarlo, e modificarlo li aggiorna tutti
shader-group-examples = Esempi
shader-group-this-workspace = Questo spazio di lavoro
shader-group-scenes = Scene
shader-group-workspace-scenes = Scene dello spazio di lavoro
shader-group-overlays = Overlay
shader-group-workspace-overlays = Overlay dello spazio di lavoro

## Saving a panel preset
preset-save = Salva preimpostazione
preset-save-name = Nome preimpostazione
preset-save-replaces = Sostituisce la preimpostazione che questo spazio di lavoro chiama già { $name }
preset-save-hint-after = per salvare
preset-back-from = Recuperala da
preset-back-add-panel = Aggiungi pannello
preset-back-then = poi
preset-back-presets = Preimpostazioni
preset-back-tail = in qualsiasi menu del pannello. Le preimpostazioni valgono solo per questo spazio di lavoro; un altro spazio di lavoro non le avrà.

## Keyboard hints
hint-press = Premi
hint-key-enter = Invio

## Settings: language
settings-language = Lingua
    .description = La lingua dell'interfaccia; Sistema cerca una corrispondenza nell'elenco del sistema operativo e ricade sull'inglese quando non ne trova
    .keywords = lingua traduzione localizzazione
settings-language-system = (Lingua di sistema)
settings-language-search = Cerca una lingua
picker-no-matches = Nessun risultato
settings-search-no-matches = Nessun risultato per "{ $text }"

## Embed dialog
bake-window-title = rox - Incorpora i metadati salvati
bake-title = Incorpora i metadati salvati
bake-intro = Scrive ciò che rox ha già nei file stessi, così anche un altro lettore lo legge. Niente viene ricalcolato.
bake-formats = Solo MP3 e FLAC; gli altri formati e le tracce CUE vengono saltati
bake-source-lyrics = Testi
bake-source-gain = ReplayGain
bake-source-acoustic = Descrizioni acustiche
bake-detail-nothing = niente di salvato da incorporare
bake-detail-only-skipped = niente da scrivere, { $skipped } da saltare
bake-detail-writes = { $count ->
    [one] { $count } file da scrivere
   *[other] { $count } file da scrivere
}
bake-detail-writes-skipped = { $count ->
    [one] { $count } file da scrivere, { $skipped } da saltare
   *[other] { $count } file da scrivere, { $skipped } da saltare
}
bake-error-read = Impossibile leggere la libreria: { $error }
bake-survey-counting = Scansione della libreria...
bake-survey-progress = Lettura dei tag, { $done } di { $total }
bake-nothing-to-embed = Niente da incorporare: i file contengono già tutto ciò che rox ha salvato
bake-rewrites = { $count ->
    [one] { $count } file verrà riscritto
   *[other] { $count } file verranno riscritti
}
bake-hint-before = Premi
bake-hint-key = Invio
bake-hint-after = per incorporare
bake-embed = Incorpora
bake-cancel = Annulla
bake-summary-files = { $count ->
    [one] 1 file
   *[other] { $count } file
}
bake-summary-updated = Aggiornamento di { $files }
bake-summary-stopped = Fermato dopo l'aggiornamento di { $files }
bake-summary-skipped = { $count ->
    [one] , { $count } saltato
   *[other] , { $count } saltati
}
bake-summary-failed = { $count ->
    [one] , { $count } fallito
   *[other] , { $count } falliti
}

## Arrange editors and header pieces
arrange-shown = Mostrati
arrange-hidden = Nascosti
tile-face-mosaic = Mosaico di copertine
tile-face-tinted = Mosaico tinto
tile-face-gradient = Scheda sfumata
tile-face-color = Scheda a tinta unita
head-piece-artist = Artista
head-piece-album = Album
head-piece-year = Anno
head-piece-genre = Genere
head-piece-quality = Qualità
head-piece-tracks = Tracce
head-piece-time = Durata
head-piece-spacer = Spaziatore
head-piece-divider = Divisore
head-piece-art = Copertina
head-unknown = Sconosciuto
status-item-count = Conteggio
status-item-time = Durata
status-item-albums = Album
status-item-artists = Artisti
status-item-plays = Ascolti
volume-item-icon = Icona
volume-item-slider = Cursore
volume-item-percent = Percentuale

## Filter chips and search menus
filter-field-artist = Artista
filter-field-album-artist = Artista dell'album
filter-field-album = Album
filter-field-genre = Genere
filter-field-year = Anno
filter-field-folder = Cartella
filter-unknown = Sconosciuto
filter-clear = Pulisci
query-show-search-box = Mostra il campo di ricerca
query-own-query = Ricerca propria
query-shared-query = Ricerca condivisa
headers-off = Off
headers-compact = Compatte
headers-expanded = Espanse

## Panel context menu
panel-dock-back = Riaggancia
panel-pop-out = Stacca
panel-close = Chiudi
panel-duplicate = Duplica
panel-reveal-in-browser = Mostra nel gestore file
panel-play-next = Riproduci dopo
panel-add-to-queue = Aggiungi alla coda
panel-add-to-playlist = Aggiungi alla playlist
panel-favourite-add = Aggiungi ai preferiti
panel-favourite-remove = Rimuovi dai preferiti
shader-pick-missing = { $name } (mancante)
shader-pick-custom = Personalizzato

## Shipped shader examples
shader-blurb-plasma = Colore alla deriva ricavato dai soli uniform, quindi costa un semplice quad.
shader-blurb-trails = Spalma il proprio ultimo fotogramma, quindi gira nel passaggio a schermo.
shader-blurb-sheen = Una vignettatura e un bagliore che si sposta, overlay trasparente per un pannello che già disegna.
shader-blurb-shadow = Un'ombra portata che testo e controlli del pannello proiettano, letta dalla cattura della maschera.
shader-blurb-cover = La copertina del brano in riproduzione, in letterbox su una velatura del suo stesso colore.
shader-blurb-badge = La copertina come piccola scheda parcheggiata in un angolo, con uno slot per spostarla.
shader-blurb-lamp = Una luce che segue il cursore e risponde ai clic, overlay trasparente.
shader-blurb-cube = Un cubo a fil di ferro che ruzzola in finto 3D, disegnato come luce additiva.
shader-blurb-bloom = Sfere alla deriva sfumate da un secondo passaggio a metà dimensione, la catena in miniatura.
shader-blurb-tube = Ripropone il pannello sottostante attraverso uno schermo CRT curvo, scanline comprese.

## Transport strip pieces
seek-item-elapsed = Trascorso
seek-item-strip = Barra
seek-item-ending = Fine
seek-item-duration = Durata
info-item-track-no = N. traccia
info-item-title = Titolo
info-item-duration = Durata
info-item-next = Successivo
info-item-queued = In coda
info-item-output = Uscita
info-item-favourite = Preferito
info-item-rating = Valutazione
playback-item-previous = Precedente
playback-item-seek-back = Indietro
playback-item-play = Riproduci
playback-item-seek-forward = Avanti
playback-item-next = Successivo
playback-item-stop = Ferma
playback-item-volume = Volume
playback-item-loop = Ripeti
playback-item-shuffle = Casuale
playback-item-continue = Continua
playback-item-crossfade = Dissolvenza incrociata
playback-item-random = A caso
playback-item-stop-after = Ferma dopo
playback-item-favourite = Preferito
playback-item-rating = Valutazione

## Dock chrome
dock-empty-tab = Scheda vuota
dock-unnamed = Senza nome
dock-tiles = Tessere
dock-zoom-in = Ingrandisci
dock-zoom-out = Riduci
dock-collapse = Comprimi
dock-expand = Espandi

## Shader picker notes
shader-note-empty = Scegli un esempio per iniziare, o indica a rox un file .wgsl con uno stadio fragment che definisce fs_user(uv)
shader-note-missing = { $name } non è più tra gli shader di questo spazio di lavoro, quindi non si disegna niente. Scegli qualcos'altro qui e questo pannello avrà una sorgente sua.
shader-note-shared = Condiviso in questo spazio di lavoro. Modificarlo aggiorna ogni superficie che lo usa.
shader-note-file = { $path }. I tuoi salvataggi si ricaricano mentre lo shader disegna, e la sorgente è salvata dentro layout e bundle, così funziona anche su una macchina che non ha mai avuto il file.
shader-note-custom = Questa sorgente viaggia dentro il suo layout o bundle senza un file dietro. Modifica come file la riscrive su disco e raccoglie i tuoi salvataggi.

## Panel pages and shared sides
panel-page-layout = Layout
panel-page-view = Vista
panel-page-content = Contenuto
panel-page-source = Sorgente
panel-page-bindings = Collegamenti
panel-page-emitters = Emettitori
panel-page-forces = Forze
panel-page-scene = Scena
side-left = Sinistra
side-right = Destra
genre-face-mosaic = Mosaico
genre-face-tinted = Tinto
genre-face-gradient = Sfumatura
genre-face-color = Colore

## Library panel
panel-title-library = Libreria
library-play = Riproduci
library-play-album = Riproduci l'album
library-play-group = Riproduci il gruppo
library-play-tracks = Riproduci { $count } tracce
library-play-similar = Riproduci brani simili
library-filter-by-album = Filtra per album
library-filter-by-artist = Filtra per artista
library-jump-to-playing = Vai alla traccia in riproduzione
library-menu-display = Visualizzazione
library-disc = Disco { $number }
library-empty-title = Apri una cartella di musica
library-empty-note = Verrà scansionata nella libreria (flac, mp3, wav)
library-headers = Intestazioni
    .description = Interruzioni di gruppo sulla lista; un ordinamento tiene insieme le sequenze che ci sono, la ricerca appiattisce tutto
library-group-by = Raggruppa per
    .description = Su cosa si interrompono le intestazioni; genere e anno riordinano la lista
library-header-row = Riga di intestazione
    .description = Cosa mostrano le intestazioni a una riga, da sinistra a destra; uno spaziatore o un divisore separa i lati
library-header-lines = Righe di intestazione
    .description = Le righe del blocco, dall'alto in basso; una riga vuota sparisce
library-follow-description = Scorri alla traccia in riproduzione a ogni cambio di brano
library-resume-description = Torna alla riga in riproduzione quando smetti di sfogliare
library-smooth-description = Scivola fino alla riga invece di saltarci
library-columns = Colonne
    .description = Quali colonne si vedono; trascina le intestazioni nel pannello per riordinarle e dimensionarle
library-column-headers = Intestazioni di colonna
    .description = La riga di intestazione ordinabile sopra la lista; nascosta, le colonne mantengono ordine e larghezza
library-compact-plays = Ascolti compatti
    .description = La colonna degli ascolti come piccolo conteggio con un trattino accanto
library-line-height = Altezza riga
    .description = Una riga di intestazione; i blocchi prendono le righe che servono, indipendenti dalle righe delle tracce
library-text-size = Dimensione testo
    .description = Il testo delle righe di intestazione, indipendente dall'altezza della riga, così la copertina cresce da sola
library-flush-background = Sfondo a filo
    .description = Metti le intestazioni sullo sfondo della lista invece che sulla tinta rialzata; i colori del brano le muovono insieme
library-gap-above = Spazio sopra
    .description = Ritagliato dalla cima del blocco; la lista traspare, e le righe si stringono
library-gap-below = Spazio sotto
    .description = Lo stesso sotto il blocco, prima delle sue tracce
library-section-rows = Righe
library-row-height = Altezza riga
    .description = Le righe delle tracce; il testo segue, ed entrambi si scalano con il carattere dell'app
library-row-spacing = Spaziatura righe
    .description = Altezza extra per ogni riga; respiro senza ingrandire il testo
library-stripes = Evidenziazione alternata
    .description = Tinge una riga di traccia su due, così una lista lunga si legge meglio
library-row-borders = Bordi di riga
    .description = Il filetto sotto ogni riga di traccia
library-art-description = La tessera delle intestazioni espanse: la copertina, il ritratto dell'artista, o l'immagine del genere
library-art-rounding = Arrotondamento copertina
    .description = Arrotonda gli angoli della copertina
library-art-position = Posizione copertina
    .description = Su quale lato del blocco va la tessera delle intestazioni espanse
library-art-margin = Margine copertina
    .description = Rientra la tessera nel blocco; si rimpicciolisce per restare quadrata
library-circular-portraits = Ritratti circolari
    .description = Raggruppato per artista, arrotonda le tessere al cerchio pieno della parete invece che alla manopola di arrotondamento
library-genre-face = Immagine del genere
    .description = Raggruppato per genere, cosa mostra la tessera: le copertine, le copertine velate nel colore del genere, o una scheda a tinta unita sotto la sua geometria

## Album grid panel
panel-title-album-grid = Griglia album
grid-menu-scroll = Scorrimento
grid-menu-sort = Ordinamento
grid-sort-artist = Artista
grid-sort-album = Album
grid-sort-year = Anno
grid-sort-added = Aggiunti di recente
grid-sort-plays = Più ascoltati
grid-letter-rail = Barra delle lettere
    .description = Le iniziali lungo il bordo della parete; un clic salta al primo album di quella lettera
grid-vertical-scroll = Scorrimento verticale
grid-horizontal-scroll = Scorrimento orizzontale
grid-jump-to-playing = Vai all'album in riproduzione
grid-library-empty = La libreria è vuota
grid-play-albums = Riproduci { $count } album
grid-vertical-layout = Layout verticale
    .description = Scorre la parete su e giù, le righe riempiono la larghezza; off la scorre a sinistra e destra, le colonne riempiono l'altezza
grid-follow-description = Scorri all'album in riproduzione a ogni cambio di brano
grid-resume-description = Torna all'album in riproduzione quando smetti di sfogliare
grid-smooth-description = Scivola fino all'album invece di saltarci
grid-section-dimming = Attenuazione
grid-section-tiles = Tessere
grid-dim-while-playing = Attenua durante la riproduzione
    .description = Sfuma ogni copertina tranne quella dell'album in riproduzione; passandoci sopra la tessera si riaccende
grid-dim-amount = Intensità
    .description = Quanto sfumano le altre copertine; 100% le nasconde
grid-desaturate = Desatura durante la riproduzione
    .description = Porta ogni copertina tranne quella dell'album in riproduzione in scala di grigi; passandoci sopra torna il colore
grid-always = Sempre
    .description = Tieni le copertine in secondo piano anche quando non suona niente; solo la tessera sotto il puntatore si vede intera
grid-show-titles = Mostra i titoli
    .description = Stampa album e artista sotto ogni copertina, stile iTunes, invece che solo al passaggio del puntatore
grid-title-alignment = Allineamento titoli
    .description = Allinea le didascalie sotto le loro copertine
grid-tile-size = Dimensione tessere
    .description = Il lato più lungo delle tessere di copertina; le colonne si dividono la larghezza del pannello in parti uguali
grid-gap = Spazio
    .description = Spazio tra le copertine; zero le impacchetta bordo a bordo
grid-art-rounding-description = Arrotonda gli angoli di ogni copertina; 100% è un cerchio

## Settings: sidebar pages
settings-page-appearance = Aspetto
settings-page-application = Applicazione
settings-page-audio = Audio
settings-page-development = Sviluppo
settings-page-integrations = Integrazioni
settings-page-keymap = Scorciatoie
settings-page-library = Libreria
settings-page-mcp = MCP
settings-page-ml-models = Modelli ML
settings-page-playback = Riproduzione
settings-page-providers = Provider
settings-page-shader = Shader
settings-page-storage = Archiviazione
settings-page-workspace = Spazio di lavoro

## Settings: appearance
settings-appearance-backdrop-all-windows = Tutte le finestre
    .description = Metti lo sfondo anche dietro le finestre figlie: impostazioni, editor, dialoghi, pannelli staccati. Off tiene lo sfondo e la trasparenza sulle finestre dello spazio di lavoro
settings-appearance-backdrop-strength = Intensità dello sfondo
    .description = Quanto traspare lo sfondo di copertina dietro le finestre
settings-appearance-border = Bordo
    .description = Una linea attorno al bordo di ogni pannello, nel colore del ruolo Bordo; un lato a zero non ne disegna nessuna
settings-appearance-colors-locked-note = I colori del brano sono attivi, quindi la traccia in riproduzione guida questi colori e l'esportazione li salva. Disattivali qui sopra per modificarli
settings-appearance-design-mode = Modalità progettazione
    .description = Modifica il layout sul posto: le voci aggiungi, rinomina, duplica, stacca e chiudi dei menu dei pannelli, i controlli che un contenitore fa fluttuare sui suoi slot, e il trascinamento delle schede. Off nasconde tutto questo; la pagina Spazio di lavoro modifica comunque l'albero
    .keywords = modifica disposizione riordina blocca
settings-appearance-font = Carattere
    .description = Il carattere di tutta l'app; i pannelli possono sovrascriverlo nelle proprie impostazioni
    .keywords = carattere tipografia font
settings-appearance-font-size = Dimensione carattere
    .description = La dimensione base del testo da cui scala il testo di ogni pannello; controlli e icone tengono la loro
settings-appearance-hide-menubar = Nascondi la barra dei menu
    .description = Tieni nascosta la barra dei menu, facendola fluttuare sul dock mentre alt è premuto. Doppio tocco su alt per lasciarla su, così i suoi pulsanti prendono un clic normale
settings-appearance-icons-intro = Un pacchetto è una cartella di SVG che sostituisce le icone integrate; il cambio ha effetto al prossimo avvio
settings-appearance-icons-open-folder = Apri cartella
settings-appearance-inverse-from-dark = Inverti dal tema scuro
settings-appearance-inverse-from-light = Inverti dal tema chiaro
settings-appearance-keep-theme = Mantieni il tema
    .description = Tieni il tema attivo anche quando la luminosità di una copertina lo ribalterebbe; i colori del brano danno comunque la tinta
settings-appearance-margin = Margine
    .description = Tira ogni pannello dentro dalla sua cella; un pannello può sovrascriverlo nelle proprie impostazioni
settings-appearance-new-pack = Nuovo pacchetto
settings-appearance-os-decorations = Decorazioni di sistema
    .description = La barra del titolo e i bordi del sistema sulle finestre principali; off si affida ai controlli finestra e ai pannelli ancora di trascinamento
settings-appearance-pack-name-placeholder = Nome del pacchetto
settings-appearance-padding = Spaziatura interna
    .description = Spazio dentro il bordo di ogni pannello, tenuto nel suo sfondo
settings-appearance-palette-export = Esporta
settings-appearance-palette-import = Importa
settings-appearance-panel-seams = Giunzioni dei pannelli
    .description = Il filetto tra le tessere dei pannelli; off lascia invisibili le maniglie di ridimensionamento, ma restano trascinabili
settings-appearance-resize-border = Bordo di ridimensionamento
    .description = Ridimensiona le finestre principali trascinandone i bordi; vale solo con le decorazioni di sistema disattivate, e disattivarlo lascia lo snap e Win+frecce come modo per ridimensionare
settings-appearance-rounding = Arrotondamento
    .description = Arrotonda gli angoli di ogni pannello verso lo sfondo
settings-appearance-section-colors = Colori
settings-appearance-section-frame = Cornice
settings-appearance-section-icons = Icone
settings-appearance-section-interface = Interfaccia
settings-appearance-section-theming = Tema
settings-appearance-section-transparency = Trasparenza
settings-appearance-section-typography = Tipografia
settings-appearance-song-theming = Colori del brano
    .description = Tinge la palette e fa da sfondo alle finestre con la copertina della traccia in riproduzione
settings-appearance-surface-opacity = Opacità di superficie
    .description = Quanto opache appaiono le superfici dell'app sopra lo sfondo
settings-appearance-theme = Tema
    .description = La palette che l'app disegna e quella su cui lavora l'editor dei colori qui sotto; Sistema segue la preferenza chiaro o scuro del sistema operativo
settings-appearance-theme-dark = Scuro
settings-appearance-theme-light = Chiaro
settings-appearance-theme-system = Sistema

## Settings: application
settings-application-check-updates = Cerca aggiornamenti
    .description = Cerca una versione più recente una volta al giorno all'avvio di rox; la finestra Informazioni controlla subito in ogni caso
settings-application-download-updates = Scarica gli aggiornamenti
    .description = Quando un controllo trova una versione più recente, scaricala e preparala in background; il prossimo avvio la esegue
settings-application-enable-ai = Attiva le funzioni AI
    .description = Lascia che gli strumenti AI parlino con rox: aggiunge il supporto MCP e i download dei modelli ML, con le loro pagine nella barra laterale.
settings-application-lock-panel-resize = Blocca il ridimensionamento dei pannelli
    .description = Le divisioni dei pannelli si ridimensionano solo con la modalità progettazione attiva, così un trascinamento vicino a una giunzione non può spostare un layout finito
settings-application-portable-copying = Copia dei dati...
settings-application-portable-mode = Modalità portatile
    .description = Tieni impostazioni, libreria e cache in una cartella rox-data accanto all'eseguibile, così il lettore si sposta con i suoi dati. Disattivarla torna alla cartella di sistema e lascia rox-data dov'è
settings-application-portable-not-writable = La cartella dell'app non è scrivibile
settings-application-portable-restart-note = Vale dal prossimo avvio; questa sessione resta sulla cartella attuale
settings-application-remain-in-tray = Resta nell'area di notifica
    .description = Tieni la musica in riproduzione quando si chiude l'ultima finestra, con l'icona nell'area di notifica (il dock su macOS) come via di ritorno
settings-application-section-ai = AI
settings-application-section-control-socket = Socket di controllo
settings-application-section-data = Dati
settings-application-section-layout = Layout
settings-application-section-startup = Avvio
settings-application-section-window = Finestra
settings-application-socket-path = Percorso del socket
    .description = L'interfaccia macchina di rox mentre gira: JSON-RPC su un socket locale, legato a questa cartella dati. Il proxy rox-mcp serve i client MCP su di esso

## Settings: audio
settings-audio-broadcast-bitrate = Bitrate
    .description = Quanto spende l'encoder MP3 per ogni secondo di stream
settings-audio-broadcast-enable = Trasmetti su Icecast
    .description = Manda ciò che rox riproduce a un server icecast come client sorgente, codificato in MP3. Il mount, gli ascoltatori e il lato rete appartengono tutti a icecast; rox si connette solo in uscita, e un server irraggiungibile non tocca mai la riproduzione locale
settings-audio-broadcast-host-placeholder = host icecast
settings-audio-broadcast-login = Credenziali sorgente
    .description = Le credenziali sorgente di icecast, l'utente e la password che la sua configurazione indica
settings-audio-broadcast-mount = Mount
    .description = Il mount su cui si sintonizzano gli ascoltatori, e il nome dello stream che annuncia
settings-audio-broadcast-name-placeholder = Nome dello stream
settings-audio-broadcast-password-placeholder = Password sorgente
settings-audio-broadcast-server = Server
    .description = L'host e la porta del server icecast; il protocollo sorgente gira su un socket semplice
settings-audio-broadcast-user-placeholder = source
settings-audio-crossfade = Dissolvenza incrociata
    .description = Quanto a lungo una traccia si sovrappone a quella dopo. La dissolvenza serve per il casuale e i salti, così i confini interni di un album restano intatti a meno che la riga qui sotto non dica altro. Zero la disattiva
    .keywords = dissolvenza transizione incrociata senza pausa
settings-audio-equalizer-note = Dieci bande d'ottava sull'uscita. Si apre in una finestra sua, perché si lavora mentre la musica suona invece di impostarlo una volta sola
settings-audio-exclusive-mode = Modalità esclusiva
    .description = Prendi il dispositivo per rox soltanto e fallo girare alla frequenza del file dove l'hardware la accetta; off condivide il mixer di sistema con tutto il resto del desktop
settings-audio-fade-inside-albums = Dissolvi dentro gli album
    .description = Sovrapponi anche le tracce che appartengono allo stesso disco. Off tiene le giunzioni di un disco esattamente come sono state masterizzate, ed è lì che il gapless conta di più
settings-audio-open-equalizer = Apri l'equalizzatore
settings-audio-output-buffer = Buffer
    .description = Quanto audio tiene la scheda alla volta. Corto reagisce prima e gracchia prima su una macchina occupata; lungo è più sicuro e più pigro
settings-audio-output-buffer-default = Predefinito (10 ms)
settings-audio-output-device = Dispositivo
    .description-default = Il predefinito di sistema segue quello che è impostato sul desktop
    .description-linux = L'esclusiva prende una scheda direttamente dal kernel, quindi la lista è di schede audio invece che di uscite del desktop. Bluetooth e altri dispositivi di sound server non hanno una scheda da prendere e compaiono solo con l'esclusiva disattivata
    .description-other = L'esclusiva prende il dispositivo per rox soltanto, quindi nient'altro sul desktop può suonarci finché la modalità non è disattivata
settings-audio-output-device-system-default = Predefinito di sistema
settings-audio-output-experimental-badge = Sperimentale
settings-audio-output-experimental-tooltip = Il backend esclusivo di questa piattaforma è scritto a partire dal contratto audio che la piattaforma documenta, ma gli sviluppatori non l'hanno mai fatto girare su hardware reale. Dovrebbe prendere il dispositivo o ripiegare su condiviso con una motivazione, mai restare muto. Se si comporta male, disattivalo e segnala cos'è successo con il pulsante accanto a questo badge.
settings-audio-output-format = Formato
    .description = Cosa passa rox alla scheda. Una scheda che non accetta la scelta gira nel formato più ampio che ha, e lo stato qui sotto mostra quale
settings-audio-output-format-f32 = Float a 32 bit
settings-audio-output-format-s16 = Intero a 16 bit
settings-audio-output-format-s32 = Intero a 32 bit
settings-audio-output-format-widest = Il più ampio disponibile
settings-audio-output-issue-tooltip = Segnala come si è comportata la modalità esclusiva su questa macchina. Apre una issue su GitHub con la piattaforma e lo stream negoziato già compilati.
settings-audio-output-mode-exclusive = Esclusiva
settings-audio-output-mode-shared = Condivisa
settings-audio-output-not-built = Non ancora compilato per questa piattaforma
settings-audio-output-rate-follow = Segui il file
settings-audio-output-sample-rate = Frequenza di campionamento
    .description = Seguire riapre il dispositivo alla frequenza di ogni file, il che costa una pausa al confine dove la frequenza cambia; fissare una frequenza non lo paga mai e ricampiona tutto ciò che non corrisponde
settings-audio-output-status-error-hint = Scegli un altro dispositivo, o disattiva l'esclusiva
settings-audio-output-status-error-title = Nessuna uscita
settings-audio-output-status-idle-hint = Avvia una traccia per vedere il formato che il dispositivo ha accettato
settings-audio-output-status-idle-title = Niente in riproduzione
settings-audio-replaygain-level-by = Livella per
    .description = Riproduci ogni traccia al volume misurato dai suoi tag ReplayGain, così una riproduzione casuale smette di saltare tra masterizzazioni diverse. Traccia livella ogni file per conto suo; Album usa il guadagno del disco su tutte le sue tracce, il che lascia i passaggi silenziosi e quelli forti di un album dove sono stati messi
    .keywords = normalizzazione volume livellamento
settings-audio-replaygain-measure-missing-button = Misura i mancanti
settings-audio-replaygain-measure-new = Misura i nuovi file
    .description = Misura i file nuovi appena la sorveglianza li rileva, una volta che la sincronizzazione si è assestata, così una libreria che cresce tiene i suoi guadagni senza dover tornare qui. I numeri vanno dove punta Salva i guadagni misurati. Attivarlo propone di misurare prima ciò che già manca; dopo vede solo i file appena arrivati
settings-audio-replaygain-measuring-progress = Misurazione di { $done } su { $total }
settings-audio-replaygain-measuring-start = Misurazione: calcolo cosa manca...
settings-audio-replaygain-mode-album = Album
settings-audio-replaygain-mode-off = Off
settings-audio-replaygain-mode-track = Traccia
settings-audio-replaygain-preamp = Preamplificazione
    .description = Aggiunta a ogni guadagno taggato. Il riferimento di ReplayGain è più basso del livello a cui si masterizzano i dischi moderni, quindi una libreria livellata suona più piano della stessa libreria grezza; è qui che lo si recupera. Un aumento non satura mai: il picco taggato lo limita
settings-audio-replaygain-save = Salva i guadagni misurati
    .description = Dove la misurazione mette i suoi numeri. Il database della libreria lascia intatti i tuoi file; i tag mettono gli stessi valori dove li legge ogni altro lettore, al prezzo di riscrivere i file audio
settings-audio-replaygain-status-measured = { $measured ->
    [one] Tutte le { $total } tracce scansionate hanno un guadagno su cui livellare, { $measured } misurata da rox
   *[other] Tutte le { $total } tracce scansionate hanno un guadagno su cui livellare, { $measured } misurate da rox
}
settings-audio-replaygain-status-tagged = Tutte le { $total } tracce scansionate hanno i tag ReplayGain
settings-audio-replaygain-untagged = File senza tag
    .description = A che livello suona un file senza tag ReplayGain. Niente l'ha misurato, quindi questa è una stima al posto di una misura. Lascialo a zero e le tracce senza tag suoneranno come hanno sempre fatto
settings-audio-section-broadcast = Trasmissione
settings-audio-section-equalizer = Equalizzatore
settings-audio-section-output = Uscita
settings-audio-section-playback = Riproduzione
settings-audio-section-replaygain = ReplayGain
settings-audio-transport = Trasporto
    .description = Avvia e ferma senza lasciare questa pagina, visto che ogni impostazione qui sotto si giudica a orecchio

## Settings: integrations
settings-integrations-discord-enable = Attiva Rich Presence
    .description = Mostra l'attività di rox su Discord quando la musica suona
settings-integrations-discord-show-lastfm = Mostra il pulsante Last.fm
    .description = Includi un pulsante cliccabile 'Vedi su Last.fm' nello stato Discord
settings-integrations-discord-show-youtube = Mostra il pulsante YouTube
    .description = Includi un pulsante cliccabile 'Cerca su YouTube' nello stato Discord
settings-integrations-ffmpeg-binary = Binario FFmpeg
    .description = Quale ffmpeg esegue le conversioni; lascia vuoto per quello nel PATH
settings-integrations-ffmpeg-fail-note = Converti resta nascosto finché ffmpeg non punta a un binario funzionante
settings-integrations-ffmpeg-fail-title = Questo ffmpeg non è partito
settings-integrations-ffmpeg-missing-note = Converti resta nascosto; installa ffmpeg o fai puntare il percorso a un binario
settings-integrations-ffmpeg-missing-title = Nessun ffmpeg funzionante trovato
settings-integrations-ffmpeg-ok-note = ffmpeg funziona. Converti è disponibile.
settings-integrations-ffmpeg-test = Prova
settings-integrations-lastfm-api-key-row = Chiave API
settings-integrations-lastfm-connect = Connetti
settings-integrations-lastfm-disconnect = Disconnetti
settings-integrations-lastfm-finish-connecting = Completa la connessione
settings-integrations-lastfm-hearts = { $n ->
    [one] { $n } cuore
   *[other] { $n } cuori
}
settings-integrations-lastfm-import-loved = Importa i brani amati
settings-integrations-lastfm-intro-builtin = Connetti il tuo account Last.fm: autorizza rox nel browser e le tracce riprodotte vengono scrobblate lì
settings-integrations-lastfm-intro-custom = Questa build non include un'identità api, quindi lo scrobbling richiede un account api tuo (Last.fm/api/account/create); incolla la sua chiave e il segreto condiviso, poi connetti
settings-integrations-lastfm-key-placeholder = Chiave API
settings-integrations-lastfm-love-failed = L'ultimo è fallito: { $error }
settings-integrations-lastfm-love-pending = { $hearts } in attesa di invio
settings-integrations-lastfm-love-pending-failed = { $hearts } in attesa di invio, ultimo tentativo: { $error }
settings-integrations-lastfm-reconnect = Riconnetti
settings-integrations-lastfm-secret-placeholder = Segreto condiviso
settings-integrations-lastfm-secret-row = Segreto condiviso
settings-integrations-lastfm-status-confirming = Conferma in corso...
settings-integrations-lastfm-status-connected = Connesso come { $username }
settings-integrations-lastfm-status-elsewhere = Connesso su un'altra installazione di rox; ognuna autorizza con la propria identità api, quindi connetti anche questa
settings-integrations-lastfm-status-failed = Connessione fallita: { $error }
settings-integrations-lastfm-status-not-connected = Non connesso
settings-integrations-lastfm-status-rejected = Last.fm ha rifiutato la sessione ed è stata scartata. Riconnettiti per continuare lo scrobbling
settings-integrations-lastfm-status-requesting = Richiesta di un token...
settings-integrations-lastfm-status-waiting = Autorizza rox nel browser, poi completa la connessione
settings-integrations-lastfm-working = In corso...
settings-integrations-love-favourites = Ama i preferiti
    .description = Rispecchia i cuori su Last.fm come brani amati; togliere un cuore lo toglie anche lì
settings-integrations-scrobble-threshold = Soglia di scrobbling
    .description = Quanto deve suonare una traccia prima che venga scrobblata; la barra di avanzamento e la forma d'onda possono segnarlo
settings-integrations-scrobble-tracks = Scrobbla le tracce
    .description = Manda le tracce riprodotte a Last.fm una volta superata la soglia
settings-integrations-section-conversion = Conversione
settings-integrations-section-discord = Discord Rich Presence
settings-integrations-section-favourites = Preferiti
settings-integrations-section-lastfm = Last.fm
settings-integrations-section-scrobbling = Scrobbling

## Settings: keymap
settings-keymap-clash = { $chord } è anche { $other }; solo una delle due scatterà
settings-keymap-not-bound = Non assegnata
settings-keymap-recording = Premi i tasti
settings-keymap-restore = Ripristina
settings-keymap-restore-all = Ripristina ogni combinazione
    .description = Rimetti ogni comando sui tasti con cui arriva, compresi quelli per cui questa build non ha più una riga
settings-keymap-section-defaults = Predefiniti
settings-keymap-undo = Annulla
settings-keymap-undo-last = Annulla l'ultimo ripristino
    .description = Riporta indietro le combinazioni che l'ultimo ripristino ha buttato via, una riga o tutte

## Settings: library
settings-library-acoustic-all-described = Tutte le { $total } tracce scansionate sono descritte da { $label }
settings-library-acoustic-auto = Descrivi i nuovi file
    .description = Descrivi i file nuovi appena la sorveglianza li rileva, una volta che la sincronizzazione si è assestata, così una libreria che cresce tiene le sue descrizioni senza dover tornare qui. Off, i nuovi file aspettano il pulsante Analizza i mancanti. Attivarlo propone di analizzare prima ciò che già manca; dopo vede solo i file appena arrivati
settings-library-acoustic-enable = Descrivi come suonano le tracce
    .description = Ricava come suona ogni traccia, così la libreria può trovare musica che assomiglia a quella in riproduzione. Tutto gira su questa macchina, e descrivere una libreria grande richiede un po'
    .keywords = simile suono impronta descrivere
settings-library-acoustic-extractor = Estrattore
settings-library-acoustic-extractor-model = Modello
settings-library-acoustic-fallback = Analisi
settings-library-acoustic-partial = { $label } descrive { $done } di { $total } tracce scansionate. Analizza i mancanti lavora sul resto
settings-library-acoustic-progress = { $running } è a { $done } su { $total }
settings-library-acoustic-progress-start = { $running }: calcolo cosa manca...
settings-library-acoustic-save = Salva le descrizioni
    .description = Dove la passata mette ciò che ricava. Il database da solo lascia intatti i tuoi file; i tag mettono una copia anche in ogni file, così le descrizioni restano se la libreria viene ricostruita o la cartella si sposta su un'altra macchina, al prezzo di riscrivere i file audio. I tag valgono solo per MP3 e FLAC; ogni altro formato tiene la copia nel database
settings-library-add-folder = Aggiungi cartella
settings-library-duplicates = Duplicati...
settings-library-embed-button = Incorpora i metadati salvati...
settings-library-folder-col-albums = Album
settings-library-folder-col-folder = Cartella
settings-library-folder-col-size = Dimensione
settings-library-folder-col-tracks = Tracce
settings-library-folders-intro = Cartelle scansionate nella libreria; rimuoverne una toglie le sue tracce dal catalogo e lascia stare i file
settings-library-genre-separator-nudge = Separatori cambiati: la navigazione si adegua subito. Le liste di generi salvate da scansioni precedenti tengono la vecchia forma finché non premi Riscansiona nell'intestazione Cartelle
settings-library-merge-case = Unisci le varianti di maiuscole
    .description = Tratta come uno solo i valori che differiscono solo per maiuscole: Rock e rock diventano lo stesso genere, artista e album, mostrati con la grafia usata dalla maggior parte delle tracce. I file tengono i tag come sono scritti
settings-library-no-folders = Ancora nessuna cartella
settings-library-repair-tags = Ripara i tag...
settings-library-section-folders = Cartelle
settings-library-section-stored-metadata = Metadati salvati
settings-library-section-tempo = Analisi del tempo
settings-library-split-genres = Dividi i generi su virgole e barre
    .description = "Dubstep, Trap" e "Drum & Bass / Neurofunk" contano ogni valore come genere a sé; i punti e virgola dividono sempre. Off tiene interi i nomi con barra per i tag dove significano un genere solo. I file tengono i tag come sono scritti
settings-library-tempo-auto = Cronometra i nuovi file
    .description = Conta i battiti nei file nuovi appena la sorveglianza li rileva, una volta che la sincronizzazione si è assestata, così una libreria che cresce tiene i suoi tempi senza dover tornare qui. Off, i nuovi file aspettano il pulsante Analizza i mancanti. Attivarlo propone di cronometrare prima ciò che già manca; dopo vede solo i file appena arrivati
settings-library-tempo-enable = Ricava quanto vanno veloci le tracce
    .description = Conta i battiti nelle tracce i cui tag non lo dicono, così la libreria può mostrare e ordinare per tempo. Tutto gira su questa macchina, i numeri vanno nel database della libreria, e i tuoi file restano intatti
settings-library-tempo-progress = Cronometraggio di { $done } su { $total }
settings-library-tempo-progress-start = Calcolo cosa manca...
settings-library-tempo-status-measured = { $measured ->
    [one] Tutte le { $total } tracce scansionate hanno un tempo, { $measured } ricavata da rox
   *[other] Tutte le { $total } tracce scansionate hanno un tempo, { $measured } ricavate da rox
}
settings-library-tempo-status-tagged = Tutte le { $total } tracce scansionate hanno un tag di tempo
settings-library-watch-folders = Sorveglia le cartelle
    .description = Fai entrare nella libreria i file aggiunti, modificati ed eliminati mentre succede, senza una riscansione manuale
settings-library-write-stored = Scrivi nei file ciò che è salvato
    .description = Le tre impostazioni di salvataggio valgono solo per la prossima scrittura, quindi tutto ciò che è stato salvato prima che una passasse a Tag è ancora solo in rox. Questo scrive nei file stessi i testi, i guadagni e le descrizioni che rox ha già, così un altro lettore che legge la cartella li vede. Niente viene ricalcolato

## Settings: MCP
settings-mcp-client-config = Configurazione client
    .description = Incollala nella lista dei server di un client MCP (Claude Code, Claude Desktop, o qualsiasi altro) per lasciargli chiedere a rox della libreria, di cosa sta suonando e del trasporto. rox deve essere in esecuzione; gli strumenti girano sul suo socket di controllo
settings-mcp-enable = Attiva il server MCP
    .description = Rispondi alle chiamate di strumento dai client MCP connessi. Il proxy controlla questa opzione a ogni chiamata, quindi mentre è off i client vengono rifiutati con la motivazione; la configurazione qui sotto si può preparare comunque

## Settings: ML models
settings-mlmodels-checking = Controllo in corso...
settings-mlmodels-choose-file = Scegli un file
settings-mlmodels-custom-description-empty = Indica a rox un checkpoint PANNs CNN10 tuo, in safetensors. Viene letto dov'è e prende il nome dal suo hash, così un secondo checkpoint descrive la libreria a parte invece di riusare le coordinate del primo
settings-mlmodels-download-failed = Non è stato possibile scaricare { $label }: { $reason }
settings-mlmodels-downloading = Scaricamento di { $label }: { $done } di { $total }
settings-mlmodels-stopping = Interruzione del download di { $label }...
settings-mlmodels-fallback-model = modello
settings-mlmodels-fallback-the-model = Il modello
settings-mlmodels-kind-custom = Personalizzato
settings-mlmodels-kind-recommended = Consigliato
settings-mlmodels-pass-stopped = L'ultima passata si è fermata: { $reason }
settings-mlmodels-weights-file = File dei pesi

## Settings: playback
settings-playback-continuation-continue = Continua
    .description = Vai avanti nella lista da cui sei partito, poi il resto della libreria dietro. Riproduci un album da metà di una vista e la vista va avanti
settings-playback-continuation-off = Off
    .description = Niente riempie di nuovo la coda; la riproduzione si ferma alla fine
settings-playback-continuation-weighted = Ponderato
    .description = Pesca da tutta la libreria, prima quello che non hai mai riprodotto e per ultimo quello che hai sentito di recente
settings-playback-keep-playing = Continua a suonare
    .description = Cosa suona quando la coda finisce. Qualunque cosa scelga viene accodata alla timeline come contesto normale, così resta visibile e rimovibile, non uno stato nascosto. Con l'ordine qui sopra su Simili continua a trovare tracce che suonano come quella in riproduzione, qualunque di queste sia scelta
    .keywords = continua riempi automatico coda
settings-playback-play-order = Ordine di riproduzione
    .description = Come sono disposte le tracce già in coda mentre il casuale è attivo. Il pulsante casuale del trasporto lo attiva e disattiva; questo decide cosa fa una volta attivo
settings-playback-rating-scale = Scala di valutazione
    .description = Stelle per un clic veloce, 0-10 a mezzi passi per punteggi da recensione più fini
settings-playback-rating-scale-numeric = 0-10
settings-playback-rating-scale-stars = Stelle
settings-playback-restore-last-session = Ripristina l'ultima sessione
    .description = Parti con la coda di riproduzione come l'hai lasciata, in pausa sulla traccia che suonava e nel punto in cui era. Le tracce in coda fuori dalle cartelle della tua libreria non si possono ripristinare ed escono dall'ordine
settings-playback-section-queue = Coda
settings-playback-section-ratings = Valutazioni
settings-playback-section-startup = Avvio
settings-playback-shuffle-random = Casuale
    .description = Il casuale che tutti intendono con la parola. Quello che arriva suona senza un ordine particolare
settings-playback-shuffle-similar = Simili
    .description = Prima i più vicini per suono. Quello che arriva è ordinato per quanto assomiglia alla traccia che suonava quando l'hai attivato, e riordinato a ogni salto. Serve la libreria descritta nella pagina Libreria
settings-playback-unrated-dots = Punti per le non valutate
    .description = Segna le stelle vuote con un punto tenue invece di lasciarle vuote

## Settings: providers
settings-providers-artist = Last.fm
    .description = Recupera biografie, statistiche e artisti simili per il pannello biografia, con un ritratto da Deezer; tutto resta nella cartella dati e poi si legge offline
settings-providers-deezer = Deezer
    .description = Cerca copertine su Deezer, fino a 1000 pixel
settings-providers-itunes = iTunes
    .description = Cerca copertine su iTunes; la ricerca dell'editor di copertine mostra i risultati da scegliere prima di impostarne una
settings-providers-lastfm-art = Last.fm
    .description = Cerca copertine su Last.fm
settings-providers-lrclib = LRCLIB
    .description = Recupera i testi mancanti da lrclib.net, sincronizzati quando ci sono
settings-providers-lyrics-intro = Le ricerche online partono solo quando un'azione di un pannello ne chiede una; riproduzione e navigazione non toccano mai la rete
settings-providers-musicbrainz = MusicBrainz
    .description = Cerca i tag su musicbrainz.org; la ricerca del pannello metadati mostra i risultati da confermare campo per campo prima di scrivere
settings-providers-save-lyrics = Salva i testi recuperati
    .description = Dove viene salvato un foglio recuperato: la cartella dati di rox che tiene pulita la libreria, un .lrc accanto alla traccia, o il tag incorporato
settings-providers-save-lyrics-data-folder = Cartella dati
settings-providers-save-lyrics-sidecar = Sidecar
settings-providers-save-lyrics-tag = Tag
settings-providers-section-artist = Artista
settings-providers-section-cover-art = Copertine
settings-providers-section-lyrics = Testi
settings-providers-section-metadata = Metadati

## Settings: shader
settings-shader-backdrop-all-windows = Tutte le finestre
    .description = Ombreggia lo sfondo di ogni finestra: impostazioni, editor, dialoghi, pannelli staccati. Off lo tiene sulle finestre dello spazio di lavoro
settings-shader-backdrop-enabled = Shader di sfondo
    .description = Fa girare uno shader WGSL reattivo alla musica sopra lo sfondo di copertina, sotto ogni pannello. Fa parte dello spazio di lavoro, quindi viaggia con il look
settings-shader-backdrop-fallback-name = Sfondo
settings-shader-backdrop-run-idle = Continua da fermo
    .description = Continua a disegnare senza niente in riproduzione. L'animazione resta ferma in ogni caso
settings-shader-compile-error-title = Questo shader non compila
settings-shader-legacy-note = Senza niente instradato il pool riempie gli slot nel suo ordine: il primo segnale nello slot 0, il secondo nello slot 1, e così via. La prima route che aggiungi prende il controllo di tutta la mappatura.
settings-shader-overlay-enabled = Shader di overlay
    .description = Fa girare uno shader WGSL reattivo alla musica su tutta la finestra. Vengono offerti solo gli shader che lasciano usabile l'app sotto
settings-shader-scene-covers-window = Questo shader è una scena, quindi copre la finestra invece di disegnarci sopra. Viene da un bundle o da una configurazione più vecchia; la lista qui sopra offre solo shader che lasciano usabile l'app.
settings-shader-screen-all-windows = Tutte le finestre
    .description = Ombreggia anche le finestre figlie: impostazioni, statistiche, equalizzatore, pannelli staccati. Il conto alla rovescia per tornare indietro resta non ombreggiato in ogni caso
settings-shader-screen-fallback-name = Schermo
settings-shader-screen-run-idle = Continua da fermo
    .description = Continua a disegnare senza niente in riproduzione. L'animazione resta ferma in ogni caso. Uno shader che legge il mouse segue il cursore a musica ferma anche senza questo; si ferma solo un paio di secondi dopo il puntatore
settings-shader-section-backdrop = Shader di sfondo
settings-shader-section-overlay = Shader di overlay
settings-shader-signals-block = Segnali
    .description = Quale segnale condiviso riceve ognuno dei sedici slot dello shader
settings-shader-slots-block = Slot
    .description = Ogni slot così come lo riceve lo shader; gli slot senza route sono manopole impostate a mano

## Settings: storage
settings-storage-artist-images = Immagini degli artisti
    .description = Ritratti, banner e biografie recuperati per le viste artista (artists/); quelli cancellati vengono recuperati di nuovo alla prossima apertura di una vista
settings-storage-catalog = Catalogo
    .description = L'indice delle tracce che costruiscono le scansioni: una riga una traccia con i suoi tag, i dettagli del file e gli eventuali intervalli cue, dentro library.db
settings-storage-cover-thumbnails = Miniature delle copertine
    .description = Copertine piccole tenute dopo il primo rendering (thumbs.db); quelle cancellate si ricostruiscono quando rientrano in vista
settings-storage-logs = Log
    .description = Cosa scrive ogni esecuzione per le segnalazioni di bug (logs/rox.log), ruotato a un tetto di dimensione così non cresce mai troppo
settings-storage-looks-layouts = Look e layout
    .description = Il look che l'app sta usando (workspace.json) con i tuoi spazi di lavoro salvati, i file shader estratti e i pacchetti di icone accanto. Piccolo, e ogni suo byte è qualcosa che hai impostato tu
settings-storage-lyrics = Testi
    .description = Fogli recuperati e modificati tenuti nell'archivio dell'app (lyrics/), così le cartelle della libreria restano pulite
settings-storage-measured-tempos = Tempi misurati
    .description = I tempi che rox ha contato dall'audio, per le tracce i cui tag non ne hanno; i numeri dei tag non vengono toccati. Cancellarli rimette quelle tracce nella lista di Analizza i mancanti nella pagina Libreria, così un conteggio dei battiti migliorato può sostituire i numeri scritti da una passata più vecchia
settings-storage-model-fallback-this = Questo modello
settings-storage-music-summary = { $tracks }, { $albums }, { $size }
settings-storage-model-weights = Pesi dei modelli
    .description = I modelli scaricati per l'analisi acustica (models/). La pagina Modelli ML è dove si recuperano e si eliminano, una riga un modello
settings-storage-models-empty = Modelli
    .description = Niente ha ancora descritto la libreria. Attivare l'analisi acustica nella pagina Libreria riempie questa voce, e ogni modello che ha girato prende una riga qui
settings-storage-music-files = File musicali
    .description = Cosa contengono le cartelle scansionate; i file restano dove sono
settings-storage-none = Nessuno
settings-storage-playlists-history = Playlist e cronologia
    .description = Le tue playlist e i loro membri, cosa hai riprodotto, e le note di genere della libreria. Tutto piccolo accanto al resto di library.db
settings-storage-reclaimable = Spazio recuperabile
    .description = Pagine dentro library.db che le eliminazioni hanno lasciato indietro. Le nuove scritture le riempiono di nuovo, così il file smette di crescere prima di iniziare a rimpicciolirsi
    .keywords = compatta riduci pulisci database
settings-storage-section-acoustic = Descrizioni acustiche
settings-storage-section-app-data = Dati dell'app
settings-storage-section-caches = Cache
settings-storage-section-diagnostics = Diagnostica
settings-storage-section-library = Libreria
settings-storage-section-tempo = Tempo
settings-storage-vectors = Vettori
    .description = Quanto pesa ogni descrizione dentro library.db. Su una libreria già percorsa dalla passata di analisi è la parte più grossa del file, un paio di kilobyte a traccia contro qualche centinaio di byte di tag
settings-storage-waveforms = Forme d'onda
    .description = La striscia dei picchi di ogni traccia, tenuta dopo la prima riproduzione; quelle cancellate si ridecodificano alla riproduzione successiva

## Settings: workspace
settings-workspace-card-author = Autore
settings-workspace-card-author-placeholder = Chi l'ha fatto
settings-workspace-card-created = Creato il { $date }
settings-workspace-card-created-updated = Creato il { $created }, aggiornato il { $updated }
settings-workspace-card-description = Descrizione
settings-workspace-card-description-placeholder = Cosa vuole essere il look
settings-workspace-card-empty = Questo spazio di lavoro non ha una scheda
settings-workspace-card-hint = La scheda è salvata nel file, così chi riceve questo look la vede
settings-workspace-card-license = Licenza
settings-workspace-card-license-placeholder = I termini con cui lo condividi
settings-workspace-card-save = Salva la scheda
settings-workspace-card-updated = Aggiornato il { $date }
settings-workspace-card-version = Versione
settings-workspace-card-version-placeholder = La tua versione, comunque tu la conti
settings-workspace-card-website = Sito web
settings-workspace-card-website-placeholder = Dove vive
settings-workspace-composition-closed = La finestra dello spazio di lavoro è chiusa
settings-workspace-composition-hint = I pannelli della finestra come sono disposti in divisioni e gruppi di schede; le frecce riordinano una riga tra i suoi pari, il lucchetto fissa un pannello al suo posto, e l'ingranaggio apre le sue impostazioni
settings-workspace-empty = Ancora nessuno spazio di lavoro
settings-workspace-hint = Uno spazio di lavoro è un look intero: layout, palette, aspetto. Applicarne uno sostituisce tutti e tre
settings-workspace-layout-name-placeholder = Nome del layout
settings-workspace-layouts-empty = Ancora nessun layout
settings-workspace-layouts-hint = Primario e mini sono i due tra cui alterna il pulsante mini player della barra dei menu
settings-workspace-name-placeholder = Nome dello spazio di lavoro
settings-workspace-panel-preset-unknown-kind = Pannello sconosciuto
settings-workspace-panel-presets-empty = Ancora nessuna preimpostazione di pannello
settings-workspace-panel-presets-hint-after = in qualsiasi menu del pannello. Valgono solo per questo spazio di lavoro; un altro spazio di lavoro non le avrà.
settings-workspace-panel-presets-hint-before = Un pannello configurato ciascuna, salvata dal menu di un pannello e recuperata da
settings-workspace-role-mini = Mini
settings-workspace-role-primary = Primario
settings-workspace-section-composition = Composizione
settings-workspace-section-layouts = Layout
settings-workspace-section-panel-presets = Preimpostazioni di pannello
settings-workspace-section-workspaces = Spazi di lavoro
settings-workspace-tree-empty-slot = Slot vuoto
settings-workspace-tree-split-column = Divisione, impilati
settings-workspace-tree-split-row = Divisione, affiancati
settings-workspace-tree-tabs = Schede

## Settings: development
settings-development-experimental-panels = Pannelli sperimentali
    .description = Mostra nel menu Pannelli e nel launcher i pannelli ancora in costruzione; cambiano forma tra una release e l'altra, e un layout che ne contiene già uno lo tiene quando questa opzione torna off
settings-development-section-features = Funzioni

## Settings: shared
settings-acoustic-analysis-heading = Analisi acustica
settings-analyze-nothing-scanned = Ancora niente di scansionato da analizzare
settings-common-active = Attivo
settings-common-analyze-missing = Analizza i mancanti
settings-common-built-in = Integrato
settings-common-clear = Cancella
settings-common-copy = Copia
settings-common-database = Database
settings-common-delete = Elimina
settings-common-download = Scarica
settings-common-rescan = Riscansiona
settings-common-reveal = Mostra
settings-common-stop = Ferma
settings-common-stopping = In arresto...
settings-common-tags = Tag
settings-common-tracks-count = { $count ->
    [one] { $count } traccia
   *[other] { $count } tracce
}
settings-common-use = Usa
settings-confirm-apply-body = Questo sostituisce i tuoi layout, la palette e l'aspetto con quelli dello spazio di lavoro.
settings-confirm-apply-imported-body = È salvato nei tuoi spazi di lavoro. Applicarlo ora sostituisce i tuoi layout, la palette e l'aspetto con quelli dello spazio di lavoro.
settings-confirm-clear = Cancella
settings-confirm-clear-embeddings-body = Le descrizioni se ne vanno e lo spazio torna. Per riaverle bisogna rieseguire la passata di analisi su ogni traccia della libreria.
settings-confirm-clear-embeddings-title = Cancellare ciò che "{ $model }" ha descritto?
settings-confirm-clear-measured-bpm-body = Ogni tempo ricavato da rox torna non misurato; i numeri dei tag dei tuoi file restano. Per riaverli bisogna rieseguire la passata del tempo su ognuna di quelle tracce.
settings-confirm-clear-measured-bpm-title = Cancellare i tempi misurati?
settings-confirm-overwrite-workspace-body = Questo sostituisce lo spazio di lavoro salvato con lo stato attuale.
settings-confirm-overwrite-workspace-title = Sovrascrivere lo spazio di lavoro "{ $name }"?
settings-sidebar-data-folder = Cartella dati
settings-sidebar-settings-file = File delle impostazioni

## Menubar
menu-about = Informazioni
menu-application = Applicazione
menu-apply-layout = Applica layout
menu-apply-workspace = Applica spazio di lavoro
menu-chat = Chat
menu-close = Chiudi
menu-console = Console
menu-design-mode = Modalità progettazione
menu-discussions = Discussioni
menu-empty-window = Finestra vuota
menu-equalizer = Equalizzatore
menu-exit = Esci
menu-hide-menubar = Nascondi la barra dei menu
menu-import-workspace = Importa spazio di lavoro...
menu-new-ellipsis = Nuovo...
menu-new-window = Nuova finestra
menu-new-window-from-layout = Nuova finestra da layout
menu-new-window-from-panel = Nuova finestra da pannello
menu-no-layouts = Nessun layout
menu-no-presets = Nessuna preimpostazione
menu-no-workspaces = Nessuno spazio di lavoro
menu-os-decorations = Decorazioni di sistema
menu-overlay-shader = Shader di overlay
menu-panel-built-in = Integrato
menu-panel-new = Nuovo...
menu-panel-no-layouts = Nessun layout
menu-panel-no-presets = Nessuna preimpostazione
menu-panel-no-workspaces = Nessuno spazio di lavoro
menu-panel-title = Menu
menu-panels = Pannelli
menu-panels-presets = Preimpostazioni
menu-pause = Pausa
menu-playback = Riproduzione
menu-remain-in-tray = Resta nell'area di notifica
menu-report-issue = Segnala un problema
menu-save-layout = Salva layout
menu-save-workspace = Salva spazio di lavoro
menu-section-add = Aggiungi
menu-section-app = App
menu-section-interface = Interfaccia
menu-section-layouts = Layout
menu-section-library = Libreria
menu-section-session = Sessione
menu-section-track = Traccia
menu-section-tuning = Regolazione
menu-settings = Impostazioni
menu-signals = Segnali
menu-song-theming = Colori del brano
menu-stats = Statistiche
menu-tasks = Attività
menu-update-available = Aggiornamento disponibile
menu-welcome = Benvenuto
menu-window = Finestra
menu-workspace = Spazio di lavoro
menu-workspace-builtin-tag = Integrato

## Workspaces
workspace-apply-body = Questo sostituisce tutto il look: layout, palette, aspetto.
workspace-apply-imported-body = È salvato nei tuoi spazi di lavoro. Applicarlo ora sostituisce tutto il look: layout, palette, aspetto.
workspace-apply-imported-title = "{ $name }" importato
workspace-apply-screen-shader-named = Applica lo shader di overlay { $name } su tutta la finestra.
workspace-apply-screen-shader-plain = Applica uno shader di overlay su tutta la finestra.
workspace-apply-shader-count = { $count ->
    [one] Include { $count } shader: { $names }
   *[other] Include { $count } shader: { $names }
}
workspace-apply-shaders-approve-body = Approvarli li lascia girare su questa macchina. Applicarlo senza di loro lascia il look spoglio, con gli shader comunque nel suo pool.
workspace-apply-shaders-plain-body = Applicarlo senza di loro lascia il look spoglio, con gli shader comunque nel suo pool.
workspace-byline-author = di { $author }
workspace-byline-version = versione { $version }
workspace-context-add-panel = Aggiungi pannello
workspace-dialog-apply = Applica
workspace-dialog-apply-title = Applicare "{ $name }"?
workspace-dialog-approve-apply = Approva e applica
workspace-dialog-cancel = Annulla
workspace-dialog-close = Chiudi
workspace-dialog-close-title = Chiudere "{ $name }"?
workspace-dialog-export = Esporta
workspace-dialog-layout-name-placeholder = Nome del layout
workspace-dialog-not-now = Non ora
workspace-dialog-overwrite = Sovrascrivi
workspace-dialog-overwrite-title = Sovrascrivere "{ $name }"?
workspace-dialog-save = Salva
workspace-dialog-save-layout-title = Salva layout
workspace-dialog-save-workspace-title = Salva spazio di lavoro
workspace-dialog-with-shaders = Con gli shader
workspace-dialog-without-shaders = Senza gli shader
workspace-dialog-workspace-name-placeholder = Nome dello spazio di lavoro
workspace-drop-add-queue = Aggiungi alla coda
workspace-drop-play-now = Riproduci ora
workspace-hint-or = o
workspace-hint-then = poi
workspace-import = Importa
workspace-launcher-hint = Aggiungi il tuo primo pannello per iniziare a costruire, oppure scegli una preimpostazione sotto Spazio di lavoro > Applica spazio di lavoro
workspace-launcher-need-help = Serve aiuto?
workspace-launcher-open-welcome = Apri la finestra di benvenuto
workspace-launcher-title = Una finestra vuota
workspace-layout-apply-body = Questo sostituisce il layout attuale di questa finestra.
workspace-layout-overwrite-body = Questo sostituisce il layout salvato con quello attuale.
workspace-layout-preset-restore-failed = Non è stato possibile ripristinare la preimpostazione di layout di questa finestra, quindi parte vuota.
workspace-layout-restore-failed = Non è stato possibile ripristinare il layout salvato, quindi questa finestra parte vuota.
workspace-mini-tip-back = Torna al layout intero
workspace-mini-tip-shrink = Riduci al mini player
workspace-overwrite-body = Questo sostituisce lo spazio di lavoro salvato con il look attuale.
workspace-panel-locked-close-body = Questo pannello è fissato al suo posto. Chiuderlo lo toglie dal layout.
workspace-save-current = Salva l'attuale
workspace-screen-shader-hint-before = Disattivalo quando vuoi con
workspace-workspace-restore-failed = Non è stato possibile ripristinare il layout dello spazio di lavoro, quindi questa finestra parte vuota.

## Tasks window
tasks-acoustic-all-described = Tutte le { $count } tracce scansionate sono descritte da { $label }
tasks-acoustic-off = Descrivere come suonano le tracce è disattivato nelle Impostazioni, sotto Libreria
tasks-acoustic-partial = { $label } descrive { $embedded } di { $total } tracce scansionate
tasks-analyzing = Analisi di { $progress }
tasks-bake-writing = Scrittura dei tag...
tasks-chip-count = { $count } attività
tasks-convert-starting = Avvio di ffmpeg...
tasks-converting = Conversione di { $progress }
tasks-count-of-total = { $done } di { $total }
tasks-embedding = Incorporazione di { $progress }
tasks-estimate-at = { $estimate } a { $workers }
tasks-import-failed = L'ultima importazione è fallita: { $error }
tasks-import-reading = Lettura dell'elenco dei brani amati...
tasks-import-unmatched = { $count } senza corrispondenza in questa libreria
tasks-importing = Importazione di { $progress }
tasks-job-acoustic = Analisi acustica
tasks-job-convert = Converti audio
tasks-job-loved-import = Brani amati di Last.fm
tasks-job-replaygain = ReplayGain
tasks-job-scan = Scansione della libreria
tasks-job-tempo = Analisi del tempo
tasks-last-pass-stopped = L'ultima passata si è fermata: { $reason }
tasks-last-run-finished = { $count ->
    [one] Ultima esecuzione finita, { $count } fatta
   *[other] Ultima esecuzione finita, { $count } fatte
}
tasks-last-run-stopped = Ultima esecuzione fermata dopo { $count }
tasks-library-busy = La libreria è occupata
tasks-library-scanning = La libreria è in scansione
tasks-measuring = Misurazione di { $progress }
tasks-model-downloading = Un modello si sta ancora scaricando
tasks-no-library-window = Non c'è nessuna finestra della libreria aperta, quindi non si possono avviare da qui
tasks-nothing-to-measure = Ancora niente di scansionato da misurare
tasks-rg-all-gain = Tutte le { $count } tracce hanno un guadagno a cui suonare
tasks-rg-partial = { $missing ->
    [one] { $missing } di { $total } tracce non ha guadagno
   *[other] { $missing } di { $total } tracce non hanno guadagno
}
tasks-scan-folder-count = { $count ->
    [one] { $count } cartella
   *[other] { $count } cartelle
}
tasks-scan-last-scanned = { $folders }, ultima scansione { $ago } fa
tasks-scan-never-scanned = { $folders }, mai scansionate
tasks-scan-no-folders = Ancora nessuna cartella aggiunta. Aggiungine una nelle Impostazioni, sotto Libreria
tasks-start-analyze-missing = Analizza i mancanti
tasks-start-measure-missing = Misura i mancanti
tasks-start-rescan = Riscansiona
tasks-stop = Ferma
tasks-stopping = Arresto...
tasks-tempo-all = Tutte le { $count } tracce hanno un tempo
tasks-tempo-off = Il calcolo della velocità delle tracce è disattivato nelle Impostazioni, sotto Libreria
tasks-tempo-partial = { $missing ->
    [one] { $missing } di { $total } tracce non ha un tempo
   *[other] { $missing } di { $total } tracce non hanno un tempo
}
tasks-timing = Cronometraggio di { $progress }
tasks-tip = Apri le attività della libreria
tasks-window-title = rox - Attività
tasks-working-out-missing = Calcolo cosa manca...

## Stats window
stats-bucket-listens = { $count ->
    [one] { $count } ascolto, { $ago }
   *[other] { $count } ascolti, { $ago }
}
stats-chart-start-all = Primo ascolto
stats-chart-start-month = 30 giorni fa
stats-chart-start-week = 7 giorni fa
stats-chart-start-year = Un anno fa
stats-click-opens = Il clic apre le statistiche
stats-click-section = Clic
stats-count-menu = Conteggio
    .description = Su quale periodo recente il numero conta gli ascolti; l'elenco al passaggio del mouse li mostra sempre tutti
stats-empty-all = Ancora nessun ascolto
stats-empty-range = Nessun ascolto in questo intervallo
stats-now = Adesso
stats-open = Apri le statistiche
stats-open-on-click = Apri le statistiche al clic
    .description = Clicca il widget per aprire la finestra delle statistiche, il registro completo degli ascolti
stats-play-these-tracks = Riproduci queste tracce
stats-play-this-track = Riproduci questa traccia
stats-plays-count = { $count ->
    [one] { $count } ascolto
   *[other] { $count } ascolti
}
stats-range-all = Sempre
stats-range-all-short = Tutto
stats-range-day-short = Giorno
stats-range-label = Intervallo
stats-range-month = Questo mese
stats-range-month-short = Mese
stats-range-today = Oggi
stats-range-week = Questa settimana
stats-range-week-short = Settimana
stats-range-year = Quest'anno
stats-range-year-short = Anno
stats-readout-section = Lettura
stats-section-listens = Ascolti
stats-section-listens-over-time = Ascolti nel tempo
stats-section-recent-listens = Ascolti recenti
stats-section-top-albums = Album principali
stats-section-top-artists = Artisti principali
stats-section-top-genres = Generi principali
stats-show-change = Mostra la variazione
    .description = Aggiunge un chip che confronta il periodo con quello precedente, in su o in giù; Sempre non ha niente prima con cui confrontarsi
stats-show-number = Mostra il numero
    .description = Disegna il conteggio accanto all'icona; disattivato lascia un'icona nuda con i conteggi al passaggio del mouse
stats-title = Widget statistiche
stats-tooltip-listens = Ascolti
stats-window-title = rox - Statistiche

## About window
about-check-failed = Impossibile raggiungere GitHub
about-check-for-updates = Cerca aggiornamenti
about-checking = Controllo...
about-download = Scarica
about-downloading = Scaricamento... { $percent }%
about-get-it = Scaricalo
about-license-lead = rox è software libero sotto la GNU AGPLv3. Il sorgente è su
about-notice-lead = Dovresti aver ricevuto una copia della licenza con questo programma. In caso contrario, vedi
about-release-notes = Note di rilascio
about-restart-now = Riavvia adesso
about-up-to-date = Sei all'ultima versione
about-update-failed = Aggiornamento fallito: { $error }
about-version = Versione { $version }
about-version-available = La versione { $version } è disponibile
about-version-ready = La versione { $version } è pronta
about-window-title = rox - Informazioni

## Welcome window
welcome-add-folder = Aggiungi cartella
welcome-and = e
welcome-back = Indietro
welcome-card-menubar-title = Barra dei menu
welcome-card-music-title = Musica
welcome-card-panels-title = Pannelli
welcome-card-playback-title = Riproduzione
welcome-card-rearranging-title = Riorganizzazione
welcome-card-settings-title = Impostazioni
welcome-close = Chiudi
welcome-design-mode-note = Per riorganizzare serve la Modalità progettazione, attiva di default in cima a quel menu. Disattivata blocca il layout, così una configurazione finita non si sposta.
welcome-done = Fatto
welcome-drop-note = Rilascialo sul bordo di un pannello per dividere lì, al centro per condividere un gruppo di schede, o fuori dalla finestra per farne una finestra a sé.
welcome-key-left-click = Clic sinistro
welcome-key-middle-mouse = Tasto centrale
welcome-layout-note = Salva una disposizione come layout; uno spazio di lavoro raccoglie layout e palette in un unico look condivisibile.
welcome-menubar-after = due volte per lasciarla su.
welcome-menubar-before = Con la barra dei menu nascosta, tieni premuto
welcome-menubar-mid = per farla tornare sopra il dock, o premi
welcome-music-note = rox la scansiona nella libreria e i file restano dove sono. Altre cartelle si aggiungono nelle impostazioni sotto libreria.
welcome-next = Avanti
welcome-or = o
welcome-panels-note = Ogni superficie è un pannello, e il menu Pannelli della barra dei menu ne apre altri.
welcome-playback-after = spostano nel brano.
welcome-playback-before = avvia e ferma la riproduzione;
welcome-quickplay-after = e parte.
welcome-quickplay-before = apre la riproduzione rapida: scrivi una traccia, premi
welcome-rearrange-after = ovunque su un pannello per spostarlo.
welcome-rearrange-before = Trascina una scheda, o tieni premuto
welcome-settings-hint-after = apre le impostazioni: la palette, la trasparenza e il comportamento.
welcome-shelf-caption = Sceglierne uno sostituisce il look della finestra principale e chiude il tour. Questa finestra è qui in ogni momento sotto Applicazione > Benvenuto.
welcome-stage-lead-quick-start = Scegli uno spazio di lavoro e la finestra principale passa a quello: layout, palette, tutto il look.
welcome-stage-lead-welcome = Foobar se fosse fatto nel 20XX.
welcome-stage-title-quick-start = Avvio rapido
welcome-stage-title-welcome = Benvenuto in rox
welcome-step-hint-after = , o con i pulsanti qui sotto.
welcome-step-hint-before = Scorri le tappe con
welcome-tile-by = di { $author }
welcome-tour-intro = Un giro rapido su dove entra la musica e dove si imposta il look. Finisce allo scaffale degli spazi di lavoro inclusi, un clic ciascuno.
welcome-window-title = rox - Benvenuto

## Console window
console-clear = Pulisci
console-copy = Copia
console-empty-filtered = Niente a questi livelli
console-empty-none = Ancora niente nel log
console-filter-error = Errore
console-filter-info = Info
console-filter-warn = Avviso
console-follow = Segui
console-line-count = { $count ->
    [one] { $count } riga
   *[other] { $count } righe
}
console-open-button = Apri la console
console-reveal = Mostra
console-window-title = rox - Console

## Signals window
signals-about-toggle = Informazioni sui segnali
signals-blurb-marked = I pannelli contrassegnati così nei menu possono avere quasi tutti i parametri collegati: fai clic destro su un parametro nelle impostazioni del pannello e scegli un segnale, o aggiungine uno da lì.
signals-blurb-shared = Quello che si regola qui è condiviso: una modifica vale per ogni parametro instradato su quel segnale, in ogni pannello e finestra.
signals-blurb-total = Un Totale è il quarto tipo: somma un altro segnale nel tempo e arrivato a 1 riparte da 0, quindi sale finché la musica è forte e si ferma quando non lo è. Usalo quando uno shader ha bisogno di una fase che si muove col brano invece che con l'orologio.
signals-blurb-what = Un segnale trasforma ciò che sta suonando in un numero tra 0 e 1: l'energia in una banda di frequenza, il livello dell'intero mix, o un impulso a ogni colpo dentro una banda. Risposta imposta quanto in fretta segue, Soglia lo zittisce sotto un livello che scegli tu.
signals-no-library = Non c'è nessuna finestra della libreria aperta, quindi questi non mostrano audio. Le modifiche si salvano lo stesso.
signals-window-title = rox - Segnali

## Equaliser
eq-analyzer-bars = Barre
eq-analyzer-off = Nessun analizzatore
eq-analyzer-wave = Onda
eq-band-badge = Badge delle bande
    .description = Mostra quante bande non sono piatte, su un badge sopra l'icona
eq-band-label = Banda { $number }
eq-click-nothing = Niente
eq-click-open = Apri
eq-click-section = Clic
    .description = Cosa fa un clic: aprire la finestra dell'equalizzatore, o accendere e spegnere l'intera curva sul posto
eq-click-toggle = Commuta
eq-flatten = Appiattisci
eq-freq-label = Freq
eq-gain-label = Guadagno
eq-heading = Equalizzatore
eq-help-text = Trascina una banda per spostarla, scorri sopra una per allargarla o stringerla. L'elaborazione avviene prima del buffer che passa l'audio alla scheda, quindi uno spostamento impiega fino a mezzo secondo per arrivare agli altoparlanti.
eq-hint-off = Clicca per spegnerlo
eq-hint-on = Clicca per accenderlo
eq-hint-open = Clicca per aprire l'equalizzatore
eq-open = Apri l'equalizzatore
eq-readout-curve = Curva
eq-readout-icon = Icona
eq-readout-section = Lettura
    .description = L'icona, la curva di risposta come sparkline, o entrambe. La curva ha bisogno di una cinquantina di pixel di larghezza per essere leggibile
eq-reset-bands = Reimposta le bande
eq-shape-active = { $count ->
    [one] { $count } banda non piatta, picco { $peak } dB
   *[other] { $count } bande non piatte, picco { $peak } dB
}
eq-shape-flat = Piatto, ogni banda a 0 dB
eq-status-off = Equalizzatore spento
eq-status-on = Equalizzatore acceso
eq-title = Widget EQ
eq-widget-section = Widget
eq-width-label = Larghezza
eq-window-title = rox - Equalizzatore

## Keymap
keymap-close-window = Chiudi finestra
    .description = Chiude la finestra in primo piano. Assegnata ovunque, pannelli staccati compresi
keymap-decrease-font-size = Riduci la dimensione del testo
    .description = Abbassa di un passo la dimensione del testo in tutta l'app
keymap-focus-search = Vai alla ricerca
    .description = Mette il cursore nel campo di ricerca della libreria
keymap-group-editing = Modifica
keymap-group-playback = Riproduzione
keymap-group-view = Vista
keymap-group-windows = Finestre
keymap-increase-font-size = Aumenta la dimensione del testo
    .description = Alza di un passo la dimensione del testo in tutta l'app
keymap-key-backspace = Backspace
keymap-key-delete = Canc
keymap-key-down = Giù
keymap-key-end = Fine
keymap-key-esc = Esc
keymap-key-home = Home
keymap-key-insert = Ins
keymap-key-left = Sinistra
keymap-key-page-down = Pag giù
keymap-key-page-up = Pag su
keymap-key-right = Destra
keymap-key-space = Spazio
keymap-key-tab = Tab
keymap-key-up = Su
keymap-mod-alt = Alt
keymap-mod-cmd = Cmd
keymap-mod-ctrl = Ctrl
keymap-mod-fn = Fn
keymap-mod-option = Option
keymap-mod-shift = Maiusc
keymap-mod-super = Super
keymap-mod-win = Win
keymap-open-quick-play = Riproduzione rapida
    .description = Apre il prompt cerca-e-riproduci sopra la finestra
keymap-open-settings = Apri le impostazioni
    .description = Apre questa finestra
keymap-open-stats = Apri le statistiche
    .description = Apre la finestra delle statistiche di ascolto
keymap-quit = Esci
    .description = Esce da rox. Assegnata ovunque, visto che non c'è finestra da cui non debba funzionare
keymap-reset-font-size = Reimposta la dimensione del testo
    .description = Riporta la dimensione del testo a quella di serie
keymap-seek-backward = Sposta indietro
    .description = Torna indietro nella traccia in riproduzione
keymap-seek-forward = Sposta avanti
    .description = Va avanti nella traccia in riproduzione
keymap-stamp-line = Marca la riga del testo
    .description = Scrive la posizione di riproduzione sulla riga di testo in modifica
keymap-toggle-playback = Riproduci / Pausa
    .description = Avvia la traccia corrente, o la mette in pausa dov'è
keymap-toggle-post-shader = Attiva o disattiva lo shader di overlay
    .description = Spegne e accende lo shader dello schermo. Assegnata ovunque, visto che uno shader può coprire i controlli con cui altrimenti lo si disattiverebbe
keymap-toggle-zoom = Ingrandisci il gruppo di pannelli
    .description = Riempie il dock con l'ultimo gruppo di pannelli cliccato, o ne esce

## Panel catalog
panel-catalog-album-carousel = Carosello album
panel-catalog-artist-grid = Griglia artisti
panel-catalog-biography = Biografia
panel-catalog-cover-art = Copertina
panel-catalog-drawer = Cassetto
panel-catalog-eq-widget = Widget EQ
panel-catalog-filter = Filtro
panel-catalog-folder-tree = Albero delle cartelle
panel-catalog-genre-grid = Griglia generi
panel-catalog-group-application = Applicazione
panel-catalog-group-arrangement = Disposizione
panel-catalog-group-catalogue = Catalogo
panel-catalog-group-controls = Controlli
panel-catalog-group-details = Dettagli
panel-catalog-group-experimental = Sperimentali
panel-catalog-group-visualizers = Visualizzatori
panel-catalog-history = Cronologia
panel-catalog-menu = Menu
panel-catalog-metadata = Metadati
panel-catalog-mini-toggle = Interruttore mini
panel-catalog-oscilloscope = Oscilloscopio
panel-catalog-overlay = Overlay
panel-catalog-particles = Particelle
panel-catalog-playlists = Playlist
panel-catalog-queue = Coda
panel-catalog-queue-widget = Widget coda
panel-catalog-seek = Avanzamento
panel-catalog-slide = Diapositiva
panel-catalog-spectrogram = Spettrogramma
panel-catalog-spectrum = Spettro
panel-catalog-stats-widget = Widget statistiche
panel-catalog-status = Stato
panel-catalog-theme-toggle = Interruttore tema
panel-catalog-track-info = Info traccia
panel-catalog-vu-meter = VU meter
panel-catalog-waveform = Forma d'onda
panel-catalog-window-controls = Controlli finestra

## Updater
updater-already-latest = già all'ultima versione
updater-checksum-mismatch = il checksum del download è { $digest }, non il { $expected } dichiarato dalla release
updater-checksum-missing-entry = { $sums } non ha una voce per { $name }; un download non verificabile viene rifiutato
updater-no-asset = la release non ha { $name }
updater-no-checksums = la release non ha { $sums }; un download non verificabile viene rifiutato
updater-no-release-build = nessuna build di release per questa piattaforma
updater-overran = il download ha superato la dimensione dichiarata dalla release
updater-short = il download si è fermato a { $done } di { $bytes } byte
updater-size-mismatch = il server ha offerto { $claimed } byte, la release ne dichiara { $bytes }

## Last.fm
lastfm-import-matching = Confronto con la libreria
lastfm-import-read = { $count ->
    [one] Letta { $count } traccia preferita
   *[other] Lette { $count } tracce preferite
}
lastfm-import-stopped = { $count ->
    [one] Fermato dopo { $count } traccia preferita
   *[other] Fermato dopo { $count } tracce preferite
}
lastfm-import-matched = , { $count } con corrispondenza
lastfm-import-added = { $count ->
    [one] , { $count } aggiunta ai preferiti
   *[other] , { $count } aggiunte ai preferiti
}

## Tag tools
tags-editor-clear-all = pulisci tutto
tags-editor-form-view = Modulo
tags-editor-format-unsupported-all = I tag di questo formato non si possono ancora leggere o scrivere.
tags-editor-format-unsupported-some = Alcuni di questi file sono in un formato i cui tag non si possono ancora leggere o scrivere.
tags-editor-guess-button = Indovina
tags-editor-guess-folded = { $count ->
    [one] { $status }, { $count } altro non mostrato
   *[other] { $status }, altri { $count } non mostrati
}
tags-editor-guess-help = { $placeholders }; / corrisponde alla cartella sopra, %skip% scarta
tags-editor-guess-match-count = { $hits ->
    [one] { $hits } di { $total } corrisponde
   *[other] { $hits } di { $total } corrispondono
}
tags-editor-guess-no-match = nessuna corrispondenza
tags-editor-guess-pattern-label = schema
tags-editor-loading = Caricamento dei tag...
tags-editor-look-up = Cerca
tags-editor-multiple-values = Valori multipli
tags-editor-clear-on-save = Pulizia al salvataggio
tags-editor-other-tags = Altri tag ({ $count })
tags-editor-remove = rimuovi
tags-editor-reveal = Mostra
tags-editor-save-errors = { $count ->
    [one] { $count } file fallito; { $error }
   *[other] { $count } file falliti; { $error }
}
tags-editor-saving-progress = Salvataggio { $done }/{ $total }...
tags-editor-table-view = Tabella
tags-editor-tags-section = Tag
tags-editor-unknown-partial = { $count } di { $total }
tags-editor-unread-count = Non è stato possibile leggere i tag di { $failed } file su { $total }
tags-editor-will-clear = verrà pulito
tags-editor-will-remove = verrà rimosso
tags-editor-window-title = rox - Editor dei tag
tags-guess-empty-segment = lo schema produce un nome di cartella o file vuoto
tags-guess-no-placeholders = nessun segnaposto
tags-guess-skip-renders-nothing = %skip% non ha niente da produrre
tags-guess-unclosed = % non chiuso
tags-guess-unknown-placeholder = segnaposto sconosciuto %{ $name }%
tags-matcher-blocked-arm = Attiva un campo per applicare
tags-matcher-blocked-no-match = Nessuna corrispondenza da applicare
tags-matcher-blocked-pick = Scegli una corrispondenza
tags-matcher-blocked-writing = Scrittura dei tag...
tags-matcher-match-count = { $count ->
    [one] 1 corrispondenza
   *[other] { $count } corrispondenze
}
tags-matcher-no-matches = Nessuna corrispondenza trovata
tags-matcher-pick-match = Scegli una corrispondenza
tags-matcher-search-failed = Ricerca fallita: { $error }
tags-matcher-searching = Ricerca...
tags-matcher-tagging = Applicazione dei tag a { $track }
tags-matcher-window-title = rox - Trova metadati
tags-rename-blocked-cue = traccia cue, senza un file proprio
tags-rename-blocked-duplicate = due tracce puntano a questo nome
tags-rename-blocked-occupied = c'è già un file
tags-rename-blocked-outside-roots = fuori da ogni radice della libreria
tags-rename-blocked-unresolved = non ancora nel catalogo
tags-rename-move-error = { $name }: { $error }
tags-rename-move-errors = { $count ->
    [one] { $count } file fallito; { $error }
   *[other] { $count } file falliti; { $error }
}
tags-rename-moving = Spostamento { $done }/{ $total }...
tags-rename-nothing-to-move = Niente da spostare
tags-rename-pattern-help = { $placeholders }; / crea una cartella, l'estensione segue il file
tags-rename-pattern-section = Schema
tags-rename-preview-section = Anteprima
tags-rename-unchanged = invariato
tags-rename-will-move = { $count ->
    [one] { $count } di { $total } verrà spostato
   *[other] { $count } di { $total } verranno spostati
}
tags-rename-window-title = rox - Rinomina file
tags-repair-affected-files = File interessati
tags-repair-section = Riparazione
tags-repair-check-to-repair = Spunta un file per ripararlo
tags-repair-count = { $count ->
    [one] 1 file
   *[other] { $count } file
}
tags-repair-count-so-far = { $count } finora
tags-repair-label-scope = ambito
tags-repair-no-affected = Nessun file interessato trovato.
tags-repair-no-folder = Nessuna cartella da scansionare; aggiungine una alla libreria o scegline una.
tags-repair-pick-folder = Scegli una cartella...
tags-repair-progress = Riparazione { $done }/{ $total }...
tags-repair-repair-button = { $count ->
    [0] Ripara
   *[other] Ripara ({ $count })
}
tags-repair-result = { $count ->
    [one] Riparato 1 file
   *[other] Riparati { $count } file
}
tags-repair-result-failed = { $count ->
    [one] Riparato { $count }, { $failed ->
        [one] { $failed } fallito
       *[other] { $failed } falliti
    }
   *[other] Riparati { $count }, { $failed ->
        [one] { $failed } fallito
       *[other] { $failed } falliti
    }
}
tags-repair-scan-first = Prima scansiona
tags-repair-scan-hint = Scansiona per trovare file con tag danneggiati che una riscrittura ripara.
tags-repair-select-all = Seleziona tutto
tags-repair-select-none = Deseleziona tutto
tags-repair-whole-library = Tutta la libreria
tags-repair-window-title = rox - Riparazione tag

## Convert
convert-arg-names-file = "{ $token }" nomina un file; la destinazione viene dalla cartella e dallo schema
convert-section-output = Output
convert-section-preview = Anteprima
convert-arg-not-flag-or-value = "{ $token }" non è un flag né un valore per uno
convert-check-wrote-nothing = ffmpeg è uscito senza errori ma non ha scritto niente
convert-custom-ext-empty = L'estensione sceglie il contenitore, quindi ne serve una
convert-custom-ext-invalid = "{ $ext }" non è un nome di contenitore; lettere e cifre, senza punto
convert-dialog-browse = Sfoglia...
convert-dialog-check-passed = ffmpeg ha codificato un attimo di silenzio con questi, quindi funzionano
convert-dialog-check-waiting = La verifica con ffmpeg parte quando smetti di scrivere
convert-dialog-checking = Verifica con ffmpeg in corso...
convert-dialog-choose-folder = Scegli una cartella in cui scrivere
convert-dialog-convert-button = Converti
convert-dialog-custom-label = Personalizzato
convert-dialog-custom-menu-item = Personalizzato...
convert-dialog-custom-note = Gli argomenti si dividono sugli spazi, quindi niente virgolette; la copertina incorporata non viene copiata nei formati personalizzati
convert-dialog-format-not-ready = Il formato digitato non ha ancora passato la verifica di ffmpeg
convert-dialog-label-extension = estensione
convert-dialog-label-format = formato
convert-dialog-label-into = in
convert-dialog-label-named = chiamato
convert-dialog-mirror = Rispecchia le cartelle della libreria
convert-dialog-nothing-to-convert = Niente da convertire: ogni riga è saltata
convert-dialog-pattern-help = { $placeholders }; / crea una cartella, il formato imposta l'estensione
convert-dialog-pick-folder = Seleziona una cartella in cui scrivere
convert-dialog-span-note = { $count ->
    [one] { $count } ritagliato da un'immagine cue e taggato dalla libreria
   *[other] { $count } ritagliati da un'immagine cue e taggati dalla libreria
}
convert-dialog-will-convert = { $count ->
    [one] { $count } di { $total } verrà convertito
   *[other] { $count } di { $total } verranno convertiti
}
convert-dialog-window-title = rox - Converti
convert-ffmpeg-silent-failure = ffmpeg ha fallito senza dire perché
convert-flag-attach = -attach legge un file suo, cosa che qui non è permessa
convert-flag-f = È l'estensione a scegliere il contenitore, quindi -f non si tocca
convert-flag-i = L'ingresso è la traccia che hai scelto, quindi -i non si tocca
convert-flag-n = -n c'è già a ogni esecuzione
convert-flag-y = Qui niente sovrascrive, quindi -y non è disponibile; una destinazione che esiste viene saltata
convert-preset-flac = FLAC
convert-preset-mp3-320 = MP3 320 kbps
convert-preset-mp3-v0 = MP3 V0
convert-preset-opus-192 = Opus 192 kbps
convert-preset-wav = WAV
convert-skip-duplicate = due tracce puntano a questo nome
convert-skip-exists = già presente
convert-summary-failed = { $count ->
    [one] , { $count } fallito
   *[other] , { $count } falliti
}
convert-summary-files = { $count ->
    [one] 1 file
   *[other] { $count } file
}
convert-summary-line = { $files } in { $dest }
convert-summary-skipped = { $count ->
    [one] , { $count } saltato
   *[other] , { $count } saltati
}
convert-summary-stopped = Fermato dopo { $files } in { $dest }
convert-version-answered = { $binary } è partito, ma non ha riportato una versione

## Duplicates
duplicates-auto-select = Selezione automatica
duplicates-check-to-trash = Spunta le copie per cestinarle
duplicates-copy-count = { $count ->
    [one] 2 copie
   *[other] { $count } copie
}
duplicates-different-albums = album diversi
duplicates-filter-placeholder = Filtra per titolo, artista o cartella
duplicates-groups-summary = { $groups ->
    [one] 1 gruppo, { $extras ->
        [one] { $extras } copia in più
       *[other] { $extras } copie in più
    }
   *[other] { $groups } gruppi, { $extras ->
        [one] { $extras } copia in più
       *[other] { $extras } copie in più
    }
}
duplicates-library-loading = La libreria si sta ancora caricando; riprova tra poco.
duplicates-no-duplicates = Nessun duplicato trovato.
duplicates-no-filter-matches = Nessun gruppo corrisponde al filtro.
duplicates-policy-newest = Tieni il più recente
duplicates-policy-oldest = Tieni il più vecchio
duplicates-policy-quality = Tieni la qualità migliore
duplicates-scan-hint = Scansiona la libreria per trovare tracce che compaiono più di una volta.
duplicates-select-none = Deseleziona tutto
duplicates-selected-count = { $count ->
    [one] { $count } selezionato
   *[other] { $count } selezionati
}
duplicates-trash-button = { $count ->
    [0] Cestina
   *[other] Cestina ({ $count })
}
duplicates-trash-error = { $name }: { $error }
duplicates-trash-result = { $count ->
    [one] Spostato 1 file nel cestino
   *[other] Spostati { $count } file nel cestino
}
duplicates-trash-result-failed = { $count ->
    [one] Spostato { $count } nel cestino, { $failed ->
        [one] { $failed } fallito
       *[other] { $failed } falliti
    }
   *[other] Spostati { $count } nel cestino, { $failed ->
        [one] { $failed } fallito
       *[other] { $failed } falliti
    }
}
duplicates-trashing = Spostamento nel cestino { $done }/{ $total }...
duplicates-window-title = rox - Duplicati

## Smart playlists
smart-playlist-descending = Decrescente
smart-playlist-edit-title = Modifica playlist intelligente
smart-playlist-limit-label = Limite
smart-playlist-limit-placeholder = Nessun limite
smart-playlist-match-count = { $count ->
    [one] 1 traccia corrisponde
   *[other] { $count } tracce corrispondono
}
smart-playlist-matched-tracks = Tracce corrispondenti
smart-playlist-new-title = Nuova playlist intelligente
smart-playlist-no-matches = Nessuna traccia corrisponde
smart-playlist-query-label = Query
smart-playlist-sort-default = Ordine predefinito
smart-playlist-sort-added = Data di aggiunta
smart-playlist-sort-label = Ordinamento
smart-playlist-unknown-field = "{ $field }:" non è un campo, quindi il termine corrisponde come testo semplice
smart-playlist-window-title = rox - { $verb }

## Playlist creation
playlist-create-not-savable = Dai un nome alla playlist per salvarla
playlist-create-placeholder = Nome della playlist
playlist-create-rename-title = Rinomina playlist
playlist-create-title = Nuova playlist
playlist-create-window-title = rox - { $verb }

## Cover tools
cover-art-back = Retro
cover-art-disc = Disco
cover-art-front = Fronte
cover-artwork = Immagine
    .description = Quale immagine mostrare; uno slot che il file non ha ricade sulla copertina frontale
cover-disc-style = Stile del disco
    .description = Rende l'immagine come un CD o come l'etichetta di un disco in vinile
cover-disc-off = Off
cover-disc-cd = CD
cover-disc-vinyl = Vinile
cover-editor-choose-image = Scegli un'immagine
cover-editor-multiple = Più immagini
cover-editor-none = Nessuna
cover-editor-not-an-image = Quel file non è un'immagine che rox può incorporare
cover-editor-not-decoded = Non è stato possibile decodificare quell'immagine
cover-editor-reading = Lettura della copertina attuale...
cover-editor-remove = Rimuovi
cover-editor-replace = Sostituisci
cover-editor-revert = Ripristina
cover-editor-save-errors = { $count ->
    [one] { $count } file fallito; { $error }
   *[other] { $count } file falliti; { $error }
}
cover-editor-saving-progress = Salvataggio { $done }/{ $total }...
cover-editor-search-online = Cerca online
cover-editor-section = Copertina
cover-editor-slot-back = Copertina posteriore
cover-editor-slot-front = Copertina frontale
cover-editor-slot-media = Supporto
cover-editor-will-remove = Verrà rimossa
cover-editor-window-title = rox - Copertina
cover-matcher-blocked-fetching = Recupero dell'immagine intera...
cover-matcher-blocked-no-cover = Nessuna copertina da impostare
cover-matcher-blocked-pick = Scegli una copertina per impostarla
cover-matcher-cover-count = { $count ->
    [one] 1 copertina
   *[other] { $count } copertine
}
cover-matcher-editor-closed = L'editor della copertina è stato chiuso
cover-matcher-no-covers = Nessuna copertina trovata
cover-matcher-search-failed = Ricerca fallita: { $error }
cover-matcher-set-cover = Imposta la copertina
cover-matcher-setting = Impostazione...
cover-matcher-tile-info = { $provider }  { $width }px
cover-matcher-unsupported-format = Formato immagine non supportato
cover-matcher-window-title = rox - Trova copertina
cover-spin = Rotazione
    .description = Ruota il disco mentre una traccia suona; vale per lo slot disco o per uno stile disco
cover-spin-disc = Ruota il disco
cover-spin-ramp = Rampa di rotazione
    .description = Quanto ci mette il disco ad arrivare a piena velocità, e a rallentare per inerzia
cover-spin-speed = Velocità di rotazione
    .description = Velocità piena, in giri al minuto
cover-stretch = Estendi
    .description = Riempie il pannello, ignorando le proporzioni dell'immagine
cover-stretch-to-fill = Estendi per riempire
cover-title = Copertina

## Lyrics
lyrics-always-centered = Sempre al centro
    .description = Riempie le estremità così anche la prima e l'ultima riga possono stare al centro
lyrics-auto-search = Ricerca automatica
    .description = Cerca online su una traccia senza testo e salva una corrispondenza sicura, senza selettore
lyrics-bold = Grassetto
lyrics-build-word-by-word = Costruisci parola per parola
    .description = Rivela le parole man mano che vengono cantate, stile karaoke; le righe non ancora cantate restano nascoste
lyrics-edge-bottom = Basso
lyrics-edge-top = Alto
lyrics-edit-hint-after-stamp = per marcare
lyrics-edit-hint-or = o
lyrics-edit-loading = Caricamento del testo...
lyrics-edit-lyrics = Modifica il testo
lyrics-edit-saving = Salvataggio...
lyrics-edit-section = Testo
lyrics-edit-stamp = Marca
lyrics-edit-stamp-time = Marca { $time }
lyrics-edit-window-title = rox - Modifica testo
lyrics-fade-lines-in = Dissolvi le righe in entrata
    .description = Accende in dissolvenza una riga attenuata quando diventa quella attiva
lyrics-falloff-edge = Lato di attenuazione
    .description = Quale lato della riga attiva viene smorzato dall'attenuazione
lyrics-find-online = Trova il testo online...
lyrics-follow-playback = Segui la riproduzione
    .description = Fa scivolare la riga attiva al centro mentre scorre un testo sincronizzato
lyrics-font = Carattere
    .description = Il carattere dei testi; il predefinito segue quello dell'app
lyrics-gap-threshold = Soglia dello stacco
    .description = Quanto deve durare un'intro o uno stacco prima di ricevere una pausa
lyrics-lead-in-rest = Pausa d'ingresso
    .description = Mostra una pausa vuota prima di un'intro lunga, così la prima riga appare in dissolvenza quando arriva
lyrics-line-falloff = Attenuazione delle righe
    .description = Quanto si smorza ogni riga per ogni passo di distanza da quella attiva
lyrics-line-spacing = Interlinea
    .description = Quanto distano tra loro le righe sincronizzate, come multiplo della dimensione del testo
lyrics-mark-dots = Punti
lyrics-mark-note = Nota
lyrics-matcher-blocked-no-match = Nessuna corrispondenza da applicare
lyrics-matcher-blocked-pick = Scegli una corrispondenza da applicare
lyrics-matcher-blocked-saving = Salvataggio delle parole...
lyrics-matcher-match-count = { $count ->
    [one] 1 corrispondenza
   *[other] { $count } corrispondenze
}
lyrics-matcher-no-query = Questa traccia non ha artista e titolo su cui cercare
lyrics-matcher-pick-preview = Scegli una corrispondenza da vedere in anteprima
lyrics-matcher-search-failed = Ricerca fallita: { $error }
lyrics-matcher-synced-tag = { $provider }  sincronizzato
lyrics-matcher-window-title = rox - Trova testo
lyrics-no-lyrics-notice = Nessun testo
lyrics-no-lyrics-track = Nessun testo per questa traccia
lyrics-rest-in-gaps = Pausa negli stacchi
    .description = Passa a una pausa vuota durante un lungo stacco strumentale invece di tenere l'ultima riga
lyrics-rest-marker = Segno di pausa
    .description = Cosa mostra una riga senza parole in un testo sincronizzato, gli stacchi e le righe vuote
lyrics-search-button = Pulsante di ricerca online
    .description = Mostra il pulsante di ricerca sulla schermata vuota; il menu col clic destro trova comunque i testi
lyrics-search-online = Cerca online
lyrics-show-song-name = Mostra il nome del brano
    .description = Mostra il nome della traccia sulla schermata vuota, sopra la riga del testo mancante
lyrics-text-size = Dimensione testo
    .description = Il testo del brano; l'altezza delle righe sincronizzate lo segue
lyrics-title = Testo
lyrics-title-unsynced = Titolo sui non sincronizzati
    .description = Fissa il titolo della traccia sopra un testo non sincronizzato, così un pannello basso lo mostra comunque
lyrics-wipe-lyrics = Cancella i testi

## Analysis passes
pass-acoustic-body = { $model } ricava come suona ognuna, così la libreria può trovare musica che assomiglia a ciò che è in riproduzione. Tutto gira su questa macchina, e ciò che è già descritto viene saltato. { $lands }
pass-acoustic-lands-database = I risultati vanno nel database della libreria e i tuoi file restano intatti.
pass-acoustic-lands-tags = I risultati vanno nel database della libreria e, per MP3 e FLAC, anche nei tag di ogni file, così restano anche se il database viene ricostruito. Gli altri formati tengono solo la copia nel database.
pass-acoustic-title = { $count ->
    [one] Analizzare 1 traccia?
   *[other] Analizzare { $count } tracce?
}
pass-analyze = Analizza
pass-estimate-at = { $estimate } con { $workers_phrase }.
pass-estimate-button = Stima
pass-estimating = Stima in corso...
pass-measure = Misura
pass-no-estimate = Su questa macchina non è ancora stato eseguito niente, quindi non c'è una stima. Stima cronometra qualche traccia e ricava il resto da lì.
pass-replaygain-body = Ogni file viene decodificato e misurato così può suonare al volume a cui è stato masterizzato. Gli album si misurano interi quando a tutte le loro tracce manca il guadagno. { $lands }
pass-replaygain-lands-database = I numeri vanno nel database della libreria e i tuoi file restano intatti.
pass-replaygain-lands-tags = I numeri vengono riscritti nei tag di ogni file, dove ogni altro lettore li legge.
pass-replaygain-title = { $count ->
    [one] Misurare 1 traccia?
   *[other] Misurare { $count } tracce?
}
pass-tempo-body = Di ogni file vengono decodificate due finestre da mezzo minuto e contati i battiti, così la libreria può mostrare a che andatura va una traccia. Funziona meglio sulla musica registrata a click e salta tutto ciò che non riesce a misurare. I numeri vanno nel database della libreria e i tuoi file restano intatti.
pass-tempo-title = { $count ->
    [one] Trovare il tempo di 1 traccia?
   *[other] Trovare il tempo di { $count } tracce?
}
pass-timing = Cronometraggio di qualche traccia...
pass-timing-failed = Impossibile cronometrare questa libreria: { $error }
pass-workers = Worker

## Quick play
quick-play-comfortable-rows = Righe comode
    .description = Dai più altezza a ogni risultato
quick-play-cover = Copertina
    .description = Mostra una miniatura della copertina a sinistra di ogni risultato
quick-play-duration = Durata
    .description = Mostra la durata di ogni risultato a destra
quick-play-narrow-by = Restringi per
quick-play-search-placeholder = Cerca nella libreria
quick-play-subtitle = Sottotitolo
    .description = Mostra artista e album sotto ogni risultato
quick-play-tag-album = Album
quick-play-tag-artist = Artista

## Drawer panel
drawer-add-tooltip = Aggiungi pannello cassetto
drawer-answers = Risponde a
    .description = Quali selezioni aprono il cassetto: solo il suo pannello principale, o qualsiasi pannello esterno
drawer-dim = Attenua
    .description = Quanto si attenua il pannello principale dietro il cassetto aperto
drawer-edge = Lato
    .description = Il lato contro cui poggia il cassetto e da cui scivola fuori
drawer-edge-bottom = Basso
drawer-edge-top = Alto
drawer-handle = Maniglia
    .description = Mostra la presa sul lato del pannello. Nascosta, del cassetto non si vede niente finché non c'è una scelta, e la presa poi resta finché dura la selezione, così un cassetto che si è richiuso si può tirare di nuovo fuori
drawer-open-on = Apri con
    .description = Passare sulla maniglia apre sempre il cassetto; Selezione lo apre anche a una scelta nel pannello principale
drawer-pin-open = Tieni aperto
drawer-reveal = Apertura
    .description = Quanto del pannello copre il cassetto aperto
drawer-scope-elsewhere = Altrove
drawer-scope-main = Pannello principale
drawer-title = Cassetto
drawer-trigger-hover = Passaggio
drawer-trigger-selection = Selezione

## Mini player
mini-tip-back = Torna al layout completo
mini-tip-none = Nessun layout mini assegnato
mini-tip-shrink = Riduci al lettore mini
mini-title = Interruttore mini

## System tray
tray-open = Apri
tray-pause = Pausa
tray-play = Riproduci
tray-quit = Esci

## Window controls
window-controls-mini-toggle = Interruttore mini
    .description = Metti davanti l'interruttore del layout mini; compare quando un layout mini è assegnato
window-controls-minimize = Riduci a icona
window-controls-style = Stile
    .description = Icone piatte, o i semafori di macOS
window-controls-style-icons = Icone
window-controls-title = Controlli finestra
window-controls-traffic-lights = Semafori

## Section names the audio panels share. Each one heads the same kind of
## rows in the spectrum, VU, oscilloscope, spectrogram and particles
## settings, so they're defined once rather than per panel.
viz-section-analysis = Analisi
viz-section-color = Colore
viz-section-peaks = Picchi
viz-section-playback = Riproduzione
viz-section-scale = Scala
viz-section-signal = Segnale

## Particles panel
particles-add-emitter = Aggiungi emettitore
particles-aim = Mira
particles-aim-fixed = Fissa
particles-aim-outward = Verso l'esterno
particles-burst = Raffica
particles-color = Colore
particles-cone = Cono
particles-direction = Direzione
    .description = Da che parte tira; 0 è su, 180 è giù
particles-drag = Attrito
    .description = Quanta velocità mangia l'aria ogni secondo; zero è il vuoto
particles-drift = Deriva
    .description = Quanto veloce si muove il campo stesso, così i vortici non stanno fermi
particles-edit-emitters = Modifica emettitori
particles-emitter-label = Emettitore { $index }
particles-emitter-target = Emettitore { $index } { $target }
particles-emitters-empty = Non ci sono ancora emettitori. Aggiungine uno per avviare il campo.
particles-glow = Bagliore
    .description = Posa un alone morbido dietro ogni particella
particles-gravity = Gravità
particles-gravity-strength = Intensità
    .description = Tiro costante su tutto ciò che è in volo
particles-height = Altezza
particles-hold-on-pause = Blocca in pausa
    .description = Congela il campo mentre è in pausa invece di lasciarlo andare alla deriva
particles-length = Lunghezza
particles-lifetime = Vita
particles-position-x = Posizione X
particles-position-y = Posizione Y
particles-radius = Raggio
particles-rate = Frequenza
particles-rotation = Rotazione
particles-round-particles = Particelle tonde
    .description = Disegna punti invece di quadrati
particles-scale = Scala
    .description = Quanto è largo un vortice; piccolo ribolle, grande rotola
particles-section-emitters = Emettitori
particles-section-medium = Mezzo
particles-section-particles = Particelle
particles-shape = Forma
particles-shape-box = Rettangolo
particles-shape-line = Linea
particles-shape-point = Punto
particles-shape-ring = Anello
particles-size = Dimensione
particles-speed = Velocità
particles-trigger = Trigger
particles-trigger-continuous = Continuo
particles-turbulence = Turbolenza
particles-turbulence-drift = Deriva turbolenza
particles-turbulence-scale = Scala turbolenza
particles-turbulence-strength = Intensità
    .description = Quanto forte il campo spinge in giro le particelle; zero è spento
particles-width = Larghezza

## Spectrum panel
spectrum-axis-labels = Etichette degli assi
    .description = Segna l'intervallo lungo il pannello: ottave (C1, C2, ...) o frequenze (100, 1k, 10k)
spectrum-bar-gap = Spazio tra le barre
    .description = Spazio tra le barre, spazi più larghi fanno stare meno barre
spectrum-bar-width = Larghezza barra
    .description = Quanto spessa si disegna ogni barra, barre più sottili fanno stare più bande
spectrum-block-gap = Spazio tra i blocchi
    .description = La giunzione tra le celle di una pila
spectrum-block-height = Altezza blocco
    .description = Quanto alta si disegna ogni cella di una pila
spectrum-cap-gravity = Gravità dei picchi
    .description = Quanto forte cadono i segni di picco quando la banda si abbassa
spectrum-fft-size = Dimensione FFT
    .description = Finestra di analisi; corta reagisce in fretta, lunga separa meglio le frequenze
spectrum-gradient-base-color = Colore di base
    .description = L'estremo silenzioso della rampa personalizzata
spectrum-gradient-cover = Copertina
spectrum-gradient-mode = Sfumatura
    .description = Colora le bande per volume: la rampa del tema, i colori della copertina con i colori del brano attivi, o una coppia personalizzata
spectrum-gradient-theme = Tema
spectrum-gradient-tip-color = Colore di punta
    .description = L'estremo forte della rampa personalizzata
spectrum-high-bound-description = Frequenza più alta analizzata dalle barre
spectrum-high-fft-size = Dimensione FFT alta
    .description = Finestra di analisi per le bande sopra il taglio
spectrum-hold-on-pause = Blocca in pausa
    .description = Congela le barre durante la pausa invece di lasciarle cadere al silenzio
spectrum-labels-frequency = Frequenza
spectrum-labels-pitch = Note
spectrum-low-bound-description = Frequenza più bassa analizzata dalle barre
spectrum-orientation = Orientamento
    .description = Il lato da cui crescono le bande
spectrum-outline-bars = Barre a contorno
    .description = Disegna ogni barra come un contorno vuoto invece di una rampa piena
spectrum-outline-width = Spessore contorno
    .description = Lo spessore del tratto delle barre vuote
spectrum-peak-caps = Segni di picco
    .description = Tiene un segno sul picco recente di ogni banda
spectrum-section-bands = Bande
spectrum-split-at = Taglio a
    .description = Dove si incontrano le zone, agganciato alla barra più vicina
spectrum-split-zones = Zone divise
    .description = Analizza sotto e sopra una frequenza di taglio con finestre di dimensioni diverse
spectrum-style = Stile
    .description = Barre classiche, blocchi stile LED, o una linea piena
spectrum-style-bars = Barre
spectrum-style-blocks = Blocchi
spectrum-style-line = Linea
spectrum-symmetry = Simmetria
    .description = Piega lo spettro attorno al centro; avanti mette i bassi ai lati, inverso li fa incontrare al centro
spectrum-symmetry-forward = Avanti
spectrum-symmetry-reverse = Inverso

## Waveform panel
waveform-bar-gap = Spazio tra le barre
    .description = Spazio tra le barre, zero le fonde in una forma piena
waveform-bar-width = Larghezza barra
    .description = Quanto spessa si disegna ogni barra
waveform-outline = Contorno
    .description = Traccia le barre invece di riempirle; le barre fuse diventano una sola forma
waveform-scrobble-marker = Segno di scrobble
    .description = Una linea sottile dove la traccia conta come scrobblata su Last.fm
waveform-split-channels = Canali separati
    .description = Una riga per canale, il sinistro sopra il destro; le tracce mono restano una riga sola
waveform-unavailable = Forma d'onda non disponibile per questa traccia

## VU panel
vu-ballistics = Balistica
    .description = VU integra il volume lentamente; Picco scatta su e scende piano
vu-ballistics-peak = Picco
vu-cap-gravity = Gravità dei picchi
    .description = Quanto forte cadono i segni di picco quando l'indicatore si abbassa
vu-channels = Canali
    .description = Separa la coppia stereo, o fondi tutto in un indicatore solo
vu-channels-mono = Mono
vu-channels-stereo = Stereo
vu-db-scale = Scala dB
    .description = Disegna linee guida etichettate ai segni dB dietro gli indicatori
vu-gradient-mode = Sfumatura
    .description = Colora gli indicatori per livello: la rampa del tema, i colori della copertina con i colori del brano attivi, o una coppia personalizzata
vu-hold-on-pause = Blocca in pausa
    .description = Congela gli indicatori durante la pausa invece di lasciarli cadere al silenzio
vu-orientation = Orientamento
    .description = Il lato da cui crescono gli indicatori
vu-peak-caps = Segni di picco
    .description = Tiene un segno sul picco recente di ogni indicatore
vu-section-meter = Indicatore
vu-segment-gap = Spazio tra i segmenti
    .description = La giunzione tra le celle di una pila
vu-segment-height = Altezza segmento
    .description = Quanto alta si disegna ogni cella di una pila
vu-style = Stile
    .description = Una colonna piena, o segmenti stile LED
vu-style-continuous = Continuo
vu-style-segments = Segmenti

## Spectrogram panel
spectrogram-ceiling = Tetto
    .description = Livello che corrisponde all'estremità chiara della mappa colori, così tutto ciò che è più forte resta lì
spectrogram-colormap = Mappa colori
    .description = Come il volume si traduce in colore
spectrogram-colormap-cover = Copertina
spectrogram-colormap-grayscale = Scala di grigi
spectrogram-colormap-ice = Ghiaccio
spectrogram-colormap-magma = Magma
spectrogram-colormap-theme = Tema
spectrogram-colormap-viridis = Viridis
spectrogram-direction = Direzione
    .description = Il lato da cui entrano le nuove colonne, che decide anche se l'asse delle frequenze sale lungo il pannello o lo attraversa
spectrogram-fft-size = Dimensione FFT
    .description = Dimensione della finestra di analisi, un compromesso tra la rapidità con cui una colonna segue un transiente e quanto bene separa due note basse
spectrogram-floor = Base
    .description = Livello che corrisponde all'estremità scura della mappa colori, così tutto ciò che è più debole si legge come sfondo
spectrogram-grid = Griglia
    .description = Linee di frequenza sopra l'immagine
spectrogram-high-bound = Limite superiore
    .description = Cima dell'asse delle frequenze, limitata sotto Nyquist per scartare le ottave più alte, quasi silenziose
spectrogram-history = Cronologia
    .description = Quante colonne il pannello conserva prima che la più vecchia esca scorrendo
spectrogram-hold-on-pause = Blocca in pausa
    .description = Tenere ferma l'immagine durante la pausa invece di farci scorrere silenzio
spectrogram-labels = Etichette
    .description = I numeri di frequenza lungo il righello, dove il pannello ha spazio per loro
spectrogram-log-scale = Scala log
    .description = Dare a ogni ottava lo stesso spazio, la lettura musicale, invece della spaziatura uniforme in Hz di uno strumento da laboratorio
spectrogram-low-bound = Limite inferiore
    .description = Fondo dell'asse delle frequenze
spectrogram-section-picture = Immagine
spectrogram-speed = Velocità
    .description = Quanto velocemente scorre l'immagine, in colonne al secondo

## Oscilloscope panel

oscilloscope-channels = Canali
    .description = Fondi in una sola traccia, sovrapponile, o impila un riquadro per ciascuna
oscilloscope-channels-mono = Mono
oscilloscope-channels-overlay = Sovrapposto
oscilloscope-channels-split = Separato
oscilloscope-fill = Riempimento
    .description = Un riempimento morbido tra la traccia e la linea centrale
oscilloscope-gain = Guadagno
    .description = Scala verticale, per portare un brano silenzioso a una traccia leggibile
oscilloscope-gradient-mode = Sfumatura
    .description = Colora la traccia per escursione: la rampa del tema, i colori della copertina con i colori del brano attivi, o una coppia personalizzata
oscilloscope-grid = Griglia
    .description = Disegna il reticolo dietro la traccia
oscilloscope-hold-on-pause = Blocca in pausa
    .description = Tieni fermo il fotogramma in pausa invece di lasciare che la traccia si appiattisca
oscilloscope-line-width = Spessore linea
    .description = Quanto spesso si disegna la traccia
oscilloscope-persistence = Persistenza
    .description = Quanto restano visibili i fotogrammi precedenti dietro la traccia, l'effetto di persistenza fosforescente
oscilloscope-section-trace = Traccia
oscilloscope-trigger = Trigger
    .description = Inizia ogni fotogramma dove il segnale attraversa il livello di trigger, così il materiale periodico resta fermo
oscilloscope-trigger-falling = Discesa
oscilloscope-trigger-level = Livello di trigger
    .description = Il livello a cui si cerca l'attraversamento
oscilloscope-trigger-off = Off
oscilloscope-trigger-rising = Salita
oscilloscope-window = Finestra
    .description = Quanto tempo copre la traccia da un lato all'altro del pannello

## Shader panel
shader-panel-compile-error = Questo shader non compila:
shader-panel-compile-title = Questo shader non compila
shader-panel-enable = Abilita
shader-panel-inspect = Ispeziona
shader-panel-note-empty-body = Scegli un esempio, o indica al pannello un file .wgsl che definisce fs_user(uv).
shader-panel-note-empty-title = Nessuno shader caricato.
shader-panel-note-missing-body = Questo pannello fa riferimento a uno shader che lo spazio di lavoro non ha, quindi non c'è niente da eseguire.
shader-panel-note-missing-title = { $name } non è tra gli shader di questo spazio di lavoro.
shader-panel-note-off-body = La sorgente e i suoi collegamenti sono ancora qui, solo che non sono in esecuzione.
shader-panel-note-off-title = Questo shader è spento.
shader-panel-note-pending-body = È arrivato con un layout o uno spazio di lavoro invece che da questa macchina, quindi resta spento finché non l'hai controllato.
shader-panel-note-pending-title = Questo shader non è ancora stato letto.
shader-pending-origin-file = Dice di venire da { $path }
shader-pending-origin-inline = Nessun file dietro; la sorgente è arrivata con il layout
shader-pending-more-lines = { $count ->
    [one] ... { $count } altra riga
   *[other] ... altre { $count } righe
}
shader-eject-name-taken = { $name } ha già { $count } copie numerate tra gli shader di questo spazio di lavoro
shader-eject-not-in-pool = { $name } non è tra gli shader di questo spazio di lavoro
shader-eject-failed = estrazione: { $error }
shader-panel-pick = Scegli uno shader
shader-panel-run-shader = Esegui shader
    .description = Off tiene al loro posto la sorgente, il segnalibro e i collegamenti e non dipinge niente
shader-panel-section-routes = Route

## Genre grid panel
genre-grid-clear-picked = Pulisci i generi scelti
genre-grid-desaturate = Desatura durante la riproduzione
    .description = Porta ogni tessera in scala di grigi tranne quella del genere in riproduzione; passandoci sopra torna il colore della tessera
genre-grid-dim-while-playing = Attenua durante la riproduzione
    .description = Sfuma ogni tessera tranne quella del genere in riproduzione; passandoci sopra la tessera si riaccende
genre-grid-follow-description = Scorri al genere in riproduzione a ogni cambio di brano
genre-grid-merge-many = Unisci { $count } generi in "{ $target }"
genre-grid-merge-one = Unisci "{ $source }" in "{ $target }"
genre-grid-pick-filters = La scelta filtra la libreria
    .description = Cliccare un genere restringe a quello ogni pannello che segue la ricerca condivisa; off lascia il clic come semplice selezione
genre-grid-play-genres = Riproduci { $count } generi
genre-grid-resume-description = Torna al genere in riproduzione quando smetti di sfogliare
genre-grid-show-names = Mostra i nomi
    .description = Stampa il genere sotto ogni tessera invece che solo al passaggio del mouse
genre-grid-smooth-description = Scivola fino al genere invece di saltarci
genre-grid-tally = { $tracks ->
    [one] { $albums } album, { $tracks } traccia
   *[other] { $albums } album, { $tracks } tracce
}
genre-grid-tile-face = Immagine della tessera
    .description = Cosa mostra una tessera: le copertine degli album del genere, le copertine velate nel colore del genere stesso, o una scheda a tinta unita col nome sopra
genre-grid-unmerge = { $count ->
    [one] Separa { $count } valore
   *[other] Separa { $count } valori
}

## Artist grid panel
artist-grid-clear-picked = Pulisci gli artisti scelti
artist-grid-desaturate = Desatura durante la riproduzione
    .description = Porta ogni tessera in scala di grigi tranne quella dell'artista in riproduzione; passandoci sopra torna il colore della tessera
artist-grid-dim-while-playing = Attenua durante la riproduzione
    .description = Sfuma ogni tessera tranne quella dell'artista in riproduzione; passandoci sopra la tessera si riaccende
artist-grid-follow-description = Scorri all'artista in riproduzione a ogni cambio di brano
artist-grid-group-mode = Una tessera per
    .description = L'artista dell'album accreditato tiene gli ospiti di un disco sotto il nome che l'ha pubblicato; l'artista della traccia dà a ogni featuring una tessera sua
artist-grid-pick-filters = La scelta filtra la libreria
    .description = Cliccare un artista restringe a lui ogni pannello che segue la ricerca condivisa; off lascia il clic come semplice selezione
artist-grid-play-artists = Riproduci { $count } artisti
artist-grid-portraits = Ritratti degli artisti
    .description = Mostra la foto di ogni artista, cercata una volta per nome e tenuta su disco; off mostra la copertina del primo album
artist-grid-resume-description = Torna all'artista in riproduzione quando smetti di sfogliare
artist-grid-section-grouping = Raggruppamento
artist-grid-show-names = Mostra i nomi
    .description = Stampa l'artista sotto ogni tessera invece che solo al passaggio del mouse
artist-grid-smooth-description = Scivola fino all'artista invece di saltarci
artist-grid-tally = { $tracks ->
    [one] { $albums } album, { $tracks } traccia
   *[other] { $albums } album, { $tracks } tracce
}
artist-grid-track-artist = Artista della traccia

## Wall panels
wall-dim-always = Sempre
    .description = Tieni le tessere in secondo piano anche quando non suona niente; solo la tessera sotto il puntatore si vede intera
wall-dim-amount = Intensità
    .description = Quanto sfumano le altre tessere; 100% le nasconde
wall-gap = Spazio
    .description = Spazio tra le tessere
wall-name-alignment = Allineamento nomi
    .description = Allinea le didascalie sotto le loro tessere
wall-rounding = Arrotondamento
    .description = Arrotonda gli angoli di ogni tessera; 100% è un cerchio
wall-section-picking = Scelta
wall-show-counts = Mostra i conteggi
    .description = Il conto di album e tracce sotto ogni nome
wall-tile-size = Dimensione tessere
    .description = Il lato più lungo delle tessere; le colonne dividono la larghezza del pannello in parti uguali

## Metadata panel
metadata-cover-background = Sfondo copertina
    .description = La copertina della traccia dietro i campi
metadata-display = Visualizzazione
    .description = La scheda col titolo in testa, o una tabella piatta di etichette e valori dall'alto
metadata-display-sheet = Scheda
metadata-display-table = Tabella
metadata-edit-save = Salva
metadata-field-bit-depth = Profondità di bit
metadata-field-bitrate = Bitrate
metadata-field-codec = Codec
metadata-field-comment = Commento
metadata-field-disc = Disco
metadata-field-file = File
metadata-field-sample-rate = Frequenza di campionamento
metadata-field-track = Traccia
metadata-fields = Campi
    .description = Quali campi elenca la scheda; un campo che la traccia non ha resta nascosto
metadata-find-online = Cerca metadati online...
metadata-no-library = Nessuna libreria
metadata-row-borders-description = Il filetto sotto ogni riga della tabella
metadata-source = Sorgente
    .description = Segui ciò che è in riproduzione o selezionato, o leggi la libreria nel suo insieme
metadata-stripes-description = Tinge una riga su due della tabella

## History panel
history-column-last-played = Ultimo ascolto
history-descending = Decrescente
    .description = Inverti l'ordinamento
history-empty-never = Ogni traccia è stata ascoltata
history-empty-recent = Ancora nessun ascolto
history-headings = Spezza la lista recente in blocchi per album; Espanse aggiunge la copertina e le statistiche
history-sort-browse = Ordine di navigazione
history-sort-date-added = Data di aggiunta
history-sort-menu = Ordina
    .description = Come sono ordinate le tracce mai ascoltate
history-title = Cronologia
history-view-most = Più ascoltate
history-view-never = Mai ascoltate
history-view-recent = Ascoltate di recente
history-view-recent-short = Recenti
history-view-row = Vista
    .description = Quale taglio del registro degli ascolti mostra il pannello

## Folder tree panel
folder-tree-clear-scope = Pulisci l'ambito cartella
folder-tree-collapse-all = Comprimi tutto
folder-tree-collapse-branch = Comprimi ramo
folder-tree-cover-art = Copertina
    .description = Mostra la copertina al posto dell'icona della riga, su cartelle o brani
folder-tree-cover-folders = Cartelle
folder-tree-cover-songs = Brani
folder-tree-empty = Ancora nessuna cartella nella libreria
folder-tree-expand-branch = Espandi ramo
folder-tree-follow-description = Rivela e scorri alla traccia in riproduzione a ogni cambio
folder-tree-nonmatch-folders = Cartelle senza corrispondenza
    .description = Nascondi le cartelle senza corrispondenza, o tienile attenuate
folder-tree-nonmatch-songs = Brani senza corrispondenza
    .description = Dentro una cartella che corrisponde, attenua i brani sparsi o nascondili
folder-tree-play-folder = Riproduci la cartella
folder-tree-play-songs = { $count ->
    [one] Riproduci
   *[other] Riproduci { $count } brani
}
folder-tree-resume-description = Torna alla traccia in riproduzione quando smetti di sfogliare
folder-tree-scope-to-folder = Limita il filtro alla cartella
folder-tree-smooth-description = Scivola fino alla traccia invece di saltarci
folder-tree-title = Albero

## Art panel
art-always = Tieni le copertine in secondo piano anche quando non suona niente; solo la copertina sotto il puntatore si vede intera
art-convert = Converti...
art-covers-section = Copertine
matcher-section-matches = Corrispondenze
art-desaturate = Porta ogni copertina in scala di grigi tranne quella dell'album in riproduzione; passandoci sopra torna il colore della copertina
art-dim-while-playing = Sfuma ogni copertina tranne quella dell'album in riproduzione; passandoci sopra la copertina si riaccende
art-disc-style = Stile disco
    .description = Rende ogni copertina come un CD o come l'etichetta di un disco in vinile
art-edit-tags = Modifica i tag...
art-fill-panel = Riempi il pannello
    .description = Dimensiona la copertina centrale solo sull'altezza del pannello (la larghezza quando è verticale); le copertine laterali escono dal bordo invece di rimpicciolirla
art-follow-description = Centra l'album in riproduzione a ogni cambio di brano
art-glow = Bagliore
    .description = Raccoglie il colore d'accento dietro la copertina centrale; con la tinta della copertina attiva prende il colore dell'album in riproduzione
art-label-position = Posizione dell'etichetta
    .description = Dove sta la didascalia dell'album: in alto, sotto la copertina, sul bordo inferiore o nascosta
art-letter-rail = Barra delle lettere
    .description = Le iniziali degli artisti lungo il bordo dello scaffale; un clic salta al primo album di quella lettera
art-layout-section = Layout
art-perspective = Prospettiva
    .description = Gira le copertine laterali in vero 3D invece dello schiacciamento piatto
art-reflections = Riflessi
    .description = Specchia ogni copertina nel pavimento sotto lo scaffale
art-resume-description = Ricentra l'album in riproduzione quando smetti di sfogliare
art-shadows = Ombre
    .description = Un'ombra morbida sotto ogni copertina
art-smooth-description = Scivola fino all'album invece di saltarci
art-title = Carosello album
art-vertical-layout = Layout verticale
    .description = Impila lo scaffale come una colonna che scorre su e giù invece di una riga

## Playlists panel
playlists-columns = Quali colonne della traccia si vedono accanto al titolo
playlists-delete = Elimina playlist
playlists-edit-query = Modifica la query...
playlists-empty = Ancora nessuna playlist, aggiungi tracce o usa Nuova playlist
playlists-headings = Spezza le tracce di ogni playlist in blocchi per album; Espanse aggiunge la copertina e le statistiche
playlists-import-tooltip = Importa playlist
playlists-imported-fallback = Importata
playlists-new = Nuova playlist...
playlists-new-smart = Nuova playlist intelligente...
playlists-refuse-drag-out = Le tracce di una playlist intelligente non si possono trascinare fuori
playlists-refuse-edit-query = Modifica la query per cambiare cosa contiene una playlist intelligente
playlists-refuse-smart-source = Una playlist intelligente prende le sue tracce dalla sua query
playlists-remove = { $count ->
    [one] Rimuovi dalla playlist
   *[other] Rimuovi { $count } dalla playlist
}
playlists-rename = Rinomina...
playlists-title = Playlist

## Queue panel
queue-clear = Svuota la coda
queue-empty = La coda è vuota
queue-headings = Spezza la coda in blocchi per album; Espanse aggiunge la copertina e le statistiche
queue-play-now = Riproduci ora
queue-remove = { $count ->
    [one] Rimuovi dalla coda
   *[other] Rimuovi { $count } dalla coda
}
queue-title = Coda
queue-widget-always-modal = Apri sempre come modale
    .description = Apri la coda in una finestra modale ogni volta, invece di saltare a un pannello coda già aperto
queue-widget-clear-queue = Svuota la coda
queue-widget-more = { $count ->
    [one] +{ $count } altra
   *[other] +{ $count } altre
}
queue-widget-open-on-click = Apri la coda al clic
    .description = Clicca il widget per saltare a un pannello coda aperto, o apri la coda in una finestra quando non ce n'è nessuno
queue-widget-section-click = Clic
queue-widget-title = Widget coda
queue-widget-up-next = Prossime

## Biography panel
biography-background = Sfondo
    .description = La fanart dell'artista dietro il testo, attenuata e sfumata verso il basso
biography-fill-width = Riempi la larghezza
    .description = Lascia che un'intestazione alta occupi tutta la larghezza invece di restare limitata e centrata
biography-from-lastfm = Da Last.fm
biography-header-image = Immagine di intestazione
    .description = Il banner largo dell'artista in cima, o il ritratto quando non c'è un banner
biography-keep-aspect = Mantieni le proporzioni
    .description = Mostra l'intestazione nelle sue proporzioni invece di ritagliarla per riempire una fascia
biography-listeners-count = ascoltatori: { $count }
biography-looking-up = Ricerca di { $name }
biography-no-artist-tag = Nessun tag artista
biography-no-text = Nessuna biografia in archivio
biography-not-found = Niente trovato per { $name }
biography-plays-count = ascolti: { $count }
biography-refresh = Aggiorna
biography-similar-artists = Artisti simili
    .description = Artisti correlati secondo i dati di ascolto, in fondo
biography-similar-heading = Artisti simili
biography-stats = Statistiche
    .description = Ascoltatori e ascolti su Last.fm, sotto il nome
biography-tags = Tag
    .description = I tag di genere come riga di chip
biography-title = Biografia

## Status panel
status-count-albums = { $count ->
    [one] 1 album
   *[other] { $count } album
}
status-count-artists = { $count ->
    [one] 1 artista
   *[other] { $count } artisti
}
status-count-plays = { $count ->
    [one] 1 ascolto
   *[other] { $count } ascolti
}
status-count-selected = { $count ->
    [one] { $count } selezionata
   *[other] { $count } selezionate
}
status-count-tracks = { $count ->
    [one] 1 traccia
   *[other] { $count } tracce
}
status-readouts = Indicatori
    .description = Trascina lungo la barra per riordinare; trascina tra le righe, o usa la x e il più di un chip, per nascondere e mostrare
status-scope-selection = Selezione
status-title = Stato

## Output panel
output-detail-badge = Badge
output-detail-compact = Compatto
output-detail-expanded = Espanso
output-detail-label = Dettaglio
    .description = Badge lo tiene a un chip col resto al passaggio del mouse; compatto dà al titolo una riga sua, per una striscia lungo un bordo; espanso aggiunge le ragioni di fianco, o sotto quando il pannello è troppo stretto
output-device-name = Nome del dispositivo
    .description = Nomina il dispositivo in funzione nel titolo; off lascia nella riga solo modalità, frequenza e formato
output-file-rate = Frequenza del file
    .description = Conferma la frequenza del file in riproduzione quando niente lo sta convertendo. Una conversione viene segnalata comunque, visto che è di questo che avvisa
output-mode-exclusive = Esclusiva
output-mode-shared = Condivisa
output-no-output = Nessuna uscita
output-nothing-playing = Niente in riproduzione
output-pick-another-device = Scegli un altro dispositivo, o disattiva l'esclusiva
output-headline-numbers = { $rate } Hz, { $channels } can., { $format }
output-headline = { $mode }, { output-headline-numbers }
output-headline-device = { $mode } su { $device }, { output-headline-numbers }
output-fell-back-to-shared = Esclusiva ripiegata su condivisa: { $why }
output-replaygain-levelling = Il ReplayGain sta livellando questo file di { $db } dB
output-replaygain-short = ReplayGain { $db } dB
output-rate-resampled = Il file in riproduzione è a { $rate } Hz, ricampionato per raggiungere il dispositivo
output-rate-resampled-short = File a { $rate } Hz ricampionato
output-rate-native = Il file in riproduzione è a { $rate } Hz, quindi non viene ricampionato
output-rate-native-short = File a { $rate } Hz, senza ricampionamento
output-start-track-hint = Avvia una traccia per vedere il formato che il dispositivo ha accettato
output-title = Uscita

## Track columns
columns-bits = Bit
columns-bpm = BPM
columns-codec = Codec
columns-cover = Copertina
columns-fav = Pref
columns-gain = Guadagno
columns-kbps = Kbps
columns-khz = kHz
columns-name = Nome
columns-number = Numero
columns-scanned = Scansionato
columns-similar = Simili

## Filter panel
filter-add-column = Aggiungi colonna
filter-add-column-tooltip = Aggiungi una colonna
filter-all = Tutti
filter-clear-filters = Pulisci i filtri
filter-clear-selection = Pulisci la selezione
filter-empty = Scegli un campo per iniziare a filtrare
filter-remove-column = Rimuovi colonna

## Search panel
search-chips-below = Sotto
search-chips-inline = In linea
search-filter-chips = Chip dei filtri
search-placeholder = Cerca nella libreria

## Playback panel
playback-buttons = Pulsanti
    .description = Trascina lungo la barra per riordinare; trascina tra le righe, o usa la x e il più di un chip, per nascondere e mostrare
playback-continue-down-list = Continua a suonare, avanti lungo la lista
playback-continue-off = Continua a suonare disattivato
playback-continue-weighted = Continua a suonare, prima le mai ascoltate
playback-crossfade-inside-albums = Dentro gli album
playback-crossfade-off = Dissolvenza incrociata disattivata
playback-crossfade-tip = Dissolvenza incrociata { $length }
playback-highlight-circle = Cerchio
playback-highlight-square = Quadrato
playback-hold-draw = { $tip }. Tieni premuto per scegliere un'estrazione
playback-hold-length = { $tip }. Tieni premuto per scegliere una durata
playback-hold-order = { $tip }. Tieni premuto per scegliere un ordine
playback-loop-off = Ripetizione disattivata
playback-loop-queue = Ripeti la coda
playback-loop-track = Ripeti questa traccia
playback-menu-continue = Pulsante Continua
playback-menu-crossfade = Pulsante Dissolvenza incrociata
playback-menu-favourite = Pulsante Preferito
playback-menu-random = Pulsante A caso
playback-menu-rating = Stelle di valutazione
playback-menu-stop = Pulsante Stop
playback-menu-stop-after = Pulsante Ferma dopo
playback-menu-volume = Pulsante Volume
playback-pause = Pausa
playback-play-highlight = Evidenziazione Riproduci
    .description = Il riempimento d'accento del pulsante Riproduci: un cerchio, un quadrato morbido, o niente
playback-random-tip-random = Riproduci una traccia a caso
playback-random-tip-similar = Riproduci una traccia simile a questa
playback-seek-back-tip = Indietro di 10 secondi
playback-seek-forward-tip = Avanti di 10 secondi
playback-shuffle-off = Casuale disattivato
playback-shuffle-on = Casuale attivo, ordine { $order }
playback-stop-after-armed = Ferma dopo questa traccia, armato
playback-stop-after-tip = Ferma dopo questa traccia
playback-stop-tip = Ferma e scarica la traccia
playback-volume-tip-muted = Riattiva l'audio, { $percent }%. Clic destro per il cursore
playback-volume-tip-unmuted = Silenzia, { $percent }%. Clic destro per il cursore

## Track info panel
track-info-color-output-chip = Colora il chip dell'uscita
    .description = Lascia che il chip prenda i colori d'avviso quando l'uscita ripiega o ricampiona. Off lo tiene sempre nello stesso tono smorzato, e la nota al passaggio del mouse spiega comunque lo stato
track-info-cycle-every = Ruota ogni
    .description = Quanto resta ogni riga prima della dissolvenza
track-info-cycle-rows = Ruota le righe
    .description = Mostra le righe della disposizione una alla volta in una sola linea, dissolvendo tra loro; una riga da sola resta così com'è
track-info-delay = Ritardo
    .description = Quanto riposa la riga a ogni estremo prima di ripartire
track-info-marquee = Scritta scorrevole
    .description = Cosa fa una riga troppo lunga per il pannello: striscia e torna, o cicla senza fine
track-info-menu-overflow = Testo in eccesso
track-info-next = Prossimo: { $line }
track-info-opening = apertura...
track-info-output-fallback = L'uscita esclusiva è stata rifiutata dal dispositivo, quindi la riproduzione passa dal mixer condiviso. Il dispositivo ha riferito: { $reason }
track-info-output-resample-exclusive = Questo file è a { $source } kHz e la scheda ha accettato { $device } kHz, quindi ogni campione viene convertito in uscita. Il dispositivo non ha voluto girare alla frequenza del file.
track-info-output-resample-mixer = Questo file è a { $source } kHz e il mixer gira a { $device } kHz, quindi ogni campione viene convertito in uscita. La modalità esclusiva passerebbe invece alla scheda la frequenza del file.
track-info-overflow-loop = Cicla
track-info-overflow-scroll = Scorri
track-info-overflow-truncate = Tronca
track-info-queued-count = { $count } in coda
track-info-row-size = Dimensione riga { $number }
track-info-speed = Velocità
    .description = Quanto veloce striscia la riga
track-info-text-size = Dimensione testo

## Seek panel
seek-ending = Fine
    .description = Conta alla rovescia il tempo che resta o mostra la durata intera
seek-ending-remaining = Rimanente
seek-ending-total = Totale
seek-playhead = Testina
    .description = Occupa tutta l'altezza della barra o aderisce alla linea
seek-playhead-full = Intera
seek-playhead-line = Linea
seek-playhead-max-height = Altezza massima testina
    .description = Limita la testina intera, centrata sulla linea; 0 riempie il pannello
seek-playhead-width = Larghezza testina
    .description = La larghezza del segno di posizione che si muove
seek-rounding = Arrotondamento
    .description = Il raggio degli angoli della linea, fino a una pillola a metà dello spessore
seek-scrobble-marker = Segno di scrobble
    .description = Una linea sottile dove la traccia conta come scrobblata su Last.fm
seek-show-timings = Mostra i tempi
seek-thickness = Spessore
    .description = L'altezza della linea della traccia

## Volume panel
volume-pieces = Elementi
    .description = Trascina lungo la barra per riordinare; trascina tra le righe, o usa la x e il più di un chip, per nascondere e mostrare. Con la percentuale nascosta è il suggerimento dell'altoparlante a mostrarla
volume-readout = Lettura
    .description = Mostra il livello come percentuale o come il guadagno in decibel che applica
volume-readout-decibels = Decibel
volume-readout-percent = Percentuale
volume-stretch = Allunga
    .description = Lascia che il cursore riempia il pannello invece di limitarne la larghezza
volume-tip-mute = Silenzia
volume-tip-mute-level = Silenzia, { $level }
volume-tip-unmute = Riattiva l'audio
volume-tip-unmute-level = Riattiva l'audio, { $level }

## Shared panel content
content-filter = Filtro
content-no-track = Nessuna traccia
content-total-genres = Generi
content-total-time = Durata totale

## Shared panel chrome
panel-columns-description = Quali colonne della traccia si vedono
panel-headings = Intestazioni
panel-jump-to-playing = Vai a ciò che è in riproduzione
panel-menu-display = Visualizzazione
panel-title-artists = Artisti
panel-title-genres = Generi
panel-title-oscilloscope = Oscilloscopio
panel-title-particles = Particelle
panel-title-playback = Riproduzione
panel-title-seek = Avanzamento
panel-title-shader = Shader
panel-title-spectrogram = Spettrogramma
panel-title-spectrum = Spettro
panel-title-theme-toggle = Interruttore tema
panel-title-track-info = Info traccia
panel-title-volume = Volume
panel-title-vu = VU meter
panel-title-waveform = Forma d'onda

## Everything else
choice-both = Entrambi
choice-dim = Attenuare
choice-hide = Nascondere
composite-add-panel = Aggiungi pannello
composite-host-settings = Impostazioni { $host }
composite-move-left = Sposta a sinistra
composite-move-right = Sposta a destra
composite-remove = Rimuovi
composite-replace = Sostituisci
group-panel-add-slot = Aggiungi slot
group-panel-move-down = Sposta giù
group-panel-move-up = Sposta su
group-panel-remove-slot = Rimuovi slot
group-panel-split-side-by-side = Dividi affiancati
group-panel-split-stacked = Dividi impilati
group-panel-swap-panels = Scambia i pannelli
group-panel-title = Gruppo
overlay-dim = Attenua
    .description = Quanto si attenua il pannello principale sotto l'overlay rivelato
overlay-title = Overlay
overlay-toggle = Attiva o disattiva l'overlay
shader-confirm-hint-after = attiva o disattiva lo shader da ovunque.
shader-confirm-hint-before = Uno shader può rendere le finestre difficili da usare. Ripristina o chiudi questa finestra per tornare a com'era.
shader-confirm-keep = Tieni
shader-confirm-question = Tenere questo shader di schermo?
shader-confirm-revert = Ripristina
shader-confirm-window-title = rox - Shader di overlay
slide-add = Aggiungi diapositiva
slide-next = Diapositiva successiva
slide-previous = Diapositiva precedente
slide-title = Diapositiva
theme-toggle-to-dark = Passa al tema scuro
theme-toggle-to-light = Passa al tema chiaro
transport-favourite-add = Aggiungi ai preferiti
transport-favourite-nothing = Niente da aggiungere ai preferiti
transport-favourite-remove = Rimuovi dai preferiti
transport-pieces = Elementi
    .description = Trascina lungo una riga per riordinare e tra le righe per spostare; la x e il più di un chip nascondono e mostrano

## Stragglers picked up in the final sweep

duplicates-scanning = Scansione in corso...
about-copyright = Copyright © 2026
signal-name-placeholder = Nome del segnale
signals-empty = Ancora nessun segnale. Aggiungine uno, oppure fai clic destro su una manopola collegabile.
signal-add = Aggiungi segnale
panel-approve = Approva
panel-turn-off = Disattiva
shader-from-file = Da file...
arrange-add-row = Aggiungi riga
smart-playlist-name-placeholder = Nome della playlist
smart-playlist-name-to-save = Dai un nome alla playlist per salvarla
panel-new-playlist = Nuova playlist...
panel-edit-tags = Modifica i tag...
panel-edit-cover = Modifica la copertina...
panel-rename-files = Rinomina i file...
panel-convert = Converti...
panel-catalog-drag-anchor = Ancora di trascinamento
panel-catalog-spacer = Spaziatore

## Duration and worker phrasing

pace-under-a-minute = meno di un minuto
pace-minutes = { $count ->
    [one] circa un minuto
   *[other] circa { $count } minuti
}
pace-hours = { $count ->
    [one] circa un'ora
   *[other] circa { $count } ore
}
pace-half-hours = circa { $value } ore
pace-days = { $count ->
    [one] circa un giorno
   *[other] circa { $count } giorni
}
pace-workers = { $count ->
    [one] { $count } processo
   *[other] { $count } processi
}
tasks-rest-takes = , il resto richiede { $estimate }
tasks-measuring-takes = , misurarli richiede { $estimate }
tasks-working-out-takes = , calcolarli richiede { $estimate }
tasks-time-left = , ancora { $left }
tasks-failed-suffix = { $count ->
    [one] ({ $count } fallito)
   *[other] ({ $count } falliti)
}
tasks-file-suffix = - { $file }
tasks-no-beat-suffix = ({ $count } senza battito chiaro)
tasks-estimate-at-workers = ({ tasks-estimate-at })

## Panel vanity names

panel-title-art-view = Vista copertina
panel-title-artist-grid = Griglia artisti
panel-title-genre-grid = Griglia generi
panel-title-biography = Biografia
panel-title-cover-art = Copertina
panel-title-drag-anchor = Ancora di trascinamento
panel-title-drawer = Cassetto
panel-title-eq-widget = Widget EQ
panel-title-filter = Filtro
panel-title-folder-tree = Albero delle cartelle
panel-title-group = Gruppo
panel-title-history = Cronologia
panel-title-lyrics = Testo
panel-title-menu = Menu
panel-title-metadata = Metadati
panel-title-mini-toggle = Interruttore mini
panel-title-output = Uscita
panel-title-overlay = Overlay
panel-title-playlists = Playlist
panel-title-queue = Coda
panel-title-queue-widget = Widget coda
panel-title-search = Ricerca
panel-title-slide = Diapositiva
panel-title-spacer = Spaziatore
panel-title-stats-widget = Widget statistiche
panel-title-vu-meter = VU meter
panel-title-window-controls = Controlli finestra

## Relative time and the output headline

ago-just-now = proprio ora
ago-minutes = { $count } min fa
ago-hours = { $count } h fa
ago-days = { $count } g fa
ago-weeks = { $count } sett. fa
ago-years = { $count } anni fa

span-seconds = { $count ->
    [one] { $count } secondo
   *[other] { $count } secondi
}
span-minutes = { $count ->
    [one] { $count } minuto
   *[other] { $count } minuti
}
span-hours = { $count ->
    [one] { $count } ora
   *[other] { $count } ore
}
span-days = { $count ->
    [one] { $count } giorno
   *[other] { $count } giorni
}
span-weeks = { $count ->
    [one] { $count } settimana
   *[other] { $count } settimane
}
span-years = { $count ->
    [one] { $count } anno
   *[other] { $count } anni
}
span-pair = { $first }, { $second }
unit-percent = { $value }%

settings-audio-output-headline = { $mode }{ $note } su { $device }, { $rate } Hz, { $channels } can., { $format }
settings-audio-output-experimental =  (sperimentale)

## ML model catalog

settings-mlmodels-description = { $summary }. { $dim } valori per traccia. { $licence }
settings-mlmodels-on-disk = , { $size } su disco
settings-mlmodels-to-download = , { $size } da scaricare
model-summary-dsp-timbre-1 = Integrato, nessun download. Un riassunto dell'energia per banda, della forma spettrale e del tasso di attacchi di ogni traccia. Grossolano accanto a una rete addestrata, ma non richiede nulla e gira ovunque
model-summary-panns-cnn10 = Una rete convoluzionale addestrata su AudioSet per riconoscere che cos'è un suono. La sua descrizione di una traccia in 512 valori è molto più ricca dello schizzo integrato, al costo di un download da 24 MB e di un'analisi più lenta

## Shipped workspaces

workspace-shipped-default = (Predefinito)
workspace-shipped-default-blurb = Com'è rox appena installato: superfici traslucide sopra il desktop, nessuna cornice di finestra, tinta dalla copertina disattivata. Il punto di partenza da cui ogni altro look qui si allontana.
workspace-shipped-catrox-blurb = La skin di foobar2000 che ha dato il via a tutto, ricostruita: una resa circolare della copertina come CD, i campi dei metadati lungo la sinistra, e tracce raggruppate per album con i pallini di valutazione.
workspace-shipped-critters-blurb = Tutta l'app come una stampa a 1 bit: un dithering ordinato su ogni superficie, toni che si schiacciano con i bassi profondi, e un muro di rumore che si contorce con il brano. Ispirato a Critters for Sale.
workspace-shipped-diffuse-blurb = Solo l'album in riproduzione: la copertina e la scheda di riproduzione come un unico gruppo che riempie la finestra, superfici trasparenti sullo sfondo, senza giunture. Libreria, coda e testo aspettano in un cassetto sul bordo destro e scivolano sopra la musica quando si passa sulla maniglia. Monocromatico, così il colore arriva dalle copertine.
workspace-shipped-foobar-blurb = Il layout con cui questo intero progetto discute. Pannelli opachi, colonne di filtro per artista e album, una tabella di tracce densa, e la barra dei menu esattamente dov'è sempre stata.
workspace-shipped-llama-winamp-blurb = Winamp come te lo ricordi piuttosto che com'era davvero. Tahoma, scuro, senza cornice, uno spettro punteggiato in cima, e una modalità ridotta sul layout mini.
workspace-shipped-metro-blurb = Pannelli piatti e righe comode in Segoe UI, con la tinta dalla copertina attiva così l'intera palette segue la copertina in riproduzione.
workspace-shipped-phosphor-blurb = Tutto a spaziatura fissa. Consolas, verde su nero, nessuna copertina nella riproduzione rapida: un terminale che per caso suona musica.
