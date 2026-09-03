### Español. Refleja en-CA/rox.ftl clave por clave; la prueba de
### paridad en rox-i18n es lo que lo mantiene así.

## Shared widgets
tracking-title = Seguimiento
tracking-follow = Seguir la reproducción
tracking-resume = Volver al dejar de navegar
tracking-smooth = Desplazamiento suave
align-row = Alineación
    .description = Dónde se coloca el contenido cuando al panel le sobra espacio
valign-row = Alineación vertical
    .description = Dónde se coloca el contenido cuando al panel le sobra altura
valign-top = Arriba
valign-middle = Centro
valign-bottom = Abajo
letter-rail-compact = Barra compacta
    .description = Limita la barra a una sola línea que se desplaza en vez de envolverse
letter-rail-side = Posición de la barra
    .description = En qué borde de la pared cuelga la barra

## Panel source and search rows
source-track = Pista
    .description = Sigue lo que suena, o lo que está seleccionado en la biblioteca
source-follow-playing = Seguir la reproducción
source-follow-selection = Seguir la selección
source-playing = En reproducción
source-selected = Seleccionado
query-search = Búsqueda
query-search-box = Campo de búsqueda
    .description = Muestra el campo de búsqueda; la consulta solo se aplica mientras está visible
query-source = Origen de la búsqueda
    .description = Sigue la consulta compartida, filtra por el campo propio de este panel, o muestra lo que otro panel tenga seleccionado
query-source-shared = Compartida
query-source-own = Propia
query-source-selection = Selección

## Signals and routes
signal-source = Origen
    .description = Qué sigue la señal: Banda rastrea un rango de frecuencias, Nivel toda la mezcla, Ataque pulsa en cada golpe del rango, Disparo lanza un pulso cuando el rango alcanza su umbral, Total suma otra señal a lo largo del tiempo
signal-kind-band = Banda
signal-kind-level = Nivel
signal-kind-onset = Ataque
signal-kind-trigger = Disparo
signal-kind-total = Total
signal-response = Respuesta
signal-response-pulse = Cuánto resuena cada pulso antes de apagarse
signal-response-drift = 0 se pega a la música, 100 va a la zaga
signal-threshold = Umbral
signal-threshold-trigger = El nivel que el rango tiene que alcanzar para lanzar el pulso; no vuelve a dispararse hasta que el nivel cae por debajo de la marca del medidor de arriba
signal-threshold-gate = Por debajo de esto la señal no marca nada, y por encima la salida vuelve a subir desde cero, así las partes suaves no mueven el mando. La marca del medidor de arriba señala dónde está
signal-low-bound = Límite inferior
signal-high-bound = Límite superior
signal-adds-up = Suma
    .description = Qué señal se totaliza aquí; sube mientras aquella marca alto y se estanca mientras está callada
signal-aggregate-nothing = No hay nada que seguir
signal-aggregate-pick = Elige una señal
signal-aggregate-alone = No hay otra señal en el conjunto que sumar, así que esto se queda en cero. Añade una y aparecerá en la lista.
signal-aggregate-unpicked = No hay nada elegido, así que este total se queda en cero. Elige una señal arriba.
signal-rate = Tasa
    .description = Vueltas por segundo a plena entrada; pasa de 1 a 0 y sigue subiendo, que es lo que un shader lee como fase
signal-reset-on-track = Reiniciar al cambiar de pista
    .description = Vuelve a cero cuando empieza una canción nueva, para que una fase no arrastre el total de la anterior
signal-flush = Vaciar
signal-routes-in-panel = { $count ->
    [one] { $count } ruta en este panel
   *[other] { $count } rutas en este panel
}
    .description = Devuélvelo a cero ahora. Se vacía poco a poco en vez de saltar, así nada de lo que lo sigue pega un tirón
route-header = Ruta
route-signal = Señal
    .description = Qué señal compartida sigue esta ruta; ajustarla aquí ajusta todas las rutas que cuelgan de ella
route-new-signal = Nueva señal
route-shared-note = Compartido por todas las rutas de esta señal
route-signal-gone = La señal de esta ruta ya no está; el mando mantiene el valor de su deslizador hasta que elijas otra arriba.
route-range-note = Rango solo para este parámetro
route-quiet = Suave
    .description = Qué marca el mando en silencio, como fracción de su propio ajuste
route-loud = Fuerte
    .description = Qué marca a plena señal; 100% es el valor del propio deslizador, por debajo de Suave modula hacia abajo
route-slot = Slot
    .description = Cuál de los dieciséis slots de señal del shader llena esta ruta
route-slot-quiet-description = Qué marca el slot en silencio
route-slot-loud-description = Qué marca a plena señal; por debajo de Suave el slot corre al revés
route-slot-signal-description = Qué señal compartida sigue esta ruta
route-slot-signal-gone = La señal de esta ruta ya no está; el slot marca cero hasta que elijas otra.
route-add = Añadir ruta
route-unrouted = Sin ruta
route-pick-slot = Elige un slot
route-pick-signal = Elige una señal
route-no-signal = sin señal
route-no-signals-yet = Todavía no hay señales que seguir. Crea una y aparecerá aquí; hasta entonces el slot marca cero.
route-open-signals = Abrir señales
route-create-signal = Crear señal nueva

## Panel settings window
panel-settings = Ajustes del panel
panel-menu-label = Panel
panel-save-as-preset = Guardar como preajuste
panel-rename = Renombrar
panel-rename-name = Nombre
panel-rename-note = Se muestra como pestaña del panel; en blanco vuelve al nombre integrado
panel-rename-hint-after = para renombrar
panel-was-closed = El panel se cerró
panel-reset = Restablecer
panel-inverse = Invertir
panel-apply-song-theme = Aplicar el color de la canción
panel-page-appearance = Apariencia
panel-page-behavior = Comportamiento
panel-page-shader = Shader
panel-section-placement = Colocación
panel-section-size = Tamaño
panel-section-opacity = Opacidad
panel-section-frame = Marco
panel-section-colors = Colores
panel-section-font = Fuente
panel-section-shader = Shader
panel-section-signals = Señales
panel-section-slots = Slots
panel-awaiting-approval = Esperando aprobación
panel-size-off = Desactivado
panel-locked = Bloqueado
    .description = Fija el panel en su sitio; no se puede arrastrar ni reordenar en el dock
panel-drag-anchor = Ancla de arrastre
    .description = Arrastrar desde cualquier punto del panel mueve la ventana, mientras que los clics normales siguen llegando a sus controles; para disposiciones sin decoraciones
panel-slot-controls = Controles de slot
    .description = Muestra los botones de esquina para intercambiar y quitar los paneles que este aloja. Ocultos, la disposición se sigue editando desde el árbol de la página Espacio de trabajo en los ajustes
panel-min-width = Ancho mínimo
    .description = Dónde deja de estrecharse el panel al redimensionarlo. Se toma tal cual, incluso por debajo del mínimo propio del panel, así una tira compacta puede quedar más ajustada de lo normal; en blanco deja el mínimo en paz
panel-max-width = Ancho máximo
    .description = Limita el ancho del panel para que no se estire cuando la ventana se ensancha
panel-min-height = Altura mínima
    .description = Dónde deja de acortarse el panel al redimensionarlo. Se toma tal cual, incluso por debajo del mínimo propio del panel, así una tira compacta puede quedar más ajustada de lo normal; en blanco deja el mínimo en paz
panel-max-height = Altura máxima
    .description = Limita la altura del panel para que no se estire cuando la ventana crece
panel-own-opacity = Opacidad de superficie propia
    .description = Dale a este panel su propia opacidad sobre el fondo en lugar de la de la aplicación
panel-surface-opacity = Opacidad de la superficie
panel-margin = Margen
    .description = Mete el panel hacia dentro de su celda, dejando que el fondo se vea por el hueco
panel-padding = Relleno
    .description = Espacio dentro del borde del panel, con su propio fondo
panel-rounding = Redondeo
    .description = Redondea las esquinas del panel contra el fondo
panel-border = Borde
    .description = Una línea alrededor del borde del panel, en el color del rol Borde; un lado a cero no dibuja ninguna
panel-font = Fuente
    .description = La tipografía del panel; el valor predeterminado sigue la fuente de la aplicación
panel-font-size = Tamaño de fuente
    .description = El tamaño del texto del panel respecto a la fuente de la aplicación; las filas escalan con él
panel-surface-shader = Shader de superficie
    .description = Ejecuta un shader WGSL sobre el cuerpo de este panel, por debajo del shader de pantalla de la aplicación
panel-run-when-idle = Seguir en reposo
    .description = Sigue dibujando fotogramas mientras el audio está en silencio. Desactivado, el shader se congela en su último fotograma y el panel no cuesta nada
panel-shader-is-scene = Este shader es una escena, así que cubre el cuerpo del panel en vez de dibujar encima. Vino de un paquete o de una configuración antigua; la lista de arriba solo ofrece shaders que dejan el panel legible.

## Shader picker and saving
shader-source = Origen
shader-pick-none = Ninguno
shader-reload = Recargar
shader-edit-as-file = Editar como archivo
shader-make-private-copy = Hacer copia privada
shader-save-replace = Reemplazar
shader-save-to-workspace = Guardar en el espacio de trabajo
shader-save-replaces = Reemplaza el shader que este espacio de trabajo ya llama { $name }. Todos los paneles que usan ese nombre cambian con él
shader-save-adds = Lo añade a los shaders de este espacio de trabajo bajo { $name }. Cualquier panel puede usarlo, y editarlo los actualiza todos
shader-group-examples = Ejemplos
shader-group-this-workspace = Este espacio de trabajo
shader-group-scenes = Escenas
shader-group-workspace-scenes = Escenas del espacio de trabajo
shader-group-overlays = Superposiciones
shader-group-workspace-overlays = Superposiciones del espacio de trabajo

## Saving a panel preset
preset-save = Guardar preajuste
preset-save-name = Nombre del preajuste
preset-save-replaces = Reemplaza el preajuste que este espacio de trabajo ya llama { $name }
preset-save-hint-after = para guardar
preset-back-from = Vuelve a añadirlo desde
preset-back-add-panel = Añadir panel
preset-back-then = y luego
preset-back-presets = Preajustes
preset-back-tail = en cualquier menú de panel. Los preajustes son solo de este espacio de trabajo; otro no los tendrá.

## Keyboard hints
hint-press = Pulsa
hint-key-enter = Intro

## Settings: language
settings-language = Idioma
    .description = El idioma de la interfaz. Sistema compara con la lista del sistema operativo y recurre al inglés cuando no coincide nada
    .keywords = traducción idioma lengua configuración regional
settings-language-system = (Idioma del sistema)
settings-language-search = Buscar idiomas
picker-no-matches = Sin coincidencias
settings-search-no-matches = Nada coincide con "{ $text }"

## Embed dialog
bake-window-title = rox - Incrustar metadatos guardados
bake-title = Incrustar metadatos guardados
bake-intro = Escribe los metadatos guardados en los propios archivos, para que otro reproductor también los lea. No se recalcula nada.
bake-formats = Solo MP3 y FLAC; se omiten los demás formatos y las pistas de CUE
bake-source-lyrics = Letras
bake-source-gain = ReplayGain
bake-source-acoustic = Descripciones acústicas
bake-detail-nothing = no hay nada guardado que incrustar
bake-detail-only-skipped = { $skipped ->
    [one] nada que escribir, { $skipped } omitido
   *[other] nada que escribir, { $skipped } omitidos
}
bake-detail-writes = { $count ->
    [one] { $count } archivo por escribir
   *[other] { $count } archivos por escribir
}
bake-detail-writes-skipped = { $count ->
    [one] { $count } archivo por escribir, { $skipped ->
        [one] { $skipped } omitido
       *[other] { $skipped } omitidos
    }
   *[other] { $count } archivos por escribir, { $skipped ->
        [one] { $skipped } omitido
       *[other] { $skipped } omitidos
    }
}
bake-error-read = No se pudo leer la biblioteca: { $error }
bake-survey-counting = Revisando la biblioteca...
bake-survey-progress = Leyendo etiquetas, { $done } de { $total }
bake-nothing-to-embed = Nada que incrustar: los archivos ya tienen todo lo que rox guarda
bake-rewrites = { $count ->
    [one] Se reescribirá { $count } archivo
   *[other] Se reescribirán { $count } archivos
}
bake-hint-before = Pulsa
bake-hint-key = Intro
bake-hint-after = para incrustar
bake-embed = Incrustar
bake-cancel = Cancelar
bake-summary-files = { $count ->
    [one] 1 archivo
   *[other] { $count } archivos
}
bake-summary-updated = Actualización de { $files }
bake-summary-stopped = Detenido tras actualizar { $files }
bake-summary-skipped = { $count ->
    [one] , { $count } omitido
   *[other] , { $count } omitidos
}
bake-summary-failed = { $count ->
    [one] , { $count } falló
   *[other] , { $count } fallaron
}

## Arrange editors and header pieces
arrange-shown = Visible
arrange-hidden = Oculto
tile-face-mosaic = Mosaico de carátulas
tile-face-tinted = Mosaico teñido
tile-face-gradient = Tarjeta de degradado
tile-face-color = Tarjeta de color
head-piece-artist = Artista
head-piece-album = Álbum
head-piece-year = Año
head-piece-genre = Género
head-piece-quality = Calidad
head-piece-tracks = Pistas
head-piece-time = Duración
head-piece-spacer = Separador
head-piece-divider = Divisor
head-piece-art = Carátula
head-unknown = Desconocido
status-item-count = Cantidad
status-item-time = Duración
status-item-albums = Álbumes
status-item-artists = Artistas
status-item-plays = Reproducciones
volume-item-icon = Icono
volume-item-slider = Deslizador
volume-item-percent = Porcentaje

## Filter chips and search menus
filter-field-artist = Artista
filter-field-album-artist = Artista del álbum
filter-field-album = Álbum
filter-field-genre = Género
filter-field-year = Año
filter-field-folder = Carpeta
filter-unknown = Desconocido
filter-clear = Borrar
query-show-search-box = Mostrar el campo de búsqueda
query-own-query = Consulta propia
query-shared-query = Consulta compartida
headers-off = Desactivados
headers-compact = Compactos
headers-expanded = Ampliados

## Panel context menu
panel-dock-back = Volver a acoplar
panel-pop-out = Sacar a ventana
panel-close = Cerrar
panel-duplicate = Duplicar
panel-reveal-in-browser = Mostrar en el explorador de archivos
panel-play-next = Reproducir a continuación
panel-add-to-queue = Añadir a la cola
panel-add-to-playlist = Añadir a una lista
panel-favourite-add = Añadir a favoritos
panel-favourite-remove = Quitar de favoritos
panel-copy = Copiar
panel-copy-title = Copiar título
panel-copy-artist = Copiar artista
panel-copy-album = Copiar álbum
panel-copy-filename = Copiar nombre de archivo
panel-copy-path = Copiar ruta
shader-pick-missing = { $name } (falta)
shader-pick-custom = Personalizado

## Shipped shader examples
shader-blurb-plasma = Color a la deriva sacado solo de sus uniforms, así que cuesta lo que un quad simple.
shader-blurb-trails = Emborrona su propio fotograma anterior, así que corre en la pasada de pantalla.
shader-blurb-sheen = Un viñeteado y un brillo errante, superposición transparente para un panel que ya dibuja.
shader-blurb-shadow = Una sombra proyectada por el propio texto y los controles del panel, leída de la captura de máscara.
shader-blurb-cover = La carátula de la pista en reproducción, con bandas sobre un baño de su propio color.
shader-blurb-badge = La carátula como una tarjeta pequeña en una esquina, con un slot para moverla de sitio.
shader-blurb-lamp = Una luz que sigue al cursor y reacciona a los clics, superposición transparente.
shader-blurb-cube = Un cubo de alambre dando vueltas en 3D falso, dibujado como luz aditiva.
shader-blurb-bloom = Orbes a la deriva con bloom a través de una segunda pasada a media resolución, la cadena en miniatura.
shader-blurb-tube = Repite el panel de debajo a través de una pantalla CRT curva, con líneas de barrido incluidas.

## Transport strip pieces
seek-item-elapsed = Transcurrido
seek-item-strip = Barra
seek-item-ending = Final
seek-item-duration = Duración
info-item-track-no = N.º de pista
info-item-title = Título
info-item-duration = Duración
info-item-next = Siguiente
info-item-queued = En cola
info-item-output = Salida
info-item-favourite = Favorito
info-item-rating = Valoración
playback-item-previous = Anterior
playback-item-seek-back = Retroceder
playback-item-play = Reproducir
playback-item-seek-forward = Avanzar
playback-item-next = Siguiente
playback-item-stop = Detener
playback-item-volume = Volumen
playback-item-loop = Repetir
playback-item-shuffle = Aleatorio
playback-item-continue = Continuar
playback-item-crossfade = Fundido encadenado
playback-item-random = Al azar
playback-item-stop-after = Parar después
playback-item-favourite = Favorito
playback-item-rating = Valoración

## Dock chrome
dock-empty-tab = Pestaña vacía
dock-unnamed = Sin nombre
dock-tiles = Mosaicos
dock-zoom-in = Acercar
dock-zoom-out = Alejar
dock-collapse = Contraer
dock-expand = Expandir

## Shader picker notes
shader-note-empty = Elige un ejemplo para empezar, o apunta rox a un archivo .wgsl con una etapa de fragmento que defina fs_user(uv)
shader-note-missing = { $name } ya no está en los shaders de este espacio de trabajo, así que no se pinta nada. Elige otra cosa aquí y este panel tendrá un origen propio.
shader-note-shared = Compartido en este espacio de trabajo. Editarlo actualiza todas las superficies que lo usan.
shader-note-file = { $path }. Lo que guardas se recarga mientras el shader dibuja, y el código queda guardado dentro de las disposiciones y los paquetes, así que sigue funcionando en una máquina que nunca tuvo el archivo.
shader-note-custom = Este código está guardado dentro de su disposición o paquete, sin ningún archivo detrás. Editar como archivo lo vuelca fuera y recoge lo que guardes.

## Panel pages and shared sides
panel-page-layout = Disposición
panel-page-view = Vista
panel-page-content = Contenido
panel-page-source = Origen
panel-page-bindings = Enlaces
panel-page-emitters = Emisores
panel-page-forces = Fuerzas
panel-page-scene = Escena
side-left = Izquierda
side-right = Derecha
genre-face-mosaic = Mosaico
genre-face-tinted = Teñido
genre-face-gradient = Degradado
genre-face-color = Color

## Library panel
panel-title-library = Biblioteca
library-play = Reproducir
library-play-album = Reproducir el álbum
library-play-group = Reproducir el grupo
library-play-tracks = Reproducir { $count } pistas
library-play-similar = Reproducir algo parecido
library-filter-by-album = Filtrar por álbum
library-filter-by-artist = Filtrar por artista
library-jump-to-playing = Ir a lo que suena
library-menu-display = Presentación
library-disc = Disco { $number }
library-empty-title = Abre una carpeta de música
library-empty-note = Se escanea a la biblioteca (flac, mp3, wav)
library-headers = Encabezados
    .description = Cortes de grupo sobre la lista; una ordenación mantiene juntas las tandas, y al buscar se ve todo plano
library-group-by = Agrupar por
    .description = Sobre qué cortan los encabezados; género y año reordenan la lista
library-header-row = Fila de encabezado
    .description = Qué muestran los encabezados de una fila, de izquierda a derecha; un separador o un divisor parte los lados
library-header-lines = Líneas del encabezado
    .description = Las filas del bloque, de arriba abajo; una línea vacía desaparece
library-follow-description = Desplázate a la fila que suena cada vez que cambia la pista
library-resume-description = Vuelve a la fila que suena cuando dejas de navegar
library-smooth-description = Deslízate hasta la fila en vez de saltar
library-columns = Columnas
    .description = Qué columnas se ven; arrastra los encabezados en el panel para reordenarlas y ajustar su ancho
library-column-headers = Encabezados de columna
    .description = La fila de encabezados ordenables sobre la lista; ocúltala y las columnas conservan su orden y su ancho
library-column-rename = Renombrar...
library-column-rename-reset = Restablecer nombre
library-column-rename-name = Encabezado
library-column-rename-note = Se muestra en lugar del encabezado integrado; vacío lo devuelve, y un solo espacio deja el encabezado en blanco
library-sort-on-click = Ordenar al hacer clic
    .description = Ordena al hacer clic en cualquier parte del encabezado en vez de en su icono; reordenar una columna pasa a pedir Alt y arrastrar
library-compact-plays = Reproducciones compactas
    .description = La columna de reproducciones como un número pequeño con un guion al lado
library-line-height = Altura de línea
    .description = Una línea de encabezado; los bloques toman las filas que necesitan, al margen de las filas de pista
library-text-size = Tamaño del texto
    .description = El texto de las líneas de encabezado, al margen de la altura de línea, para que la carátula crezca sola
library-flush-background = Fondo a ras
    .description = Coloca los encabezados sobre el fondo de la lista en vez del tono elevado; el color de la canción los mueve juntos
library-gap-above = Espacio arriba
    .description = Recortado de la parte alta del bloque; la lista se ve a través, y las líneas se aprietan para caber
library-gap-below = Espacio abajo
    .description = Lo mismo debajo del bloque, antes de sus pistas
library-section-rows = Filas
library-row-height = Altura de fila
    .description = Las filas de pista; el texto las acompaña, y ambos escalan con la fuente de la aplicación
library-row-spacing = Espaciado de filas
    .description = Altura extra que llena cada fila; aire sin agrandar el texto
library-stripes = Resaltado alterno
    .description = Tiñe una fila de pista sí y otra no para que una lista larga se lea de un vistazo
library-row-borders = Líneas de fila
    .description = La línea fina bajo cada fila de pista
library-art-description = El mosaico de los encabezados ampliados: la carátula, el retrato del artista o la cara del género
library-art-rounding = Redondeo de la carátula
    .description = Redondea las esquinas de la carátula
library-art-position = Posición de la carátula
    .description = En qué lado del bloque se coloca el mosaico de los encabezados ampliados
library-art-margin = Margen de la carátula
    .description = Mete el mosaico hacia dentro del bloque; se encoge para seguir siendo cuadrado
library-circular-portraits = Retratos circulares
    .description = Agrupado por artista, redondea los mosaicos hasta el círculo completo del muro en vez de usar el mando de redondeo
library-genre-face = Cara del género
    .description = Agrupado por género, qué muestra el mosaico: las carátulas, las carátulas bañadas en el color del género, o una tarjeta de color bajo su geometría

## Album grid panel
panel-title-album-grid = Cuadrícula de álbumes
grid-menu-scroll = Desplazamiento
grid-menu-sort = Orden
grid-sort-artist = Artista
grid-sort-album = Álbum
grid-sort-year = Año
grid-sort-added = Añadidos hace poco
grid-sort-plays = Más reproducidos
grid-letter-rail = Barra de letras
    .description = Las iniciales en el borde del muro; un clic salta al primer álbum de esa letra
grid-vertical-scroll = Desplazamiento vertical
grid-horizontal-scroll = Desplazamiento horizontal
grid-jump-to-playing = Ir a lo que suena
grid-library-empty = La biblioteca está vacía
grid-play-albums = Reproducir { $count } álbumes
grid-vertical-layout = Disposición vertical
    .description = Desplaza el muro arriba y abajo, con las filas llenando el ancho; desactivado se desplaza a izquierda y derecha, con las columnas llenando la altura
grid-follow-description = Desplázate al álbum que suena cada vez que cambia la pista
grid-resume-description = Vuelve al álbum que suena cuando dejas de navegar
grid-smooth-description = Deslízate hasta el álbum en vez de saltar
grid-section-dimming = Atenuación
grid-section-tiles = Mosaicos
grid-dim-while-playing = Atenuar durante la reproducción
    .description = Apaga todas las carátulas salvo la del álbum que suena; al pasar el cursor un mosaico vuelve a encenderse
grid-dim-amount = Intensidad
    .description = Cuánto se apagan las demás carátulas; al 100% desaparecen
grid-desaturate = Desaturar durante la reproducción
    .description = Pasa a escala de grises todas las carátulas salvo la del álbum que suena; al pasar el cursor vuelve el color de un mosaico
grid-always = Siempre
    .description = Mantén las carátulas atrás incluso cuando no suena nada; solo el mosaico bajo el cursor se ve entero
grid-show-titles = Mostrar títulos
    .description = Escribe el álbum y el artista bajo cada carátula, al estilo de iTunes, en vez de solo al pasar el cursor
grid-title-alignment = Alineación de títulos
    .description = Alinea los pies de foto bajo sus carátulas
grid-tile-size = Tamaño del mosaico
    .description = El lado más largo de los mosaicos de carátula; las columnas reparten el ancho del panel por igual
grid-gap = Separación
    .description = Espacio entre las carátulas; cero las junta borde con borde
grid-art-rounding-description = Redondea las esquinas de cada carátula; al 100% es un círculo

## Settings: sidebar pages
settings-page-appearance = Apariencia
settings-page-application = Aplicación
settings-page-audio = Audio
settings-page-development = Desarrollo
settings-page-integrations = Integraciones
settings-page-keymap = Atajos de teclado
settings-page-library = Biblioteca
settings-page-mcp = MCP
settings-page-ml-models = Modelos de ML
settings-page-playback = Reproducción
settings-page-providers = Proveedores
settings-page-shader = Shader
settings-page-storage = Almacenamiento
settings-page-workspace = Espacio de trabajo

## Settings: appearance
settings-appearance-backdrop-all-windows = Todas las ventanas
    .description = Pon fondo también a las ventanas secundarias: ajustes, editores, diálogos, paneles sacados a ventana. Desactivado deja el fondo y la transparencia solo en las ventanas del espacio de trabajo
settings-appearance-backdrop-strength = Intensidad del fondo
    .description = Con cuánta fuerza se ve el fondo de carátula detrás de ellas
settings-appearance-border = Borde
    .description = Una línea alrededor del borde de cada panel, en el color del rol Borde; un lado a cero no dibuja ninguna
settings-appearance-colors-locked-note = El color de la canción está activado, así que la pista en reproducción manda sobre estos colores y la exportación los guarda. Desactívalo arriba para editarlos
settings-appearance-design-mode = Modo de diseño
    .description = Edita la disposición en su sitio: las filas de añadir, renombrar, duplicar, sacar a ventana y cerrar de los menús de panel, los controles que un contenedor pone sobre sus slots, y el arrastre de pestañas. Desactivado esconde todo eso; la página Espacio de trabajo sigue editando el árbol
    .keywords = editar disposición reordenar bloquear
settings-appearance-font = Fuente
    .description = La tipografía de toda la aplicación; los paneles pueden sustituirla en sus propios ajustes
    .keywords = tipografía familia texto
settings-appearance-font-size = Tamaño de fuente
    .description = El tamaño base desde el que escala el texto de cada panel; los controles y los iconos mantienen su tamaño
settings-appearance-hide-menubar = Ocultar la barra de menús
    .description = Mantén la barra de menús oculta y hazla flotar sobre el dock mientras mantienes pulsado Alt. Pulsa Alt dos veces para dejarla fija, así sus botones aceptan un clic normal
settings-appearance-icons-intro = Un paquete es una carpeta de SVG que reemplaza los iconos integrados; el cambio se aplica al arrancar de nuevo
settings-appearance-icons-open-folder = Abrir carpeta
settings-appearance-inverse-from-dark = Invertir desde el tema oscuro
settings-appearance-inverse-from-light = Invertir desde el tema claro
settings-appearance-keep-theme = Mantener el tema
    .description = Mantén el tema activo aunque el brillo de una carátula lo haría cambiar; el color de la canción sigue tiñendo
settings-appearance-margin = Margen
    .description = Mete cada panel hacia dentro de su celda; un panel puede sustituirlo en sus propios ajustes
settings-appearance-new-pack = Paquete nuevo
settings-appearance-os-decorations = Decoraciones del sistema
    .description = La barra de título y los bordes del sistema en las ventanas principales; desactivadas, todo recae en los controles de ventana y los paneles con ancla de arrastre
settings-appearance-pack-name-placeholder = Nombre del paquete
settings-appearance-padding = Relleno
    .description = Espacio dentro del borde de cada panel, con su propio fondo
settings-appearance-palette-export = Exportar
settings-appearance-palette-import = Importar
settings-appearance-panel-seams = Juntas entre paneles
    .description = La línea fina entre mosaicos de panel; desactivada deja los tiradores de redimensión invisibles pero igual de arrastrables
settings-appearance-resize-border = Borde de redimensión
    .description = Redimensionar las ventanas principales arrastrando sus bordes; solo se aplica con las decoraciones del sistema desactivadas, y al apagarlo el ajuste a bordes y Win+flecha quedan como la forma de redimensionar
settings-appearance-rounding = Redondeo
    .description = Redondea las esquinas de cada panel contra el fondo
settings-appearance-section-colors = Colores
settings-appearance-section-frame = Marco
settings-appearance-section-icons = Iconos
settings-appearance-section-interface = Interfaz
settings-appearance-section-theming = Coloreado
settings-appearance-section-transparency = Transparencia
settings-appearance-section-typography = Tipografía
settings-appearance-song-theming = Color de la canción
    .description = Tiñe la paleta y pon de fondo la carátula de la pista en reproducción
settings-appearance-surface-opacity = Opacidad de las superficies
    .description = Cuán opacas se ven las superficies de la aplicación sobre el fondo
settings-appearance-theme = Tema
    .description = La paleta que dibuja la aplicación y la que apunta el editor de color de abajo; Sistema sigue la preferencia clara u oscura del sistema operativo
settings-appearance-theme-dark = Oscuro
settings-appearance-theme-light = Claro
settings-appearance-theme-system = Sistema

## Settings: application
settings-application-check-updates = Buscar actualizaciones
    .description = Busca una versión más nueva una vez al día cuando arranca rox; la ventana Acerca de comprueba al momento de todas formas
settings-application-download-updates = Descargar actualizaciones
    .description = Cuando una comprobación encuentra una versión más nueva, la descarga y la deja lista en segundo plano; el siguiente arranque la ejecuta
settings-application-enable-ai = Activar funciones de IA
    .description = Deja que las herramientas de IA hablen con rox: añade soporte MCP y las descargas de modelos de ML, con sus páginas en la barra lateral.
settings-application-lock-panel-resize = Bloquear el tamaño de los paneles
    .description = Las divisiones de panel solo se redimensionan con el modo de diseño activado, para que un arrastre cerca de una junta no descoloque una disposición terminada
settings-application-portable-copying = Copiando datos...
settings-application-portable-mode = Modo portátil
    .description = Guarda los ajustes, la biblioteca y las cachés en una carpeta rox-data junto al ejecutable, para que el reproductor viaje con sus datos. Desactivarlo vuelve a la carpeta del sistema y deja rox-data donde está
settings-application-portable-not-writable = No se puede escribir en la carpeta de la aplicación
settings-application-portable-restart-note = Se aplica en el siguiente arranque; esta ejecución sigue con su carpeta actual
settings-application-remain-in-tray = Quedarse en la bandeja
    .description = Mantén la música sonando cuando se cierra la última ventana, con el icono de la bandeja (el dock en macOS) como forma de volver
settings-application-section-ai = IA
settings-application-section-control-socket = Socket de control
settings-application-section-data = Datos
settings-application-section-layout = Disposición
settings-application-section-startup = Arranque
settings-application-section-window = Ventana
settings-application-socket-path = Ruta del socket
    .description = La interfaz de máquina de rox mientras corre: JSON-RPC sobre un socket local, atado a esta carpeta de datos. El proxy rox-mcp atiende con ella a los clientes MCP

## Settings: audio
settings-audio-broadcast-bitrate = Tasa de bits
    .description = Lo que gasta el codificador MP3 por segundo de emisión
settings-audio-broadcast-enable = Emitir a Icecast
    .description = Empuja lo que rox reproduce a un servidor icecast como cliente de origen, codificado a MP3. El punto de montaje, los oyentes y la cara de red son todos de icecast; rox solo conecta hacia fuera, y un servidor inalcanzable nunca toca la reproducción local
settings-audio-broadcast-host-placeholder = servidor icecast
settings-audio-broadcast-login = Credenciales de origen
    .description = Las credenciales de origen de icecast, el usuario y la contraseña que nombra su configuración
settings-audio-broadcast-mount = Punto de montaje
    .description = El punto de montaje al que sintonizan los oyentes, y el nombre de emisión que anuncia
settings-audio-broadcast-name-placeholder = Nombre de la emisión
settings-audio-broadcast-password-placeholder = Contraseña de origen
settings-audio-broadcast-server = Servidor
    .description = El servidor y el puerto de icecast; el protocolo de origen va sobre un socket simple
settings-audio-broadcast-user-placeholder = source
settings-audio-crossfade = Fundido encadenado
    .description = Cuánto se solapa una pista con la siguiente. El fundido está pensado para el modo aleatorio y los saltos, así que las fronteras propias de un álbum quedan intactas salvo que la fila de abajo diga otra cosa. Cero lo desactiva
    .keywords = sin huecos solape transición fundido
settings-audio-equalizer-note = Diez bandas de octava sobre la salida. Se abre en su propia ventana, porque se trabaja mientras suena la música en vez de ajustarse una sola vez
settings-audio-exclusive-mode = Modo exclusivo
    .description = Reclama el dispositivo solo para rox y hazlo funcionar a la frecuencia del propio archivo donde el hardware la acepte; desactivado comparte el mezclador del sistema con todo lo demás del escritorio
settings-audio-fade-inside-albums = Fundir dentro de los álbumes
    .description = Solapa también las pistas que pertenecen al mismo disco. Desactivado deja los empalmes propios de un disco exactamente como se masterizaron, que es donde más importa la reproducción sin huecos
settings-audio-open-equalizer = Abrir el ecualizador
settings-audio-output-buffer = Búfer
    .description = Cuánto audio guarda la tarjeta de una vez. Más corto reacciona antes y chasquea antes en una máquina ocupada; más largo es más seguro y más perezoso
settings-audio-output-buffer-default = Predeterminado (10 ms)
settings-audio-output-device = Dispositivo
    .description-default = El predeterminado del sistema sigue lo que tenga configurado el escritorio
    .description-linux = El modo exclusivo reclama una tarjeta directamente al kernel, así que la lista son tarjetas de sonido y no las salidas del escritorio. Bluetooth y otros dispositivos del servidor de sonido no tienen tarjeta que reclamar y solo aparecen con el modo exclusivo desactivado
    .description-other = El modo exclusivo toma el dispositivo solo para rox, así que nada más del escritorio puede sonar por él hasta que lo desactives
settings-audio-output-device-system-default = Predeterminado del sistema
settings-audio-output-experimental-badge = Experimental
settings-audio-output-experimental-tooltip = El backend exclusivo de esta plataforma está escrito a partir de su contrato de audio documentado, pero los desarrolladores nunca lo han ejecutado en hardware real. Debería reclamar el dispositivo o volver al modo compartido con un motivo, nunca quedarse mudo. Si se porta mal, desactívalo y cuenta qué pasó con el botón que hay junto a este distintivo.
settings-audio-output-format = Formato
    .description = Lo que rox le entrega a la tarjeta. Una tarjeta que no acepte la elección usa el formato más ancho que tenga, y el estado de abajo muestra cuál
settings-audio-output-format-f32 = Coma flotante de 32 bits
settings-audio-output-format-s16 = Entero de 16 bits
settings-audio-output-format-s32 = Entero de 32 bits
settings-audio-output-format-widest = El más ancho disponible
settings-audio-output-issue-tooltip = Cuenta cómo se comportó el modo exclusivo en esta máquina. Abre un issue en GitHub con la plataforma y el flujo negociado ya rellenados.
settings-audio-output-mode-exclusive = Exclusivo
settings-audio-output-mode-shared = Compartido
settings-audio-output-not-built = Todavía no compilado para esta plataforma
settings-audio-output-rate-follow = Seguir el archivo
settings-audio-output-sample-rate = Frecuencia de muestreo
    .description = Seguirla reabre el dispositivo con la frecuencia propia de cada archivo, lo que cuesta un hueco en una frontera donde la frecuencia cambia; fijar una frecuencia no paga eso nunca y remuestrea todo lo que no encaje
settings-audio-output-status-error-hint = Elige otro dispositivo, o desactiva el modo exclusivo
settings-audio-output-status-error-title = Sin salida
settings-audio-output-status-idle-hint = Pon una pista para ver el formato que aceptó el dispositivo
settings-audio-output-status-idle-title = No suena nada
settings-audio-replaygain-level-by = Nivelar por
    .description = Reproduce cada pista con la sonoridad que midieron sus etiquetas ReplayGain, para que el modo aleatorio deje de saltar entre masterizaciones. Pista nivela cada archivo por su cuenta; Álbum usa la ganancia del disco en todas sus pistas, lo que deja los pasajes suaves y fuertes de un álbum donde los pusieron
    .keywords = normalización sonoridad nivelado volumen
settings-audio-replaygain-measure-missing-button = Medir lo que falta
settings-audio-replaygain-measure-new = Medir archivos nuevos
    .description = Mide lo que trae el vigilante según llega, una vez que la sincronización se ha asentado, para que una biblioteca que crece mantenga sus ganancias sin volver aquí. Los números van a donde apunte Guardar las ganancias medidas. Al activarlo se ofrece medir primero lo que ya falta; después de eso solo ve archivos recién llegados
settings-audio-replaygain-measuring-progress = Midiendo { $done } de { $total }
settings-audio-replaygain-measuring-start = Midiendo: calculando lo que falta...
settings-audio-replaygain-mode-album = Álbum
settings-audio-replaygain-mode-off = Desactivado
settings-audio-replaygain-mode-track = Pista
settings-audio-replaygain-preamp = Preamplificación
    .description = Se suma a cada ganancia etiquetada. La referencia de ReplayGain queda por debajo de donde se cortan los discos modernos, así que una biblioteca nivelada suena más floja que la misma biblioteca en crudo; aquí es donde eso vuelve. Un realce nunca satura: el pico etiquetado lo limita
settings-audio-replaygain-save = Guardar las ganancias medidas
    .description = Dónde deja sus números la pasada de medición. La base de datos de la biblioteca deja tus archivos intactos; las etiquetas ponen los mismos valores donde los lee cualquier otro reproductor, a costa de reescribir los archivos de audio
settings-audio-replaygain-status-measured = { $total ->
    [one] La única pista escaneada tiene una ganancia con la que nivelar, y rox midió { $measured }
   *[other] Las { $total } pistas escaneadas tienen una ganancia con la que nivelar, y rox midió { $measured }
}
settings-audio-replaygain-status-tagged = { $total ->
    [one] La única pista escaneada tiene etiquetas ReplayGain
   *[other] Las { $total } pistas escaneadas tienen etiquetas ReplayGain
}
settings-audio-replaygain-untagged = Archivos sin etiquetar
    .description = Con qué ganancia suena un archivo sin etiquetas ReplayGain. Nadie lo midió, así que esto es una suposición que hace las veces de medida. Déjalo en cero y las pistas sin etiquetar sonarán como siempre
settings-audio-section-broadcast = Emisión
settings-audio-section-equalizer = Ecualizador
settings-audio-section-output = Salida
settings-audio-section-playback = Reproducción
settings-audio-section-replaygain = ReplayGain
settings-audio-transport = Transporte
    .description = Arranca y para sin salir de esta página, porque cada ajuste de abajo se juzga de oído

## Settings: integrations
settings-integrations-discord-enable = Activar Rich Presence
    .description = Muestra la actividad de rox en Discord mientras suena la música
settings-integrations-discord-show-lastfm = Mostrar el botón de Last.fm
    .description = Incluye un botón 'Ver en Last.fm' en el estado de Discord
settings-integrations-discord-show-youtube = Mostrar el botón de YouTube
    .description = Incluye un botón 'Buscar en YouTube' en el estado de Discord
settings-integrations-ffmpeg-binary = Binario de FFmpeg
    .description = Qué ffmpeg hace las conversiones; déjalo vacío para el que esté en el PATH
settings-integrations-ffmpeg-fail-note = Convertir sigue oculto hasta que ffmpeg apunte a un binario que funcione
settings-integrations-ffmpeg-fail-title = Este ffmpeg no se ejecutó
settings-integrations-ffmpeg-missing-note = Convertir sigue oculto; instala ffmpeg o apunta la ruta a un binario
settings-integrations-ffmpeg-missing-title = No se encontró un ffmpeg que funcione
settings-integrations-ffmpeg-ok-note = ffmpeg funciona. Convertir está disponible.
settings-integrations-ffmpeg-test = Probar
settings-integrations-lastfm-api-key-row = Clave de API
settings-integrations-lastfm-connect = Conectar
settings-integrations-lastfm-disconnect = Desconectar
settings-integrations-lastfm-finish-connecting = Terminar de conectar
settings-integrations-lastfm-hearts = { $n ->
    [one] { $n } corazón
   *[other] { $n } corazones
}
settings-integrations-lastfm-import-loved = Importar pistas favoritas
settings-integrations-lastfm-intro-builtin = Conecta tu cuenta de Last.fm: autoriza rox en el navegador y las pistas reproducidas se scrobblean allí
settings-integrations-lastfm-intro-custom = Esta compilación no trae identidad de api, así que el scrobbling necesita tu propia cuenta de api (Last.fm/api/account/create); pega su clave y su secreto compartido, y luego conecta
settings-integrations-lastfm-key-placeholder = Clave de API
settings-integrations-lastfm-love-failed = El último falló: { $error }
settings-integrations-lastfm-love-pending = { $hearts } esperando a enviarse
settings-integrations-lastfm-love-pending-failed = { $hearts } esperando a enviarse, último intento: { $error }
settings-integrations-lastfm-reconnect = Reconectar
settings-integrations-lastfm-secret-placeholder = Secreto compartido
settings-integrations-lastfm-secret-row = Secreto compartido
settings-integrations-lastfm-status-confirming = Confirmando...
settings-integrations-lastfm-status-connected = Conectado como { $username }
settings-integrations-lastfm-status-elsewhere = Conectado en otra instalación de rox; cada una autoriza con su propia identidad de api, así que conecta también esta
settings-integrations-lastfm-status-failed = Falló la conexión: { $error }
settings-integrations-lastfm-status-not-connected = Sin conectar
settings-integrations-lastfm-status-rejected = Last.fm rechazó la sesión y se descartó. Vuelve a conectar para seguir scrobbleando
settings-integrations-lastfm-status-requesting = Pidiendo un token...
settings-integrations-lastfm-status-waiting = Autoriza rox en el navegador y luego termina de conectar
settings-integrations-lastfm-working = Trabajando...
settings-integrations-love-favourites = Marcar favoritos en Last.fm
    .description = Refleja los corazones en Last.fm como pistas favoritas; quitar un corazón también lo quita allí
settings-integrations-scrobble-threshold = Umbral de scrobble
    .description = Cuánto tiene que sonar una pista antes de scrobblearse; la barra de posición y la forma de onda pueden marcarlo
settings-integrations-scrobble-tracks = Scrobblear pistas
    .description = Envía las pistas reproducidas a Last.fm en cuanto cruzan el umbral
settings-integrations-section-conversion = Conversión
settings-integrations-section-discord = Discord Rich Presence
settings-integrations-section-favourites = Favoritos
settings-integrations-section-lastfm = Last.fm
settings-integrations-section-scrobbling = Scrobbling

## Settings: keymap
settings-keymap-clash = { $chord } también es { $other }; solo uno de los dos se disparará
settings-keymap-not-bound = Sin asignar
settings-keymap-recording = Pulsa las teclas
settings-keymap-restore = Restaurar
settings-keymap-restore-all = Restaurar todas las combinaciones
    .description = Devuelve cada comando a las teclas con las que viene de fábrica, incluidos los que esta compilación ya no tiene en una fila
settings-keymap-section-defaults = Predeterminados
settings-keymap-undo = Deshacer
settings-keymap-undo-last = Deshacer el último restablecimiento
    .description = Recupera las combinaciones que descartó el último restablecimiento, de una fila o de todas

## Settings: library
settings-library-acoustic-all-described = { $total ->
    [one] La única pista escaneada está descrita por { $label }
   *[other] Las { $total } pistas escaneadas están descritas por { $label }
}
settings-library-acoustic-auto = Describir archivos nuevos
    .description = Describe lo que trae el vigilante según llega, una vez que la sincronización se ha asentado, para que una biblioteca que crece mantenga sus descripciones sin volver aquí. Desactivado, los archivos nuevos esperan al botón Analizar lo que falta. Al activarlo se ofrece analizar primero lo que ya falta; después de eso solo ve archivos recién llegados
settings-library-acoustic-enable = Describir cómo suenan las pistas
    .description = Averigua a qué suena cada pista, para que la biblioteca pueda encontrar música parecida a lo que está sonando. Todo corre en esta máquina, y describir una biblioteca grande lleva un rato
    .keywords = parecido sonido huella describir
settings-library-acoustic-extractor = Extractor
settings-library-acoustic-extractor-model = Modelo
settings-library-acoustic-fallback = Analizando
settings-library-acoustic-partial = { $label } describe { $done } de { $total } pistas escaneadas. Analizar lo que falta se ocupa del resto
settings-library-acoustic-progress = { $running } va por { $done } de { $total }
settings-library-acoustic-progress-start = { $running }: calculando lo que falta...
settings-library-acoustic-save = Guardar las descripciones
    .description = Dónde deja la pasada lo que averigua. La base de datos sola deja tus archivos intactos; las etiquetas ponen además una copia en cada archivo, así que las descripciones se conservan si se reconstruye la biblioteca o si la carpeta se mueve a otra máquina, a costa de reescribir los archivos de audio. Las etiquetas solo funcionan en MP3 y FLAC; cualquier otro formato se queda con la copia de la base de datos
settings-library-add-folder = Añadir carpeta
settings-library-duplicates = Duplicados...
settings-library-embed-button = Incrustar metadatos guardados...
settings-library-folder-col-albums = Álbumes
settings-library-folder-col-folder = Carpeta
settings-library-folder-col-size = Tamaño
settings-library-folder-col-tracks = Pistas
settings-library-folders-intro = Carpetas escaneadas a la biblioteca; quitar una saca sus pistas del catálogo y deja los archivos en paz
settings-library-genre-separator-nudge = Separadores cambiados: la navegación lo sigue al momento. Las listas de géneros guardadas por escaneos anteriores conservan su forma antigua hasta que pulses Volver a escanear arriba, en la cabecera de Carpetas
settings-library-merge-case = Unificar variantes de mayúsculas
    .description = Trata como uno solo los valores que solo se diferencian en mayúsculas: Rock y rock pasan a ser el mismo género, artista y álbum, mostrados con la grafía que llevan más pistas. Los archivos conservan sus etiquetas tal como están escritas
settings-library-no-folders = Todavía no hay carpetas
settings-library-repair-tags = Reparar etiquetas...
settings-library-section-folders = Carpetas
settings-library-section-stored-metadata = Metadatos guardados
settings-library-section-tempo = Análisis de tempo
settings-library-split-genres = Separar géneros en comas y barras
    .description = "Dubstep, Trap" y "Drum & Bass / Neurofunk" cuentan cada valor como un género propio; el punto y coma siempre separa. Desactivado deja enteros los nombres con barra, para las etiquetas donde significan un solo género. Los archivos conservan sus etiquetas tal como están escritas
settings-library-tempo-auto = Contar archivos nuevos
    .description = Cuenta los tiempos de lo que trae el vigilante según llega, una vez que la sincronización se ha asentado, para que una biblioteca que crece mantenga sus tempos sin volver aquí. Desactivado, los archivos nuevos esperan al botón Analizar lo que falta. Al activarlo se ofrece medir primero lo que ya falta; después de eso solo ve archivos recién llegados
settings-library-tempo-enable = Averiguar a qué velocidad van las pistas
    .description = Cuenta los tiempos de las pistas cuyas etiquetas no lo dicen, para que la biblioteca pueda mostrar y ordenar por tempo. Todo corre en esta máquina, los números van a la base de datos de la biblioteca, y tus archivos quedan intactos
settings-library-tempo-progress = Contando { $done } de { $total }
settings-library-tempo-progress-start = Calculando lo que falta...
settings-library-tempo-refused = { $count ->
    [one] . rox no pudo contar el pulso de 1 pista, así que Analizar lo que falta la deja de lado
   *[other] . rox no pudo contar el pulso de { $count } pistas, así que Analizar lo que falta las deja de lado
}
settings-library-tempo-retry = Reintentar las rechazadas
settings-library-tempo-status-measured = { $total ->
    [one] La única pista escaneada tiene tempo, y rox calculó { $measured }
   *[other] Las { $total } pistas escaneadas tienen tempo, y rox calculó { $measured }
}
settings-library-tempo-status-measured-some = { $covered } de { $total } pistas escaneadas tienen tempo, y rox calculó { $measured }
settings-library-tempo-status-none = { $total ->
    [one] La única pista escaneada no dice a qué velocidad va. Analizar lo que falta lo averigua
   *[other] Ninguna de las { $total } pistas escaneadas dice a qué velocidad va. Analizar lo que falta lo averigua
}
settings-library-tempo-status-partial = { $covered } de { $total } pistas escaneadas tienen tempo, y rox calculó { $measured }. Analizar lo que falta se encarga de las otras { $missing }
settings-library-tempo-status-tagged = { $total ->
    [one] La única pista escaneada tiene una etiqueta de tempo
   *[other] Las { $total } pistas escaneadas tienen una etiqueta de tempo
}
settings-library-tempo-status-tagged-some = { $covered } de { $total } pistas escaneadas tienen una etiqueta de tempo
settings-library-watch-folders = Vigilar carpetas
    .description = Incorpora a la biblioteca los archivos añadidos, editados y borrados según ocurre, sin volver a escanear a mano
settings-library-write-stored = Escribir lo guardado en los archivos
    .description = Los tres ajustes de guardado solo se aplican a la siguiente escritura, así que todo lo guardado antes de pasar uno a Etiquetas sigue estando solo en rox. Esto escribe las letras, las ganancias y las descripciones que rox ya tiene en los propios archivos, para que otro reproductor que lea la carpeta las vea. No se recalcula nada
settings-show-readings = Mostrar lecturas
    .description = Poner la lectura romanizada después de un nombre escrito en una grafía que este alfabeto no sabe pronunciar: 秋ノ風 (Aki no kaze). La lectura es el nombre de ordenación que el valor ya lleva, así que un nombre sin ella no muestra nada y un nombre latino nunca la recibe

## Settings: MCP
settings-mcp-client-config = Configuración del cliente
    .description = Pégalo en la lista de servidores de un cliente MCP (Claude Code, Claude Desktop o cualquier otro) para que pueda preguntarle a rox por la biblioteca, por lo que suena y por el transporte. rox tiene que estar en marcha; las herramientas van por su socket de control
settings-mcp-enable = Activar el servidor MCP
    .description = Responde a las llamadas de herramientas de los clientes MCP conectados. El proxy lo comprueba en cada llamada, así que mientras esté apagado los clientes reciben el rechazo con su motivo; la configuración de abajo se puede preparar igualmente

## Settings: ML models
settings-mlmodels-checking = Comprobando...
settings-mlmodels-choose-file = Elegir archivo
settings-mlmodels-custom-description-empty = Apunta rox a un checkpoint propio de PANNs CNN10, en safetensors. Se lee donde está y se nombra por su hash, así que un segundo checkpoint describe la biblioteca por separado en vez de reutilizar las coordenadas del primero
settings-mlmodels-download-failed = No se pudo descargar { $label }: { $reason }
settings-mlmodels-downloading = Descargando { $label }: { $done } de { $total }
settings-mlmodels-stopping = Deteniendo la descarga de { $label }...
settings-dictionary-description = { $summary }. { $licence }
settings-dictionary-download-failed = No se pudo descargar el diccionario: { $reason }
settings-dictionary-downloading = Descargando el diccionario: { $done } de { $total }
settings-dictionary-heading = Romanización
settings-dictionary-stopping = Deteniendo la descarga del diccionario...
settings-mlmodels-fallback-model = modelo
settings-mlmodels-fallback-the-model = El modelo
settings-mlmodels-kind-custom = Personalizado
settings-mlmodels-kind-recommended = Recomendado
settings-mlmodels-pass-stopped = La última pasada se detuvo: { $reason }
settings-mlmodels-weights-file = Archivo de pesos

## Settings: playback
settings-playback-continuation-continue = Continuar
    .description = Sigue bajando por la lista desde la que empezaste, y luego el resto de la biblioteca detrás. Reproduce un álbum desde la mitad de una vista y la vista sigue adelante
settings-playback-continuation-off = Desactivado
    .description = Nada rellena la cola; la reproducción se para al final
settings-playback-continuation-weighted = Ponderado
    .description = Toma de toda la biblioteca, primero lo que nunca has puesto y al final lo que has oído hace poco
settings-playback-keep-playing = Seguir reproduciendo
    .description = Qué suena cuando se acaba la cola. Lo que elija se añade a la línea de tiempo como contexto normal, así que se ve y se puede quitar en vez de ser un estado oculto. Con el orden de arriba en Parecido sigue buscando pistas que suenen como la que está sonando, sea cual sea de estos el elegido
    .keywords = continuación rellenar reproducción automática cola
settings-playback-play-order = Orden de reproducción
    .description = Cómo se ordenan las pistas ya encoladas mientras el modo aleatorio está activo. El botón de aleatorio del transporte lo activa y lo desactiva; esto es lo que hace una vez activo
settings-playback-rating-scale = Escala de valoración
    .description = Estrellas para clics rápidos, 0-10 en medios pasos para notas de reseña más finas
settings-playback-rating-scale-numeric = 0-10
settings-playback-rating-scale-stars = Estrellas
settings-playback-restore-last-session = Restaurar la última sesión
    .description = Arranca con la cola tal como la dejaste, en pausa sobre la pista que sonaba y por donde se quedó. Las pistas encoladas fuera de tus carpetas de biblioteca no se pueden restaurar y caen del orden
settings-playback-section-queue = Cola
settings-playback-section-ratings = Valoraciones
settings-playback-section-startup = Arranque
settings-playback-shuffle-random = Al azar
    .description = El aleatorio que todo el mundo quiere decir con la palabra. Lo que viene suena sin ningún orden concreto
settings-playback-shuffle-similar = Parecido
    .description = Lo más cercano primero por sonido. Lo que viene se ordena por cuánto se parece a la pista que sonaba cuando lo activaste, y se reordena en cada salto. Necesita la biblioteca descrita en la página Biblioteca
settings-playback-unrated-dots = Puntos sin valorar
    .description = Marca los huecos de estrella sin rellenar con un punto tenue en vez de dejarlos vacíos

## Settings: providers
settings-providers-artist = Last.fm
    .description = Trae biografías de artistas, estadísticas y artistas parecidos para el panel de biografía, con un retrato de Deezer; todo se guarda en la carpeta de datos y luego se lee sin conexión
settings-providers-deezer = Deezer
    .description = Busca carátulas en Deezer, hasta 1000 píxeles
settings-providers-itunes = iTunes
    .description = Busca carátulas en iTunes; la búsqueda del editor de carátulas muestra las coincidencias para elegir antes de fijar una
settings-providers-lastfm-art = Last.fm
    .description = Busca carátulas en Last.fm
settings-providers-lrclib = LRCLIB
    .description = Trae las letras que faltan de lrclib.net, sincronizadas cuando las tiene
settings-providers-lyrics-intro = Las consultas en línea solo se lanzan cuando una acción de panel lo pide; la reproducción y la navegación nunca tocan la red
settings-providers-musicbrainz = MusicBrainz
    .description = Consulta etiquetas en musicbrainz.org; la búsqueda del panel de metadatos muestra las coincidencias para confirmarlas campo por campo antes de escribir
settings-providers-save-lyrics = Guardar las letras descargadas
    .description = Dónde se guarda una letra descargada: la carpeta de datos propia de rox, que mantiene la biblioteca limpia, un .lrc junto a la pista, o la etiqueta incrustada
settings-providers-save-lyrics-data-folder = Carpeta de datos
settings-providers-save-lyrics-sidecar = Archivo adjunto
settings-providers-save-lyrics-tag = Etiqueta
settings-providers-section-artist = Artista
settings-providers-section-cover-art = Carátulas
settings-providers-section-lyrics = Letras
settings-providers-section-metadata = Metadatos

## Settings: shader
settings-shader-backdrop-all-windows = Todas las ventanas
    .description = Sombrea el fondo de cada ventana: ajustes, editores, diálogos, paneles sacados a ventana. Desactivado lo deja en las ventanas del espacio de trabajo
settings-shader-backdrop-enabled = Shader de fondo
    .description = Ejecuta un shader WGSL reactivo a la música sobre el fondo de carátula, por debajo de todos los paneles. Forma parte del espacio de trabajo, así que viaja con el aspecto
settings-shader-backdrop-fallback-name = Fondo
settings-shader-backdrop-run-idle = Seguir en reposo
    .description = Sigue dibujando cuando no suena nada. La animación se queda congelada de todas formas
settings-shader-compile-error-title = Este shader no compiló
settings-shader-legacy-note = Sin nada enrutado, el conjunto llena los slots en su propio orden: la primera señal al slot 0, la segunda al slot 1, y así sucesivamente. La primera ruta que añadas se queda con toda la asignación.
settings-shader-overlay-enabled = Shader de superposición
    .description = Ejecuta un shader WGSL reactivo a la música sobre toda la ventana. Solo se ofrecen shaders que dejan la aplicación usable por debajo
settings-shader-scene-covers-window = Este shader es una escena, así que cubre la ventana en vez de dibujar encima. Vino de un paquete o de una configuración antigua; la lista de arriba solo ofrece shaders que dejan la aplicación usable.
settings-shader-screen-all-windows = Todas las ventanas
    .description = Sombrea también las ventanas secundarias: ajustes, estadísticas, ecualizador, paneles sacados a ventana. La cuenta atrás para revertir se queda sin sombrear de todas formas
settings-shader-screen-fallback-name = Pantalla
settings-shader-screen-run-idle = Seguir en reposo
    .description = Sigue dibujando cuando no suena nada. La animación se queda congelada de todas formas. Un shader que lee el ratón sigue al cursor con la música parada sin necesidad de esto; solo se detiene un par de segundos después que el puntero
settings-shader-section-backdrop = Shader de fondo
settings-shader-section-overlay = Shader de superposición
settings-shader-signals-block = Señales
    .description = Qué señal compartida lee cada uno de los dieciséis slots del shader
settings-shader-slots-block = Slots
    .description = Cada slot tal como llega al shader; los slots sin ruta son mandos que se ponen a mano

## Settings: storage
settings-storage-artist-images = Imágenes de artistas
    .description = Retratos, banners y biografías descargados para las vistas de artista (artists/); los que borres se vuelven a descargar la próxima vez que se abra una vista
settings-storage-catalog = Catálogo
    .description = El índice de pistas que construyen los escaneos: una fila por pista con sus etiquetas, los datos de su archivo y cualquier tramo de cue, dentro de library.db
settings-storage-cover-thumbnails = Miniaturas de carátula
    .description = Carátulas pequeñas guardadas tras su primer dibujado (thumbs.db); las que borres se rehacen según entran en pantalla
settings-storage-logs = Registros
    .description = Lo que escribe cada ejecución para informes de fallos (logs/rox.log), rotado con un límite de tamaño para que nunca crezca mucho
settings-storage-looks-layouts = Aspectos y disposiciones
    .description = El aspecto que está usando la aplicación (workspace.json) con tus espacios de trabajo guardados, los archivos de shader volcados y los paquetes de iconos al lado. Pequeño, y cada byte es algo que configuraste tú
settings-storage-lyrics = Letras
    .description = Letras descargadas y editadas guardadas en el almacén propio de la aplicación (lyrics/), para que las carpetas de la biblioteca queden limpias
settings-storage-measured-tempos = Tempos medidos
    .description = Los tempos que rox contó del audio, para pistas cuyas etiquetas no llevan ninguno; los números propios de las etiquetas no se tocan. Borrarlos devuelve esas pistas a la lista de Analizar lo que falta de la página Biblioteca, para que un conteo de tiempos mejorado pueda reemplazar los números que escribió una pasada anterior
settings-storage-model-fallback-this = Este modelo
settings-storage-music-summary = { $tracks }, { $albums }, { $size }
settings-storage-model-weights = Pesos del modelo
    .description = Los modelos descargados para el análisis acústico (models/). La página Modelos de ML es donde se descargan y se borran, una fila por modelo
settings-storage-models-empty = Modelos
    .description = Todavía nada ha descrito la biblioteca. Activar el análisis acústico en la página Biblioteca es lo que rellena esto, y cada modelo que se haya ejecutado tiene aquí su fila
settings-storage-music-files = Archivos de música
    .description = Lo que contienen las carpetas escaneadas; los archivos se quedan donde están
settings-storage-none = Ninguno
settings-storage-playlists-history = Listas e historial
    .description = Tus listas de reproducción y sus miembros, lo que has escuchado, y las notas de género de la biblioteca. Todo pequeño al lado del resto de library.db
settings-storage-reclaimable = Espacio recuperable
    .description = Páginas dentro de library.db que dejaron atrás los borrados. Las escrituras nuevas las vuelven a llenar, así que el archivo deja de crecer antes de empezar a encoger
    .keywords = vaciar compactar encoger base de datos
settings-storage-section-acoustic = Descripciones acústicas
settings-storage-section-app-data = Datos de la aplicación
settings-storage-section-caches = Cachés
settings-storage-section-diagnostics = Diagnóstico
settings-storage-section-library = Biblioteca
settings-storage-section-tempo = Tempo
settings-storage-vectors = Vectores
    .description = Lo que pesa cada descripción dentro de library.db. En una biblioteca por la que ha pasado el análisis esto es la mayor parte del archivo, un par de kilobytes por pista frente a unos cientos de bytes de etiquetas
settings-storage-waveforms = Formas de onda
    .description = La tira de picos de cada pista, guardada tras su primera reproducción; las que borres se vuelven a decodificar la próxima vez

## Settings: workspace
settings-workspace-card-author = Autor
settings-workspace-card-author-placeholder = Quién lo hizo
settings-workspace-card-created = Creado el { $date }
settings-workspace-card-created-updated = Creado el { $created }, actualizado el { $updated }
settings-workspace-card-description = Descripción
settings-workspace-card-description-placeholder = A qué apunta el aspecto
settings-workspace-card-empty = Este espacio de trabajo no tiene ficha
settings-workspace-card-hint = La ficha va dentro del archivo, así que la ve cualquiera con quien compartas este aspecto
settings-workspace-card-license = Licencia
settings-workspace-card-license-placeholder = Los términos con los que lo compartes
settings-workspace-card-save = Guardar la ficha
settings-workspace-card-updated = Actualizado el { $date }
settings-workspace-card-version = Versión
settings-workspace-card-version-placeholder = Tu propia versión, como quieras contarla
settings-workspace-card-website = Sitio web
settings-workspace-card-website-placeholder = Dónde vive
settings-workspace-composition-closed = La ventana del espacio de trabajo está cerrada
settings-workspace-composition-hint = Los paneles de la ventana tal como están en divisiones y grupos de pestañas; las flechas reordenan una fila entre sus hermanas, el candado fija un panel en su sitio, y el engranaje abre sus ajustes
settings-workspace-empty = Todavía no hay espacios de trabajo
settings-workspace-hint = Un espacio de trabajo es un aspecto entero: disposiciones, paleta, apariencia. Aplicar uno reemplaza los tres
settings-workspace-layout-name-placeholder = Nombre de la disposición
settings-workspace-layouts-empty = Todavía no hay disposiciones
settings-workspace-layouts-hint = Principal y mini son las dos entre las que alterna el botón de mini reproductor de la barra de menús
settings-workspace-name-placeholder = Nombre del espacio de trabajo
settings-workspace-panel-preset-unknown-kind = Panel desconocido
settings-workspace-panel-presets-empty = Todavía no hay preajustes de panel
settings-workspace-panel-presets-hint-after = en cualquier menú de panel. Son solo de este espacio de trabajo; otro no los tendrá.
settings-workspace-panel-presets-hint-before = Un panel configurado cada uno, guardado desde el menú del propio panel y recuperado desde
settings-workspace-role-mini = Mini
settings-workspace-role-primary = Principal
settings-workspace-section-composition = Composición
settings-workspace-section-layouts = Disposiciones
settings-workspace-section-panel-presets = Preajustes de panel
settings-workspace-section-workspaces = Espacios de trabajo
settings-workspace-tree-empty-slot = Slot vacío
settings-workspace-tree-split-column = Dividido, apilado
settings-workspace-tree-split-row = Dividido, lado a lado
settings-workspace-tree-tabs = Pestañas

## Settings: development
settings-development-experimental-panels = Paneles experimentales
    .description = Muestra los paneles todavía en construcción en el menú Paneles y en el lanzador; cambian de forma entre versiones, y una disposición que ya tenga uno lo conserva cuando esto se vuelva a apagar
settings-development-section-features = Funciones

## Settings: shared
settings-acoustic-analysis-heading = Análisis acústico
settings-analyze-nothing-scanned = Todavía no hay nada escaneado que analizar
settings-common-active = Activo
settings-common-analyze-missing = Analizar lo que falta
settings-common-built-in = Integrado
settings-common-cancel = Cancelar
settings-common-clear = Borrar
settings-common-copy = Copiar
settings-common-database = Base de datos
settings-common-delete = Eliminar
settings-common-download = Descargar
settings-common-rescan = Volver a escanear
settings-common-reveal = Mostrar
settings-common-stop = Detener
settings-common-stopping = Deteniendo...
settings-common-tags = Etiquetas
settings-common-tracks-count = { $count ->
    [one] { $count } pista
   *[other] { $count } pistas
}
settings-common-use = Usar
settings-confirm-apply-body = Esto reemplaza tus disposiciones, tu paleta y tu apariencia por las del espacio de trabajo.
settings-confirm-apply-imported-body = Está guardado en tus espacios de trabajo. Aplicarlo ahora reemplaza tus disposiciones, tu paleta y tu apariencia por las del espacio de trabajo.
settings-confirm-clear = Borrar
settings-confirm-clear-embeddings-body = Las descripciones se van y el espacio vuelve. Tenerlas de nuevo significa volver a pasar el análisis por todas las pistas de la biblioteca.
settings-confirm-clear-embeddings-title = ¿Borrar lo que describió "{ $model }"?
settings-confirm-clear-measured-bpm-body = Cada tempo que calculó rox vuelve a quedar sin medir; los números de las etiquetas de tus archivos se quedan. Tenerlos de nuevo significa volver a pasar el análisis de tempo por cada una de esas pistas.
settings-confirm-clear-measured-bpm-title = ¿Borrar los tempos medidos?
settings-confirm-overwrite-workspace-body = Esto reemplaza el espacio de trabajo guardado por el estado actual.
settings-confirm-overwrite-workspace-title = ¿Sobrescribir el espacio de trabajo "{ $name }"?
settings-sidebar-data-folder = Carpeta de datos
settings-sidebar-settings-file = Archivo de ajustes

## Menubar
menu-about = Acerca de
menu-analyze-tempo = Analizar el tempo...
menu-application = Aplicación
menu-apply-layout = Aplicar disposición
menu-apply-workspace = Aplicar espacio de trabajo
menu-build-acoustic = Crear vectores acústicos...
menu-chat = Chat
menu-close = Cerrar
menu-console = Consola
menu-design-mode = Modo de diseño
menu-discussions = Debates
menu-empty-window = Ventana vacía
menu-equalizer = Ecualizador
menu-exit = Salir
menu-fill-sort-names = Rellenar los nombres de ordenación...
menu-romanize-library = Romanizar la biblioteca...
menu-find-duplicates = Buscar duplicados...
menu-tag-genres = Etiquetar géneros...
menu-health = Salud de la biblioteca
menu-power-search = Búsqueda avanzada
menu-hide-menubar = Ocultar la barra de menús
menu-import-workspace = Importar espacio de trabajo...
menu-library = Biblioteca
menu-measure-replaygain = Medir ReplayGain...
menu-new-ellipsis = Nuevo...
menu-new-window = Ventana nueva
menu-new-window-from-layout = Ventana nueva desde una disposición
menu-new-window-from-panel = Ventana nueva desde un panel
menu-no-layouts = No hay disposiciones
menu-no-presets = No hay preajustes
menu-no-workspaces = No hay espacios de trabajo
menu-os-decorations = Decoraciones del sistema
menu-overlay-shader = Shader de superposición
menu-panel-built-in = Integrado
menu-panel-new = Nuevo...
menu-panel-no-layouts = No hay disposiciones
menu-panel-no-presets = No hay preajustes
menu-panel-no-workspaces = No hay espacios de trabajo
menu-panel-title = Menú
menu-panels = Paneles
menu-panels-presets = Preajustes
menu-pause = Pausa
menu-playback = Reproducción
menu-remain-in-tray = Quedarse en la bandeja
menu-report-issue = Informar de un problema
menu-rescan-library = Volver a escanear la biblioteca
menu-save-layout = Guardar disposición
menu-save-workspace = Guardar espacio de trabajo
menu-section-add = Añadir
menu-section-analyze = Analizar
menu-section-app = App
menu-section-interface = Interfaz
menu-section-layouts = Disposiciones
menu-section-listening = Escucha
menu-section-maintain = Mantener
menu-section-session = Sesión
menu-section-track = Pista
menu-section-tuning = Ajuste
menu-settings = Ajustes
menu-signals = Señales
menu-song-theming = Color de la canción
menu-stats = Estadísticas
menu-tasks = Tareas
menu-update-available = Actualización disponible
menu-welcome = Bienvenida
menu-window = Ventana
menu-workspace = Espacio de trabajo
menu-workspace-builtin-tag = Integrado

## Workspaces
workspace-apply-body = Esto reemplaza el aspecto entero: disposiciones, paleta, apariencia.
workspace-apply-imported-body = Está guardado en tus espacios de trabajo. Aplicarlo ahora reemplaza el aspecto entero: disposiciones, paleta, apariencia.
workspace-apply-imported-title = "{ $name }" importado
workspace-apply-screen-shader-named = Aplica el shader de superposición { $name } sobre toda la ventana.
workspace-apply-screen-shader-plain = Aplica un shader de superposición sobre toda la ventana.
workspace-apply-shader-count = { $count ->
    [one] Incluye { $count } shader: { $names }
   *[other] Incluye { $count } shaders: { $names }
}
workspace-apply-shaders-approve-body = Aprobarlos les deja correr en esta máquina. Aplicarlo sin ellos deja el aspecto desnudo, con los shaders todavía en su conjunto.
workspace-apply-shaders-plain-body = Aplicarlo sin ellos deja el aspecto desnudo, con los shaders todavía en su conjunto.
workspace-byline-author = de { $author }
workspace-byline-version = versión { $version }
workspace-context-add-panel = Añadir panel
workspace-dialog-apply = Aplicar
workspace-dialog-apply-title = ¿Aplicar "{ $name }"?
workspace-dialog-approve-apply = Aprobar y aplicar
workspace-dialog-cancel = Cancelar
workspace-dialog-close = Cerrar
workspace-dialog-close-title = ¿Cerrar "{ $name }"?
workspace-dialog-export = Exportar
workspace-dialog-layout-name-placeholder = Nombre de la disposición
workspace-dialog-not-now = Ahora no
workspace-dialog-overwrite = Sobrescribir
workspace-dialog-overwrite-title = ¿Sobrescribir "{ $name }"?
workspace-dialog-save = Guardar
workspace-dialog-save-layout-title = Guardar disposición
workspace-dialog-save-workspace-title = Guardar espacio de trabajo
workspace-dialog-with-shaders = Con shaders
workspace-dialog-without-shaders = Sin shaders
workspace-dialog-workspace-name-placeholder = Nombre del espacio de trabajo
workspace-drop-add-queue = Añadir a la cola
workspace-drop-play-now = Reproducir ahora
workspace-hint-or = o
workspace-hint-then = y luego
workspace-import = Importar
workspace-launcher-hint = Añade tu primer panel para empezar a construir, o elige un preajuste en Espacio de trabajo > Aplicar espacio de trabajo
workspace-launcher-need-help = ¿Necesitas ayuda?
workspace-launcher-open-welcome = Abrir la ventana de bienvenida
workspace-launcher-title = Una ventana vacía
workspace-layout-apply-body = Esto reemplaza la disposición actual de esta ventana.
workspace-layout-overwrite-body = Esto reemplaza la disposición guardada por la actual.
workspace-layout-preset-restore-failed = No se pudo restaurar el preajuste de disposición de esta ventana, así que arranca vacía.
workspace-layout-restore-failed = No se pudo restaurar la disposición guardada, así que esta ventana arranca vacía.
workspace-mini-tip-back = Volver a la disposición completa
workspace-mini-tip-shrink = Encoger al mini reproductor
workspace-overwrite-body = Esto reemplaza el espacio de trabajo guardado por el aspecto actual.
workspace-panel-locked-close-body = Este panel está fijado en su sitio. Cerrarlo lo saca de la disposición.
workspace-save-current = Guardar el actual
workspace-screen-shader-hint-before = Desactívalo cuando quieras con
workspace-workspace-restore-failed = No se pudo restaurar la disposición del espacio de trabajo, así que esta ventana arranca vacía.

## Tasks window
tasks-acoustic-all-described = { $count ->
    [one] La única pista escaneada está descrita por { $label }
   *[other] Las { $count } pistas escaneadas están descritas por { $label }
}
tasks-acoustic-off = Describir cómo suenan las pistas está desactivado en Ajustes, dentro de Biblioteca
tasks-acoustic-partial = { $label } describe { $embedded } de { $total } pistas escaneadas
tasks-analyzing = Analizando { $progress }
tasks-bake-writing = Escribiendo etiquetas...
tasks-chip-count = { $count } tareas
tasks-convert-starting = Arrancando ffmpeg...
tasks-converting = Convirtiendo { $progress }
tasks-count-of-total = { $done } de { $total }
tasks-embedding = Incrustando { $progress }
tasks-estimate-at = { $estimate } con { $workers }
tasks-import-failed = La última importación falló: { $error }
tasks-import-reading = Leyendo la lista de favoritas...
tasks-import-unmatched = { $count ->
    [one] { $count } no tenía equivalente en esta biblioteca
   *[other] { $count } no tenían equivalente en esta biblioteca
}
tasks-importing = Importando { $progress }
tasks-job-acoustic = Análisis acústico
tasks-job-convert = Convertir audio
tasks-job-loved-import = Pistas favoritas de Last.fm
tasks-job-replaygain = ReplayGain
tasks-job-scan = Escaneo de la biblioteca
tasks-job-tempo = Análisis de tempo
tasks-last-pass-stopped = La última pasada se detuvo: { $reason }
tasks-last-run-finished = { $count ->
    [one] La última ejecución terminó, { $count } hecha
   *[other] La última ejecución terminó, { $count } hechas
}
tasks-last-run-stopped = La última ejecución se detuvo tras { $count }
tasks-library-busy = La biblioteca está ocupada
tasks-library-scanning = La biblioteca está escaneando
tasks-measuring = Midiendo { $progress }
tasks-model-downloading = Todavía se está descargando un modelo
tasks-no-library-window = No hay ninguna ventana de biblioteca abierta, así que esto no se puede arrancar desde aquí
tasks-nothing-to-measure = Todavía no hay nada escaneado que medir
tasks-rg-all-gain = { $count ->
    [one] La única pista tiene una ganancia con la que sonar
   *[other] Las { $count } pistas tienen una ganancia con la que sonar
}
tasks-rg-partial = { $missing ->
    [one] { $missing } de { $total } pistas no tiene ganancia
   *[other] { $missing } de { $total } pistas no tienen ganancia
}
tasks-scan-folder-count = { $count ->
    [one] { $count } carpeta
   *[other] { $count } carpetas
}
tasks-scan-last-scanned = { $folders }, escaneadas hace { $ago }
tasks-scan-never-scanned = { $folders }, nunca escaneadas
tasks-scan-no-folders = Todavía no has añadido carpetas. Añade una en Ajustes, dentro de Biblioteca
tasks-start-analyze-missing = Analizar lo que falta
tasks-start-measure-missing = Medir lo que falta
tasks-start-rescan = Volver a escanear
tasks-stop = Detener
tasks-stopping = Deteniendo...
tasks-tempo-all = { $count ->
    [one] La única pista tiene tempo
   *[other] Las { $count } pistas tienen tempo
}
tasks-tempo-counted = { $count ->
    [one] La única pista tiene tempo
   *[other] { $count } pistas tienen tempo
}
tasks-tempo-off = Averiguar a qué velocidad van las pistas está desactivado en Ajustes, dentro de Biblioteca
tasks-tempo-partial = { $missing ->
    [one] { $missing } de { $total } pistas no tiene tempo
   *[other] { $missing } de { $total } pistas no tienen tempo
}
tasks-tempo-refused = { $count ->
    [one] rox no pudo contar el pulso de 1 pista
   *[other] rox no pudo contar el pulso de { $count } pistas
}
tasks-timing = Contando { $progress }
tasks-filling = Rellenando { $progress }
tasks-job-sortnames = Nombres de ordenación
tasks-sortnames-all = { $count ->
    [one] El único artista tiene nombre de ordenación
   *[other] Los { $count } artistas tienen nombre de ordenación
}
tasks-sortnames-non-latin = , { $count } de ellos fuera del alfabeto latino, { $estimate }
tasks-sortnames-nothing = Todavía no hay nada escaneado que buscar
tasks-sortnames-partial = { $missing } de { $total } artistas no tienen nombre de ordenación
tasks-start-fill-missing = Rellenar lo que falta
tasks-job-romanize = Romanizar
tasks-reading-takes = , leerlos tarda { $estimate }
tasks-romanize-all = { $count ->
    [one] El único título, álbum o artista tiene nombre de ordenación
   *[other] Los { $count } títulos, álbumes y artistas tienen nombre de ordenación
}
tasks-romanize-nothing = Todavía no hay nada escaneado que leer
tasks-romanize-partial = { $missing } de { $total } títulos, álbumes y artistas no tienen nombre de ordenación
tasks-romanizing = Leyendo { $progress }
tasks-romanize-skipped = Se omitieron { $count } por falta del diccionario japonés
tasks-romanize-skipping = { $kanji } de ellos son kanji y necesitan el diccionario japonés de Ajustes > Biblioteca
tasks-start-romanize = Romanizar
tasks-tip = Abrir las tareas de la biblioteca
tasks-window-title = rox - Tareas
tasks-working-out-missing = Calculando lo que falta...

## Stats window
stats-bars-daily = Barras diarias, haz clic en una para abrirla
stats-bars-days = Barras de { $days } días, haz clic en una para abrirlas
stats-bars-hourly = Barras por hora, la vista más cercana
stats-bars-hours = Barras de { $hours } horas, haz clic en una para abrir su día
stats-bars-weekly = Barras semanales, haz clic en una para abrirla
stats-bucket-listens = { $count ->
    [one] { $count } escucha, { $ago } ({ $date })
   *[other] { $count } escuchas, { $ago } ({ $date })
}
stats-chart-end-day = Medianoche
stats-chart-start-all = Primera escucha
stats-chart-start-month = Hace 30 días
stats-chart-start-week = Hace 7 días
stats-chart-start-year = Hace un año
stats-click-opens = El clic abre las estadísticas
stats-click-section = Clic
stats-count-menu = Cantidad
    .description = Sobre qué ventana de tiempo cuenta escuchas el número; la lista al pasar el cursor las muestra siempre todas
stats-empty-all = Todavía no hay escuchas
stats-empty-range = No hay escuchas en este rango
stats-library-held = { $tracks } pistas, { $size } en memoria
stats-now = Ahora
stats-open = Abrir estadísticas
stats-open-on-click = Abrir las estadísticas al hacer clic
    .description = Haz clic en el widget para abrir la ventana de estadísticas, el registro completo de escuchas
stats-play-these-tracks = Reproducir estas pistas
stats-play-this-track = Reproducir esta pista
stats-plays-count = { $count ->
    [one] { $count } reproducción
   *[other] { $count } reproducciones
}
stats-range-all = Desde siempre
stats-range-all-short = Todo
stats-range-day-short = Día
stats-range-label = Rango
stats-range-month = Este mes
stats-range-month-short = Mes
stats-range-span = Del { $from } al { $to }
stats-range-today = Hoy
stats-range-week = Esta semana
stats-range-week-short = Semana
stats-range-year = Este año
stats-range-year-short = Año
stats-readout-section = Lectura
stats-section-listens = Escuchas
stats-section-listens-over-time = Escuchas a lo largo del tiempo
stats-section-recent-listens = Escuchas recientes
stats-section-top-albums = Álbumes más escuchados
stats-section-top-artists = Artistas más escuchados
stats-section-top-genres = Géneros más escuchados
stats-show-change = Mostrar el cambio
    .description = Añade una ficha con cómo se compara el periodo con el anterior, arriba o abajo; Desde siempre no tiene nada detrás
stats-show-number = Mostrar el número
    .description = Dibuja la cantidad junto al icono; desactivado deja un icono a secas con las cantidades al pasar el cursor
stats-title = Widget de estadísticas
stats-tooltip-listens = Escuchas
stats-window-title = rox - Estadísticas

## Library health window

health-caption-art = { $albums } de { $total }, { $tracks }
health-caption-duplicates = { $groups } en { $tracks }
health-caption-formats = { $unwritable } de { $total }
health-caption-gaps = { $albums } de { $total }
health-caption-missing = { $missing } que faltan de { $total }
health-caption-sort = Artistas del álbum { $album_artists }, álbumes { $albums }, títulos { $titles }
health-caption-split = { $tagged } etiquetados, { $measured } medidos, { $missing } faltantes
health-caption-split-refused = { $tagged } etiquetados, { $measured } medidos, { $missing } faltantes, { $refused } sin pulso
health-checks-menu = Etiquetas contadas
    .description = Cuáles de las cinco etiquetas principales cuenta la lectura; la lista al pasar el cursor siempre las muestra todas
health-click-opens = El clic abre la salud de la biblioteca
health-click-section = Clic
health-complete = No falta nada
health-count-groups = { $count ->
    [one] { $count } grupo
   *[other] { $count } grupos
}
health-desc-acoustic = Pistas sin huella acústica, así que no se les puede encontrar nada parecido.
health-desc-art = Álbumes sin carátula, ni incrustada en los archivos ni junto a ellos como imagen.
health-desc-duplicates = Grupos de pistas que comparten artista y título y duran más o menos lo mismo.
health-desc-gaps = Álbumes cuya numeración de pistas se salta un número, o donde alguna pista no lleva ninguno.
health-desc-genre = Pistas cuyos archivos no llevan género.
health-desc-rating = Pistas que aún no has valorado.
health-desc-replaygain = Pistas sin medición de volumen, así que suenan más altas o más bajas que el resto.
health-desc-sort-names = Cuántos nombres llevan nombre de ordenación, la grafía que decide su lugar alfabético.
health-desc-tempo = Pistas sin tempo, que es justo lo que leen la ordenación y la coincidencia por BPM.
health-desc-writable = Pistas en formatos que rox puede leer pero en los que no puede escribir etiquetas. Los archivos MP4 fragmentados también rechazan la escritura y no se cuentan aquí.
health-desc-year = Pistas sin año de publicación.
health-drill = Mostrar estos
health-fix-analyze = Analizar lo que falta
health-fix-duplicates = Abrir duplicados
health-fix-genres = Etiquetar géneros
health-fix-measure = Medir lo que falta
health-fix-fill = Rellenar lo que falta
health-measuring-art = Explorando carátulas de álbum, { $done } de { $total }
health-measuring-duplicates = Comparando duplicados
health-measuring-formats = Leyendo formatos de archivo
health-measuring-gaps = Comprobando números de pista
health-open = Abrir la salud de la biblioteca
health-open-on-click = Abrir la salud de la biblioteca al hacer clic
    .description = Haz clic en el widget para abrir la ventana de salud de la biblioteca, donde se desglosa la cobertura
health-overview-complete = { $complete } de { $total } totalmente etiquetados
health-overview-missing = { $missing } faltantes
health-readout-section = Lectura
health-running = En curso
health-section-audio = Audio
health-section-files = Archivos y estructura
health-section-overview = Resumen
health-section-tags = Etiquetas
health-show-percent = Mostrar el porcentaje
    .description = Dibuja la cobertura junto al icono; desactivado deja un icono a secas con las cantidades al pasar el cursor
health-tile-acoustic = Vectores acústicos
health-tile-album = Álbum
health-tile-art = Carátula del álbum
health-tile-artist = Artista
health-tile-duplicates = Duplicados
health-tile-gaps = Huecos de álbum
health-tile-genre = Género
health-tile-rating = Valoración
health-tile-replaygain = ReplayGain
health-tile-sort-names = Nombres de ordenación
health-tile-tempo = Tempo
health-tile-writable = No compatibles
health-tile-year = Año
health-tile-title = Título
health-tooltip-missing = Etiquetas faltantes
health-waiting = En espera
health-widget-title = Widget de salud
health-window-title = rox - Salud de la biblioteca

## Power search window

search-seed-caption = { $source }: { $count }
search-window-title = rox - Búsqueda avanzada

## About window
about-check-failed = No se pudo llegar a GitHub
about-check-for-updates = Buscar actualizaciones
about-checking = Comprobando...
about-download = Descargar
about-downloading = Descargando... { $percent }%
about-get-it = Conseguirla
about-license-lead = rox es software libre bajo la GNU AGPLv3. El código está en
about-notice-lead = Deberías haber recibido una copia de la licencia con este programa. Si no, mira en
about-release-notes = Notas de la versión
about-restart-now = Reiniciar ahora
about-up-to-date = Tienes la última versión
about-update-failed = La actualización falló: { $error }
about-version = Versión { $version }
about-version-available = La versión { $version } está disponible
about-version-ready = La versión { $version } está lista
about-window-title = rox - Acerca de

## Welcome window
welcome-add-folder = Añadir carpeta
welcome-and = y
welcome-back = Atrás
welcome-card-menubar-title = Barra de menús
welcome-card-music-title = Música
welcome-card-panels-title = Paneles
welcome-card-playback-title = Reproducción
welcome-card-rearranging-title = Reorganizar
welcome-card-settings-title = Ajustes
welcome-close = Cerrar
welcome-design-mode-note = Reorganizar necesita el modo de diseño, activado por defecto arriba de ese menú. Desactivado bloquea la disposición, así una configuración terminada no se descoloca.
welcome-done = Listo
welcome-drop-note = Suéltalo en el borde de un panel para dividir ahí, en el medio para compartir un grupo de pestañas, o fuera de la ventana para que sea su propia ventana.
welcome-key-left-click = Clic izquierdo
welcome-key-middle-mouse = Botón central
welcome-layout-note = Guarda una organización como disposición; un espacio de trabajo junta disposiciones y paleta en un aspecto que se puede compartir.
welcome-menubar-after = dos veces para dejarla fija.
welcome-menubar-before = Con la barra de menús oculta, mantén
welcome-menubar-mid = para hacerla flotar sobre el dock, o pulsa
welcome-music-note = rox la escanea a la biblioteca y los archivos se quedan donde están. Más carpetas se añaden en los ajustes, dentro de biblioteca.
welcome-next = Siguiente
welcome-or = o
welcome-panels-note = Cada superficie es un panel, y el menú Paneles de la barra de menús abre más.
welcome-playback-after = busca.
welcome-playback-before = alterna la reproducción;
welcome-quickplay-after = y suena.
welcome-quickplay-before = abre la reproducción rápida: escribe una pista, pulsa
welcome-rearrange-after = en cualquier punto de un panel para moverlo.
welcome-rearrange-before = Arrastra una pestaña, o mantén
welcome-settings-hint-after = abre los ajustes: la paleta, la transparencia y el comportamiento.
welcome-shelf-caption = Elegir uno reemplaza el aspecto de la ventana principal y cierra el recorrido. Esta ventana está siempre disponible en Aplicación > Bienvenida.
welcome-stage-lead-quick-start = Elige un espacio de trabajo y la ventana principal cambia a él: disposiciones, paleta, el aspecto entero.
welcome-stage-lead-welcome = Foobar si se hubiera hecho en 20XX.
welcome-stage-title-quick-start = Inicio rápido
welcome-stage-title-welcome = Bienvenido a rox
welcome-step-hint-after = , o con los botones de abajo.
welcome-step-hint-before = Avanza paso a paso con
welcome-tile-by = de { $author }
welcome-tour-intro = Un recorrido rápido por dónde entra la música y dónde se configura el aspecto. Termina en el estante de espacios de trabajo incluidos, a un clic cada uno.
welcome-window-title = rox - Bienvenida

## Console window
console-clear = Vaciar
console-copy = Copiar
console-empty-filtered = Nada en estos niveles
console-empty-none = Todavía no hay nada registrado
console-filter-error = Error
console-filter-info = Info
console-filter-warn = Aviso
console-follow = Seguir
console-line-count = { $count ->
    [one] { $count } línea
   *[other] { $count } líneas
}
console-open-button = Abrir la consola
console-reveal = Mostrar
console-window-title = rox - Consola

## Signals window
signals-about-toggle = Sobre las señales
signals-blurb-marked = Los paneles marcados con esto en los menús pueden enlazar casi todos sus parámetros: haz clic derecho en un parámetro de los ajustes del panel y elige una señal, o añade una desde ahí.
signals-blurb-shared = Lo que se ajusta aquí es compartido: un cambio se aplica a todos los parámetros enrutados a esa señal, en todos los paneles y ventanas.
signals-blurb-total = Un Total es la cuarta clase: suma otra señal a lo largo del tiempo y da la vuelta en 1, así que sube mientras la música está fuerte y se estanca mientras no lo está. Úsalo cuando un shader necesite una fase que avance con la canción y no con el reloj.
signals-blurb-what = Una señal convierte lo que suena en un número entre 0 y 1: la energía de una banda de frecuencias, el nivel de toda la mezcla, o un pulso en cada golpe dentro de una banda. Respuesta marca la rapidez con que la sigue, Umbral la silencia por debajo de un nivel que elijas.
signals-no-library = No hay ninguna ventana de biblioteca abierta, así que estas no muestran audio. Los cambios se guardan igual.
signals-window-title = rox - Señales

## Equaliser
eq-analyzer-bars = Barras
eq-analyzer-off = Sin analizador
eq-analyzer-wave = Onda
eq-band-badge = Distintivo de bandas
    .description = Muestra cuántas bandas están fuera de plano, en un distintivo sobre el icono
eq-band-label = Banda { $number }
eq-click-nothing = Nada
eq-click-open = Abrir
eq-click-section = Clic
    .description = Qué hace un clic: abrir la ventana del ecualizador, o activar y desactivar toda la curva sin salir de aquí
eq-click-toggle = Alternar
eq-flatten = Aplanar
eq-freq-label = Frec
eq-gain-label = Ganancia
eq-heading = Ecualizador
eq-help-text = Arrastra una banda para moverla, desplaza la rueda sobre ella para ensancharla o estrecharla. El procesado va por delante del búfer que alimenta la tarjeta de sonido, así que un movimiento tarda hasta medio segundo en llegar a los altavoces.
eq-hint-off = Haz clic para desactivarlo
eq-hint-on = Haz clic para activarlo
eq-hint-open = Haz clic para abrir el ecualizador
eq-open = Abrir el ecualizador
eq-readout-curve = Curva
eq-readout-icon = Icono
eq-readout-section = Lectura
    .description = El icono, la curva de respuesta como minigráfico, o ambos. La curva necesita unos cincuenta píxeles de ancho para leerse
eq-reset-bands = Restablecer bandas
eq-shape-active = { $count ->
    [one] { $count } banda fuera de plano, pico { $peak } dB
   *[other] { $count } bandas fuera de plano, pico { $peak } dB
}
eq-shape-flat = Plano, todas las bandas a 0 dB
eq-status-off = Ecualizador desactivado
eq-status-on = Ecualizador activado
eq-title = Widget de EQ
eq-widget-section = Widget
eq-width-label = Ancho
eq-window-title = rox - Ecualizador

## Keymap
keymap-close-window = Cerrar la ventana
    .description = Cierra la ventana que esté delante. Asignado en todas partes, paneles sacados a ventana incluidos
keymap-decrease-font-size = Reducir el tamaño del texto
    .description = Baja un paso el tamaño del texto de toda la aplicación
keymap-focus-search = Enfocar la búsqueda
    .description = Pon el cursor en el campo de búsqueda de la biblioteca
keymap-group-browsing = Navegación
keymap-group-editing = Edición
keymap-group-library = Biblioteca
keymap-group-playback = Reproducción
keymap-group-view = Vista
keymap-group-windows = Ventanas
keymap-increase-font-size = Aumentar el tamaño del texto
    .description = Sube un paso el tamaño del texto de toda la aplicación
keymap-key-backspace = Retroceso
keymap-key-delete = Supr
keymap-key-down = Abajo
keymap-key-end = Fin
keymap-key-esc = Esc
keymap-key-home = Inicio
keymap-key-insert = Insert
keymap-key-left = Izquierda
keymap-key-page-down = Av Pág
keymap-key-page-up = Re Pág
keymap-key-right = Derecha
keymap-key-space = Espacio
keymap-key-tab = Tab
keymap-key-up = Arriba
keymap-mod-alt = Alt
keymap-mod-cmd = Cmd
keymap-mod-ctrl = Ctrl
keymap-mod-fn = Fn
keymap-mod-option = Option
keymap-mod-shift = Mayús
keymap-mod-super = Super
keymap-mod-win = Win
keymap-new-window = Ventana nueva
    .description = Abre otra ventana de trabajo con la disposición guardada
keymap-next-track = Pista siguiente
    .description = Salta a la pista siguiente de la cola
keymap-open-about = Acerca de
    .description = Muestra la versión y los créditos
keymap-open-console = Consola
    .description = Abre la consola de registro
keymap-open-equalizer = Ecualizador
    .description = Abre la ventana del ecualizador
keymap-open-quick-play = Reproducción rápida
    .description = Levanta sobre la ventana el cuadro de buscar y reproducir
keymap-open-settings = Abrir los ajustes
    .description = Abre esta ventana
keymap-open-panel-settings = Ajustes del panel
    .description = Abre la ventana de ajustes del panel enfocado
keymap-open-health = Salud de la biblioteca
    .description = Abre la ventana de salud de la biblioteca, donde se cuentan la cobertura de etiquetas y los problemas estructurales
keymap-open-power-search = Búsqueda avanzada
    .description = Abre una ventana de búsqueda con su propia consulta, para que buscar aquí no altere el espacio de trabajo
keymap-open-stats = Abrir las estadísticas
    .description = Abre la ventana de estadísticas de escucha
keymap-open-tasks = Tareas
    .description = Muestra en qué está trabajando rox en segundo plano
keymap-open-welcome = Bienvenida
    .description = Vuelve a abrir la ventana de bienvenida
keymap-play-random = Reproducir al azar
    .description = Saca una pista al azar de la biblioteca y la reproduce
keymap-previous-track = Pista anterior
    .description = Vuelve a la pista anterior
keymap-quit = Salir
    .description = Cierra rox. Asignado en todas partes, porque no hay ventana desde la que no deba funcionar
keymap-reset-font-size = Restablecer el tamaño del texto
    .description = Devuelve el tamaño del texto al de fábrica
keymap-seek-backward = Retroceder
    .description = Retrocede un paso dentro de la pista en reproducción
keymap-seek-forward = Avanzar
    .description = Avanza un paso dentro de la pista en reproducción
keymap-stamp-line = Marcar la línea de letra
    .description = Escribe la posición de reproducción en la línea de letra que estés editando
keymap-stop-playback = Detener
    .description = Detiene la reproducción y libera la pista
keymap-toggle-playback = Reproducir / Pausar
    .description = Arranca la pista actual, o la pausa donde esté
keymap-toggle-post-shader = Alternar el shader de superposición
    .description = Activa y desactiva el shader de pantalla. Asignado en todas partes, porque un shader puede tapar los controles con los que normalmente lo desactivarías
keymap-toggle-zoom = Ampliar el grupo de paneles
    .description = Llena el dock con el último grupo de paneles en el que hiciste clic, o sal de ahí
keymap-type-ahead-next = Coincidencia siguiente
    .description = Salta a la fila siguiente que coincide con lo escrito
keymap-type-ahead-prev = Coincidencia anterior
    .description = Vuelve a la coincidencia anterior de lo escrito
keymap-next-tab = Pestaña siguiente
    .description = Muestra la pestaña siguiente del grupo de paneles enfocado
keymap-prev-tab = Pestaña anterior
    .description = Muestra la pestaña anterior del grupo de paneles enfocado
keymap-toggle-mute = Silenciar
    .description = Silencia la salida sin perder el nivel. Pulsa otra vez para recuperarla
keymap-toggle-shuffle = Alternar aleatorio
    .description = Activa o desactiva la reproducción aleatoria de la cola
keymap-cycle-loop = Alternar repetición
    .description = Pasa la repetición de apagada a todo, a una y vuelta a empezar
keymap-toggle-stop-after = Parar después
    .description = Deja terminar la pista actual y luego pausa. Pulsa otra vez para desactivarlo
keymap-volume-up = Subir volumen
    .description = Sube el volumen un paso
keymap-volume-down = Bajar volumen
    .description = Baja el volumen un paso
keymap-close-panel = Cerrar panel
    .description = Cierra el panel activo del grupo de paneles enfocado
keymap-new-empty-window = Ventana vacía
    .description = Abre una ventana de espacio de trabajo sin nada dentro
keymap-open-signals = Señales
    .description = Abre la ventana de señales, el conjunto detrás de las rutas de cada panel
keymap-import-workspace = Importar espacio de trabajo
    .description = Elige un archivo de espacio de trabajo y lo añade a la colección
keymap-toggle-quit-to-tray = Alternar quedarse en la bandeja
    .description = Cambia si cerrar la última ventana deja a rox en la bandeja
keymap-toggle-design-mode = Alternar modo de diseño
    .description = Cambia si los paneles se pueden reorganizar en el sitio
keymap-toggle-theme = Alternar claro / oscuro
    .description = Cambia al otro lado de la paleta. Vinculado en todas partes, ya que todas las ventanas comparten el tema
keymap-toggle-resize-lock = Alternar bloqueo del tamaño de los paneles
    .description = Cambia si el tamaño de los paneles solo se ajusta en modo de diseño
keymap-toggle-menubar = Alternar ocultar la barra de menús
    .description = Muestra la barra de menús de la ventana, o la oculta hasta que se mantenga Alt
keymap-toggle-decorations = Alternar decoraciones del sistema
    .description = Cambia las ventanas del espacio de trabajo entre el marco del sistema y el propio de rox
keymap-toggle-art-theming = Alternar color de la canción
    .description = Cambia si la carátula de la pista actual tiñe la paleta
keymap-rescan-library = Volver a escanear la biblioteca
    .description = Escanear otra vez todas las carpetas recordadas de la biblioteca
keymap-measure-replaygain = Medir ReplayGain
    .description = Abrir el diálogo que mide el volumen de las pistas que no lo llevan
keymap-analyze-tempo = Analizar el tempo
    .description = Abrir el diálogo que escucha el pulso de las pistas sin BPM
keymap-build-acoustic = Crear vectores acústicos
    .description = Abrir el diálogo que construye los vectores que lee la búsqueda acústica
keymap-fill-sort-names = Rellenar los nombres de ordenación
    .description = Abrir el diálogo que pide a MusicBrainz los nombres de ordenación que faltan en los archivos
keymap-romanize-library = Romanizar la biblioteca
    .description = Abrir el diálogo que lee en letras latinas los títulos, álbumes y artistas que no lo están
keymap-find-duplicates = Buscar duplicados
    .description = Abrir el buscador de duplicados sobre la biblioteca
keymap-tag-genres = Etiquetar géneros
    .description = Abrir el etiquetador de géneros sobre las pistas sin género

## Panel catalog
panel-catalog-album-carousel = Carrusel de álbumes
panel-catalog-artist-grid = Cuadrícula de artistas
panel-catalog-biography = Biografía
panel-catalog-cover-art = Carátula
panel-catalog-drawer = Cajón
panel-catalog-eq-widget = Widget de EQ
panel-catalog-filter = Filtro
panel-catalog-folder-tree = Árbol de carpetas
panel-catalog-genre-grid = Cuadrícula de géneros
panel-catalog-health-widget = Widget de salud
panel-catalog-group-application = Aplicación
panel-catalog-group-arrangement = Organización
panel-catalog-group-catalogue = Catálogo
panel-catalog-group-controls = Controles
panel-catalog-group-details = Detalles
panel-catalog-group-experimental = Experimentales
panel-catalog-group-visualizers = Visualizadores
panel-catalog-history = Historial
panel-catalog-menu = Menú
panel-catalog-metadata = Metadatos
panel-catalog-mini-toggle = Alternar mini
panel-catalog-oscilloscope = Osciloscopio
panel-catalog-overlay = Superposición
panel-catalog-particles = Partículas
panel-catalog-playlists = Listas de reproducción
panel-catalog-queue = Cola
panel-catalog-queue-widget = Widget de cola
panel-catalog-seek = Posición
panel-catalog-slide = Diapositiva
panel-catalog-spectrogram = Espectrograma
panel-catalog-spectrum = Espectro
panel-catalog-stats-widget = Widget de estadísticas
panel-catalog-status = Estado
panel-catalog-theme-toggle = Alternar tema
panel-catalog-track-info = Info de la pista
panel-catalog-vu-meter = Medidor VU
panel-catalog-waveform = Forma de onda
panel-catalog-window-controls = Controles de ventana

## Updater
updater-already-latest = ya estás en la última versión
updater-checksum-mismatch = la suma de comprobación de la descarga es { $digest }, no la { $expected } que indica la publicación
updater-checksum-missing-entry = { $sums } no tiene entrada para { $name }; se rechaza una descarga que no se puede verificar
updater-no-asset = la publicación no tiene { $name }
updater-no-checksums = la publicación no tiene { $sums }; se rechaza una descarga que no se puede verificar
updater-no-release-build = no hay compilación publicada para esta plataforma
updater-overran = la descarga se pasó del tamaño que indica la publicación
updater-short = la descarga se detuvo en { $done } de { $bytes } bytes
updater-size-mismatch = el servidor ofreció { $claimed } bytes, la publicación indica { $bytes }

## Last.fm
lastfm-import-matching = Buscando coincidencias en la biblioteca
lastfm-import-read = { $count ->
    [one] Leída { $count } pista favorita
   *[other] Leídas { $count } pistas favoritas
}
lastfm-import-stopped = { $count ->
    [one] Detenido tras { $count } pista favorita
   *[other] Detenido tras { $count } pistas favoritas
}
lastfm-import-matched = , { $count } con coincidencia
lastfm-import-added = { $count ->
    [one] , { $count } añadida a favoritos
   *[other] , { $count } añadidas a favoritos
}

## Tag tools
tags-editor-add-tag = Añadir
tags-editor-clear-all = borrar todo
tags-editor-form-view = Formulario
tags-editor-format-unsupported-all = Las etiquetas de este formato todavía no se pueden leer ni escribir.
tags-editor-format-unsupported-some = Algunos de estos archivos están en un formato cuyas etiquetas todavía no se pueden leer ni escribir.
tags-editor-guess-button = Deducir
tags-editor-guess-folded = { $status }, { $count } más sin mostrar
tags-editor-guess-help = { $placeholders }; / coincide con la carpeta de arriba, %skip% descarta
tags-editor-guess-match-count = { $hits ->
    [one] { $hits } de { $total } coincide
   *[other] { $hits } de { $total } coinciden
}
tags-editor-guess-no-match = sin coincidencias
tags-editor-guess-pattern-label = Patrón
tags-editor-loading = Cargando etiquetas...
tags-editor-look-up = Consultar
tags-editor-multiple-values = Varios valores
tags-editor-clear-on-save = Se vacía al guardar
tags-editor-additional-tags = Etiquetas adicionales ({ $count })
tags-editor-remove = quitar
tags-editor-reveal = Mostrar
tags-editor-save-errors = { $count ->
    [one] falló { $count } archivo; { $error }
   *[other] fallaron { $count } archivos; { $error }
}
tags-editor-saving-progress = Guardando { $done }/{ $total }...
tags-editor-sort-names = Nombres de ordenación
tags-editor-table-view = Tabla
tags-editor-tag-columns = Etiquetas adicionales
tags-editor-tag-field-conflict = el campo { $field } escribe esta etiqueta
tags-editor-tag-key-placeholder = Nombre de etiqueta
tags-editor-tag-value-placeholder = Valor
tags-editor-tags-section = Etiquetas
tags-editor-unknown-partial = { $count } de { $total }
tags-editor-unread-count = No se pudieron leer las etiquetas de { $failed } de { $total } archivos
tags-editor-will-clear = se borrará
tags-editor-will-remove = se quitará
tags-editor-window-title = rox - Editor de etiquetas
tags-guess-empty-segment = el patrón deja un nombre de carpeta o de archivo vacío
tags-guess-no-placeholders = sin marcadores
tags-guess-skip-renders-nothing = %skip% no tiene nada que representar
tags-guess-unclosed = % sin cerrar
tags-guess-unknown-placeholder = marcador desconocido %{ $name }%
tags-matcher-blocked-arm = Activa un campo para aplicar
tags-matcher-blocked-no-match = No hay coincidencia que aplicar
tags-matcher-blocked-pick = Elige una coincidencia
tags-matcher-blocked-writing = Escribiendo las etiquetas...
tags-matcher-match-count = { $count ->
    [one] 1 coincidencia
   *[other] { $count } coincidencias
}
tags-matcher-no-matches = No se encontraron coincidencias
tags-matcher-pick-match = Elige una coincidencia
tags-matcher-search-failed = Falló la búsqueda: { $error }
tags-matcher-searching = Buscando...
tags-matcher-tagging = Etiquetando { $track }
tags-matcher-window-title = rox - Buscar metadatos
tags-rename-blocked-cue = pista de cue, sin archivo propio
tags-rename-blocked-duplicate = dos pistas dan el mismo nombre
tags-rename-blocked-occupied = ya hay un archivo ahí
tags-rename-blocked-outside-roots = fuera de todas las raíces de la biblioteca
tags-rename-blocked-unresolved = todavía no está en el catálogo
tags-rename-move-error = { $name }: { $error }
tags-rename-move-errors = { $count ->
    [one] falló { $count } archivo; { $error }
   *[other] fallaron { $count } archivos; { $error }
}
tags-rename-moving = Moviendo { $done }/{ $total }...
tags-rename-nothing-to-move = No hay nada que mover
tags-rename-pattern-help = { $placeholders }; / crea una carpeta, la extensión sigue al archivo
tags-rename-pattern-section = Patrón
tags-rename-preview-section = Vista previa
tags-rename-unchanged = sin cambios
tags-rename-will-move = { $count ->
    [one] { $count } de { $total } se moverá
   *[other] { $count } de { $total } se moverán
}
tags-rename-window-title = rox - Renombrar archivos
tags-repair-affected-files = Archivos afectados
tags-repair-section = Reparación
tags-repair-check-to-repair = Marca un archivo para repararlo
tags-repair-count = { $count ->
    [one] 1 archivo
   *[other] { $count } archivos
}
tags-repair-count-so-far = { $count } hasta ahora
tags-repair-label-scope = alcance
tags-repair-no-affected = No se encontraron archivos afectados.
tags-repair-no-folder = No hay carpeta que escanear; añade una a la biblioteca o elige una.
tags-repair-pick-folder = Elige una carpeta...
tags-repair-progress = Reparando { $done }/{ $total }...
tags-repair-repair-button = { $count ->
    [0] Reparar
   *[other] Reparar ({ $count })
}
tags-repair-result = { $count ->
    [one] Reparado 1 archivo
   *[other] Reparados { $count } archivos
}
tags-repair-result-failed = { $count ->
    [one] Reparado { $count }, { $failed ->
        [one] { $failed } falló
       *[other] { $failed } fallaron
    }
   *[other] Reparados { $count }, { $failed ->
        [one] { $failed } falló
       *[other] { $failed } fallaron
    }
}
tags-repair-scan-first = Escanea primero
tags-repair-scan-hint = Escanea para encontrar archivos con daños en las etiquetas que una reescritura repara.
tags-repair-select-all = Seleccionar todo
tags-repair-select-none = No seleccionar nada
tags-repair-whole-library = Toda la biblioteca
tags-repair-window-title = rox - Reparar etiquetas

## Convert
convert-arg-names-file = "{ $token }" nombra un archivo; el destino sale de la carpeta y del patrón
convert-section-output = Salida
convert-section-preview = Vista previa
convert-arg-not-flag-or-value = "{ $token }" no es una opción ni el valor de una
convert-check-wrote-nothing = ffmpeg terminó sin errores pero no escribió nada
convert-custom-ext-empty = La extensión es lo que elige el contenedor, así que hace falta una
convert-custom-ext-invalid = "{ $ext }" no es un nombre de contenedor; letras y dígitos, sin punto
convert-dialog-browse = Examinar...
convert-dialog-check-passed = ffmpeg codificó un instante de silencio con esto, así que funciona
convert-dialog-check-waiting = Se comprueba con ffmpeg en cuanto dejes de escribir
convert-dialog-checking = Comprobando con ffmpeg...
convert-dialog-choose-folder = Elige una carpeta donde escribir
convert-dialog-convert-button = Convertir
convert-dialog-custom-label = Personalizado
convert-dialog-custom-menu-item = Personalizado...
convert-dialog-custom-note = Los argumentos se separan por espacios, así que no hay comillas; la carátula incrustada no se copia en los formatos personalizados
convert-dialog-format-not-ready = El formato escrito todavía no ha pasado por ffmpeg
convert-dialog-label-extension = extensión
convert-dialog-label-format = formato
convert-dialog-label-into = en
convert-dialog-label-named = llamado
convert-dialog-mirror = Reflejar las carpetas de la biblioteca
convert-dialog-nothing-to-convert = Nada que convertir: se omiten todas las filas
convert-dialog-pattern-help = { $placeholders }; / crea una carpeta, el formato pone la extensión
convert-dialog-pick-folder = Elige una carpeta donde escribir
convert-dialog-span-note = { $count ->
    [one] { $count } recortado de una imagen de cue y etiquetado desde la biblioteca
   *[other] { $count } recortados de una imagen de cue y etiquetados desde la biblioteca
}
convert-dialog-will-convert = { $count ->
    [one] { $count } de { $total } se convertirá
   *[other] { $count } de { $total } se convertirán
}
convert-dialog-window-title = rox - Convertir
convert-ffmpeg-silent-failure = ffmpeg falló sin decir por qué
convert-flag-attach = -attach lee un archivo aparte, y eso aquí no se permite
convert-flag-f = La extensión elige el contenedor, así que -f no te toca a ti
convert-flag-i = La entrada es la pista que elegiste, así que -i no te toca a ti
convert-flag-n = -n ya va en todas las ejecuciones
convert-flag-y = Aquí no se sobrescribe nada, así que -y no está disponible; un destino que ya existe se omite
convert-preset-flac = FLAC
convert-preset-mp3-320 = MP3 320 kbps
convert-preset-mp3-v0 = MP3 V0
convert-preset-opus-192 = Opus 192 kbps
convert-preset-wav = WAV
convert-skip-duplicate = dos pistas dan el mismo nombre
convert-skip-exists = ya está ahí
convert-summary-failed = { $count ->
    [one] , { $count } falló
   *[other] , { $count } fallaron
}
convert-summary-files = { $count ->
    [one] 1 archivo
   *[other] { $count } archivos
}
convert-summary-line = { $files } a { $dest }
convert-summary-skipped = { $count ->
    [one] , { $count } omitido
   *[other] , { $count } omitidos
}
convert-summary-stopped = Detenido tras { $files } a { $dest }
convert-version-answered = { $binary } se ejecutó, pero no dijo su versión

## Duplicates
duplicates-auto-select = Selección automática
duplicates-check-to-trash = Marca las copias para mandarlas a la papelera
duplicates-copy-count = { $count ->
    [one] 2 copias
   *[other] { $count } copias
}
duplicates-different-albums = álbumes distintos
duplicates-filter-placeholder = Filtrar por título, artista o carpeta
duplicates-groups-summary = { $groups ->
    [one] 1 grupo, { $extras ->
        [one] { $extras } copia de más
       *[other] { $extras } copias de más
    }
   *[other] { $groups } grupos, { $extras ->
        [one] { $extras } copia de más
       *[other] { $extras } copias de más
    }
}
duplicates-library-loading = La biblioteca todavía está cargando; inténtalo dentro de un momento.
duplicates-no-duplicates = No se encontraron duplicados.
duplicates-no-filter-matches = Ningún grupo coincide con el filtro.
duplicates-policy-newest = Quedarse con el más nuevo
duplicates-policy-oldest = Quedarse con el más antiguo
duplicates-policy-quality = Quedarse con la mejor calidad
duplicates-scan-hint = Escanea la biblioteca en busca de pistas que aparecen más de una vez.
duplicates-select-none = No seleccionar nada
duplicates-selected-count = { $count ->
    [one] { $count } seleccionado
   *[other] { $count } seleccionados
}
duplicates-trash-button = { $count ->
    [0] A la papelera
   *[other] A la papelera ({ $count })
}
duplicates-trash-error = { $name }: { $error }
duplicates-trash-result = { $count ->
    [one] 1 archivo movido a la papelera
   *[other] { $count } archivos movidos a la papelera
}
duplicates-trash-result-failed = { $count ->
    [one] { $count } movido a la papelera, { $failed ->
        [one] { $failed } falló
       *[other] { $failed } fallaron
    }
   *[other] { $count } movidos a la papelera, { $failed ->
        [one] { $failed } falló
       *[other] { $failed } fallaron
    }
}
duplicates-trashing = Moviendo a la papelera { $done }/{ $total }...
duplicates-window-title = rox - Duplicados

## Etiquetador de géneros

tag-genres-empty = Todas las pistas tienen género. Reproduce algo para reetiquetarlo.
tag-genres-heading = Etiquetar géneros
tag-genres-input-placeholder = Escribe un género
tag-genres-keys-hint = 1-8 eligen una fila, Mayús+1-8 la añaden al cuadro, Ctrl+1-8 la aplican a todo el álbum, Enter aplica lo escrito, L consulta Last.fm, S salta, Ctrl+Z deshace
tag-genres-library-loading = La biblioteca todavía está cargando; inténtalo dentro de un momento.
tag-genres-no-file = La biblioteca no tiene ningún archivo para esta pista.
tag-genres-no-suggestions = Nada que sugerir; escribe un género.
tag-genres-progress = { $at } de { $total } sin género
tag-genres-skip = Saltar
tag-genres-thinking = Leyendo el vecindario...
tag-genres-undo = Deshacer
tag-genres-unwritable = Esta pista vive dentro de una imagen cue compartida, así que su género no se puede escribir. Sáltala.
tag-genres-window-title = rox - Etiquetar géneros
tag-genres-looking-up = Consultando Last.fm...
tag-genres-lookup = Buscar en Last.fm
tag-genres-auto-lookup = Automáticamente
tag-genres-lookup-found = Last.fm etiqueta a { $artist } como: { $tags }
tag-genres-lookup-none = Last.fm no tiene etiquetas para { $artist }.
tag-genres-lookup-off = La búsqueda de artistas en línea está desactivada en Ajustes.
tag-genres-why-acoustic = { $count ->
    [one] { $count } pista que suena parecido
   *[other] { $count } pistas que suenan parecido
}
tag-genres-why-album = { $count ->
    [one] { $count } pista en este álbum
   *[other] { $count } pistas en este álbum
}
tag-genres-why-artist = { $count ->
    [one] { $count } pista de este artista
   *[other] { $count } pistas de este artista
}
tag-genres-why-lookup = Last.fm
tag-genres-album-too = { $count ->
    [one] Etiquetar todo el álbum de la pista y su { $count } vecina
   *[other] Etiquetar todo el álbum de la pista y sus { $count } vecinas
}
tag-genres-apply = Aplicar
tag-genres-begin = Iniciar cola
tag-genres-col-genre = Género
tag-genres-col-match = Coincidencia
tag-genres-col-why = Por qué
tag-genres-current-genre = Género: { $genre }
tag-genres-idle = No suena nada. Inicia la cola para recorrer las pistas sin género, o reproduce algo para reetiquetarlo.
tag-genres-no-genre = Aún sin género
tag-genres-stop = Detener cola
tag-genres-untagged-count = { $count ->
    [one] { $count } pista sin género
   *[other] { $count } pistas sin género
}
tag-genres-write-error = { $name }: { $error }
tag-genres-writing = Escribiendo { $done } de { $total }...

## Smart playlists
smart-playlist-descending = Descendente
smart-playlist-edit-title = Editar lista inteligente
smart-playlist-limit-label = Límite
smart-playlist-limit-placeholder = Sin límite
smart-playlist-match-count = { $count ->
    [one] 1 pista coincide
   *[other] { $count } pistas coinciden
}
smart-playlist-matched-tracks = Pistas coincidentes
smart-playlist-new-title = Nueva lista inteligente
smart-playlist-no-matches = Ninguna pista coincide
smart-playlist-query-label = Consulta
smart-playlist-sort-default = Orden predeterminado
smart-playlist-sort-added = Añadido
smart-playlist-sort-label = Orden
smart-playlist-unknown-field = "{ $field }:" no es un campo, así que el término se busca como texto normal
smart-playlist-window-title = rox - { $verb }

## Playlist creation
playlist-create-not-savable = Ponle nombre a la lista para guardarla
playlist-create-placeholder = Nombre de la lista
playlist-create-rename-title = Renombrar lista
playlist-create-title = Nueva lista
playlist-create-window-title = rox - { $verb }

## Cover tools
cover-art-back = Contraportada
cover-art-disc = Disco
cover-art-front = Portada
cover-artwork = Ilustración
    .description = Qué imagen mostrar; si el archivo no tiene la elegida, se usa la portada
cover-disc-style = Estilo de disco
    .description = Presenta la ilustración como un CD o como la etiqueta de un vinilo
cover-disc-off = Desactivado
cover-disc-cd = CD
cover-disc-vinyl = Vinilo
cover-editor-choose-image = Elegir imagen
cover-editor-multiple = Varias
cover-editor-none = Ninguna
cover-editor-not-an-image = Ese archivo no es una imagen que rox pueda incrustar
cover-editor-not-decoded = No se pudo decodificar esa imagen
cover-editor-reading = Leyendo la carátula actual...
cover-editor-remove = Quitar
cover-editor-replace = Reemplazar
cover-editor-revert = Revertir
cover-editor-save-errors = { $count ->
    [one] falló { $count } archivo; { $error }
   *[other] fallaron { $count } archivos; { $error }
}
cover-editor-saving-progress = Guardando { $done }/{ $total }...
cover-editor-search-online = Buscar en línea
cover-editor-section = Carátula
cover-editor-slot-back = Contraportada
cover-editor-slot-front = Portada
cover-editor-slot-media = Soporte
cover-editor-will-remove = Se quitará
cover-editor-window-title = rox - Carátula
cover-matcher-blocked-fetching = Descargando la imagen completa...
cover-matcher-blocked-no-cover = No hay carátula que poner
cover-matcher-blocked-pick = Elige una carátula para ponerla
cover-matcher-cover-count = { $count ->
    [one] 1 carátula
   *[other] { $count } carátulas
}
cover-matcher-editor-closed = El editor de carátulas se cerró
cover-matcher-no-covers = No se encontraron carátulas
cover-matcher-search-failed = Falló la búsqueda: { $error }
cover-matcher-set-cover = Poner la carátula
cover-matcher-setting = Poniendo...
cover-matcher-tile-info = { $provider }  { $width }px
cover-matcher-unsupported-format = Formato de imagen no admitido
cover-matcher-window-title = rox - Buscar carátulas
cover-spin = Giro
    .description = Gira el disco mientras suena una pista; se aplica a la imagen de disco o a un estilo de disco
cover-spin-disc = Girar el disco
cover-spin-ramp = Rampa de giro
    .description = Cuánto tarda el disco en llegar a plena velocidad, y en volver a pararse
cover-spin-speed = Velocidad de giro
    .description = Velocidad máxima, en revoluciones por minuto
cover-stretch = Estirar
    .description = Llena el panel, ignorando la proporción de la ilustración
cover-stretch-to-fill = Estirar hasta llenar
cover-title = Carátula

## Lyrics
lyrics-always-centered = Siempre centrado
    .description = Rellena los extremos para que la primera y la última línea también puedan centrarse
lyrics-auto-search = Búsqueda automática
    .description = Busca en línea en una pista sin letra y guarda una coincidencia segura, sin selector
lyrics-bold = Negrita
lyrics-build-word-by-word = Construir palabra a palabra
    .description = Revela las palabras según se cantan, al estilo karaoke; las líneas sin cantar se quedan ocultas
lyrics-edge-bottom = Abajo
lyrics-edge-top = Arriba
lyrics-edit-hint-after-stamp = para marcar
lyrics-edit-hint-or = o
lyrics-edit-loading = Cargando la letra...
lyrics-edit-lyrics = Editar la letra
lyrics-edit-saving = Guardando...
lyrics-edit-section = Letras
lyrics-edit-stamp = Marcar
lyrics-edit-stamp-time = Marcar { $time }
lyrics-edit-window-title = rox - Editar la letra
lyrics-fade-lines-in = Aparecer con fundido
    .description = Sube una línea desde tenue según pasa a ser la activa
lyrics-falloff-edge = Lado de la caída
    .description = En qué lado de la línea activa atenúa la caída
lyrics-find-online = Buscar letras en línea...
lyrics-follow-playback = Seguir la reproducción
    .description = Desliza la línea activa hasta el centro según suena una letra sincronizada
lyrics-font = Fuente
    .description = La tipografía de la letra; el valor predeterminado sigue la fuente de la aplicación
lyrics-gap-threshold = Umbral de hueco
    .description = Cuánto tiene que durar una intro o un hueco antes de recibir un descanso
lyrics-lead-in-rest = Descanso de entrada
    .description = Muestra un descanso en blanco antes de una intro larga, para que la primera línea aparezca con fundido cuando llegue
lyrics-line-falloff = Caída de línea
    .description = Cuánto se atenúa cada línea por paso de distancia respecto a la activa
lyrics-line-spacing = Espaciado de líneas
    .description = Cuánto se separan las líneas sincronizadas, como múltiplo del tamaño del texto
lyrics-look-again = Buscar de nuevo
lyrics-mark-dots = Puntos
lyrics-mark-note = Nota
lyrics-marked-notice = Marcada sin letra
lyrics-matcher-blocked-no-match = No hay coincidencia que aplicar
lyrics-matcher-blocked-pick = Elige una coincidencia para aplicarla
lyrics-matcher-blocked-saving = Guardando la letra...
lyrics-matcher-match-count = { $count ->
    [one] 1 coincidencia
   *[other] { $count } coincidencias
}
lyrics-matcher-no-query = Esta pista no tiene artista ni título con los que buscar
lyrics-matcher-pick-preview = Elige una coincidencia para verla
lyrics-matcher-search-failed = Falló la búsqueda: { $error }
lyrics-matcher-synced-tag = { $provider }  sincronizada
lyrics-matcher-window-title = rox - Buscar letras
lyrics-no-lyrics-notice = Sin letra
lyrics-no-lyrics-track = Esta pista no tiene letra
lyrics-rest-in-gaps = Descansar en los huecos
    .description = Pasa a un descanso en blanco durante un hueco instrumental largo en vez de mantener la última línea
lyrics-rest-marker = Marca de descanso
    .description = Qué muestra una línea sin palabras en una letra sincronizada, los huecos y las líneas en blanco
lyrics-search-button = Botón de búsqueda en línea
    .description = Muestra el botón de búsqueda en la cara vacía; el menú del clic derecho sigue encontrando letras
lyrics-search-online = Buscar en línea
lyrics-show-song-name = Mostrar el nombre de la canción
    .description = Muestra el nombre de la pista en la cara vacía, sobre la línea de sin letra
lyrics-text-size = Tamaño del texto
    .description = El texto de la letra; la altura de la línea sincronizada lo sigue
lyrics-title = Letras
lyrics-title-unsynced = Título en las no sincronizadas
    .description = Fija el título de la pista sobre una letra sin sincronizar, para que un panel bajo lo siga mostrando
lyrics-wipe-lyrics = Borrar la letra

## Analysis passes
pass-acoustic-body = { $model } averigua a qué suena cada una, para que la biblioteca pueda encontrar música parecida a lo que está sonando. Todo corre en esta máquina, y se omite lo que ya esté descrito. { $lands }
pass-acoustic-lands-database = Los resultados van a la base de datos de la biblioteca y tus archivos quedan intactos.
pass-acoustic-lands-tags = Los resultados van a la base de datos de la biblioteca y, en MP3 y FLAC, también a las etiquetas de cada archivo, así que se conservan si se reconstruye la base de datos. Los demás formatos se quedan solo con la copia de la base de datos.
pass-acoustic-title = { $count ->
    [one] ¿Analizar 1 pista?
   *[other] ¿Analizar { $count } pistas?
}
pass-analyze = Analizar
pass-estimate-at = { $estimate } con { $workers_phrase }.
pass-estimate-button = Estimar
pass-estimating = Estimando...
pass-measure = Medir
pass-no-estimate = Todavía no se ha ejecutado nada en esta máquina, así que no hay estimación. Estimar mide unas cuantas pistas y saca el resto de ahí.
pass-replaygain-body = Cada archivo se decodifica y se mide para que pueda sonar con la sonoridad a la que se masterizó. Los álbumes se miden enteros cuando a todas sus pistas les falta la ganancia. { $lands }
pass-replaygain-lands-database = Los números van a la base de datos de la biblioteca y tus archivos quedan intactos.
pass-replaygain-lands-tags = Los números se escriben en las etiquetas de cada archivo, donde los lee cualquier otro reproductor.
pass-replaygain-title = { $count ->
    [one] ¿Medir 1 pista?
   *[other] ¿Medir { $count } pistas?
}
pass-tempo-body = Se decodifican dos ventanas de medio minuto de cada archivo y se cuentan los tiempos, para que la biblioteca pueda mostrar a qué velocidad va una pista. Funciona mejor con música grabada a claqueta y omite lo que no puede medir. Los números van a la base de datos de la biblioteca y tus archivos quedan intactos.
pass-tempo-retry-body = Una pasada anterior ya escuchó estas pistas y no encontró pulso en ninguna. Reintentarlo vuelve a decodificarlas todas, así que solo compensa cuando el conteo de pulsos ha mejorado.
pass-tempo-retry-title = { $count ->
    [one] ¿Volver a escuchar 1 pista rechazada?
   *[other] ¿Volver a escuchar { $count } pistas rechazadas?
}
pass-tempo-title = { $count ->
    [one] ¿Averiguar el tempo de 1 pista?
   *[other] ¿Averiguar el tempo de { $count } pistas?
}
pass-timing = Contando unas cuantas pistas...
pass-timing-failed = No se pudo medir esta biblioteca: { $error }
pass-fill = Rellenar
pass-sortnames-body = Cada artista se busca en MusicBrainz para dar con la grafía latina bajo la que se ordena, de modo que 米津玄師 acabe en la Y. El servicio permite una petición por segundo, y eso marca el ritmo. Las respuestas van a la base de datos de la biblioteca; tus archivos no se tocan nunca.
pass-sortnames-scope-all = Buscar también los nombres que ya se ordenan en alfabeto latino
pass-romanize = Romanizar
pass-romanize-body = Cada título, álbum y artista que sigue sin nombre de ordenación se lee en letras latinas, de modo que レモン se encuentre escribiendo "lemon". El coreano y el chino no necesitan nada más. Los kanji japoneses necesitan el diccionario de Ajustes > Biblioteca, e IPADIC se equivoca con los nombres lo bastante a menudo como para que el editor de etiquetas esté ahí para corregirlo. Las respuestas van a la base de datos de la biblioteca; tus archivos no se tocan nunca.
pass-romanize-title = { $count ->
    [one] ¿Leer 1 nombre en letras latinas?
   *[other] ¿Leer { $count } nombres en letras latinas?
}
pass-romanize-skips-kanji = { $kanji } de { $total } valores son kanji y se omitirán hasta que se instale el diccionario japonés. Consíguelo en Ajustes > Biblioteca.
pass-sortnames-title = { $count ->
    [one] ¿Buscar 1 artista?
   *[other] ¿Buscar { $count } artistas?
}
pass-workers = Procesos

## Quick play
quick-play-comfortable-rows = Filas holgadas
    .description = Dale más altura a cada resultado
quick-play-cover = Carátula
    .description = Muestra una miniatura de carátula a la izquierda de cada resultado
quick-play-duration = Duración
    .description = Muestra a la derecha la duración de cada resultado
quick-play-search-placeholder = Buscar en la biblioteca
quick-play-subtitle = Subtítulo
    .description = Muestra el artista y el álbum bajo cada resultado
quick-play-syntax-absent = Filas sin ningún valor
    .example = -year
quick-play-syntax-exclude = Todo menos las coincidencias
    .example = -genre:rock
quick-play-syntax-field = Fija un campo; entrecomilla los valores con espacios
    .example = artist:"Daft Punk"
quick-play-syntax-free = Coincide con el título, el artista, el artista del álbum, el álbum o el género
    .example = daft punk
quick-play-syntax-numeric = Compara un número; plays:0 y added:<90d funcionan igual
    .example = rating:>=4
quick-play-syntax-title = Sintaxis de búsqueda
quick-play-syntax-year = Son dígitos, así que un prefijo toma toda la década
    .example = year:199
quick-play-tag-album = Álbum
quick-play-tag-artist = Artista

## Drawer panel
drawer-add-tooltip = Añadir un panel de cajón
drawer-answers = Responde a
    .description = Qué selecciones abren el cajón: solo las de su propio panel principal, o las de cualquier panel de fuera
drawer-dim = Atenuar
    .description = Cuánto se atenúa el panel principal detrás del cajón abierto
drawer-edge = Borde
    .description = El borde contra el que descansa el cajón y del que sale deslizándose
drawer-edge-bottom = Abajo
drawer-edge-top = Arriba
drawer-handle = Tirador
    .description = Muestra el agarre en el borde del panel. Oculto, no se ve nada del cajón hasta que hay una selección, y entonces el agarre se queda mientras dure, así que un cajón que se cerró se puede volver a sacar
drawer-open-on = Abrir con
    .description = Descansar sobre el tirador siempre abre el cajón; selección añade elegir algo en el panel principal
drawer-pin-open = Fijar abierto
drawer-reveal = Apertura
    .description = Cuánto del panel cubre el cajón abierto
drawer-scope-elsewhere = En otro sitio
drawer-scope-main = Panel principal
drawer-title = Cajón
drawer-trigger-hover = Cursor encima
drawer-trigger-selection = Selección

## Mini player
mini-tip-back = Volver a la disposición completa
mini-tip-none = No hay disposición mini asignada
mini-tip-shrink = Encoger al mini reproductor
mini-title = Alternar mini

## System tray
tray-open = Abrir
tray-pause = Pausa
tray-play = Reproducir
tray-quit = Salir

## Window controls
window-controls-mini-toggle = Alternar mini
    .description = Empieza por el botón de disposición mini; aparece en cuanto hay una disposición mini asignada
window-controls-minimize = Minimizar
window-controls-style = Estilo
    .description = Iconos planos, o los semáforos de macOS
window-controls-style-icons = Iconos
window-controls-title = Controles de ventana
window-controls-traffic-lights = Semáforos

## Section names the audio panels share. Each one heads the same kind of
## rows in the spectrum, VU, oscilloscope, spectrogram and particles
## settings, so they're defined once rather than per panel.
viz-section-analysis = Análisis
viz-section-color = Color
viz-section-peaks = Picos
viz-section-playback = Reproducción
viz-section-scale = Escala
viz-section-signal = Señal

## Particles panel
particles-add-emitter = Añadir emisor
particles-aim = Puntería
particles-aim-fixed = Fija
particles-aim-outward = Hacia fuera
particles-burst = Ráfaga
particles-color = Color
particles-cone = Cono
particles-direction = Dirección
    .description = Hacia dónde tira; 0 es arriba, 180 es abajo
particles-drag = Resistencia
    .description = Cuánta velocidad se come el aire cada segundo; cero es el vacío
particles-drift = Deriva
    .description = A qué velocidad se mueve el propio campo, para que los remolinos no se queden quietos
particles-edit-emitters = Editar los emisores
particles-emitter-label = Emisor { $index }
particles-emitter-target = Emisor { $index } { $target }
particles-emitters-empty = Todavía no hay emisores. Añade uno para arrancar el campo.
particles-glow = Resplandor
    .description = Pon un halo suave detrás de cada partícula
particles-gravity = Gravedad
particles-gravity-strength = Fuerza
    .description = Tirón constante sobre todo lo que está en vuelo
particles-height = Altura
particles-hold-on-pause = Mantener en pausa
    .description = Congela el campo mientras está en pausa en vez de dejar que se disperse
particles-length = Longitud
particles-lifetime = Vida
particles-position-x = Posición X
particles-position-y = Posición Y
particles-radius = Radio
particles-rate = Tasa
particles-rotation = Rotación
particles-round-particles = Partículas redondas
    .description = Dibuja puntos en vez de cuadrados
particles-scale = Escala
    .description = Cuánto abarca un remolino; pequeño revuelve, grande rueda
particles-section-emitters = Emisores
particles-section-medium = Medio
particles-section-particles = Partículas
particles-shape = Forma
particles-shape-box = Caja
particles-shape-line = Línea
particles-shape-point = Punto
particles-shape-ring = Anillo
particles-size = Tamaño
particles-speed = Velocidad
particles-trigger = Disparo
particles-trigger-continuous = Continuo
particles-turbulence = Turbulencia
particles-turbulence-drift = Deriva de la turbulencia
particles-turbulence-scale = Escala de la turbulencia
particles-turbulence-strength = Fuerza
    .description = Con cuánta fuerza empuja el campo a las partículas; cero es apagado
particles-width = Ancho

## Spectrum panel
spectrum-axis-labels = Etiquetas de eje
    .description = Marca el rango a lo ancho del panel: octavas (C1, C2, ...) o frecuencias (100, 1k, 10k)
spectrum-bar-gap = Separación entre barras
    .description = Espacio entre barras; cuanto mayor sea, menos barras caben
spectrum-bar-width = Ancho de barra
    .description = Cuán gruesa se dibuja cada barra; las barras finas dejan sitio a más bandas
spectrum-block-gap = Separación entre bloques
    .description = La junta entre celdas de una columna
spectrum-block-height = Altura de bloque
    .description = Cuán alta se dibuja cada celda de una columna
spectrum-cap-gravity = Gravedad de los picos
    .description = Con cuánta fuerza caen las marcas de pico una vez que la banda baja
spectrum-fft-size = Tamaño de FFT
    .description = Ventana de análisis; corta reacciona rápido, larga resuelve más fino
spectrum-gradient-base-color = Color base
    .description = El extremo suave de la rampa personalizada
spectrum-gradient-cover = Carátula
spectrum-gradient-mode = Degradado
    .description = Colorea las bandas por sonoridad: la rampa del tema, los colores de la carátula con el color de la canción activado, o un par personalizado
spectrum-gradient-theme = Tema
spectrum-gradient-tip-color = Color de la punta
    .description = El extremo fuerte de la rampa personalizada
spectrum-high-bound-description = La frecuencia más alta que analizan las barras
spectrum-high-fft-size = Tamaño de FFT alto
    .description = Ventana de análisis para las bandas por encima del corte
spectrum-hold-on-pause = Mantener en pausa
    .description = Congela las barras mientras está en pausa en vez de dejarlas caer al silencio
spectrum-labels-frequency = Frecuencia
spectrum-labels-pitch = Tono
spectrum-low-bound-description = La frecuencia más baja que analizan las barras
spectrum-orientation = Orientación
    .description = El borde desde el que crecen las bandas
spectrum-outline-bars = Barras de contorno
    .description = Dibuja cada barra como un contorno hueco en vez de una rampa rellena
spectrum-outline-width = Grosor del contorno
    .description = Grosor del trazo de las barras huecas
spectrum-peak-caps = Marcas de pico
    .description = Mantén una marca en el pico reciente de cada banda
spectrum-section-bands = Bandas
spectrum-split-at = Cortar en
    .description = Dónde se juntan las zonas, ajustado a la barra más cercana
spectrum-split-zones = Zonas partidas
    .description = Analiza por debajo y por encima de una frecuencia de corte con ventanas de distinto tamaño
spectrum-style = Estilo
    .description = Barras clásicas, bloques estilo LED, o una línea continua
spectrum-style-bars = Barras
spectrum-style-blocks = Bloques
spectrum-style-line = Línea
spectrum-symmetry = Simetría
    .description = Pliega el espectro sobre el centro; hacia delante pone los graves en los bordes, al revés los junta en el medio
spectrum-symmetry-forward = Hacia delante
spectrum-symmetry-reverse = Al revés

## Waveform panel
waveform-bar-gap = Separación entre barras
    .description = Espacio entre barras; cero las funde en una sola forma
waveform-bar-width = Ancho de barra
    .description = Cuán gruesa se dibuja cada barra
waveform-outline = Contorno
    .description = Traza las barras en vez de rellenarlas; las barras fundidas se leen como una sola forma
waveform-scrobble-marker = Marca de scrobble
    .description = Una línea fina donde la pista cuenta como scrobbleada a Last.fm
waveform-split-channels = Separar canales
    .description = Una fila por canal, izquierdo sobre derecho; las pistas mono se quedan en una sola fila
waveform-unavailable = No hay forma de onda para esta pista

## VU panel
vu-ballistics = Balística
    .description = VU integra la sonoridad despacio; Pico salta arriba y baja suave
vu-ballistics-peak = Pico
vu-cap-gravity = Gravedad de los picos
    .description = Con cuánta fuerza caen las marcas de pico una vez que el medidor baja
vu-channels = Canales
    .description = Separa el par estéreo, o pliégalo en un solo medidor
vu-channels-mono = Mono
vu-channels-stereo = Estéreo
vu-db-scale = Escala en dB
    .description = Dibuja líneas de rejilla etiquetadas en las marcas de dB detrás de los medidores
vu-gradient-mode = Degradado
    .description = Colorea los medidores por nivel: la rampa del tema, los colores de la carátula con el color de la canción activado, o un par personalizado
vu-hold-on-pause = Mantener en pausa
    .description = Congela los medidores mientras está en pausa en vez de dejarlos caer al silencio
vu-orientation = Orientación
    .description = El borde desde el que crecen los medidores
vu-peak-caps = Marcas de pico
    .description = Mantén una marca en el pico reciente de cada medidor
vu-section-meter = Medidor
vu-segment-gap = Separación entre segmentos
    .description = La junta entre celdas de una columna
vu-segment-height = Altura de segmento
    .description = Cuán alta se dibuja cada celda de una columna
vu-style = Estilo
    .description = Una columna continua, o segmentos estilo LED
vu-style-continuous = Continuo
vu-style-segments = Segmentos

## Spectrogram panel
spectrogram-ceiling = Techo
    .description = Nivel que corresponde al extremo claro del mapa de colores, así que todo lo más fuerte se queda ahí
spectrogram-colormap = Mapa de colores
    .description = Cómo se traduce el volumen a color
spectrogram-colormap-cover = Carátula
spectrogram-colormap-grayscale = Escala de grises
spectrogram-colormap-ice = Hielo
spectrogram-colormap-magma = Magma
spectrogram-colormap-theme = Tema
spectrogram-colormap-viridis = Viridis
spectrogram-direction = Dirección
    .description = El borde por el que entran las columnas nuevas, que también decide si el eje de frecuencias sube por el panel o lo cruza
spectrogram-fft-size = Tamaño de FFT
    .description = Tamaño de la ventana de análisis, un equilibrio entre lo rápido que una columna sigue un transitorio y lo bien que separa dos notas graves
spectrogram-floor = Suelo
    .description = Nivel que corresponde al extremo oscuro del mapa de colores, así que todo lo más suave se lee como fondo
spectrogram-grid = Cuadrícula
    .description = Líneas de frecuencia sobre la imagen
spectrogram-high-bound = Límite superior
    .description = Parte alta del eje de frecuencias, limitada por debajo de Nyquist para descartar las octavas más altas, casi silenciosas
spectrogram-history = Historial
    .description = Cuántas columnas conserva el panel antes de que la más antigua salga por el borde
spectrogram-hold-on-pause = Mantener en pausa
    .description = Mantener la imagen fija en pausa en vez de dejar que le entre silencio deslizándose
spectrogram-labels = Etiquetas
    .description = Los números de frecuencia a lo largo de la regla, donde el panel tiene sitio para ellos
spectrogram-log-scale = Escala logarítmica
    .description = Dar a cada octava el mismo espacio, la lectura musical, en vez del espaciado uniforme en Hz de un instrumento de laboratorio
spectrogram-low-bound = Límite inferior
    .description = Parte baja del eje de frecuencias
spectrogram-section-picture = Imagen
spectrogram-speed = Velocidad
    .description = Con qué rapidez se desplaza la imagen, en columnas por segundo

## Oscilloscope panel

oscilloscope-channels = Canales
    .description = Pliega en una sola traza, superpón ambas, o apila un recuadro para cada una
oscilloscope-channels-mono = Mono
oscilloscope-channels-overlay = Superposición
oscilloscope-channels-split = Separado
oscilloscope-fill = Relleno
    .description = Un relleno suave entre la traza y la línea central
oscilloscope-gain = Ganancia
    .description = Escala vertical, para llevar una pista silenciosa a una traza legible
oscilloscope-gradient-mode = Degradado
    .description = Colorea la traza por excursión: la rampa del tema, los colores de la carátula con el color de la canción activado, o un par personalizado
oscilloscope-grid = Cuadrícula
    .description = Dibuja la retícula detrás de la traza
oscilloscope-hold-on-pause = Mantener en pausa
    .description = Mantén el fotograma detenido en pausa en vez de dejar que la traza se aplane
oscilloscope-line-width = Grosor de la línea
    .description = Con qué grosor se dibuja la traza
oscilloscope-persistence = Persistencia
    .description = Cuánto tiempo quedan visibles los fotogramas anteriores detrás de la traza, el efecto de persistencia fosforescente
oscilloscope-section-trace = Trazo
oscilloscope-trigger = Disparo
    .description = Inicia cada fotograma donde la señal cruza el nivel de disparo, para que el material periódico se quede quieto
oscilloscope-trigger-falling = Descendente
oscilloscope-trigger-level = Nivel de disparo
    .description = El nivel en el que se busca el cruce
oscilloscope-trigger-off = Desactivado
oscilloscope-trigger-rising = Ascendente
oscilloscope-window = Ventana
    .description = Cuánto tiempo abarca la traza a lo largo del panel

## Shader panel
shader-panel-compile-error = Este shader no compiló:
shader-panel-compile-title = Este shader no compiló
shader-panel-enable = Activar
shader-panel-inspect = Inspeccionar
shader-panel-note-empty-body = Elige un ejemplo, o apunta el panel a un archivo .wgsl que defina fs_user(uv).
shader-panel-note-empty-title = No hay ningún shader cargado.
shader-panel-note-missing-body = Este panel hace referencia a un shader que el espacio de trabajo no tiene, así que no hay nada que ejecutar.
shader-panel-note-missing-title = { $name } no está en los shaders de este espacio de trabajo.
shader-panel-note-off-body = El código y sus enlaces siguen aquí, solo que no se están ejecutando.
shader-panel-note-off-title = Este shader está desactivado.
shader-panel-note-pending-body = Llegó con una disposición o un espacio de trabajo en vez de salir de esta máquina, así que sigue desactivado hasta que lo revises.
shader-panel-note-pending-title = Este shader todavía no se ha revisado.
shader-pending-origin-file = Dice venir de { $path }
shader-pending-origin-inline = No hay ningún archivo detrás; el código vino con la disposición
shader-pending-more-lines = { $count ->
    [one] ... { $count } línea más
   *[other] ... { $count } líneas más
}
shader-eject-name-taken = { $count ->
    [one] { $name } ya tiene { $count } copia numerada en los shaders de este espacio de trabajo
   *[other] { $name } ya tiene { $count } copias numeradas en los shaders de este espacio de trabajo
}
shader-eject-not-in-pool = { $name } no está en los shaders de este espacio de trabajo
shader-eject-failed = al volcar: { $error }
shader-panel-pick = Elegir un shader
shader-panel-run-shader = Ejecutar el shader
    .description = Desactivado mantiene el código, el marcador y los enlaces en su sitio y no pinta nada
shader-panel-section-routes = Rutas

## Editor de shaders

shader-edit-here = Editar
shader-editor-window-title = rox - Editor de shaders
shader-editor-target-screen = Shader de pantalla
shader-editor-target-backdrop = Shader de fondo
shader-editor-origin-pool = Un shader del espacio de trabajo: aplicar llega a cada superficie que lo usa
shader-editor-origin-pool-file = Un shader del espacio de trabajo, con su copia de trabajo en { $path }
shader-editor-origin-file = Vinculado a { $path }, que también se escribe al aplicar
shader-editor-origin-inline = El código propio de esta superficie
shader-editor-apply = Aplicar
shader-editor-revert = Revertir
shader-editor-close = Cerrar
shader-editor-hint-press = Pulsa
shader-editor-hint-apply = para aplicar
shader-editor-status-unchecked = Nada que comprobar todavía
shader-editor-status-ok = Compila
shader-editor-status-error = Este shader no compiló:
shader-editor-section-uniforms = Uniforms
shader-editor-section-textures = Texturas
shader-editor-section-slots = Slots
shader-editor-section-signals = Señales
shader-editor-slot-unnamed = Slot { $n }
shader-editor-signals-empty = El pool aún no tiene señales. Añade algunas en la ventana Señales y aparecerán aquí con medidores en vivo.
shader-editor-uniform-time = Segundos que lleva el shader, congelado mientras el feed lo esté
shader-editor-uniform-delta = Segundos desde su último fotograma, 0 en el primero
shader-editor-uniform-resolution = La superficie en píxeles de dispositivo
shader-editor-uniform-mouse = xy el cursor en píxeles de dispositivo, z y w los botones
shader-editor-uniform-meta-0 = x volumen, y posición de la pista, z reproduciendo, w duración de la pista en segundos
shader-editor-uniform-meta-1 = x brillo de la página, y tema claro, z presencia del cursor, w forma del contenido
shader-editor-texture-screen = Lo que hay bajo la superficie en este fotograma
shader-editor-texture-prev = El último fotograma de esta superficie, para estelas

## Genre grid panel
genre-grid-clear-picked = Borrar los géneros elegidos
genre-grid-desaturate = Desaturar durante la reproducción
    .description = Pasa a escala de grises todos los mosaicos salvo el del género que suena; al pasar el cursor vuelve el color de un mosaico
genre-grid-dim-while-playing = Atenuar durante la reproducción
    .description = Apaga todos los mosaicos salvo el del género que suena; al pasar el cursor un mosaico vuelve a encenderse
genre-grid-follow-description = Desplázate al género que suena cada vez que cambia la pista
genre-grid-merge-many = Fusionar { $count } géneros en "{ $target }"
genre-grid-merge-one = Fusionar "{ $source }" en "{ $target }"
genre-grid-pick-filters = Elegir filtra la biblioteca
    .description = Hacer clic en un género acota a él todos los paneles que siguen la búsqueda compartida; desactivado deja el clic como una selección normal
genre-grid-play-genres = Reproducir { $count } géneros
genre-grid-resume-description = Vuelve al género que suena cuando dejas de navegar
genre-grid-show-names = Mostrar nombres
    .description = Escribe el género bajo cada mosaico en vez de solo al pasar el cursor
genre-grid-smooth-description = Deslízate hasta el género en vez de saltar
genre-grid-tally = { $albums ->
    [one] { $albums } álbum, { $tracks } pista(s)
   *[other] { $albums } álbumes, { $tracks } pista(s)
}
genre-grid-tile-face = Cara del mosaico
    .description = Qué muestra un mosaico: las carátulas de los álbumes del género, las carátulas bañadas en el color propio del género, o una tarjeta de color plano con el nombre encima
genre-grid-unmerge = { $count ->
    [one] Deshacer la fusión de { $count } valor
   *[other] Deshacer la fusión de { $count } valores
}

## Artist grid panel
artist-grid-clear-picked = Borrar los artistas elegidos
artist-grid-desaturate = Desaturar durante la reproducción
    .description = Pasa a escala de grises todos los mosaicos salvo el del artista que suena; al pasar el cursor vuelve el color de un mosaico
artist-grid-dim-while-playing = Atenuar durante la reproducción
    .description = Apaga todos los mosaicos salvo el del artista que suena; al pasar el cursor un mosaico vuelve a encenderse
artist-grid-follow-description = Desplázate al artista que suena cada vez que cambia la pista
artist-grid-group-mode = Un mosaico por
    .description = El artista del álbum acreditado deja los invitados de un disco en el acto que lo publicó; el artista de la pista pone cada colaboración en un mosaico propio
artist-grid-pick-filters = Elegir filtra la biblioteca
    .description = Hacer clic en un artista acota a él todos los paneles que siguen la búsqueda compartida; desactivado deja el clic como una selección normal
artist-grid-play-artists = Reproducir { $count } artistas
artist-grid-portraits = Retratos de artista
    .description = Muestra la foto de cada artista, buscada una vez por nombre y guardada en disco; desactivado muestra la carátula del primer álbum
artist-grid-resume-description = Vuelve al artista que suena cuando dejas de navegar
artist-grid-section-grouping = Agrupación
artist-grid-show-names = Mostrar nombres
    .description = Escribe el artista bajo cada mosaico en vez de solo al pasar el cursor
artist-grid-smooth-description = Deslízate hasta el artista en vez de saltar
artist-grid-tally = { $albums ->
    [one] { $albums } álbum, { $tracks } pista(s)
   *[other] { $albums } álbumes, { $tracks } pista(s)
}
artist-grid-track-artist = Artista de la pista

## Wall panels
wall-dim-always = Siempre
    .description = Mantén los mosaicos atrás incluso cuando no suena nada; solo el mosaico bajo el cursor se ve entero
wall-dim-amount = Intensidad
    .description = Cuánto se apagan los demás mosaicos; al 100% desaparecen
wall-gap = Separación
    .description = Espacio entre los mosaicos
wall-name-alignment = Alineación de nombres
    .description = Alinea los pies de foto bajo sus mosaicos
wall-rounding = Redondeo
    .description = Redondea las esquinas de cada mosaico; al 100% es un círculo
wall-section-picking = Selección
wall-show-counts = Mostrar recuentos
    .description = El recuento de álbumes y pistas bajo cada nombre
wall-tile-size = Tamaño del mosaico
    .description = El lado más largo de los mosaicos; las columnas reparten el ancho del panel por igual

## Metadata panel
metadata-copy-field = Copiar { $field }
metadata-cover-background = Carátula de fondo
    .description = La carátula de la pista detrás de los campos
metadata-display = Presentación
    .description = La ficha encabezada por el título, o una tabla plana de etiqueta y valor desde arriba
metadata-display-sheet = Ficha
metadata-display-table = Tabla
metadata-edit-save = Guardar
metadata-field-album-artist-sort = Orden de artista del álbum
metadata-field-album-sort = Orden de álbum
metadata-field-artist-sort = Orden de artista
metadata-field-bit-depth = Profundidad de bits
metadata-field-bitrate = Tasa de bits
metadata-field-bpm-measured = { $bpm } (medido por rox)
metadata-field-codec = Códec
metadata-field-comment = Comentario
metadata-field-disc = Disco
metadata-field-file = Archivo
metadata-field-gain-album = Ganancia del álbum
metadata-field-gain-track = Ganancia de la pista
metadata-field-sample-rate = Frecuencia de muestreo
metadata-field-title-sort = Orden de título
metadata-field-track = Pista
metadata-fields = Campos
    .description = Qué campos lista la ficha; un campo que la pista no tenga se queda oculto
metadata-find-online = Buscar metadatos en línea...
metadata-no-library = Sin biblioteca
metadata-romanize = Romanizar
metadata-romanize-needs-dictionary = Un nombre en kanji necesita el diccionario japonés. Consíguelo en Ajustes > Biblioteca.
metadata-romanize-sort-names = Romanizar los nombres de ordenación
metadata-row-borders-description = La línea fina bajo cada fila de la tabla
metadata-source = Origen
    .description = Sigue lo que suena o lo seleccionado, o lee la biblioteca en conjunto
metadata-stripes-description = Tiñe una fila de la tabla sí y otra no

## History panel
history-column-last-played = Última reproducción
history-descending = Descendente
    .description = Invierte el orden
history-empty-never = Todas las pistas se han reproducido
history-empty-recent = Todavía no hay escuchas
history-headings = Parte la lista reciente en tandas por álbum; Ampliado añade la carátula y las estadísticas
history-sort-browse = Orden de navegación
history-sort-date-added = Fecha de incorporación
history-sort-menu = Orden
    .description = Cómo se ordenan las pistas nunca reproducidas
history-title = Historial
history-view-most = Más reproducidas
history-view-never = Nunca reproducidas
history-view-recent = Reproducidas hace poco
history-view-recent-short = Recientes
history-view-row = Vista
    .description = Qué corte del registro de escuchas muestra el panel

## Folder tree panel
folder-tree-clear-scope = Borrar el alcance de carpeta
folder-tree-collapse-all = Contraer todo
folder-tree-collapse-branch = Contraer rama
folder-tree-cover-art = Carátula
    .description = Muestra la carátula en lugar del icono de la fila, en carpetas o en canciones
folder-tree-cover-folders = Carpetas
folder-tree-cover-songs = Canciones
folder-tree-empty = Todavía no hay carpetas en la biblioteca
folder-tree-expand-branch = Expandir rama
folder-tree-follow-description = Revela la pista que suena y desplázate a ella cada vez que cambia
folder-tree-nonmatch-folders = Carpetas sin coincidencias
    .description = Oculta las carpetas sin coincidencias, o déjalas atenuadas
folder-tree-nonmatch-songs = Canciones sin coincidencias
    .description = Dentro de una carpeta que coincide, atenúa las canciones sueltas u ocúltalas
folder-tree-play-folder = Reproducir la carpeta
folder-tree-play-songs = { $count ->
    [one] Reproducir
   *[other] Reproducir { $count } canciones
}
folder-tree-resume-description = Vuelve a la pista que suena cuando dejas de navegar
folder-tree-scope-to-folder = Acotar el filtro a la carpeta
folder-tree-smooth-description = Deslízate hasta la pista en vez de saltar
folder-tree-title = Árbol

## Art panel
art-always = Mantén las carátulas atrás incluso cuando no suena nada; solo la carátula bajo el cursor se ve entera
art-convert = Convertir...
art-covers-section = Carátulas
matcher-section-matches = Coincidencias
art-desaturate = Pasa a escala de grises todas las carátulas salvo la del álbum que suena; al pasar el cursor vuelve el color de una carátula
art-dim-while-playing = Apaga todas las carátulas salvo la del álbum que suena; al pasar el cursor una carátula vuelve a encenderse
art-disc-style = Estilo de disco
    .description = Presenta cada carátula como un CD o como la etiqueta de un vinilo
art-edit-tags = Editar etiquetas...
art-fill-panel = Llenar el panel
    .description = Dimensiona la carátula centrada solo por la altura del panel (por el ancho cuando está vertical); las carátulas laterales se salen del borde en vez de encogerla
art-follow-description = Centra el álbum que suena cada vez que cambia la pista
art-glow = Resplandor
    .description = Concentra el color de acento detrás de la carátula centrada; con el tinte de carátula activado toma el color del álbum que suena
art-label-position = Posición de la etiqueta
    .description = Dónde va el rótulo del álbum: arriba, bajo la carátula, en el borde inferior u oculto
art-letter-rail = Barra de letras
    .description = Las iniciales de los artistas en el borde del estante; un clic salta al primer álbum de esa letra
art-layout-section = Disposición
art-perspective = Perspectiva
    .description = Gira las carátulas laterales con el ángulo de abajo; apagada quedan planas y cuadradas, el único modo donde se aplica el redondeo de carátulas
art-recede = Luz de fondo
    .description = Cuánta luz recibe la carátula más al fondo; las que hay entre ella y el centro se reparten la distancia por igual
art-spacing = Separación de carátulas
    .description = A qué distancia de la carátula central queda la primera de cada lado; pasada la mitad la despeja y le deja sitio
art-stride = Separación de la pila
    .description = Cuánto se separan entre sí las carátulas detrás de la primera; también fija cuánto recorre un arrastre por carátula
art-visible = Carátulas visibles
    .description = Cuántas carátulas hay a cada lado de la central; la última se desvanece al salir
art-tilt = Ángulo de giro
    .description = Cuánto giran las carátulas laterales alejándose de ti
art-reflections = Reflejos
    .description = Refleja cada carátula en el suelo bajo el estante
art-resume-description = Vuelve a centrar el álbum que suena cuando dejas de navegar
art-shadows = Sombras
    .description = Una sombra suave bajo cada carátula
art-smooth-description = Deslízate hasta el álbum en vez de saltar
art-title = Carrusel de álbumes
art-vertical-layout = Disposición vertical
    .description = Apila el estante como una columna que se desplaza arriba y abajo en vez de una fila

## Playlists panel
playlists-art-description = The expanded headings' cover tile
playlists-line-height-description = One heading line; it draws inside the rows the block already has, so shrinking a line opens space instead of growing the block
playlists-meta-line = Meta Line
playlists-meta-line-description = What the second row under it shows, the same way
playlists-name-line = Name Line
playlists-name-line-description = What the heading's first row shows, left to right; a spacer or divider splits the sides
playlists-columns = Qué columnas de pista se ven junto al título
playlists-delete = Eliminar la lista
playlists-edit-query = Editar la consulta...
playlists-empty = Todavía no hay listas; añade pistas o usa Nueva lista
playlists-headings = Parte las pistas de cada lista en tandas por álbum; Ampliado añade la carátula y las estadísticas
playlists-import-tooltip = Importar una lista
playlists-imported-fallback = Importada
playlists-new = Nueva lista...
playlists-new-smart = Nueva lista inteligente...
playlists-refuse-drag-out = Las pistas de una lista inteligente no se pueden arrastrar fuera
playlists-refuse-edit-query = Edita la consulta para cambiar lo que tiene una lista inteligente
playlists-refuse-smart-source = Una lista inteligente saca sus pistas de su consulta
playlists-remove = { $count ->
    [one] Quitar de la lista
   *[other] Quitar { $count } de la lista
}
playlists-rename = Renombrar...
playlists-title = Listas de reproducción

## Queue panel
queue-clear = Vaciar la cola
queue-empty = La cola está vacía
queue-headings = Parte la cola en tandas por álbum; Ampliado añade la carátula y las estadísticas
queue-play-now = Reproducir ahora
queue-remove = { $count ->
    [one] Quitar de la cola
   *[other] Quitar { $count } de la cola
}
queue-title = Cola
queue-widget-always-modal = Abrir siempre como ventana modal
    .description = Abre la cola en una ventana modal siempre, en vez de saltar a un panel de cola que ya esté abierto
queue-widget-clear-queue = Vaciar la cola
queue-widget-more = +{ $count } más
queue-widget-open-on-click = Abrir la cola al hacer clic
    .description = Haz clic en el widget para saltar a un panel de cola abierto, o abrir la cola en una ventana cuando no haya ninguno
queue-widget-section-click = Clic
queue-widget-title = Widget de cola
queue-widget-up-next = A continuación

## Biography panel
biography-background = Fondo
    .description = El fanart del artista detrás del texto, atenuado y desvaneciéndose hacia abajo
biography-fill-width = Llenar el ancho
    .description = Deja que una cabecera alta ocupe todo el ancho en vez de quedarse limitada y centrada
biography-from-lastfm = De Last.fm
biography-header-image = Imagen de cabecera
    .description = El banner ancho del artista de arriba, o el retrato cuando no hay banner
biography-keep-aspect = Mantener la proporción
    .description = Muestra la cabecera con sus proporciones en vez de recortarla para llenar una franja
biography-listeners-count = { $count ->
    [one] { $count } oyente
   *[other] { $count } oyentes
}
biography-looking-up = Buscando { $name }
biography-no-artist-tag = Sin etiqueta de artista
biography-no-text = No hay biografía guardada
biography-not-found = No se encontró nada de { $name }
biography-plays-count = { $count ->
    [one] { $count } reproducción
   *[other] { $count } reproducciones
}
biography-refresh = Actualizar
biography-similar-artists = Artistas parecidos
    .description = Artistas relacionados según los datos de escucha, al pie
biography-similar-heading = Artistas parecidos
biography-stats = Estadísticas
    .description = Oyentes y reproducciones en Last.fm, bajo el nombre
biography-tags = Etiquetas
    .description = Las etiquetas de género como una fila de fichas
biography-title = Biografía

## Status panel
status-count-albums = { $count ->
    [one] 1 álbum
   *[other] { $count } álbumes
}
status-count-artists = { $count ->
    [one] 1 artista
   *[other] { $count } artistas
}
status-count-plays = { $count ->
    [one] 1 reproducción
   *[other] { $count } reproducciones
}
status-count-selected = { $count ->
    [one] { $count } seleccionado
   *[other] { $count } seleccionados
}
status-count-tracks = { $count ->
    [one] 1 pista
   *[other] { $count } pistas
}
status-readouts = Lecturas
    .description = Arrastra a lo largo de la barra para reordenar; arrastra entre las filas, o usa la x y el más de una ficha, para ocultar y mostrar
status-scope-selection = Selección
status-title = Estado

## Output panel
output-detail-badge = Distintivo
output-detail-compact = Compacto
output-detail-expanded = Ampliado
output-detail-label = Detalle
    .description = Distintivo lo deja en una ficha con el resto al pasar el cursor; compacto le da al titular una línea propia, para una tira a lo largo de un borde; ampliado añade los motivos al lado, o debajo cuando el panel es demasiado estrecho
output-device-name = Nombre del dispositivo
    .description = Nombra el dispositivo activo en el titular; desactivado deja la línea con el modo, la frecuencia y el formato
output-file-rate = Frecuencia del archivo
    .description = Confirma la frecuencia propia del archivo en reproducción cuando nada la está convirtiendo. Una conversión se señala de todas formas, porque de eso trata el aviso
output-mode-exclusive = Exclusivo
output-mode-shared = Compartido
output-no-output = Sin salida
output-nothing-playing = No suena nada
output-pick-another-device = Elige otro dispositivo, o desactiva el modo exclusivo
output-headline-numbers = { $rate } Hz, { $channels } can., { $format }
output-headline = { $mode }, { output-headline-numbers }
output-headline-device = { $mode } en { $device }, { output-headline-numbers }
output-fell-back-to-shared = Exclusivo pasó a compartido: { $why }
output-replaygain-levelling = ReplayGain está nivelando este archivo en { $db } dB
output-replaygain-short = ReplayGain { $db } dB
output-rate-resampled = El archivo en reproducción es de { $rate } Hz, remuestreado para alcanzar el dispositivo
output-rate-resampled-short = Archivo de { $rate } Hz remuestreado
output-rate-native = El archivo en reproducción es de { $rate } Hz, así que nada lo remuestrea
output-rate-native-short = Archivo de { $rate } Hz, sin remuestreo
output-start-track-hint = Pon una pista para ver el formato que aceptó el dispositivo
output-title = Salida

## Track columns
columns-album-artist-sort = Orden de artista del álbum
columns-album-sort = Orden de álbum
columns-artist-sort = Orden de artista
columns-bits = Bits
columns-bpm = BPM
columns-codec = Códec
columns-cover = Carátula
columns-fav = Fav
columns-gain = Ganancia
columns-kbps = Kbps
columns-khz = kHz
columns-name = Nombre
columns-number = Número
columns-scanned = Escaneada
columns-similar = Parecido
columns-title-sort = Orden de título

## Filter panel
filter-add-column = Añadir columna
filter-add-column-tooltip = Añadir columna
filter-all = Todo
filter-clear-filters = Borrar los filtros
filter-clear-selection = Borrar la selección
filter-empty = Elige un campo para empezar a filtrar
filter-over-cap = { $count } más, busca para acotar
filter-remove-column = Quitar columna

## Search panel
search-chips-below = Debajo
search-chips-inline = En línea
search-filter-chips = Fichas de filtro
search-placeholder = Buscar en la biblioteca

## Playback panel
playback-buttons = Botones
    .description = Arrastra a lo largo de la barra para reordenar; arrastra entre las filas, o usa la x y el más de una ficha, para ocultar y mostrar
playback-continue-down-list = Seguir reproduciendo, lista abajo
playback-continue-off = Seguir reproduciendo desactivado
playback-continue-weighted = Seguir reproduciendo, primero lo nunca puesto
playback-crossfade-inside-albums = Dentro de los álbumes
playback-crossfade-off = Fundido encadenado desactivado
playback-crossfade-tip = Fundido encadenado de { $length }
playback-highlight-circle = Círculo
playback-highlight-square = Cuadrado
playback-hold-draw = { $tip }. Mantén pulsado para elegir un sorteo
playback-hold-length = { $tip }. Mantén pulsado para elegir una duración
playback-hold-order = { $tip }. Mantén pulsado para elegir un orden
playback-loop-off = Repetición desactivada
playback-loop-queue = Repetir la cola
playback-loop-track = Repetir esta pista
playback-menu-continue = Botón de continuar
playback-menu-crossfade = Botón de fundido encadenado
playback-menu-favourite = Botón de favorito
playback-menu-random = Botón de al azar
playback-menu-rating = Estrellas de valoración
playback-menu-stop = Botón de detener
playback-menu-stop-after = Botón de parar después
playback-menu-volume = Botón de volumen
playback-pause = Pausa
playback-play-highlight = Resalte de reproducir
    .description = El relleno de acento del botón de reproducir: un círculo, un cuadrado suave, o ninguno
playback-random-tip-random = Reproducir una pista al azar
playback-random-tip-similar = Reproducir una pista parecida a esta
playback-seek-back-tip = 10 segundos atrás
playback-seek-forward-tip = 10 segundos adelante
playback-shuffle-off = Aleatorio desactivado
playback-shuffle-on = Aleatorio activado, orden { $order }
playback-stop-after-armed = Parar después de esta pista, armado
playback-stop-after-tip = Parar después de esta pista
playback-stop-tip = Detener y descargar la pista
playback-volume-tip-muted = Quitar el silencio, { $percent }%. Clic derecho para el deslizador
playback-volume-tip-unmuted = Silenciar, { $percent }%. Clic derecho para el deslizador

## Track info panel
track-info-color-output-chip = Colorear la ficha de salida
    .description = Deja que la ficha se ponga de color de aviso cuando la salida cae a compartida o remuestrea. Desactivado la deja siempre en el mismo tono apagado, y la nota al pasar el cursor sigue explicando el estado
track-info-cycle-every = Rotar cada
    .description = Cuánto se queda cada fila antes del fundido
track-info-cycle-rows = Rotar las filas
    .description = Muestra las filas de la organización de una en una en una sola línea, con fundido entre ellas; una fila sola se lee tal cual
track-info-delay = Retardo
    .description = Cuánto descansa la línea en cada extremo antes de volver a moverse
track-info-marquee = Marquesina
    .description = Qué hace una línea demasiado larga para el panel: recorrer y volver, o girar sin fin
track-info-menu-overflow = Desbordamiento
track-info-next = Siguiente: { $line }
track-info-opening = abriendo...
track-info-output-fallback = El dispositivo rechazó la salida exclusiva, así que la reproducción va por el mezclador compartido. El dispositivo dijo: { $reason }
track-info-output-resample-exclusive = Este archivo es de { $source } kHz y la tarjeta aceptó { $device } kHz, así que cada muestra se convierte de camino a la salida. El dispositivo no quiso funcionar a la frecuencia propia del archivo.
track-info-output-resample-mixer = Este archivo es de { $source } kHz y el mezclador va a { $device } kHz, así que cada muestra se convierte de camino a la salida. El modo exclusivo le daría a la tarjeta la frecuencia propia del archivo.
track-info-overflow-loop = Girar
track-info-overflow-scroll = Recorrer
track-info-overflow-truncate = Truncar
track-info-queued-count = { $count } en cola
track-info-row-size = Tamaño de la fila { $number }
track-info-speed = Velocidad
    .description = A qué velocidad recorre la línea
track-info-text-size = Tamaño del texto

## Seek panel
seek-ending = Final
    .description = Cuenta atrás el tiempo restante o muestra la duración completa
seek-ending-remaining = Restante
seek-ending-total = Total
seek-playhead = Cursor de reproducción
    .description = Ocupa toda la altura de la barra o cíñete a la línea
seek-playhead-full = Completo
seek-playhead-line = Línea
seek-playhead-max-height = Altura máxima del cursor
    .description = Limita el cursor completo, centrado en la línea; 0 llena el panel
seek-playhead-width = Ancho del cursor
    .description = El ancho del marcador de posición que se mueve
seek-rounding = Redondeo
    .description = El radio de esquina de la línea, hasta una cápsula a la mitad del grosor
seek-scrobble-marker = Marca de scrobble
    .description = Una línea fina donde la pista cuenta como scrobbleada a Last.fm
seek-show-timings = Mostrar los tiempos
seek-thickness = Grosor
    .description = La altura de la línea de la pista

## Volume panel
volume-pieces = Piezas
    .description = Arrastra a lo largo de la barra para reordenar; arrastra entre las filas, o usa la x y el más de una ficha, para ocultar y mostrar. Con el porcentaje oculto lo muestra la ayuda emergente del altavoz
volume-readout = Lectura
    .description = Muestra el nivel como porcentaje o como la ganancia en decibelios que aplica
volume-readout-decibels = Decibelios
volume-readout-percent = Porcentaje
volume-stretch = Estirar
    .description = Deja que el deslizador llene el panel en vez de limitar su ancho
volume-tip-mute = Silenciar
volume-tip-mute-level = Silenciar, { $level }
volume-tip-unmute = Quitar el silencio
volume-tip-unmute-level = Quitar el silencio, { $level }

## Shared panel content
content-filter = Filtro
content-no-track = Sin pista
content-total-genres = Géneros
content-total-time = Tiempo total

## Shared panel chrome
panel-columns-description = Qué columnas de pista se ven
panel-headings = Encabezados
panel-jump-to-playing = Ir a lo que suena
panel-menu-display = Presentación
panel-title-artists = Artistas
panel-title-genres = Géneros
panel-title-oscilloscope = Osciloscopio
panel-title-particles = Partículas
panel-title-playback = Reproducción
panel-title-seek = Posición
panel-title-shader = Shader
panel-title-spectrogram = Espectrograma
panel-title-spectrum = Espectro
panel-title-theme-toggle = Alternar tema
panel-title-track-info = Info de la pista
panel-title-volume = Volumen
panel-title-vu = Medidor VU
panel-title-waveform = Forma de onda

## Everything else
choice-both = Ambos
choice-dim = Atenuar
choice-hide = Ocultar
composite-add-panel = Añadir panel
composite-host-settings = Ajustes de { $host }
composite-move-left = Mover a la izquierda
composite-move-right = Mover a la derecha
composite-remove = Quitar
composite-replace = Reemplazar
group-panel-add-slot = Añadir slot
group-panel-move-down = Mover abajo
group-panel-move-up = Mover arriba
group-panel-remove-slot = Quitar slot
group-panel-split-side-by-side = Dividir lado a lado
group-panel-split-stacked = Dividir apilado
group-panel-swap-panels = Intercambiar paneles
group-panel-title = Grupo
overlay-dim = Atenuar
    .description = Cuánto se atenúa el panel principal bajo la superposición revelada
overlay-title = Superposición
overlay-toggle = Alternar la superposición
shader-confirm-hint-after = alterna el shader desde cualquier sitio.
shader-confirm-hint-before = Un shader puede hacer que las ventanas cuesten de usar. Revierte o cierra esta ventana para volver a como estaba todo.
shader-confirm-keep = Mantener
shader-confirm-question = ¿Mantener este shader de pantalla?
shader-confirm-revert = Revertir
shader-confirm-window-title = rox - Shader de superposición
slide-add = Añadir diapositiva
slide-next = Diapositiva siguiente
slide-previous = Diapositiva anterior
slide-title = Diapositiva
theme-toggle-to-dark = Cambiar al tema oscuro
theme-toggle-to-light = Cambiar al tema claro
transport-favourite-add = Añadir a favoritos
transport-favourite-nothing = No hay nada que marcar como favorito
transport-favourite-remove = Quitar de favoritos
transport-pieces = Piezas
    .description = Arrastra a lo largo de una fila para reordenar y entre filas para mover; la x y el más de una ficha ocultan y muestran

## Stragglers picked up in the final sweep

duplicates-scanning = Escaneando...
about-copyright = Copyright © 2026
signal-name-placeholder = Nombre de la señal
signals-empty = Todavía no hay señales. Añade una, o haz clic derecho en cualquier mando enlazable.
signal-add = Añadir señal
panel-approve = Aprobar
panel-turn-off = Desactivar
shader-from-file = Desde un archivo...
arrange-add-row = Añadir fila
smart-playlist-name-placeholder = Nombre de la lista
smart-playlist-name-to-save = Ponle nombre a la lista para guardarla
panel-new-playlist = Nueva lista...
panel-edit-tags = Editar etiquetas...
panel-edit-cover = Editar la carátula...
panel-rename-files = Renombrar archivos...
panel-convert = Convertir...
panel-catalog-drag-anchor = Ancla de arrastre
panel-catalog-spacer = Separador

## Duration and worker phrasing

pace-under-a-minute = menos de un minuto
pace-minutes = { $count ->
    [one] alrededor de un minuto
   *[other] unos { $count } minutos
}
pace-hours = { $count ->
    [one] alrededor de una hora
   *[other] unas { $count } horas
}
pace-half-hours = unas { $value } horas
pace-days = { $count ->
    [one] alrededor de un día
   *[other] unos { $count } días
}
pace-workers = { $count ->
    [one] { $count } proceso
   *[other] { $count } procesos
}
tasks-rest-takes = , el resto tarda { $estimate }
tasks-measuring-takes = , medirlas tarda { $estimate }
tasks-working-out-takes = , calcularlas tarda { $estimate }
tasks-time-left = , { $left } para terminar
tasks-failed-suffix = { $count ->
    [one] ({ $count } falló)
   *[other] ({ $count } fallaron)
}
tasks-file-suffix = - { $file }
tasks-no-beat-suffix = ({ $count } sin tempo claro)
tasks-estimate-at-workers = ({ tasks-estimate-at })

## Panel vanity names

panel-title-art-view = Vista de carátulas
panel-title-artist-grid = Cuadrícula de artistas
panel-title-genre-grid = Cuadrícula de géneros
panel-title-biography = Biografía
panel-title-cover-art = Carátula
panel-title-drag-anchor = Ancla de arrastre
panel-title-drawer = Cajón
panel-title-eq-widget = Widget de EQ
panel-title-filter = Filtro
panel-title-folder-tree = Árbol de carpetas
panel-title-group = Grupo
panel-title-history = Historial
panel-title-lyrics = Letras
panel-title-menu = Menú
panel-title-metadata = Metadatos
panel-title-mini-toggle = Alternar mini
panel-title-output = Salida
panel-title-overlay = Superposición
panel-title-playlists = Listas de reproducción
panel-title-queue = Cola
panel-title-queue-widget = Widget de cola
panel-title-search = Búsqueda
panel-title-slide = Diapositiva
panel-title-spacer = Separador
panel-title-stats-widget = Widget de estadísticas
panel-title-vu-meter = Medidor VU
panel-title-window-controls = Controles de ventana

## Relative time and the output headline

ago-just-now = ahora mismo
ago-minutes = hace { $count } min
ago-hours = hace { $count } h
ago-days = hace { $count } d
ago-weeks = hace { $count } sem
ago-years = hace { $count } a

span-seconds = { $count ->
    [one] { $count } segundo
   *[other] { $count } segundos
}
span-minutes = { $count ->
    [one] { $count } minuto
   *[other] { $count } minutos
}
span-hours = { $count ->
    [one] { $count } hora
   *[other] { $count } horas
}
span-days = { $count ->
    [one] { $count } día
   *[other] { $count } días
}
span-weeks = { $count ->
    [one] { $count } semana
   *[other] { $count } semanas
}
span-years = { $count ->
    [one] { $count } año
   *[other] { $count } años
}
span-pair = { $first }, { $second }
unit-percent = { $value } %

settings-audio-output-headline = { $mode }{ $note } en { $device }, { $rate } Hz, { $channels } can., { $format }
settings-audio-output-experimental =  (experimental)

## ML model catalog

settings-mlmodels-description = { $summary }. { $dim } valores por pista. { $licence }
settings-mlmodels-on-disk = , { $size } en disco
settings-mlmodels-to-download = , { $size } por descargar
model-summary-dsp-timbre-1 = Integrado, sin descarga. Un resumen de la energía por bandas logarítmicas, la forma espectral y la tasa de ataques de cada pista. Tosco al lado de una red entrenada, pero no necesita nada y funciona en todas partes
model-summary-panns-cnn10 = Una red convolucional entrenada con AudioSet para reconocer qué es un sonido. Su descripción de una pista en 512 valores es mucho más rica que el boceto integrado, a costa de una descarga de 24 MB y un análisis más lento
dictionary-summary-lindera-ipadic = El diccionario japonés que hay detrás de las lecturas de los kanji. Sin él, el kana y el hangul se siguen romanizando y el chino se sigue leyendo como pinyin, pero un título en kanji se omite

## Shipped workspaces

workspace-shipped-default = (Predeterminado)
workspace-shipped-default-blurb = Cómo se ve rox recién instalado: superficies translúcidas sobre el escritorio, sin marco de ventana, sin tinte de carátula. El punto de partida del que se aparta cualquier otro aspecto de aquí.
workspace-shipped-catrox-blurb = La skin de foobar2000 con la que empezó todo, reconstruida: una representación circular de la carátula como CD, los campos de metadatos por la izquierda, y pistas agrupadas por álbum con puntos de valoración.
workspace-shipped-critters-blurb = Toda la aplicación como una impresión de 1 bit: un dithering ordenado sobre cada superficie, tonos que se aplastan con los subgraves, y un muro de ruido que se retuerce con la canción. Inspirado en Critters for Sale.
workspace-shipped-diffuse-blurb = Solo el álbum en reproducción: la carátula y la tarjeta de reproducción como un único grupo que llena la ventana, superficies transparentes sobre el fondo, sin juntas. La biblioteca, la cola y las letras esperan en un cajón del borde derecho y salen deslizándose sobre la música cuando el cursor toca el tirador. Monocromo, así que el color lo ponen las carátulas.
workspace-shipped-foobar-blurb = La disposición con la que discute todo este proyecto. Paneles opacos, columnas de filtro por artista y álbum, una tabla de pistas densa, y la barra de menús justo donde estuvo siempre.
workspace-shipped-llama-winamp-blurb = Winamp tal como lo recuerdas y no tal como era. Tahoma, oscuro, sin marco, un espectro punteado arriba, y un modo reducido en la disposición mini.
workspace-shipped-metro-blurb = Paneles planos y filas holgadas en Segoe UI, con el tinte de carátula activado para que toda la paleta siga a la carátula que esté sonando.
workspace-shipped-phosphor-blurb = Todo en monoespaciada. Consolas, verde sobre negro, sin carátula en la reproducción rápida: un terminal que resulta que reproduce música.
