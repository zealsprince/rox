### Français. Reflète en-CA/rox.ftl clé pour clé ; le test de parité
### dans rox-i18n y veille. Les clés sont en kebab-case préfixé par
### surface ; la description d'une ligne est un attribut sur le
### message de son libellé.

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
    .description = Afficher le champ de recherche ; la requête ne s'applique que tant qu'il est affiché
query-source = Source de recherche
    .description = Suivre la requête partagée, filtrer avec le champ propre à ce panneau, ou montrer ce qu'un autre panneau a sélectionné
query-source-shared = Partagée
query-source-own = Propre
query-source-selection = Sélection

## Signals and routes
signal-source = Source
    .description = Ce que le signal écoute : Bande suit une plage de fréquences, Niveau tout le mixage, Attaque pulse à chaque frappe dans la plage, Déclencheur lance une impulsion quand la plage atteint son seuil, Total cumule un autre signal dans le temps
signal-kind-band = Bande
signal-kind-level = Niveau
signal-kind-onset = Attaque
signal-kind-trigger = Déclencheur
signal-kind-total = Total
signal-response = Réponse
signal-response-pulse = Combien de temps chaque impulsion résonne avant de s'éteindre
signal-response-drift = 0 colle à la musique, 100 dérive derrière elle
signal-threshold = Seuil
signal-threshold-trigger = Le niveau que la plage doit atteindre pour lancer l'impulsion ; elle ne repart pas tant que le niveau n'est pas redescendu sous la marque du vumètre au-dessus
signal-threshold-gate = En dessous, le signal est coupé ; au-dessus, la sortie remonte depuis zéro, donc les passages calmes ne font pas bouger le bouton. La marque sur le vumètre au-dessus montre où il se trouve
signal-low-bound = Limite basse
signal-high-bound = Limite haute
signal-adds-up = Cumule
    .description = Quel signal est totalisé ici ; il monte tant que celui-là est fort et cale tant qu'il est calme
signal-aggregate-nothing = Rien à suivre
signal-aggregate-pick = Choisir un signal
signal-aggregate-alone = Il n'y a aucun autre signal à cumuler, donc il reste à zéro. Ajoutes-en un et il apparaîtra dans la liste.
signal-aggregate-unpicked = Rien de choisi, donc ce total reste à zéro. Choisis un signal ci-dessus.
signal-rate = Cadence
    .description = Tours par seconde à plein régime ; après 1 il repasse à 0 et continue de monter, ce qu'un shader interprète comme une phase
signal-reset-on-track = Réinitialiser au changement de piste
    .description = Redescendre à zéro quand un nouveau morceau commence, pour qu'une phase ne reparte pas du total du précédent
signal-flush = Vider
signal-routes-in-panel = { $count ->
    [one] { $count } route dans ce panneau
   *[other] { $count } routes dans ce panneau
}
    .description = Le renvoyer à zéro maintenant ; il redescend progressivement plutôt que d'un coup, pour que rien de ce qui le suit ne saute
route-header = Route
route-signal = Signal
    .description = Quel signal partagé cette route suit ; le régler ici règle toutes les routes qui le suivent
route-new-signal = Nouveau signal
route-shared-note = Partagé par toutes les routes sur ce signal
route-signal-gone = Le signal de cette route a disparu ; le bouton garde sa valeur jusqu'à ce qu'un autre soit choisi ci-dessus.
route-range-note = Plage pour ce paramètre uniquement
route-quiet = Silence
    .description = Ce que vaut le bouton dans le silence, en part de son propre réglage
route-loud = Plein
    .description = Ce qu'il vaut à plein signal ; 100 % est la valeur du curseur lui-même, sous Silence module vers le bas
route-slot = Emplacement
    .description = Lequel des seize emplacements de signal du shader cette route remplit
route-slot-quiet-description = Ce que vaut l'emplacement dans le silence
route-slot-loud-description = Ce qu'il vaut à plein signal ; sous Silence l'emplacement tourne à l'envers
route-slot-signal-description = Quel signal partagé cette route suit
route-slot-signal-gone = Le signal de cette route a disparu ; l'emplacement reste à zéro jusqu'à ce qu'un autre soit choisi.
route-add = Ajouter une route
route-unrouted = Sans route
route-pick-slot = Choisir un emplacement
route-pick-signal = Choisir un signal
route-no-signal = aucun signal
route-no-signals-yet = Il n'y a encore aucun signal à suivre. Crées-en un et il apparaîtra ici ; jusque-là l'emplacement reste à zéro.
route-open-signals = Ouvrir les signaux
route-create-signal = Créer un nouveau signal

## Panel settings window
panel-settings = Réglages du panneau
panel-menu-label = Panneau
panel-save-as-preset = Enregistrer comme préréglage
panel-rename = Renommer
panel-rename-name = Nom
panel-rename-note = Affiché comme onglet du panneau ; vide revient au nom d'origine
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
    .description = Fixer le panneau en place ; on ne peut plus le déplacer ni le réorganiser dans le dock
panel-drag-anchor = Poignée de déplacement
    .description = Un glissement n'importe où sur le panneau déplace la fenêtre, tandis que les clics simples atteignent toujours ses contrôles ; pour les dispositions sans décorations
panel-slot-controls = Contrôles d'emplacement
    .description = Afficher les boutons de coin pour échanger et retirer les panneaux hébergés ici. Masqués, la disposition se modifie toujours depuis l'arbre de la page Espace de travail dans les réglages
panel-min-width = Largeur minimale
    .description = Où un redimensionnement cesse de resserrer le panneau. Pris tel quel, y compris sous le plancher propre du panneau, si bien qu'une bande compacte peut aller plus étroit que d'origine ; vide laisse le plancher tranquille
panel-max-width = Largeur maximale
    .description = Plafonner la largeur du panneau pour qu'il ne s'étire pas quand la fenêtre s'élargit
panel-min-height = Hauteur minimale
    .description = Où un redimensionnement cesse de tasser le panneau. Pris tel quel, y compris sous le plancher propre du panneau, si bien qu'une bande compacte peut aller plus court que d'origine ; vide laisse le plancher tranquille
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
    .description = Une ligne autour du bord du panneau, dans la couleur du rôle Bordure ; un côté à zéro n'en dessine aucune
panel-font = Police
    .description = La typographie du panneau ; par défaut elle suit la police de l'application
panel-font-size = Taille de police
    .description = La taille du texte du panneau relative à la police de l'application ; les lignes se mettent à l'échelle avec
panel-surface-shader = Shader de surface
    .description = Faire tourner un shader WGSL sur le corps de ce panneau, sous le shader d'écran de l'application
panel-run-when-idle = Continuer au repos
    .description = Continuer à dessiner des images tant que l'audio est silencieux. Désactivé, le shader se fige sur sa dernière image et le panneau ne coûte rien
panel-shader-is-scene = Ce shader est une scène, il couvre donc le corps du panneau au lieu de dessiner par-dessus. Il vient d'un lot ou d'une ancienne config ; la liste ci-dessus ne propose que des shaders qui laissent le panneau lisible.

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
preset-back-tail = dans n'importe quel menu de panneau. Les préréglages ne valent que pour cet espace de travail ; un autre espace ne les aura pas.

## Keyboard hints
hint-press = Appuie sur
hint-key-enter = Entrée

## Settings: language
settings-language = Langue
    .description = La langue de l'interface ; Système se cale sur la liste du système d'exploitation et retombe sur l'anglais si rien ne correspond
    .keywords = langue traduction localisation
settings-language-system = (Langue du système)
settings-language-search = Rechercher une langue
picker-no-matches = Aucun résultat
settings-search-no-matches = Aucun résultat pour « { $text } »

## Embed dialog
bake-window-title = rox - Intégrer les métadonnées stockées
bake-title = Intégrer les métadonnées stockées
bake-intro = Écrit ce que rox détient déjà dans les fichiers eux-mêmes, pour qu'un autre lecteur le lise aussi. Rien n'est recalculé.
bake-formats = MP3 et FLAC uniquement ; les autres formats et les pistes CUE sont ignorés
bake-source-lyrics = Paroles
bake-source-gain = ReplayGain
bake-source-acoustic = Descriptions acoustiques
bake-detail-nothing = rien de stocké à intégrer
bake-detail-only-skipped = { $skipped ->
    [one] rien à écrire, { $skipped } ignoré
   *[other] rien à écrire, { $skipped } ignorés
}
bake-detail-writes = { $count ->
    [one] { $count } fichier à écrire
   *[other] { $count } fichiers à écrire
}
bake-detail-writes-skipped = { $count ->
    [one]
        { $count } fichier à écrire, { $skipped ->
            [one] { $skipped } ignoré
           *[other] { $skipped } ignorés
        }
   *[other]
        { $count } fichiers à écrire, { $skipped ->
            [one] { $skipped } ignoré
           *[other] { $skipped } ignorés
        }
}
bake-error-read = La bibliothèque n'a pas pu être lue : { $error }
bake-survey-counting = Parcours de la bibliothèque...
bake-survey-progress = Lecture des tags, { $done } sur { $total }
bake-nothing-to-embed = Rien à intégrer : les fichiers contiennent déjà tout ce que rox a stocké
bake-rewrites = { $count ->
    [one] { $count } fichier sera réécrit
   *[other] { $count } fichiers seront réécrits
}
bake-hint-before = Appuie sur
bake-hint-key = Entrée
bake-hint-after = pour intégrer
bake-embed = Intégrer
bake-cancel = Annuler
bake-summary-files = { $count ->
    [one] { $count } fichier
   *[other] { $count } fichiers
}
bake-summary-updated = { $files } mis à jour
bake-summary-stopped = Arrêt après { $files } mis à jour
bake-summary-skipped = { $count ->
    [one] , { $count } ignoré
   *[other] , { $count } ignorés
}
bake-summary-failed = , { $count } en échec

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
panel-favourite-add = Ajouter aux favoris
panel-favourite-remove = Retirer des favoris
shader-pick-missing = { $name } (manquant)
shader-pick-custom = Personnalisé

## Shipped shader examples
shader-blurb-plasma = Couleur à la dérive tirée de ses seuls uniforms, donc il ne coûte qu'un simple quad.
shader-blurb-trails = Étale sa propre image précédente, ce qui le place sur la passe écran.
shader-blurb-sheen = Un vignettage et un éclat qui dérive, surcouche transparente pour un panneau qui dessine déjà.
shader-blurb-shadow = Une ombre portée que projettent le texte et les contrôles du panneau, lue depuis la capture du masque.
shader-blurb-cover = La pochette du morceau en cours, en letterbox sur un lavis de sa propre couleur.
shader-blurb-badge = La pochette en petite carte posée dans un coin, avec un emplacement pour la déplacer.
shader-blurb-lamp = Une lumière qui suit le curseur et répond aux clics, surcouche transparente.
shader-blurb-cube = Un cube en fil de fer qui culbute en faux 3D, dessiné en lumière additive.
shader-blurb-bloom = Des orbes à la dérive bloomés par une seconde passe en demi-taille, la chaîne en miniature.
shader-blurb-tube = Rejoue le panneau du dessous à travers une dalle cathodique bombée, lignes de balayage comprises.

## Transport strip pieces
seek-item-elapsed = Écoulé
seek-item-strip = Barre
seek-item-ending = Fin
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
shader-note-file = { $path }. Tes enregistrements se rechargent pendant que le shader dessine, et la source est stockée dans les dispositions et les lots, donc elle marche encore sur une machine qui n'a jamais eu le fichier.
shader-note-custom = Cette source est stockée dans sa disposition ou son lot, sans fichier derrière elle. Modifier comme fichier la réécrit et reprend tes enregistrements.

## Panel pages and shared sides
panel-page-layout = Disposition
panel-page-view = Vue
panel-page-content = Contenu
panel-page-source = Source
panel-page-bindings = Liaisons
panel-page-emitters = Émetteurs
panel-page-forces = Forces
panel-page-scene = Scène
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
library-play-similar = Lire des pistes proches
library-filter-by-album = Filtrer par album
library-filter-by-artist = Filtrer par artiste
library-jump-to-playing = Aller à la piste en cours
library-menu-display = Affichage
library-disc = Disque { $number }
library-empty-title = Ouvrir un dossier de musique
library-empty-note = Il sera analysé dans la bibliothèque (flac, mp3, wav)
library-headers = En-têtes
    .description = Ruptures de groupe sur la liste ; un tri garde ensemble ce qui se suit, une recherche affiche la liste à plat
library-group-by = Grouper par
    .description = Sur quoi les en-têtes rompent ; genre et année retrient la liste
library-header-row = Ligne d'en-tête
    .description = Ce qu'affichent les en-têtes d'une ligne, de gauche à droite ; un espace ou un séparateur partage les côtés
library-header-lines = Lignes d'en-tête
    .description = Les lignes du bloc, de haut en bas ; une ligne vide disparaît
library-follow-description = Défiler jusqu'à la piste en cours à chaque changement de morceau
library-resume-description = Revenir à la piste en cours quand tu arrêtes de parcourir
library-smooth-description = Glisser jusqu'à la ligne au lieu de sauter
library-columns = Colonnes
    .description = Quelles colonnes s'affichent ; fais glisser les en-têtes dans le panneau pour les réordonner et les dimensionner
library-column-headers = En-têtes de colonnes
    .description = La ligne d'en-tête triable au-dessus de la liste ; masquée, les colonnes gardent leur ordre et leur largeur
library-compact-plays = Écoutes compactes
    .description = La colonne des écoutes en petit compteur avec un tiret à côté
library-line-height = Hauteur de ligne
    .description = Une ligne d'en-tête ; les blocs prennent les lignes qu'il leur faut, indépendamment des lignes de pistes
library-text-size = Taille du texte
    .description = Le texte des lignes d'en-tête, indépendant de la hauteur de ligne, pour que la pochette grandisse seule
library-flush-background = Fond aligné
    .description = Poser les en-têtes sur le fond de la liste plutôt que sur la teinte relevée ; les couleurs du morceau les déplacent ensemble
library-gap-above = Espace au-dessus
    .description = Retranché du haut du bloc ; la liste transparaît, et les lignes se resserrent
library-gap-below = Espace en dessous
    .description = La même chose sous le bloc, avant ses pistes
library-section-rows = Lignes
library-row-height = Hauteur de ligne
    .description = Les lignes de pistes ; le texte suit, et les deux se mettent à l'échelle avec la police de l'application
library-row-spacing = Espacement des lignes
    .description = Hauteur supplémentaire par ligne ; de l'air sans grossir le texte
library-stripes = Surlignage alterné
    .description = Teinter une ligne de piste sur deux pour qu'une longue liste se lise d'un coup d'œil
library-row-borders = Bordures de ligne
    .description = Le filet sous chaque ligne de piste
library-art-description = La tuile des en-têtes étendus : la pochette, le portrait de l'artiste, ou l'image du genre
library-art-rounding = Arrondi de la pochette
    .description = Arrondir les coins de la pochette
library-art-position = Position de la pochette
    .description = De quel côté du bloc se place la tuile des en-têtes étendus
library-art-margin = Marge de la pochette
    .description = Rentrer la tuile dans le bloc ; elle rétrécit pour rester carrée
library-circular-portraits = Portraits ronds
    .description = Groupé par artiste, arrondir les tuiles au cercle complet du mur plutôt qu'au réglage d'arrondi
library-genre-face = Image du genre
    .description = Groupé par genre, ce qu'affiche la tuile : les pochettes, les pochettes teintées de la couleur du genre, ou une carte unie sous sa géométrie

## Album grid panel
panel-title-album-grid = Grille d'albums
grid-menu-scroll = Défilement
grid-vertical-scroll = Défilement vertical
grid-horizontal-scroll = Défilement horizontal
grid-jump-to-playing = Aller à l'album en cours
grid-library-empty = La bibliothèque est vide
grid-play-albums = Lire { $count } albums
grid-vertical-layout = Disposition verticale
    .description = Faire défiler le mur de haut en bas, les lignes remplissant la largeur ; désactivé, il défile de gauche à droite, les colonnes remplissant la hauteur
grid-follow-description = Défiler jusqu'à l'album en cours à chaque changement de morceau
grid-resume-description = Revenir à l'album en cours quand tu arrêtes de parcourir
grid-smooth-description = Glisser jusqu'à l'album au lieu de sauter
grid-section-dimming = Assombrissement
grid-section-tiles = Tuiles
grid-dim-while-playing = Assombrir pendant la lecture
    .description = Estomper toutes les pochettes sauf celle de l'album en cours ; le survol rallume une tuile
grid-dim-amount = Intensité
    .description = À quel point les autres pochettes s'estompent ; 100 % les masque
grid-desaturate = Désaturer pendant la lecture
    .description = Passer en gris toutes les pochettes sauf celle de l'album en cours ; le survol rend sa couleur à une tuile
grid-always = Toujours
    .description = Garder les pochettes en retrait même quand rien ne joue ; seule la tuile survolée s'affiche pleinement
grid-show-titles = Afficher les titres
    .description = Afficher l'album et l'artiste sous chaque pochette, façon iTunes, au lieu du seul survol
grid-title-alignment = Alignement des titres
    .description = Aligner les légendes sous leurs pochettes
grid-tile-size = Taille des tuiles
    .description = Le plus grand côté des tuiles de pochette ; les colonnes se partagent la largeur du panneau à égalité
grid-gap = Écart
    .description = Espace entre les pochettes ; zéro les colle bord à bord
grid-art-rounding-description = Arrondir les coins de chaque pochette ; 100 % donne un cercle

## Settings: sidebar pages
settings-page-appearance = Apparence
settings-page-application = Application
settings-page-audio = Audio
settings-page-development = Développement
settings-page-integrations = Intégrations
settings-page-keymap = Raccourcis
settings-page-library = Bibliothèque
settings-page-mcp = MCP
settings-page-ml-models = Modèles ML
settings-page-playback = Lecture
settings-page-providers = Fournisseurs
settings-page-shader = Shader
settings-page-storage = Stockage
settings-page-workspace = Espace de travail

## Settings: appearance
settings-appearance-backdrop-all-windows = Toutes les fenêtres
    .description = Appliquer le fond aux fenêtres enfants aussi : réglages, éditeurs, dialogues, panneaux détachés. Désactivé, le fond et la transparence s'en tiennent aux fenêtres de l'espace de travail
settings-appearance-backdrop-strength = Intensité du fond
    .description = À quel point le fond de pochette se voit derrière elles
settings-appearance-border = Bordure
    .description = Une ligne autour du bord de chaque panneau, dans la couleur du rôle Bordure ; un côté à zéro n'en dessine aucune
settings-appearance-colors-locked-note = Les couleurs du morceau sont actives, donc la piste en cours pilote ces couleurs et l'export les enregistre. Désactive-les ci-dessus pour les modifier
settings-appearance-design-mode = Mode conception
    .description = Modifier la disposition sur place : les lignes ajouter, renommer, dupliquer, détacher et fermer des menus de panneau, les contrôles qu'un conteneur fait flotter sur ses emplacements, et le glissement d'onglets. Désactivé cache tout ça ; la page Espace de travail modifie toujours l'arbre
    .keywords = modifier disposition reorganiser verrouiller
settings-appearance-font = Police
    .description = La typographie de toute l'application ; les panneaux peuvent la remplacer dans leurs propres réglages
    .keywords = police typographie caractere
settings-appearance-font-size = Taille de police
    .description = La taille de texte de base dont dépend le texte de chaque panneau ; les contrôles et les icônes gardent la leur
settings-appearance-hide-menubar = Masquer la barre de menus
    .description = Garder la barre de menus cachée, la faire flotter sur le dock tant qu'alt est enfoncée. Appuie deux fois sur alt pour la laisser affichée, ses boutons acceptent alors un simple clic
settings-appearance-icons-intro = Un pack est un dossier de SVG qui remplace les icônes intégrées ; le changement prend effet au prochain lancement
settings-appearance-icons-open-folder = Ouvrir le dossier
settings-appearance-inverse-from-dark = Inverser depuis le thème sombre
settings-appearance-inverse-from-light = Inverser depuis le thème clair
settings-appearance-keep-theme = Garder le thème
    .description = Conserver le thème actif même quand la luminosité d'une pochette le ferait basculer ; les couleurs du morceau teintent toujours
settings-appearance-margin = Marge
    .description = Rentrer chaque panneau depuis sa cellule ; un panneau peut le remplacer dans ses propres réglages
settings-appearance-new-pack = Nouveau pack
settings-appearance-os-decorations = Décorations système
    .description = La barre de titre et les bordures du système sur les fenêtres principales ; désactivé, on s'appuie sur les contrôles de fenêtre et les panneaux à poignée de déplacement
settings-appearance-pack-name-placeholder = Nom du pack
settings-appearance-padding = Remplissage
    .description = Espace à l'intérieur du bord de chaque panneau, gardé dans son propre fond
settings-appearance-palette-export = Exporter
settings-appearance-palette-import = Importer
settings-appearance-panel-seams = Coutures des panneaux
    .description = Le filet entre les tuiles de panneau ; désactivé, les poignées de redimensionnement restent invisibles mais toujours glissables
settings-appearance-resize-border = Bordure de redimensionnement
    .description = Redimensionner les fenêtres principales en glissant leurs bords ; ne s'applique qu'avec les décorations système désactivées, et le désactiver laisse l'accrochage et Win+flèche comme moyen de redimensionner
settings-appearance-rounding = Arrondi
    .description = Arrondir les coins de chaque panneau vers le fond
settings-appearance-section-colors = Couleurs
settings-appearance-section-frame = Cadre
settings-appearance-section-icons = Icônes
settings-appearance-section-interface = Interface
settings-appearance-section-theming = Thématisation
settings-appearance-section-transparency = Transparence
settings-appearance-section-typography = Typographie
settings-appearance-song-theming = Couleurs du morceau
    .description = Teinter la palette et habiller les fenêtres avec la pochette de la piste en cours
settings-appearance-surface-opacity = Opacité de surface
    .description = À quel point les surfaces de l'application paraissent opaques sur le fond
settings-appearance-theme = Thème
    .description = La palette que l'application affiche et celle que vise l'éditeur de couleurs ci-dessous ; Système suit la préférence claire ou sombre du système
settings-appearance-theme-dark = Sombre
settings-appearance-theme-light = Clair
settings-appearance-theme-system = Système

## Settings: application
settings-application-check-updates = Rechercher les mises à jour
    .description = Chercher une version plus récente une fois par jour au démarrage de rox ; la fenêtre À propos vérifie tout de suite dans les deux cas
settings-application-download-updates = Télécharger les mises à jour
    .description = Quand une vérification trouve une version plus récente, la télécharger et la préparer en arrière-plan ; le prochain démarrage la lance
settings-application-enable-ai = Activer les fonctions IA
    .description = Laisser les outils d'IA parler à rox : ajoute la prise en charge MCP et les téléchargements de modèles ML, avec leurs pages dans la barre latérale.
settings-application-lock-panel-resize = Verrouiller le redimensionnement des panneaux
    .description = Les séparations de panneaux ne se redimensionnent qu'avec le mode conception actif, pour qu'un glissement près d'une couture ne déplace pas une disposition finie
settings-application-portable-copying = Copie des données...
settings-application-portable-mode = Mode portable
    .description = Garder les réglages, la bibliothèque et les caches dans un dossier rox-data à côté de l'exécutable, pour que le lecteur voyage avec ses données. Le désactiver revient au dossier système et laisse rox-data en place
settings-application-portable-not-writable = Le dossier de l'application n'est pas accessible en écriture
settings-application-portable-restart-note = S'applique au prochain lancement ; cette session reste sur son dossier actuel
settings-application-remain-in-tray = Rester dans la zone de notification
    .description = Garder la musique en lecture quand la dernière fenêtre se ferme, avec l'icône de la zone de notification (le dock sur macOS) comme moyen de revenir
settings-application-section-ai = IA
settings-application-section-control-socket = Socket de contrôle
settings-application-section-data = Données
settings-application-section-layout = Disposition
settings-application-section-startup = Démarrage
settings-application-section-window = Fenêtre
settings-application-socket-path = Chemin du socket
    .description = L'interface machine de rox pendant qu'il tourne : JSON-RPC sur un socket local, rattaché à ce dossier de données. roxctl le pilote depuis un shell, et le proxy rox-mcp sert les clients MCP par-dessus

## Settings: audio
settings-audio-broadcast-bitrate = Débit
    .description = Ce que l'encodeur MP3 dépense par seconde de flux
settings-audio-broadcast-enable = Diffuser vers Icecast
    .description = Pousser ce que rox joue vers un serveur icecast en tant que client source, encodé en MP3. Le point de montage, les auditeurs et la face réseau appartiennent à icecast ; rox ne fait que se connecter, et un serveur injoignable ne touche jamais à la lecture locale
settings-audio-broadcast-host-placeholder = hôte icecast
settings-audio-broadcast-login = Identifiants source
    .description = Les identifiants source d'icecast, l'utilisateur et le mot de passe que sa config nomme
settings-audio-broadcast-mount = Point de montage
    .description = Le point de montage sur lequel les auditeurs se branchent, et le nom de flux qu'il annonce
settings-audio-broadcast-name-placeholder = Nom du flux
settings-audio-broadcast-password-placeholder = Mot de passe source
settings-audio-broadcast-server = Serveur
    .description = L'hôte et le port du serveur icecast ; le protocole source passe par un socket simple
settings-audio-broadcast-user-placeholder = source
settings-audio-crossfade = Fondu enchaîné
    .description = Combien de temps une piste chevauche la suivante. Le fondu est là pour l'aléatoire et les sauts, donc les frontières d'un album restent intactes sauf si la ligne ci-dessous en décide autrement. Zéro le désactive
    .keywords = fondu transition enchaine sans blanc
settings-audio-equalizer-note = Dix bandes d'octave sur la sortie. Il s'ouvre dans sa propre fenêtre, parce qu'on le travaille pendant que la musique joue au lieu de le régler une fois pour toutes
settings-audio-exclusive-mode = Mode exclusif
    .description = Réserver le périphérique à rox seul et le faire tourner à la fréquence propre du fichier là où le matériel l'accepte ; désactivé, le mélangeur système est partagé avec tout le reste du bureau
settings-audio-fade-inside-albums = Fondre à l'intérieur des albums
    .description = Faire chevaucher aussi les pistes du même disque. Désactivé, les raccords d'un disque restent exactement comme ils ont été masterisés, là où l'enchaînement sans blanc compte le plus
settings-audio-open-equalizer = Ouvrir l'égaliseur
settings-audio-output-buffer = Tampon
    .description = Combien d'audio la carte garde à la fois. Plus court réagit plus vite et craque plus tôt sur une machine chargée ; plus long est plus sûr et plus mou
settings-audio-output-buffer-default = Par défaut (10 ms)
settings-audio-output-device = Périphérique
    .description-default = Le réglage par défaut du système suit ce qui est configuré sur le bureau
    .description-linux = L'exclusif réserve une carte directement auprès du noyau, donc la liste montre des cartes son plutôt que les sorties du bureau. Le Bluetooth et les autres périphériques de serveur de son n'ont pas de carte à réserver et n'apparaissent qu'avec l'exclusif désactivé
    .description-other = L'exclusif prend le périphérique pour rox seul, donc rien d'autre sur le bureau ne peut sonner par lui tant que le mode est actif
settings-audio-output-device-system-default = Par défaut du système
settings-audio-output-experimental-badge = Expérimental
settings-audio-output-experimental-tooltip = Le moteur exclusif de cette plateforme est écrit d'après le contrat audio documenté de la plateforme, mais il n'a jamais tourné sur du vrai matériel chez les développeurs. Il devrait réserver le périphérique ou retomber en partagé avec une raison, jamais rester muet. S'il se comporte mal, désactive-le et signale ce qui s'est passé avec le bouton à côté de ce badge.
settings-audio-output-format = Format
    .description = Ce que rox passe à la carte. Une carte qui refuse le choix tourne au format le plus large qu'elle a, et l'état ci-dessous indique lequel
settings-audio-output-format-f32 = Flottant 32 bits
settings-audio-output-format-s16 = Entier 16 bits
settings-audio-output-format-s32 = Entier 32 bits
settings-audio-output-format-widest = Le plus large disponible
settings-audio-output-issue-tooltip = Signaler le comportement du mode exclusif sur cette machine. Ouvre un ticket GitHub avec la plateforme et le flux négocié déjà remplis.
settings-audio-output-mode-exclusive = Exclusif
settings-audio-output-mode-shared = Partagé
settings-audio-output-not-built = Pas encore compilé pour cette plateforme
settings-audio-output-rate-follow = Suivre le fichier
settings-audio-output-sample-rate = Fréquence d'échantillonnage
    .description = Suivre rouvre le périphérique à la fréquence propre de chaque fichier, ce qui coûte un blanc à chaque frontière où la fréquence change ; fixer une fréquence ne paie jamais ça et rééchantillonne tout ce qui ne correspond pas
settings-audio-output-status-error-hint = Choisis un autre périphérique, ou désactive l'exclusif
settings-audio-output-status-error-title = Aucune sortie
settings-audio-output-status-idle-hint = Lance une piste pour voir le format que le périphérique a accepté
settings-audio-output-status-idle-title = Rien en lecture
settings-audio-replaygain-level-by = Niveler par
    .description = Jouer chaque piste au volume que ses tags ReplayGain ont mesuré, pour qu'une lecture aléatoire arrête de sauter d'un master à l'autre. Piste nivelle chaque fichier séparément ; Album applique le gain du disque à toutes ses pistes, ce qui garde les passages calmes et forts d'un album là où on les a mis
    .keywords = normalisation sonie volume egalisation
settings-audio-replaygain-measure-missing-button = Mesurer les manquants
settings-audio-replaygain-measure-new = Mesurer les nouveaux fichiers
    .description = Mesurer ce que la surveillance ramène au fur et à mesure, une fois la synchro stabilisée, pour qu'une bibliothèque qui grandit garde ses gains sans repasser par ici. Les chiffres vont là où pointe Enregistrer les gains mesurés. L'activer propose d'abord de mesurer ce qui manque déjà ; ensuite il ne voit plus que les fichiers qui viennent d'arriver
settings-audio-replaygain-measuring-progress = Mesure { $done } sur { $total }
settings-audio-replaygain-measuring-start = Mesure : recherche de ce qui manque...
settings-audio-replaygain-mode-album = Album
settings-audio-replaygain-mode-off = Désactivé
settings-audio-replaygain-mode-track = Piste
settings-audio-replaygain-preamp = Préampli
    .description = Ajouté à chaque gain taggé. La référence de ReplayGain se situe sous le niveau où les disques modernes sont gravés, donc une bibliothèque nivelée joue plus bas que la même bibliothèque brute ; c'est ici qu'on récupère la différence. Un gain ne sature jamais : le pic taggé le plafonne
settings-audio-replaygain-save = Enregistrer les gains mesurés
    .description = Où la passe de mesure met ses chiffres. La base de la bibliothèque laisse tes fichiers intacts ; les tags mettent les mêmes valeurs là où tous les autres lecteurs les lisent, au prix d'une réécriture des fichiers audio
settings-audio-replaygain-status-measured = { $total ->
    [one] La seule piste analysée a un gain de nivellement mesuré par rox
   *[other]
        Les { $total } pistes analysées ont toutes un gain de nivellement, dont { $measured ->
            [one] { $measured } mesurée par rox
           *[other] { $measured } mesurées par rox
        }
}
settings-audio-replaygain-status-tagged = { $total ->
    [one] La seule piste analysée a des tags ReplayGain
   *[other] Les { $total } pistes analysées ont toutes des tags ReplayGain
}
settings-audio-replaygain-untagged = Fichiers sans tags
    .description = À quel niveau joue un fichier sans tags ReplayGain. Rien ne l'a mesuré, donc c'est une supposition qui en tient lieu. Laisse-le à zéro et les pistes sans tags jouent comme elles l'ont toujours fait
settings-audio-section-broadcast = Diffusion
settings-audio-section-equalizer = Égaliseur
settings-audio-section-output = Sortie
settings-audio-section-playback = Lecture
settings-audio-section-replaygain = ReplayGain
settings-audio-transport = Transport
    .description = Lancer et arrêter sans quitter cette page, puisque chaque réglage ci-dessous se juge à l'oreille

## Settings: integrations
settings-integrations-discord-enable = Activer la présence enrichie
    .description = Afficher l'activité de rox sur Discord pendant la lecture
settings-integrations-discord-show-lastfm = Afficher le bouton Last.fm
    .description = Inclure un bouton cliquable « Voir sur Last.fm » dans le statut Discord
settings-integrations-discord-show-youtube = Afficher le bouton YouTube
    .description = Inclure un bouton cliquable « Rechercher sur YouTube » dans le statut Discord
settings-integrations-ffmpeg-binary = Binaire FFmpeg
    .description = Quel ffmpeg effectue les conversions ; laisse vide pour celui du PATH
settings-integrations-ffmpeg-fail-note = Convertir reste caché tant que ffmpeg ne pointe pas vers un binaire qui marche
settings-integrations-ffmpeg-fail-title = Ce ffmpeg n'a pas démarré
settings-integrations-ffmpeg-missing-note = Convertir reste caché ; installe ffmpeg ou pointe le chemin vers un binaire
settings-integrations-ffmpeg-missing-title = Aucun ffmpeg fonctionnel trouvé
settings-integrations-ffmpeg-ok-note = ffmpeg marche. Convertir est disponible.
settings-integrations-ffmpeg-test = Tester
settings-integrations-lastfm-api-key-row = Clé API
settings-integrations-lastfm-connect = Connecter
settings-integrations-lastfm-disconnect = Déconnecter
settings-integrations-lastfm-finish-connecting = Terminer la connexion
settings-integrations-lastfm-hearts = { $n ->
    [one] { $n } cœur
   *[other] { $n } cœurs
}
settings-integrations-lastfm-import-loved = Importer les morceaux aimés
settings-integrations-lastfm-intro-builtin = Connecte ton compte Last.fm : autorise rox dans le navigateur et les pistes jouées y sont scrobblées
settings-integrations-lastfm-intro-custom = Cette build n'embarque aucune identité api, donc le scrobbling demande ton propre compte api (Last.fm/api/account/create) ; colle sa clé et son secret partagé, puis connecte-toi
settings-integrations-lastfm-key-placeholder = Clé API
settings-integrations-lastfm-love-failed = Le dernier a échoué : { $error }
settings-integrations-lastfm-love-pending = { $hearts } en attente d'envoi
settings-integrations-lastfm-love-pending-failed = { $hearts } en attente d'envoi, dernière tentative : { $error }
settings-integrations-lastfm-reconnect = Reconnecter
settings-integrations-lastfm-secret-placeholder = Secret partagé
settings-integrations-lastfm-secret-row = Secret partagé
settings-integrations-lastfm-status-confirming = Confirmation...
settings-integrations-lastfm-status-connected = Connecté en tant que { $username }
settings-integrations-lastfm-status-elsewhere = Connecté sur une autre installation de rox ; chacune autorise sous sa propre identité api, donc connecte celle-ci aussi
settings-integrations-lastfm-status-failed = La connexion a échoué : { $error }
settings-integrations-lastfm-status-not-connected = Non connecté
settings-integrations-lastfm-status-rejected = Last.fm a rejeté la session et elle a été abandonnée. Reconnecte-toi pour continuer à scrobbler
settings-integrations-lastfm-status-requesting = Demande d'un jeton...
settings-integrations-lastfm-status-waiting = Autorise rox dans le navigateur, puis termine la connexion
settings-integrations-lastfm-working = En cours...
settings-integrations-love-favourites = Aimer les favoris
    .description = Refléter les cœurs sur Last.fm comme morceaux aimés ; reprendre un cœur le retire aussi là-bas
settings-integrations-scrobble-threshold = Seuil de scrobble
    .description = Quelle part d'une piste doit être jouée avant qu'elle scrobble ; la barre de lecture et la forme d'onde peuvent le marquer
settings-integrations-scrobble-tracks = Scrobbler les pistes
    .description = Envoyer les pistes jouées à Last.fm une fois le seuil franchi
settings-integrations-section-conversion = Conversion
settings-integrations-section-discord = Présence enrichie Discord
settings-integrations-section-favourites = Favoris
settings-integrations-section-lastfm = Last.fm
settings-integrations-section-scrobbling = Scrobbling

## Settings: keymap
settings-keymap-clash = { $chord } est aussi { $other } ; un seul des deux se déclenchera
settings-keymap-not-bound = Non assigné
settings-keymap-recording = Appuie sur les touches
settings-keymap-restore = Restaurer
settings-keymap-restore-all = Restaurer tous les raccourcis
    .description = Remettre chaque commande sur les touches d'origine, y compris celles dont cette build n'a plus de ligne
settings-keymap-section-defaults = Valeurs par défaut
settings-keymap-undo = Annuler
settings-keymap-undo-last = Annuler la dernière réinitialisation
    .description = Ramener les raccourcis que la dernière réinitialisation a jetés, une ligne ou toutes

## Settings: library
settings-library-acoustic-all-described = { $total ->
    [one] La seule piste analysée est décrite par { $label }
   *[other] Les { $total } pistes analysées sont toutes décrites par { $label }
}
settings-library-acoustic-auto = Décrire les nouveaux fichiers
    .description = Décrire ce que la surveillance ramène au fur et à mesure, une fois la synchro stabilisée, pour qu'une bibliothèque qui grandit garde ses descriptions sans repasser par ici. Désactivé, les nouveaux fichiers attendent le bouton Analyser les manquants. L'activer propose d'abord d'analyser ce qui manque déjà ; ensuite il ne voit plus que les fichiers qui viennent d'arriver
settings-library-acoustic-enable = Décrire le son des pistes
    .description = Déterminer à quoi ressemble chaque piste, pour que la bibliothèque puisse trouver de la musique proche de ce qui joue. Tout tourne sur cette machine, et décrire une grande bibliothèque prend un moment
    .keywords = similaire son empreinte decrire
settings-library-acoustic-extractor = Extracteur
settings-library-acoustic-extractor-model = Modèle
settings-library-acoustic-fallback = Analyse
settings-library-acoustic-partial = { $done ->
    [one] { $label } décrit { $done } piste analysée sur { $total }. Analyser les manquants traite le reste
   *[other] { $label } décrit { $done } pistes analysées sur { $total }. Analyser les manquants traite le reste
}
settings-library-acoustic-progress = { $running } en est à { $done } sur { $total }
settings-library-acoustic-progress-start = { $running } : évaluation de ce qui manque...
settings-library-acoustic-save = Enregistrer les descriptions
    .description = Où la passe met ce qu'elle trouve. La base seule laisse tes fichiers intacts ; les tags mettent aussi une copie dans chaque fichier, donc les descriptions sont conservées si la bibliothèque est reconstruite ou si le dossier part sur une autre machine, au prix d'une réécriture des fichiers audio. Les tags ne marchent que pour MP3 et FLAC ; tous les autres formats gardent la copie en base
settings-library-add-folder = Ajouter un dossier
settings-library-duplicates = Doublons...
settings-library-embed-button = Intégrer les métadonnées stockées...
settings-library-folder-col-albums = Albums
settings-library-folder-col-folder = Dossier
settings-library-folder-col-size = Taille
settings-library-folder-col-tracks = Pistes
settings-library-folders-intro = Dossiers analysés dans la bibliothèque ; en retirer un enlève ses pistes du catalogue et laisse les fichiers tranquilles
settings-library-genre-separator-nudge = Séparateurs modifiés : la navigation suit tout de suite. Les listes de genres stockées par les analyses précédentes gardent leur ancienne forme jusqu'à ce que tu lances Réanalyser dans l'en-tête Dossiers
settings-library-merge-case = Fusionner les variantes de casse
    .description = Traiter comme une seule les valeurs qui ne diffèrent que par la casse. Rock et rock deviennent le même genre, le même artiste et le même album, affichés dans la casse qu'utilise le plus de pistes. Les fichiers gardent leurs tags tels qu'ils sont écrits
settings-library-no-folders = Aucun dossier pour l'instant
settings-library-repair-tags = Réparer les tags...
settings-library-section-folders = Dossiers
settings-library-section-stored-metadata = Métadonnées stockées
settings-library-section-tempo = Analyse du tempo
settings-library-split-genres = Séparer les genres aux virgules et aux barres obliques
    .description = « Dubstep, Trap » et « Drum & Bass / Neurofunk » comptent chaque valeur comme un genre à part ; les points-virgules séparent toujours. Désactivé, les noms à barre restent entiers pour les tags où ils désignent un seul genre. Les fichiers gardent leurs tags tels qu'ils sont écrits
settings-library-tempo-auto = Chronométrer les nouveaux fichiers
    .description = Compter les temps dans ce que la surveillance ramène au fur et à mesure, une fois la synchro stabilisée, pour qu'une bibliothèque qui grandit garde ses tempos sans repasser par ici. Désactivé, les nouveaux fichiers attendent le bouton Analyser les manquants. L'activer propose d'abord de chronométrer ce qui manque déjà ; ensuite il ne voit plus que les fichiers qui viennent d'arriver
settings-library-tempo-enable = Déterminer le tempo des pistes
    .description = Compter les temps des pistes dont les tags ne le disent pas, pour que la bibliothèque puisse afficher et trier par tempo. Tout tourne sur cette machine, les chiffres vont dans la base de la bibliothèque, et tes fichiers restent intacts
settings-library-tempo-progress = Chronométrage { $done } sur { $total }
settings-library-tempo-progress-start = Recherche de ce qui manque...
settings-library-tempo-status-measured = { $total ->
    [one] La seule piste analysée a un tempo trouvé par rox
   *[other]
        Les { $total } pistes analysées ont toutes un tempo, dont { $measured ->
            [one] { $measured } trouvé par rox
           *[other] { $measured } trouvés par rox
        }
}
settings-library-tempo-status-tagged = { $total ->
    [one] La seule piste analysée a un tag de tempo
   *[other] Les { $total } pistes analysées ont toutes un tag de tempo
}
settings-library-watch-folders = Surveiller les dossiers
    .description = Intégrer les fichiers ajoutés, modifiés et supprimés dans la bibliothèque au fil de l'eau, sans réanalyse manuelle
settings-library-write-stored = Écrire ce qui est stocké dans les fichiers
    .description = Les trois réglages d'enregistrement ne valent que pour la prochaine écriture, donc tout ce qui a été enregistré avant qu'on en passe un sur Tags reste dans rox seul. Ceci écrit les paroles, les gains et les descriptions que rox détient déjà dans les fichiers eux-mêmes, pour qu'un autre lecteur qui lit le dossier les voie. Rien n'est recalculé

## Settings: MCP
settings-mcp-client-config = Config client
    .description = À coller dans la liste de serveurs d'un client MCP (Claude Code, Claude Desktop, ou un autre) pour qu'il interroge rox sur la bibliothèque, ce qui joue et le transport. rox doit tourner ; les outils passent par son socket de contrôle
settings-mcp-enable = Activer le serveur MCP
    .description = Répondre aux appels d'outils des clients MCP connectés. Le proxy vérifie ce réglage à chaque appel, donc tant qu'il est désactivé les clients sont refusés avec la raison ; la config ci-dessous se prépare dans les deux cas

## Settings: ML models
settings-mlmodels-checking = Vérification...
settings-mlmodels-choose-file = Choisir un fichier
settings-mlmodels-custom-description-empty = Pointe rox vers ton propre checkpoint PANNs CNN10, en safetensors. Il est lu sur place et nommé par son hash, donc un second checkpoint décrit la bibliothèque à part au lieu de réutiliser les coordonnées du premier
settings-mlmodels-download-failed = { $label } n'a pas pu être téléchargé : { $reason }
settings-mlmodels-downloading = Téléchargement de { $label } : { $done } sur { $total }
settings-mlmodels-stopping = Arrêt du téléchargement de { $label }...
settings-mlmodels-fallback-model = modèle
settings-mlmodels-fallback-the-model = Le modèle
settings-mlmodels-kind-custom = Personnalisé
settings-mlmodels-kind-recommended = Recommandé
settings-mlmodels-pass-stopped = La dernière passe s'est arrêtée : { $reason }
settings-mlmodels-weights-file = Fichier de poids

## Settings: playback
settings-playback-continuation-continue = Continuer
    .description = Poursuivre la liste d'où tu es parti, puis le reste de la bibliothèque derrière. Lance un album depuis le milieu d'une vue et la vue continue
settings-playback-continuation-off = Désactivé
    .description = Rien ne remplit la file ; la lecture s'arrête à sa fin
settings-playback-continuation-weighted = Pondéré
    .description = Piocher dans toute la bibliothèque, ce que tu n'as jamais joué en premier et ce que tu as écouté récemment en dernier
settings-playback-keep-playing = Continuer la lecture
    .description = Ce qui joue quand la file se vide. Ce que ça choisit est ajouté à la chronologie comme contexte ordinaire, donc visible et supprimable plutôt qu'un état caché. Avec l'ordre ci-dessus sur Proches, il continue de trouver des pistes qui sonnent comme celle en cours, quel que soit le choix ici
    .keywords = continuer remplir automatique file
settings-playback-play-order = Ordre de lecture
    .description = Comment les pistes déjà en file sont rangées quand l'aléatoire est actif. Le bouton aléatoire du transport l'active et le désactive ; ceci décide de ce qu'il fait une fois actif
settings-playback-rating-scale = Échelle de note
    .description = Des étoiles pour cliquer vite, 0-10 par demi-points pour des notes de critique plus fines
settings-playback-rating-scale-numeric = 0-10
settings-playback-rating-scale-stars = Étoiles
settings-playback-restore-last-session = Restaurer la dernière session
    .description = Démarrer avec la file de lecture telle que tu l'as laissée, en pause sur la piste qui jouait et là où elle s'est arrêtée. Les pistes en file hors de tes dossiers de bibliothèque ne peuvent pas être restaurées et sortent de l'ordre
settings-playback-section-queue = File
settings-playback-section-ratings = Notes
settings-playback-section-startup = Démarrage
settings-playback-shuffle-random = Au hasard
    .description = L'aléatoire au sens où tout le monde l'entend. Ce qui vient joue dans un ordre quelconque
settings-playback-shuffle-similar = Proches
    .description = Le plus proche d'abord, par le son. Ce qui vient est trié selon sa ressemblance avec la piste qui jouait quand tu l'as activé, et retrié à chaque saut. Demande que la bibliothèque soit décrite sur la page Bibliothèque
settings-playback-unrated-dots = Points sans note
    .description = Marquer les étoiles non remplies d'un point discret au lieu de les laisser vides

## Settings: providers
settings-providers-artist = Last.fm
    .description = Récupérer biographies, statistiques et artistes proches pour le panneau biographie, avec un portrait depuis Deezer ; tout est gardé dans le dossier de données et se lit hors ligne ensuite
settings-providers-deezer = Deezer
    .description = Chercher des pochettes sur Deezer, jusqu'à 1000 pixels
settings-providers-itunes = iTunes
    .description = Chercher des pochettes sur iTunes ; la recherche de l'éditeur de pochette montre les résultats à choisir avant de définir
settings-providers-lastfm-art = Last.fm
    .description = Chercher des pochettes sur Last.fm
settings-providers-lrclib = LRCLIB
    .description = Récupérer les paroles manquantes sur lrclib.net, synchronisées quand elles existent
settings-providers-lyrics-intro = Les recherches en ligne ne partent que lorsqu'une action de panneau en demande une ; la lecture et la navigation ne touchent jamais au réseau
settings-providers-musicbrainz = MusicBrainz
    .description = Chercher les tags sur musicbrainz.org ; la recherche du panneau métadonnées montre les résultats à confirmer champ par champ avant écriture
settings-providers-save-lyrics = Enregistrer les paroles récupérées
    .description = Où va une feuille récupérée : le dossier de données de rox, qui garde la bibliothèque propre, un .lrc à côté de la piste, ou le tag intégré
settings-providers-save-lyrics-data-folder = Dossier de données
settings-providers-save-lyrics-sidecar = Fichier voisin
settings-providers-save-lyrics-tag = Tag
settings-providers-section-artist = Artiste
settings-providers-section-cover-art = Pochettes
settings-providers-section-lyrics = Paroles
settings-providers-section-metadata = Métadonnées

## Settings: shader
settings-shader-backdrop-all-windows = Toutes les fenêtres
    .description = Ombrer le fond de chaque fenêtre : réglages, éditeurs, dialogues, panneaux détachés. Désactivé, il s'en tient aux fenêtres de l'espace de travail
settings-shader-backdrop-enabled = Shader de fond
    .description = Faire tourner un shader WGSL réactif à la musique sur le fond de pochette, sous chaque panneau. Il fait partie de l'espace de travail, donc il voyage avec l'habillage
settings-shader-backdrop-fallback-name = Fond
settings-shader-backdrop-run-idle = Continuer au repos
    .description = Continuer à dessiner quand rien ne joue. L'animation reste figée dans les deux cas
settings-shader-compile-error-title = Ce shader n'a pas compilé
settings-shader-legacy-note = Sans aucune route, les signaux remplissent les emplacements dans leur ordre d'ajout : le premier signal dans l'emplacement 0, le deuxième dans le 1, et ainsi de suite. La première route que tu ajoutes prend en charge toute la correspondance.
settings-shader-overlay-enabled = Shader de surcouche
    .description = Faire tourner un shader WGSL réactif à la musique sur toute la fenêtre. Seuls les shaders qui laissent l'application utilisable en dessous sont proposés
settings-shader-scene-covers-window = Ce shader est une scène, il couvre donc la fenêtre au lieu de dessiner par-dessus. Il vient d'un lot ou d'une ancienne config ; la liste ci-dessus ne propose que des shaders qui laissent l'application utilisable.
settings-shader-screen-all-windows = Toutes les fenêtres
    .description = Ombrer aussi les fenêtres enfants : réglages, statistiques, égaliseur, panneaux détachés. Le compte à rebours de retour arrière reste sans ombrage dans les deux cas
settings-shader-screen-fallback-name = Écran
settings-shader-screen-run-idle = Continuer au repos
    .description = Continuer à dessiner quand rien ne joue. L'animation reste figée dans les deux cas. Un shader qui lit la souris suit le curseur même quand la musique est à l'arrêt, sans ce réglage ; il s'arrête juste quelques secondes après le pointeur
settings-shader-section-backdrop = Shader de fond
settings-shader-section-overlay = Shader de surcouche
settings-shader-signals-block = Signaux
    .description = Le signal partagé que suit chacun des seize emplacements du shader
settings-shader-slots-block = Emplacements
    .description = Chaque emplacement tel que le shader le reçoit ; les emplacements sans route sont des boutons réglés à la main

## Settings: storage
settings-storage-artist-images = Images d'artistes
    .description = Portraits, bannières et biographies récupérés pour les vues artiste (artists/) ; ceux qu'on efface sont récupérés à nouveau à la prochaine ouverture d'une vue
settings-storage-catalog = Catalogue
    .description = L'index de pistes que construisent les analyses : une ligne par piste avec ses tags, les détails de son fichier et ses plages cue, dans library.db
settings-storage-cover-thumbnails = Vignettes de pochettes
    .description = Petites pochettes gardées après leur premier rendu (thumbs.db) ; celles qu'on efface se reconstruisent quand elles reviennent à l'écran
settings-storage-logs = Journaux
    .description = Ce que chaque session écrit pour les rapports de bug (logs/rox.log), avec rotation à une taille plafond pour qu'il ne grossisse jamais
settings-storage-looks-layouts = Habillages et dispositions
    .description = L'habillage qu'utilise l'application (workspace.json) avec tes espaces de travail enregistrés, les fichiers de shader extraits et les packs d'icônes à côté. Petit, et chaque octet est quelque chose que tu as réglé
settings-storage-lyrics = Paroles
    .description = Feuilles récupérées et modifiées gardées dans le magasin de l'application (lyrics/), pour que les dossiers de bibliothèque restent propres
settings-storage-measured-tempos = Tempos mesurés
    .description = Les tempos que rox a comptés depuis l'audio, pour les pistes dont les tags n'en ont pas ; les chiffres des tags ne sont pas touchés. Effacer remet ces pistes sur la liste d'Analyser les manquants dans la page Bibliothèque, pour qu'un meilleur comptage des temps puisse remplacer les chiffres écrits par une passe plus ancienne
settings-storage-model-fallback-this = Ce modèle
settings-storage-music-summary = { $tracks }, { $albums }, { $size }
settings-storage-model-weights = Poids des modèles
    .description = Les modèles téléchargés pour l'analyse acoustique (models/). La page Modèles ML est là où on les récupère et les supprime, une ligne par modèle
settings-storage-models-empty = Modèles
    .description = Rien n'a encore décrit la bibliothèque. Activer l'analyse acoustique sur la page Bibliothèque remplit cette section, et chaque modèle qui a tourné y a sa ligne
settings-storage-music-files = Fichiers de musique
    .description = Ce que contiennent les dossiers analysés ; les fichiers restent où ils sont
settings-storage-none = Aucun
settings-storage-playlists-history = Playlists et historique
    .description = Tes playlists et ce qu'elles contiennent, ce que tu as joué, et les notes de genre de la bibliothèque. Tout ça est petit à côté du reste de library.db
settings-storage-reclaimable = Espace récupérable
    .description = Des pages dans library.db que les suppressions ont laissées derrière elles. Les nouvelles écritures les remplissent, donc le fichier arrête de grossir avant de rétrécir
    .keywords = compacter reduire nettoyer base
settings-storage-section-acoustic = Descriptions acoustiques
settings-storage-section-app-data = Données de l'application
settings-storage-section-caches = Caches
settings-storage-section-diagnostics = Diagnostics
settings-storage-section-library = Bibliothèque
settings-storage-section-tempo = Tempo
settings-storage-vectors = Vecteurs
    .description = Ce que pèsent toutes les descriptions dans library.db. Sur une bibliothèque déjà passée par l'analyse, c'est l'essentiel du fichier, quelques kilo-octets par piste contre quelques centaines d'octets de tags
settings-storage-waveforms = Formes d'onde
    .description = La barre de crêtes de chaque piste, gardée après sa première écoute ; celles qu'on efface se redécodent à la lecture suivante

## Settings: workspace
settings-workspace-card-author = Auteur
settings-workspace-card-author-placeholder = Qui l'a fait
settings-workspace-card-created = Créé le { $date }
settings-workspace-card-created-updated = Créé le { $created }, mis à jour le { $updated }
settings-workspace-card-description = Description
settings-workspace-card-description-placeholder = Ce que vise l'habillage
settings-workspace-card-empty = Cet espace de travail n'a pas de fiche
settings-workspace-card-hint = La fiche est stockée dans le fichier, donc ceux avec qui tu partages cet habillage la voient
settings-workspace-card-license = Licence
settings-workspace-card-license-placeholder = Les conditions de partage
settings-workspace-card-save = Enregistrer la fiche
settings-workspace-card-updated = Mis à jour le { $date }
settings-workspace-card-version = Version
settings-workspace-card-version-placeholder = Ta propre version, comme tu comptes
settings-workspace-card-website = Site web
settings-workspace-card-website-placeholder = Où le trouver
settings-workspace-composition-closed = La fenêtre de l'espace de travail est fermée
settings-workspace-composition-hint = Les panneaux de la fenêtre tels qu'ils sont agencés dans les séparations et les groupes d'onglets ; les flèches réordonnent une ligne parmi ses voisines, le cadenas fixe un panneau en place, et l'engrenage ouvre ses réglages
settings-workspace-empty = Aucun espace de travail pour l'instant
settings-workspace-hint = Un espace de travail est un habillage entier : dispositions, palette, apparence. En appliquer un remplace les trois
settings-workspace-layout-name-placeholder = Nom de la disposition
settings-workspace-layouts-empty = Aucune disposition pour l'instant
settings-workspace-layouts-hint = Principale et mini sont les deux entre lesquelles bascule le bouton mini-lecteur de la barre de menus
settings-workspace-name-placeholder = Nom de l'espace de travail
settings-workspace-panel-preset-unknown-kind = Panneau inconnu
settings-workspace-panel-presets-empty = Aucun préréglage de panneau pour l'instant
settings-workspace-panel-presets-hint-after = dans n'importe quel menu de panneau. Ils ne valent que pour cet espace de travail ; un autre espace ne les aura pas.
settings-workspace-panel-presets-hint-before = Un panneau configuré chacun, enregistré depuis le menu d'un panneau et récupéré depuis
settings-workspace-role-mini = Mini
settings-workspace-role-primary = Principale
settings-workspace-section-composition = Composition
settings-workspace-section-layouts = Dispositions
settings-workspace-section-panel-presets = Préréglages de panneau
settings-workspace-section-workspaces = Espaces de travail
settings-workspace-tree-empty-slot = Emplacement vide
settings-workspace-tree-split-column = Séparation, empilée
settings-workspace-tree-split-row = Séparation, côte à côte
settings-workspace-tree-tabs = Onglets

## Settings: development
settings-development-experimental-panels = Panneaux expérimentaux
    .description = Afficher les panneaux encore en construction dans le menu Panneaux et le lanceur ; ils changent de forme d'une version à l'autre, et une disposition qui en contient déjà un le garde même une fois ceci désactivé
settings-development-section-features = Fonctionnalités

## Settings: shared
settings-acoustic-analysis-heading = Analyse acoustique
settings-analyze-nothing-scanned = Aucune piste analysée à traiter pour l'instant
settings-common-active = Actif
settings-common-analyze-missing = Analyser les manquants
settings-common-built-in = Intégré
settings-common-clear = Effacer
settings-common-copy = Copier
settings-common-database = Base de données
settings-common-delete = Supprimer
settings-common-download = Télécharger
settings-common-rescan = Réanalyser
settings-common-reveal = Afficher
settings-common-stop = Arrêter
settings-common-stopping = Arrêt en cours...
settings-common-tags = Tags
settings-common-tracks-count = { $count ->
    [one] { $count } piste
   *[other] { $count } pistes
}
settings-common-use = Utiliser
settings-confirm-apply-body = Ceci remplace tes dispositions, ta palette et ton apparence par celles de l'espace de travail.
settings-confirm-apply-imported-body = Il est enregistré dans tes espaces de travail. L'appliquer maintenant remplace tes dispositions, ta palette et ton apparence par les siennes.
settings-confirm-clear = Effacer
settings-confirm-clear-embeddings-body = Les descriptions partent et la place revient. Les retrouver veut dire relancer la passe d'analyse sur chaque piste de la bibliothèque.
settings-confirm-clear-embeddings-title = Effacer ce que « { $model } » a décrit ?
settings-confirm-clear-measured-bpm-body = Chaque tempo trouvé par rox repasse à non mesuré ; les chiffres des tags de tes fichiers restent. Les retrouver veut dire relancer la passe de tempo sur chacune de ces pistes.
settings-confirm-clear-measured-bpm-title = Effacer les tempos mesurés ?
settings-confirm-overwrite-workspace-body = Ceci remplace l'espace de travail enregistré par l'état actuel.
settings-confirm-overwrite-workspace-title = Écraser l'espace de travail « { $name } » ?
settings-sidebar-data-folder = Dossier de données
settings-sidebar-settings-file = Fichier de réglages

## Menubar
menu-about = À propos
menu-application = Application
menu-apply-layout = Appliquer la disposition
menu-apply-workspace = Appliquer l'espace de travail
menu-chat = Chat
menu-close = Fermer
menu-console = Console
menu-design-mode = Mode conception
menu-discussions = Discussions
menu-empty-window = Fenêtre vide
menu-equalizer = Égaliseur
menu-exit = Quitter
menu-hide-menubar = Masquer la barre de menus
menu-import-workspace = Importer un espace de travail...
menu-new-ellipsis = Nouveau...
menu-new-window = Nouvelle fenêtre
menu-new-window-from-layout = Nouvelle fenêtre depuis une disposition
menu-new-window-from-panel = Nouvelle fenêtre depuis un panneau
menu-no-layouts = Aucune disposition
menu-no-presets = Aucun préréglage
menu-no-workspaces = Aucun espace de travail
menu-os-decorations = Décorations système
menu-overlay-shader = Shader de surcouche
menu-panel-built-in = Intégré
menu-panel-new = Nouveau...
menu-panel-no-layouts = Aucune disposition
menu-panel-no-presets = Aucun préréglage
menu-panel-no-workspaces = Aucun espace de travail
menu-panel-title = Menu
menu-panels = Panneaux
menu-panels-presets = Préréglages
menu-pause = Pause
menu-playback = Lecture
menu-remain-in-tray = Rester dans la zone de notification
menu-report-issue = Signaler un problème
menu-save-layout = Enregistrer la disposition
menu-save-workspace = Enregistrer l'espace de travail
menu-section-add = Ajouter
menu-section-app = App
menu-section-interface = Interface
menu-section-layouts = Dispositions
menu-section-library = Bibliothèque
menu-section-session = Session
menu-section-track = Piste
menu-section-tuning = Réglage
menu-settings = Réglages
menu-signals = Signaux
menu-song-theming = Couleurs du morceau
menu-stats = Statistiques
menu-tasks = Tâches
menu-welcome = Bienvenue
menu-window = Fenêtre
menu-workspace = Espace de travail
menu-workspace-builtin-tag = Intégré

## Workspaces
workspace-apply-body = Ceci remplace tout l'habillage : dispositions, palette, apparence.
workspace-apply-imported-body = Il est enregistré dans tes espaces de travail. L'appliquer maintenant remplace tout l'habillage : dispositions, palette, apparence.
workspace-apply-imported-title = « { $name } » importé
workspace-apply-screen-shader-named = Applique le shader de surcouche { $name } sur toute la fenêtre.
workspace-apply-screen-shader-plain = Applique un shader de surcouche sur toute la fenêtre.
workspace-apply-shader-count = { $count ->
    [one] Contient { $count } shader : { $names }
   *[other] Contient { $count } shaders : { $names }
}
workspace-apply-shaders-approve-body = Les approuver les laisse tourner sur cette machine. L'appliquer sans eux laisse l'habillage nu, avec les shaders toujours dans l'espace de travail.
workspace-apply-shaders-plain-body = L'appliquer sans eux laisse l'habillage nu, avec les shaders toujours dans l'espace de travail.
workspace-byline-author = par { $author }
workspace-byline-version = version { $version }
workspace-context-add-panel = Ajouter un panneau
workspace-dialog-apply = Appliquer
workspace-dialog-apply-title = Appliquer « { $name } » ?
workspace-dialog-approve-apply = Approuver et appliquer
workspace-dialog-cancel = Annuler
workspace-dialog-close = Fermer
workspace-dialog-close-title = Fermer « { $name } » ?
workspace-dialog-export = Exporter
workspace-dialog-layout-name-placeholder = Nom de la disposition
workspace-dialog-not-now = Pas maintenant
workspace-dialog-overwrite = Écraser
workspace-dialog-overwrite-title = Écraser « { $name } » ?
workspace-dialog-save = Enregistrer
workspace-dialog-save-layout-title = Enregistrer la disposition
workspace-dialog-save-workspace-title = Enregistrer l'espace de travail
workspace-dialog-with-shaders = Avec les shaders
workspace-dialog-without-shaders = Sans les shaders
workspace-dialog-workspace-name-placeholder = Nom de l'espace de travail
workspace-drop-add-queue = Ajouter à la file
workspace-drop-play-now = Lire maintenant
workspace-hint-or = ou
workspace-hint-then = puis
workspace-import = Importer
workspace-launcher-hint = Ajoute ton premier panneau pour commencer, ou choisis un préréglage sous Espace de travail > Appliquer l'espace de travail
workspace-launcher-need-help = Besoin d'aide ?
workspace-launcher-open-welcome = Ouvrir la fenêtre de bienvenue
workspace-launcher-title = Une fenêtre vide
workspace-layout-apply-body = Ceci remplace la disposition actuelle de cette fenêtre.
workspace-layout-overwrite-body = Ceci remplace la disposition enregistrée par l'actuelle.
workspace-layout-preset-restore-failed = Le préréglage de disposition de cette fenêtre n'a pas pu être restauré, donc elle démarre vide.
workspace-layout-restore-failed = La disposition enregistrée n'a pas pu être restaurée, donc cette fenêtre démarre vide.
workspace-mini-tip-back = Retour à la disposition complète
workspace-mini-tip-shrink = Réduire au mini-lecteur
workspace-overwrite-body = Ceci remplace l'espace de travail enregistré par l'habillage actuel.
workspace-panel-locked-close-body = Ce panneau est fixé en place. Le fermer le sort de la disposition.
workspace-save-current = Enregistrer l'actuel
workspace-screen-shader-hint-before = Désactive-le à tout moment avec
workspace-workspace-restore-failed = La disposition de l'espace de travail n'a pas pu être restaurée, donc cette fenêtre démarre vide.

## Tasks window
tasks-acoustic-all-described = { $count ->
    [one] La seule piste analysée est décrite par { $label }
   *[other] Les { $count } pistes analysées sont toutes décrites par { $label }
}
tasks-acoustic-off = La description du son des pistes est désactivée dans Réglages, sous Bibliothèque
tasks-acoustic-partial = { $embedded ->
    [one] { $label } décrit { $embedded } piste analysée sur { $total }
   *[other] { $label } décrit { $embedded } pistes analysées sur { $total }
}
tasks-analyzing = Analyse { $progress }
tasks-bake-writing = Écriture des tags...
tasks-chip-count = { $count } tâches
tasks-convert-starting = Démarrage de ffmpeg...
tasks-converting = Conversion { $progress }
tasks-count-of-total = { $done } sur { $total }
tasks-embedding = Intégration { $progress }
tasks-estimate-at = { $estimate } avec { $workers }
tasks-import-failed = Le dernier import a échoué : { $error }
tasks-import-reading = Lecture de la liste des morceaux aimés...
tasks-import-unmatched = { $count } sans correspondance dans cette bibliothèque
tasks-importing = Import { $progress }
tasks-job-acoustic = Analyse acoustique
tasks-job-convert = Convertir l'audio
tasks-job-loved-import = Morceaux aimés Last.fm
tasks-job-replaygain = ReplayGain
tasks-job-scan = Analyse de la bibliothèque
tasks-job-tempo = Analyse du tempo
tasks-last-pass-stopped = La dernière passe s'est arrêtée : { $reason }
tasks-last-run-finished = { $count ->
    [one] Dernière exécution terminée, { $count } traité
   *[other] Dernière exécution terminée, { $count } traités
}
tasks-last-run-stopped = Dernière exécution arrêtée après { $count }
tasks-library-busy = La bibliothèque est occupée
tasks-library-scanning = La bibliothèque est en cours d'analyse
tasks-measuring = Mesure { $progress }
tasks-model-downloading = Un modèle est encore en téléchargement
tasks-no-library-window = Aucune fenêtre de bibliothèque n'est ouverte, donc on ne peut pas les lancer d'ici
tasks-nothing-to-measure = Aucune piste analysée à mesurer pour l'instant
tasks-rg-all-gain = { $count ->
    [one] La seule piste a un gain de lecture
   *[other] Les { $count } pistes ont toutes un gain de lecture
}
tasks-rg-partial = { $missing ->
    [one] { $missing } piste sur { $total } n'a pas de gain
   *[other] { $missing } pistes sur { $total } n'ont pas de gain
}
tasks-scan-folder-count = { $count ->
    [one] { $count } dossier
   *[other] { $count } dossiers
}
tasks-scan-last-scanned = { $folders }, dernière analyse il y a { $ago }
tasks-scan-never-scanned = { $folders }, jamais analysés
tasks-scan-no-folders = Aucun dossier ajouté pour l'instant. Ajoutes-en un dans Réglages, sous Bibliothèque
tasks-start-analyze-missing = Analyser les manquants
tasks-start-measure-missing = Mesurer les manquants
tasks-start-rescan = Réanalyser
tasks-stop = Arrêter
tasks-stopping = Arrêt en cours...
tasks-tempo-all = { $count ->
    [one] La seule piste a un tempo
   *[other] Les { $count } pistes ont toutes un tempo
}
tasks-tempo-off = Le calcul de la vitesse des pistes est désactivé dans les Réglages, sous Bibliothèque
tasks-tempo-partial = { $missing ->
    [one] { $missing } piste sur { $total } n'a pas de tempo
   *[other] { $missing } pistes sur { $total } n'ont pas de tempo
}
tasks-timing = Chronométrage { $progress }
tasks-tip = Ouvrir les tâches de la bibliothèque
tasks-window-title = rox - Tâches
tasks-working-out-missing = Recherche de ce qui manque...

## Stats window
stats-bucket-listens = { $count ->
    [one] { $count } écoute, { $ago }
   *[other] { $count } écoutes, { $ago }
}
stats-chart-start-all = Première écoute
stats-chart-start-month = Il y a 30 jours
stats-chart-start-week = Il y a 7 jours
stats-chart-start-year = Il y a un an
stats-click-opens = Le clic ouvre les stats
stats-click-section = Clic
stats-count-menu = Nombre
    .description = Sur quelle fenêtre glissante le nombre compte les écoutes ; la liste au survol les affiche toutes
stats-empty-all = Aucune écoute pour l'instant
stats-empty-range = Aucune écoute sur cette période
stats-now = Maintenant
stats-open = Ouvrir les stats
stats-open-on-click = Ouvrir les stats au clic
    .description = Cliquer sur le widget pour ouvrir la fenêtre des stats, le relevé d'écoute complet
stats-play-these-tracks = Lire ces pistes
stats-play-this-track = Lire cette piste
stats-plays-count = { $count ->
    [one] { $count } écoute
   *[other] { $count } écoutes
}
stats-range-all = Depuis le début
stats-range-all-short = Tout
stats-range-day-short = Jour
stats-range-label = Période
stats-range-month = Ce mois-ci
stats-range-month-short = Mois
stats-range-today = Aujourd'hui
stats-range-week = Cette semaine
stats-range-week-short = Semaine
stats-range-year = Cette année
stats-range-year-short = Année
stats-readout-section = Affichage
stats-section-listens = Écoutes
stats-section-listens-over-time = Écoutes dans le temps
stats-section-recent-listens = Écoutes récentes
stats-section-top-albums = Top albums
stats-section-top-artists = Top artistes
stats-section-top-genres = Top genres
stats-show-change = Afficher l'évolution
    .description = Ajouter une pastille pour dire comment la période se compare à la précédente, en hausse ou en baisse ; Depuis le début n'a rien à quoi se comparer
stats-show-number = Afficher le nombre
    .description = Dessiner le compte à côté de l'icône ; désactivé, il ne reste que l'icône, avec les nombres au survol
stats-title = Widget Stats
stats-tooltip-listens = Écoutes
stats-window-title = rox - Statistiques

## About window
about-check-failed = GitHub est injoignable
about-check-for-updates = Vérifier les mises à jour
about-checking = Vérification...
about-download = Télécharger
about-downloading = Téléchargement... { $percent } %
about-get-it = Récupérer
about-license-lead = rox est un logiciel libre sous GNU AGPLv3. Les sources sont sur
about-notice-lead = Tu devrais avoir reçu une copie de la licence avec ce programme. Sinon, voir
about-release-notes = Notes de version
about-restart-now = Redémarrer maintenant
about-up-to-date = Tu as la dernière version
about-update-failed = La mise à jour a échoué : { $error }
about-version = Version { $version }
about-version-available = La version { $version } est disponible
about-version-ready = La version { $version } est prête
about-window-title = rox - À propos

## Welcome window
welcome-add-folder = Ajouter un dossier
welcome-and = et
welcome-back = Retour
welcome-card-menubar-title = Barre de menus
welcome-card-music-title = Musique
welcome-card-panels-title = Panneaux
welcome-card-playback-title = Lecture
welcome-card-rearranging-title = Réagencement
welcome-card-settings-title = Réglages
welcome-close = Fermer
welcome-design-mode-note = Le réagencement demande le mode conception, actif par défaut en haut de ce menu. Désactivé, il verrouille la disposition, pour qu'un agencement terminé ne bouge plus.
welcome-done = Terminé
welcome-drop-note = Dépose-le sur le bord d'un panneau pour diviser à cet endroit, au milieu pour partager un groupe d'onglets, ou hors de la fenêtre pour en faire sa propre fenêtre.
welcome-key-left-click = Clic gauche
welcome-key-middle-mouse = Clic milieu
welcome-layout-note = Enregistre un agencement comme disposition ; un espace de travail réunit dispositions et palette en un habillage partageable.
welcome-menubar-after = deux fois pour la garder affichée.
welcome-menubar-before = Barre de menus masquée, maintiens
welcome-menubar-mid = pour la faire flotter au-dessus du dock, ou appuie
welcome-music-note = rox l'analyse dans la bibliothèque et les fichiers restent où ils sont. D'autres dossiers s'ajoutent dans les réglages, sous bibliothèque.
welcome-next = Suivant
welcome-or = ou
welcome-panels-note = Chaque surface est un panneau, et le menu Panneaux de la barre de menus en ouvre d'autres.
welcome-playback-after = pour avancer ou reculer.
welcome-playback-before = bascule la lecture ;
welcome-quickplay-after = et ça joue.
welcome-quickplay-before = ouvre la lecture rapide : tape une piste, appuie sur
welcome-rearrange-after = n'importe où dans un panneau pour le déplacer.
welcome-rearrange-before = Fais glisser un onglet, ou maintiens
welcome-settings-hint-after = ouvre les réglages : la palette, la transparence et le comportement.
welcome-shelf-caption = En choisir un remplace l'habillage de la fenêtre principale et ferme la visite. Cette fenêtre reste là à tout moment sous Application > Bienvenue.
welcome-stage-lead-quick-start = Choisis un espace de travail et la fenêtre principale bascule dessus : dispositions, palette, tout l'habillage.
welcome-stage-lead-welcome = Foobar s'il avait été fait en 20XX.
welcome-stage-title-quick-start = Démarrage rapide
welcome-stage-title-welcome = Bienvenue dans rox
welcome-step-hint-after = , ou les boutons ci-dessous.
welcome-step-hint-before = Avance d'étape en étape avec
welcome-tile-by = par { $author }
welcome-tour-intro = Un tour rapide de là où la musique entre et là où se règle l'habillage. Ça se termine sur l'étagère des espaces de travail livrés, un clic chacun.
welcome-window-title = rox - Bienvenue

## Console window
console-clear = Effacer
console-copy = Copier
console-empty-filtered = Rien à ces niveaux
console-empty-none = Rien de consigné pour l'instant
console-filter-error = Erreur
console-filter-info = Info
console-filter-warn = Alerte
console-follow = Suivre
console-line-count = { $count ->
    [one] { $count } ligne
   *[other] { $count } lignes
}
console-open-button = Ouvrir la console
console-reveal = Afficher
console-window-title = rox - Console

## Signals window
signals-about-toggle = À propos des signaux
signals-blurb-marked = Les panneaux marqués de ceci dans les menus peuvent lier la plupart de leurs paramètres : fais un clic droit sur un paramètre dans les réglages du panneau et choisis un signal, ou ajoutes-en un depuis là.
signals-blurb-shared = Ce qui se règle ici est partagé : un changement s'applique à chaque paramètre routé vers ce signal, dans chaque panneau et chaque fenêtre.
signals-blurb-total = Un Total est le quatrième genre : il cumule un autre signal dans le temps et boucle à 1, donc il monte tant que la musique est forte et cale quand elle ne l'est pas. À utiliser quand un shader a besoin d'une phase qui suit le morceau plutôt que l'horloge.
signals-blurb-what = Un signal transforme ce qui joue en un seul nombre entre 0 et 1 : l'énergie dans une bande de fréquences, le niveau du mixage entier, ou une impulsion à chaque frappe dans une bande. Réponse règle la vitesse à laquelle il suit, Seuil le fait taire sous un niveau que tu choisis.
signals-no-library = Aucune fenêtre de bibliothèque n'est ouverte, donc ceux-ci n'affichent aucun audio. Les modifications s'enregistrent quand même.
signals-window-title = rox - Signaux

## Equaliser
eq-analyzer-bars = Barres
eq-analyzer-off = Pas d'analyseur
eq-analyzer-wave = Onde
eq-band-badge = Pastille de bandes
    .description = Afficher le nombre de bandes écartées du plat, sur une pastille par-dessus l'icône
eq-band-label = Bande { $number }
eq-click-nothing = Rien
eq-click-open = Ouvrir
eq-click-section = Clic
    .description = Ce que fait un clic : ouvrir la fenêtre de l'égaliseur, ou activer et désactiver toute la courbe sur place
eq-click-toggle = Basculer
eq-flatten = Aplatir
eq-freq-label = Fréq
eq-gain-label = Gain
eq-heading = Égaliseur
eq-help-text = Fais glisser une bande pour la déplacer, la molette dessus pour l'élargir ou la resserrer. Le traitement se fait en amont du tampon qui alimente la carte son, donc un geste met jusqu'à une demi-seconde à atteindre les enceintes.
eq-hint-off = Clique pour le désactiver
eq-hint-on = Clique pour l'activer
eq-hint-open = Clique pour ouvrir l'égaliseur
eq-open = Ouvrir l'égaliseur
eq-readout-curve = Courbe
eq-readout-icon = Icône
eq-readout-section = Affichage
    .description = L'icône, la courbe de réponse en sparkline, ou les deux. La courbe a besoin d'une cinquantaine de pixels de large pour être lisible
eq-reset-bands = Réinitialiser les bandes
eq-shape-active = { $count ->
    [one] { $count } bande écartée du plat, pic { $peak } dB
   *[other] { $count } bandes écartées du plat, pic { $peak } dB
}
eq-shape-flat = À plat, toutes les bandes à 0 dB
eq-status-off = Égaliseur désactivé
eq-status-on = Égaliseur activé
eq-title = Widget EQ
eq-widget-section = Widget
eq-width-label = Largeur
eq-window-title = rox - Égaliseur

## Keymap
keymap-close-window = Fermer la fenêtre
    .description = Fermer la fenêtre qui est devant. Assigné partout, panneaux détachés compris
keymap-decrease-font-size = Réduire la taille du texte
    .description = Baisser d'un cran la taille du texte de toute l'appli
keymap-focus-search = Aller à la recherche
    .description = Mettre le curseur dans le champ de recherche de la bibliothèque
keymap-group-editing = Édition
keymap-group-playback = Lecture
keymap-group-view = Vue
keymap-group-windows = Fenêtres
keymap-increase-font-size = Agrandir la taille du texte
    .description = Monter d'un cran la taille du texte de toute l'appli
keymap-key-backspace = Retour arrière
keymap-key-delete = Suppr
keymap-key-down = Bas
keymap-key-end = Fin
keymap-key-esc = Échap
keymap-key-home = Début
keymap-key-insert = Inser
keymap-key-left = Gauche
keymap-key-page-down = Page suiv.
keymap-key-page-up = Page préc.
keymap-key-right = Droite
keymap-key-space = Espace
keymap-key-tab = Tab
keymap-key-up = Haut
keymap-mod-alt = Alt
keymap-mod-cmd = Cmd
keymap-mod-ctrl = Ctrl
keymap-mod-fn = Fn
keymap-mod-option = Option
keymap-mod-shift = Maj
keymap-mod-super = Super
keymap-mod-win = Win
keymap-open-quick-play = Lecture rapide
    .description = Afficher l'invite de lecture rapide par-dessus la fenêtre
keymap-open-settings = Ouvrir les réglages
    .description = Ouvrir cette fenêtre
keymap-open-stats = Ouvrir les statistiques
    .description = Ouvrir la fenêtre des statistiques d'écoute
keymap-quit = Quitter
    .description = Quitter rox. Assigné partout, puisqu'il n'y a aucune fenêtre d'où ça ne devrait pas marcher
keymap-reset-font-size = Réinitialiser la taille du texte
    .description = Ramener d'un coup la taille du texte à celle d'origine
keymap-seek-backward = Reculer
    .description = Reculer d'un cran dans la piste en cours
keymap-seek-forward = Avancer
    .description = Avancer d'un cran dans la piste en cours
keymap-stamp-line = Horodater la ligne de paroles
    .description = Écrire la position de lecture sur la ligne de paroles en cours d'édition
keymap-toggle-playback = Lecture / Pause
    .description = Lancer la piste en cours, ou la mettre en pause là où elle est
keymap-toggle-post-shader = Basculer le shader de surcouche
    .description = Éteindre et rallumer le shader d'écran. Assigné partout, puisqu'un shader peut recouvrir les contrôles qui serviraient autrement à l'éteindre
keymap-toggle-zoom = Zoomer le groupe de panneaux
    .description = Remplir le dock avec le dernier groupe de panneaux cliqué, ou en ressortir

## Panel catalog
panel-catalog-album-carousel = Carrousel d'albums
panel-catalog-artist-grid = Grille d'artistes
panel-catalog-biography = Biographie
panel-catalog-cover-art = Pochette
panel-catalog-drawer = Tiroir
panel-catalog-eq-widget = Widget EQ
panel-catalog-filter = Filtre
panel-catalog-folder-tree = Arborescence de dossiers
panel-catalog-genre-grid = Grille de genres
panel-catalog-group-application = Application
panel-catalog-group-arrangement = Agencement
panel-catalog-group-catalogue = Catalogue
panel-catalog-group-controls = Contrôles
panel-catalog-group-details = Détails
panel-catalog-group-experimental = Expérimental
panel-catalog-group-visualizers = Visualiseurs
panel-catalog-history = Historique
panel-catalog-menu = Menu
panel-catalog-metadata = Métadonnées
panel-catalog-mini-toggle = Bascule mini
panel-catalog-oscilloscope = Oscilloscope
panel-catalog-overlay = Surcouche
panel-catalog-particles = Particules
panel-catalog-playlists = Playlists
panel-catalog-queue = File d'attente
panel-catalog-queue-widget = Widget de file
panel-catalog-seek = Progression
panel-catalog-slide = Diaporama
panel-catalog-spectrogram = Spectrogramme
panel-catalog-spectrum = Spectre
panel-catalog-stats-widget = Widget Stats
panel-catalog-status = État
panel-catalog-theme-toggle = Bascule de thème
panel-catalog-track-info = Infos de piste
panel-catalog-vu-meter = Vumètre
panel-catalog-waveform = Forme d'onde
panel-catalog-window-controls = Contrôles de fenêtre

## Updater
updater-already-latest = déjà sur la dernière version
updater-checksum-mismatch = la somme de contrôle du téléchargement est { $digest }, pas { $expected } comme l'annonce la version
updater-checksum-missing-entry = { $sums } n'a aucune entrée pour { $name } ; téléchargement invérifiable refusé
updater-no-asset = la version n'a pas de { $name }
updater-no-checksums = la version n'a pas de { $sums } ; téléchargement invérifiable refusé
updater-no-release-build = pas de version compilée pour cette plateforme
updater-overran = le téléchargement a dépassé la taille annoncée par la version
updater-short = le téléchargement s'est arrêté à { $done } sur { $bytes } octets
updater-size-mismatch = le serveur a proposé { $claimed } octets, la version en annonce { $bytes }

## Last.fm
lastfm-import-matching = Rapprochement avec la bibliothèque
lastfm-import-read = { $count ->
    [one] { $count } piste aimée lue
   *[other] { $count } pistes aimées lues
}
lastfm-import-stopped = { $count ->
    [one] Arrêt après { $count } piste aimée
   *[other] Arrêt après { $count } pistes aimées
}
lastfm-import-matched = , { $count } avec correspondance
lastfm-import-added = { $count ->
    [one] , { $count } ajoutée aux favoris
   *[other] , { $count } ajoutées aux favoris
}

## Tag tools
tags-editor-clear-all = tout effacer
tags-editor-form-view = Formulaire
tags-editor-format-unsupported-all = Les tags de ce format ne peuvent pas encore être lus ni écrits.
tags-editor-format-unsupported-some = Certains de ces fichiers sont dans un format dont les tags ne peuvent pas encore être lus ni écrits.
tags-editor-guess-button = Deviner
tags-editor-guess-folded = { $count ->
    [one] { $status }, { $count } de plus non affiché
   *[other] { $status }, { $count } de plus non affichés
}
tags-editor-guess-help = { $placeholders } ; / correspond au dossier au-dessus, %skip% écarte
tags-editor-guess-match-count = { $hits ->
    [one] { $hits } sur { $total } correspond
   *[other] { $hits } sur { $total } correspondent
}
tags-editor-guess-no-match = aucune correspondance
tags-editor-guess-pattern-label = motif
tags-editor-loading = Chargement des tags...
tags-editor-look-up = Rechercher
tags-editor-multiple-values = Valeurs multiples
tags-editor-clear-on-save = Effacé à l'enregistrement
tags-editor-other-tags = Autres tags ({ $count })
tags-editor-remove = retirer
tags-editor-reveal = Afficher
tags-editor-save-errors = { $count ->
    [one] { $count } fichier a échoué ; { $error }
   *[other] { $count } fichiers ont échoué ; { $error }
}
tags-editor-saving-progress = Enregistrement { $done }/{ $total }...
tags-editor-table-view = Tableau
tags-editor-tags-section = Tags
tags-editor-unknown-partial = { $count } sur { $total }
tags-editor-unread-count = { $failed ->
    [one] Les tags de { $failed } fichier sur { $total } n'ont pas pu être lus
   *[other] Les tags de { $failed } fichiers sur { $total } n'ont pas pu être lus
}
tags-editor-will-clear = sera effacé
tags-editor-will-remove = sera retiré
tags-editor-window-title = rox - Éditeur de tags
tags-guess-empty-segment = le motif produit un nom de dossier ou de fichier vide
tags-guess-no-placeholders = aucune variable
tags-guess-skip-renders-nothing = %skip% n'a rien à produire
tags-guess-unclosed = % non fermé
tags-guess-unknown-placeholder = variable inconnue %{ $name }%
tags-matcher-blocked-arm = Active un champ pour appliquer
tags-matcher-blocked-no-match = Aucune correspondance à appliquer
tags-matcher-blocked-pick = Choisis une correspondance
tags-matcher-blocked-writing = Écriture des tags...
tags-matcher-match-count = { $count ->
    [one] 1 correspondance
   *[other] { $count } correspondances
}
tags-matcher-no-matches = Aucune correspondance trouvée
tags-matcher-pick-match = Choisis une correspondance
tags-matcher-search-failed = Recherche échouée : { $error }
tags-matcher-searching = Recherche...
tags-matcher-tagging = Écriture des tags de { $track }
tags-matcher-window-title = rox - Trouver des métadonnées
tags-rename-blocked-cue = piste cue, pas de fichier à elle
tags-rename-blocked-duplicate = deux pistes tombent sur ce nom
tags-rename-blocked-occupied = un fichier est déjà là
tags-rename-blocked-outside-roots = hors de toutes les racines de la bibliothèque
tags-rename-blocked-unresolved = pas encore au catalogue
tags-rename-move-error = { $name } : { $error }
tags-rename-move-errors = { $count ->
    [one] { $count } fichier a échoué ; { $error }
   *[other] { $count } fichiers ont échoué ; { $error }
}
tags-rename-moving = Déplacement { $done }/{ $total }...
tags-rename-nothing-to-move = Rien à déplacer
tags-rename-pattern-help = { $placeholders } ; / crée un dossier, l'extension suit le fichier
tags-rename-pattern-section = Motif
tags-rename-preview-section = Aperçu
tags-rename-unchanged = inchangé
tags-rename-will-move = { $count } sur { $total } à déplacer
tags-rename-window-title = rox - Renommer les fichiers
tags-repair-affected-files = Fichiers concernés
tags-repair-section = Réparation
tags-repair-check-to-repair = Coche un fichier pour le réparer
tags-repair-count = { $count ->
    [one] 1 fichier
   *[other] { $count } fichiers
}
tags-repair-count-so-far = { $count } jusqu'ici
tags-repair-label-scope = portée
tags-repair-no-affected = Aucun fichier concerné trouvé.
tags-repair-no-folder = Aucun dossier à analyser ; ajoutes-en un à la bibliothèque ou choisis-en un.
tags-repair-pick-folder = Choisir un dossier...
tags-repair-progress = Réparation { $done }/{ $total }...
tags-repair-repair-button = { $count ->
    [0] Réparer
   *[other] Réparer ({ $count })
}
tags-repair-result = { $count ->
    [one] 1 fichier réparé
   *[other] { $count } fichiers réparés
}
tags-repair-result-failed = { $count ->
    [one] { $count } réparé, { $failed } en échec
   *[other] { $count } réparés, { $failed } en échec
}
tags-repair-scan-first = Analyser d'abord
tags-repair-scan-hint = Analyse pour trouver les fichiers aux tags abîmés qu'une réécriture répare.
tags-repair-select-all = Tout sélectionner
tags-repair-select-none = Tout désélectionner
tags-repair-whole-library = Toute la bibliothèque
tags-repair-window-title = rox - Réparation des tags

## Convert
convert-arg-names-file = « { $token } » nomme un fichier ; la destination vient du dossier et du motif
convert-section-output = Sortie
convert-section-preview = Aperçu
convert-arg-not-flag-or-value = « { $token } » n'est ni une option ni la valeur d'une option
convert-check-wrote-nothing = ffmpeg s'est terminé proprement mais n'a rien écrit
convert-custom-ext-empty = L'extension choisit le conteneur, il en faut donc une
convert-custom-ext-invalid = « { $ext } » n'est pas un nom de conteneur ; lettres et chiffres, pas de point
convert-dialog-browse = Parcourir...
convert-dialog-check-passed = ffmpeg a encodé un instant de silence avec ces arguments, donc ils tournent
convert-dialog-check-waiting = Vérifié auprès de ffmpeg dès que tu arrêtes de taper
convert-dialog-checking = Vérification auprès de ffmpeg...
convert-dialog-choose-folder = Choisir un dossier où écrire
convert-dialog-convert-button = Convertir
convert-dialog-custom-label = Personnalisé
convert-dialog-custom-menu-item = Personnalisé...
convert-dialog-custom-note = Les arguments se coupent aux espaces, donc pas de guillemets ; les pochettes intégrées ne sont pas copiées pour les formats personnalisés
convert-dialog-format-not-ready = Le format saisi n'est pas encore passé par ffmpeg
convert-dialog-label-extension = extension
convert-dialog-label-format = format
convert-dialog-label-into = dans
convert-dialog-label-named = nommé
convert-dialog-mirror = Refléter les dossiers de la bibliothèque
convert-dialog-nothing-to-convert = Rien à convertir : chaque ligne est ignorée
convert-dialog-pattern-help = { $placeholders } ; / crée un dossier, le format fixe l'extension
convert-dialog-pick-folder = Choisir un dossier où écrire
convert-dialog-span-note = { $count ->
    [one] { $count } extrait d'une image cue et tagué depuis la bibliothèque
   *[other] { $count } extraits d'une image cue et tagués depuis la bibliothèque
}
convert-dialog-will-convert = { $count } sur { $total } à convertir
convert-dialog-window-title = rox - Convertir
convert-ffmpeg-silent-failure = ffmpeg a échoué sans dire pourquoi
convert-flag-attach = -attach lit un fichier à lui, ce qui n'est pas permis ici
convert-flag-f = L'extension choisit le conteneur, donc -f ne se règle pas ici
convert-flag-i = L'entrée est la piste que tu as choisie, donc -i ne se règle pas ici
convert-flag-n = -n est déjà appliqué à chaque exécution
convert-flag-y = Rien ici n'écrase, donc -y n'est pas disponible ; une destination qui existe est ignorée
convert-preset-flac = FLAC
convert-preset-mp3-320 = MP3 320 kbps
convert-preset-mp3-v0 = MP3 V0
convert-preset-opus-192 = Opus 192 kbps
convert-preset-wav = WAV
convert-skip-duplicate = deux pistes tombent sur ce nom
convert-skip-exists = déjà là
convert-summary-failed = , { $count } en échec
convert-summary-files = { $count ->
    [one] { $count } fichier
   *[other] { $count } fichiers
}
convert-summary-line = { $files } vers { $dest }
convert-summary-skipped = { $count ->
    [one] , { $count } ignoré
   *[other] , { $count } ignorés
}
convert-summary-stopped = Arrêt après { $files } vers { $dest }
convert-version-answered = { $binary } s'est lancé, sans annoncer de version

## Duplicates
duplicates-auto-select = Sélection auto
duplicates-check-to-trash = Coche les copies pour les mettre à la corbeille
duplicates-copy-count = { $count ->
    [one] 2 copies
   *[other] { $count } copies
}
duplicates-different-albums = albums différents
duplicates-filter-placeholder = Filtrer par titre, artiste ou dossier
duplicates-groups-summary = { $groups ->
    [one]
        { $groups } groupe, { $extras ->
            [one] { $extras } copie en trop
           *[other] { $extras } copies en trop
        }
   *[other]
        { $groups } groupes, { $extras ->
            [one] { $extras } copie en trop
           *[other] { $extras } copies en trop
        }
}
duplicates-library-loading = La bibliothèque charge encore ; réessaie dans un instant.
duplicates-no-duplicates = Aucun doublon trouvé.
duplicates-no-filter-matches = Aucun groupe ne correspond au filtre.
duplicates-policy-newest = Garder le plus récent
duplicates-policy-oldest = Garder le plus ancien
duplicates-policy-quality = Garder la meilleure qualité
duplicates-scan-hint = Analyse la bibliothèque pour trouver les pistes qui apparaissent plus d'une fois.
duplicates-select-none = Tout désélectionner
duplicates-selected-count = { $count ->
    [one] { $count } sélectionné
   *[other] { $count } sélectionnés
}
duplicates-trash-button = { $count ->
    [0] Mettre à la corbeille
   *[other] Mettre à la corbeille ({ $count })
}
duplicates-trash-error = { $name } : { $error }
duplicates-trash-result = { $count ->
    [one] 1 fichier mis à la corbeille
   *[other] { $count } fichiers mis à la corbeille
}
duplicates-trash-result-failed = { $count } mis à la corbeille, { $failed } en échec
duplicates-trashing = Mise à la corbeille { $done }/{ $total }...
duplicates-window-title = rox - Doublons

## Smart playlists
smart-playlist-descending = Décroissant
smart-playlist-edit-title = Modifier la playlist intelligente
smart-playlist-limit-label = Limite
smart-playlist-limit-placeholder = Sans limite
smart-playlist-match-count = { $count ->
    [one] 1 piste correspond
   *[other] { $count } pistes correspondent
}
smart-playlist-matched-tracks = Pistes correspondantes
smart-playlist-new-title = Nouvelle playlist intelligente
smart-playlist-no-matches = Aucune piste ne correspond
smart-playlist-query-label = Requête
smart-playlist-sort-default = Ordre par défaut
smart-playlist-sort-added = Date d'ajout
smart-playlist-sort-label = Tri
smart-playlist-unknown-field = « { $field }: » n'est pas un champ, le terme correspond donc comme texte brut
smart-playlist-window-title = rox - { $verb }

## Playlist creation
playlist-create-not-savable = Nomme la playlist pour l'enregistrer
playlist-create-placeholder = Nom de la playlist
playlist-create-rename-title = Renommer la playlist
playlist-create-title = Nouvelle playlist
playlist-create-window-title = rox - { $verb }

## Cover tools
cover-art-back = Arrière
cover-art-disc = Disque
cover-art-front = Avant
cover-artwork = Illustration
    .description = Quelle image afficher ; un emplacement absent du fichier retombe sur la pochette avant
cover-disc-style = Style de disque
    .description = Donner à l'illustration l'aspect d'un CD ou de l'étiquette d'un vinyle
cover-disc-off = Désactivé
cover-disc-cd = CD
cover-disc-vinyl = Vinyle
cover-editor-choose-image = Choisir une image
cover-editor-multiple = Multiple
cover-editor-none = Aucune
cover-editor-not-an-image = Ce fichier n'est pas une image que rox peut intégrer
cover-editor-not-decoded = Cette image n'a pas pu être décodée
cover-editor-reading = Lecture de la pochette actuelle...
cover-editor-remove = Retirer
cover-editor-replace = Remplacer
cover-editor-revert = Rétablir
cover-editor-save-errors = { $count ->
    [one] { $count } fichier a échoué ; { $error }
   *[other] { $count } fichiers ont échoué ; { $error }
}
cover-editor-saving-progress = Enregistrement { $done }/{ $total }...
cover-editor-search-online = Chercher en ligne
cover-editor-section = Pochette
cover-editor-slot-back = Pochette arrière
cover-editor-slot-front = Pochette avant
cover-editor-slot-media = Média
cover-editor-will-remove = Sera retiré
cover-editor-window-title = rox - Pochette
cover-matcher-blocked-fetching = Récupération de l'image complète...
cover-matcher-blocked-no-cover = Aucune pochette à définir
cover-matcher-blocked-pick = Choisis une pochette pour la définir
cover-matcher-cover-count = { $count ->
    [one] 1 pochette
   *[other] { $count } pochettes
}
cover-matcher-editor-closed = L'éditeur de pochette a été fermé
cover-matcher-no-covers = Aucune pochette trouvée
cover-matcher-search-failed = Recherche échouée : { $error }
cover-matcher-set-cover = Définir la pochette
cover-matcher-setting = Application...
cover-matcher-tile-info = { $provider }  { $width }px
cover-matcher-unsupported-format = Format d'image non pris en charge
cover-matcher-window-title = rox - Trouver une pochette
cover-spin = Rotation
    .description = Faire tourner le disque pendant qu'une piste joue ; s'applique à l'emplacement disque ou à un style de disque
cover-spin-disc = Rotation du disque
cover-spin-ramp = Montée en régime
    .description = Le temps que met le disque à atteindre sa pleine vitesse, et à redescendre
cover-spin-speed = Vitesse de rotation
    .description = Pleine vitesse, en tours par minute
cover-stretch = Étirer
    .description = Remplir le panneau, sans tenir compte des proportions de l'illustration
cover-stretch-to-fill = Étirer pour remplir
cover-title = Pochette

## Lyrics
lyrics-always-centered = Toujours centré
    .description = Compléter les extrémités pour que la première et la dernière ligne se centrent aussi
lyrics-auto-search = Recherche auto
    .description = Chercher en ligne sur une piste sans paroles et enregistrer une correspondance sûre, sans sélecteur
lyrics-bold = Gras
lyrics-build-word-by-word = Construire mot à mot
    .description = Dévoiler les mots à mesure qu'ils sont chantés, façon karaoké ; les lignes pas encore chantées restent cachées
lyrics-edge-bottom = Bas
lyrics-edge-top = Haut
lyrics-edit-hint-after-stamp = pour horodater
lyrics-edit-hint-or = ou
lyrics-edit-loading = Chargement de la feuille...
lyrics-edit-lyrics = Modifier les paroles
lyrics-edit-saving = Enregistrement...
lyrics-edit-section = Paroles
lyrics-edit-stamp = Horodater
lyrics-edit-stamp-time = Horodater { $time }
lyrics-edit-window-title = rox - Modifier les paroles
lyrics-fade-lines-in = Faire apparaître les lignes en fondu
    .description = Éclaircir une ligne en fondu quand elle devient la ligne active
lyrics-falloff-edge = Côté de l'atténuation
    .description = De quel côté de la ligne active l'atténuation assombrit
lyrics-find-online = Trouver des paroles en ligne...
lyrics-follow-playback = Suivre la lecture
    .description = Glisser la ligne active vers le milieu quand une feuille synchronisée défile
lyrics-font = Police
    .description = La typographie des paroles ; par défaut elle suit la police de l'application
lyrics-gap-threshold = Seuil de blanc
    .description = Combien de temps une intro ou un blanc doit durer avant d'avoir droit à une pause
lyrics-lead-in-rest = Pause d'intro
    .description = Afficher une pause vide avant une longue intro, pour que la première ligne arrive en fondu
lyrics-line-falloff = Atténuation des lignes
    .description = De combien chaque ligne s'assombrit par cran d'écart avec la ligne active
lyrics-line-spacing = Espacement des lignes
    .description = L'écart entre les lignes synchronisées, en multiple de la taille du texte
lyrics-mark-dots = Points
lyrics-mark-note = Note
lyrics-matcher-blocked-no-match = Aucune correspondance à appliquer
lyrics-matcher-blocked-pick = Choisis une correspondance à appliquer
lyrics-matcher-blocked-saving = Enregistrement des paroles...
lyrics-matcher-match-count = { $count ->
    [one] 1 correspondance
   *[other] { $count } correspondances
}
lyrics-matcher-no-query = Cette piste n'a ni artiste ni titre sur quoi chercher
lyrics-matcher-pick-preview = Choisis une correspondance pour l'aperçu
lyrics-matcher-search-failed = Recherche échouée : { $error }
lyrics-matcher-synced-tag = { $provider }  synchronisé
lyrics-matcher-window-title = rox - Trouver des paroles
lyrics-no-lyrics-notice = Pas de paroles
lyrics-no-lyrics-track = Pas de paroles pour cette piste
lyrics-rest-in-gaps = Pause dans les blancs
    .description = Passer à une pause vide pendant un long passage instrumental au lieu de tenir la dernière ligne
lyrics-rest-marker = Marqueur de pause
    .description = Ce qu'affiche une ligne sans mots dans une feuille synchronisée, les blancs et les lignes vides
lyrics-search-button = Bouton de recherche en ligne
    .description = Afficher le bouton de recherche sur la face vide ; le menu contextuel trouve toujours les paroles
lyrics-search-online = Chercher en ligne
lyrics-show-song-name = Afficher le nom du morceau
    .description = Afficher le nom de la piste sur la face vide, au-dessus de la mention Pas de paroles
lyrics-text-size = Taille du texte
    .description = Le texte des paroles ; la hauteur des lignes synchronisées le suit
lyrics-title = Paroles
lyrics-title-unsynced = Titre sur feuille non synchronisée
    .description = Épingler le titre de la piste au-dessus d'une feuille non synchronisée, pour qu'un panneau court l'affiche quand même
lyrics-wipe-lyrics = Effacer les paroles

## Analysis passes
pass-acoustic-body = { $model } détermine à quoi ressemble chacune, pour que la bibliothèque puisse trouver de la musique proche de ce qui joue. Tout tourne sur cette machine, et tout ce qui est déjà décrit est ignoré. { $lands }
pass-acoustic-lands-database = Les résultats vont dans la base de données de la bibliothèque et tes fichiers ne sont pas touchés.
pass-acoustic-lands-tags = Les résultats vont dans la base de données de la bibliothèque et, pour le MP3 et le FLAC, dans les tags de chaque fichier aussi, pour qu'ils soient conservés si la base est reconstruite. Les autres formats ne gardent que la copie en base.
pass-acoustic-title = { $count ->
    [one] Analyser { $count } piste ?
   *[other] Analyser { $count } pistes ?
}
pass-analyze = Analyser
pass-estimate-at = { $estimate } avec { $workers_phrase }.
pass-estimate-button = Estimer
pass-estimating = Estimation...
pass-measure = Mesurer
pass-no-estimate = Rien n'a encore tourné sur cette machine, donc il n'y a pas d'estimation. Estimer chronomètre quelques pistes et en déduit le reste.
pass-replaygain-body = Chaque fichier est décodé et mesuré pour pouvoir jouer au volume auquel il a été masterisé. Les albums sont mesurés en entier quand aucune de leurs pistes n'a de gain. { $lands }
pass-replaygain-lands-database = Les chiffres vont dans la base de données de la bibliothèque et tes fichiers ne sont pas touchés.
pass-replaygain-lands-tags = Les chiffres sont réécrits dans les tags de chaque fichier, là où tous les autres lecteurs les lisent.
pass-replaygain-title = { $count ->
    [one] Mesurer { $count } piste ?
   *[other] Mesurer { $count } pistes ?
}
pass-tempo-body = Deux fenêtres d'une demi-minute de chaque fichier sont décodées et les temps comptés, pour que la bibliothèque puisse montrer à quel tempo tourne une piste. Ça marche mieux sur de la musique enregistrée au clic et ça ignore tout ce qui ne se mesure pas. Les chiffres vont dans la base de données de la bibliothèque et tes fichiers ne sont pas touchés.
pass-tempo-title = { $count ->
    [one] Trouver le tempo de { $count } piste ?
   *[other] Trouver le tempo de { $count } pistes ?
}
pass-timing = Chronométrage de quelques pistes...
pass-timing-failed = Impossible de chronométrer cette bibliothèque : { $error }
pass-workers = Processus

## Quick play
quick-play-comfortable-rows = Lignes aérées
    .description = Donner plus de hauteur à chaque résultat
quick-play-cover = Pochette
    .description = Afficher une vignette de pochette à gauche de chaque résultat
quick-play-duration = Durée
    .description = Afficher la durée de chaque résultat à droite
quick-play-narrow-by = Restreindre par
quick-play-search-placeholder = Rechercher dans la bibliothèque
quick-play-subtitle = Sous-titre
    .description = Afficher l'artiste et l'album sous chaque résultat
quick-play-tag-album = Album
quick-play-tag-artist = Artiste

## Drawer panel
drawer-add-tooltip = Ajouter un panneau tiroir
drawer-answers = Répond à
    .description = Quels choix ouvrent le tiroir : seulement ceux de son propre panneau principal, ou ceux de n'importe quel panneau hors de lui
drawer-dim = Assombrir
    .description = À quel point le panneau principal s'assombrit derrière le tiroir ouvert
drawer-edge = Bord
    .description = Le bord contre lequel le tiroir se pose et depuis lequel il sort
drawer-edge-bottom = Bas
drawer-edge-top = Haut
drawer-handle = Poignée
    .description = Afficher la prise au bord du panneau. Masquée, rien du tiroir n'apparaît avant un choix, et la prise reste ensuite tant que la sélection tient, pour qu'un tiroir replié puisse être ressorti
drawer-open-on = Ouvrir sur
    .description = Se poser sur la poignée ouvre toujours le tiroir ; la sélection ajoute un choix dans le panneau principal
drawer-pin-open = Garder ouvert
drawer-reveal = Recouvrement
    .description = Quelle part du panneau le tiroir ouvert couvre
drawer-scope-elsewhere = Ailleurs
drawer-scope-main = Panneau principal
drawer-title = Tiroir
drawer-trigger-hover = Survol
drawer-trigger-selection = Sélection

## Mini player
mini-tip-back = Retour à la disposition complète
mini-tip-none = Aucune disposition mini assignée
mini-tip-shrink = Réduire au mini-lecteur
mini-title = Bascule mini

## System tray
tray-open = Ouvrir
tray-pause = Pause
tray-play = Lire
tray-quit = Quitter

## Window controls
window-controls-mini-toggle = Bascule mini
    .description = Mettre en tête la bascule de disposition mini ; elle apparaît dès qu'une disposition mini est assignée
window-controls-minimize = Réduire
window-controls-style = Style
    .description = Icônes plates, ou les feux tricolores de macOS
window-controls-style-icons = Icônes
window-controls-title = Contrôles de fenêtre
window-controls-traffic-lights = Feux tricolores

## Particles panel
particles-add-emitter = Ajouter un émetteur
particles-aim = Visée
particles-aim-fixed = Fixe
particles-aim-outward = Vers l'extérieur
particles-burst = Salve
particles-color = Couleur
particles-cone = Cône
particles-direction = Direction
    .description = Le sens de l'attraction ; 0 c'est le haut, 180 le bas
particles-drag = Traînée
    .description = Combien de vitesse l'air mange chaque seconde ; zéro c'est le vide
particles-drift = Dérive
    .description = À quelle vitesse le champ lui-même bouge, pour que les tourbillons ne restent pas figés
particles-edit-emitters = Modifier les émetteurs
particles-emitter-label = Émetteur { $index }
particles-emitter-target = Émetteur { $index } { $target }
particles-emitters-empty = Pas encore d'émetteurs. Ajoutes-en un pour lancer le champ.
particles-glow = Lueur
    .description = Poser un halo doux derrière chaque particule
particles-gravity = Gravité
particles-gravity-strength = Force
    .description = Attraction constante sur tout ce qui vole
particles-height = Hauteur
particles-hold-on-pause = Figer en pause
    .description = Garder le champ en place pendant la pause au lieu de le laisser s'en aller
particles-length = Longueur
particles-lifetime = Durée de vie
particles-position-x = Position X
particles-position-y = Position Y
particles-radius = Rayon
particles-rate = Cadence
particles-rotation = Rotation
particles-round-particles = Particules rondes
    .description = Dessiner des points au lieu de carrés
particles-scale = Échelle
    .description = Quelle largeur fait un tourbillon ; petit ça brasse, grand ça roule
particles-section-emitters = Émetteurs
particles-section-medium = Milieu
particles-section-particles = Particules
particles-section-playback = Lecture
particles-shape = Forme
particles-shape-box = Boîte
particles-shape-line = Ligne
particles-shape-point = Point
particles-shape-ring = Anneau
particles-size = Taille
particles-speed = Vitesse
particles-trigger = Déclencheur
particles-trigger-continuous = Continu
particles-turbulence = Turbulence
particles-turbulence-drift = Dérive de la turbulence
particles-turbulence-scale = Échelle de la turbulence
particles-turbulence-strength = Force
    .description = À quel point le champ pousse les particules ; zéro c'est éteint
particles-width = Largeur

## Spectrum panel
spectrum-axis-labels = Étiquettes d'axe
    .description = Marquer la plage sur le panneau : octaves (C1, C2, ...) ou fréquences (100, 1k, 10k)
spectrum-bar-gap = Écart des barres
    .description = Espace entre les barres, des écarts plus larges font tenir moins de barres
spectrum-bar-width = Largeur des barres
    .description = L'épaisseur de chaque barre, des barres plus fines font tenir plus de bandes
spectrum-block-gap = Écart des blocs
    .description = La jointure entre les cellules d'une pile
spectrum-block-height = Hauteur des blocs
    .description = La hauteur de chaque cellule d'une pile
spectrum-cap-gravity = Gravité des crêtes
    .description = À quelle vitesse les marques de crête retombent une fois la bande redescendue
spectrum-fft-size = Taille de FFT
    .description = Fenêtre d'analyse ; courte elle réagit vite, longue elle résout plus fin
spectrum-gradient-base-color = Couleur de base
    .description = L'extrémité calme du dégradé personnalisé
spectrum-gradient-cover = Pochette
spectrum-gradient-mode = Dégradé
    .description = Colorer les bandes selon le volume : le dégradé du thème, les couleurs de la pochette quand les couleurs du morceau sont actives, ou une paire personnalisée
spectrum-gradient-theme = Thème
spectrum-gradient-tip-color = Couleur de pointe
    .description = L'extrémité forte du dégradé personnalisé
spectrum-high-bound-description = Fréquence la plus haute analysée par les barres
spectrum-high-fft-size = Taille de FFT haute
    .description = Fenêtre d'analyse pour les bandes au-dessus de la coupure
spectrum-hold-on-pause = Figer en pause
    .description = Garder les barres en place pendant la pause au lieu de les laisser tomber au silence
spectrum-labels-frequency = Fréquence
spectrum-labels-pitch = Hauteur
spectrum-low-bound-description = Fréquence la plus basse analysée par les barres
spectrum-orientation = Orientation
    .description = Le bord depuis lequel les bandes poussent
spectrum-outline-bars = Barres en contour
    .description = Dessiner chaque barre en contour creux au lieu d'un dégradé plein
spectrum-outline-width = Épaisseur du contour
    .description = L'épaisseur du trait des barres creuses
spectrum-peak-caps = Marques de crête
    .description = Tenir une marque au sommet récent de chaque bande
spectrum-split-at = Coupure à
    .description = Où les zones se rejoignent, aligné sur la barre la plus proche
spectrum-split-zones = Zones séparées
    .description = Analyser en dessous et au-dessus d'une fréquence de coupure avec des tailles de fenêtre différentes
spectrum-style = Style
    .description = Barres classiques, blocs façon LED, ou une ligne pleine
spectrum-style-bars = Barres
spectrum-style-blocks = Blocs
spectrum-style-line = Ligne
spectrum-symmetry = Symétrie
    .description = Replier le spectre autour du centre ; en avant met les graves sur les bords, en arrière les fait se rejoindre au milieu
spectrum-symmetry-forward = En avant
spectrum-symmetry-reverse = En arrière

## Waveform panel
waveform-bar-gap = Écart des barres
    .description = Espace entre les barres, zéro les fond en une forme pleine
waveform-bar-width = Largeur des barres
    .description = L'épaisseur de chaque barre
waveform-outline = Contour
    .description = Tracer les barres au lieu de les remplir ; des barres fondues ne font qu'une seule forme
waveform-scrobble-marker = Marque de scrobble
    .description = Une ligne fine là où la piste compte comme scrobblée sur Last.fm
waveform-split-channels = Séparer les canaux
    .description = Une rangée par canal, gauche au-dessus de droite ; les pistes mono restent sur une seule rangée
waveform-unavailable = Forme d'onde indisponible pour cette piste

## VU panel
vu-ballistics = Balistique
    .description = VU intègre le volume lentement ; Crête monte d'un coup et redescend doucement
vu-ballistics-peak = Crête
vu-cap-gravity = Gravité des crêtes
    .description = À quelle vitesse les marques de crête retombent une fois l'indicateur redescendu
vu-channels = Canaux
    .description = Séparer la paire stéréo, ou tout replier sur un seul indicateur
vu-channels-mono = Mono
vu-channels-stereo = Stéréo
vu-db-scale = Échelle dB
    .description = Tracer des lignes de repère étiquetées aux marques de dB derrière les indicateurs
vu-gradient-mode = Dégradé
    .description = Colorer les indicateurs selon le niveau : le dégradé du thème, les couleurs de la pochette quand les couleurs du morceau sont actives, ou une paire personnalisée
vu-hold-on-pause = Figer en pause
    .description = Garder les indicateurs en place pendant la pause au lieu de les laisser tomber au silence
vu-orientation = Orientation
    .description = Le bord depuis lequel les indicateurs poussent
vu-peak-caps = Marques de crête
    .description = Tenir une marque au sommet récent de chaque indicateur
vu-segment-gap = Écart des segments
    .description = La jointure entre les cellules d'une pile
vu-segment-height = Hauteur des segments
    .description = La hauteur de chaque cellule d'une pile
vu-style = Style
    .description = Une colonne pleine, ou des segments façon LED
vu-style-continuous = Continu
vu-style-segments = Segments

## Spectrogram panel
spectrogram-ceiling = Plafond
    .description = Niveau qui correspond à l'extrémité claire de la palette, si bien que tout ce qui est plus fort s'y bloque
spectrogram-colormap = Palette de couleurs
    .description = Comment le volume se traduit en couleur
spectrogram-colormap-cover = Pochette
spectrogram-colormap-grayscale = Niveaux de gris
spectrogram-colormap-ice = Glace
spectrogram-colormap-magma = Magma
spectrogram-colormap-theme = Thème
spectrogram-colormap-viridis = Viridis
spectrogram-direction = Direction
    .description = Le bord par lequel les nouvelles colonnes entrent, ce qui décide aussi si l'axe des fréquences monte le long du panneau ou le traverse
spectrogram-fft-size = Taille de FFT
    .description = Taille de la fenêtre d'analyse, un compromis entre la rapidité d'une colonne à suivre un transitoire et sa capacité à séparer deux notes graves
spectrogram-floor = Plancher
    .description = Niveau qui correspond à l'extrémité sombre de la palette, si bien que tout ce qui est plus faible se lit comme fond
spectrogram-grid = Grille
    .description = Lignes de fréquence par-dessus l'image
spectrogram-high-bound = Limite haute
    .description = Haut de l'axe des fréquences, plafonné sous Nyquist pour écarter les octaves les plus hautes, presque silencieuses
spectrogram-history = Historique
    .description = Combien de colonnes le panneau garde avant que la plus ancienne ne défile hors champ
spectrogram-hold-on-pause = Figer en pause
    .description = Garder l'image figée pendant la pause au lieu d'y faire défiler du silence
spectrogram-labels = Étiquettes
    .description = Les chiffres de fréquence le long de la règle, là où le panneau a la place pour eux
spectrogram-log-scale = Échelle log
    .description = Donner le même espace à chaque octave, la lecture musicale, plutôt que l'espacement régulier en Hz d'un outil de labo
spectrogram-low-bound = Limite basse
    .description = Bas de l'axe des fréquences
spectrogram-speed = Vitesse
    .description = À quelle vitesse l'image défile, en colonnes par seconde

## Oscilloscope panel

oscilloscope-channels = Canaux
    .description = Replier en une seule courbe, superposer les deux, ou empiler un cadre pour chacune
oscilloscope-channels-mono = Mono
oscilloscope-channels-overlay = Superposition
oscilloscope-channels-split = Séparé
oscilloscope-fill = Remplissage
    .description = Un remplissage doux entre la courbe et la ligne centrale
oscilloscope-gain = Gain
    .description = Échelle verticale, pour faire remonter un morceau discret jusqu'à une courbe lisible
oscilloscope-gradient-mode = Dégradé
    .description = Colorer la courbe selon l'excursion : le dégradé du thème, les couleurs de la pochette quand les couleurs du morceau sont actives, ou une paire personnalisée
oscilloscope-grid = Grille
    .description = Tracer le réticule derrière la courbe
oscilloscope-hold-on-pause = Figer en pause
    .description = Garder l'image figée pendant la pause au lieu de laisser la courbe s'aplatir
oscilloscope-line-width = Épaisseur du trait
    .description = L'épaisseur du trait de la courbe
oscilloscope-persistence = Persistance
    .description = Combien de temps les images précédentes traînent derrière la courbe, l'effet de rémanence du phosphore
oscilloscope-trigger = Déclenchement
    .description = Démarrer chaque image là où le signal croise le niveau de déclenchement, pour que le contenu périodique reste immobile
oscilloscope-trigger-falling = Descendant
oscilloscope-trigger-level = Niveau de déclenchement
    .description = Le niveau auquel le franchissement est recherché
oscilloscope-trigger-off = Désactivé
oscilloscope-trigger-rising = Montant
oscilloscope-window = Fenêtre
    .description = Combien de temps la courbe couvre sur toute la largeur du panneau

## Shader panel
shader-panel-compile-error = Ce shader n'a pas compilé :
shader-panel-compile-title = Ce shader n'a pas compilé
shader-panel-enable = Activer
shader-panel-inspect = Inspecter
shader-panel-note-empty-body = Choisis un exemple, ou pointe le panneau vers un fichier .wgsl qui définit fs_user(uv).
shader-panel-note-empty-title = Aucun shader chargé.
shader-panel-note-missing-body = Ce panneau renvoie à un shader que l'espace de travail n'a pas, donc il n'y a rien à faire tourner.
shader-panel-note-missing-title = { $name } n'est pas dans les shaders de cet espace de travail.
shader-panel-note-off-body = La source et ses liaisons sont toujours là, elles ne tournent simplement pas.
shader-panel-note-off-title = Ce shader est éteint.
shader-panel-note-pending-body = Il est arrivé avec une disposition ou un espace de travail plutôt que depuis cette machine, donc il reste éteint tant que tu ne l'as pas examiné.
shader-panel-note-pending-title = Ce shader n'a pas encore été examiné.
shader-pending-origin-file = Annoncé comme venant de { $path }
shader-pending-origin-inline = Aucun fichier derrière lui ; la source est arrivée avec la disposition
shader-pending-more-lines = { $count ->
    [one] ... { $count } ligne de plus
   *[other] ... { $count } lignes de plus
}
shader-eject-name-taken = { $name } a déjà { $count } copies numérotées dans les shaders de cet espace de travail
shader-eject-not-in-pool = { $name } n'est pas dans les shaders de cet espace de travail
shader-eject-failed = éjection : { $error }
shader-panel-pick = Choisir un shader
shader-panel-run-shader = Faire tourner le shader
    .description = Éteint garde la source, le signet et les liaisons en place et ne peint rien
shader-panel-section-routes = Routes

## Genre grid panel
genre-grid-clear-picked = Effacer les genres choisis
genre-grid-desaturate = Désaturer pendant la lecture
    .description = Passer en gris toutes les tuiles sauf celle du genre en cours ; le survol rend sa couleur à une tuile
genre-grid-dim-while-playing = Assombrir pendant la lecture
    .description = Estomper toutes les tuiles sauf celle du genre en cours ; le survol rallume une tuile
genre-grid-follow-description = Défiler jusqu'au genre en cours à chaque changement de morceau
genre-grid-merge-many = Fusionner { $count } genres dans « { $target } »
genre-grid-merge-one = Fusionner « { $source } » dans « { $target } »
genre-grid-pick-filters = Le choix filtre la bibliothèque
    .description = Cliquer sur un genre y restreint tous les panneaux qui suivent la recherche partagée ; désactivé, le clic reste une simple sélection
genre-grid-play-genres = Lire { $count } genres
genre-grid-resume-description = Revenir au genre en cours quand tu arrêtes de parcourir
genre-grid-show-names = Afficher les noms
    .description = Afficher le genre sous chaque tuile au lieu de seulement au survol
genre-grid-smooth-description = Glisser jusqu'au genre au lieu de sauter
genre-grid-tally = { $albums ->
    [one] { $albums } album, { $tracks } piste(s)
   *[other] { $albums } albums, { $tracks } piste(s)
}
genre-grid-tile-face = Image de la tuile
    .description = Ce qu'affiche une tuile : les pochettes d'albums du genre, les pochettes teintées de la couleur du genre, ou une carte unie avec le nom posé dessus
genre-grid-unmerge = { $count ->
    [one] Séparer { $count } valeur
   *[other] Séparer { $count } valeurs
}

## Artist grid panel
artist-grid-clear-picked = Effacer les artistes choisis
artist-grid-desaturate = Désaturer pendant la lecture
    .description = Passer en gris toutes les tuiles sauf celle de l'artiste en cours ; le survol rend sa couleur à une tuile
artist-grid-dim-while-playing = Assombrir pendant la lecture
    .description = Estomper toutes les tuiles sauf celle de l'artiste en cours ; le survol rallume une tuile
artist-grid-follow-description = Défiler jusqu'à l'artiste en cours à chaque changement de morceau
artist-grid-group-mode = Une tuile par
    .description = L'artiste de l'album crédité garde les invités d'un disque sur l'artiste qui l'a sorti ; l'artiste de la piste met chaque featuring sur sa propre tuile
artist-grid-pick-filters = Le choix filtre la bibliothèque
    .description = Cliquer sur un artiste y restreint tous les panneaux qui suivent la recherche partagée ; désactivé, le clic reste une simple sélection
artist-grid-play-artists = Lire { $count } artistes
artist-grid-portraits = Portraits d'artistes
    .description = Afficher l'image propre à chaque artiste, cherchée une fois par nom et gardée sur le disque ; désactivé, c'est la pochette du premier album
artist-grid-resume-description = Revenir à l'artiste en cours quand tu arrêtes de parcourir
artist-grid-section-grouping = Regroupement
artist-grid-show-names = Afficher les noms
    .description = Afficher l'artiste sous chaque tuile au lieu de seulement au survol
artist-grid-smooth-description = Glisser jusqu'à l'artiste au lieu de sauter
artist-grid-tally = { $albums ->
    [one] { $albums } album, { $tracks } piste(s)
   *[other] { $albums } albums, { $tracks } piste(s)
}
artist-grid-track-artist = Artiste de la piste

## Wall panels
wall-dim-always = Toujours
    .description = Garder les tuiles en retrait même quand rien ne joue ; seule une tuile survolée s'affiche en entier
wall-dim-amount = Intensité
    .description = À quel point les autres tuiles s'estompent ; 100 % les masque
wall-gap = Écart
    .description = Espace entre les tuiles
wall-name-alignment = Alignement des noms
    .description = Aligner les légendes sous leurs tuiles
wall-rounding = Arrondi
    .description = Arrondir les coins de chaque tuile ; 100 % donne un cercle
wall-section-picking = Choix
wall-show-counts = Afficher les totaux
    .description = Le compte d'albums et de pistes sous chaque nom
wall-tile-size = Taille des tuiles
    .description = Le plus grand côté des tuiles ; les colonnes se partagent la largeur du panneau à parts égales

## Metadata panel
metadata-cover-background = Pochette en fond
    .description = La pochette de la piste derrière les champs
metadata-display = Affichage
    .description = La fiche menée par le titre, ou un tableau plat d'étiquettes et de valeurs depuis le haut
metadata-display-sheet = Fiche
metadata-display-table = Tableau
metadata-edit-save = Enregistrer
metadata-field-bit-depth = Profondeur de bits
metadata-field-bitrate = Débit
metadata-field-codec = Codec
metadata-field-comment = Commentaire
metadata-field-disc = Disque
metadata-field-file = Fichier
metadata-field-sample-rate = Fréquence d'échantillonnage
metadata-field-track = Piste
metadata-fields = Champs
    .description = Quels champs la fiche liste ; un champ absent de la piste reste masqué
metadata-find-online = Chercher les métadonnées en ligne...
metadata-no-library = Aucune bibliothèque
metadata-row-borders-description = Le filet sous chaque ligne du tableau
metadata-source = Source
    .description = Suivre ce qui joue ou ce qui est sélectionné, ou lire la bibliothèque en entier
metadata-stripes-description = Teinter une ligne du tableau sur deux

## History panel
history-column-last-played = Dernière écoute
history-descending = Décroissant
    .description = Inverser le tri
history-empty-never = Toutes les pistes ont été écoutées
history-empty-recent = Aucune écoute pour l'instant
history-headings = Découper la liste récente en séries d'albums ; Étendu ajoute la pochette et les stats
history-sort-browse = Ordre de parcours
history-sort-date-added = Date d'ajout
history-sort-menu = Tri
    .description = Comment les pistes jamais écoutées sont ordonnées
history-title = Historique
history-view-most = Les plus écoutées
history-view-never = Jamais écoutées
history-view-recent = Écoutées récemment
history-view-recent-short = Récentes
history-view-row = Vue
    .description = Quelle tranche du relevé d'écoutes le panneau montre

## Folder tree panel
folder-tree-clear-scope = Effacer la portée du dossier
folder-tree-collapse-all = Tout replier
folder-tree-cover-art = Pochette
    .description = Afficher la pochette à la place de l'icône de la ligne, sur les dossiers ou les morceaux
folder-tree-cover-folders = Dossiers
folder-tree-cover-songs = Morceaux
folder-tree-empty = Aucun dossier dans la bibliothèque pour l'instant
folder-tree-follow-description = Révéler la piste en cours et défiler jusqu'à elle à chaque changement
folder-tree-nonmatch-folders = Dossiers sans correspondance
    .description = Masquer les dossiers sans correspondance, ou les garder estompés
folder-tree-nonmatch-songs = Morceaux sans correspondance
    .description = Dans un dossier qui correspond, estomper les morceaux isolés ou les masquer
folder-tree-play-folder = Lire le dossier
folder-tree-play-songs = { $count ->
    [one] Lire
   *[other] Lire { $count } morceaux
}
folder-tree-resume-description = Revenir à la piste en cours quand tu arrêtes de parcourir
folder-tree-scope-to-folder = Restreindre le filtre au dossier
folder-tree-smooth-description = Glisser jusqu'à la piste au lieu de sauter
folder-tree-title = Arborescence

## Art panel
art-always = Garder les pochettes en retrait même quand rien ne joue ; seule une pochette survolée s'affiche en entier
art-convert = Convertir...
art-covers-section = Pochettes
matcher-section-matches = Correspondances
art-desaturate = Passer en gris toutes les pochettes sauf celle de l'album en cours ; le survol rend sa couleur à une pochette
art-dim-while-playing = Estomper toutes les pochettes sauf celle de l'album en cours ; le survol rallume une pochette
art-disc-style = Style de disque
    .description = Donner à chaque pochette l'aspect d'un CD ou de l'étiquette d'un vinyle
art-edit-tags = Modifier les tags...
art-fill-panel = Remplir le panneau
    .description = Dimensionner la pochette centrée sur la seule hauteur du panneau (la largeur en vertical) ; les pochettes latérales débordent du bord au lieu de la rétrécir
art-follow-description = Centrer l'album en cours à chaque changement de morceau
art-glow = Lueur
    .description = Étaler la couleur d'accent derrière la pochette centrée ; avec la teinte de pochette active, elle prend la couleur de l'album en cours
art-layout-section = Disposition
art-perspective = Perspective
    .description = Tourner les pochettes latérales en vraie 3D au lieu de l'aplatissement
art-reflections = Reflets
    .description = Refléter chaque pochette dans le sol sous l'étagère
art-resume-description = Recentrer l'album en cours quand tu arrêtes de parcourir
art-shadows = Ombres
    .description = Une ombre douce sous chaque pochette
art-smooth-description = Glisser jusqu'à l'album au lieu de sauter
art-title = Carrousel d'albums
art-vertical-layout = Disposition verticale
    .description = Empiler l'étagère en colonne qui défile de haut en bas au lieu d'une rangée

## Playlists panel
playlists-columns = Quelles colonnes de piste s'affichent à côté du titre
playlists-delete = Supprimer la playlist
playlists-edit-query = Modifier la requête...
playlists-empty = Aucune playlist pour l'instant, ajoute des pistes ou utilise Nouvelle playlist
playlists-headings = Découper les pistes de chaque playlist en séries d'albums ; Étendu ajoute la pochette et les stats
playlists-import-tooltip = Importer une playlist
playlists-imported-fallback = Importée
playlists-new = Nouvelle playlist...
playlists-new-smart = Nouvelle playlist intelligente...
playlists-refuse-drag-out = Les pistes d'une playlist intelligente ne peuvent pas être sorties par glissement
playlists-refuse-edit-query = Modifie la requête pour changer ce que contient une playlist intelligente
playlists-refuse-smart-source = Une playlist intelligente tire ses pistes de sa requête
playlists-remove = { $count ->
    [one] Retirer de la playlist
   *[other] Retirer { $count } de la playlist
}
playlists-rename = Renommer...
playlists-title = Playlists

## Queue panel
queue-clear = Vider la file
queue-empty = La file est vide
queue-headings = Découper la file en séries d'albums ; Étendu ajoute la pochette et les stats
queue-play-now = Lire maintenant
queue-remove = { $count ->
    [one] Retirer de la file
   *[other] Retirer { $count } de la file
}
queue-title = File d'attente
queue-widget-always-modal = Toujours ouvrir en modale
    .description = Ouvrir la file dans une modale à chaque fois, au lieu de sauter à un panneau de file déjà ouvert
queue-widget-clear-queue = Vider la file
queue-widget-more = +{ $count } de plus
queue-widget-open-on-click = Ouvrir la file au clic
    .description = Cliquer sur le widget saute à un panneau de file ouvert, ou ouvre la file dans une fenêtre s'il n'y en a pas
queue-widget-section-click = Clic
queue-widget-title = Widget de file
queue-widget-up-next = À suivre

## Biography panel
biography-background = Fond
    .description = Le fanart de l'artiste derrière le texte, assombri et disparaissant vers le bas
biography-fill-width = Remplir la largeur
    .description = Laisser un en-tête haut prendre toute la largeur au lieu de rester plafonné et centré
biography-from-lastfm = Depuis Last.fm
biography-header-image = Image d'en-tête
    .description = La large bannière de l'artiste en haut, ou le portrait quand il n'y a pas de bannière
biography-keep-aspect = Garder les proportions
    .description = Afficher l'en-tête à ses propres proportions au lieu de le rogner pour remplir une bande
biography-listeners-count = { $count ->
    [one] { $count } auditeur
   *[other] { $count } auditeurs
}
biography-looking-up = Recherche de { $name }
biography-no-artist-tag = Aucun tag d'artiste
biography-no-text = Aucune biographie enregistrée
biography-not-found = Rien trouvé pour { $name }
biography-plays-count = { $count ->
    [one] { $count } écoute
   *[other] { $count } écoutes
}
biography-refresh = Actualiser
biography-similar-artists = Artistes proches
    .description = Les artistes proches d'après les données d'écoute, tout en bas
biography-similar-heading = Artistes proches
biography-stats = Stats
    .description = Auditeurs et écoutes sur Last.fm, sous le nom
biography-tags = Tags
    .description = Les tags de genre en rangée de pastilles
biography-title = Biographie

## Status panel
status-count-albums = { $count ->
    [one] { $count } album
   *[other] { $count } albums
}
status-count-artists = { $count ->
    [one] { $count } artiste
   *[other] { $count } artistes
}
status-count-plays = { $count ->
    [one] { $count } écoute
   *[other] { $count } écoutes
}
status-count-selected = { $count ->
    [one] { $count } sélectionné
   *[other] { $count } sélectionnés
}
status-count-tracks = { $count ->
    [one] { $count } piste
   *[other] { $count } pistes
}
status-readouts = Relevés
    .description = Fais glisser le long de la barre pour réordonner ; fais glisser entre les rangées, ou utilise le x et le plus d'une pastille, pour masquer et afficher
status-scope-selection = Sélection
status-title = État

## Output panel
output-detail-badge = Badge
output-detail-compact = Compact
output-detail-expanded = Étendu
output-detail-label = Détail
    .description = Badge s'en tient à une pastille avec le reste au survol ; compact donne une ligne à part au titre, pour une barre le long d'un bord ; étendu ajoute les raisons à côté, ou en dessous quand le panneau est trop étroit
output-device-name = Nom du périphérique
    .description = Nommer le périphérique en cours dans le titre ; désactivé, la ligne s'en tient au mode, à la fréquence et au format
output-file-rate = Fréquence du fichier
    .description = Confirmer la fréquence propre du fichier en lecture quand rien ne la convertit. Une conversion est signalée dans tous les cas, puisque c'est de ça que parle l'avertissement
output-mode-exclusive = Exclusif
output-mode-shared = Partagé
output-no-output = Aucune sortie
output-nothing-playing = Rien en lecture
output-pick-another-device = Choisis un autre périphérique, ou désactive l'exclusif
output-headline-numbers = { $rate } Hz, { $channels } can., { $format }
output-headline = { $mode }, { output-headline-numbers }
output-headline-device = { $mode } sur { $device }, { output-headline-numbers }
output-fell-back-to-shared = Exclusif est retombé en partagé : { $why }
output-replaygain-levelling = ReplayGain nivelle ce fichier de { $db } dB
output-replaygain-short = ReplayGain { $db } dB
output-rate-resampled = Le fichier en lecture est en { $rate } Hz, rééchantillonné pour atteindre le périphérique
output-rate-resampled-short = Fichier { $rate } Hz rééchantillonné
output-rate-native = Le fichier en lecture est en { $rate } Hz, donc rien ne le rééchantillonne
output-rate-native-short = Fichier { $rate } Hz, sans rééchantillonnage
output-start-track-hint = Lance une piste pour voir le format accepté par le périphérique
output-title = Sortie

## Track columns
columns-bits = Bits
columns-bpm = BPM
columns-codec = Codec
columns-cover = Pochette
columns-fav = Fav
columns-gain = Gain
columns-kbps = Kbps
columns-khz = kHz
columns-name = Nom
columns-number = Numéro
columns-scanned = Analysé
columns-similar = Proches

## Filter panel
filter-add-column = Ajouter une colonne
filter-add-column-tooltip = Ajouter une colonne
filter-all = Tous
filter-clear-filters = Effacer les filtres
filter-clear-selection = Effacer la sélection
filter-empty = Choisis un champ pour commencer à filtrer
filter-remove-column = Retirer la colonne

## Search panel
search-chips-below = En dessous
search-chips-inline = En ligne
search-filter-chips = Pastilles de filtre
search-placeholder = Rechercher dans la bibliothèque

## Playback panel
playback-buttons = Boutons
    .description = Fais glisser le long de la barre pour réordonner ; fais glisser entre les rangées, ou utilise le x et le plus d'une pastille, pour masquer et afficher
playback-continue-down-list = Continuer la lecture, en descendant la liste
playback-continue-off = Continuer désactivé
playback-continue-weighted = Continuer la lecture, jamais écoutées d'abord
playback-crossfade-inside-albums = Dans les albums
playback-crossfade-off = Fondu enchaîné désactivé
playback-crossfade-tip = Fondu enchaîné { $length }
playback-highlight-circle = Cercle
playback-highlight-square = Carré
playback-hold-draw = { $tip }. Maintiens pour choisir un tirage
playback-hold-length = { $tip }. Maintiens pour choisir une durée
playback-hold-order = { $tip }. Maintiens pour choisir un ordre
playback-loop-off = Boucle désactivée
playback-loop-queue = Boucler la file
playback-loop-track = Boucler cette piste
playback-menu-continue = Bouton Continuer
playback-menu-crossfade = Bouton Fondu enchaîné
playback-menu-favourite = Bouton Favori
playback-menu-random = Bouton Au hasard
playback-menu-rating = Étoiles de note
playback-menu-stop = Bouton Arrêt
playback-menu-stop-after = Bouton Arrêter après
playback-menu-volume = Bouton Volume
playback-pause = Pause
playback-play-highlight = Surbrillance de lecture
    .description = Le fond d'accent du bouton lecture : un cercle, un carré doux, ou rien
playback-random-tip-random = Lire une piste au hasard
playback-random-tip-similar = Lire une piste comme celle-ci
playback-seek-back-tip = 10 secondes en arrière
playback-seek-forward-tip = 10 secondes en avant
playback-shuffle-off = Aléatoire désactivé
playback-shuffle-on = Aléatoire activé, ordre { $order }
playback-stop-after-armed = Arrêt après cette piste, armé
playback-stop-after-tip = Arrêter après cette piste
playback-stop-tip = Arrêter et décharger la piste
playback-volume-tip-muted = Réactiver le son, { $percent } %. Clic droit pour le curseur
playback-volume-tip-unmuted = Couper le son, { $percent } %. Clic droit pour le curseur

## Track info panel
track-info-color-output-chip = Colorer la pastille de sortie
    .description = Laisser la pastille virer aux couleurs d'avertissement quand la sortie se rabat ou rééchantillonne. Désactivé, elle garde toujours le même ton discret, et la note au survol explique quand même l'état
track-info-cycle-every = Alterner toutes les
    .description = Combien de temps chaque rangée reste avant le fondu
track-info-cycle-rows = Alterner les rangées
    .description = Montrer les rangées de l'arrangement une à une sur une seule ligne, en fondu entre elles ; une rangée seule s'affiche telle quelle
track-info-delay = Délai
    .description = Combien de temps la ligne se repose à chaque bout avant de repartir
track-info-marquee = Défilement
    .description = Ce que fait une ligne trop longue pour le panneau : défiler et revenir, ou boucler sans fin
track-info-menu-overflow = Débordement
track-info-next = Suivant : { $line }
track-info-opening = ouverture...
track-info-output-fallback = La sortie exclusive a été refusée par le périphérique, donc la lecture passe par le mixeur partagé. Le périphérique a répondu : { $reason }
track-info-output-resample-exclusive = Ce fichier est en { $source } kHz et la carte a pris { $device } kHz, donc chaque échantillon est converti en sortie. Le périphérique ne voulait pas tourner à la fréquence propre du fichier.
track-info-output-resample-mixer = Ce fichier est en { $source } kHz et le mixeur tourne à { $device } kHz, donc chaque échantillon est converti en sortie. Le mode exclusif donnerait plutôt à la carte la fréquence propre du fichier.
track-info-overflow-loop = Boucler
track-info-overflow-scroll = Défiler
track-info-overflow-truncate = Tronquer
track-info-queued-count = { $count } en file
track-info-row-size = Taille de la rangée { $number }
track-info-speed = Vitesse
    .description = À quelle vitesse la ligne défile
track-info-text-size = Taille du texte

## Seek panel
seek-ending = Fin
    .description = Décompter le temps restant ou montrer la durée totale
seek-ending-remaining = Restant
seek-ending-total = Totale
seek-playhead = Tête de lecture
    .description = Couvrir toute la hauteur de la barre ou coller à la ligne
seek-playhead-full = Pleine
seek-playhead-line = Ligne
seek-playhead-max-height = Hauteur max de la tête de lecture
    .description = Plafonner la tête pleine, centrée sur la ligne ; 0 remplit le panneau
seek-playhead-width = Largeur de la tête de lecture
    .description = La largeur du marqueur de position mobile
seek-rounding = Arrondi
    .description = Le rayon des coins de la ligne, jusqu'à une pilule à la moitié de l'épaisseur
seek-scrobble-marker = Marque de scrobble
    .description = Une ligne fine là où la piste compte comme scrobblée sur Last.fm
seek-show-timings = Afficher les temps
seek-thickness = Épaisseur
    .description = La hauteur de la ligne de piste

## Volume panel
volume-pieces = Éléments
    .description = Fais glisser le long de la barre pour réordonner ; fais glisser entre les rangées, ou utilise le x et le plus d'une pastille, pour masquer et afficher. Avec le pourcentage masqué, l'infobulle du haut-parleur l'affiche
volume-readout = Relevé
    .description = Montrer le niveau en pourcentage ou en décibels de gain appliqué
volume-readout-decibels = Décibels
volume-readout-percent = Pourcentage
volume-stretch = Étirer
    .description = Laisser le curseur remplir le panneau au lieu de plafonner sa largeur
volume-tip-mute = Couper le son
volume-tip-mute-level = Couper le son, { $level }
volume-tip-unmute = Réactiver le son
volume-tip-unmute-level = Réactiver le son, { $level }

## Shared panel content
content-filter = Filtre
content-no-track = Aucune piste
content-total-genres = Genres
content-total-time = Durée totale

## Shared panel chrome
panel-columns-description = Quelles colonnes de piste s'affichent
panel-headings = En-têtes
panel-jump-to-playing = Aller à la lecture en cours
panel-menu-display = Affichage
panel-title-artists = Artistes
panel-title-genres = Genres
panel-title-oscilloscope = Oscilloscope
panel-title-particles = Particules
panel-title-playback = Lecture
panel-title-seek = Progression
panel-title-shader = Shader
panel-title-spectrogram = Spectrogramme
panel-title-spectrum = Spectre
panel-title-theme-toggle = Bascule de thème
panel-title-track-info = Infos de piste
panel-title-volume = Volume
panel-title-vu = Vumètre
panel-title-waveform = Forme d'onde

## Everything else
choice-both = Les deux
choice-dim = Estomper
choice-hide = Masquer
composite-add-panel = Ajouter un panneau
composite-host-settings = Réglages de { $host }
composite-move-left = Déplacer à gauche
composite-move-right = Déplacer à droite
composite-remove = Retirer
composite-replace = Remplacer
group-panel-add-slot = Ajouter un emplacement
group-panel-move-down = Déplacer vers le bas
group-panel-move-up = Déplacer vers le haut
group-panel-remove-slot = Retirer l'emplacement
group-panel-split-side-by-side = Diviser côte à côte
group-panel-split-stacked = Diviser en pile
group-panel-swap-panels = Échanger les panneaux
group-panel-title = Groupe
overlay-dim = Assombrir
    .description = À quel point le panneau principal s'assombrit sous la surcouche révélée
overlay-title = Surcouche
overlay-toggle = Basculer la surcouche
shader-confirm-hint-after = bascule le shader depuis n'importe où.
shader-confirm-hint-before = Un shader peut rendre les fenêtres difficiles à utiliser. Reviens en arrière ou ferme cette fenêtre pour retrouver l'état d'avant.
shader-confirm-keep = Garder
shader-confirm-question = Garder ce shader d'écran ?
shader-confirm-revert = Revenir en arrière
shader-confirm-window-title = rox - Shader de surcouche
slide-add = Ajouter une diapositive
slide-next = Diapositive suivante
slide-previous = Diapositive précédente
slide-title = Diapositive
theme-toggle-to-dark = Passer au thème sombre
theme-toggle-to-light = Passer au thème clair
transport-favourite-add = Ajouter aux favoris
transport-favourite-nothing = Rien à mettre en favori
transport-favourite-remove = Retirer des favoris
transport-pieces = Éléments
    .description = Fais glisser le long d'une rangée pour réordonner et entre les rangées pour déplacer ; le x et le plus d'une pastille masquent et affichent

## Stragglers picked up in the final sweep

duplicates-scanning = Analyse en cours...
about-copyright = Copyright © 2026
signal-name-placeholder = Nom du signal
signals-empty = Aucun signal pour l'instant. Ajoutes-en un, ou fais un clic droit sur n'importe quel bouton qui accepte une liaison.
signal-add = Ajouter un signal
panel-approve = Approuver
panel-turn-off = Désactiver
shader-from-file = Depuis un fichier...
arrange-add-row = Ajouter une ligne
smart-playlist-name-placeholder = Nom de la playlist
smart-playlist-name-to-save = Nomme la playlist pour l'enregistrer
panel-new-playlist = Nouvelle playlist...
panel-edit-tags = Modifier les tags...
panel-edit-cover = Modifier la pochette...
panel-rename-files = Renommer les fichiers...
panel-convert = Convertir...
panel-catalog-drag-anchor = Poignée de déplacement
panel-catalog-spacer = Espaceur

## Duration and worker phrasing

pace-under-a-minute = moins d'une minute
pace-minutes = { $count ->
    [one] environ une minute
   *[other] environ { $count } minutes
}
pace-hours = { $count ->
    [one] environ une heure
   *[other] environ { $count } heures
}
pace-half-hours = { $value ->
    [one] environ { $value } heure
   *[other] environ { $value } heures
}
pace-days = { $count ->
    [one] environ un jour
   *[other] environ { $count } jours
}
pace-workers = { $count ->
    [one] { $count } processus
   *[other] { $count } processus
}
tasks-rest-takes = , le reste prend { $estimate }
tasks-measuring-takes = , les mesurer prend { $estimate }
tasks-working-out-takes = , les calculer prend { $estimate }
tasks-time-left = , il reste { $left }
tasks-failed-suffix = ({ $count } en échec)
tasks-file-suffix = - { $file }
tasks-no-beat-suffix = ({ $count } sans tempo net)
tasks-estimate-at-workers = ({ tasks-estimate-at })

## Panel vanity names

panel-title-art-view = Vue pochette
panel-title-artist-grid = Grille d'artistes
panel-title-genre-grid = Grille de genres
panel-title-biography = Biographie
panel-title-cover-art = Pochette
panel-title-drag-anchor = Poignée de déplacement
panel-title-drawer = Tiroir
panel-title-eq-widget = Widget EQ
panel-title-filter = Filtre
panel-title-folder-tree = Arborescence
panel-title-group = Groupe
panel-title-history = Historique
panel-title-lyrics = Paroles
panel-title-menu = Menu
panel-title-metadata = Métadonnées
panel-title-mini-toggle = Bascule mini
panel-title-output = Sortie
panel-title-overlay = Surcouche
panel-title-playlists = Playlists
panel-title-queue = File d'attente
panel-title-queue-widget = Widget de file
panel-title-search = Recherche
panel-title-slide = Diapositive
panel-title-spacer = Espaceur
panel-title-stats-widget = Widget Stats
panel-title-vu-meter = Vumètre
panel-title-window-controls = Contrôles de fenêtre

## Relative time and the output headline

ago-just-now = à l'instant
ago-minutes = il y a { $count } min
ago-hours = il y a { $count } h
ago-days = il y a { $count } j
ago-weeks = il y a { $count } sem.
ago-years = { $count ->
    [one] il y a { $count } an
   *[other] il y a { $count } ans
}

span-seconds = { $count ->
    [one] { $count } seconde
   *[other] { $count } secondes
}
span-minutes = { $count ->
    [one] { $count } minute
   *[other] { $count } minutes
}
span-hours = { $count ->
    [one] { $count } heure
   *[other] { $count } heures
}
span-days = { $count ->
    [one] { $count } jour
   *[other] { $count } jours
}
span-weeks = { $count ->
    [one] { $count } semaine
   *[other] { $count } semaines
}
span-years = { $count ->
    [one] { $count } an
   *[other] { $count } ans
}
span-pair = { $first }, { $second }
unit-percent = { $value } %

settings-audio-output-headline = { $mode }{ $note } sur { $device }, { $rate } Hz, { $channels } can., { $format }
settings-audio-output-experimental =  (expérimental)

## ML model catalog

settings-mlmodels-description = { $summary }. { $dim } valeurs par piste. { $licence }
settings-mlmodels-on-disk = , { $size } sur le disque
settings-mlmodels-to-download = , { $size } à télécharger
model-summary-dsp-timbre-1 = Intégré, sans téléchargement. Un résumé de l'énergie par bande, de la forme spectrale et du taux d'attaques de chaque piste. Grossier à côté d'un réseau entraîné, mais il n'a besoin de rien et tourne partout
model-summary-panns-cnn10 = Un réseau convolutif entraîné sur AudioSet pour reconnaître ce qu'est un son. Sa description d'une piste en 512 valeurs est bien plus riche que l'esquisse intégrée, au prix d'un téléchargement de 24 Mo et d'une analyse plus lente

## Shipped workspaces

workspace-shipped-default = (Par défaut)
workspace-shipped-default-blurb = À quoi rox ressemble à la sortie de la boîte : des surfaces translucides sur le bureau, aucune décoration de fenêtre, teinte de pochette désactivée. Le point de départ dont tous les autres habillages s'écartent.
workspace-shipped-catrox-blurb = Le skin foobar2000 qui a tout lancé, reconstruit : un rendu circulaire de la pochette en CD, les champs de métadonnées le long de la gauche, et des pistes groupées par album avec des pastilles de note.
workspace-shipped-critters-blurb = Toute l'application en impression 1 bit : un tramage ordonné sur chaque surface, des tons qui s'écrasent avec les infrabasses, et un mur de bruit qui se tord avec le morceau. D'après Critters for Sale.
workspace-shipped-diffuse-blurb = Juste l'album en cours : la pochette et la carte de lecture en un seul groupe qui remplit la fenêtre, des surfaces transparentes sur le fond, sans couture. La bibliothèque, la file et les paroles attendent dans un tiroir sur le bord droit et glissent par-dessus la musique quand on survole la poignée. Monochrome, donc la couleur vient des pochettes.
workspace-shipped-foobar-blurb = La disposition avec laquelle ce projet tout entier discute. Panneaux opaques, colonnes de filtre par artiste et par album, une table de pistes dense, et la barre de menus exactement là où elle a toujours été.
workspace-shipped-llama-winamp-blurb = Winamp tel que tu t'en souviens plutôt que tel qu'il était. Tahoma, sombre, sans décorations, un spectre pointillé en haut, et un mode réduit sur la disposition mini.
workspace-shipped-metro-blurb = Panneaux plats et lignes confortables en Segoe UI, avec la teinte de pochette activée pour que toute la palette suive la pochette en cours.
workspace-shipped-phosphor-blurb = Tout en chasse fixe. Consolas, vert sur noir, pas de pochette en lecture rapide : un terminal qui joue de la musique.
