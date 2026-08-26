### The source locale. Every key lives here first, the other locales
### mirror this file one for one, and the parity test in rox-i18n is
### what holds them to it. Keys are surface-prefixed kebab-case; a
### row's description rides the label's message as an attribute.

## Shared widgets

# The tracking section every scrolling panel's customize window carries;
# what each toggle follows is the panel's own wording, passed in.
tracking-title = Tracking
tracking-follow = Follow Playing
tracking-resume = Resume When Idle
tracking-smooth = Smooth Scrolling
align-row = Alignment
    .description = Where the content sits when the panel has room to spare
valign-row = Vertical Alignment
    .description = Where the content sits when the panel has height to spare
valign-top = Top
valign-middle = Middle
valign-bottom = Bottom

## Panel source and search rows

source-track = Track
    .description = Follow what is playing, or what is selected in the library
source-follow-playing = Follow Playing
source-follow-selection = Follow Selection
source-playing = Playing
source-selected = Selected
query-search = Search
query-search-box = Search Box
    .description = Show the search box; the query only applies while it shows
query-source = Search Source
    .description = Follow the shared search query, filter by this panel's own box, or show what another panel has selected
query-source-shared = Shared
query-source-own = Own
query-source-selection = Selection

## Signals and routes

signal-source = Source
    .description = What the signal listens to: Band follows one frequency range, Level the whole mix, Onset pulses on each hit in the range, Trigger fires a pulse when the range reaches its threshold, Total adds up another signal over time
signal-kind-band = Band
signal-kind-level = Level
signal-kind-onset = Onset
signal-kind-trigger = Trigger
signal-kind-total = Total
signal-response = Response
signal-response-pulse = How long each pulse rings before it dies away
signal-response-drift = 0 snaps to the music, 100 drifts after it
signal-threshold = Threshold
signal-threshold-trigger = The level the range has to reach to fire the pulse; it can't fire again until the level falls back under the mark on the meter above
signal-threshold-gate = Under this the signal reads as nothing, and above it the output climbs from zero again, so the quiet parts leave the knob alone; the mark on the meter above is where it sits
signal-low-bound = Low Bound
signal-high-bound = High Bound
signal-adds-up = Adds Up
    .description = Which signal this totals; it climbs while that one reads high and stalls while it's quiet
signal-aggregate-nothing = Nothing to follow
signal-aggregate-pick = Pick a signal
signal-aggregate-alone = There's no other signal in the pool for this to add up, so it sits at zero. Add one and it shows up in the list.
signal-aggregate-unpicked = Nothing picked, so this total sits at zero. Pick a signal above.
signal-rate = Rate
    .description = Wraps per second at full input; it rolls over 1 back to 0 and keeps climbing, which a shader reads as a phase
signal-reset-on-track = Reset on Track
    .description = Drain back to zero when a new song starts, so a phase doesn't carry the last one's total into it
signal-flush = Flush
    .description = Send it back to zero now; it drains over a moment rather than snapping, so nothing riding it jumps
route-header = Route
route-signal = Signal
    .description = Which shared signal this route rides; tuning it here tunes every route on it
route-new-signal = New Signal
route-shared-note = Shared by every route on this signal
route-signal-gone = This route's signal is gone; the knob holds its slider value until another is picked above.
route-range-note = Range for this parameter only
route-quiet = Quiet
    .description = What the knob reaches at silence, as a share of its own setting
route-loud = Loud
    .description = What it reaches at full signal; 100% is the slider's own value, below Quiet modulates down
route-slot = Slot
    .description = Which of the shader's sixteen signal slots this route fills
route-slot-quiet-description = What the slot reads at silence
route-slot-loud-description = What it reads at full signal; below Quiet runs the slot backwards
route-slot-signal-description = Which shared signal this route rides
route-slot-signal-gone = This route's signal is gone; the slot reads zero until another is picked.
route-add = Add Route
route-unrouted = Unrouted
route-pick-slot = Pick a slot
route-pick-signal = Pick a signal
route-no-signal = no signal
route-no-signals-yet = There are no signals to ride yet. Make one and it shows up here; until then the slot reads zero.
route-open-signals = Open Signals
route-create-signal = Create New Signal

## Panel settings window

panel-settings = Panel Settings
panel-menu-label = Panel
panel-save-as-preset = Save As Preset
panel-rename = Rename
panel-rename-name = Name
panel-rename-note = Shown as the panel's tab; empty goes back to the built-in name
panel-rename-hint-after = to rename
panel-was-closed = The panel was closed
panel-reset = Reset
panel-inverse = Inverse
panel-apply-song-theme = Apply Song Theme
panel-page-appearance = Appearance
panel-page-behavior = Behavior
panel-page-shader = Shader
panel-section-placement = Placement
panel-section-size = Size
panel-section-opacity = Opacity
panel-section-frame = Frame
panel-section-colors = Colors
panel-section-font = Font
panel-section-shader = Shader
panel-section-signals = Signals
panel-section-slots = Slots
panel-awaiting-approval = Awaiting Approval
panel-size-off = Off
panel-locked = Locked
    .description = Pin the panel in place; the dock won't let it be dragged or rearranged
panel-drag-anchor = Drag Anchor
    .description = A drag anywhere on the panel moves the window, while plain clicks still land on its controls; for decorations-off layouts
panel-slot-controls = Slot Controls
    .description = Show the corner buttons for swapping and removing the panels this one hosts. Hidden, the layout is still edited from the tree on the Workspace page in Settings
panel-min-width = Min Width
    .description = Where a resize stops squeezing the panel narrower. Taken as written, under the panel's own floor included, so a compact strip can go tighter than stock; empty leaves the floor alone
panel-max-width = Max Width
    .description = Cap the panel's width so it doesn't stretch when the window widens
panel-min-height = Min Height
    .description = Where a resize stops squeezing the panel shorter. Taken as written, under the panel's own floor included, so a compact strip can go tighter than stock; empty leaves the floor alone
panel-max-height = Max Height
    .description = Cap the panel's height so it doesn't stretch when the window grows taller
panel-own-opacity = Own Surface Opacity
    .description = Give this panel its own opacity over the backdrop instead of the app's
panel-surface-opacity = Surface Opacity
panel-margin = Margin
    .description = Pull the panel in from its cell, the backdrop showing through the gap
panel-padding = Padding
    .description = Space inside the panel's edge, kept in its own background
panel-rounding = Rounding
    .description = Round the panel's corners off into the backdrop
panel-border = Border
    .description = A line around the panel's edge, in the Border role's color; a side at zero draws none
panel-font = Font
    .description = The panel's typeface; default follows the app font
panel-font-size = Font Size
    .description = The panel's text size relative to the app font; rows scale with it
panel-surface-shader = Surface Shader
    .description = Run a WGSL shader over this panel's body, under the app's screen shader
panel-run-when-idle = Run When Idle
    .description = Keep drawing frames while the audio is silent. Off, the shader parks where it stands and the panel costs nothing
panel-shader-is-scene = This shader is a scene, so it covers the panel's body rather than drawing over it. It came from a bundle or an older config; the list above only offers shaders that leave the panel readable.

## Shader picker and saving

shader-source = Source
shader-pick-none = None
shader-reload = Reload
shader-edit-as-file = Edit as File
shader-make-private-copy = Make Private Copy
shader-save-replace = Replace
shader-save-to-workspace = Save to Workspace
shader-save-replaces = Replaces the shader this workspace already calls { $name }. Every panel using that name changes with it
shader-save-adds = Adds it to this workspace's shaders under { $name }. Any panel can use it, and editing it updates them all
shader-group-examples = Examples
shader-group-this-workspace = This Workspace
shader-group-scenes = Scenes
shader-group-workspace-scenes = Workspace Scenes
shader-group-overlays = Overlays
shader-group-workspace-overlays = Workspace Overlays

## Saving a panel preset

preset-save = Save Preset
preset-save-name = Preset Name
preset-save-replaces = Replaces the preset this workspace already calls { $name }
preset-save-hint-after = to save
preset-back-from = Add it back from
preset-back-add-panel = Add Panel
preset-back-then = then
preset-back-presets = Presets
preset-back-tail = in any panel menu. Presets ride this workspace only, so another workspace won't carry it.

## Keyboard hints

hint-press = Press
hint-key-enter = Enter

## Settings: language

settings-language = Language
    .description = The language the interface speaks; System negotiates against the OS's list and lands on English when nothing matches
settings-language-system = (System Language)
settings-language-search = Search languages
picker-no-matches = No matches

## Embed dialog

bake-window-title = rox - Embed Stored Metadata
bake-title = Embed Stored Metadata
bake-intro = Writes what rox is already holding into the files themselves, so another player reads it too. Nothing is worked out again.
bake-formats = MP3 and FLAC only; other formats and CUE tracks are skipped
bake-source-lyrics = Lyrics
bake-source-gain = ReplayGain
bake-source-acoustic = Acoustic descriptions
bake-detail-nothing = nothing stored to embed
bake-detail-only-skipped = nothing to write, { $skipped } skipped
bake-detail-writes = { $count ->
    [one] { $count } file to write
   *[other] { $count } files to write
}
bake-detail-writes-skipped = { $count ->
    [one] { $count } file to write, { $skipped } skipped
   *[other] { $count } files to write, { $skipped } skipped
}
bake-error-read = The library couldn't be read: { $error }
bake-survey-counting = Looking through the library...
bake-survey-progress = Reading tags, { $done } of { $total }
bake-nothing-to-embed = Nothing to embed: the files already carry everything rox holds
bake-rewrites = { $count ->
    [one] { $count } file will be rewritten
   *[other] { $count } files will be rewritten
}
bake-hint-before = Press
bake-hint-key = Enter
bake-hint-after = to embed
bake-embed = Embed
bake-cancel = Cancel

## Arrange editors and header pieces

arrange-shown = Shown
arrange-hidden = Hidden
tile-face-mosaic = Cover Mosaic
tile-face-tinted = Tinted Mosaic
tile-face-gradient = Gradient Card
tile-face-color = Color Card
head-piece-artist = Artist
head-piece-album = Album
head-piece-year = Year
head-piece-genre = Genre
head-piece-quality = Quality
head-piece-tracks = Tracks
head-piece-time = Time
head-piece-spacer = Spacer
head-piece-divider = Divider
head-piece-art = Art
head-unknown = Unknown
status-item-count = Count
status-item-time = Time
status-item-albums = Albums
status-item-artists = Artists
status-item-plays = Plays
volume-item-icon = Icon
volume-item-slider = Slider
volume-item-percent = Percent

## Filter chips and search menus

filter-field-artist = Artist
filter-field-album-artist = Album Artist
filter-field-album = Album
filter-field-genre = Genre
filter-field-year = Year
filter-field-folder = Folder
filter-unknown = Unknown
filter-clear = Clear
query-show-search-box = Show Search Box
query-own-query = Own Query
query-shared-query = Shared Query
headers-off = Off
headers-compact = Compact
headers-expanded = Expanded

## Panel context menu

panel-dock-back = Dock Back
panel-pop-out = Pop Out
panel-close = Close
panel-duplicate = Duplicate
panel-reveal-in-browser = Reveal in File Browser
panel-play-next = Play Next
panel-add-to-queue = Add to Queue
panel-add-to-playlist = Add to Playlist
shader-pick-missing = { $name } (missing)
shader-pick-custom = Custom

## Shipped shader examples

shader-blurb-plasma = Drifting colour drawn from its uniforms alone, so it costs a plain quad.
shader-blurb-trails = Smears its own last frame, which puts it on the screen pass.
shader-blurb-sheen = A vignette and a drifting gleam, transparent overlay for a panel that already draws.
shader-blurb-shadow = A drop shadow the panel's own text and controls cast, read off the mask capture.
shader-blurb-cover = The playing track's art, letterboxed over a wash of its own color.
shader-blurb-badge = The cover as a small card parked in a corner, with a slot to walk it around.
shader-blurb-lamp = A light that follows the cursor and answers the buttons, transparent overlay.
shader-blurb-cube = A wireframe cube tumbling in fake 3D, drawn as added light.
shader-blurb-bloom = Drifting orbs bloomed through a half-size second pass, the chain in miniature.
shader-blurb-tube = Replays the panel under it through a curved CRT face, scanlines and all.

## Transport strip pieces

seek-item-elapsed = Elapsed
seek-item-strip = Strip
seek-item-ending = Ending
seek-item-duration = Duration
info-item-track-no = Track No
info-item-title = Title
info-item-duration = Duration
info-item-next = Next
info-item-queued = Queued
info-item-output = Output
info-item-favourite = Favourite
info-item-rating = Rating
playback-item-previous = Previous
playback-item-seek-back = Seek Back
playback-item-play = Play
playback-item-seek-forward = Seek Forward
playback-item-next = Next
playback-item-stop = Stop
playback-item-volume = Volume
playback-item-loop = Loop
playback-item-shuffle = Shuffle
playback-item-continue = Continue
playback-item-crossfade = Crossfade
playback-item-random = Random
playback-item-stop-after = Stop After
playback-item-favourite = Favourite
playback-item-rating = Rating

## Dock chrome

dock-empty-tab = Empty Tab
dock-unnamed = Unnamed
dock-tiles = Tiles
dock-zoom-in = Zoom In
dock-zoom-out = Zoom Out
dock-collapse = Collapse
dock-expand = Expand

## Shader picker notes

shader-note-empty = Pick an example to start, or point rox at a .wgsl file with a fragment stage defining fs_user(uv)
shader-note-missing = { $name } isn't in this workspace's shaders anymore, so nothing paints. Pick something else here and this panel gets a source of its own.
shader-note-shared = Shared across this workspace. Editing it updates every surface that uses it.
shader-note-file = { $path }. Your saves reload while the shader draws, and the source travels inside layouts and bundles, so it survives a machine that never had the file.
shader-note-custom = This source travels inside its layout or bundle with no file behind it. Edit as File writes it back out and picks up your saves.

## Panel pages and shared sides

panel-page-layout = Layout
panel-page-view = View
panel-page-content = Content
panel-page-source = Source
panel-page-bindings = Bindings
panel-page-emitters = Emitters
panel-page-forces = Forces
side-left = Left
side-right = Right
genre-face-mosaic = Mosaic
genre-face-tinted = Tinted
genre-face-gradient = Gradient
genre-face-color = Color

## Library panel

panel-title-library = Library
library-play = Play
library-play-album = Play Album
library-play-group = Play Group
library-play-tracks = Play { $count } Tracks
library-play-similar = Play Similar
library-filter-by-album = Filter by Album
library-filter-by-artist = Filter by Artist
library-jump-to-playing = Jump to Playing
library-menu-display = Display
library-disc = Disc { $number }
library-empty-title = Open a music folder
library-empty-note = It gets scanned into the library (flac, mp3, wav)
library-headers = Headers
    .description = Group breaks over the list; a sort keeps whatever runs stay together, searching renders flat
library-group-by = Group By
    .description = What the headers break on; genre and year re-sort the list
library-header-row = Header Row
    .description = What the one-row headers pack, left to right; a spacer or divider splits the sides
library-header-lines = Header Lines
    .description = The block's rows, top to bottom; an empty line drops out
library-follow-description = Scroll to the playing row whenever the track changes
library-resume-description = Scroll back to the playing row after you stop browsing
library-smooth-description = Glide to the row instead of jumping
library-columns = Columns
    .description = Which columns show; drag the headers in the panel to reorder and size them
library-column-headers = Column Headers
    .description = The sortable header row over the list; hide it and the columns keep their order and widths
library-compact-plays = Compact Plays
    .description = The plays column as a small count with a dash beside it
library-line-height = Line Height
    .description = One header line; blocks take the rows they need, free of the track rows
library-text-size = Text Size
    .description = The header lines' text, free of the line height, so the art grows alone
library-flush-background = Flush Background
    .description = Sit the headers on the list background instead of the raised tint; song theming moves them together
library-gap-above = Gap Above
    .description = Carved off the block's top; the list shows through, and the lines tighten to fit
library-gap-below = Gap Below
    .description = The same under the block, before its tracks
library-section-rows = Rows
library-row-height = Row Height
    .description = The track rows; the text follows, and both scale with the app font
library-row-spacing = Row Spacing
    .description = Extra height each row fills; breathing room without growing the text
library-stripes = Alternating Highlights
    .description = Tint every other track row so a long list scans
library-row-borders = Row Borders
    .description = The hairline under each track row
library-art-description = The expanded headers' tile: the cover, the artist's portrait, or the genre face
library-art-rounding = Art Rounding
    .description = Round the art's corners
library-art-position = Art Position
    .description = Which side of the block the expanded headers' tile sits on
library-art-margin = Art Margin
    .description = Inset the tile inside the block; it shrinks to keep the square
library-circular-portraits = Circular Portraits
    .description = Grouped by artist, round the tiles to the wall's full circle instead of the rounding knob
library-genre-face = Genre Face
    .description = Grouped by genre, what the tile wears: the covers, the covers washed in the genre's color, or a color card under its geometry

## Album grid panel

panel-title-album-grid = Album Grid
grid-menu-scroll = Scroll
grid-vertical-scroll = Vertical Scroll
grid-horizontal-scroll = Horizontal Scroll
grid-jump-to-playing = Jump to Playing
grid-library-empty = The library is empty
grid-play-albums = Play { $count } Albums
grid-vertical-layout = Vertical Layout
    .description = Scroll the wall up and down, rows filling the width; off scrolls it left and right, columns filling the height
grid-follow-description = Scroll to the playing album whenever the track changes
grid-resume-description = Slide back to the playing album after you stop browsing
grid-smooth-description = Glide to the album instead of jumping
grid-section-dimming = Dimming
grid-section-tiles = Tiles
grid-dim-while-playing = Dim While Playing
    .description = Fade every cover but the playing album's; hovering lights a tile back up
grid-dim-amount = Dim Amount
    .description = How far the other covers fade; 100% hides them
grid-desaturate = Desaturate While Playing
    .description = Drain every cover but the playing album's to grayscale; hovering brings a tile's color back
grid-always = Always
    .description = Keep the covers pushed back even when nothing plays; only a hovered tile shows in full
grid-show-titles = Show Titles
    .description = Print the album and artist under every cover, iTunes style, instead of only on hover
grid-title-alignment = Title Alignment
    .description = Line the captions up under their covers
grid-tile-size = Tile Size
    .description = The cover tiles' widest edge; columns split the panel width evenly
grid-gap = Gap
    .description = Space between the covers; zero packs them edge to edge
grid-art-rounding-description = Round each cover's corners; 100% is a circle
