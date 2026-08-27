### The source locale. Every key is defined here first and the other
### locales match it one for one, which the parity test in rox-i18n
### enforces. Keys are surface-prefixed kebab-case; a row's description
### is an attribute on the label's message.

## Shared widgets

# The tracking section in every scrolling panel's customize window.
# What each toggle follows is worded by the panel itself and passed in.
tracking-title = Tracking
tracking-follow = Follow Playing
tracking-resume = Resume When Idle
tracking-smooth = Smooth Scrolling
align-row = Alignment
    .description = Where the content goes when the panel has room to spare
valign-row = Vertical Alignment
    .description = Where the content goes when the panel has height to spare
valign-top = Top
valign-middle = Middle
valign-bottom = Bottom

## Panel source and search rows

source-track = Track
    .description = Follow what's playing, or what's selected in the library
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
    .description = What the signal follows: Band tracks one frequency range, Level the whole mix, Onset pulses on each hit in the range, Trigger fires a pulse when the range reaches its threshold, Total adds up another signal over time
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
signal-threshold-gate = Under this the signal reads as nothing, and above it the output climbs from zero again, so quiet parts don't move the knob. The mark on the meter above shows where it is
signal-low-bound = Low Bound
signal-high-bound = High Bound
signal-adds-up = Adds Up
    .description = Which signal this totals; it climbs while that one reads high and stalls while it's quiet
signal-aggregate-nothing = Nothing to follow
signal-aggregate-pick = Pick a signal
signal-aggregate-alone = There's no other signal in the pool to add up, so this stays at zero. Add one and it shows up in the list.
signal-aggregate-unpicked = Nothing picked, so this total stays at zero. Pick a signal above.
signal-rate = Rate
    .description = Wraps per second at full input; it rolls over 1 back to 0 and keeps climbing, which a shader reads as a phase
signal-reset-on-track = Reset on Track
    .description = Drain back to zero when a new song starts, so a phase doesn't begin from the last one's total
signal-flush = Flush
## How many of a panel's bindings use one signal.
signal-routes-in-panel = { $count ->
    [one] { $count } route in this panel
   *[other] { $count } routes in this panel
}
    .description = Send it back to zero now. It drains over a moment rather than snapping, so nothing following it jumps
route-header = Route
route-signal = Signal
    .description = Which shared signal this route follows; tuning it here tunes every route on it
route-new-signal = New Signal
route-shared-note = Shared by every route on this signal
route-signal-gone = This route's signal is gone; the knob holds its slider value until another is picked above.
route-range-note = Range for this parameter only
route-quiet = Quiet
    .description = What the knob reads at silence, as a share of its own setting
route-loud = Loud
    .description = What it reads at full signal; 100% is the slider's own value, below Quiet modulates down
route-slot = Slot
    .description = Which of the shader's sixteen signal slots this route fills
route-slot-quiet-description = What the slot reads at silence
route-slot-loud-description = What it reads at full signal; below Quiet runs the slot backwards
route-slot-signal-description = Which shared signal this route follows
route-slot-signal-gone = This route's signal is gone; the slot reads zero until another is picked.
route-add = Add Route
route-unrouted = Unrouted
route-pick-slot = Pick a slot
route-pick-signal = Pick a signal
route-no-signal = no signal
route-no-signals-yet = There are no signals to follow yet. Make one and it shows up here; until then the slot reads zero.
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
    .description = Pin the panel in place; it can't be dragged or rearranged in the dock
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
    .description = Keep drawing frames while the audio is silent. Off, the shader freezes on its last frame and the panel costs nothing
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
preset-back-tail = in any panel menu. Presets belong to this workspace only; another workspace won't have them.

## Keyboard hints

hint-press = Press
hint-key-enter = Enter

## Settings: language

settings-language = Language
    .description = The interface language. System matches against the OS's list and falls back to English when nothing matches
    .keywords = translation locale language
settings-language-system = (System Language)
settings-language-search = Search languages
picker-no-matches = No matches
settings-search-no-matches = Nothing matches "{ $text }"

## Embed dialog

bake-window-title = rox - Embed Stored Metadata
bake-title = Embed Stored Metadata
bake-intro = Writes stored metadata into the files themselves, so another player reads it too. Nothing is recalculated.
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
bake-nothing-to-embed = Nothing to embed: the files already have everything rox has stored
bake-rewrites = { $count ->
    [one] { $count } file will be rewritten
   *[other] { $count } files will be rewritten
}
bake-hint-before = Press
bake-hint-key = Enter
bake-hint-after = to embed
bake-embed = Embed
bake-cancel = Cancel
## The one-line report after an embed run. All three numbers show, zeros
## included: the skips are most of what someone wants to know afterwards.
## The head is built first, then the two tails append, so each count sits
## in a message of its own and a locale that inflects around it can select
## on that count without touching the others.
bake-summary-files = { $count ->
    [one] { $count } file
   *[other] { $count } files
}
bake-summary-updated = { $files } updated
bake-summary-stopped = Stopped after { $files } updated
bake-summary-skipped = , { $count } skipped
bake-summary-failed = , { $count } failed

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
panel-favourite-add = Add to Favourites
panel-favourite-remove = Remove from Favourites
shader-pick-missing = { $name } (missing)
shader-pick-custom = Custom

## Shipped shader examples

shader-blurb-plasma = Drifting colour drawn from its uniforms alone, so it costs a plain quad.
shader-blurb-trails = Smears its own last frame, so it runs on the screen pass.
shader-blurb-sheen = A vignette and a drifting gleam, transparent overlay for a panel that already draws.
shader-blurb-shadow = A drop shadow the panel's own text and controls cast, read off the mask capture.
shader-blurb-cover = The playing track's art, letterboxed over a wash of its own color.
shader-blurb-badge = The cover as a small card parked in a corner, with a slot to move it around.
shader-blurb-lamp = A light that follows the cursor and responds to clicks, transparent overlay.
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
shader-note-file = { $path }. Your saves reload while the shader draws, and the source is stored inside layouts and bundles, so it still works on a machine that never had the file.
shader-note-custom = This source is stored inside its layout or bundle with no file behind it. Edit as File writes it back out and picks up your saves.

## Panel pages and shared sides

panel-page-layout = Layout
panel-page-view = View
panel-page-content = Content
panel-page-source = Source
panel-page-bindings = Bindings
panel-page-emitters = Emitters
panel-page-forces = Forces
panel-page-scene = Scene
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
    .description = Group breaks over the list; a sort keeps whatever runs there are together, searching renders flat
library-group-by = Group By
    .description = What the headers break on; genre and year re-sort the list
library-header-row = Header Row
    .description = What the one-row headers show, left to right; a spacer or divider splits the sides
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
    .description = One header line; blocks take the rows they need, independent of the track rows
library-text-size = Text Size
    .description = The header lines' text, independent of the line height, so the art grows alone
library-flush-background = Flush Background
    .description = Put the headers on the list background instead of the raised tint; song theming moves them together
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
    .description = Which side of the block the expanded headers' tile goes on
library-art-margin = Art Margin
    .description = Inset the tile inside the block; it shrinks to keep the square
library-circular-portraits = Circular Portraits
    .description = Grouped by artist, round the tiles to the wall's full circle instead of the rounding knob
library-genre-face = Genre Face
    .description = Grouped by genre, what the tile shows: the covers, the covers washed in the genre's color, or a color card under its geometry

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

## Settings: sidebar pages

settings-page-appearance = Appearance
settings-page-application = Application
settings-page-audio = Audio
settings-page-development = Development
settings-page-integrations = Integrations
settings-page-keymap = Keymap
settings-page-library = Library
settings-page-mcp = MCP
settings-page-ml-models = ML Models
settings-page-playback = Playback
settings-page-providers = Providers
settings-page-shader = Shader
settings-page-storage = Storage
settings-page-workspace = Workspace

## Settings: appearance

settings-appearance-backdrop-all-windows = All Windows
    .description = Back the child windows too: settings, editors, dialogs, popped-out panels. Off keeps the backdrop and the transparency to the workspace windows
settings-appearance-backdrop-strength = Backdrop Strength
    .description = How strongly the cover backdrop shows behind them
settings-appearance-border = Border
    .description = A line around every panel's edge, in the Border role's color; a side at zero draws none
settings-appearance-colors-locked-note = Song theming is on, so the playing track drives these colors and export saves them. Turn it off above to edit them
settings-appearance-design-mode = Design Mode
    .description = Edit the layout in place: the panel menus' add, rename, duplicate, pop out and close rows, the controls a container floats over its slots, and tab dragging. Off hides all of that; the Workspace page still edits the tree
    .keywords = edit layout rearrange lock
settings-appearance-font = Font
    .description = The app-wide typeface; panels can override it in their own settings
    .keywords = typeface family text
settings-appearance-font-size = Font Size
    .description = The base text size every panel's text scales from; controls and icons hold their size
settings-appearance-hide-menubar = Hide Menubar
    .description = Keep the menubar hidden, floating it over the dock while alt is held. Double-tap alt to leave it up, so its buttons take a plain click
settings-appearance-icons-intro = A pack is a folder of SVGs that replaces the built-in icons; switching takes effect on the next launch
settings-appearance-icons-open-folder = Open Folder
settings-appearance-inverse-from-dark = Inverse From Dark Theme
settings-appearance-inverse-from-light = Inverse From Light Theme
settings-appearance-keep-theme = Keep Theme
    .description = Hold the active theme even when a cover's brightness would flip it; song theming still tints the color
settings-appearance-margin = Margin
    .description = Pull every panel in from its cell; a panel can override this in its own settings
settings-appearance-new-pack = New Pack
settings-appearance-os-decorations = OS Decorations
    .description = The OS titlebar and borders on the main windows; off relies on the window controls and drag anchor panels
settings-appearance-pack-name-placeholder = Pack name
settings-appearance-padding = Padding
    .description = Space inside every panel's edge, kept in its own background
settings-appearance-palette-export = Export
settings-appearance-palette-import = Import
settings-appearance-panel-seams = Panel Seams
    .description = The hairline between panel tiles; off leaves the resize grips invisible but still draggable
settings-appearance-resize-border = Resize Border
    .description = Resizing the main windows by dragging their edges; only applies with OS Decorations off, and switching it off leaves snapping and Win+arrow as the way to resize
settings-appearance-rounding = Rounding
    .description = Round every panel's corners off into the backdrop
settings-appearance-section-colors = Colors
settings-appearance-section-frame = Frame
settings-appearance-section-icons = Icons
settings-appearance-section-interface = Interface
settings-appearance-section-theming = Theming
settings-appearance-section-transparency = Transparency
settings-appearance-section-typography = Typography
settings-appearance-song-theming = Song Theming
    .description = Tint the palette and back windows with the playing track's cover art
settings-appearance-surface-opacity = Surface Opacity
    .description = How opaque the app's surfaces read over the backdrop
settings-appearance-theme = Theme
    .description = The palette the app renders and the one the color editor below targets; System follows the OS's light or dark preference
settings-appearance-theme-dark = Dark
settings-appearance-theme-light = Light
settings-appearance-theme-system = System

## Settings: application

settings-application-check-updates = Check for Updates
    .description = Look for a newer release once a day when rox starts; the About window checks now either way
settings-application-download-updates = Download Updates
    .description = When a check finds a newer release, download and stage it in the background; the next start runs it
settings-application-enable-ai = Enable AI Features
    .description = Let AI tooling talk to rox: adds MCP support, and the ML model downloads, with their pages joining the sidebar.
settings-application-lock-panel-resize = Lock Panel Resize
    .description = Panel splits only resize while Design Mode is on, so a drag near a seam can't nudge a finished layout
settings-application-portable-copying = Copying data...
settings-application-portable-mode = Portable Mode
    .description = Keep settings, library, and caches in a rox-data folder beside the executable, so the player moves with its data. Turning it off goes back to the system folder and leaves rox-data in place
settings-application-portable-not-writable = The app's folder is not writable
settings-application-portable-restart-note = Applies on the next launch; this run stays on its current folder
settings-application-remain-in-tray = Remain in Tray
    .description = Keep the music playing when the last window closes, with the tray icon (the dock on macOS) as the way back in
settings-application-section-ai = AI
settings-application-section-control-socket = Control Socket
settings-application-section-data = Data
settings-application-section-layout = Layout
settings-application-section-startup = Startup
settings-application-section-window = Window
settings-application-socket-path = Socket Path
    .description = rox's machine interface while it runs: JSON-RPC over a local socket, keyed to this data folder. roxctl drives it from a shell, and the rox-mcp proxy serves MCP clients over it

## Settings: audio

settings-audio-broadcast-bitrate = Bitrate
    .description = What the MP3 encoder spends per second of stream
settings-audio-broadcast-enable = Stream to Icecast
    .description = Push what rox plays to an icecast server as a source client, encoded to MP3. The mount, the listeners, and the network face all belong to icecast; rox only connects out, and an unreachable server never touches local playback
settings-audio-broadcast-host-placeholder = icecast host
settings-audio-broadcast-login = Source Login
    .description = icecast's source credentials, the user and password its config names
settings-audio-broadcast-mount = Mount
    .description = The mount listeners tune to, and the stream name it advertises
settings-audio-broadcast-name-placeholder = Stream name
settings-audio-broadcast-password-placeholder = Source password
settings-audio-broadcast-server = Server
    .description = The icecast server's host and port; the source protocol runs over a plain socket
settings-audio-broadcast-user-placeholder = source
settings-audio-crossfade = Crossfade
    .description = How long a track overlaps the one after it. The fade is for shuffle and skipping, so an album's own boundaries stay untouched unless the row below says otherwise. Zero turns it off
    .keywords = gapless overlap transition fade
settings-audio-equalizer-note = Ten octave bands over the output. It opens in its own window, since it's worked while the music plays rather than set once
settings-audio-exclusive-mode = Exclusive Mode
    .description = Claim the device for rox alone and run it at the file's own rate where the hardware takes one; off shares the system mixer with everything else on the desktop
settings-audio-fade-inside-albums = Fade Inside Albums
    .description = Overlap tracks that belong to the same record as well. Off keeps a record's own splices exactly as they were mastered, which is where gapless matters most
settings-audio-open-equalizer = Open Equalizer
settings-audio-output-buffer = Buffer
    .description = How much audio the card holds at a time. Shorter reacts quicker and crackles sooner on a busy machine; longer is safer and lazier
settings-audio-output-buffer-default = Default (10 ms)
settings-audio-output-device = Device
    .description-default = The system default follows whatever the desktop is set to
    .description-linux = Exclusive claims a card straight from the kernel, so the list is sound cards rather than the desktop's outputs. Bluetooth and other sound-server devices have no card to claim and only show with exclusive off
    .description-other = Exclusive takes the device for rox alone, so nothing else on the desktop can sound through it until the mode is off
settings-audio-output-device-system-default = System Default
settings-audio-output-experimental-badge = Experimental
settings-audio-output-experimental-tooltip = This platform's exclusive backend is written from the platform's documented audio contract but has never been run on real hardware by the developers. It should claim the device or fall back to shared with a reason, never go silent. If it misbehaves, turn it off and report what happened with the button beside this badge.
settings-audio-output-format = Format
    .description = What rox hands the card. A card that won't take the pick runs the widest format it has, and the status below shows which
settings-audio-output-format-f32 = 32-bit float
settings-audio-output-format-s16 = 16-bit integer
settings-audio-output-format-s32 = 32-bit integer
settings-audio-output-format-widest = Widest available
settings-audio-output-issue-tooltip = Report how exclusive mode behaved on this machine. Opens a GitHub issue with the platform and the negotiated stream filled in.
settings-audio-output-mode-exclusive = Exclusive
settings-audio-output-mode-shared = Shared
settings-audio-output-not-built = Not built for this platform yet
settings-audio-output-rate-follow = Follow the file
settings-audio-output-sample-rate = Sample Rate
    .description = Following reopens the device at each file's own rate, which costs a gap at a boundary where the rate changes; pinning one rate never pays that and resamples anything that doesn't match
settings-audio-output-status-error-hint = Pick another device, or turn exclusive off
settings-audio-output-status-error-title = No output
settings-audio-output-status-idle-hint = Start a track to see the format the device accepted
settings-audio-output-status-idle-title = Nothing playing
settings-audio-replaygain-level-by = Level By
    .description = Play every track at the loudness its ReplayGain tags measured, so a shuffle stops jumping between masters. Track levels each file on its own; Album uses the record's gain across all its tracks, which keeps an album's own quiet and loud passages where they were put
    .keywords = normalization loudness leveling volume
settings-audio-replaygain-measure-missing-button = Measure Missing
settings-audio-replaygain-measure-new = Measure New Files
    .description = Measure what the watcher brings in as it arrives, once the sync has settled, so a library that grows keeps its gains without a trip back here. The numbers go wherever Save Measured Gains points. Turning this on offers to measure what's already missing first; after that it only ever sees files that just arrived
settings-audio-replaygain-measuring-progress = Measuring { $done } of { $total }
settings-audio-replaygain-measuring-start = Measuring: working out what's missing...
settings-audio-replaygain-mode-album = Album
settings-audio-replaygain-mode-off = Off
settings-audio-replaygain-mode-track = Track
settings-audio-replaygain-preamp = Preamp
    .description = Added to every tagged gain. ReplayGain's reference is below where modern records are cut, so a levelled library plays quieter than the same library raw; this is where that comes back. A boost never clips: the tagged peak caps it
settings-audio-replaygain-save = Save Measured Gains
    .description = Where the measurement pass puts its numbers. The library database keeps your files untouched; tags put the same values where every other player reads them, at the cost of rewriting the audio files
settings-audio-replaygain-status-measured = { $total ->
    [one] The one scanned track has a gain to level by, { $measured } of them measured by rox
   *[other] All { $total } scanned tracks have a gain to level by, { $measured } of them measured by rox
}
settings-audio-replaygain-status-tagged = { $total ->
    [one] The one scanned track has ReplayGain tags
   *[other] All { $total } scanned tracks have ReplayGain tags
}
settings-audio-replaygain-untagged = Untagged Files
    .description = What a file with no ReplayGain tags plays at. Nothing measured it, so this is a guess standing in for one. Leave it at zero and untagged tracks play as they always did
settings-audio-section-broadcast = Broadcast
settings-audio-section-equalizer = Equalizer
settings-audio-section-output = Output
settings-audio-section-playback = Playback
settings-audio-section-replaygain = ReplayGain
settings-audio-transport = Transport
    .description = Start and stop without leaving this page, since every setting below is judged by ear

## Settings: integrations

settings-integrations-discord-enable = Enable Rich Presence
    .description = Show rox activity on Discord when playing music
settings-integrations-discord-show-lastfm = Show Last.fm Button
    .description = Include a clickable 'View on Last.fm' button in Discord status
settings-integrations-discord-show-youtube = Show YouTube Button
    .description = Include a clickable 'Search on YouTube' button in Discord status
settings-integrations-ffmpeg-binary = FFmpeg Binary
    .description = Which ffmpeg runs conversions; leave empty for the one on PATH
settings-integrations-ffmpeg-fail-note = Convert stays hidden until ffmpeg is set to a working binary
settings-integrations-ffmpeg-fail-title = This ffmpeg didn't run
settings-integrations-ffmpeg-missing-note = Convert stays hidden; install ffmpeg or point the path at a binary
settings-integrations-ffmpeg-missing-title = No working ffmpeg found
settings-integrations-ffmpeg-ok-note = ffmpeg works. Convert is available.
settings-integrations-ffmpeg-test = Test
settings-integrations-lastfm-api-key-row = API Key
settings-integrations-lastfm-connect = Connect
settings-integrations-lastfm-disconnect = Disconnect
settings-integrations-lastfm-finish-connecting = Finish Connecting
settings-integrations-lastfm-hearts = { $n ->
    [one] { $n } heart
   *[other] { $n } hearts
}
settings-integrations-lastfm-import-loved = Import Loved Tracks
settings-integrations-lastfm-intro-builtin = Connect your Last.fm account: authorize rox in the browser and played tracks scrobble to it
settings-integrations-lastfm-intro-custom = This build ships no api identity, so scrobbling needs your own api account (Last.fm/api/account/create); paste its key and shared secret, then connect
settings-integrations-lastfm-key-placeholder = API key
settings-integrations-lastfm-love-failed = Last one failed: { $error }
settings-integrations-lastfm-love-pending = { $hearts } waiting to send
settings-integrations-lastfm-love-pending-failed = { $hearts } waiting to send, last attempt: { $error }
settings-integrations-lastfm-reconnect = Reconnect
settings-integrations-lastfm-secret-placeholder = Shared secret
settings-integrations-lastfm-secret-row = Shared Secret
settings-integrations-lastfm-status-confirming = Confirming...
settings-integrations-lastfm-status-connected = Connected as { $username }
settings-integrations-lastfm-status-elsewhere = Connected on another install of rox; each one authorizes under its own api identity, so connect this one too
settings-integrations-lastfm-status-failed = Connection failed: { $error }
settings-integrations-lastfm-status-not-connected = Not connected
settings-integrations-lastfm-status-rejected = Last.fm rejected the session and it was dropped. Connect again to keep scrobbling
settings-integrations-lastfm-status-requesting = Requesting a token...
settings-integrations-lastfm-status-waiting = Authorize rox in the browser, then finish connecting
settings-integrations-lastfm-working = Working...
settings-integrations-love-favourites = Love Favourites
    .description = Mirror hearts to Last.fm as loved tracks; taking a heart back unloves it there
settings-integrations-scrobble-threshold = Scrobble Threshold
    .description = How much of a track has to play before it scrobbles; the seek strip and waveform can mark it
settings-integrations-scrobble-tracks = Scrobble Tracks
    .description = Send played tracks to Last.fm once they cross the threshold
settings-integrations-section-conversion = Conversion
settings-integrations-section-discord = Discord Rich Presence
settings-integrations-section-favourites = Favourites
settings-integrations-section-lastfm = Last.fm
settings-integrations-section-scrobbling = Scrobbling

## Settings: keymap

settings-keymap-clash = { $chord } is also { $other }; only one of them will fire
settings-keymap-not-bound = Not bound
settings-keymap-recording = Press the keys
settings-keymap-restore = Restore
settings-keymap-restore-all = Restore Every Chord
    .description = Put every command back on the keys it ships with, including any this build no longer has a row for
settings-keymap-section-defaults = Defaults
settings-keymap-undo = Undo
settings-keymap-undo-last = Undo the Last Reset
    .description = Bring back the chords the last reset threw out, row or all

## Settings: library

settings-library-acoustic-all-described = { $total ->
    [one] The one scanned track is described by { $label }
   *[other] All { $total } scanned tracks are described by { $label }
}
settings-library-acoustic-auto = Describe New Files
    .description = Describe what the watcher brings in as it arrives, once the sync has settled, so a library that grows keeps its descriptions without a trip back here. Off, new files wait for the Analyze Missing button. Turning this on offers to analyze what's already missing first; after that it only ever sees files that just arrived
settings-library-acoustic-enable = Describe How Tracks Sound
    .description = Work out what each track sounds like, so the library can find music that resembles what's playing. Everything runs on this machine, and describing a large library takes a while
    .keywords = similar sound fingerprint describe
settings-library-acoustic-extractor = Extractor
settings-library-acoustic-extractor-model = Model
settings-library-acoustic-fallback = Analyzing
settings-library-acoustic-partial = { $label } describes { $done } of { $total } scanned tracks. Analyze Missing works through the rest
settings-library-acoustic-progress = { $running } is on { $done } of { $total }
settings-library-acoustic-progress-start = { $running }: working out what's missing...
settings-library-acoustic-save = Save Descriptions
    .description = Where the pass puts what it works out. The database alone keeps your files untouched; tags put a copy in each file as well, so the descriptions are kept if the library is rebuilt or the folder moves to another machine, at the cost of rewriting the audio files. Tags work for MP3 and FLAC only; every other format keeps the database copy
settings-library-add-folder = Add Folder
settings-library-duplicates = Duplicates...
settings-library-embed-button = Embed Stored Metadata...
settings-library-folder-col-albums = Albums
settings-library-folder-col-folder = Folder
settings-library-folder-col-size = Size
settings-library-folder-col-tracks = Tracks
settings-library-folders-intro = Folders scanned into the library; removing one drops its tracks from the catalog and leaves the files alone
settings-library-genre-separator-nudge = Separators changed: browsing follows right away. Genre lists stored by earlier scans keep their old shape until you hit Rescan up in the Folders header
settings-library-merge-case = Merge case variants
    .description = Treat values differing only by case as one: Rock and rock become the same genre, artist, and album, shown under the casing most tracks use. Files keep their tags as written
settings-library-no-folders = No folders yet
settings-library-repair-tags = Repair Tags...
settings-library-section-folders = Folders
settings-library-section-stored-metadata = Stored Metadata
settings-library-section-tempo = Tempo Analysis
settings-library-split-genres = Split genres on commas and slashes
    .description = "Dubstep, Trap" and "Drum & Bass / Neurofunk" count each value as its own genre; semicolons always split. Off keeps slashed names whole for tags where they mean one genre. Files keep their tags as written
settings-library-tempo-auto = Time New Files
    .description = Count the beats in what the watcher brings in as it arrives, once the sync has settled, so a library that grows keeps its tempos without a trip back here. Off, new files wait for the Analyze Missing button. Turning this on offers to time what's already missing first; after that it only ever sees files that just arrived
settings-library-tempo-enable = Work Out How Fast Tracks Run
    .description = Count the beats in tracks whose tags don't say, so the library can show and sort by tempo. Everything runs on this machine, the numbers go in the library database, and your files are untouched
settings-library-tempo-progress = Timing { $done } of { $total }
settings-library-tempo-progress-start = Working out what's missing...
settings-library-tempo-status-measured = { $total ->
    [one] The one scanned track has a tempo, { $measured } of them worked out by rox
   *[other] All { $total } scanned tracks have a tempo, { $measured } of them worked out by rox
}
settings-library-tempo-status-tagged = { $total ->
    [one] The one scanned track has a tempo tag
   *[other] All { $total } scanned tracks have a tempo tag
}
settings-library-watch-folders = Watch folders
    .description = Fold added, edited, and deleted files into the library as they happen, without a manual rescan
settings-library-write-stored = Write What's Stored Into the Files
    .description = The three save settings only apply to the next write, so anything saved before one was switched to Tags is still in rox alone. This writes the lyrics, gains and descriptions rox already holds into the files themselves, so another player reading the folder sees them. Nothing is recalculated

## Settings: MCP

settings-mcp-client-config = Client Config
    .description = Paste into an MCP client's server list (Claude Code, Claude Desktop, or any other) to let it query rox about the library, what's playing, and the transport. rox has to be running; the tools run over its control socket
settings-mcp-enable = Enable MCP Server
    .description = Respond to tool calls from connected MCP clients. The proxy checks this on every call, so while it's off clients are refused with the reason; the config below can be set up either way

## Settings: ML models

settings-mlmodels-checking = Checking...
settings-mlmodels-choose-file = Choose File
settings-mlmodels-custom-description-empty = Point rox at a PANNs CNN10 checkpoint of your own, as safetensors. It's read in place and named by its hash, so a second checkpoint describes the library separately rather than reusing the first one's coordinates
settings-mlmodels-download-failed = { $label } could not be downloaded: { $reason }
settings-mlmodels-downloading = Downloading { $label }: { $done } of { $total }
settings-mlmodels-stopping = Stopping the { $label } download...
settings-mlmodels-fallback-model = model
settings-mlmodels-fallback-the-model = The model
settings-mlmodels-kind-custom = Custom
settings-mlmodels-kind-recommended = Recommended
settings-mlmodels-pass-stopped = The last pass stopped: { $reason }
settings-mlmodels-weights-file = Weights File

## Settings: playback

settings-playback-continuation-continue = Continue
    .description = Carry on down the list you started from, then the rest of the library behind it. Play an album from the middle of a view and the view keeps going
settings-playback-continuation-off = Off
    .description = Nothing refills the queue; playback stops at its end
settings-playback-continuation-weighted = Weighted
    .description = Draw from the whole library, what you've never played first and what you heard recently last
settings-playback-keep-playing = Keep Playing
    .description = What plays when the queue runs out. Whatever this picks is appended to the timeline as ordinary context, so it's visible and removable rather than hidden state. With the order above set to Similar it keeps finding tracks that sound like the one playing, whichever of these is chosen
    .keywords = continuation refill autoplay queue
settings-playback-play-order = Play Order
    .description = How the tracks already queued are arranged while shuffle is on. The transport's shuffle button turns it on and off; this sets what it does once it's on
settings-playback-rating-scale = Rating Scale
    .description = Stars for quick clicks, 0-10 in half steps for finer review scores
settings-playback-rating-scale-numeric = 0-10
settings-playback-rating-scale-stars = Stars
settings-playback-restore-last-session = Restore Last Session
    .description = Launch with the play queue as you left it, paused on the track that was playing and where it left off. Queued tracks outside your library folders can't be restored and drop from the order
settings-playback-section-queue = Queue
settings-playback-section-ratings = Ratings
settings-playback-section-startup = Startup
settings-playback-shuffle-random = Random
    .description = The shuffle everyone means by the word. What's coming plays in no particular order
settings-playback-shuffle-similar = Similar
    .description = Nearest first by sound. What's coming is sorted by how much it resembles the track that was playing when you turned it on, and re-sorted on every skip. Needs the library described on the Library page
settings-playback-unrated-dots = Unrated Dots
    .description = Mark unfilled star slots with a faint dot instead of leaving them empty

## Settings: providers

settings-providers-artist = Last.fm
    .description = Fetch artist biographies, stats, and similar artists for the biography panel, with a portrait from Deezer; everything is kept in the data folder and reads offline afterwards
settings-providers-deezer = Deezer
    .description = Search Deezer for cover art, up to 1000 pixels
settings-providers-itunes = iTunes
    .description = Search iTunes for cover art; the cover editor's search shows matches to pick before setting
settings-providers-lastfm-art = Last.fm
    .description = Search Last.fm for cover art
settings-providers-lrclib = LRCLIB
    .description = Fetch missing lyrics from lrclib.net, synced sheets when it has them
settings-providers-lyrics-intro = Online lookups run only when a panel action asks for one; playback and browsing never touch the network
settings-providers-musicbrainz = MusicBrainz
    .description = Look up tags on musicbrainz.org; the metadata panel's search shows matches to confirm field by field before writing
settings-providers-save-lyrics = Save Fetched Lyrics
    .description = Where a fetched sheet goes: rox's own data folder keeping the library clean, an .lrc next to the track, or the embedded tag
settings-providers-save-lyrics-data-folder = Data Folder
settings-providers-save-lyrics-sidecar = Sidecar
settings-providers-save-lyrics-tag = Tag
settings-providers-section-artist = Artist
settings-providers-section-cover-art = Cover Art
settings-providers-section-lyrics = Lyrics
settings-providers-section-metadata = Metadata

## Settings: shader

settings-shader-backdrop-all-windows = All Windows
    .description = Shade every window's backdrop: settings, editors, dialogs, popped-out panels. Off keeps it to the workspace windows
settings-shader-backdrop-enabled = Backdrop Shader
    .description = Run a music-reactive WGSL shader over the album-art backdrop, under every panel. Part of the workspace, so it travels with the look
settings-shader-backdrop-fallback-name = Backdrop
settings-shader-backdrop-run-idle = Run While Idle
    .description = Keep drawing with nothing playing. The animation stays frozen either way
settings-shader-compile-error-title = This shader didn't compile
settings-shader-legacy-note = With nothing routed the pool fills the slots in its own order: the first signal into slot 0, the second into slot 1, and so on. The first route you add takes over the whole mapping.
settings-shader-overlay-enabled = Overlay Shader
    .description = Run a music-reactive WGSL shader over the whole window. Only shaders that leave the app usable underneath are offered
settings-shader-scene-covers-window = This shader is a scene, so it covers the window rather than drawing over it. It came from a bundle or an older config; the list above only offers shaders that leave the app usable.
settings-shader-screen-all-windows = All Windows
    .description = Shade the child windows too: settings, stats, equalizer, popped-out panels. The revert countdown stays unshaded either way
settings-shader-screen-fallback-name = Screen
settings-shader-screen-run-idle = Run While Idle
    .description = Keep drawing with nothing playing. The animation stays frozen either way. A shader that reads the mouse follows the cursor with the music stopped without this; it just stops a couple of seconds after the pointer does
settings-shader-section-backdrop = Backdrop Shader
settings-shader-section-overlay = Overlay Shader
settings-shader-signals-block = Signals
    .description = Which shared signal each of the shader's sixteen slots reads
settings-shader-slots-block = Slots
    .description = Each slot as the shader receives it; slots without a route are hand-set knobs

## Settings: storage

settings-storage-artist-images = Artist Images
    .description = Portraits, banners and biographies fetched for the artist views (artists/); cleared ones are fetched again the next time a view opens
settings-storage-catalog = Catalog
    .description = The track index scans build: a row a track with its tags, its file details and any cue spans, inside library.db
settings-storage-cover-thumbnails = Cover Thumbnails
    .description = Small covers kept after their first render (thumbs.db); cleared ones rebuild as they scroll into view
settings-storage-logs = Logs
    .description = What each run writes for bug reports (logs/rox.log), rolled at a size cap so it never grows large
settings-storage-looks-layouts = Looks and Layouts
    .description = The look the app is using (workspace.json) with your saved workspaces, ejected shader files and icon packs beside it. Small, and every byte of it is something you set up
settings-storage-lyrics = Lyrics
    .description = Fetched and edited sheets kept in the app's own store (lyrics/), so library folders stay clean
settings-storage-measured-tempos = Measured Tempos
    .description = The tempos rox counted from the audio, for tracks whose tags have none; the tags' own numbers aren't touched. Clearing puts those tracks back on Analyze Missing's list on the Library page, so improved beat counting can replace numbers an older pass wrote
settings-storage-model-fallback-this = This model
## The library's line on the storage page. The two counts come in already
## worded by status-count-tracks and status-count-albums, so they stay
## plural-correct without this message selecting on anything itself.
settings-storage-music-summary = { $tracks }, { $albums }, { $size }
settings-storage-model-weights = Model Weights
    .description = The models downloaded for acoustic analysis (models/). The ML Models page is where they're fetched and deleted, one row a model
settings-storage-models-empty = Models
    .description = Nothing has described the library yet. Turning on acoustic analysis on the Library page fills this in, and every model that has run gets a row here
settings-storage-music-files = Music Files
    .description = What the scanned folders hold; the files stay where they are
settings-storage-none = None
settings-storage-playlists-history = Playlists and History
    .description = Your playlists and their members, what you've played, and the library's genre notes. All of it small next to the rest of library.db
settings-storage-reclaimable = Reclaimable Space
    .description = Pages inside library.db that deletes left behind. New writes fill them again, so the file stops growing before it starts shrinking
    .keywords = vacuum compact shrink database
settings-storage-section-acoustic = Acoustic Descriptions
settings-storage-section-app-data = App Data
settings-storage-section-caches = Caches
settings-storage-section-diagnostics = Diagnostics
settings-storage-section-library = Library
settings-storage-section-tempo = Tempo
settings-storage-vectors = Vectors
    .description = What every description weighs inside library.db. On a library the analysis pass has been through this is most of the file, a couple of kilobytes a track against a few hundred bytes of tags
settings-storage-waveforms = Waveforms
    .description = Each track's peak strip, kept after its first play; cleared ones re-decode next play

## Settings: workspace

settings-workspace-card-author = Author
settings-workspace-card-author-placeholder = Who made it
settings-workspace-card-created = Created { $date }
settings-workspace-card-created-updated = Created { $created }, updated { $updated }
settings-workspace-card-description = Description
settings-workspace-card-description-placeholder = What the look is going for
settings-workspace-card-empty = This workspace has no card
settings-workspace-card-hint = The card is stored in the file, so whoever you share this look with sees it
settings-workspace-card-license = License
settings-workspace-card-license-placeholder = The terms you share it under
settings-workspace-card-save = Save Card
settings-workspace-card-updated = Updated { $date }
settings-workspace-card-version = Version
settings-workspace-card-version-placeholder = Your own version, whatever you count in
settings-workspace-card-website = Website
settings-workspace-card-website-placeholder = Where it lives
settings-workspace-composition-closed = The workspace window is closed
settings-workspace-composition-hint = The window's panels as they're arranged in splits and tab groups; the arrows reorder a row among its siblings, the lock pins a panel in place, and the gear opens its settings
settings-workspace-empty = No workspaces yet
settings-workspace-hint = A workspace is a whole look: layouts, palette, appearance. Applying one replaces all three
settings-workspace-layout-name-placeholder = Layout name
settings-workspace-layouts-empty = No layouts yet
settings-workspace-layouts-hint = Primary and mini are the two the menubar's mini-player button swaps between
settings-workspace-name-placeholder = Workspace name
settings-workspace-panel-preset-unknown-kind = Unknown panel
settings-workspace-panel-presets-empty = No panel presets yet
settings-workspace-panel-presets-hint-after = in any panel menu. They belong to this workspace only; another workspace won't have them.
settings-workspace-panel-presets-hint-before = One configured panel each, saved from a panel's own menu and added back from
settings-workspace-role-mini = Mini
settings-workspace-role-primary = Primary
settings-workspace-section-composition = Composition
settings-workspace-section-layouts = Layouts
settings-workspace-section-panel-presets = Panel Presets
settings-workspace-section-workspaces = Workspaces
settings-workspace-tree-empty-slot = Empty slot
settings-workspace-tree-split-column = Split, stacked
settings-workspace-tree-split-row = Split, side by side
settings-workspace-tree-tabs = Tabs

## Settings: development

settings-development-experimental-panels = Experimental Panels
    .description = Show the panels still being built in the Panels menu and the launcher; they change shape between releases, and a layout that already holds one keeps it when this goes back off
settings-development-section-features = Features

## Settings: shared

settings-acoustic-analysis-heading = Acoustic Analysis
settings-analyze-nothing-scanned = Nothing scanned to analyze yet
settings-common-active = Active
settings-common-analyze-missing = Analyze Missing
settings-common-built-in = Built-in
settings-common-clear = Clear
settings-common-copy = Copy
settings-common-database = Database
settings-common-delete = Delete
settings-common-download = Download
settings-common-rescan = Rescan
settings-common-reveal = Reveal
settings-common-stop = Stop
settings-common-stopping = Stopping...
settings-common-tags = Tags
settings-common-tracks-count = { $count ->
    [one] { $count } track
   *[other] { $count } tracks
}
settings-common-use = Use
settings-confirm-apply-body = This replaces your layouts, palette, and appearance with the workspace's.
settings-confirm-apply-imported-body = It's saved to your workspaces. Applying it now replaces your layouts, palette, and appearance with the workspace's.
settings-confirm-clear = Clear
settings-confirm-clear-embeddings-body = The descriptions go and the space comes back. Having them again means running the analysis pass over every track in the library.
settings-confirm-clear-embeddings-title = Clear what "{ $model }" described?
settings-confirm-clear-measured-bpm-body = Every tempo rox worked out goes back to unmeasured; numbers from your files' own tags stay. Having them again means running the tempo pass over each of those tracks.
settings-confirm-clear-measured-bpm-title = Clear the measured tempos?
settings-confirm-overwrite-workspace-body = This replaces the saved workspace with the current state.
settings-confirm-overwrite-workspace-title = Overwrite workspace "{ $name }"?
settings-sidebar-data-folder = Data Folder
settings-sidebar-settings-file = Settings File

## Menubar

menu-about = About
menu-application = Application
menu-apply-layout = Apply Layout
menu-apply-workspace = Apply Workspace
menu-chat = Chat
menu-close = Close
menu-console = Console
menu-design-mode = Design Mode
menu-discussions = Discussions
menu-empty-window = Empty Window
menu-equalizer = Equalizer
menu-exit = Exit
menu-hide-menubar = Hide Menubar
menu-import-workspace = Import Workspace...
menu-new-ellipsis = New...
menu-new-window = New Window
menu-new-window-from-layout = New Window from Layout
menu-new-window-from-panel = New Window from Panel
menu-no-layouts = No layouts
menu-no-presets = No presets
menu-no-workspaces = No workspaces
menu-os-decorations = OS Decorations
menu-overlay-shader = Overlay Shader
menu-panel-built-in = Built-in
menu-panel-new = New...
menu-panel-no-layouts = No layouts
menu-panel-no-presets = No presets
menu-panel-no-workspaces = No workspaces
menu-panel-title = Menu
menu-panels = Panels
menu-panels-presets = Presets
menu-pause = Pause
menu-playback = Playback
menu-remain-in-tray = Remain in Tray
menu-report-issue = Report Issue
menu-save-layout = Save Layout
menu-save-workspace = Save Workspace
menu-section-add = Add
menu-section-app = App
menu-section-interface = Interface
menu-section-layouts = Layouts
menu-section-library = Library
menu-section-session = Session
menu-section-track = Track
menu-section-tuning = Tuning
menu-settings = Settings
menu-signals = Signals
menu-song-theming = Song Theming
menu-stats = Stats
menu-tasks = Tasks
menu-welcome = Welcome
menu-window = Window
menu-workspace = Workspace
menu-workspace-builtin-tag = Built-in

## Workspaces

workspace-apply-body = This replaces the whole look: layouts, palette, appearance.
workspace-apply-imported-body = It's saved to your workspaces. Applying it now replaces the whole look: layouts, palette, appearance.
workspace-apply-imported-title = Imported "{ $name }"
workspace-apply-screen-shader-named = Applies the { $name } overlay shader over the whole window.
workspace-apply-screen-shader-plain = Applies an overlay shader over the whole window.
workspace-apply-shader-count = { $count ->
    [one] Includes { $count } shader: { $names }
   *[other] Includes { $count } shaders: { $names }
}
workspace-apply-shaders-approve-body = Approving lets them run on this machine. Applying without them leaves the look bare, with the shaders still in its pool.
workspace-apply-shaders-plain-body = Applying without them leaves the look bare, with the shaders still in its pool.
workspace-byline-author = by { $author }
workspace-byline-version = version { $version }
workspace-context-add-panel = Add Panel
workspace-dialog-apply = Apply
workspace-dialog-apply-title = Apply "{ $name }"?
workspace-dialog-approve-apply = Approve and Apply
workspace-dialog-cancel = Cancel
workspace-dialog-close = Close
workspace-dialog-close-title = Close "{ $name }"?
workspace-dialog-export = Export
workspace-dialog-layout-name-placeholder = Layout name
workspace-dialog-not-now = Not Now
workspace-dialog-overwrite = Overwrite
workspace-dialog-overwrite-title = Overwrite "{ $name }"?
workspace-dialog-save = Save
workspace-dialog-save-layout-title = Save Layout
workspace-dialog-save-workspace-title = Save Workspace
workspace-dialog-with-shaders = With Shaders
workspace-dialog-without-shaders = Without Shaders
workspace-dialog-workspace-name-placeholder = Workspace name
workspace-drop-add-queue = Add to queue
workspace-drop-play-now = Play now
workspace-hint-or = or
workspace-hint-then = then
workspace-import = Import
workspace-launcher-hint = Add your first panel to start building, or choose a preset under Workspace > Apply Workspace
workspace-launcher-need-help = Need help?
workspace-launcher-open-welcome = Open the welcome window
workspace-launcher-title = An empty window
workspace-layout-apply-body = This replaces this window's current layout.
workspace-layout-overwrite-body = This replaces the saved layout with the current one.
workspace-layout-preset-restore-failed = This window's layout preset couldn't be restored, so it starts empty.
workspace-layout-restore-failed = The saved layout couldn't be restored, so this window starts empty.
workspace-mini-tip-back = Back to the full layout
workspace-mini-tip-shrink = Shrink to the mini player
workspace-overwrite-body = This replaces the saved workspace with the current look.
workspace-panel-locked-close-body = This panel is pinned in place. Closing it takes it out of the layout.
workspace-save-current = Save Current
workspace-screen-shader-hint-before = Turn it off any time with
workspace-workspace-restore-failed = The workspace's layout couldn't be restored, so this window starts empty.

## Tasks window

tasks-acoustic-all-described = { $count ->
    [one] The one scanned track is described by { $label }
   *[other] All { $count } scanned tracks are described by { $label }
}
tasks-acoustic-off = Describing how tracks sound is switched off in Settings, under Library
tasks-acoustic-partial = { $label } describes { $embedded } of { $total } scanned tracks
tasks-analyzing = Analyzing { $progress }
tasks-bake-writing = Writing tags...
tasks-chip-count = { $count } tasks
tasks-convert-starting = Starting ffmpeg...
tasks-converting = Converting { $progress }
tasks-count-of-total = { $done } of { $total }
tasks-embedding = Embedding { $progress }
tasks-estimate-at = { $estimate } at { $workers }
tasks-import-failed = The last import failed: { $error }
tasks-import-reading = Reading the loved list...
tasks-import-unmatched = { $count } had no match in this library
tasks-importing = Importing { $progress }
tasks-job-acoustic = Acoustic Analysis
tasks-job-convert = Convert Audio
tasks-job-loved-import = Last.fm Loved Tracks
tasks-job-replaygain = ReplayGain
tasks-job-scan = Library Scan
tasks-job-tempo = Tempo Analysis
tasks-last-pass-stopped = The last pass stopped: { $reason }
tasks-last-run-finished = Last run finished, { $count } done
tasks-last-run-stopped = Last run stopped after { $count }
tasks-library-busy = The library is busy
tasks-library-scanning = The library is scanning
tasks-measuring = Measuring { $progress }
tasks-model-downloading = A model is still downloading
tasks-no-library-window = No library window is open, so these can't be started from here
tasks-nothing-to-measure = Nothing scanned to measure yet
tasks-rg-all-gain = { $count ->
    [one] The one track has a gain to play at
   *[other] All { $count } tracks have a gain to play at
}
tasks-rg-partial = { $missing } of { $total } tracks have no gain
tasks-scan-folder-count = { $count ->
    [one] { $count } folder
   *[other] { $count } folders
}
tasks-scan-last-scanned = { $folders }, last scanned { $ago } ago
tasks-scan-never-scanned = { $folders }, never scanned
tasks-scan-no-folders = No folders added yet. Add one in Settings, under Library
tasks-start-analyze-missing = Analyze Missing
tasks-start-measure-missing = Measure Missing
tasks-start-rescan = Rescan
tasks-stop = Stop
tasks-stopping = Stopping...
tasks-tempo-all = { $count ->
    [one] The one track has a tempo
   *[other] All { $count } tracks have a tempo
}
tasks-tempo-off = Working out how fast tracks run is switched off in Settings, under Library
tasks-tempo-partial = { $missing } of { $total } tracks have no tempo
tasks-timing = Timing { $progress }
tasks-tip = Open library tasks
tasks-window-title = rox - Tasks
tasks-working-out-missing = Working out what's missing...

## Stats window

stats-bucket-listens = { $count ->
    [one] { $count } listen, { $ago }
   *[other] { $count } listens, { $ago }
}
stats-chart-start-all = First listen
stats-chart-start-month = 30 days ago
stats-chart-start-week = 7 days ago
stats-chart-start-year = A year ago
stats-click-opens = Click Opens Stats
stats-click-section = Click
stats-count-menu = Count
    .description = Which trailing window the number counts listens over; the hover list always shows them all
stats-empty-all = No listens yet
stats-empty-range = No listens in this range
stats-now = Now
stats-open = Open Stats
stats-open-on-click = Open Stats on Click
    .description = Click the widget to open the stats window, the full listening record
stats-play-these-tracks = Play these tracks
stats-play-this-track = Play this track
stats-plays-count = { $count ->
    [one] { $count } play
   *[other] { $count } plays
}
stats-range-all = All Time
stats-range-all-short = All
stats-range-day-short = Day
stats-range-label = Range
stats-range-month = This Month
stats-range-month-short = Month
stats-range-today = Today
stats-range-week = This Week
stats-range-week-short = Week
stats-range-year = This Year
stats-range-year-short = Year
stats-readout-section = Readout
stats-section-listens = Listens
stats-section-listens-over-time = Listens Over Time
stats-section-recent-listens = Recent Listens
stats-section-top-albums = Top Albums
stats-section-top-artists = Top Artists
stats-section-top-genres = Top Genres
stats-show-change = Show the Change
    .description = Add a chip for how the window compares with the one before it, up or down; All Time has nothing behind it
stats-show-number = Show the Number
    .description = Draw the count beside the icon; off leaves a bare icon with the counts on hover
stats-title = Stats Widget
stats-tooltip-listens = Listens
stats-window-title = rox - Stats

## About window

about-check-failed = Couldn't reach GitHub
about-check-for-updates = Check for Updates
about-checking = Checking...
about-download = Download
about-downloading = Downloading... { $percent }%
about-get-it = Get It
about-license-lead = rox is free software under the GNU AGPLv3. The source is on
about-notice-lead = You should have received a copy of the license with this program. If not, see
about-release-notes = Release Notes
about-restart-now = Restart Now
about-up-to-date = You're on the latest version
about-update-failed = The update failed: { $error }
about-version = Version { $version }
about-version-available = Version { $version } is available
about-version-ready = Version { $version } is ready
about-window-title = rox - About

## Welcome window

welcome-add-folder = Add Folder
welcome-and = and
welcome-back = Back
welcome-card-menubar-title = Menubar
welcome-card-music-title = Music
welcome-card-panels-title = Panels
welcome-card-playback-title = Playback
welcome-card-rearranging-title = Rearranging
welcome-card-settings-title = Settings
welcome-close = Close
welcome-design-mode-note = Rearranging needs Design Mode, on by default at the top of that menu. Off locks the layout, so a finished setup can't be nudged.
welcome-done = Done
welcome-drop-note = Drop it on a panel's edge to split there, on the middle to share a tab group, or outside the window to make it its own window.
welcome-key-left-click = Left Click
welcome-key-middle-mouse = Middle Mouse
welcome-layout-note = Save an arrangement as a layout; a workspace bundles layouts and palette into one shareable look.
welcome-menubar-after = twice to leave it up.
welcome-menubar-before = With the menubar hidden, hold
welcome-menubar-mid = to float it back over the dock, or tap
welcome-music-note = rox scans it into the library and the files stay where they are. More folders go in settings under library.
welcome-next = Next
welcome-or = or
welcome-panels-note = Every surface is a panel, and the menubar's Panels menu opens more of them.
welcome-playback-after = seek.
welcome-playback-before = toggles playback;
welcome-quickplay-after = and it plays.
welcome-quickplay-before = opens quick play: type a track, hit
welcome-rearrange-after = anywhere in a panel to move it.
welcome-rearrange-before = Drag a tab, or hold
welcome-settings-hint-after = opens settings: the palette, transparency, and behavior.
welcome-shelf-caption = Picking one replaces the main window's look and closes the tour. This window is here any time under Application > Welcome.
welcome-stage-lead-quick-start = Pick a workspace and the main window switches to it: layouts, palette, the whole look.
welcome-stage-lead-welcome = Foobar if it was made in 20XX.
welcome-stage-title-quick-start = Quick Start
welcome-stage-title-welcome = Welcome to rox
welcome-step-hint-after = , or the buttons below.
welcome-step-hint-before = Step through it with
welcome-tile-by = by { $author }
welcome-tour-intro = A quick tour of where music comes in and where the look is set. It ends at the shelf of shipped workspaces, one click each.
welcome-window-title = rox - Welcome

## Console window

console-clear = Clear
console-copy = Copy
console-empty-filtered = Nothing at these levels
console-empty-none = Nothing logged yet
console-filter-error = Error
console-filter-info = Info
console-filter-warn = Warn
console-follow = Follow
console-line-count = { $count ->
    [one] { $count } line
   *[other] { $count } lines
}
console-open-button = Open Console
console-reveal = Reveal
console-window-title = rox - Console

## Signals window

signals-about-toggle = About Signals
signals-blurb-marked = Panels marked with this in the menus can have most of their parameters bound: right-click a parameter in the panel's settings and pick a signal, or add one from there.
signals-blurb-shared = What's tuned here is shared: a change applies to every parameter routed to that signal, in every panel and window.
signals-blurb-total = A Total is the fourth kind: it adds another signal up over time and wraps at 1, so it climbs while the music is loud and stalls while it isn't. Use it when a shader needs a phase that moves with the song rather than with the clock.
signals-blurb-what = A signal turns what's playing into one number between 0 and 1: the energy in a frequency band, the level of the whole mix, or a pulse on every hit inside a band. Response sets how fast it follows, Threshold silences it under a level you pick.
signals-no-library = No library window is open, so these show no audio. Edits still save.
signals-window-title = rox - Signals

## Equaliser

eq-analyzer-bars = Bars
eq-analyzer-off = No analyzer
eq-analyzer-wave = Wave
eq-band-badge = Band Badge
    .description = Show how many bands are off flat, on a badge over the icon
eq-band-label = Band { $number }
eq-click-nothing = Nothing
eq-click-open = Open
eq-click-section = Click
    .description = What a click does: open the equalizer window, or flip the whole curve on and off in place
eq-click-toggle = Toggle
eq-flatten = Flatten
eq-freq-label = Freq
eq-gain-label = Gain
eq-heading = Equalizer
eq-help-text = Drag a band to move it, scroll over one to widen or narrow it. The processing runs ahead of the buffer that feeds the sound card, so a move takes up to half a second to reach the speakers.
eq-hint-off = Click to turn it off
eq-hint-on = Click to turn it on
eq-hint-open = Click to open the equalizer
eq-open = Open Equalizer
eq-readout-curve = Curve
eq-readout-icon = Icon
eq-readout-section = Readout
    .description = The icon, the response curve as a sparkline, or both. The curve needs about fifty pixels of width to be readable
eq-reset-bands = Reset Bands
eq-shape-active = { $count ->
    [one] { $count } band off flat, peak { $peak } dB
   *[other] { $count } bands off flat, peak { $peak } dB
}
eq-shape-flat = Flat, every band at 0 dB
eq-status-off = Equalizer off
eq-status-on = Equalizer on
eq-title = EQ Widget
eq-widget-section = Widget
eq-width-label = Width
eq-window-title = rox - Equalizer

## Keymap

keymap-close-window = Close Window
    .description = Close whichever window is in front. Bound everywhere, popped-out panels included
keymap-decrease-font-size = Decrease Text Size
    .description = Step the app-wide text size down
keymap-focus-search = Focus Search
    .description = Put the cursor in the library search box
keymap-group-editing = Editing
keymap-group-playback = Playback
keymap-group-view = View
keymap-group-windows = Windows
keymap-increase-font-size = Increase Text Size
    .description = Step the app-wide text size up
keymap-key-backspace = Backspace
keymap-key-delete = Delete
keymap-key-down = Down
keymap-key-end = End
keymap-key-esc = Esc
keymap-key-home = Home
keymap-key-insert = Insert
keymap-key-left = Left
keymap-key-page-down = Page Down
keymap-key-page-up = Page Up
keymap-key-right = Right
keymap-key-space = Space
keymap-key-tab = Tab
keymap-key-up = Up
keymap-mod-alt = Alt
keymap-mod-cmd = Cmd
keymap-mod-ctrl = Ctrl
keymap-mod-fn = Fn
keymap-mod-option = Option
keymap-mod-shift = Shift
keymap-mod-super = Super
keymap-mod-win = Win
keymap-open-quick-play = Quick Play
    .description = Raise the search-and-play prompt over the window
keymap-open-settings = Open Settings
    .description = Open this window
keymap-open-stats = Open Statistics
    .description = Open the listening statistics window
keymap-quit = Quit
    .description = Leave rox. Bound everywhere, since there's no window it shouldn't work from
keymap-reset-font-size = Reset Text Size
    .description = Snap the text size back to stock
keymap-seek-backward = Seek Backward
    .description = Step back through the playing track
keymap-seek-forward = Seek Forward
    .description = Step forward through the playing track
keymap-stamp-line = Stamp Lyric Line
    .description = Write the playing position onto the lyric line being edited
keymap-toggle-playback = Play / Pause
    .description = Start the current track, or pause it where it is
keymap-toggle-post-shader = Toggle Overlay Shader
    .description = Turn the screen shader off and on. Bound everywhere, since a shader can bury the controls you would otherwise use to turn it off
keymap-toggle-zoom = Zoom Panel Group
    .description = Fill the dock with the last-clicked panel group, or back out of it

## Panel catalog

panel-catalog-album-carousel = Album Carousel
panel-catalog-artist-grid = Artist Grid
panel-catalog-biography = Biography
panel-catalog-cover-art = Cover Art
panel-catalog-drawer = Drawer
panel-catalog-eq-widget = EQ Widget
panel-catalog-filter = Filter
panel-catalog-folder-tree = Folder Tree
panel-catalog-genre-grid = Genre Grid
panel-catalog-group-application = Application
panel-catalog-group-arrangement = Arrangement
panel-catalog-group-catalogue = Catalogue
panel-catalog-group-controls = Controls
panel-catalog-group-details = Details
panel-catalog-group-experimental = Experimental
panel-catalog-group-visualizers = Visualizers
panel-catalog-history = History
panel-catalog-menu = Menu
panel-catalog-metadata = Metadata
panel-catalog-mini-toggle = Mini Toggle
panel-catalog-oscilloscope = Oscilloscope
panel-catalog-overlay = Overlay
panel-catalog-particles = Particles
panel-catalog-playlists = Playlists
panel-catalog-queue = Queue
panel-catalog-queue-widget = Queue Widget
panel-catalog-seek = Seek
panel-catalog-slide = Slide
panel-catalog-spectrogram = Spectrogram
panel-catalog-spectrum = Spectrum
panel-catalog-stats-widget = Stats Widget
panel-catalog-status = Status
panel-catalog-theme-toggle = Theme Toggle
panel-catalog-track-info = Track Info
panel-catalog-vu-meter = VU Meter
panel-catalog-waveform = Waveform
panel-catalog-window-controls = Window Controls

## Updater

updater-already-latest = already on the latest version
updater-checksum-mismatch = the download's checksum is { $digest }, not the { $expected } the release states
updater-checksum-missing-entry = { $sums } has no entry for { $name }; refusing an unverifiable download
updater-no-asset = the release has no { $name }
updater-no-checksums = the release has no { $sums }; refusing an unverifiable download
updater-no-release-build = no release build for this platform
updater-overran = the download ran past the size the release states
updater-short = the download stopped at { $done } of { $bytes } bytes
updater-size-mismatch = the server offered { $claimed } bytes, the release states { $bytes }

## Last.fm

lastfm-import-matching = Matching against the library
lastfm-import-read = Read { $count } loved tracks
lastfm-import-stopped = Stopped after { $count } loved tracks
lastfm-import-matched = , matched { $count }
lastfm-import-added = , added { $count } to favourites

## Tag tools

tags-editor-clear-all = clear all
tags-editor-form-view = Form
tags-editor-format-unsupported-all = Tags for this format can't be read or written yet.
tags-editor-format-unsupported-some = Some of these files are in a format whose tags can't be read or written yet.
tags-editor-guess-button = Guess
tags-editor-guess-folded = { $status }, { $count } more not shown
tags-editor-guess-help = { $placeholders }; / matches the folder above, %skip% discards
tags-editor-guess-match-count = { $hits } of { $total } match
tags-editor-guess-no-match = no match
tags-editor-guess-pattern-label = pattern
tags-editor-loading = Loading tags...
tags-editor-look-up = Look Up
tags-editor-multiple-values = Multiple values
tags-editor-clear-on-save = Clear on save
tags-editor-other-tags = Other Tags ({ $count })
tags-editor-remove = remove
tags-editor-reveal = Reveal
tags-editor-save-errors = { $count } files failed; { $error }
tags-editor-saving-progress = Saving { $done }/{ $total }...
tags-editor-table-view = Table
tags-editor-tags-section = Tags
tags-editor-unknown-partial = { $count } of { $total }
tags-editor-unread-count = Couldn't read tags for { $failed } of { $total } files
tags-editor-will-clear = will clear
tags-editor-will-remove = will remove
tags-editor-window-title = rox - Tag Editor
tags-guess-empty-segment = pattern renders an empty folder or file name
tags-guess-no-placeholders = no placeholders
tags-guess-skip-renders-nothing = %skip% has nothing to render
tags-guess-unclosed = unclosed %
tags-guess-unknown-placeholder = unknown placeholder %{ $name }%
tags-matcher-blocked-arm = Arm a field to apply
tags-matcher-blocked-no-match = No match to apply
tags-matcher-blocked-pick = Pick a match
tags-matcher-blocked-writing = Writing the tags...
tags-matcher-match-count = { $count ->
    [one] { $count } match
   *[other] { $count } matches
}
tags-matcher-no-matches = No matches found
tags-matcher-pick-match = Pick a match
tags-matcher-search-failed = Search failed: { $error }
tags-matcher-searching = Searching...
tags-matcher-tagging = Tagging { $track }
tags-matcher-window-title = rox - Find Metadata
tags-rename-blocked-cue = cue track, no file of its own
tags-rename-blocked-duplicate = two tracks map to this name
tags-rename-blocked-occupied = a file is already there
tags-rename-blocked-outside-roots = outside every library root
tags-rename-blocked-unresolved = not in the catalog yet
tags-rename-move-error = { $name }: { $error }
tags-rename-move-errors = { $count } files failed; { $error }
tags-rename-moving = Moving { $done }/{ $total }...
tags-rename-nothing-to-move = Nothing to move
tags-rename-pattern-help = { $placeholders }; / makes a folder, the extension follows the file
tags-rename-pattern-section = Pattern
tags-rename-preview-section = Preview
tags-rename-unchanged = unchanged
tags-rename-will-move = { $count } of { $total } will move
tags-rename-window-title = rox - Rename Files
tags-repair-affected-files = Affected Files
tags-repair-section = Repair
tags-repair-check-to-repair = Check a file to repair it
tags-repair-count = { $count ->
    [one] { $count } file
   *[other] { $count } files
}
tags-repair-count-so-far = { $count } so far
tags-repair-label-scope = scope
tags-repair-no-affected = No affected files found.
tags-repair-no-folder = No folder to scan; add one to the library or pick one.
tags-repair-pick-folder = Pick a folder...
tags-repair-progress = Repairing { $done }/{ $total }...
tags-repair-repair-button = { $count ->
    [0] Repair
   *[other] Repair ({ $count })
}
tags-repair-result = { $count ->
    [one] Repaired 1 file
   *[other] Repaired { $count } files
}
tags-repair-result-failed = Repaired { $count }, { $failed } failed
tags-repair-scan-first = Scan first
tags-repair-scan-hint = Scan to find files with tag damage a rewrite repairs.
tags-repair-select-all = Select all
tags-repair-select-none = Select none
tags-repair-whole-library = Whole library
tags-repair-window-title = rox - Tag Repair

## Convert

convert-arg-names-file = "{ $token }" names a file; the destination comes from the folder and the pattern
convert-section-output = Output
convert-section-preview = Preview
convert-arg-not-flag-or-value = "{ $token }" isn't a flag or a value for one
convert-check-wrote-nothing = ffmpeg exited clean but wrote nothing
convert-custom-ext-empty = The extension picks the container, so it needs one
convert-custom-ext-invalid = "{ $ext }" isn't a container name; letters and digits, no dot
convert-dialog-browse = Browse...
convert-dialog-check-passed = ffmpeg encoded a moment of silence with these, so they run
convert-dialog-check-waiting = Checked against ffmpeg once you stop typing
convert-dialog-checking = Checking with ffmpeg...
convert-dialog-choose-folder = Choose a folder to write into
convert-dialog-convert-button = Convert
convert-dialog-custom-label = Custom
convert-dialog-custom-menu-item = Custom...
convert-dialog-custom-note = Arguments split on spaces, so no quoting; embedded art isn't copied for custom formats
convert-dialog-format-not-ready = The typed format hasn't passed ffmpeg yet
convert-dialog-label-extension = extension
convert-dialog-label-format = format
convert-dialog-label-into = into
convert-dialog-label-named = named
convert-dialog-mirror = Mirror the library's folders
convert-dialog-nothing-to-convert = Nothing to convert: every row is skipped
convert-dialog-pattern-help = { $placeholders }; / makes a folder, the format sets the extension
convert-dialog-pick-folder = Pick a folder to write into
convert-dialog-span-note = { $count } trimmed out of a cue image and tagged from the library
convert-dialog-will-convert = { $count } of { $total } will convert
convert-dialog-window-title = rox - Convert
convert-ffmpeg-silent-failure = ffmpeg failed without saying why
convert-flag-attach = -attach reads a file of its own, which this doesn't allow
convert-flag-f = The extension picks the container, so -f isn't yours to set
convert-flag-i = The input is the track you picked, so -i isn't yours to set
convert-flag-n = -n is already on every run
convert-flag-y = Nothing here overwrites, so -y isn't available; a destination that exists is skipped
convert-preset-flac = FLAC
convert-preset-mp3-320 = MP3 320 kbps
convert-preset-mp3-v0 = MP3 V0
convert-preset-opus-192 = Opus 192 kbps
convert-preset-wav = WAV
convert-skip-duplicate = two tracks map to this name
convert-skip-exists = already there
convert-summary-failed = , { $count } failed
convert-summary-files = { $count ->
    [one] { $count } file
   *[other] { $count } files
}
convert-summary-line = { $files } to { $dest }
convert-summary-skipped = , { $count } skipped
convert-summary-stopped = Stopped after { $files } to { $dest }
convert-version-answered = { $binary } ran, no version reported

## Duplicates

duplicates-auto-select = Auto-select
duplicates-check-to-trash = Check copies to trash them
duplicates-copy-count = { $count ->
    [one] 2 copies
   *[other] { $count } copies
}
duplicates-different-albums = different albums
duplicates-filter-placeholder = Filter by title, artist, or folder
duplicates-groups-summary = { $groups ->
    [one] { $groups } group, { $extras } extra copies
   *[other] { $groups } groups, { $extras } extra copies
}
duplicates-library-loading = The library is still loading; try again shortly.
duplicates-no-duplicates = No duplicates found.
duplicates-no-filter-matches = No groups match the filter.
duplicates-policy-newest = Keep newest
duplicates-policy-oldest = Keep oldest
duplicates-policy-quality = Keep best quality
duplicates-scan-hint = Scan the library for tracks that appear more than once.
duplicates-select-none = Select none
duplicates-selected-count = { $count } selected
duplicates-trash-button = { $count ->
    [0] Trash
   *[other] Trash ({ $count })
}
duplicates-trash-error = { $name }: { $error }
duplicates-trash-result = { $count ->
    [one] Moved 1 file to trash
   *[other] Moved { $count } files to trash
}
duplicates-trash-result-failed = Moved { $count } to trash, { $failed } failed
duplicates-trashing = Trashing { $done }/{ $total }...
duplicates-window-title = rox - Duplicates

## Smart playlists

smart-playlist-descending = Descending
smart-playlist-edit-title = Edit Smart Playlist
smart-playlist-limit-label = Limit
smart-playlist-limit-placeholder = No limit
smart-playlist-match-count = { $count ->
    [one] { $count } track matches
   *[other] { $count } tracks match
}
smart-playlist-matched-tracks = Matched Tracks
smart-playlist-new-title = New Smart Playlist
smart-playlist-no-matches = No tracks match
smart-playlist-query-label = Query
smart-playlist-sort-default = Default order
smart-playlist-sort-added = Added
smart-playlist-sort-label = Sort
smart-playlist-unknown-field = "{ $field }:" isn't a field, so the term matches as plain text
smart-playlist-window-title = rox - { $verb }

## Playlist creation

playlist-create-not-savable = Name the playlist to save it
playlist-create-placeholder = Playlist name
playlist-create-rename-title = Rename Playlist
playlist-create-title = New Playlist
playlist-create-window-title = rox - { $verb }

## Cover tools

cover-art-back = Back
cover-art-disc = Disc
cover-art-front = Front
cover-artwork = Artwork
    .description = Which picture to show; a slot the file doesn't have falls back to the front cover
cover-disc-style = Disc Style
    .description = Style the artwork as a CD or as a vinyl record's label
cover-disc-off = Off
cover-disc-cd = CD
cover-disc-vinyl = Vinyl
cover-editor-choose-image = Choose Image
cover-editor-multiple = Multiple
cover-editor-none = None
cover-editor-not-an-image = That file is not an image rox can embed
cover-editor-not-decoded = That image could not be decoded
cover-editor-reading = Reading current art...
cover-editor-remove = Remove
cover-editor-replace = Replace
cover-editor-revert = Revert
cover-editor-save-errors = { $count } files failed; { $error }
cover-editor-saving-progress = Saving { $done }/{ $total }...
cover-editor-search-online = Search Online
cover-editor-section = Cover Art
cover-editor-slot-back = Back Cover
cover-editor-slot-front = Front Cover
cover-editor-slot-media = Media
cover-editor-will-remove = Will remove
cover-editor-window-title = rox - Cover Art
cover-matcher-blocked-fetching = Fetching the full image...
cover-matcher-blocked-no-cover = No cover to set
cover-matcher-blocked-pick = Pick a cover to set it
cover-matcher-cover-count = { $count ->
    [one] { $count } cover
   *[other] { $count } covers
}
cover-matcher-editor-closed = The cover editor was closed
cover-matcher-no-covers = No covers found
cover-matcher-search-failed = Search failed: { $error }
cover-matcher-set-cover = Set Cover
cover-matcher-setting = Setting...
cover-matcher-tile-info = { $provider }  { $width }px
cover-matcher-unsupported-format = Unsupported image format
cover-matcher-window-title = rox - Find Cover Art
cover-spin = Spin
    .description = Rotate the disc while a track plays; applies to the disc slot or a disc style
cover-spin-disc = Spin Disc
cover-spin-ramp = Spin Ramp
    .description = How long the disc takes to reach full speed, and to coast back down
cover-spin-speed = Spin Speed
    .description = Full speed, in revolutions per minute
cover-stretch = Stretch
    .description = Fill the panel, ignoring the artwork aspect ratio
cover-stretch-to-fill = Stretch to Fill
cover-title = Cover Art

## Lyrics

lyrics-always-centered = Always Centered
    .description = Pad the ends so the first and last lines can center too
lyrics-auto-search = Auto Search
    .description = Search online on a track with no words and save a confident match, no picker
lyrics-bold = Bold
lyrics-build-word-by-word = Build Word by Word
    .description = Reveal words as they are sung, karaoke style; unsung lines stay hidden
lyrics-edge-bottom = Bottom
lyrics-edge-top = Top
lyrics-edit-hint-after-stamp = to stamp
lyrics-edit-hint-or = or
lyrics-edit-loading = Loading the sheet...
lyrics-edit-lyrics = Edit Lyrics
lyrics-edit-saving = Saving...
lyrics-edit-section = Lyrics
lyrics-edit-stamp = Stamp
lyrics-edit-stamp-time = Stamp { $time }
lyrics-edit-window-title = rox - Edit Lyrics
lyrics-fade-lines-in = Fade Lines In
    .description = Fade a line up from dim as it becomes the active one
lyrics-falloff-edge = Falloff Edge
    .description = Which side of the active line the falloff dims
lyrics-find-online = Find Lyrics Online...
lyrics-follow-playback = Follow Playback
    .description = Glide the active line to the middle as a synced sheet plays
lyrics-font = Font
    .description = The lyric typeface; default follows the app font
lyrics-gap-threshold = Gap Threshold
    .description = How long an intro or gap has to run before it gets a rest
lyrics-lead-in-rest = Lead-in Rest
    .description = Show a blank rest before a long intro, so the first line fades in when it arrives
lyrics-line-falloff = Line Falloff
    .description = How far each line dims per step away from the active one
lyrics-line-spacing = Line Spacing
    .description = How far apart the synced lines are, as a multiple of the text size
lyrics-mark-dots = Dots
lyrics-mark-note = Note
lyrics-matcher-blocked-no-match = No match to apply
lyrics-matcher-blocked-pick = Pick a match to apply
lyrics-matcher-blocked-saving = Saving the words...
lyrics-matcher-match-count = { $count ->
    [one] { $count } match
   *[other] { $count } matches
}
lyrics-matcher-no-query = This track has no artist and title to match on
lyrics-matcher-pick-preview = Pick a match to preview
lyrics-matcher-search-failed = Search failed: { $error }
lyrics-matcher-synced-tag = { $provider }  synced
lyrics-matcher-window-title = rox - Find Lyrics
lyrics-no-lyrics-notice = No lyrics
lyrics-no-lyrics-track = No Lyrics for This Track
lyrics-rest-in-gaps = Rest in Gaps
    .description = Move to a blank rest through a long instrumental gap instead of holding the last line
lyrics-rest-marker = Rest Marker
    .description = What a wordless line shows in a synced sheet, the gaps and blank lines
lyrics-search-button = Online Search Button
    .description = Show the search button on the empty face; the right-click menu still finds lyrics
lyrics-search-online = Search Online
lyrics-show-song-name = Show Song Name
    .description = Show the track's name on the empty face, over the no-lyrics line
lyrics-text-size = Text Size
    .description = The lyric text; the synced line height follows it
lyrics-title = Lyrics
lyrics-title-unsynced = Title on Unsynced
    .description = Pin the track's title above an unsynced sheet, so a short panel still shows it
lyrics-wipe-lyrics = Wipe Lyrics

## Analysis passes

pass-acoustic-body = { $model } works out what each one sounds like, so the library can find music that resembles what's playing. Everything runs on this machine, and anything already described is skipped. { $lands }
pass-acoustic-lands-database = The results go in the library database and your files are left alone.
pass-acoustic-lands-tags = The results go in the library database and, for MP3 and FLAC, into each file's own tags as well, so they're kept if the database is rebuilt. Other formats keep the database copy only.
pass-acoustic-title = { $count ->
    [one] Analyze 1 track?
   *[other] Analyze { $count } tracks?
}
pass-analyze = Analyze
pass-estimate-at = { $estimate } at { $workers_phrase }.
pass-estimate-button = Estimate
pass-estimating = Estimating...
pass-measure = Measure
pass-no-estimate = Nothing has run on this machine yet, so there's no estimate. Estimate times a few tracks and works the rest out from there.
pass-replaygain-body = Each file is decoded and metered so it can play at the loudness it was mastered to. Albums are measured whole where every one of their tracks is missing a gain. { $lands }
pass-replaygain-lands-database = The numbers go in the library database and your files are left alone.
pass-replaygain-lands-tags = The numbers are written back into each file's tags, where every other player reads them.
pass-replaygain-title = { $count ->
    [one] Measure 1 track?
   *[other] Measure { $count } tracks?
}
pass-tempo-body = Two half-minute windows of each file are decoded and the beats counted, so the library can show what a track runs at. It works best on music recorded to a click and skips anything it can't measure. The numbers go in the library database and your files are untouched.
pass-tempo-title = { $count ->
    [one] Find the tempo of 1 track?
   *[other] Find the tempo of { $count } tracks?
}
pass-timing = Timing a few tracks...
pass-timing-failed = Couldn't time this library: { $error }
pass-workers = Workers

## Quick play

quick-play-comfortable-rows = Comfortable Rows
    .description = Give each result more height
quick-play-cover = Cover
    .description = Show a cover thumbnail at the left of each result
quick-play-duration = Duration
    .description = Show each result's length on the right
quick-play-narrow-by = Narrow By
quick-play-search-placeholder = Search the library
quick-play-subtitle = Subtitle
    .description = Show the artist and album under each result
quick-play-tag-album = Album
quick-play-tag-artist = Artist

## Drawer panel

drawer-add-tooltip = Add Drawer Panel
drawer-answers = Responds To
    .description = Which picks open the drawer: only its own main panel, or any panel outside it
drawer-dim = Dim
    .description = How hard the main panel dims behind the open drawer
drawer-edge = Edge
    .description = The edge the drawer rests against and slides out from
drawer-edge-bottom = Bottom
drawer-edge-top = Top
drawer-handle = Handle
    .description = Show the grip at the panel's edge. Hidden, nothing of the drawer shows until a pick, and the grip then stays while the selection holds, so a drawer that folded closed can be pulled back out
drawer-open-on = Open On
    .description = Resting on the handle always opens the drawer; selection adds a pick in the main panel
drawer-pin-open = Pin Open
drawer-reveal = Reveal
    .description = How much of the panel the open drawer covers
drawer-scope-elsewhere = Elsewhere
drawer-scope-main = Main Panel
drawer-title = Drawer
drawer-trigger-hover = Hover
drawer-trigger-selection = Selection

## Mini player

mini-tip-back = Back to the full layout
mini-tip-none = No mini layout assigned
mini-tip-shrink = Shrink to the mini player
mini-title = Mini Toggle

## System tray

tray-open = Open
tray-pause = Pause
tray-play = Play
tray-quit = Quit

## Window controls

window-controls-mini-toggle = Mini Toggle
    .description = Lead with the mini-layout toggle; shows once a mini layout is assigned
window-controls-minimize = Minimize
window-controls-style = Style
    .description = Flat icons, or the macOS traffic lights
window-controls-style-icons = Icons
window-controls-title = Window Controls
window-controls-traffic-lights = Traffic Lights

## Section names the audio panels share. Each one heads the same kind of
## rows in the spectrum, VU, oscilloscope, spectrogram and particles
## settings, so they're defined once rather than per panel.

viz-section-analysis = Analysis
viz-section-color = Color
viz-section-peaks = Peaks
viz-section-playback = Playback
viz-section-scale = Scale
viz-section-signal = Signal

## Particles panel

particles-add-emitter = Add Emitter
particles-aim = Aim
particles-aim-fixed = Fixed
particles-aim-outward = Outward
particles-burst = Burst
particles-color = Color
particles-cone = Cone
particles-direction = Direction
    .description = Which way it pulls; 0 is up, 180 is down
particles-drag = Drag
    .description = How much speed the air eats each second; zero is a vacuum
particles-drift = Drift
    .description = How fast the field itself moves, so the swirls don't stand still
particles-edit-emitters = Edit Emitters
particles-emitter-label = Emitter { $index }
particles-emitter-target = Emitter { $index } { $target }
particles-emitters-empty = No emitters yet. Add one to start the field.
particles-glow = Glow
    .description = Lay a soft halo behind each particle
particles-gravity = Gravity
particles-gravity-strength = Strength
    .description = Constant pull on everything in flight
particles-height = Height
particles-hold-on-pause = Hold on Pause
    .description = Freeze the field while paused instead of letting it drift out
particles-length = Length
particles-lifetime = Lifetime
particles-position-x = Position X
particles-position-y = Position Y
particles-radius = Radius
particles-rate = Rate
particles-rotation = Rotation
particles-round-particles = Round Particles
    .description = Draw dots instead of squares
particles-scale = Scale
    .description = How wide one swirl runs; small churns, large rolls
particles-section-emitters = Emitters
particles-section-medium = Medium
particles-section-particles = Particles
particles-shape = Shape
particles-shape-box = Box
particles-shape-line = Line
particles-shape-point = Point
particles-shape-ring = Ring
particles-size = Size
particles-speed = Speed
particles-trigger = Trigger
particles-trigger-continuous = Continuous
particles-turbulence = Turbulence
particles-turbulence-drift = Turbulence Drift
particles-turbulence-scale = Turbulence Scale
particles-turbulence-strength = Strength
    .description = How hard the field pushes particles around; zero is off
particles-width = Width

## Spectrum panel

spectrum-axis-labels = Axis Labels
    .description = Mark the range across the panel: octaves (C1, C2, ...) or frequencies (100, 1k, 10k)
spectrum-bar-gap = Bar Gap
    .description = Space between bars, wider gaps fit fewer bars
spectrum-bar-width = Bar Width
    .description = How thick each bar draws, thinner bars fit more bands
spectrum-block-gap = Block Gap
    .description = The seam between cells in a stack
spectrum-block-height = Block Height
    .description = How tall each cell in a stack draws
spectrum-cap-gravity = Cap Gravity
    .description = How hard the peak marks fall once the band drops away
spectrum-fft-size = FFT Size
    .description = Analysis window; short reacts fast, long resolves finer
spectrum-gradient-base-color = Base Color
    .description = The quiet end of the custom ramp
spectrum-gradient-cover = Cover
spectrum-gradient-mode = Gradient
    .description = Color the bands by loudness: the theme's ramp, the cover art's colors under song theming, or a custom pair
spectrum-gradient-theme = Theme
spectrum-gradient-tip-color = Tip Color
    .description = The loud end of the custom ramp
spectrum-high-bound-description = Highest frequency the bars analyze
spectrum-high-fft-size = High FFT Size
    .description = Analysis window for the bands above the split
spectrum-hold-on-pause = Hold on Pause
    .description = Freeze the bars while paused instead of letting them fall to silence
spectrum-labels-frequency = Frequency
spectrum-labels-pitch = Pitch
spectrum-low-bound-description = Lowest frequency the bars analyze
spectrum-orientation = Orientation
    .description = The edge the bands grow from
spectrum-outline-bars = Outline Bars
    .description = Draw each bar as a hollow outline instead of a filled ramp
spectrum-outline-width = Outline Width
    .description = Stroke thickness of the hollow bars
spectrum-peak-caps = Peak Caps
    .description = Hold a mark at each band's recent peak
spectrum-section-bands = Bands
spectrum-split-at = Split At
    .description = Where the zones meet, snapped to the nearest bar
spectrum-split-zones = Split Zones
    .description = Analyze below and above a split frequency at different window sizes
spectrum-style = Style
    .description = Classic bars, LED-style blocks, or a solid line
spectrum-style-bars = Bars
spectrum-style-blocks = Blocks
spectrum-style-line = Line
spectrum-symmetry = Symmetry
    .description = Fold the spectrum around the center; forward puts lows at the edges, reverse meets them in the middle
spectrum-symmetry-forward = Forward
spectrum-symmetry-reverse = Reverse

## Waveform panel

waveform-bar-gap = Bar Gap
    .description = Space between bars, zero merges them into a solid shape
waveform-bar-width = Bar Width
    .description = How thick each bar draws
waveform-outline = Outline
    .description = Trace the bars instead of filling them; merged bars read as one shape
waveform-scrobble-marker = Scrobble Marker
    .description = A thin line where the track counts as scrobbled to Last.fm
waveform-split-channels = Split Channels
    .description = One row per channel, left above right; mono tracks stay a single row
waveform-unavailable = Waveform unavailable for this track

## VU panel

vu-ballistics = Ballistics
    .description = VU integrates the loudness slowly; Peak snaps up and eases down
vu-ballistics-peak = Peak
vu-cap-gravity = Cap Gravity
    .description = How hard the peak marks fall once the meter drops away
vu-channels = Channels
    .description = Split the stereo pair, or fold to one meter
vu-channels-mono = Mono
vu-channels-stereo = Stereo
vu-db-scale = dB Scale
    .description = Draw labeled gridlines at the dB marks behind the meters
vu-gradient-mode = Gradient
    .description = Color the meters by level: the theme's ramp, the cover art's colors under song theming, or a custom pair
vu-hold-on-pause = Hold on Pause
    .description = Freeze the meters while paused instead of letting them fall to silence
vu-orientation = Orientation
    .description = The edge the meters grow from
vu-peak-caps = Peak Caps
    .description = Hold a mark at each meter's recent peak
vu-section-meter = Meter
vu-segment-gap = Segment Gap
    .description = The seam between cells in a stack
vu-segment-height = Segment Height
    .description = How tall each cell in a stack draws
vu-style = Style
    .description = A solid column, or LED-style segments
vu-style-continuous = Continuous
vu-style-segments = Segments

## Spectrogram panel

spectrogram-ceiling = Ceiling
    .description = Level that maps to the colormap's bright end, so anything louder pins there
spectrogram-colormap = Colormap
    .description = How loudness maps to color
spectrogram-colormap-cover = Cover
spectrogram-colormap-grayscale = Grayscale
spectrogram-colormap-ice = Ice
spectrogram-colormap-magma = Magma
spectrogram-colormap-theme = Theme
spectrogram-colormap-viridis = Viridis
spectrogram-direction = Direction
    .description = Edge new columns enter from, which also decides whether the frequency axis runs up the panel or across it
spectrogram-fft-size = FFT Size
    .description = Window size the analysis runs at, trading how fast a column follows a transient against how well it separates two notes down low
spectrogram-floor = Floor
    .description = Level that maps to the colormap's dark end, so anything quieter reads as background
spectrogram-grid = Grid
    .description = Frequency dividers over the picture
spectrogram-high-bound = High Bound
    .description = Top of the frequency axis, capped below Nyquist to drop the near-silent top octaves
spectrogram-history = History
    .description = How many columns the panel keeps before the oldest scrolls off
spectrogram-hold-on-pause = Hold on Pause
    .description = Keep the standing picture while playback is paused instead of scrolling silence into it
spectrogram-labels = Labels
    .description = Frequency numbers along the ruler, where the panel has room for them
spectrogram-log-scale = Log Scale
    .description = Give every octave the same room, the musical reading, instead of the even Hz spacing a lab tool shows
spectrogram-low-bound = Low Bound
    .description = Bottom of the frequency axis
spectrogram-section-picture = Picture
spectrogram-speed = Speed
    .description = How fast the picture scrolls, in columns a second

## Oscilloscope panel

oscilloscope-channels = Channels
    .description = Fold to one trace, lay both over each other, or stack a frame each
oscilloscope-channels-mono = Mono
oscilloscope-channels-overlay = Overlay
oscilloscope-channels-split = Split
oscilloscope-fill = Fill
    .description = A soft fill between the trace and the center line
oscilloscope-gain = Gain
    .description = Vertical scale, for pulling a quiet track up to a readable trace
oscilloscope-gradient-mode = Gradient
    .description = Color the trace by excursion: the theme's ramp, the cover art's colors under song theming, or a custom pair
oscilloscope-grid = Grid
    .description = Draw the graticule behind the trace
oscilloscope-hold-on-pause = Hold on Pause
    .description = Keep the standing frame while paused instead of letting the trace fall flat
oscilloscope-line-width = Line Width
    .description = How thick the trace draws
oscilloscope-persistence = Persistence
    .description = How long previous frames linger behind the trace, the phosphor afterglow look
oscilloscope-section-trace = Trace
oscilloscope-trigger = Trigger
    .description = Start each frame where the signal crosses the trigger level, so periodic material stands still
oscilloscope-trigger-falling = Falling
oscilloscope-trigger-level = Trigger Level
    .description = The level the crossing is looked for at
oscilloscope-trigger-off = Off
oscilloscope-trigger-rising = Rising
oscilloscope-window = Window
    .description = How much time the trace spans across the panel

## Shader panel

shader-panel-compile-error = This shader didn't compile:
shader-panel-compile-title = This shader didn't compile
shader-panel-enable = Enable
shader-panel-inspect = Inspect
shader-panel-note-empty-body = Pick an example, or point the panel at a .wgsl file defining fs_user(uv).
shader-panel-note-empty-title = No shader loaded.
shader-panel-note-missing-body = This panel references a shader the workspace doesn't have, so there's nothing to run.
shader-panel-note-missing-title = { $name } isn't in this workspace's shaders.
shader-panel-note-off-body = The source and its bindings are still here, just not running.
shader-panel-note-off-title = This shader is off.
shader-panel-note-pending-body = It arrived with a layout or a workspace rather than from this machine, so it stays off until you've reviewed it.
shader-panel-note-pending-title = This shader hasn't been read yet.
## The pending-shader review card: where the source came from, and the
## clipped tail of the listing when it runs past the box.
shader-pending-origin-file = Said to come from { $path }
shader-pending-origin-inline = No file behind it; the source came with the layout
shader-pending-more-lines = ... { $count } more lines
## Writing a shader back out to a file.
shader-eject-name-taken = { $name } already has { $count } numbered copies in this workspace's shaders
shader-eject-not-in-pool = { $name } isn't in this workspace's shaders
shader-eject-failed = ejecting: { $error }
shader-panel-pick = Pick a Shader
shader-panel-run-shader = Run Shader
    .description = Off keeps the source, the bookmark and the bindings in place and paints nothing
shader-panel-section-routes = Routes

## Genre grid panel

genre-grid-clear-picked = Clear Picked Genres
genre-grid-desaturate = Desaturate While Playing
    .description = Drain every tile but the playing genre's to grayscale; hovering brings a tile's color back
genre-grid-dim-while-playing = Dim While Playing
    .description = Fade every tile but the playing genre's; hovering lights a tile back up
genre-grid-follow-description = Scroll to the playing genre whenever the track changes
genre-grid-merge-many = Merge { $count } Genres Into "{ $target }"
genre-grid-merge-one = Merge "{ $source }" Into "{ $target }"
genre-grid-pick-filters = Pick Filters the Library
    .description = Clicking a genre narrows every panel following the shared search to it; off leaves the click as a plain selection
genre-grid-play-genres = Play { $count } Genres
genre-grid-resume-description = Slide back to the playing genre after you stop browsing
genre-grid-show-names = Show Names
    .description = Print the genre under every tile instead of only on hover
genre-grid-smooth-description = Glide to the genre instead of jumping
genre-grid-tally = { $albums ->
    [one] { $albums } album, { $tracks } track(s)
   *[other] { $albums } albums, { $tracks } track(s)
}
genre-grid-tile-face = Tile Face
    .description = What a tile shows: the genre's album covers, the covers washed in the genre's own color, or a flat color card with the name set on it
genre-grid-unmerge = { $count ->
    [one] Unmerge { $count } value
   *[other] Unmerge { $count } values
}

## Artist grid panel

artist-grid-clear-picked = Clear Picked Artists
artist-grid-desaturate = Desaturate While Playing
    .description = Drain every tile but the playing artist's to grayscale; hovering brings a tile's color back
artist-grid-dim-while-playing = Dim While Playing
    .description = Fade every tile but the playing artist's; hovering lights a tile back up
artist-grid-follow-description = Scroll to the playing artist whenever the track changes
artist-grid-group-mode = One Tile Per
    .description = The credited album artist keeps a record's guests on the act that released it; the track artist splits every feature onto a tile of its own
artist-grid-pick-filters = Pick Filters the Library
    .description = Clicking an artist narrows every panel following the shared search to them; off leaves the click as a plain selection
artist-grid-play-artists = Play { $count } Artists
artist-grid-portraits = Artist Portraits
    .description = Show each artist's own picture, looked up once per name and kept on disk; off shows the first album's cover
artist-grid-resume-description = Slide back to the playing artist after you stop browsing
artist-grid-section-grouping = Grouping
artist-grid-show-names = Show Names
    .description = Print the artist under every tile instead of only on hover
artist-grid-smooth-description = Glide to the artist instead of jumping
artist-grid-tally = { $albums ->
    [one] { $albums } album, { $tracks } track(s)
   *[other] { $albums } albums, { $tracks } track(s)
}
artist-grid-track-artist = Track Artist

## Wall panels

wall-dim-always = Always
    .description = Keep the tiles pushed back even when nothing plays; only a hovered tile shows in full
wall-dim-amount = Dim Amount
    .description = How far the other tiles fade; 100% hides them
wall-gap = Gap
    .description = Space between the tiles
wall-name-alignment = Name Alignment
    .description = Line the captions up under their tiles
wall-rounding = Rounding
    .description = Round each tile's corners; 100% is a circle
wall-section-picking = Picking
wall-show-counts = Show Counts
    .description = The album and track tally under each name
wall-tile-size = Tile Size
    .description = The tiles' widest edge; columns split the panel width evenly

## Metadata panel

metadata-cover-background = Cover Background
    .description = The track's cover art behind the fields
metadata-display = Display
    .description = The title-led sheet, or a flat label and value table from the top
metadata-display-sheet = Sheet
metadata-display-table = Table
metadata-edit-save = Save
metadata-field-bit-depth = Bit Depth
metadata-field-bitrate = Bitrate
metadata-field-codec = Codec
metadata-field-comment = Comment
metadata-field-disc = Disc
metadata-field-file = File
metadata-field-sample-rate = Sample Rate
metadata-field-track = Track
metadata-fields = Fields
    .description = Which fields the sheet lists; a field the track doesn't have stays hidden
metadata-find-online = Find Metadata Online...
metadata-no-library = No library
metadata-row-borders-description = The hairline under each row of the table
metadata-source = Source
    .description = Follow what's playing or selected, or read the library as a whole
metadata-stripes-description = Tint every other row of the table

## History panel

history-column-last-played = Last Played
history-descending = Descending
    .description = Run the sort backwards
history-empty-never = Every track has been played
history-empty-recent = No listens yet
history-headings = Break the recent list into album runs; Expanded adds the cover and stats
history-sort-browse = Browse Order
history-sort-date-added = Date Added
history-sort-menu = Sort
    .description = How the never-played tracks are ordered
history-title = History
history-view-most = Most Played
history-view-never = Never Played
history-view-recent = Recently Played
history-view-recent-short = Recent
history-view-row = View
    .description = Which cut of the listen record the panel shows

## Folder tree panel

folder-tree-clear-scope = Clear Folder Scope
folder-tree-collapse-all = Collapse All
folder-tree-cover-art = Cover Art
    .description = Show album art in place of the row icon, on folders or songs
folder-tree-cover-folders = Folders
folder-tree-cover-songs = Songs
folder-tree-empty = No folders in the library yet
folder-tree-follow-description = Reveal and scroll to the playing track whenever it changes
folder-tree-nonmatch-folders = Non-matching Folders
    .description = Hide the folders with no match, or keep them dim
folder-tree-nonmatch-songs = Non-matching Songs
    .description = Inside a folder that matches, dim the stray songs or hide them
folder-tree-play-folder = Play Folder
folder-tree-play-songs = { $count ->
    [one] Play
   *[other] Play { $count } Songs
}
folder-tree-resume-description = Scroll back to the playing track after you stop browsing
folder-tree-scope-to-folder = Scope Filter to Folder
folder-tree-smooth-description = Glide to the track instead of jumping
folder-tree-title = Tree

## Art panel

art-always = Keep the covers pushed back even when nothing plays; only a hovered cover shows in full
art-convert = Convert...
art-covers-section = Covers
matcher-section-matches = Matches
art-desaturate = Drain every cover but the playing album's to grayscale; hovering brings a cover's color back
art-dim-while-playing = Fade every cover but the playing album's; hovering lights a cover back up
art-disc-style = Disc Style
    .description = Style every cover as a CD or as a vinyl record's label
art-edit-tags = Edit Tags...
art-fill-panel = Fill the Panel
    .description = Size the centered cover off the panel's height alone (width when vertical); the side covers run off the edge instead of shrinking it
art-follow-description = Center the playing album whenever the track changes
art-glow = Glow
    .description = Pool the accent color behind the centered cover; with the art tint on it takes the playing album's color
art-layout-section = Layout
art-perspective = Perspective
    .description = Turn the side covers in real 3D instead of the flat squash
art-reflections = Reflections
    .description = Mirror each cover into the floor below the shelf
art-resume-description = Center the playing album again after you stop browsing
art-shadows = Shadows
    .description = A soft shadow under every cover
art-smooth-description = Glide to the album instead of jumping
art-title = Album Carousel
art-vertical-layout = Vertical Layout
    .description = Stack the shelf as a column that scrolls up and down instead of a row

## Playlists panel

playlists-columns = Which track columns show beside the title
playlists-delete = Delete Playlist
playlists-edit-query = Edit Query...
playlists-empty = No playlists yet, add tracks or use New Playlist
playlists-headings = Break each playlist's tracks into album runs; Expanded adds the cover and stats
playlists-import-tooltip = Import Playlist
playlists-imported-fallback = Imported
playlists-new = New Playlist...
playlists-new-smart = New Smart Playlist...
playlists-refuse-drag-out = Tracks in a smart playlist can't be dragged out
playlists-refuse-edit-query = Edit the query to change what a smart playlist holds
playlists-refuse-smart-source = A smart playlist takes its tracks from its query
playlists-remove = { $count ->
    [one] Remove from Playlist
   *[other] Remove { $count } from Playlist
}
playlists-rename = Rename...
playlists-title = Playlists

## Queue panel

queue-clear = Clear Queue
queue-empty = Queue is empty
queue-headings = Break the queue into album runs; Expanded adds the cover and stats
queue-play-now = Play Now
queue-remove = { $count ->
    [one] Remove from Queue
   *[other] Remove { $count } from Queue
}
queue-title = Queue
queue-widget-always-modal = Always Open as a Modal
    .description = Open the queue in a modal every time, instead of jumping to a queue panel that is already open
queue-widget-clear-queue = Clear Queue
queue-widget-more = +{ $count } more
queue-widget-open-on-click = Open Queue on Click
    .description = Click the widget to jump to an open queue panel, or open the queue in a window when none is up
queue-widget-section-click = Click
queue-widget-title = Queue Widget
queue-widget-up-next = Up Next

## Biography panel

biography-background = Background
    .description = The artist fanart behind the text, dimmed and fading out toward the bottom
biography-fill-width = Fill Width
    .description = Let a tall header span the full width instead of staying capped and centered
biography-from-lastfm = From Last.fm
biography-header-image = Header Image
    .description = The wide artist banner across the top, or the portrait when there is no banner
biography-keep-aspect = Keep Aspect Ratio
    .description = Show the header at its own proportions instead of cropping it to fill a band
biography-listeners-count = { $count } listeners
biography-looking-up = Looking up { $name }
biography-no-artist-tag = No artist tag
biography-no-text = No biography on file
biography-not-found = Nothing found for { $name }
biography-plays-count = { $count } plays
biography-refresh = Refresh
biography-similar-artists = Similar Artists
    .description = Related artists by listening data, at the foot
biography-similar-heading = Similar artists
biography-stats = Stats
    .description = Listeners and plays on Last.fm, under the name
biography-tags = Tags
    .description = The genre tags as a chip row
biography-title = Biography

## Status panel

status-count-albums = { $count ->
    [one] { $count } album
   *[other] { $count } albums
}
status-count-artists = { $count ->
    [one] { $count } artist
   *[other] { $count } artists
}
status-count-plays = { $count ->
    [one] { $count } play
   *[other] { $count } plays
}
status-count-selected = { $count } selected
status-count-tracks = { $count ->
    [one] { $count } track
   *[other] { $count } tracks
}
status-readouts = Readouts
    .description = Drag along the bar to reorder; drag between the rows, or use a chip's x and plus, to hide and show
status-scope-selection = Selection
status-title = Status

## Output panel

output-detail-badge = Badge
output-detail-compact = Compact
output-detail-expanded = Expanded
output-detail-label = Detail
    .description = Badge keeps it to a chip with the rest on hover; compact gives the headline a line of its own, for a strip along an edge; expanded adds the reasons beside it, or under it when the panel is too narrow
output-device-name = Device Name
    .description = Name the running device in the headline; off keeps the line to the mode, the rate, and the format
output-file-rate = File Rate
    .description = Confirm the playing file's own rate when nothing is converting it. A conversion is flagged either way, since that's what the warning is about
output-mode-exclusive = Exclusive
output-mode-shared = Shared
output-no-output = No output
output-nothing-playing = Nothing playing
output-pick-another-device = Pick another device, or turn exclusive off
## What the output panel reports the device accepted. The numbers are
## their own message so naming the device doesn't fork the whole line.
output-headline-numbers = { $rate } Hz, { $channels } ch, { $format }
output-headline = { $mode }, { output-headline-numbers }
output-headline-device = { $mode } on { $device }, { output-headline-numbers }
## Why what you're hearing isn't what the toggle says, and what the
## file's own rate is. Each has a full sentence for the expanded panel
## and a fragment for the one-line register. The gain arrives already
## signed and rounded, since a number formatter won't force a +.
output-fell-back-to-shared = Exclusive fell back to shared: { $why }
output-replaygain-levelling = ReplayGain is levelling this file by { $db } dB
output-replaygain-short = ReplayGain { $db } dB
output-rate-resampled = The playing file is { $rate } Hz, resampled to reach the device
output-rate-resampled-short = { $rate } Hz file resampled
output-rate-native = The playing file is { $rate } Hz, so nothing is resampling it
output-rate-native-short = { $rate } Hz file, no resampling
output-start-track-hint = Start a track to see the format the device accepted
output-title = Output

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
columns-number = Number
columns-scanned = Scanned
columns-similar = Similar

## Filter panel

filter-add-column = Add Column
filter-add-column-tooltip = Add column
filter-all = All
filter-clear-filters = Clear Filters
filter-clear-selection = Clear Selection
filter-empty = Pick a field to start filtering
filter-remove-column = Remove Column

## Search panel

search-chips-below = Below
search-chips-inline = Inline
search-filter-chips = Filter Chips
search-placeholder = Search the library

## Playback panel

playback-buttons = Buttons
    .description = Drag along the bar to reorder; drag between the rows, or use a chip's x and plus, to hide and show
playback-continue-down-list = Keep playing, on down the list
playback-continue-off = Keep playing off
playback-continue-weighted = Keep playing, never played first
playback-crossfade-inside-albums = Inside Albums
playback-crossfade-off = Crossfade off
playback-crossfade-tip = Crossfade { $length }
playback-highlight-circle = Circle
playback-highlight-square = Square
playback-hold-draw = { $tip }. Hold to pick a draw
playback-hold-length = { $tip }. Hold to pick a length
playback-hold-order = { $tip }. Hold to pick an order
playback-loop-off = Loop off
playback-loop-queue = Loop the queue
playback-loop-track = Loop this track
playback-menu-continue = Continue Button
playback-menu-crossfade = Crossfade Button
playback-menu-favourite = Favourite Button
playback-menu-random = Random Button
playback-menu-rating = Rating Stars
playback-menu-stop = Stop Button
playback-menu-stop-after = Stop After Button
playback-menu-volume = Volume Button
playback-pause = Pause
playback-play-highlight = Play Highlight
    .description = The play button's accent fill: a circle, a soft square, or none
playback-random-tip-random = Play a random track
playback-random-tip-similar = Play a track like this one
playback-seek-back-tip = Back 10 seconds
playback-seek-forward-tip = Forward 10 seconds
playback-shuffle-off = Shuffle off
playback-shuffle-on = Shuffle on, { $order } order
playback-stop-after-armed = Stop after this track, armed
playback-stop-after-tip = Stop after this track
playback-stop-tip = Stop and unload the track
playback-volume-tip-muted = Unmute, { $percent }%. Right-click for the slider
playback-volume-tip-unmuted = Mute, { $percent }%. Right-click for the slider

## Track info panel

track-info-color-output-chip = Color Output Chip
    .description = Let the chip turn warning colors when the output falls back or resamples. Off keeps it the same muted tone always, and the hover note still explains the state
track-info-cycle-every = Cycle every
    .description = How long each row stays before the fade
track-info-cycle-rows = Cycle Rows
    .description = Show the arrangement's rows one at a time in a single line, fading between them; one row alone reads as itself
track-info-delay = Delay
    .description = How long the line rests at each end before moving again
track-info-marquee = Marquee
    .description = What a line too long for the panel does: crawl and return, or loop without end
track-info-menu-overflow = Overflow
track-info-next = Next: { $line }
track-info-opening = opening...
track-info-output-fallback = Exclusive output was refused by the device, so playback is running through the shared mixer. The device reported: { $reason }
track-info-output-resample-exclusive = This file is { $source } kHz and the card took { $device } kHz, so every sample is being converted on the way out. The device wouldn't run at the file's own rate.
track-info-output-resample-mixer = This file is { $source } kHz and the mixer is running at { $device } kHz, so every sample is being converted on the way out. Exclusive mode would hand the card the file's own rate instead.
track-info-overflow-loop = Loop
track-info-overflow-scroll = Scroll
track-info-overflow-truncate = Truncate
track-info-queued-count = { $count } queued
track-info-row-size = Row { $number } Size
track-info-speed = Speed
    .description = How fast the line crawls
track-info-text-size = Text Size

## Seek panel

seek-ending = Ending
    .description = Count down the time left or show the full length
seek-ending-remaining = Remaining
seek-ending-total = Total
seek-playhead = Playhead
    .description = Span the strip's full height or hug the line
seek-playhead-full = Full
seek-playhead-line = Line
seek-playhead-max-height = Playhead Max Height
    .description = Cap the full playhead, centered on the line; 0 fills the panel
seek-playhead-width = Playhead Width
    .description = The moving position marker's width
seek-rounding = Rounding
    .description = The line's corner radius, up to a pill at half the thickness
seek-scrobble-marker = Scrobble Marker
    .description = A thin line where the track counts as scrobbled to Last.fm
seek-show-timings = Show Timings
seek-thickness = Thickness
    .description = The track line's height

## Volume panel

volume-pieces = Pieces
    .description = Drag along the bar to reorder; drag between the rows, or use a chip's x and plus, to hide and show. With the percent hidden the speaker's tooltip shows it
volume-readout = Readout
    .description = Show the level as a percent or as the decibel gain it applies
volume-readout-decibels = Decibels
volume-readout-percent = Percent
volume-stretch = Stretch
    .description = Let the slider fill the panel instead of capping its width
volume-tip-mute = Mute
volume-tip-mute-level = Mute, { $level }
volume-tip-unmute = Unmute
volume-tip-unmute-level = Unmute, { $level }

## Shared panel content

content-filter = Filter
content-no-track = No track
content-total-genres = Genres
content-total-time = Total Time

## Shared panel chrome

panel-columns-description = Which track columns show
panel-headings = Headings
panel-jump-to-playing = Jump to Playing
panel-menu-display = Display
panel-title-artists = Artists
panel-title-genres = Genres
panel-title-oscilloscope = Oscilloscope
panel-title-particles = Particles
panel-title-playback = Playback
panel-title-seek = Seek
panel-title-shader = Shader
panel-title-spectrogram = Spectrogram
panel-title-spectrum = Spectrum
panel-title-theme-toggle = Theme Toggle
panel-title-track-info = Track Info
panel-title-volume = Volume
panel-title-vu = VU Meter
panel-title-waveform = Waveform

## Everything else

choice-both = Both
choice-dim = Dim
choice-hide = Hide
composite-add-panel = Add Panel
composite-host-settings = { $host } Settings
composite-move-left = Move Left
composite-move-right = Move Right
composite-remove = Remove
composite-replace = Replace
group-panel-add-slot = Add Slot
group-panel-move-down = Move Down
group-panel-move-up = Move Up
group-panel-remove-slot = Remove Slot
group-panel-split-side-by-side = Split Side by Side
group-panel-split-stacked = Split Stacked
group-panel-swap-panels = Swap Panels
group-panel-title = Group
overlay-dim = Dim
    .description = How hard the main panel dims under the revealed overlay
overlay-title = Overlay
overlay-toggle = Toggle overlay
shader-confirm-hint-after = toggles the shader from anywhere.
shader-confirm-hint-before = A shader can make windows hard to use. Revert or close this window to go back to how things were.
shader-confirm-keep = Keep
shader-confirm-question = Keep this screen shader?
shader-confirm-revert = Revert
shader-confirm-window-title = rox - Overlay Shader
slide-add = Add Slide
slide-next = Next Slide
slide-previous = Previous Slide
slide-title = Slide
theme-toggle-to-dark = Switch to the dark theme
theme-toggle-to-light = Switch to the light theme
transport-favourite-add = Add to favourites
transport-favourite-nothing = Nothing to favourite
transport-favourite-remove = Remove from favourites
transport-pieces = Pieces
    .description = Drag along a row to reorder and between rows to move; a chip's x and plus hide and show

## Stragglers picked up in the final sweep

duplicates-scanning = Scanning...
about-copyright = Copyright © 2026
signal-name-placeholder = Signal name
signals-empty = No signals yet. Add one, or right-click any bindable knob.
signal-add = Add Signal
panel-approve = Approve
panel-turn-off = Turn Off
shader-from-file = From File...
arrange-add-row = Add Row
smart-playlist-name-placeholder = Playlist name
smart-playlist-name-to-save = Name the playlist to save it
panel-new-playlist = New Playlist...
panel-edit-tags = Edit Tags...
panel-edit-cover = Edit Cover Art...
panel-rename-files = Rename Files...
panel-convert = Convert...
panel-catalog-drag-anchor = Drag Anchor
panel-catalog-spacer = Spacer

## Duration and worker phrasing

pace-under-a-minute = under a minute
pace-minutes = { $count ->
    [one] about a minute
   *[other] about { $count } minutes
}
pace-hours = { $count ->
    [one] about an hour
   *[other] about { $count } hours
}
pace-half-hours = about { $value } hours
pace-days = { $count ->
    [one] about a day
   *[other] about { $count } days
}
pace-workers = { $count ->
    [one] { $count } worker
   *[other] { $count } workers
}
tasks-rest-takes = , the rest takes { $estimate }
tasks-measuring-takes = , measuring them takes { $estimate }
tasks-working-out-takes = , working them out takes { $estimate }
tasks-time-left = , { $left } left
tasks-failed-suffix = ({ $count } failed)
## The rest of the pieces a long pass's progress line is built from. The
## caller puts a space in front of each, the same way it already does for
## the skipped suffix, so the separator stays in one place.
tasks-file-suffix = - { $file }
tasks-no-beat-suffix = ({ $count } with no clear beat)
tasks-estimate-at-workers = ({ tasks-estimate-at })

## Panel vanity names

panel-title-art-view = Art View
panel-title-artist-grid = Artist Grid
panel-title-genre-grid = Genre Grid
panel-title-biography = Biography
panel-title-cover-art = Cover Art
panel-title-drag-anchor = Drag Anchor
panel-title-drawer = Drawer
panel-title-eq-widget = EQ Widget
panel-title-filter = Filter
panel-title-folder-tree = Folder Tree
panel-title-group = Group
panel-title-history = History
panel-title-lyrics = Lyrics
panel-title-menu = Menu
panel-title-metadata = Metadata
panel-title-mini-toggle = Mini Toggle
panel-title-output = Output
panel-title-overlay = Overlay
panel-title-playlists = Playlists
panel-title-queue = Queue
panel-title-queue-widget = Queue Widget
panel-title-search = Search
panel-title-slide = Slide
panel-title-spacer = Spacer
panel-title-stats-widget = Stats Widget
panel-title-vu-meter = VU Meter
panel-title-window-controls = Window Controls

## Relative time and the output headline

ago-just-now = just now
ago-minutes = { $count }m ago
ago-hours = { $count }h ago
ago-days = { $count }d ago
ago-weeks = { $count }w ago
ago-years = { $count }y ago

## Long spans spelled out, for the library totals. The short clocks stop
## meaning much past a day, so these include the noun.

span-seconds = { $count ->
    [one] { $count } second
   *[other] { $count } seconds
}
span-minutes = { $count ->
    [one] { $count } minute
   *[other] { $count } minutes
}
span-hours = { $count ->
    [one] { $count } hour
   *[other] { $count } hours
}
span-days = { $count ->
    [one] { $count } day
   *[other] { $count } days
}
span-weeks = { $count ->
    [one] { $count } week
   *[other] { $count } weeks
}
span-years = { $count ->
    [one] { $count } year
   *[other] { $count } years
}

## How a span joins its second unit: "3 weeks, 2 days".
span-pair = { $first }, { $second }

## A percentage. The space before the sign is a locale question, not a
## notation one, so each locale spells the whole thing out.
unit-percent = { $value }%

settings-audio-output-headline = { $mode }{ $note } on { $device }, { $rate } Hz, { $channels } ch, { $format }
settings-audio-output-experimental =  (experimental)

## ML model catalog

settings-mlmodels-description = { $summary }. { $dim } values per track. { $licence }
settings-mlmodels-on-disk = , { $size } on disk
settings-mlmodels-to-download = , { $size } to download
model-summary-dsp-timbre-1 = Built in, no download. A summary of each track's log-band energy, spectral shape, and onset rate. Coarse next to a trained network, but it needs nothing and it runs everywhere
model-summary-panns-cnn10 = A convolutional network trained on AudioSet to recognize what a sound is. Its 512-value description of a track is far richer than the built-in sketch, at the cost of a 24 MB download and a slower analysis pass

## Shipped workspaces

# The names stay as written: they are proper names, and `bundle.name` is
# the lookup key. Only (Default) is a word rather than a name.
workspace-shipped-default = (Default)
workspace-shipped-default-blurb = What rox looks like out of the box: translucent surfaces over the desktop, no window chrome, art tinting off. The starting point every other look here is a departure from.
workspace-shipped-catrox-blurb = The foobar2000 skin that started it all, rebuilt: a circular CD render of the cover, the metadata fields down the left, and album-grouped tracks with rating dots.
workspace-shipped-critters-blurb = The whole app as a 1-bit print: an ordered dither over every surface, tones that crush with the sub-bass, and a noise wall that writhes with the song. After Critters for Sale.
workspace-shipped-diffuse-blurb = Just the playing album: the cover and the playback card as one group filling the window, transparent surfaces over the backdrop, seam-free. The library, the queue, and the lyrics wait in a drawer on the right edge, sliding out over the music when the handle is hovered. Monochrome, so the color comes from the covers.
workspace-shipped-foobar-blurb = The layout this whole project is an argument with. Opaque panels, artist and album filter columns, a dense track table, and the menubar right where it always was.
workspace-shipped-llama-winamp-blurb = Winamp the way you remember it rather than the way it was. Tahoma, dark, no chrome, a dotted spectrum across the top, and a shade mode on the mini layout.
workspace-shipped-metro-blurb = Flat panels and comfortable rows in Segoe UI, with art theming on so the whole palette follows whatever cover is playing.
workspace-shipped-phosphor-blurb = Monospace everything. Consolas, green on black, no cover in quick play: a terminal that happens to play music.
