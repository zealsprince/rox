### Italiano. Rispecchia en-CA/rox.ftl chiave per chiave; il test di
### parità in rox-i18n lo garantisce.

## Shared widgets

tracking-title = Tracciamento
tracking-follow = Segui la riproduzione
tracking-resume = Riprendi quando inattivo
tracking-smooth = Scorrimento fluido
align-row = Allineamento
    .description = Dove sta il contenuto quando il pannello ha spazio in più
valign-row = Allineamento verticale
    .description = Dove sta il contenuto quando il pannello ha altezza in più
valign-top = Alto
valign-middle = Centro
valign-bottom = Basso

## Panel source and search rows

source-track = Traccia
    .description = Segui ciò che sta suonando, o ciò che è selezionato nella libreria
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
signal-response-drift = 0 segue la musica di scatto, 100 le va dietro
signal-threshold = Soglia
signal-threshold-trigger = Il livello che l'intervallo deve raggiungere per lanciare l'impulso; non riparte finché il livello non ricade sotto il segno sul misuratore sopra
signal-threshold-gate = Sotto questo il segnale si legge come niente, sopra l'uscita risale da zero, così i passaggi silenziosi lasciano stare la manopola; il segno sul misuratore sopra è dove si trova
signal-low-bound = Limite inferiore
signal-high-bound = Limite superiore
signal-adds-up = Somma
    .description = Quale segnale viene totalizzato; sale finché quello legge alto e si ferma finché è silenzioso
signal-aggregate-nothing = Niente da seguire
signal-aggregate-pick = Scegli un segnale
signal-aggregate-alone = Non c'è nessun altro segnale nel pool da sommare, quindi resta a zero. Aggiungine uno e comparirà nella lista.
signal-aggregate-unpicked = Niente scelto, quindi questo totale resta a zero. Scegli un segnale sopra.
signal-rate = Frequenza
    .description = Giri al secondo a pieno ingresso; dopo 1 torna a 0 e continua a salire, che uno shader legge come una fase
signal-reset-on-track = Azzera al cambio traccia
    .description = Torna a zero quando parte un nuovo brano, così una fase non si porta dietro il totale del precedente
signal-flush = Svuota
    .description = Rimandalo a zero adesso; cala in un attimo invece di scattare, così niente di ciò che lo segue sobbalza
route-header = Route
route-signal = Segnale
    .description = Quale segnale condiviso segue questa route; regolarlo qui regola ogni route che lo segue
route-new-signal = Nuovo segnale
route-shared-note = Condiviso da ogni route su questo segnale
route-signal-gone = Il segnale di questa route non c'è più; la manopola tiene il suo valore finché non se ne sceglie un altro sopra.
route-range-note = Intervallo solo per questo parametro
route-quiet = Silenzio
    .description = Cosa raggiunge la manopola nel silenzio, come quota della sua impostazione
route-loud = Pieno
    .description = Cosa raggiunge a pieno segnale; 100% è il valore del cursore stesso, sotto Silenzio modula verso il basso
route-slot = Slot
    .description = Quale dei sedici slot di segnale dello shader riempie questa route
route-slot-quiet-description = Cosa legge lo slot nel silenzio
route-slot-loud-description = Cosa legge a pieno segnale; sotto Silenzio lo slot va all'indietro
route-slot-signal-description = Quale segnale condiviso segue questa route
route-slot-signal-gone = Il segnale di questa route non c'è più; lo slot legge zero finché non se ne sceglie un altro.
route-add = Aggiungi route
route-unrouted = Senza route
route-pick-slot = Scegli uno slot
route-pick-signal = Scegli un segnale
route-no-signal = nessun segnale
route-no-signals-yet = Non ci sono ancora segnali da seguire. Creane uno e comparirà qui; fino ad allora lo slot legge zero.
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
    .description = Fissa il pannello al suo posto; il dock non lo lascia trascinare né riordinare
panel-drag-anchor = Ancora di trascinamento
    .description = Un trascinamento ovunque sul pannello sposta la finestra, mentre i clic semplici arrivano comunque ai suoi controlli; per layout senza decorazioni
panel-slot-controls = Controlli degli slot
    .description = Mostra i pulsanti d'angolo per scambiare e rimuovere i pannelli ospitati qui. Nascosti, il layout si modifica comunque dall'albero nella pagina Spazio di lavoro nelle impostazioni
panel-min-width = Larghezza minima
    .description = Dove un ridimensionamento smette di stringere il pannello. Preso così com'è, anche sotto il limite proprio del pannello, così una striscia compatta può andare più stretta del previsto; vuoto lascia stare il limite
panel-max-width = Larghezza massima
    .description = Limita la larghezza del pannello perché non si allunghi quando la finestra si allarga
panel-min-height = Altezza minima
    .description = Dove un ridimensionamento smette di schiacciare il pannello. Preso così com'è, anche sotto il limite proprio del pannello, così una striscia compatta può andare più stretta del previsto; vuoto lascia stare il limite
panel-max-height = Altezza massima
    .description = Limita l'altezza del pannello perché non si allunghi quando la finestra si alza
panel-own-opacity = Opacità di superficie propria
    .description = Dare a questo pannello un'opacità propria sullo sfondo invece di quella dell'app
panel-surface-opacity = Opacità di superficie
panel-margin = Margine
    .description = Tirare il pannello dentro dalla sua cella, con lo sfondo che traspare nello spazio
panel-padding = Spaziatura interna
    .description = Spazio dentro il bordo del pannello, tenuto nel suo stesso sfondo
panel-rounding = Arrotondamento
    .description = Arrotondare gli angoli del pannello verso lo sfondo
panel-border = Bordo
    .description = Una linea attorno al bordo del pannello, nel colore del ruolo Bordo; un lato a zero non ne disegna
panel-font = Carattere
    .description = Il carattere del pannello; il predefinito segue quello dell'app
panel-font-size = Dimensione carattere
    .description = La dimensione del testo del pannello rispetto al carattere dell'app; le righe si scalano con essa
panel-surface-shader = Shader di superficie
    .description = Fa girare uno shader WGSL sul corpo di questo pannello, sotto lo shader di schermo dell'app
panel-run-when-idle = Continua da fermo
    .description = Continua a disegnare fotogrammi mentre l'audio è muto. Off, lo shader resta dov'è e il pannello non costa nulla
panel-shader-is-scene = Questo shader è una scena, quindi copre il corpo del pannello invece di disegnarci sopra. Viene da un bundle o da una configurazione più vecchia; la lista qui sopra offre solo shader che lasciano il pannello leggibile.

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
preset-back-tail = in qualsiasi menu del pannello. Le preimpostazioni valgono solo per questo spazio di lavoro, un altro non le porta con sé.

## Keyboard hints

hint-press = Premi
hint-key-enter = Invio

## Settings: language

settings-language = Lingua
    .description = La lingua dell'interfaccia; Sistema negozia con l'elenco del sistema operativo e ricade sull'inglese se nulla corrisponde
settings-language-system = (Lingua di sistema)
settings-language-search = Cerca una lingua
picker-no-matches = Nessun risultato

## Embed dialog

bake-window-title = rox - Incorpora i metadati salvati
bake-title = Incorpora i metadati salvati
bake-intro = Scrive ciò che rox ha già nei file stessi, così anche un altro lettore lo legge. Nulla viene ricalcolato.
bake-formats = Solo MP3 e FLAC; gli altri formati e le tracce CUE vengono saltati
bake-source-lyrics = Testi
bake-source-gain = ReplayGain
bake-source-acoustic = Descrizioni acustiche
bake-detail-nothing = niente di salvato da incorporare
bake-detail-only-skipped = niente da scrivere, { $skipped } saltati
bake-detail-writes = { $count ->
    [one] { $count } file da scrivere
   *[other] { $count } file da scrivere
}
bake-detail-writes-skipped = { $count ->
    [one] { $count } file da scrivere, { $skipped } saltati
   *[other] { $count } file da scrivere, { $skipped } saltati
}
bake-error-read = Impossibile leggere la libreria: { $error }
bake-survey-counting = Scansione della libreria...
bake-survey-progress = Lettura dei tag, { $done } di { $total }
bake-nothing-to-embed = Niente da incorporare: i file contengono già tutto ciò che rox ha
bake-rewrites = { $count ->
    [one] { $count } file verrà riscritto
   *[other] { $count } file verranno riscritti
}
bake-hint-before = Premi
bake-hint-key = Invio
bake-hint-after = per incorporare
bake-embed = Incorpora
bake-cancel = Annulla

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
shader-pick-missing = { $name } (mancante)
shader-pick-custom = Personalizzato

## Shipped shader examples

shader-blurb-plasma = Colore alla deriva ricavato dai soli uniform, quindi costa un semplice quad.
shader-blurb-trails = Sbava il proprio fotogramma precedente, il che lo mette sul passaggio di schermo.
shader-blurb-sheen = Una vignettatura e un bagliore che si sposta, overlay trasparente per un pannello che già disegna.
shader-blurb-shadow = Un'ombra portata che testo e controlli del pannello proiettano, letta dalla cattura della maschera.
shader-blurb-cover = La copertina del brano in riproduzione, in letterbox su una velatura del suo stesso colore.
shader-blurb-badge = La copertina come piccola scheda parcheggiata in un angolo, con uno slot per spostarla.
shader-blurb-lamp = Una luce che segue il cursore e risponde ai pulsanti, overlay trasparente.
shader-blurb-cube = Un cubo a fil di ferro che ruzzola in finto 3D, disegnato come luce additiva.
shader-blurb-bloom = Sfere alla deriva sfumate da un secondo passaggio a metà dimensione, la catena in miniatura.
shader-blurb-tube = Ripropone il pannello sottostante attraverso uno schermo CRT curvo, scanline comprese.

## Transport strip pieces

seek-item-elapsed = Trascorso
seek-item-strip = Barra
seek-item-ending = Rimanente
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
playback-item-stop = Stop
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
dock-tiles = Riquadri
dock-zoom-in = Ingrandisci
dock-zoom-out = Riduci
dock-collapse = Comprimi
dock-expand = Espandi

## Shader picker notes

shader-note-empty = Scegli un esempio per iniziare, o indica a rox un file .wgsl con uno stadio fragment che definisce fs_user(uv)
shader-note-missing = { $name } non è più tra gli shader di questo spazio di lavoro, quindi non disegna niente. Scegli qualcos'altro qui e questo pannello avrà una sorgente propria.
shader-note-shared = Condiviso in questo spazio di lavoro. Modificarlo aggiorna ogni superficie che lo usa.
shader-note-file = { $path }. I tuoi salvataggi si ricaricano mentre lo shader disegna, e la sorgente viaggia dentro layout e bundle, quindi sopravvive a una macchina che non ha mai avuto il file.
shader-note-custom = Questa sorgente viaggia dentro il suo layout o bundle senza un file dietro. Modifica come file la riscrive e riprende i tuoi salvataggi.

## Panel pages and shared sides

panel-page-layout = Layout
panel-page-view = Vista
panel-page-content = Contenuto
panel-page-source = Sorgente
panel-page-bindings = Collegamenti
panel-page-emitters = Emettitori
panel-page-forces = Forze
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
    .description = Interruzioni di gruppo sulla lista; un ordinamento tiene insieme le sequenze, la ricerca mostra tutto piatto
library-group-by = Raggruppa per
    .description = Su cosa si interrompono le intestazioni; genere e anno riordinano la lista
library-header-row = Riga di intestazione
    .description = Cosa impacchettano le intestazioni a una riga, da sinistra a destra; uno spaziatore o un divisore separa i lati
library-header-lines = Righe di intestazione
    .description = Le righe del blocco, dall'alto in basso; una riga vuota sparisce
library-follow-description = Scorri alla traccia in riproduzione a ogni cambio di brano
library-resume-description = Torna alla traccia in riproduzione quando smetti di sfogliare
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
    .description = Il testo delle righe di intestazione, indipendente dall'altezza riga, così la copertina cresce da sola
library-flush-background = Sfondo a filo
    .description = Poggiare le intestazioni sullo sfondo della lista invece che sulla tinta rialzata; i colori del brano le muovono insieme
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
    .description = Tinge una riga di traccia su due perché una lista lunga si legga
library-row-borders = Bordi di riga
    .description = Il filetto sotto ogni riga di traccia
library-art-description = La tessera delle intestazioni espanse: la copertina, il ritratto dell'artista, o l'immagine del genere
library-art-rounding = Arrotondamento copertina
    .description = Arrotondare gli angoli della copertina
library-art-position = Posizione copertina
    .description = Su quale lato del blocco sta la tessera delle intestazioni espanse
library-art-margin = Margine copertina
    .description = Rientrare la tessera nel blocco; si rimpicciolisce per restare quadrata
library-circular-portraits = Ritratti circolari
    .description = Raggruppato per artista, arrotonda le tessere al cerchio pieno della parete invece che alla manopola di arrotondamento
library-genre-face = Immagine del genere
    .description = Raggruppato per genere, cosa indossa la tessera: le copertine, le copertine lavate nel colore del genere, o una scheda a tinta unita sotto la sua geometria

## Album grid panel

panel-title-album-grid = Griglia album
grid-menu-scroll = Scorrimento
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
    .description = Tieni le copertine in secondo piano anche quando non suona nulla; solo la tessera sotto il puntatore si mostra piena
grid-show-titles = Mostra i titoli
    .description = Stampa album e artista sotto ogni copertina, stile iTunes, invece che solo al passaggio del puntatore
grid-title-alignment = Allineamento titoli
    .description = Allinea le didascalie sotto le loro copertine
grid-tile-size = Dimensione tessere
    .description = Il lato più lungo delle tessere di copertina; le colonne si dividono la larghezza del pannello in parti uguali
grid-gap = Spazio
    .description = Spazio tra le copertine; zero le impacchetta bordo a bordo
grid-art-rounding-description = Arrotonda gli angoli di ogni copertina; 100% è un cerchio
