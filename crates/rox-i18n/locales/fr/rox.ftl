### Français. Reflète en-CA/rox.ftl clé pour clé; le test de parité
### dans rox-i18n y veille.

## Shared widgets

tracking-title = Suivi
tracking-follow = Suivre la lecture
tracking-resume = Reprendre après inactivité
tracking-smooth = Défilement fluide
align-row = Alignement
    .description = Où se place le contenu quand le panneau a de la place en trop
valign-row = Alignement vertical
    .description = Où se place le contenu quand le panneau a de la hauteur en trop
valign-top = Haut
valign-middle = Milieu
valign-bottom = Bas

## Panel source and search rows

source-track = Piste
    .description = Suivre ce qui joue, ou ce qui est sélectionné dans la bibliothèque
source-follow-playing = Suivre la lecture
source-follow-selection = Suivre la sélection
source-playing = Lecture
source-selected = Sélection
query-search = Recherche
query-search-box = Champ de recherche
    .description = Afficher le champ de recherche; la requête ne s'applique que tant qu'il est affiché
query-source = Source de recherche
    .description = Suivre la requête partagée, filtrer avec le champ propre à ce panneau, ou montrer ce qu'un autre panneau a sélectionné
query-source-shared = Partagée
query-source-own = Propre
query-source-selection = Sélection

## Signals and routes

signal-source = Source
    .description = Ce que le signal écoute: Bande suit une plage de fréquences, Niveau tout le mixage, Attaque pulse à chaque frappe dans la plage, Déclencheur lance une impulsion quand la plage atteint son seuil, Total cumule un autre signal dans le temps
signal-kind-band = Bande
signal-kind-level = Niveau
signal-kind-onset = Attaque
signal-kind-trigger = Déclencheur
signal-kind-total = Total
signal-response = Réponse
signal-response-pulse = Combien de temps chaque impulsion résonne avant de s'éteindre
signal-response-drift = 0 colle à la musique, 100 dérive derrière elle
signal-threshold = Seuil
signal-threshold-trigger = Le niveau que la plage doit atteindre pour lancer l'impulsion; elle ne repart pas tant que le niveau n'est pas redescendu sous la marque du vumètre au-dessus
signal-threshold-gate = En dessous, le signal se lit comme rien, au-dessus la sortie remonte depuis zéro, si bien que les passages calmes laissent le bouton tranquille; la marque du vumètre au-dessus indique où il se trouve
signal-low-bound = Limite basse
signal-high-bound = Limite haute
signal-adds-up = Cumule
    .description = Quel signal est totalisé ici; il monte tant que celui-là lit haut et cale tant qu'il est calme
signal-aggregate-nothing = Rien à suivre
signal-aggregate-pick = Choisir un signal
signal-aggregate-alone = Il n'y a aucun autre signal dans le pool à cumuler, donc il reste à zéro. Ajoutes-en un et il apparaîtra dans la liste.
signal-aggregate-unpicked = Rien de choisi, donc ce total reste à zéro. Choisis un signal ci-dessus.
signal-rate = Cadence
    .description = Tours par seconde à plein régime; après 1 il repasse à 0 et continue de monter, ce qu'un shader lit comme une phase
signal-reset-on-track = Réinitialiser au changement de piste
    .description = Redescendre à zéro quand un nouveau morceau commence, pour qu'une phase n'emporte pas le total du précédent
signal-flush = Vider
    .description = Le renvoyer à zéro maintenant; il redescend progressivement plutôt que d'un coup, pour que rien de ce qui le suit ne saute
route-header = Route
route-signal = Signal
    .description = Quel signal partagé cette route suit; le régler ici règle toutes les routes qui le suivent
route-new-signal = Nouveau signal
route-shared-note = Partagé par toutes les routes sur ce signal
route-signal-gone = Le signal de cette route a disparu; le bouton garde sa valeur jusqu'à ce qu'un autre soit choisi ci-dessus.
route-range-note = Plage pour ce paramètre uniquement
route-quiet = Silence
    .description = Ce que le bouton atteint dans le silence, en part de son propre réglage
route-loud = Plein
    .description = Ce qu'il atteint à plein signal; 100% est la valeur du curseur lui-même, sous Silence module vers le bas
route-slot = Emplacement
    .description = Lequel des seize emplacements de signal du shader cette route remplit
route-slot-quiet-description = Ce que l'emplacement lit dans le silence
route-slot-loud-description = Ce qu'il lit à plein signal; sous Silence l'emplacement tourne à l'envers
route-slot-signal-description = Quel signal partagé cette route suit
route-slot-signal-gone = Le signal de cette route a disparu; l'emplacement lit zéro jusqu'à ce qu'un autre soit choisi.
route-add = Ajouter une route
route-unrouted = Sans route
route-pick-slot = Choisir un emplacement
route-pick-signal = Choisir un signal
route-no-signal = aucun signal
route-no-signals-yet = Il n'y a encore aucun signal à suivre. Crées-en un et il apparaîtra ici; d'ici là l'emplacement lit zéro.
route-open-signals = Ouvrir les signaux
route-create-signal = Créer un nouveau signal

## Panel settings window

panel-settings = Réglages du panneau
panel-menu-label = Panneau
panel-save-as-preset = Enregistrer comme préréglage
panel-rename = Renommer
panel-rename-name = Nom
panel-rename-note = Affiché comme onglet du panneau; vide revient au nom d'origine
panel-rename-hint-after = pour renommer
panel-was-closed = Le panneau a été fermé
panel-reset = Réinitialiser
panel-inverse = Inverser
panel-apply-song-theme = Appliquer les couleurs du morceau
panel-page-appearance = Apparence
panel-page-behavior = Comportement
panel-page-shader = Shader
panel-section-placement = Placement
panel-section-size = Taille
panel-section-opacity = Opacité
panel-section-frame = Cadre
panel-section-colors = Couleurs
panel-section-font = Police
panel-section-shader = Shader
panel-section-signals = Signaux
panel-section-slots = Emplacements
panel-awaiting-approval = En attente d'approbation
panel-size-off = Désactivé
panel-locked = Verrouillé
    .description = Fixer le panneau en place; le dock ne le laisse ni déplacer ni réorganiser
panel-drag-anchor = Poignée de déplacement
    .description = Un glissement n'importe où sur le panneau déplace la fenêtre, tandis que les clics simples atteignent toujours ses contrôles; pour les dispositions sans décorations
panel-slot-controls = Contrôles d'emplacement
    .description = Afficher les boutons de coin pour échanger et retirer les panneaux hébergés ici. Masqués, la disposition se modifie toujours depuis l'arbre de la page Espace de travail dans les réglages
panel-min-width = Largeur minimale
    .description = Où un redimensionnement cesse de resserrer le panneau. Pris tel quel, y compris sous le plancher propre du panneau, si bien qu'une bande compacte peut aller plus étroit que d'origine; vide laisse le plancher tranquille
panel-max-width = Largeur maximale
    .description = Plafonner la largeur du panneau pour qu'il ne s'étire pas quand la fenêtre s'élargit
panel-min-height = Hauteur minimale
    .description = Où un redimensionnement cesse de tasser le panneau. Pris tel quel, y compris sous le plancher propre du panneau, si bien qu'une bande compacte peut aller plus étroit que d'origine; vide laisse le plancher tranquille
panel-max-height = Hauteur maximale
    .description = Plafonner la hauteur du panneau pour qu'il ne s'étire pas quand la fenêtre grandit
panel-own-opacity = Opacité de surface propre
    .description = Donner à ce panneau sa propre opacité sur le fond plutôt que celle de l'application
panel-surface-opacity = Opacité de surface
panel-margin = Marge
    .description = Rentrer le panneau depuis sa cellule, le fond transparaissant dans l'écart
panel-padding = Remplissage
    .description = Espace à l'intérieur du bord du panneau, gardé dans son propre fond
panel-rounding = Arrondi
    .description = Arrondir les coins du panneau vers le fond
panel-border = Bordure
    .description = Une ligne autour du bord du panneau, dans la couleur du rôle Bordure; un côté à zéro n'en dessine aucune
panel-font = Police
    .description = La typographie du panneau; par défaut elle suit la police de l'application
panel-font-size = Taille de police
    .description = La taille du texte du panneau relative à la police de l'application; les lignes se mettent à l'échelle avec
panel-surface-shader = Shader de surface
    .description = Faire tourner un shader WGSL sur le corps de ce panneau, sous le shader d'écran de l'application
panel-run-when-idle = Continuer au repos
    .description = Continuer à dessiner des images tant que l'audio est silencieux. Désactivé, le shader se gare où il est et le panneau ne coûte rien
panel-shader-is-scene = Ce shader est une scène, il couvre donc le corps du panneau au lieu de dessiner par-dessus. Il vient d'un lot ou d'une config plus ancienne; la liste ci-dessus n'offre que des shaders qui laissent le panneau lisible.

## Shader picker and saving

shader-source = Source
shader-pick-none = Aucun
shader-reload = Recharger
shader-edit-as-file = Modifier comme fichier
shader-make-private-copy = Faire une copie privée
shader-save-replace = Remplacer
shader-save-to-workspace = Enregistrer dans l'espace de travail
shader-save-replaces = Remplace le shader que cet espace de travail appelle déjà { $name }. Tout panneau utilisant ce nom change avec lui
shader-save-adds = L'ajoute aux shaders de cet espace de travail sous { $name }. N'importe quel panneau peut l'utiliser, et le modifier les met tous à jour
shader-group-examples = Exemples
shader-group-this-workspace = Cet espace de travail
shader-group-scenes = Scènes
shader-group-workspace-scenes = Scènes de l'espace de travail
shader-group-overlays = Surcouches
shader-group-workspace-overlays = Surcouches de l'espace de travail

## Saving a panel preset

preset-save = Enregistrer le préréglage
preset-save-name = Nom du préréglage
preset-save-replaces = Remplace le préréglage que cet espace de travail appelle déjà { $name }
preset-save-hint-after = pour enregistrer
preset-back-from = Récupère-le depuis
preset-back-add-panel = Ajouter un panneau
preset-back-then = puis
preset-back-presets = Préréglages
preset-back-tail = dans n'importe quel menu de panneau. Les préréglages ne valent que pour cet espace de travail, un autre ne les emporte donc pas.

## Keyboard hints

hint-press = Appuie sur
hint-key-enter = Entrée

## Settings: language

settings-language = Langue
    .description = La langue de l'interface; Système négocie avec la liste du système d'exploitation et retombe sur l'anglais si rien ne correspond
settings-language-system = (Langue du système)
settings-language-search = Rechercher une langue
picker-no-matches = Aucun résultat

## Embed dialog

bake-window-title = rox - Intégrer les métadonnées stockées
bake-title = Intégrer les métadonnées stockées
bake-intro = Écrit ce que rox détient déjà dans les fichiers eux-mêmes, pour qu'un autre lecteur le lise aussi. Rien n'est recalculé.
bake-formats = MP3 et FLAC uniquement; les autres formats et les pistes CUE sont ignorés
bake-source-lyrics = Paroles
bake-source-gain = ReplayGain
bake-source-acoustic = Descriptions acoustiques
bake-detail-nothing = rien de stocké à intégrer
bake-detail-only-skipped = rien à écrire, { $skipped } ignorés
bake-detail-writes = { $count ->
    [one] { $count } fichier à écrire
   *[other] { $count } fichiers à écrire
}
bake-detail-writes-skipped = { $count ->
    [one] { $count } fichier à écrire, { $skipped } ignorés
   *[other] { $count } fichiers à écrire, { $skipped } ignorés
}
bake-error-read = La bibliothèque n'a pas pu être lue : { $error }
bake-survey-counting = Parcours de la bibliothèque...
bake-survey-progress = Lecture des tags, { $done } sur { $total }
bake-nothing-to-embed = Rien à intégrer : les fichiers portent déjà tout ce que rox détient
bake-rewrites = { $count ->
    [one] { $count } fichier sera réécrit
   *[other] { $count } fichiers seront réécrits
}
bake-hint-before = Appuyez sur
bake-hint-key = Entrée
bake-hint-after = pour intégrer
bake-embed = Intégrer
bake-cancel = Annuler

## Arrange editors and header pieces

arrange-shown = Affichés
arrange-hidden = Masqués
tile-face-mosaic = Mosaïque de pochettes
tile-face-tinted = Mosaïque teintée
tile-face-gradient = Carte dégradée
tile-face-color = Carte unie
head-piece-artist = Artiste
head-piece-album = Album
head-piece-year = Année
head-piece-genre = Genre
head-piece-quality = Qualité
head-piece-tracks = Pistes
head-piece-time = Durée
head-piece-spacer = Espace
head-piece-divider = Séparateur
head-piece-art = Pochette
head-unknown = Inconnu
status-item-count = Nombre
status-item-time = Durée
status-item-albums = Albums
status-item-artists = Artistes
status-item-plays = Écoutes
volume-item-icon = Icône
volume-item-slider = Curseur
volume-item-percent = Pourcentage

## Filter chips and search menus

filter-field-artist = Artiste
filter-field-album-artist = Artiste de l'album
filter-field-album = Album
filter-field-genre = Genre
filter-field-year = Année
filter-field-folder = Dossier
filter-unknown = Inconnu
filter-clear = Effacer
query-show-search-box = Afficher le champ de recherche
query-own-query = Recherche propre
query-shared-query = Recherche partagée
headers-off = Désactivés
headers-compact = Compacts
headers-expanded = Étendus

## Panel context menu

panel-dock-back = Réancrer
panel-pop-out = Détacher
panel-close = Fermer
panel-duplicate = Dupliquer
panel-reveal-in-browser = Afficher dans le gestionnaire de fichiers
panel-play-next = Lire ensuite
panel-add-to-queue = Ajouter à la file
panel-add-to-playlist = Ajouter à la playlist
shader-pick-missing = { $name } (manquant)
shader-pick-custom = Personnalisé

## Shipped shader examples

shader-blurb-plasma = Couleur à la dérive tirée de ses seuls uniforms, donc il ne coûte qu'un quad tout simple.
shader-blurb-trails = Étale sa propre image précédente, ce qui le place sur la passe écran.
shader-blurb-sheen = Un vignettage et un éclat qui dérive, surcouche transparente pour un panneau qui dessine déjà.
shader-blurb-shadow = Une ombre portée que projettent le texte et les contrôles du panneau, lue depuis la capture du masque.
shader-blurb-cover = La pochette du morceau en cours, en letterbox sur un lavis de sa propre couleur.
shader-blurb-badge = La pochette en petite carte garée dans un coin, avec un emplacement pour la promener.
shader-blurb-lamp = Une lumière qui suit le curseur et répond aux boutons, surcouche transparente.
shader-blurb-cube = Un cube en fil de fer qui culbute en faux 3D, dessiné en lumière additive.
shader-blurb-bloom = Des orbes à la dérive bloomés par une seconde passe en demi-taille, la chaîne en miniature.
shader-blurb-tube = Rejoue le panneau du dessous à travers une dalle cathodique bombée, lignes de balayage comprises.

## Transport strip pieces

seek-item-elapsed = Écoulé
seek-item-strip = Barre
seek-item-ending = Restant
seek-item-duration = Durée
info-item-track-no = N° de piste
info-item-title = Titre
info-item-duration = Durée
info-item-next = Suivant
info-item-queued = En file
info-item-output = Sortie
info-item-favourite = Favori
info-item-rating = Note
playback-item-previous = Précédent
playback-item-seek-back = Reculer
playback-item-play = Lecture
playback-item-seek-forward = Avancer
playback-item-next = Suivant
playback-item-stop = Arrêt
playback-item-volume = Volume
playback-item-loop = Boucle
playback-item-shuffle = Aléatoire
playback-item-continue = Continuer
playback-item-crossfade = Fondu enchaîné
playback-item-random = Au hasard
playback-item-stop-after = Arrêter après
playback-item-favourite = Favori
playback-item-rating = Note

## Dock chrome

dock-empty-tab = Onglet vide
dock-unnamed = Sans nom
dock-tiles = Tuiles
dock-zoom-in = Agrandir
dock-zoom-out = Réduire
dock-collapse = Replier
dock-expand = Déplier

## Shader picker notes

shader-note-empty = Choisis un exemple pour commencer, ou pointe rox vers un fichier .wgsl avec une étape fragment définissant fs_user(uv)
shader-note-missing = { $name } n'est plus dans les shaders de cet espace de travail, donc rien ne se dessine. Choisis autre chose ici et ce panneau aura sa propre source.
shader-note-shared = Partagé dans cet espace de travail. Le modifier met à jour toutes les surfaces qui l'utilisent.
shader-note-file = { $path }. Tes enregistrements se rechargent pendant que le shader dessine, et la source voyage dans les dispositions et les lots, elle survit donc à une machine qui n'a jamais eu le fichier.
shader-note-custom = Cette source voyage dans sa disposition ou son lot sans fichier derrière elle. Modifier comme fichier la réécrit et reprend tes enregistrements.

## Panel pages and shared sides

panel-page-layout = Disposition
panel-page-view = Vue
panel-page-content = Contenu
panel-page-source = Source
panel-page-bindings = Liaisons
panel-page-emitters = Émetteurs
panel-page-forces = Forces
side-left = Gauche
side-right = Droite
genre-face-mosaic = Mosaïque
genre-face-tinted = Teinté
genre-face-gradient = Dégradé
genre-face-color = Couleur

## Library panel

panel-title-library = Bibliothèque
library-play = Lire
library-play-album = Lire l'album
library-play-group = Lire le groupe
library-play-tracks = Lire { $count } pistes
library-play-similar = Lire des morceaux proches
library-filter-by-album = Filtrer par album
library-filter-by-artist = Filtrer par artiste
library-jump-to-playing = Aller à la piste en cours
library-menu-display = Affichage
library-disc = Disque { $number }
library-empty-title = Ouvrir un dossier de musique
library-empty-note = Il sera analysé dans la bibliothèque (flac, mp3, wav)
library-headers = En-têtes
    .description = Ruptures de groupe sur la liste; un tri garde ensemble ce qui se suit, la recherche affiche à plat
library-group-by = Grouper par
    .description = Sur quoi les en-têtes rompent; genre et année retrient la liste
library-header-row = Ligne d'en-tête
    .description = Ce que les en-têtes d'une ligne empilent de gauche à droite; un espace ou un séparateur partage les côtés
library-header-lines = Lignes d'en-tête
    .description = Les lignes du bloc, de haut en bas; une ligne vide disparaît
library-follow-description = Défiler jusqu'à la piste en cours à chaque changement de morceau
library-resume-description = Revenir à la piste en cours quand tu arrêtes de parcourir
library-smooth-description = Glisser jusqu'à la ligne au lieu de sauter
library-columns = Colonnes
    .description = Quelles colonnes s'affichent; fais glisser les en-têtes dans le panneau pour les réordonner et les dimensionner
library-column-headers = En-têtes de colonnes
    .description = La ligne d'en-tête triable au-dessus de la liste; masquée, les colonnes gardent leur ordre et leur largeur
library-compact-plays = Écoutes compactes
    .description = La colonne des écoutes en petit compteur avec un tiret à côté
library-line-height = Hauteur de ligne
    .description = Une ligne d'en-tête; les blocs prennent les lignes qu'il leur faut, indépendamment des lignes de pistes
library-text-size = Taille du texte
    .description = Le texte des lignes d'en-tête, indépendant de la hauteur de ligne, pour que la pochette grandisse seule
library-flush-background = Fond aligné
    .description = Poser les en-têtes sur le fond de la liste plutôt que sur la teinte relevée; les couleurs du morceau les déplacent ensemble
library-gap-above = Espace au-dessus
    .description = Retranché du haut du bloc; la liste transparaît, et les lignes se resserrent
library-gap-below = Espace en dessous
    .description = La même chose sous le bloc, avant ses pistes
library-section-rows = Lignes
library-row-height = Hauteur de ligne
    .description = Les lignes de pistes; le texte suit, et les deux se mettent à l'échelle avec la police de l'application
library-row-spacing = Espacement des lignes
    .description = Hauteur supplémentaire par ligne; de l'air sans grossir le texte
library-stripes = Surlignage alterné
    .description = Teinter une ligne de piste sur deux pour qu'une longue liste se parcoure
library-row-borders = Bordures de ligne
    .description = Le filet sous chaque ligne de piste
library-art-description = La tuile des en-têtes étendus: la pochette, le portrait de l'artiste, ou l'image du genre
library-art-rounding = Arrondi de la pochette
    .description = Arrondir les coins de la pochette
library-art-position = Position de la pochette
    .description = De quel côté du bloc se place la tuile des en-têtes étendus
library-art-margin = Marge de la pochette
    .description = Rentrer la tuile dans le bloc; elle rétrécit pour rester carrée
library-circular-portraits = Portraits ronds
    .description = Groupé par artiste, arrondir les tuiles au cercle complet du mur plutôt qu'au réglage d'arrondi
library-genre-face = Image du genre
    .description = Groupé par genre, ce que porte la tuile: les pochettes, les pochettes lavées dans la couleur du genre, ou une carte unie sous sa géométrie

## Album grid panel

panel-title-album-grid = Grille d'albums
grid-menu-scroll = Défilement
grid-vertical-scroll = Défilement vertical
grid-horizontal-scroll = Défilement horizontal
grid-jump-to-playing = Aller à l'album en cours
grid-library-empty = La bibliothèque est vide
grid-play-albums = Lire { $count } albums
grid-vertical-layout = Disposition verticale
    .description = Faire défiler le mur de haut en bas, les lignes remplissant la largeur; désactivé, il défile de gauche à droite, les colonnes remplissant la hauteur
grid-follow-description = Défiler jusqu'à l'album en cours à chaque changement de morceau
grid-resume-description = Revenir à l'album en cours quand tu arrêtes de parcourir
grid-smooth-description = Glisser jusqu'à l'album au lieu de sauter
grid-section-dimming = Assombrissement
grid-section-tiles = Tuiles
grid-dim-while-playing = Assombrir pendant la lecture
    .description = Estomper toutes les pochettes sauf celle de l'album en cours; le survol rallume une tuile
grid-dim-amount = Intensité
    .description = À quel point les autres pochettes s'estompent; 100% les masque
grid-desaturate = Désaturer pendant la lecture
    .description = Vider toutes les pochettes sauf celle de l'album en cours vers le gris; le survol rend sa couleur à une tuile
grid-always = Toujours
    .description = Garder les pochettes en retrait même quand rien ne joue; seule la tuile survolée s'affiche pleinement
grid-show-titles = Afficher les titres
    .description = Imprimer l'album et l'artiste sous chaque pochette, façon iTunes, au lieu du seul survol
grid-title-alignment = Alignement des titres
    .description = Aligner les légendes sous leurs pochettes
grid-tile-size = Taille des tuiles
    .description = Le plus grand côté des tuiles de pochette; les colonnes se partagent la largeur du panneau à égalité
grid-gap = Écart
    .description = Espace entre les pochettes; zéro les colle bord à bord
grid-art-rounding-description = Arrondir les coins de chaque pochette; 100% donne un cercle
