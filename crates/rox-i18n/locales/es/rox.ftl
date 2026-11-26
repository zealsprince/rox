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
toast-copied = Copiado al portapapeles
matcher-section-matches = Coincidencias

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
    .description = Qué sigue la señal: Banda rastrea un rango de frecuencias, Nivel toda la mezcla, Ataque pulsa en cada golpe del rango, Beat pulsa en cada tiempo del rango aunque la música sea densa y fuerte, Disparo lanza un pulso cuando el rango alcanza su umbral, Total suma otra señal a lo largo del tiempo
signal-kind-band = Banda
signal-kind-level = Nivel
signal-kind-onset = Ataque
signal-kind-trigger = Disparo
signal-kind-total = Total

# A signal's name until the user gives it one, worded from its source.
# $range is a band's span in hertz, the unit written once when both ends
# share it; $rate is a total's laps a second.
signal-range = { $lo }–{ $hi }
signal-label-band = { signal-kind-band } { $range }
signal-label-onset = { signal-kind-onset } { $range }
signal-label-trigger = { signal-kind-trigger } { $range }
signal-label-total = { signal-kind-total } { $rate }/s
signal-label-total-of = { signal-kind-total } de { $signal }, { $rate }/s

signal-response = Respuesta
signal-response-pulse = Cuánto resuena cada pulso antes de apagarse
signal-response-drift = 0 se pega a la música, 100 va a la zaga
signal-threshold = Umbral
signal-threshold-trigger = El nivel que el rango tiene que alcanzar para lanzar el pulso; no vuelve a dispararse hasta que el nivel cae por debajo de la marca del medidor de arriba
signal-threshold-gate = Por debajo de esto la señal no marca nada, y por encima la salida vuelve a subir desde cero, así las partes suaves no mueven el mando. La marca del medidor de arriba señala dónde está
signal-kind-beat = Beat
signal-label-beat = { signal-kind-beat } { $range }
signal-threshold-beat = El nivel que el rango tiene que alcanzar para que un beat cuente, así las partes suaves se quedan quietas. La marca del medidor de arriba señala dónde está
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
    .description = Devuélvelo a cero ahora. Se vacía poco a poco en vez de saltar, así nada de lo que lo sigue pega un tirón
signal-routes-in-panel = { $count ->
    [one] { $count } ruta en este panel
   *[other] { $count } rutas en este panel
}
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
panel-settings-window-title = rox - Ajustes de { $panel }
panel-menu-label = Panel
panel-save-as-preset = Guardar como preajuste
panel-save-preset-window-title = rox - Guardar { $panel } como preajuste
panel-rename = Renombrar
panel-rename-name = Nombre
panel-rename-note = Se muestra como pestaña del panel; en blanco vuelve al nombre integrado
panel-rename-hint-after = para renombrar
panel-rename-window-title = rox - Renombrar { $panel }
shader-confirm-hash = Hash { $hash }
shader-confirm-arrived = Este shader llegó dentro de una disposición o un espacio de trabajo. No se ejecuta hasta que lo leas y lo apruebes en este equipo.
signal-routes-none = Nada enrutado: cada ranura lee cero hasta que una ruta le da una señal. Un shader puede nombrar sus ranuras con comentarios `// @slot 0: bass` y los nombres aparecen aquí.
signal-routes-full = Las dieciséis ranuras están enrutadas. Apunta una a otro sitio o bórrala para añadir otra.
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
panel-colors-overrides-note = Las sustituciones recolorean solo este panel y no se mueven con el color de la canción; un enlace sigue en cambio otro color de la app y cambia con el tema. Restablece una muestra o vacía su campo hexadecimal para volver a seguir la paleta de la app
panel-section-font = Fuente
panel-section-slider = Deslizador
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
panel-slider-rounding = Redondeo del deslizador
    .description = Redondea la pista y el botón de los deslizadores de este panel; restablecer sigue al espacio de trabajo
panel-slider-knob = Botón del deslizador
    .description = El tirador de los deslizadores de este panel; restablecer sigue al espacio de trabajo
panel-surface-shader = Shader de superficie
    .description = Ejecuta un shader WGSL sobre el cuerpo de este panel, por debajo del shader de pantalla de la aplicación
panel-run-when-idle = Seguir en reposo
    .description = Sigue dibujando fotogramas mientras el audio está en silencio. Desactivado, el shader se congela en su último fotograma y el panel no cuesta nada
panel-shader-is-scene = Este shader es una escena, así que cubre el cuerpo del panel en vez de dibujar encima. Vino de un paquete o de una configuración antigua; la lista de arriba solo ofrece shaders que dejan el panel legible.

## Shader picker and saving
shader-source = Origen
shader-pick-none = Ninguno
shader-reload = Recargar
shader-no-pipeline-title = Los shaders no funcionan en esta compilación
shader-run-when-idle-description = Sigue dibujando mientras el audio está en silencio. Desactivado, el shader se queda donde está y el panel no cuesta nada
library-busy-loading = cargando la biblioteca...
library-busy-scanning = escaneando...
library-busy-removing = quitando...
library-busy-refreshing = actualizando...
library-busy-syncing = sincronizando...
library-busy-stopping = deteniendo...
library-busy-clearing-vectors = borrando vectores...
library-busy-clearing-tempos = borrando tempos...
library-busy-clearing-listens = borrando escuchas...
library-busy-scanning-progress = escaneando { $done }/{ $total }
shader-no-pipeline-note = Esta compilación dibuja con un backend sin canal de shaders. Los shaders van sobre los canales de render de blade, así que se rechaza cualquier fuente, diga lo que diga.
shader-unapproved-post = Este shader llegó con un espacio de trabajo y aún no se ha aprobado en este equipo
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
preset-back-tail-global = en cualquier menú de panel. Los preajustes globales aparecen en todos los espacios de trabajo.
preset-group-builtin = Incluidos
preset-group-global = Globales
preset-group-workspace = Este espacio de trabajo
preset-group-other = De { $workspace }
preset-none-for-kind = Todavía no hay preajustes para este panel
preset-reset = Restablecer cambios
preset-save-scope = Guardar en
    .description = Los preajustes globales aparecen en todos los espacios de trabajo y llevan sus shaders consigo
preset-scope-workspace = Este espacio de trabajo
preset-scope-global = Global
preset-save-replaces-global = Reemplaza el preajuste global que ya se llama { $name }

## Keyboard hints
hint-press = Pulsa
hint-key-enter = Intro
hint-to-save = para guardar
hint-to-apply = para aplicar
hint-to-convert = para convertir
hint-to-repair = para reparar

## Dialog buttons

# The commit and scan buttons the small editor windows share.
dialog-save = Guardar
dialog-apply = Aplicar
dialog-scan = Escanear
dialog-rescan = Volver a escanear

## Settings: language
settings-language = Idioma
    .description = El idioma de la interfaz. Sistema compara con la lista del sistema operativo y recurre al inglés cuando no coincide nada
    .keywords = traducción idioma lengua configuración regional
settings-language-system = (Idioma del sistema)
settings-language-search = Buscar idiomas
picker-no-matches = Sin coincidencias
picker-icon-none = Ninguno
picker-icon-search = Buscar iconos
picker-font-default = Predeterminada
picker-font-search = Buscar fuentes
settings-search-no-matches = Nada coincide con "{ $text }"
settings-search-scope = Buscar solo en la página abierta

## Embed dialog
bookmark-new-title = Nuevo marcador
bookmark-edit-title = Editar marcador
bookmark-window-title = rox - { $verb }
bookmark-name = Nombre
bookmark-name-placeholder = Opcional, sin nombre se muestra la hora
bookmark-position = En { $time }
bookmark-color = Color
bookmark-color-accent = Acento del tema
bookmark-color-custom = Personalizado...
bookmark-color-red = Rojo
bookmark-color-orange = Naranja
bookmark-color-yellow = Amarillo
bookmark-color-green = Verde
bookmark-color-teal = Turquesa
bookmark-color-blue = Azul
bookmark-color-purple = Morado
bookmark-color-pink = Rosa
bookmark-save = Guardar
bookmark-hint-before = Pulsa
bookmark-hint-key = Intro
bookmark-hint-after = para guardar
bookmark-menu-play = Reproducir desde aquí
bookmark-menu-rename = Renombrar...
bookmark-menu-color = Color
bookmark-menu-move = Mover a la posición actual
bookmark-menu-remove = Quitar
bookmark-menu-remove-many = Quitar { $count } marcadores
bookmarks-remove-title = { $count ->
    [one] ¿Quitar 1 marcador?
   *[other] ¿Quitar { $count } marcadores?
}
bookmarks-remove-body = { $count ->
    [one] El marcador desaparece y su pista se queda en la biblioteca.
   *[other] Los marcadores desaparecen y sus pistas se quedan en la biblioteca.
}
bookmarks-title = Marcadores
bookmarks-empty = Aún no hay marcadores. Pulsa M mientras suena una pista para poner uno, o Mayús+M para darle nombre.
cue-menu-insert = Insertar punto aquí
cue-menu-bookmark = Añadir marcador aquí
cue-menu-remove = Quitar punto
position-bound-streaming = No disponible durante la emisión
bake-window-title = rox - Incrustar metadatos guardados
bake-title = Incrustar metadatos guardados
bake-intro = Escribe los metadatos guardados en los propios archivos, para que otro reproductor también los lea. No se recalcula nada.
bake-formats = Solo MP3 y FLAC; se omiten los demás formatos y las pistas de CUE
bake-source-lyrics = Letras
bake-source-gain = ReplayGain
bake-source-acoustic = Descripciones acústicas
bake-source-tempo = Tempo
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
filter-field-source = Fuente
filter-field-year = Año
filter-field-folder = Carpeta
filter-field-comment = Comentario
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
panel-move-to-trash = Mover a la papelera...
panel-play-next = Reproducir a continuación
panel-add-to-queue = Añadir a la cola
panel-add-to-playlist = Añadir a una lista
panel-remove-bookmarks = { $count ->
    [one] Quitar marcador
   *[other] Quitar { $count } marcadores
}
panel-favourite-add = Añadir a favoritos
panel-favourite-remove = Quitar de favoritos
panel-copy = Copiar
panel-copy-title = Copiar título
panel-copy-artist = Copiar artista
panel-copy-album = Copiar álbum
panel-copy-filename = Copiar nombre de archivo
panel-copy-path = Copiar ruta
panel-copy-link = Copiar enlace
panel-open-in-browser = Abrir en el navegador
panel-link-missing = No tiene página web
panel-link-failed = No se pudo obtener el enlace
plugin-action-run = Ejecutar
plugin-action-done = { $action } terminó
plugin-action-failed = { $action } falló
plugin-action-stopped = { $action } se detuvo
plugin-action-show-in-folder = Mostrar en la carpeta
plugin-action-open-link = Abrir enlace
plugin-action-by-client = { $action }, ejecutada por un cliente MCP
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

## Go-to modal

goto-placeholder = 1:23, 83 o 1:02:03
goto-nothing-playing = No se está reproduciendo nada
goto-target = Ir a { $time }
goto-unreadable = No es un tiempo
goto-hint-before = Pulsa
goto-hint-key = Intro
goto-hint-after = para ir

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
info-item-artist-scrobbles = Scrobbles del artista
info-item-track-scrobbles = Scrobbles de la pista
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
playback-item-ab-repeat = Repetir A-B
playback-item-favourite = Favorito
playback-item-rating = Valoración
playback-sleep-15 = Parar después de 15 minutos
playback-sleep-30 = Parar después de 30 minutos
playback-sleep-60 = Parar después de una hora
playback-sleep-90 = Parar después de 90 minutos
playback-sleep-off = Sin temporizador
playback-sleep-off-remaining = Cancelar el temporizador (quedan { $minutes } min)
playback-sleep-tip = Temporizador de sueño
playback-sleep-tip-remaining = Temporizador, quedan { $minutes } min
menubar-button-tasks = Botón de tareas
menubar-button-sleep = Botón del temporizador
menubar-button-rescan = Botón de reescaneo
menubar-rescan-tip = Volver a escanear las carpetas de la biblioteca
menubar-stop-scan-tip = Detener el escaneo
menubar-design-mode-tip = Modo de diseño: añadir, ordenar y quitar paneles

# The menubar's line after a scan: the track count, then what changed.
library-status-with-details = { $tracks } ({ $details })
library-status-joiner = { ", " }
library-status-indexed = { $count } indexadas
library-status-unchanged = { $count } sin cambios
library-status-untagged = { $count } sin etiquetas
library-status-removed = { $count } quitadas
library-status-unreachable = { $count ->
    [one] { $count } carpeta inaccesible
   *[other] { $count } carpetas inaccesibles
}
library-status-stopped-early = detenido antes de tiempo
library-status-updated = { $count } actualizadas
library-status-renamed = { $count } renombradas
player-error-device-lost = salida de audio: dispositivo perdido
sources-error-no-server = no hay ningún servidor configurado
sources-error-sync-running = ya hay una sincronización en curso
lastfm-auth-needs-key = introduce primero una clave de API y un secreto
plugin-radio-empty = la radio volvió vacía
job-nothing-to-measure = no queda nada por medir
job-nothing-decodable = nada decodificable
job-nothing-to-romanize = no queda nada por romanizar
discord-unknown-track = Pista desconocida
discord-unknown-artist = Artista desconocido
discord-button-lastfm = Ver en Last.fm
discord-button-youtube = Buscar en YouTube
settings-library-acoustic-extractor-installed = El integrado no necesita descarga y describe timbre y ritmo. { $model } oye más y es el modelo que ofrece la página Modelos ML. Al cambiar, la biblioteca se vuelve a describir con el nuevo
settings-library-acoustic-extractor-none = El integrado no necesita descarga y describe timbre y ritmo. No hay ningún modelo instalado, así que aún no hay a qué cambiar: descarga { $model }, o elige un archivo de pesos propio, en la página Modelos ML
settings-library-watch-over-limit = Desactivado: esta biblioteca abarca { $dirs } carpetas y cada una necesita una vigilancia de archivos de Linux, más de las { $limit } que la app toma del presupuesto compartido del sistema. Vuelve a escanear a mano para incorporar los cambios
settings-storage-vectors-known = { $model }, { $dim } valores por pista. Borrar devuelve el espacio, y volver a tener las descripciones supone una pasada entera por la biblioteca
settings-storage-vectors-orphaned = { $dim } valores por pista. Nada en esta compilación escribe este modelo, así que son restos de uno renombrado o retirado, y borrarlos no le quita a la biblioteca nada que use
settings-mlmodels-custom-description = { $path }. { $dim } valores por pista, guardados bajo { $id }, así sus vectores nunca se mezclan con los del catálogo

## Custom button states

button-state-playback = Reproducción
button-state-playback-playing = En reproducción
button-state-playback-paused = En pausa
button-state-repeat = Repetir
button-state-repeat-off = Desactivado
button-state-repeat-all = Cola
button-state-repeat-one = Pista
button-state-shuffle = Orden aleatorio
button-state-shuffle-random = Al azar
button-state-shuffle-similar = Parecido
button-state-shuffle-albums = Álbumes
button-state-shuffling = Aleatorio
button-state-shuffling-on = Activado
button-state-shuffling-off = Desactivado
button-state-mute = Silenciar
button-state-mute-muted = Silenciado
button-state-mute-unmuted = Con sonido
button-state-stop-after = Parar después de la pista
button-state-stop-after-armed = Armado
button-state-stop-after-off = Desactivado
button-state-ab-repeat = Repetir A-B
button-state-ab-repeat-off = Sin marcas
button-state-ab-repeat-a-set = A marcado
button-state-ab-repeat-looping = Repitiendo
button-state-continuation = Seguir reproduciendo
button-state-continuation-off = Desactivado
button-state-continuation-continue = Lista abajo
button-state-continuation-weighted = Primero lo nunca puesto
button-state-stop = Detener
button-state-stop-active = Algo cargado
button-state-stop-idle = Nada cargado
button-state-crossfade = Fundido encadenado
button-state-crossfade-off = Desactivado
button-state-crossfade-on = Activado
button-state-crossfade-albums = Fundido dentro de los álbumes
button-state-crossfade-albums-off = Desactivado
button-state-crossfade-albums-on = Activado
button-state-sleep = Temporizador de sueño
button-state-sleep-armed = Armado
button-state-sleep-off = Desactivado
button-state-eq = Ecualizador
button-state-eq-on = Activado
button-state-eq-off = Desactivado
button-state-exclusive-output = Salida exclusiva
button-state-exclusive-output-on = Activado
button-state-exclusive-output-off = Desactivado
button-state-replaygain = ReplayGain
button-state-replaygain-off = Desactivado
button-state-replaygain-track = Pista
button-state-replaygain-album = Álbum

button-state-theme = Tema
button-state-theme-dark = Oscuro
button-state-theme-light = Claro
button-state-design-mode = Modo de diseño
button-state-design-mode-on = Activado
button-state-design-mode-off = Desactivado
button-state-resize-lock = Bloqueo del tamaño de los paneles
button-state-resize-lock-locked = Bloqueado
button-state-resize-lock-unlocked = Desbloqueado
button-state-menubar = Barra de menús
button-state-menubar-shown = Visible
button-state-menubar-hidden = Oculta
button-state-decorations = Decoraciones del sistema
button-state-decorations-on = Activado
button-state-decorations-off = Desactivado
button-state-art-theming = Color de la canción
button-state-art-theming-on = Activado
button-state-art-theming-off = Desactivado
button-state-quit-to-tray = Quedarse en la bandeja
button-state-quit-to-tray-on = Activado
button-state-quit-to-tray-off = Desactivado
button-state-seams = Juntas entre paneles
button-state-seams-on = Activado
button-state-seams-off = Desactivado
button-state-readings = Lecturas de los nombres
button-state-readings-on = Activado
button-state-readings-off = Desactivado
button-state-scrobbling = Scrobbling
button-state-scrobbling-on = Activado
button-state-scrobbling-off = Desactivado
button-state-discord = Presencia en Discord
button-state-discord-on = Activado
button-state-discord-off = Desactivado
button-state-broadcast = Emisión por Icecast
button-state-broadcast-on = Activado
button-state-broadcast-off = Desactivado
button-state-capture = Captura de canciones
button-state-capture-on = Activado
button-state-capture-off = Desactivado
button-state-favourite = Favorito
button-state-favourite-on = Favorito
button-state-favourite-half = Favorito en un solo lado
button-state-favourite-off = No es favorito
button-state-favourite-none = Nada que marcar
button-state-mini = Disposición mini
button-state-mini-mini = Mini
button-state-mini-primary = Principal
button-state-watch-folders = Vigilar carpetas
button-state-watch-folders-on = Activado
button-state-watch-folders-off = Desactivado
button-state-post-shader = Shader de superposición
button-state-post-shader-on = Activado
button-state-post-shader-off = Desactivado
button-state-backdrop-shader = Shader de fondo
button-state-backdrop-shader-on = Activado
button-state-backdrop-shader-off = Desactivado
button-state-backdrop-all-windows = Fondo en todas las ventanas
button-state-backdrop-all-windows-on = Activado
button-state-backdrop-all-windows-off = Desactivado
button-state-milkdrop-backdrop = Fondo Milkdrop
button-state-milkdrop-backdrop-on = Activado
button-state-milkdrop-backdrop-off = Desactivado
button-state-milkdrop-hard-cuts = Cortes duros de Milkdrop
button-state-milkdrop-hard-cuts-on = Activado
button-state-milkdrop-hard-cuts-off = Desactivado
button-state-milkdrop-lock = Bloqueo del preset de Milkdrop
button-state-milkdrop-lock-locked = Bloqueado
button-state-milkdrop-lock-unlocked = Desbloqueado

## Custom controls panel

# The panel of user-built buttons: its name in the catalog and on its tab,
# and the placeholder a button with no command wears.
panel-catalog-custom-controls = Controles propios
panel-title-custom-controls = Controles propios
panel-controls-unconfigured = Aún sin configurar

## Custom button editor

# The panel's settings page: the button list, what each button does, the
# state it follows, and the look it wears in each case of that state.
button-editor-section-buttons = Botones
button-editor-section-layout = Disposición
    .description = Los botones de la tira, en orden. Todos empiezan en la bandeja de abajo, y arrastrar uno de vuelta lo quita de la tira sin borrarlo.
button-editor-add = Añadir botón
button-editor-empty = Aún no hay botones. Añade uno y elige qué hace.
button-editor-unnamed = Sin comando
button-editor-action = Comando
    .description = Lo que hace un clic en este botón.
button-editor-action-none = Elige un comando
button-editor-action-search = Buscar comandos
button-editor-action-empty = Sin coincidencias
button-editor-state = Sigue
    .description = El estado del reproductor que siguen el icono, el color y el tooltip de este botón.
button-editor-state-none = Nada
button-editor-case-always = Siempre
button-editor-preview = Vista previa
button-editor-icon = Icono
button-editor-color = Color
button-editor-tip = Tooltip
button-editor-tip-placeholder = Lo que dice al pasar el puntero

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
library-collapse-all = Contraer todo
library-expand-all = Expandir todo
library-disc = Disco { $number }
library-empty-title = Abre una carpeta de música
library-empty-note = Se escanea a la biblioteca (flac, mp3, ogg, opus, m4a, aac, wav, aiff)
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
library-column-rename-window-title = rox - Renombrar { $column }
library-sort-on-click = Ordenar al hacer clic
    .description = Ordena al hacer clic en cualquier parte del encabezado en vez de en su icono; reordenar una columna pasa a pedir Alt y arrastrar
library-group-search-results = Agrupar resultados de búsqueda
    .description = Mantiene los encabezados de grupo actuales durante la búsqueda; desactívalo para mostrar las pistas coincidentes como una lista plana
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
library-preset-compact = Compacto
library-preset-showcase = Escaparate
library-preset-dj = DJ
library-preset-tagging = Etiquetado
library-preset-audiophile = Audiófilo

## Stations panel

stations-title = Emisoras
stations-play = Reproducir
stations-play-count = Reproducir { $count } emisoras
stations-remove = Quitar
stations-remove-count = Quitar { $count } emisoras
stations-remove-title = { $count ->
    [one] ¿Quitar 1 emisora?
   *[other] ¿Quitar { $count } emisoras?
}
stations-remove-body = { $count ->
    [one] La emisora sale de la biblioteca hasta que vuelvas a añadir su URL.
   *[other] Las emisoras salen de la biblioteca hasta que vuelvas a añadir sus URL.
}
stations-find = Buscar emisoras...
stations-manage = Gestionar emisoras...
stations-import = Importar emisoras...
stations-import-empty = Ese archivo no contiene ninguna URL de flujo
stations-not-a-stream = Las emisoras se escuchan por http o https; eso no es una URL de flujo
stations-checking = Comprobando el flujo...
stations-is-hls = Eso es una lista de segmentos HLS; rox reproduce un flujo continuo
stations-is-a-playlist = Eso es una lista de emisoras; léela con Importar emisoras
stations-not-audio = Esa URL responde con { $kind }, no con audio
stations-added-unchecked = Añadida sin comprobar: el flujo no respondió ({ $reason })
stations-empty-title = Aún no hay emisoras
stations-empty = Busca una en el directorio, o lleva tu lista en los ajustes.
stations-on-air = En el aire
stations-clocks = Tiempo en esta canción y en esta emisora
stations-homepage = Abrir la web

## Station directory

directory-window-title = Directorio de emisoras
directory-placeholder = Buscar emisoras en radio-browser.info
directory-search = Buscar en el directorio
directory-add = Añadir
directory-add-play = Añadir y reproducir
directory-added = Ya está en tus emisoras
directory-searching = Buscando en radio-browser.info...
directory-none = Nada en radio-browser.info coincide con "{ $text }"
directory-failed = radio-browser.info no respondió: { $reason }
directory-bitrate = { $kbps } kbps

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
grid-preset-spotlight = Foco
grid-preset-compact = Compacto
grid-preset-browse = Explorar
grid-preset-shelf = Estante
grid-preset-new-arrivals = Novedades

## Settings: sidebar pages
settings-window-title = rox - Ajustes
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
settings-page-plugins = Plugins
settings-page-providers = Proveedores
settings-page-radio = Radio
settings-page-renderers = Renderizadores
settings-page-shader = Shader
settings-page-storage = Almacenamiento
settings-page-workspace = Espacio de trabajo

## Settings: appearance
settings-appearance-backdrop-all-windows = Todas las ventanas
    .description = Pon fondo también a las ventanas secundarias: ajustes, editores, diálogos, paneles sacados a ventana. Desactivado deja el fondo y la transparencia solo en las ventanas del espacio de trabajo
settings-appearance-backdrop-strength = Intensidad del fondo
    .description = Con cuánta fuerza se ve el fondo de carátula detrás de ellas
settings-appearance-milkdrop-beat-sensitivity = Sensibilidad al beat
    .description = Con qué facilidad projectM cuenta algo como beat. Una mezcla suave necesita más que una fuerte
settings-appearance-milkdrop-color = Color
    .description = Preset conserva los colores del archivo. Tema los invierte hacia claro en el tema claro. Paleta los pinta con el fondo y el acento del tema. Carátula los pinta con el color de la carátula en reproducción, con o sin tematización por canción
settings-appearance-milkdrop-duration = Duración del preset
    .description = Cuánto se mantiene un preset antes de que aparezca el siguiente
settings-appearance-milkdrop-enabled = Fondo Milkdrop
    .description = Ejecutar una visualización Milkdrop detrás de toda la app mientras suena música
settings-appearance-milkdrop-error-title = El fondo Milkdrop no pudo arrancar
settings-appearance-milkdrop-fade-duration = Duración del fundido
    .description = Cuánto dura el fundido, de salida al pausar o parar y de entrada al reproducir
settings-appearance-milkdrop-flip-horizontal = Voltear horizontalmente
    .description = Refleja el visual de izquierda a derecha
settings-appearance-milkdrop-flip-vertical = Voltear verticalmente
    .description = Refleja el visual de arriba abajo
settings-appearance-milkdrop-fps = Fotogramas por segundo
    .description = Lo que busca el hilo de render. 30 reduce a la mitad lo que cuesta leer cada fotograma, y un fondo desenfocado rara vez necesita más
settings-appearance-milkdrop-hard-cuts = Cortes duros
    .description = Dejar que un beat lo bastante fuerte corte directo al siguiente preset
settings-appearance-milkdrop-idle = En reposo
    .description = Qué hace el visual mientras la reproducción está en pausa o detenida. Mantener deja el último fotograma detrás de la app. Fundir lo lleva al fondo de la portada y lo trae de vuelta al reproducir
settings-appearance-milkdrop-locked = Bloquear preset
    .description = Quedarse en el preset actual, entre pistas y reinicios
settings-appearance-milkdrop-preset = Preset
    .description = Qué preset muestra el fondo. Funciona sin reproducción y vuelve tras un reinicio
settings-appearance-milkdrop-rotation = Rotación
    .description = Qué presets recorren Siguiente, Anterior y el cambio por tiempo
settings-appearance-milkdrop-scale = Escala de renderizado
    .description = A qué fracción de la resolución de la ventana se renderiza la visualización. Menos cuesta menos; un fondo tan suave rara vez necesita más
settings-appearance-milkdrop-strength = Intensidad
    .description = Hasta dónde llega la visualización sobre la carátula desenfocada
settings-appearance-border = Borde
    .description = Una línea alrededor del borde de cada panel, en el color del rol Borde; un lado a cero no dibuja ninguna
settings-appearance-chrome-side = Lado de los botones de ventana
    .description = En qué extremo de la barra de título propia de rox van los botones; se dibuja donde el compositor no decora la ventana y en las ventanas secundarias sin marco
settings-appearance-chrome-side-left = Izquierda
settings-appearance-chrome-side-right = Derecha
settings-appearance-chrome-style = Estilo de los botones de ventana
    .description = Cómo se dibujan los botones de la barra de título propia de rox; se dibuja donde el compositor no decora la ventana y en las ventanas secundarias sin marco
settings-appearance-cjk-fonts-note = El texto en japonés, chino o coreano de tu biblioteca prueba todas las fuentes instaladas una por una. Eso ralentiza el desplazamiento y lo dibuja con una fuente de reemplazo. Instala las fuentes Noto Sans CJK estáticas: noto-fonts-cjk-sans-static en NixOS, google-noto-sans-cjk-fonts en Fedora, noto-fonts-cjk en Arch, fonts-noto-cjk en Debian.
settings-appearance-cjk-fonts-title = No hay una fuente CJK de peso normal
settings-appearance-colors-locked-note = El color de la canción está activado, así que la pista en reproducción manda sobre estos colores y la exportación los guarda. Desactívalo arriba para editarlos
settings-appearance-design-mode = Modo de diseño
    .description = Edita la disposición en su sitio: el menú Paneles de la barra de menús, las filas de añadir, renombrar, duplicar, sacar a ventana y cerrar de los menús de panel, los controles que un contenedor pone sobre sus slots, y el arrastre de pestañas. Desactivado esconde todo eso; la página Espacio de trabajo sigue editando el árbol
    .keywords = editar disposición reordenar bloquear
settings-appearance-font = Fuente
    .description = La tipografía de toda la aplicación; los paneles pueden sustituirla en sus propios ajustes
    .keywords = tipografía familia texto
settings-appearance-font-size = Tamaño de fuente
    .description = El tamaño base desde el que escala el texto de cada panel; los controles y los iconos mantienen su tamaño
settings-appearance-hide-menubar = Ocultar la barra de menús
    .description = Mantén la barra de menús oculta y hazla flotar sobre el dock mientras mantienes pulsado Alt. Pulsa Alt dos veces para dejarla fija, así sus botones aceptan un clic normal
settings-appearance-inverse-from-dark = Invertir desde el tema oscuro
settings-appearance-inverse-from-light = Invertir desde el tema claro
settings-appearance-keep-theme = Mantener el tema
    .description = Mantén el tema activo aunque el brillo de una carátula lo haría cambiar; el color de la canción sigue tiñendo
settings-appearance-margin = Margen
    .description = Mete cada panel hacia dentro de su celda; un panel puede sustituirlo en sus propios ajustes
settings-appearance-os-decorations = Decoraciones del sistema
    .description = La barra de título y los bordes del sistema en las ventanas principales; desactivadas, todo recae en los controles de ventana y los paneles con ancla de arrastre
settings-appearance-bare-child-windows = Ventanas secundarias sin marco
    .description = Con las decoraciones del sistema desactivadas, los ajustes, los editores y los paneles desacoplados también pierden el marco del sistema y dibujan la barra de título propia de rox; el borde de redimensión se les aplica igual
settings-appearance-child-titlebar = Barra de título de las ventanas secundarias
    .description = La barra de título que rox dibuja en las ventanas secundarias sin marco; desactivada, se quedan sin nada alrededor y el atajo Cerrar la ventana las cierra
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
settings-appearance-section-interface = Interfaz
settings-appearance-section-milkdrop = Fondo Milkdrop
settings-appearance-section-theming = Coloreado
settings-appearance-section-transparency = Transparencia
settings-appearance-section-typography = Tipografía
settings-appearance-slider-knob = Botón del deslizador
    .description = El tirador de cada deslizador; desactivado, solo la pista rellena muestra el valor
settings-appearance-slider-rounding = Redondeo de deslizadores
    .description = Redondea las esquinas de la pista y el botón de cada deslizador; 0 las deja rectas
settings-appearance-song-theming = Color de la canción
    .description = Tiñe la paleta y pon de fondo la carátula de la pista en reproducción
settings-appearance-surface-opacity = Opacidad de las superficies
    .description = Cuán opacas se ven las superficies de la aplicación sobre el fondo
settings-appearance-theme = Tema
    .description = La paleta que dibuja la aplicación y la que apunta el editor de color de abajo; Sistema sigue la preferencia clara u oscura del sistema operativo
settings-appearance-theme-dark = Oscuro
settings-appearance-theme-light = Claro
settings-appearance-theme-system = Sistema

## Palette roles

# The colour roles a palette is made of, named in the Appearance page's
# grid, the panel colour overrides and every colour picker, with the
# groups the grid heads them with. Keyed by the role's saved name.
palette-group-core = Base
palette-group-surfaces = Superficies
palette-group-tints = Tonos
palette-group-ink = Primer plano
palette-role-accent = Acento
palette-role-accent-hover = Acento al pasar
palette-role-highlight = Resalte
palette-role-border = Borde
palette-role-border-light = Borde claro
palette-role-text-bright = Texto brillante
palette-role-text-on-accent = Texto sobre acento
palette-role-bg-root = Fondo
palette-role-bg-panel = Panel
palette-role-bg-elevated = Elevado
palette-role-bg-header = Encabezado
palette-role-bg-menubar = Barra de menús
palette-role-bg-menu = Menú
palette-role-bg-control = Control
palette-role-bg-menu-hover = Menú al pasar
palette-role-bg-control-active = Control activo
palette-role-bg-control-hover = Control al pasar
palette-role-bg-input = Campo de entrada
palette-role-bg-toolbar = Barra de herramientas
palette-role-text = Texto
palette-role-text-secondary = Secundario
palette-role-text-dim = Atenuado
palette-role-text-muted = Apagado
palette-role-text-faint = Tenue
palette-role-gridline = Línea de rejilla

## Settings: application
settings-application-check-updates = Buscar actualizaciones
    .description = Busca una versión más nueva de rox y de lo instalado desde el catálogo una vez al día cuando arranca rox; la ventana Acerca de comprueba al momento de todas formas
settings-application-prerelease-updates = Incluir versiones candidatas
    .description = Ofrecer también las versiones candidatas, los adelantos publicados antes de una versión para probarla; una compilación candidata ve la siguiente de todas formas
settings-application-download-updates = Descargar actualizaciones
    .description = Cuando una comprobación encuentra una versión más nueva, la descarga y la deja lista en segundo plano; el siguiente arranque la ejecuta
settings-application-enable-ai = Activar funciones de IA
    .description = Deja que las herramientas de IA hablen con rox: añade soporte MCP y las descargas de modelos de ML, con sus páginas en la barra lateral.
settings-application-lock-panel-resize = Bloquear el tamaño de los paneles
    .description = Las divisiones de panel solo se redimensionan con el modo de diseño activado, para que un arrastre cerca de una junta no descoloque una disposición terminada
settings-application-menu-entry = Entrada en el menú de aplicaciones
    .description = Una entrada de lanzador y las asociaciones de archivos para este AppImage, siempre apuntando a donde esté el archivo
settings-application-portable-copying = Copiando datos...
settings-application-portable-mode = Modo portátil
    .description = Guarda los ajustes, la biblioteca y las cachés en una carpeta rox-data junto al ejecutable, para que el reproductor viaje con sus datos. Desactivarlo vuelve a la carpeta del sistema y deja rox-data donde está
settings-application-portable-not-writable = No se puede escribir en la carpeta de la aplicación
settings-application-portable-restart-note = Se aplica en el siguiente arranque; esta ejecución sigue con su carpeta actual
settings-application-remain-in-tray = Quedarse en la bandeja
    .description = Mantén la música sonando cuando se cierra la última ventana, con el icono de la bandeja (el dock en macOS) como forma de volver
settings-application-mini-on-top = Reproductor mini siempre encima
    .description = Mantén la ventana sobre las demás mientras está en la disposición mini. Es el mismo fijado que pone ahí el botón de fijar de los controles de ventana
settings-application-section-ai = IA
settings-application-section-control-socket = Socket de control
settings-application-section-data = Datos
settings-application-section-desktop = Escritorio
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
settings-audio-replaygain-status-none = Ninguna de las { $total } pistas escaneadas tiene un ReplayGain con el que nivelar. Medir las que faltan las analiza y guarda los valores
settings-audio-replaygain-status-partial = { $covered } de { $total } pistas escaneadas tienen una ganancia con la que nivelar, { $measured } de ellas medidas por rox. Las otras { $missing } suenan con el ajuste para pistas sin etiqueta
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
settings-integrations-discord-first-line = Primera línea
    .description = La primera línea de la tarjeta, debajo de rox, y lo que la lista de miembros repite por defecto
settings-integrations-discord-hover-line = Tooltip de la portada
    .description = El texto detrás de la portada, al pasar el puntero. Vacío deja la portada sin tooltip
settings-integrations-discord-line-off = Vacía, así que la tarjeta deja esta línea fuera
settings-integrations-discord-line-preview = Vista previa: { $line }
settings-integrations-discord-second-line = Segunda línea
    .description = La segunda línea de la tarjeta. Déjala vacía para quitarla de la tarjeta
settings-integrations-discord-show-lastfm = Mostrar el botón de Last.fm
    .description = Incluye un botón 'Ver en Last.fm' en el estado de Discord
settings-integrations-discord-show-youtube = Mostrar el botón de YouTube
    .description = Incluye un botón 'Buscar en YouTube' en el estado de Discord
settings-integrations-discord-status-line = Línea de estado
    .description = Lo que Discord muestra junto a tu nombre en la lista de miembros
settings-integrations-discord-status-line-app = Nombre de la app
settings-integrations-discord-status-line-first = Primera línea
settings-integrations-discord-status-line-second = Segunda línea
settings-integrations-ffmpeg-binary = Binario de FFmpeg
    .description = Qué ffmpeg hace las conversiones; déjalo vacío para usar una compilación en la carpeta de datos de rox, luego Carpetas de programas y luego el PATH
settings-integrations-ffmpeg-fail-note = Convertir sigue oculto hasta que ffmpeg apunte a un binario que funcione
settings-integrations-ffmpeg-fail-title = Este ffmpeg no se ejecutó
settings-integrations-ffmpeg-missing-note = Convertir sigue oculto; instala ffmpeg o apunta la ruta a un binario
settings-integrations-ffmpeg-missing-note-flatpak = Convertir sigue oculto. Deja una compilación estática de ffmpeg en la carpeta de datos de rox; una instalada en el sistema no puede ejecutarse dentro del sandbox.
settings-integrations-ffmpeg-missing-title = No se encontró un ffmpeg que funcione
settings-integrations-ffmpeg-ok-note = ffmpeg funciona. Convertir está disponible.
settings-integrations-ffmpeg-reveal = Mostrar carpeta de datos
settings-integrations-ffmpeg-test = Probar
settings-integrations-lastfm-api-key-row = Clave de API
settings-integrations-lastfm-hearts = { $n ->
    [one] { $n } corazón
   *[other] { $n } corazones
}
settings-integrations-lastfm-import-loved = Importar pistas favoritas
settings-integrations-lastfm-import-plays = Importar reproducciones
    .description = Rellenar reproducciones e historial de escuchas desde tus scrobbles de Last.fm
settings-integrations-lastfm-import-plays-button = Importar reproducciones
settings-integrations-lastfm-intro-builtin = Conecta tu cuenta de Last.fm: autoriza rox en el navegador y las pistas reproducidas se scrobblean allí
settings-integrations-lastfm-intro-custom = Esta compilación no trae identidad de api, así que el scrobbling necesita tu propia cuenta de api (Last.fm/api/account/create); pega su clave y su secreto compartido, y luego conecta
settings-integrations-lastfm-key-placeholder = Clave de API
settings-integrations-lastfm-love-failed = El último falló: { $error }
settings-integrations-lastfm-love-pending = { $hearts } esperando a enviarse
settings-integrations-lastfm-love-pending-failed = { $hearts } esperando a enviarse, último intento: { $error }
settings-integrations-lastfm-secret-placeholder = Secreto compartido
settings-integrations-lastfm-secret-row = Secreto compartido
settings-integrations-lastfm-status-elsewhere = Conectado en otra instalación de rox; cada una autoriza con su propia identidad de api, así que conecta también esta
settings-integrations-librefm-intro = Conecta tu cuenta de Libre.fm: autoriza rox en el navegador y las pistas reproducidas se scrobblean allí en cuanto cruzan el umbral
settings-integrations-listenbrainz-api-url = URL de la API
settings-integrations-listenbrainz-api-url-placeholder = https://api.listenbrainz.org/1/
settings-integrations-listenbrainz-get-token = Obtener token
settings-integrations-listenbrainz-intro = Pega el token de usuario de listenbrainz.org/settings. Las escuchas se envían en cuanto una pista cruza el umbral de scrobble.
settings-integrations-listenbrainz-token-placeholder = Token
settings-integrations-listenbrainz-token-row = Token de usuario
settings-integrations-love-favourites = Marcar favoritos en Last.fm
    .description = Refleja los corazones en Last.fm como pistas favoritas; quitar un corazón también lo quita allí
settings-integrations-scrobble-connect = Conectar
settings-integrations-scrobble-disconnect = Desconectar
settings-integrations-scrobble-finish-connecting = Terminar de conectar
settings-integrations-scrobble-reconnect = Reconectar
settings-integrations-scrobble-status-checking = Comprobando el token
settings-integrations-scrobble-status-confirming = Confirmando...
settings-integrations-scrobble-status-connected = Conectado como { $username }
settings-integrations-scrobble-status-failed = Falló la conexión: { $error }
settings-integrations-scrobble-status-invalid = { $service } rechazó ese token
settings-integrations-scrobble-status-not-connected = Sin conectar
settings-integrations-scrobble-status-rejected = { $service } rechazó la sesión y se descartó. Vuelve a conectar para seguir scrobbleando
settings-integrations-scrobble-status-requesting = Pidiendo un token...
settings-integrations-scrobble-status-waiting = Autoriza rox en el navegador y luego termina de conectar
settings-integrations-scrobble-threshold = Umbral de scrobble
    .description = Cuánto tiene que sonar una pista antes de scrobblearse, en todos los servicios conectados; la barra de posición y la forma de onda pueden marcarlo
settings-integrations-scrobble-tracks = Scrobblear pistas
    .description = Envía las pistas reproducidas a todos los servicios conectados en cuanto cruzan el umbral
settings-integrations-scrobble-working = Trabajando...
settings-integrations-section-conversion = Conversión
settings-integrations-section-discord = Discord Rich Presence
settings-integrations-section-lastfm = Last.fm
settings-integrations-section-librefm = Libre.fm
settings-integrations-section-listenbrainz = ListenBrainz
settings-integrations-section-scrobbling = Scrobbling
settings-integrations-subsonic-connect = Conectar
settings-integrations-subsonic-credentials = Acceso
    .description = La cuenta en el servidor; la contraseña se guarda en accounts.json, nunca en settings.json
settings-integrations-subsonic-name = Nombre
    .description = Cómo llama rox a este servidor; mientras esté vacío, la dirección ocupa su lugar
settings-integrations-subsonic-name-placeholder = Casa
settings-integrations-subsonic-new = Servidor nuevo
settings-integrations-subsonic-password-placeholder = Contraseña
settings-integrations-subsonic-remove = Quitar servidor
    .description = Olvida el acceso a este servidor y borra sus pistas de la biblioteca; desactivarlo las conserva, ocultas
settings-integrations-subsonic-server = Servidor
    .description = La dirección del servidor con http:// o https://, y sin /rest al final
settings-integrations-subsonic-status-failed = No se pudo contactar con el servidor: { $error }
settings-integrations-subsonic-status-ok = Conectado a { $server }
settings-integrations-subsonic-sync-count = { $n ->
    [one] { $n } pista, última sincronización el { $date }
   *[other] { $n } pistas, última sincronización el { $date }
}
settings-integrations-subsonic-sync-failed = Falló la sincronización: { $error }
settings-integrations-subsonic-sync-never = Sin sincronizar todavía
settings-integrations-subsonic-sync-now = Sincronizar ahora
    .description = Pregunta al servidor qué tiene y ajusta la biblioteca a eso; las pistas que ya no aparecen se quitan
settings-integrations-subsonic-syncing = Sincronizando el álbum { $done } de { $total }
settings-integrations-subsonic-url-placeholder = https://musica.example.com
settings-integrations-subsonic-use = Usar este servidor
    .description = Lee su biblioteca y reproduce desde ahí; desactivado, sus pistas se quedan en la biblioteca, ocultas
settings-integrations-subsonic-user-placeholder = Usuario

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
settings-library-add-folder = Carpeta...
settings-library-add-subsonic = Servidor Subsonic...
settings-library-col-source = Fuente
settings-library-duplicates = Duplicados...
settings-library-embed-button = Incrustar metadatos guardados...
settings-library-exclude-col = Excluido
settings-library-exclude-intro = Archivos y carpetas que la biblioteca deja fuera de todas las fuentes de arriba. Un nombre como *.tmp coincide en cualquier sitio, y una ruta como Live/** empieza en lo alto de cada carpeta. Da igual mayúsculas o minúsculas, y una carpeta que coincide se salta entera. Los archivos en sí se quedan como están
settings-library-exclude-invalid = No se puede leer ese patrón: { $reason }
settings-library-exclude-none = Nada excluido
settings-library-exclude-nudge = Patrones cambiados: la vigilancia lo sigue al momento. Las pistas que ya están en la biblioteca se quedan hasta que pulses Volver a escanear arriba, en la cabecera de Fuentes
settings-library-exclude-placeholder = *.tmp o Live/**
settings-library-folder-col-albums = Álbumes
settings-library-folder-col-size = Tamaño
settings-library-folder-col-tracks = Pistas
settings-library-folders-intro = Carpetas y servidores de los que se lee la biblioteca; quitar uno saca sus pistas del catálogo y deja en paz los archivos de una carpeta
settings-library-genre-separator-nudge = Separadores cambiados: la navegación lo sigue al momento. Las listas de géneros guardadas por escaneos anteriores conservan su forma antigua hasta que pulses Volver a escanear arriba, en la cabecera de Fuentes
settings-library-merge-case = Unificar variantes de mayúsculas
    .description = Trata como uno solo los valores que solo se diferencian en mayúsculas: Rock y rock pasan a ser el mismo género, artista y álbum, mostrados con la grafía que llevan más pistas. Los archivos conservan sus etiquetas tal como están escritas
settings-library-no-sources = Todavía no hay fuentes
settings-library-portal-hint = Sustituye /path/to por la ubicación real de la carpeta.
settings-library-portal-note = Funciona, pero es lento para una biblioteca y rox no puede vigilar sus cambios. Dale a rox la ubicación real de la carpeta, luego quítala y vuelve a añadirla:
settings-library-portal-title = rox llega a esta carpeta a través del portal de archivos
settings-library-repair-tags = Reparar etiquetas...
settings-library-section-sources = Fuentes
settings-library-section-stored-metadata = Metadatos guardados
settings-library-section-tempo = Análisis de tempo
settings-library-source-off = Desactivado
settings-library-source-on = Activado
settings-library-source-synced = Sincronizado el { $date }
settings-library-split-genres = Separar géneros en comas y barras
    .description = "Dubstep, Trap" y "Drum & Bass / Neurofunk" cuentan cada valor como un género propio; el punto y coma siempre separa. Desactivado deja enteros los nombres con barra, para las etiquetas donde significan un solo género. Los archivos conservan sus etiquetas tal como están escritas
settings-library-subsonic-add-note = Funciona con Navidrome, Airsonic, gonic y cualquier otro servidor que hable la API de Subsonic
settings-library-subsonic-add-title = Añadir un servidor Subsonic
settings-library-tempo-auto = Contar archivos nuevos
    .description = Cuenta los tiempos de lo que trae el vigilante según llega, una vez que la sincronización se ha asentado, para que una biblioteca que crece mantenga sus tempos sin volver aquí. Desactivado, los archivos nuevos esperan al botón Analizar lo que falta. Al activarlo se ofrece medir primero lo que ya falta; después de eso solo ve archivos recién llegados
settings-library-tempo-embed = Escribir en los archivos...
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
settings-library-unreachable-tip = rox no llega a esta carpeta. Sus pistas siguen en la biblioteca, pero no sonarán hasta que vuelva.
settings-library-watch-folders = Vigilar carpetas
    .description = Incorpora a la biblioteca los archivos añadidos, editados y borrados según ocurre, sin volver a escanear a mano
settings-library-write-stored = Escribir lo guardado en los archivos
    .description = Los tres ajustes de guardado solo se aplican a la siguiente escritura, así que todo lo guardado antes de pasar uno a Etiquetas sigue estando solo en rox. Esto escribe las letras, las ganancias, las descripciones y los tempos que rox ya tiene en los propios archivos, para que otro reproductor que lea la carpeta las vea. No se recalcula nada
settings-show-readings = Mostrar lecturas
    .description = Poner la lectura romanizada después de un nombre escrito en una grafía que este alfabeto no sabe pronunciar: 秋ノ風 (Aki no kaze). La lectura es el nombre de ordenación que el valor ya lleva, así que un nombre sin ella no muestra nada y un nombre latino nunca la recibe

## Settings: MCP
settings-mcp-client-config = Configuración del cliente
    .description = Pégalo en la lista de servidores de un cliente MCP (Claude Code, Claude Desktop o cualquier otro) para que pueda preguntarle a rox por la biblioteca, por lo que suena y por el transporte. rox tiene que estar en marcha; las herramientas van por su socket de control
settings-mcp-enable = Activar el servidor MCP
    .description = Responde a las llamadas de herramientas de los clientes MCP conectados. El proxy lo comprueba en cada llamada, así que mientras esté apagado los clientes reciben el rechazo con su motivo; la configuración de abajo se puede preparar igualmente
settings-mcp-plugins = Dejar que los clientes MCP usen plugins
    .description = Permite a los clientes conectados explorar y buscar en los plugins y ejecutar las acciones que declaran. Lo que responde un plugin llega al modelo tal cual, y una acción actúa sobre el servicio del plugin igual que si se eligiera en un menú. Cada acción que ejecuta un cliente muestra un aviso

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
settings-playback-live-buffer = Búfer del directo
    .description = Cuánto se puede rebobinar una emisora. La conexión sigue abierta mientras la emisora está en pausa y sigue llenando el búfer, así que al reanudar continúa donde lo dejaste en vez de en el directo. Doce horas son el infinito práctico: nada que dejes puesto de una sentada llega a darle la vuelta.
settings-playback-live-buffer-memory = Unos { $low } a 128 kbps, { $high } a 320 kbps.
settings-playback-live-buffer-playing = Emisora en reproducción: unos { $size }.
settings-playback-live-buffer-cap = El búfer se detiene en { $cap } por mucho que se alargue: una décima parte de la memoria de este equipo.
settings-playback-capture-album = Álbum de las capturas
    .description = Lo que dice la etiqueta de álbum de una canción guardada. En blanco no escribe álbum, porque una canción de la radio no tiene lanzamiento y los nombres de emisora en el campo de álbum ensucian todas las vistas por álbum. %station% pone ahí la emisora; una palabra como Radio o Singles las archiva en un estante propio. La emisora y la URL del flujo siempre van en el comentario.
settings-playback-capture-buffer-title = El búfer del directo es más corto que la mayoría de las canciones
settings-playback-capture-buffer-note = Está en { $buffer }, y una canción solo se guarda si al terminar sigue entera en el búfer. { $default } cubre casi todo lo que pone una emisora.
settings-playback-capture-album-placeholder = Dejar en blanco
settings-playback-capture-choose = Elegir carpeta...
settings-playback-capture-enable = Guardar canciones de fuentes en streaming
    .description = Escribe en la carpeta de capturas cada canción que un stream reproduce de principio a fin, tal como llega, etiquetada con la emisora y con su portada al lado cuando se encuentra alguna
settings-playback-capture-caveat = Los límites entre canciones vienen de los títulos que anuncia la emisora, y esos van unos segundos desfasados del audio, así que una captura puede llevar el final de la canción anterior. Una canción más larga que el búfer en directo nunca se guarda, así que un pódcast o una mezcla se quedan en el aire.
settings-playback-capture-folder = Carpeta de capturas
settings-playback-capture-library = Añadir a la biblioteca
    .description = Convierte la carpeta de capturas en una fuente de la biblioteca, para que las canciones guardadas aparezcan con el resto. Desactivado las quita de la biblioteca, no del disco.
settings-playback-capture-library-held = Ya está en la biblioteca a través de { $source }
settings-playback-capture-pattern = Nombres de las capturas
    .description = Cómo se llama una canción guardada dentro de la carpeta de capturas. Una / crea una carpeta, así que la opción predeterminada archiva una noche de radio por emisora en vez de amontonarlo todo. La extensión viene del stream.
settings-playback-capture-pattern-date = %date% es el día en que se guardó la canción, como 2026-09-18.
settings-playback-capture-pattern-preview = Vista previa: { $name }
settings-playback-capture-pattern-station = %station% es el nombre de la emisora, y %album% dice lo mismo.
settings-playback-capture-pattern-source = %source% es el tipo de fuente de la que viene la transmisión, así que un tipo nuevo se guarda en su propia carpeta.
settings-playback-section-capture = Captura
settings-playback-play-order = Orden de reproducción
    .description = Cómo se ordenan las pistas ya encoladas mientras el modo aleatorio está activo. El botón de aleatorio del transporte lo activa y lo desactiva; esto es lo que hace una vez activo
settings-playback-rating-scale = Escala de valoración
    .description = Estrellas para clics rápidos, 0-10 en medios pasos para notas de reseña más finas
settings-playback-rating-scale-numeric = 0-10
settings-playback-rating-scale-stars = Estrellas
settings-playback-restore-last-session = Restaurar la última sesión
    .description = Arranca con la cola tal como la dejaste, en pausa sobre la pista que sonaba y por donde se quedó. Las pistas encoladas fuera de tus carpetas de biblioteca no se pueden restaurar y caen del orden
settings-playback-section-queue = Cola
settings-playback-section-streaming = Streaming
settings-playback-section-ratings = Valoraciones
settings-playback-section-stepping = Avance por pasos
settings-playback-step = Tamaño del paso
    .description = Cuánto mueve la cabeza de reproducción cada pulsación de una tecla de paso
settings-playback-step-preview = Vista previa del paso
    .description = Cuánto suena un paso en pausa, para que oigas dónde cayó
settings-playback-section-startup = Arranque
settings-playback-shuffle-random = Al azar
    .description = El aleatorio que todo el mundo quiere decir con la palabra. Lo que viene suena sin ningún orden concreto
settings-playback-shuffle-similar = Parecido
    .description = Lo más cercano primero por sonido. Lo que viene se ordena por cuánto se parece a la pista que sonaba cuando lo activaste, y se reordena en cada salto. Las pistas de un plugin siguen llegando de su radio. Necesita la biblioteca descrita en la página Biblioteca, o un plugin con radio
settings-playback-shuffle-albums = Álbumes
    .description = Álbumes sin ningún orden concreto, cada uno entero y en el orden de sus pistas. El álbum que suena termina primero, y una cola que se queda vacía se rellena de álbum en álbum
settings-playback-unrated-dots = Puntos sin valorar
    .description = Marca los huecos de estrella sin rellenar con un punto tenue en vez de dejarlos vacíos

## Settings: plugins
settings-plugins-enable = Activar plugins
    .description = Deja que se ejecuten los plugins de la carpeta de plugins. Cada uno sigue teniendo su propio interruptor debajo y se ejecuta como programa en este equipo con tus permisos
settings-plugins-intro = Un plugin es un archivo .roxp en la carpeta de plugins, o una carpeta mientras lo escribes. Nada de él se ejecuta hasta que lo actives, y uno que cambia en el disco se desactiva hasta que lo vuelvas a activar
settings-plugins-install = Instalar
settings-plugins-reveal = Mostrar carpeta
settings-plugins-guide = Guía de plugins
settings-plugins-empty = Aún no hay plugins. Instala uno o deja un .roxp en la carpeta de plugins
settings-plugins-installed = { $name } instalado. Sigue desactivado hasta que lo actives
settings-plugins-standing-missing = No está
settings-plugins-standing-failed = Falló
settings-plugins-standing-changed = Cambiado en el disco
settings-plugins-standing-stopped = Detenido tras fallos repetidos
settings-plugins-standing-needs = Necesita { $programs }
settings-plugins-failed-title = Este plugin no puede ejecutarse
settings-plugins-sync-failed = La última sincronización falló
settings-plugins-scrobble = Hacer scrobble
    .description = Envía lo que reproduce este plugin a tus servicios de scrobbling. Se ofrece porque el plugin lo pide
settings-plugins-favourites = Sincronizar favoritos
    .description = Un corazón en una pista de un plugin también la marca como favorita en el servicio del plugin, si el plugin lo ofrece. Los corazones de antes de activarlo se quedan como están y aparecen medio llenos hasta que haces clic en ellos
settings-plugins-lookup = Buscar canciones con
    .description = El plugin que reproduce las canciones que tu biblioteca solo conoce por su nombre, como las escuchas importadas de Last.fm. Elegir uno en Reproducir desde también lo cambia
settings-plugins-lookup-ask = Preguntar
settings-plugins-lyrics = Letras
settings-plugins-lyrics-own = Pide a este plugin las letras de sus pistas antes que a los proveedores de letras. Se ofrece porque el plugin las proporciona
settings-plugins-lyrics-library = Incluye las letras de este plugin cuando rox busca las de cualquier pista. Se ofrece porque el plugin las proporciona
settings-plugins-lyrics-both = Pide primero a este plugin las letras de sus propias pistas e incluye las suyas cuando rox busca las de cualquier otra. Se ofrece porque el plugin las proporciona
settings-plugins-metadata = Búsqueda de etiquetas
    .description = Incluye las coincidencias de este plugin cuando consultas las etiquetas de una pista. Se ofrece porque el plugin las proporciona
settings-plugins-artist = Información del artista
    .description = Pregunta primero a este plugin por un artista y deja que los proveedores de artistas completen lo que falte. Se ofrece porque el plugin la proporciona
settings-plugins-domain-lyrics = letras
settings-plugins-domain-metadata = búsqueda de etiquetas
settings-plugins-domain-artist = información de artistas
settings-plugins-developer = Modo desarrollador: hasta que cierres rox, un cambio en la carpeta de este plugin se aprueba solo y lo reinicia. Un cambio en su manifiesto lo sigue desactivando
settings-plugins-pack = Empaquetar en un .roxp para compartirlo
settings-plugins-packed = Empaquetado en { $path }
settings-plugins-install-failed = No se pudo instalar { $file }
settings-plugins-pack-failed = No se pudo empaquetar { $name }
settings-plugins-synced = Colecciones sincronizadas
settings-plugins-synced-none = Aún no hay nada sincronizado. Activa la sincronización de una colección en el explorador de la fuente del plugin
settings-plugins-sync-now = Sincronizar ahora
settings-plugins-library = En la biblioteca
settings-plugins-library-all = Todas sus pistas
settings-plugins-library-saved = Añadidas una a una
settings-plugins-show-in-library = Mostrar en la biblioteca
settings-plugins-card-title = ¿Activar "{ $name }"?
settings-plugins-card-runs = Se ejecuta como programa en este equipo con tus permisos. rox no lo aísla.
settings-plugins-card-source = Añade { $label } como fuente: rox la explora, busca, sincroniza y reproduce a través del plugin.
settings-plugins-card-scrobbles = Pide hacer scrobble de lo que reproduce.
settings-plugins-card-favourites = Puede marcar sus pistas como favoritas en su servicio cuando activas Sincronizar favoritos.
settings-plugins-card-lyrics = Ofrece letras para sus pistas. rox solo las pide cuando las activas.
settings-plugins-card-providers = Ofrece { $domains } para cualquier pista de tu biblioteca. rox solo pregunta cuando activas cada una.
settings-plugins-card-actions = Añade a los menús de rox: { $actions }
settings-plugins-card-program-found = Usa { $program }, que está en este equipo.
settings-plugins-card-program-missing = Usa { $program }, que no está en el PATH de este equipo.
settings-plugins-card-changed = Cambios desde la última vez que lo activaste:
settings-plugins-card-unchanged = El manifiesto dice lo mismo que la última vez. Cambiaron archivos del plugin.
settings-plugins-card-manifest-changed = El manifiesto cambió desde la última vez que lo activaste.
settings-plugins-change-capability-added = Capacidad nueva: { $name }
settings-plugins-change-capability-removed = Capacidad retirada: { $name }
settings-plugins-change-program = Programa nuevo: { $program }
settings-plugins-change-scrobble-on = Ahora pide hacer scrobble
settings-plugins-change-scrobble-off = Ya no pide hacer scrobble
settings-plugins-change-entry = Se inicia de otra forma
settings-plugins-change-action-added = Acción nueva: { $label }
settings-plugins-change-action-changed = Acción cambiada: { $label }
settings-plugins-change-provider = Ahora ofrece { $domain } para tu biblioteca
settings-plugins-switch-on = Activar
settings-plugins-remove-title = ¿Quitar "{ $name }"?
settings-plugins-remove-body = Se van sus pistas, colecciones sincronizadas y ajustes. Su carpeta se queda en la carpeta de plugins; bórrala allí para que deje de aparecer aquí.
settings-plugins-remove-body-missing = Se van sus pistas, colecciones sincronizadas y ajustes. Ya no está en la carpeta de plugins, así que no volverá a aparecer aquí.
settings-plugins-remove-package-body = Se van sus pistas, colecciones sincronizadas y ajustes, y se borra su archivo .roxp. Los archivos que guardó en su carpeta de datos se quedan.
settings-plugins-renderers-intro = Un renderizador es un programa que dibuja una imagen en rox, en un panel Renderizador o en un destino de render de un lienzo. Nada de él se ejecuta hasta que lo activas aquí, y entonces solo mientras algo lo muestra. Un renderizador es un .roxp en la carpeta de paquetes, o una carpeta mientras lo escribes
settings-plugins-renderers-empty = Aún no hay renderizadores. Instala un .roxp o deja la carpeta de un renderizador en la carpeta de paquetes
settings-plugins-renderers-guide = Guía de renderizadores
settings-plugins-card-renderer = Dibuja { $label } en un panel Renderizador o en un destino de render de un lienzo.
settings-plugins-card-renderer-signals = Recibe las señales de la música y la posición de reproducción.
settings-plugins-card-renderer-pointer = Recibe el puntero mientras está sobre el destino de render.
settings-plugins-card-renderer-wanted = Sigue estas señales: { $signals }. Cuando colocas un destino de render para él, rox ofrece añadir las que aún no tienes.
settings-plugins-card-renderer-palette = Recibe los colores del tema actual, que siguen a la portada cuando el color de la canción está activado.
settings-plugins-card-renderer-cover = Recibe la portada de la pista que suena, como imagen.
settings-plugins-card-renderer-spectrum = Recibe el espectro de la música: lo fuerte que suena cada frecuencia mientras se reproduce, nunca el sonido en sí.
settings-plugins-card-unchanged-renderer = El manifiesto dice lo mismo que la última vez. Cambiaron archivos del renderizador.
settings-plugins-renderer-failed-title = Este renderizador no puede ejecutarse
settings-plugins-renderer-developer = Modo desarrollador: hasta que cierres rox, un cambio en la carpeta de este renderizador se aprueba solo. Un cambio en su manifiesto lo sigue desactivando
settings-plugins-remove-renderer-body = Se van sus ajustes, y los paneles y destinos de render que lo usan no dibujan nada. Su carpeta se queda en la carpeta de paquetes; bórrala allí para que deje de aparecer aquí.
settings-plugins-remove-renderer-package-body = Se van sus ajustes y su archivo .roxp, y los paneles y destinos de render que lo usan no dibujan nada.

## Settings: providers
settings-providers-acoustid = AcoustID
    .description = Tomar una huella del audio y preguntar a acoustid.org de qué grabación se trata; el botón de identificar de la comparación de metadatos encuentra la edición de un archivo con etiquetas demasiado erróneas para buscarlas
settings-providers-acoustid-key = Clave de AcoustID
settings-providers-acoustid-key-hint = Opcional; esta compilación trae su propia clave
settings-providers-acoustid-key-placeholder = Clave de aplicación
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

## Settings: sources

settings-sources-section-stations = Emisoras de radio
settings-sources-stations-find = Buscar emisoras...
settings-sources-stations-import = Importar emisoras...
settings-sources-stations-intro = Pega la URL de un stream y pulsa Intro o Añadir. Una emisora sin nombre muestra su URL
settings-sources-stations-name-placeholder = Nombre (opcional)
settings-sources-stations-none = Aún no hay emisoras
settings-sources-stations-url-placeholder = URL del flujo

## Settings: storage
settings-storage-artist-images = Imágenes de artistas
    .description = Retratos, banners y biografías descargados para las vistas de artista (artists/); los que borres se vuelven a descargar la próxima vez que se abra una vista
settings-storage-catalog = Catálogo
    .description = El índice de pistas que construyen los escaneos: una fila por pista con sus etiquetas, los datos de su archivo y cualquier tramo de cue, dentro de library.db
settings-storage-cover-thumbnails = Miniaturas de carátula
    .description = Carátulas pequeñas guardadas tras su primer dibujado (thumbs.db); las que borres se rehacen según entran en pantalla
settings-storage-listening-history = Historial de escucha
    .description = Cada reproducción de la que la biblioteca guarda constancia, las suyas y todo lo que haya traído una importación de Last.fm. De ahí salen la columna de reproducciones, la ventana de estadísticas, el panel de historial y cualquier lista inteligente que ordene por reproducciones
settings-storage-logs = Registros
    .description = Lo que escribe cada ejecución para informes de fallos (logs/rox.log), rotado con un límite de tamaño para que nunca crezca mucho
settings-storage-looks-layouts = Aspectos y disposiciones
    .description = El aspecto que está usando la aplicación (workspace.json) con tus espacios de trabajo guardados y los archivos de shader volcados al lado. Pequeño, y cada byte es algo que configuraste tú
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
settings-workspace-apply-failed = No se pudo aplicar { $name }
settings-workspace-assets-add-failed = No se pudo añadir { $file }
settings-workspace-assets-adding = Añadiendo...
settings-workspace-assets-font-too-large = Esta fuente pasa de { $size } MB, lo máximo que carga rox
settings-workspace-assets-hint = Archivos que el aspecto lleva consigo cuando guardas o exportas el espacio de trabajo. Los shaders leen sus imágenes de aquí, y una fuente añadida aquí aparece en la lista de fuentes de la página Apariencia
settings-workspace-assets-kind-font = Fuentes
settings-workspace-assets-kind-image = Imágenes
settings-workspace-assets-reveal = Mostrar carpeta
settings-workspace-assets-too-many-fonts = Este espacio de trabajo ya lleva tantas fuentes como carga rox
settings-workspace-assets-unknown-kind = Un espacio de trabajo lleva imágenes y fuentes (TTF, OTF)
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
composition-window-title = Composición
composition-convert-title = Convertir en lienzo
composition-convert-body = Pone toda la disposición de esta ventana en una sola cuadrícula sobre un lienzo, donde se puede mover, redimensionar, inclinar y apilar entre imágenes, texto, shaders y destinos de render. Los paneles se ven igual hasta que empiezas a ordenarlos.
composition-convert-undo = No se puede deshacer. Para conservar esta disposición tal como está, guárdala antes: Espacio de trabajo y luego Guardar disposición.
composition-convert-button = Convertir...
composition-convert-done = Esta ventana ya es un lienzo, y sus elementos se ordenan en el modo de diseño.
composition-convert-empty = Esta ventana aún no tiene paneles, así que no hay nada que convertir.
settings-workspace-empty = Todavía no hay espacios de trabajo
settings-workspace-global-presets-empty = Todavía no hay preajustes globales
settings-workspace-global-presets-hint = Preajustes de panel guardados como globales. Todos los espacios de trabajo los muestran, y sus shaders viajan dentro de ellos
settings-workspace-heavy = Ocupa { $size } MB al desempaquetarse, más de lo que suele llevar un aspecto
settings-workspace-hint = Un espacio de trabajo es un aspecto entero: disposiciones, paleta, apariencia. Aplicar uno reemplaza los tres
settings-workspace-import-failed = No se pudo importar { $file }
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
settings-workspace-section-assets = Archivos
settings-workspace-section-composition = Composición
settings-workspace-section-global-presets = Preajustes globales
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
settings-common-add = Añadir
settings-common-analyze-missing = Analizar lo que falta
settings-common-built-in = Integrado
settings-common-cancel = Cancelar
settings-common-clear = Borrar
settings-common-copy = Copiar
settings-common-database = Base de datos
settings-common-delete = Eliminar
settings-common-done = Listo
settings-common-download = Descargar
settings-common-program-folders = Carpetas de programas
    .description = Carpetas donde se buscan programas antes que en el PATH, separadas como en el PATH. Añade aquí la carpeta de Homebrew si los programas que instalaste con él aparecen como ausentes
settings-common-remove = Quitar
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
settings-confirm-remove-subsonic-body = Se olvida el acceso y sus pistas salen de la biblioteca. Tenerlas de nuevo significa conectar y sincronizar otra vez; desactivarlo, en cambio, las conserva.
settings-confirm-remove-subsonic-title = ¿Quitar "{ $name }"?
settings-library-remove-folder-body = La carpeta y sus pistas salen de la biblioteca, y los archivos se quedan en el disco.
settings-confirm-delete-workspace-body = Esto elimina el espacio de trabajo guardado y deja el aspecto actual como está.
settings-confirm-delete-layout-body = Esto elimina la disposición guardada y deja cada ventana como está.
settings-confirm-delete-panel-preset-body = Esto elimina el preajuste guardado y deja cada panel como está.
settings-confirm-delete-asset-body = El archivo sale del espacio de trabajo, y todo lo que lo usa lo pierde.
settings-dictionary-delete-body = Esto elimina la descarga de { $size } y conserva lo que ya romanizó.
settings-mlmodels-delete-body = Esto elimina la descarga de { $size } y conserva los vectores que ya calculó.
settings-sidebar-data-folder = Carpeta de datos
settings-sidebar-settings-file = Archivo de ajustes

## Menubar
menu-about = Acerca de
menu-analyze-tempo = Analizar el tempo...
menu-application = Aplicación
menu-apply-layout = Aplicar disposición a esta ventana
menu-apply-workspace = Aplicar espacio de trabajo
menu-build-acoustic = Crear vectores acústicos...
menu-chat = Chat
menu-close = Cerrar
menu-composition = Composición
menu-console = Consola
menu-design-mode = Modo de diseño
menu-discussions = Debates
menu-empty-window = Ventana vacía
menu-equalizer = Ecualizador
menu-exit = Salir
menu-fill-sort-names = Rellenar los nombres de ordenación...
menu-romanize-library = Romanizar la biblioteca...
menu-find-duplicates = Buscar duplicados...
menu-go-to-time = Ir a un tiempo...
menu-tag-genres = Etiquetar géneros...
menu-health = Salud de la biblioteca
menu-power-search = Búsqueda avanzada
menu-quick-play = Reproducción rápida
menu-hide-menubar = Ocultar la barra de menús
menu-show-menubar = Mostrar la barra de menús
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
menu-section-analyze = Analizar
menu-section-app = App
menu-section-interface = Interfaz
menu-section-layouts = Disposiciones
menu-section-listening = Escucha
menu-section-maintain = Mantener
menu-section-session = Sesión
menu-section-track = Pista
menu-section-sleep = Temporizador
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

# The macOS menu bar has no checkmarks, so a toggle row names what picking
# it does to the setting it carries.
native-menu-show = Mostrar { $what }
native-menu-hide = Ocultar { $what }
native-menu-turn-on = Activar { $what }
native-menu-turn-off = Desactivar { $what }

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
workspace-requires-missing = { $count ->
    [one] Usa un plugin que no está en marcha: { $plugins }
   *[other] Usa { $count } plugins que no están en marcha: { $plugins }
}
workspace-requires-missing-renderers = { $count ->
    [one] Usa un renderizador que no está instalado o activado: { $renderers }
   *[other] Usa { $count } renderizadores que no están instalados o activados: { $renderers }
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
workspace-dialog-convert = Convertir
workspace-dialog-convert-title = ¿Convertir esta disposición en un lienzo?
workspace-dialog-convert-body = Los paneles pasan a una sola cuadrícula sobre un lienzo y se ven igual hasta que los ordenas. No se puede deshacer, así que guarda antes la disposición si quieres recuperarla.
workspace-convert-already = Esta ventana ya es un lienzo.
workspace-convert-empty = No hay nada que convertir: esta ventana aún no tiene paneles.
workspace-dialog-export = Exportar
workspace-dialog-layout-name-placeholder = Nombre de la disposición
workspace-dialog-not-now = Ahora no
workspace-dialog-overwrite = Sobrescribir
workspace-dialog-overwrite-title = ¿Sobrescribir "{ $name }"?
workspace-dialog-delete-title = ¿Eliminar "{ $name }"?
workspace-dialog-remove-title = ¿Quitar "{ $name }"?
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
workspace-trash-title = { $count ->
    [one] ¿Mover 1 archivo a la papelera?
   *[other] ¿Mover { $count } archivos a la papelera?
}
workspace-trash-body = Los archivos de la papelera salen de la biblioteca y de sus carpetas, y se quedan en ella hasta que se vacíe.
workspace-dialog-trash = Mover a la papelera
workspace-trash-done = { $count ->
    [one] 1 archivo movido a la papelera
   *[other] { $count } archivos movidos a la papelera
}
workspace-trash-failed = { $failed } de { $count } no se pudieron mover a la papelera
workspace-trash-not-files = Solo las pistas que son un archivo propio pueden ir a la papelera
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
tasks-plugin-job = { $action } { $progress }
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
tasks-job-plays-import = Recuentos de reproducciones de Last.fm
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
tasks-nothing-to-analyze = Todavía no hay nada escaneado que analizar
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
stats-empty-repeats = Ninguna canción se escuchó más de una vez en este rango
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
stats-section-top-tracks = Pistas más escuchadas
stats-show-change = Mostrar el cambio
    .description = Añade una ficha con cómo se compara el periodo con el anterior, arriba o abajo; Desde siempre no tiene nada detrás
stats-show-number = Mostrar el número
    .description = Dibuja la cantidad junto al icono; desactivado deja un icono a secas con las cantidades al pasar el cursor
stats-title = Widget de estadísticas
stats-tooltip-listens = Escuchas
stats-unknown-row = No está en tu biblioteca; reprodúcela desde el panel Historial
stats-window-title = rox - Estadísticas

## Borrar el historial de escucha: la confirmación que levantan la
## ventana de estadísticas y la página de almacenamiento.

listens-clear-body = { $imported } de { $total } escuchas vienen de una importación de Last.fm: con fecha donde la cuenta sabía cuándo, repartidas por los años donde solo quedaba un recuento. Borrar solo esas mantiene las escuchas que rox vio ocurrir; borrarlo todo deja la biblioteca sin ningún historial.
listens-clear-body-plain = { $listens }, ninguna de una importación: rox las vio ocurrir todas. Borrar deja la biblioteca sin ningún historial.
listens-clear-button = Borrar historial
listens-clear-everything = Borrarlo todo
listens-clear-imported = Borrar importadas
listens-clear-title = ¿Borrar el historial de escucha?
listens-count = { $count ->
    [one] { $count } escucha
   *[other] { $count } escuchas
}

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
health-desc-writable = Pistas en formatos que rox puede leer pero en los que no puede escribir etiquetas. Una variante rara de MP4 fragmentado también rechaza la escritura y no se cuenta aquí.
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
about-update-major-note = Esta es una actualización importante. Convierte tus espacios de trabajo guardados a un formato nuevo que las versiones anteriores de rox no pueden leer.
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
welcome-design-mode-note = Reorganizar necesita el modo de diseño: el botón del extremo izquierdo de la barra de menús, activado por defecto, que además muestra el menú Paneles. Desactivado bloquea la disposición, así una configuración terminada no se descoloca.
welcome-done = Listo
welcome-drop-note = Suéltalo en el borde de un panel para dividir ahí, en el medio para compartir un grupo de pestañas, o fuera de la ventana para que sea su propia ventana.
welcome-key-left-click = Clic izquierdo
welcome-key-middle-mouse = Botón central
welcome-layout-note = Guarda una organización como disposición; un espacio de trabajo junta disposiciones y paleta en un aspecto que se puede compartir.
welcome-menu-add = Añadir al menú de aplicaciones
welcome-menu-failed = No se pudo añadir la entrada del menú
welcome-menu-not-now = Ahora no
welcome-menu-note = Un AppImage no se registra solo. Esto añade una entrada de lanzador y las asociaciones de archivos de audio; nada fuera de tu carpeta personal cambia.
welcome-menu-title = ¿Añadir rox al menú de aplicaciones?
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
signals-blurb-total = Un Total funciona de otra manera: suma otra señal a lo largo del tiempo y da la vuelta en 1, así que sube mientras la música está fuerte y se estanca mientras no lo está. Úsalo cuando un shader necesite una fase que avance con la canción y no con el reloj.
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
eq-autoeq = AutoEq
autoeq-window-title = AutoEq
autoeq-placeholder = Buscar auriculares o IEMs (ej. HD 600, AirPods, Zero)...
autoeq-search = Buscar en AutoEq
autoeq-searching = Buscando en AutoEq...
autoeq-loading = Cargando base de datos de AutoEq...
autoeq-none = Ningún perfil de auriculares coincide con "{ $text }"
autoeq-failed = Error de AutoEq: { $reason }
autoeq-apply = Aplicar
autoeq-applied = Aplicado
autoeq-import-file = Importar archivo de ajustes
autoeq-save = Guardar
autoeq-saved = Guardado
autoeq-saved-info = Guardado como "{ $name }"
autoeq-save-failed = No se pudo escribir el archivo de preajuste
autoeq-applied-info = Aplicado: { $name } ({ $source })
autoeq-applied-info-preamp = Aplicado: { $name } ({ $source }), preamplificación { $db } dB
autoeq-imported-info = Importado: { $name }
autoeq-imported-info-preamp = Importado: { $name }, preamplificación { $db } dB
autoeq-read-failed = No se pudo abrir el archivo: { $reason }
autoeq-parse-failed = No se pudo leer el perfil: { $reason }
autoeq-import-untitled = Personalizado
eq-convolver-spatial = Espacial
eq-convolver-profile-label = Perfil
eq-convolver-no-file = Sin IR cargado (Crossfeed integrado)
eq-convolver-load = Cargar IR...
eq-convolver-clear = Borrar
eq-convolver-mode-label = Modo
eq-convolver-mode-virtual-stereo = Estéreo virtual
eq-convolver-mode-surround-71 = Envolvente 7.1
eq-convolver-wet-label = Mezcla procesada
eq-convolver-width-label = Amplitud
eq-convolver-crossfeed-label = Crossfeed
eq-convolver-gain-label = Ganancia
eq-convolver-speakers-title = Niveles de altavoces
eq-convolver-reset = Restablecer
eq-convolver-fl = Frontal I
eq-convolver-fr = Frontal D
eq-convolver-fc = Central
eq-convolver-sl = Lateral I
eq-convolver-sr = Lateral D
eq-convolver-bl = Trasero I
eq-convolver-br = Trasero D
eq-convolver-layout-hesuvi = HRIR HeSuVi de 14 canales
eq-convolver-layout-true-stereo = True Stereo (4 canales)
eq-convolver-layout-stereo = Estéreo (2 canales)
eq-convolver-layout-mono = Mono (1 canal)
eq-convolver-layout-generic = IR multicanal
eq-convolver-error-load = No se pudo cargar la respuesta al impulso: { $reason }
eq-flatten = Aplanar
eq-freq-label = Frec
eq-gain-label = Ganancia
eq-heading = Ecualizador
eq-help-text = Arrastra una banda para moverla, desplaza la rueda sobre ella para ensancharla o estrecharla. El procesado va por delante del búfer que alimenta la tarjeta de sonido, así que un movimiento tarda unos 120 ms en llegar a los altavoces mientras esta ventana está abierta.
eq-hint-off = Haz clic para desactivarlo
eq-hint-on = Haz clic para activarlo
eq-hint-open = Haz clic para abrir el ecualizador
eq-open = Abrir el ecualizador
eq-preset-export = Exportar
eq-preset-export-default = Ecualizador
eq-preset-name = Nombre del preajuste
eq-preset-save = Guardar preajuste
eq-presets = Preajustes
eq-preset-delete-body = Esto elimina el preajuste y deja la curva actual como está.
eq-preset-overwrite-body = Esto reemplaza el preajuste guardado por la curva que suena ahora.
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
eq-spatial-on = Espacial activado
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
keymap-go-to-time = Ir a un tiempo
    .description = Escribir una marca de tiempo y saltar a ella
keymap-step-backward = Paso atrás
    .description = Retroceder la cabeza de reproducción el tamaño del paso
keymap-step-forward = Paso adelante
    .description = Adelantar la cabeza de reproducción el tamaño del paso
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
keymap-report-issue = Informar de un problema
    .description = Abre el formulario de incidencias en el gestor del proyecto
keymap-open-discussions = Debates
    .description = Abre el foro de debates del proyecto
keymap-open-chat = Chat
    .description = Abre el canal de chat del proyecto
keymap-open-console = Consola
    .description = Abre la consola de registro
keymap-open-equalizer = Ecualizador
    .description = Abre la ventana del ecualizador
keymap-open-preset-picker = Presets del fondo
    .description = Abre el selector de presets de Milkdrop para el fondo
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
keymap-play-similar = Reproducir algo parecido
    .description = Saca una pista que suena como la que está sonando. Antes hay que describir la biblioteca
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
keymap-ab-repeat = Repetir A-B
    .description = Marca A, luego B, luego borra
keymap-ab-clear = Borrar la sección A-B
    .description = Descarta la sección A-B y cualquier paso a medio marcar
keymap-bookmark = Añadir marcador
    .description = Poner un marcador en la posición actual
keymap-bookmark-named = Añadir marcador con nombre
    .description = Poner un marcador en la posición actual y darle nombre
keymap-prev-bookmark = Marcador anterior
    .description = Volver al marcador anterior a la posición actual
keymap-next-bookmark = Marcador siguiente
    .description = Saltar al siguiente marcador
keymap-cue = Añadir punto
    .description = Poner un punto de sesión en la posición que suena. Los puntos duran hasta que se cierra rox
keymap-cue-prev = Punto anterior
    .description = Volver al punto anterior al cabezal
keymap-cue-next = Punto siguiente
    .description = Saltar al siguiente punto
keymap-cue-clear = Borrar puntos
    .description = Quitar todos los puntos de sesión de la pista en reproducción
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
keymap-cycle-shuffle-mode = Alternar orden aleatorio
    .description = Pasa por los órdenes del modo aleatorio: al azar, lo más parecido por sonido o álbum por álbum
keymap-cycle-loop = Alternar repetición
    .description = Pasa la repetición de apagada a todo, a una y vuelta a empezar
keymap-toggle-stop-after = Parar después
    .description = Deja terminar la pista actual y luego pausa. Pulsa otra vez para desactivarlo
keymap-toggle-continuation = Alternar seguir reproduciendo
    .description = Desactiva la continuación o la vuelve a activar con la última estrategia usada
keymap-sleep-off = Cancelar el temporizador de sueño
    .description = Cancela un temporizador de sueño en marcha, sin tocar la reproducción
keymap-clear-queue = Vaciar la cola
    .description = Vacía la cola de lo siguiente. La pista que suena y su contexto se quedan
keymap-toggle-favourite = Alternar favorito
    .description = Marca como favorita la pista que suena o le quita la marca
keymap-rename-track = Renombrar pista
    .description = Edita el título de la última pista elegida en cualquier lista
keymap-trash-selection = Mover la selección a la papelera
    .description = Pregunta si mover a la papelera del sistema los archivos de las pistas elegidas por última vez en cualquier lista
keymap-volume-up = Subir volumen
    .description = Sube el volumen un paso
keymap-volume-down = Bajar volumen
    .description = Baja el volumen un paso
keymap-toggle-crossfade = Alternar fundido encadenado
    .description = Desactiva el fundido o lo vuelve a activar con la última duración usada
keymap-toggle-crossfade-albums = Alternar fundido dentro de los álbumes
    .description = Cambia si el fundido alcanza también los cortes entre pistas contiguas de un álbum
keymap-cycle-replaygain-mode = Alternar modo de ReplayGain
    .description = Pasa por la regla de nivelado que lee el motor: desactivado, pista, álbum
keymap-toggle-exclusive-output = Alternar salida exclusiva
    .description = Reserva el dispositivo de salida solo para rox o lo devuelve. Reconstruye la sesión en curso
keymap-toggle-eq = Alternar ecualizador
    .description = Cambia si el ecualizador da forma a la salida
keymap-flatten-eq = Aplanar ecualizador
    .description = Devuelve todas las bandas del ecualizador a 0 dB
keymap-toggle-scrobbling = Alternar scrobbling
    .description = Cambia si las pistas reproducidas van a los servicios de scrobble conectados
keymap-toggle-discord = Alternar presencia en Discord
    .description = Cambia si Discord muestra lo que rox está reproduciendo
keymap-toggle-broadcast = Alternar emisión por Icecast
    .description = Inicia o detiene la emisión al servidor Icecast configurado en los ajustes
keymap-toggle-capture = Alternar captura de canciones
    .description = Cambia si las canciones de fuentes en streaming se guardan en la carpeta de capturas
keymap-close-panel = Cerrar panel
    .description = Cierra el panel activo del grupo de paneles enfocado
keymap-new-empty-window = Ventana vacía
    .description = Abre una ventana de espacio de trabajo sin nada dentro
keymap-open-signals = Señales
    .description = Abre la ventana de señales, el conjunto detrás de las rutas de cada panel
keymap-open-composition = Composición
    .description = Abre la ventana de composición: los paneles de esta ventana tal como están, y Convertir en lienzo
keymap-import-workspace = Importar espacio de trabajo
    .description = Elige un archivo de espacio de trabajo y lo añade a la colección
keymap-save-layout = Guardar disposición
    .description = Pone nombre a la disposición actual de los paneles y la guarda como preajuste
keymap-save-workspace = Guardar espacio de trabajo
    .description = Pone nombre al aspecto actual y lo guarda como paquete de espacio de trabajo
keymap-toggle-quit-to-tray = Alternar quedarse en la bandeja
    .description = Cambia si cerrar la última ventana deja a rox en la bandeja
keymap-toggle-design-mode = Alternar modo de diseño
    .description = Cambia si los paneles se pueden reorganizar en el sitio
keymap-convert-layout-to-canvas = Convertir disposición en lienzo
    .description = Convierte la disposición de esta ventana en un lienzo, tras una confirmación
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
keymap-toggle-seams = Alternar juntas entre paneles
    .description = Muestra u oculta las líneas divisorias entre paneles
keymap-toggle-readings = Alternar lecturas de los nombres
    .description = Cambia si los nombres muestran su lectura en lugar de la grafía etiquetada
keymap-toggle-watch-folders = Alternar vigilancia de carpetas
    .description = Incorpora los cambios de archivos a la biblioteca según ocurren, o vuelve a escanear a mano
keymap-toggle-backdrop-shader = Alternar shader de fondo
    .description = Activa o desactiva el shader sobre el fondo de carátula
keymap-toggle-backdrop-all-windows = Alternar fondo en todas las ventanas
    .description = Pone el fondo también tras las ventanas secundarias o lo deja solo en las de espacio de trabajo
keymap-toggle-milkdrop-hard-cuts = Alternar cortes duros de Milkdrop
    .description = Deja que un golpe fuerte corte directo al siguiente preset, o solo funde entre ellos
keymap-random-milkdrop-preset = Preset de Milkdrop al azar
    .description = Salta en el fondo Milkdrop a un preset al azar de su rotación
keymap-toggle-milkdrop-backdrop = Alternar fondo Milkdrop
    .description = Activa o desactiva el visual de Milkdrop detrás de la ventana
keymap-toggle-milkdrop-lock = Alternar bloqueo del preset de Milkdrop
    .description = Fija el preset actual de Milkdrop o lo deja rotar de nuevo
keymap-toggle-mini = Alternar disposición mini
    .description = Cambia entre la disposición mini y la principal
keymap-rescan-library = Volver a escanear la biblioteca
    .description = Escanear otra vez todas las carpetas recordadas de la biblioteca
keymap-abort-scan = Detener el escaneo
    .description = Detiene un escaneo de la biblioteca en el siguiente archivo. Lo ya indexado se queda
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
panel-catalog-group-plugins = Plugins
panel-catalog-group-visualizers = Visualizadores
panel-catalog-group-widgets = Widgets
panel-catalog-history = Historial
panel-catalog-bookmarks = Marcadores
panel-catalog-stations = Emisoras
panel-catalog-menu = Menú
panel-catalog-metadata = Metadatos
panel-catalog-oscilloscope = Osciloscopio
panel-catalog-overlay = Superposición
panel-catalog-particles = Partículas
panel-catalog-playlists = Listas de reproducción
panel-catalog-queue = Cola
panel-catalog-queue-widget = Widget de cola
panel-catalog-renderer = Renderizador
panel-catalog-canvas = Lienzo
panel-catalog-seek = Posición
panel-catalog-slide = Diapositiva
panel-catalog-spectrogram = Espectrograma
panel-catalog-spectrum = Espectro
panel-catalog-stats-widget = Widget de estadísticas
panel-catalog-status = Estado
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
lastfm-import-unknown = , { $count } que no están en tu biblioteca
lastfm-import-plays-read = { $count ->
    [one] Leída { $count } pista con reproducciones
   *[other] Leídas { $count } pistas con reproducciones
}
lastfm-import-plays-stopped = { $count ->
    [one] Detenido tras { $count } pista
   *[other] Detenido tras { $count } pistas
}
lastfm-import-plays-matched = , { $count } con coincidencia
lastfm-import-plays-updated = { $count ->
    [one] , { $count } reproducción añadida
   *[other] , { $count } reproducciones añadidas
}
lastfm-import-history = Leyendo el historial de scrobbles
lastfm-import-plays-counts = Leyendo los recuentos de reproducción
lastfm-import-plays-writing = Escribiendo el historial de reproducción
lastfm-import-plays-dated = , { $count } con fecha real
lastfm-import-plays-unknown = , { $count } de canciones que no están en tu biblioteca
lastfm-import-plays-stopped-history = Detenido tras { $count } scrobbles

## Tag tools
tags-editor-add-tag = Añadir
tags-editor-clear-all = borrar todo
tags-editor-form-view = Formulario
tags-editor-format-unsupported-all = Las etiquetas de este formato todavía no se pueden leer ni escribir.
tags-editor-format-unsupported-some = Algunos de estos archivos están en un formato cuyas etiquetas todavía no se pueden leer ni escribir.
tags-editor-guess-button = Deducir
tags-editor-guess-folded = { $status }, { $count } más sin mostrar
tags-editor-guess-help = / coincide con la carpeta de arriba, %skip% descarta
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
tags-editor-replace = reemplazar
tags-editor-replace-apply = Aplicar
tags-editor-replace-find = Buscar
tags-editor-replace-help = Cada coincidencia se reemplaza. Con regex, $1 inserta el primer grupo. Los resultados se recortan en los bordes.
tags-editor-replace-ignore-case = Ignorar mayúsculas
tags-editor-replace-match-count = { $hits ->
    [one] { $hits } de { $total } cambia
   *[other] { $hits } de { $total } cambian
}
tags-editor-replace-regex = Regex
tags-editor-replace-with = Reemplazar
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

# The tag fields as the tag editor and the release matcher name them.
tags-field-file = Archivo
tags-field-title = Título
tags-field-title-sort = Orden de título
tags-field-artist = Artista
tags-field-artist-sort = Orden de artista
tags-field-album-artist = Artista del álbum
tags-field-album-artist-sort = Orden de artista del álbum
tags-field-album = Álbum
tags-field-album-sort = Orden de álbum
tags-field-genre = Género
tags-field-year = Año
tags-field-track = N.º de pista
tags-field-disc = Disco
tags-field-comment = Comentario
tags-field-rating = Valoración

tags-guess-empty-segment = el patrón deja un nombre de carpeta o de archivo vacío
tags-guess-no-placeholders = sin marcadores
tags-guess-placeholders = Marcadores
tags-guess-skip-renders-nothing = %skip% no tiene nada que representar
tags-guess-unclosed = % sin cerrar
tags-guess-unknown-placeholder = marcador desconocido %{ $name }%
tags-matcher-blocked-arm = Activa un campo para aplicar
tags-matcher-blocked-no-match = No hay coincidencia que aplicar
tags-matcher-blocked-pick = Elige una coincidencia
tags-matcher-blocked-writing = Escribiendo las etiquetas...
tags-matcher-identify = Identificar por sonido
tags-matcher-identify-no-duration = No hay duración de la pista para identificarla
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
tags-rename-pattern-help = / crea una carpeta desde la raíz de la biblioteca, sin / se renombra en el sitio, la extensión sigue al archivo
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
tags-repair-scanning = Escaneando...
tags-repair-scanning-progress = Escaneando { $done }/{ $total }...
tags-repair-count = { $count ->
    [one] 1 archivo
   *[other] { $count } archivos
}
tags-repair-count-so-far = { $count } hasta ahora
tags-repair-label-scope = Alcance
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
convert-dialog-label-extension = Extensión
convert-dialog-label-format = Formato
convert-dialog-label-into = En
convert-dialog-label-named = Llamado
convert-dialog-mirror = Reflejar las carpetas de la biblioteca
convert-dialog-nothing-to-convert = Nada que convertir: se omiten todas las filas
convert-dialog-pattern-help = / crea una carpeta, el formato pone la extensión
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
track-title-not-savable = Ponle un título a la pista para guardarla
track-title-saving = Guardando el título...
track-title-window-title = rox - Renombrar pista

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
cover-editor-slot-artist = Artista
shader-read-error = al leer { $path }: { $error }
plugin-sync-still-paging = { $collection } sigue paginando después de { $pages } páginas
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
cover-matcher-searching = Buscando...
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
cover-preset-turntable = Tocadiscos
cover-preset-cd-player = Reproductor de CD
cover-preset-disc-scan = Disco escaneado
cover-preset-preview = Vista previa

## Lyrics
lyrics-always-centered = Siempre centrado
    .description = Rellena los extremos para que la primera y la última línea también puedan centrarse
lyrics-auto-search = Búsqueda automática
    .description = Busca en línea en una pista sin letra y guarda una coincidencia segura, sin selector
lyrics-bold = Negrita
lyrics-build-word-by-word = Seguir las palabras cantadas
    .description = Recorre la línea activa con un cursor de lectura, según los tiempos de palabra de la hoja cuando los tiene
lyrics-edge-bottom = Abajo
lyrics-edge-top = Arriba
lyrics-edit-hint-after-stamp = para marcar
lyrics-edit-hint-or = o
lyrics-edit-loading = Cargando la letra...
lyrics-edit-lyrics = Editar la letra
lyrics-edit-offset = Desfase
lyrics-edit-offset-unit = s
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
lyrics-head-build = Construcción
lyrics-head-fill = Relleno
lyrics-hide-upcoming = Ocultar las líneas siguientes
    .description = Mantén fuera de vista cada línea a la que el cursor no ha llegado, para que la hoja se vaya descubriendo sola
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
lyrics-matcher-searching = Buscando...
lyrics-matcher-no-matches = No se encontraron coincidencias
lyrics-matcher-synced-tag = { $provider }  sincronizada
lyrics-matcher-window-title = rox - Buscar letras
lyrics-no-lyrics-notice = Sin letra
lyrics-no-lyrics-track = Esta pista no tiene letra
lyrics-read-head-style = Cursor de lectura
    .description = Cómo muestra el cursor la línea activa: un relleno de karaoke que la barre, o cada palabra construyéndose a su turno
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
lyrics-unsung-dim = Atenuación de lo no cantado
    .description = Cuánto atena el relleno la parte de la línea activa que todava no se ha cantado
lyrics-wipe-lyrics = Borrar la letra
lyrics-wrap-lines = Ajustar las líneas largas
    .description = Deja que una línea demasiado ancha para el panel pase a otra fila en vez de cortarse
lyrics-preset-karaoke = Karaoke
lyrics-preset-teleprompter = Teleprompter
lyrics-preset-quiet = Discreto

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
pass-tempo-body = Se decodifican dos ventanas de medio minuto de cada archivo y se cuentan los tiempos, y se leen más ventanas cuando esas dos no coinciden, para que la biblioteca pueda mostrar a qué velocidad va una pista. Funciona mejor con música grabada a claqueta y omite lo que no puede medir. Los números van a la base de datos de la biblioteca y tus archivos quedan intactos.
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
quick-play-library-empty = La biblioteca está vacía
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
mini-tip-shrink = Encoger al mini reproductor

## System tray
tray-open = Abrir
tray-pause = Pausa
tray-play = Reproducir
tray-quit = Salir

## Window controls
window-controls-close = Cerrar
window-controls-maximize = Maximizar
window-controls-exit-fullscreen = Salir de pantalla completa
window-controls-fullscreen-zoom = Pantalla completa, u Opción-clic para ampliar
window-controls-mini-toggle = Alternar mini
window-controls-minimize = Minimizar
window-controls-pieces = Botones
    .description = Arrastra a lo largo de la barra para reordenar; arrastra entre las filas, o usa la x y el más de una ficha, para ocultar y mostrar. El botón mini aparece en cuanto hay una disposición mini asignada
window-controls-pin = Fijar
window-controls-pin-mini-only = Fijar solo en mini
    .description = Muestra el botón de fijar solo mientras la ventana está en la disposición mini. En Wayland, fijar necesita KWin y una instalación fuera de Flatpak
window-controls-pin-tip-off = Dejar de mantener sobre las demás ventanas
window-controls-pin-tip-on = Mantener sobre las demás ventanas
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
particles-preset-fountain = Fuente
particles-preset-snowfall = Nevada
particles-preset-embers = Brasas
particles-preset-starburst = Estallido
particles-preset-swarm = Enjambre

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
spectrum-gradient-curve = Curva
    .description = Cuánto debe subir el nivel antes de tomar el color de la punta. 1 mezcla de forma uniforme, un valor mayor lo reserva para los picos
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
spectrum-preset-retro = Retro
spectrum-preset-smooth = Suave
spectrum-preset-ambient = Ambiente
spectrum-preset-analyzer = Analizador

## Waveform panel
waveform-bar-gap = Separación entre barras
    .description = Espacio entre barras; cero las funde en una sola forma
waveform-bar-width = Ancho de barra
    .description = Cuán gruesa se dibuja cada barra
waveform-outline = Contorno
    .description = Traza las barras en vez de rellenarlas; las barras fundidas se leen como una sola forma
waveform-loudness = Banda de volumen
    .description = Dibuja el nivel RMS como una banda más plana dentro de la envolvente de picos
waveform-shade-played = Sombrear lo reproducido
    .description = Tiñe el fondo detrás de la parte de la pista que ya sonó
waveform-split-bars = Dividir colores de barras
    .description = Elige tú el color de las barras a cada lado del cabezal, en lugar de la fuente de color
waveform-played-bars = Barras reproducidas
    .description = Las barras detrás del cabezal
waveform-unplayed-bars = Barras pendientes
    .description = Las barras que aún están delante del cabezal
waveform-gradient-mode = Origen del color
    .description = De dónde toman su color la envolvente y la banda: el acento, la rampa del tema, la carátula, o un par personalizado
waveform-scrobble-marker = Marca de scrobble
    .description = Una línea fina donde la pista cuenta como scrobbleada a Last.fm
waveform-bookmarks = Marcadores
    .description = Los marcadores de la pista actual como flechas en el borde inferior: clic para saltar, clic derecho para editar
waveform-split-channels = Separar canales
    .description = Una fila por canal, izquierdo sobre derecho; las pistas mono se quedan en una sola fila
waveform-live-mode = Forma en vivo
    .description = Qué dibuja la franja mientras suena una emisora de radio: nada, el sonido de la propia emisora, o una forma que se dibuja sola
waveform-live-trace = Traza
waveform-live-motion = Movimiento
waveform-live-window = Ventana en vivo
    .description = Segundos de una emisora de radio que abarca la traza en movimiento; una ventana más amplia significa menos velocidad
waveform-live-expression = Expresión de movimiento
    .description = x va de 0 a 1 a lo ancho de la franja, t son los segundos del reloj del panel; sin cos tan abs sqrt exp ln floor min max clamp mix, con + - * / ^ pi y paréntesis
waveform-live-expression-error = { $reason }, así que la franja dibuja la forma por defecto

# Why a motion expression didn't parse; $at is the character position.
expr-error-stray = carácter inesperado en la posición { $at }
expr-error-unknown = nombre desconocido en la posición { $at }
expr-error-arity = número de argumentos incorrecto en la posición { $at }
expr-error-unclosed = paréntesis sin cerrar en la posición { $at }
expr-error-ended = la expresión termina antes de tiempo en la posición { $at }
expr-error-trailing = texto sobrante en la posición { $at }
expr-error-too-deep = anidado demasiado hondo en la posición { $at }
expr-error-too-long = demasiado largo en la posición { $at }

waveform-section-live = En vivo
waveform-streaming = EN STREAMING
waveform-unavailable = No hay forma de onda para esta pista
waveform-preset-progress = Progreso
waveform-preset-solid = Sólido
waveform-preset-outline = Contorno
waveform-preset-stereo = Estéreo
waveform-preset-blocks = Bloques

## VU panel
vu-ballistics = Balística
    .description = VU integra la sonoridad despacio; Pico salta arriba y baja suave
vu-ballistics-peak = Pico
vu-ballistics-vu = VU
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
vu-preset-analog = Analógico
vu-preset-led-ladder = Escalera LED
vu-preset-peak-hold = Retener picos
vu-preset-mono-strip = Barra mono

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
spectrogram-preset-lab = Laboratorio
spectrogram-preset-waterfall = Cascada
spectrogram-preset-ambient = Ambiente
spectrogram-preset-low-end = Graves

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
oscilloscope-preset-phosphor = Fósforo
oscilloscope-preset-sweep = Barrido
oscilloscope-preset-zoom = Zoom
oscilloscope-preset-stereo = Estéreo

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
genre-grid-preset-spotlight = Foco
genre-grid-preset-compact = Compacto
genre-grid-preset-browse = Explorar
genre-grid-preset-shelf = Estante
genre-grid-preset-cards = Tarjetas

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
artist-grid-preset-spotlight = Foco
artist-grid-preset-compact = Compacto
artist-grid-preset-browse = Explorar
artist-grid-preset-shelf = Estante
artist-grid-preset-portraits = Retratos

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
metadata-cover-opacity = Opacidad de la portada
    .description = Cuánto se ve la portada detrás de los campos
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
metadata-field-bookmarks = Marcadores
metadata-field-bpm-measured = { $bpm } (medido por rox)
metadata-field-codec = Códec
metadata-field-comment = Comentario
metadata-field-copies = Copias
metadata-field-cover = Portada
metadata-field-source = Fuente
metadata-source-local = Local
metadata-source-radio = Radio
metadata-source-subsonic = Subsonic
metadata-source-unknown = Desconocido
metadata-field-station = Emisora
metadata-field-homepage = Sitio web
metadata-field-disc = Disco
metadata-field-file = Archivo
metadata-field-first-played = Primera reproducción
metadata-field-folder = Carpeta
metadata-field-gain-album = Ganancia del álbum
metadata-field-gain-track = Ganancia de la pista
metadata-field-last-played = Última reproducción
metadata-field-lastfm-listeners = Oyentes globales
metadata-field-lastfm-loved = Favorita en Last.fm
metadata-field-lastfm-plays = Reproducciones globales
metadata-field-lastfm-tags = Etiquetas de Last.fm
metadata-field-lastfm-user-plays = Tus scrobbles
metadata-field-lyrics = Letra
metadata-field-mb-country = País de lanzamiento
metadata-field-mb-date = Lanzamiento
metadata-field-mb-first-release = Primer lanzamiento
metadata-field-mb-isrc = ISRC
metadata-field-mb-label = Sello
metadata-field-missing = Falta
metadata-field-rank = Puesto en la biblioteca
metadata-field-recent-plays = Reproducciones (30 días)
metadata-field-sample-rate = Frecuencia de muestreo
metadata-field-similar = Pistas parecidas
metadata-field-size = Tamaño
metadata-field-title-sort = Orden de título
metadata-field-track = Pista
metadata-fields = Campos
    .description = Qué campos lista la hoja; un campo sin valor muestra n/d, y un nombre de ordenación solo aparece donde difiere
metadata-find-online = Buscar metadatos en línea...
metadata-no-library = Sin biblioteca
metadata-romanize = Romanizar
metadata-romanize-needs-dictionary = Un nombre en kanji necesita el diccionario japonés. Consíguelo en Ajustes > Biblioteca.
metadata-romanize-sort-names = Romanizar los nombres de ordenación
metadata-row-borders-description = La línea fina bajo cada fila de la tabla
metadata-cover-embedded = Incrustada, { $px }
metadata-cover-folder = { $file }, { $px }
metadata-hide-empty = Ocultar campos vacíos
    .description = Omitir un campo activado sin valor en vez de listarlo como n/d
metadata-loved-no = No
metadata-loved-yes = Sí
metadata-lyrics-plain = Sin tiempos
metadata-lyrics-synced = Sincronizada
metadata-missing-none = Nada
metadata-rank-value = n.º { $rank }
metadata-tags-scope-album = (etiquetas del álbum)
metadata-tags-scope-artist = (etiquetas del artista)
metadata-track-of = { $track } de { $total }
metadata-value-none = n/d
metadata-source = Origen
    .description = Sigue lo que suena o lo seleccionado, o lee la biblioteca en conjunto
metadata-stripes-description = Tiñe una fila de la tabla sí y otra no
metadata-preset-tagging = Etiquetado
metadata-preset-technical = Técnico
metadata-preset-listening = Escuchas
metadata-preset-minimal = Mínimo
metadata-preset-overview = Resumen

## History panel
history-column-last-played = Última reproducción
history-live-plays-file = Reproduce tu propia copia de esta canción
history-live-plays-station = Reproduce la emisora; tu biblioteca no tiene esta canción
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
history-play-from = Reproducir desde
history-unknown-row = No está en tu biblioteca; haz doble clic para reproducirla desde un plugin
history-finding = Buscando { $title } en { $source }...
history-not-found = { $source } no tiene resultados para { $title }
history-find-failed = No se pudo buscar en { $source }: { $reason }
history-no-plugins = Ningún plugin en ejecución puede buscar esta canción
history-pick-plugin = Elige un plugin en Reproducir desde, en el menú de esta fila
history-view-row = Vista
    .description = Qué corte del registro de escuchas muestra el panel
history-preset-timeline = Cronología
history-preset-charts = Éxitos
history-preset-discover = Descubrir

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
folder-tree-song-order = Orden de las canciones
    .description = Ordena las canciones de una carpeta por álbum, disco y número de pista, o por nombre de archivo. Se reproducen en ese mismo orden
folder-tree-song-order-filename = Nombre de archivo
folder-tree-song-order-track = Número de pista
folder-tree-stripes-description = Tiñe una fila sí y otra no para que un árbol profundo se lea de un vistazo
folder-tree-title = Árbol
folder-tree-preset-covers = Portadas
folder-tree-preset-search = Búsqueda
folder-tree-preset-now-playing = Sonando

## Art panel
art-always = Mantén las carátulas atrás incluso cuando no suena nada; solo la carátula bajo el cursor se ve entera
art-convert = Convertir...
art-covers-section = Carátulas
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
art-preset-gallery = Galería
art-preset-spotlight = Foco
art-preset-browse = Explorar
art-preset-jukebox = Rocola

## Playlists panel
playlists-art-description = La carátula de los encabezados desplegados
playlists-line-height-description = Una línea de encabezado; se dibuja dentro de las filas que el bloque ya tiene, así que encoger una línea abre espacio en vez de agrandar el bloque
playlists-meta-line = Línea de datos
playlists-meta-line-description = Lo que muestra la segunda fila bajo ella, del mismo modo
playlists-name-line = Línea del nombre
playlists-name-line-description = Lo que muestra la primera fila del encabezado, de izquierda a derecha; un separador o divisor reparte los lados
playlists-columns = Qué columnas de pista se ven junto al título
playlists-delete = Eliminar la lista
playlists-delete-body = La lista desaparece y sus pistas se quedan en la biblioteca.
playlists-edit-query = Editar la consulta...
playlists-empty = Todavía no hay listas; añade pistas o usa Nueva lista
playlists-export-tooltip = Exportar una lista
playlists-headings = Parte las pistas de cada lista en tandas por álbum; Ampliado añade la carátula y las estadísticas
playlists-import-tooltip = Importar una lista
playlists-import = Importar una lista...
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
playlists-reverse-tooltip = Invertir el orden
playlists-title = Listas de reproducción
playlists-preset-albums = Álbumes
playlists-preset-classic = Clásico
playlists-preset-covers = Portadas
playlists-preset-minimal = Mínimo

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
queue-preset-albums = Álbumes
queue-preset-detailed = Detallado
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
biography-active-between = Activo de { $from } a { $to }
biography-active-since = Activo desde { $from }
biography-background = Fondo
    .description = El fanart del artista detrás del texto, atenuado y desvaneciéndose hacia abajo
biography-background-opacity = Opacidad del fondo
    .description = Cuánto se ve el fanart detrás del texto
biography-cycle = Rotar imágenes de cabecera
    .description = Recorre el banner y el fanart del artista en la cabecera con un fundido, y pasa de una a otra a mano con las flechas que aparecen al pasar el ratón
biography-cycle-interval = Intervalo de rotación
    .description = Cuánto se mantiene cada imagen de cabecera antes de que aparezca la siguiente
biography-fill-width = Llenar el ancho
    .description = Deja que una cabecera alta ocupe todo el ancho en vez de quedarse limitada y centrada
biography-flag = Bandera del país
    .description = Mostrar el país como bandera en vez de su código de dos letras
biography-from-lastfm = De Last.fm
biography-from-plugin = De { $plugin }
biography-header-banner = Banner
    .description = La franja ancha con el logo que TheAudioDB guarda del artista, en la cabecera
biography-header-blur = Ajustar con fondo difuminado
    .description = Con las proporciones apagadas, encajar la imagen entera en la franja sobre una versión difuminada de sí misma en vez de recortarla
biography-header-fanart = Fanart
    .description = Los fanarts 16:9 del artista de TheAudioDB, en la cabecera
biography-header-height = Altura de la cabecera
    .description = La altura de la franja de cabecera en px, y el tope para una que mantiene sus proporciones
biography-header-image = Imagen de cabecera
    .description = El banner ancho del artista de arriba, o el retrato cuando no hay banner
biography-header-overlay = Título sobre la cabecera
    .description = Poner el nombre, el origen y los años sobre la parte baja de la imagen de cabecera en vez de debajo
biography-keep-aspect = Mantener la proporción
    .description = Muestra la cabecera con sus proporciones en vez de recortarla para llenar una franja
biography-listeners-count = { $count ->
    [one] { $count } oyente
   *[other] { $count } oyentes
}
biography-looking-up = Buscando { $name }
biography-name-source = Nombre desde
    .description = Qué etiqueta nombra al artista: el artista de la pista, o el artista del álbum en un recopilatorio o un disco lleno de invitados
biography-no-artist-tag = Sin etiqueta de artista
biography-no-text = No hay biografía guardada
biography-not-found = No se encontró nada de { $name }
biography-plays-count = { $count ->
    [one] { $count } reproducción
   *[other] { $count } reproducciones
}
biography-profile = Origen y años
    .description = El código de país y los años en activo, de TheAudioDB
biography-refresh = Actualizar
biography-seconds = { $count } s
biography-similar-artists = Artistas parecidos
    .description = Artistas relacionados según los datos de escucha, como fichas: un clic abre su ficha, y una lupa en los que la biblioteca tiene los selecciona en la búsqueda
biography-similar-heading = Artistas parecidos
biography-stats = Estadísticas
    .description = Oyentes y reproducciones en Last.fm, bajo el nombre
biography-tags = Etiquetas
    .description = Las etiquetas de género como una fila de fichas
biography-title = Biografía
biography-top-tracks = Pistas más escuchadas
    .description = Las pistas más reproducidas en Last.fm, un clic reproduce las que tenga la biblioteca y el más las pone en cola
biography-top-tracks-count = Número de pistas
    .description = Cuántas de las pistas más escuchadas listar
biography-top-tracks-heading = Pistas más escuchadas
biography-preset-reader = Lectura
biography-preset-poster = Póster
biography-preset-discover = Descubrir
biography-preset-compact = Compacto

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
columns-date = Fecha
columns-fav = Fav
columns-gain = Ganancia
columns-kbps = Kbps
columns-khz = kHz
columns-name = Nombre
columns-number = Número
columns-scanned = Escaneada
columns-similar = Parecido
columns-source = Fuente
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
playback-ab-clear = Borrar A-B
playback-ab-set-a = Marcar A
playback-ab-set-b = Marcar B
playback-ab-tip-a = A en { $a }. Pulsa otra vez para marcar B
playback-ab-tip-looping = Repitiendo de { $a } a { $b }
playback-ab-tip-off = Marca el inicio de la sección que quieres repetir
playback-hold-order = { $tip }. Mantén pulsado para elegir un orden
playback-loop-off = Repetición desactivada
playback-loop-queue = Repetir la cola
playback-loop-track = Repetir esta pista
playback-menu-ab-repeat = Botón de repetir A-B
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
playback-shuffle-random = Aleatorio activado, orden al azar
playback-shuffle-similar = Aleatorio activado, orden parecido
playback-shuffle-albums = Aleatorio activado, orden por álbumes
playback-stop-after-armed = Parar después de esta pista, armado
playback-stop-after-tip = Parar después de esta pista
playback-stop-tip = Detener y descargar la pista
playback-volume-tip-muted = Quitar el silencio, { $percent }%. Clic derecho para el deslizador
playback-volume-tip-unmuted = Silenciar, { $percent }%. Clic derecho para el deslizador
playback-preset-minimal = Mínimo
playback-preset-practice = Práctica
playback-preset-discovery = Descubrir

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
track-info-output-exclusive = Exclusivo { $rate } kHz { $format }
track-info-queue-finished = (cola terminada)
expr-problem-stray = carácter inesperado
expr-problem-unknown = nombre desconocido
expr-problem-arity = número de argumentos incorrecto
expr-problem-unclosed = paréntesis sin cerrar
expr-problem-ended = la expresión termina antes de tiempo
expr-problem-trailing = texto sobrante
expr-problem-too-deep = anidado demasiado profundo
expr-problem-too-long = demasiado largo
track-info-output-resample-exclusive = Este archivo es de { $source } kHz y la tarjeta aceptó { $device } kHz, así que cada muestra se convierte de camino a la salida. El dispositivo no quiso funcionar a la frecuencia propia del archivo.
track-info-output-resample-mixer = Este archivo es de { $source } kHz y el mezclador va a { $device } kHz, así que cada muestra se convierte de camino a la salida. El modo exclusivo le daría a la tarjeta la frecuencia propia del archivo.
track-info-output-resample-exclusive-stream = Esta emisión es de { $source } kHz y la tarjeta aceptó { $device } kHz, así que cada muestra se convierte de camino a la salida. El dispositivo no quiso funcionar a la frecuencia propia de la emisión.
track-info-output-resample-mixer-stream = Esta emisión es de { $source } kHz y el mezclador va a { $device } kHz, así que cada muestra se convierte de camino a la salida. El modo exclusivo le daría a la tarjeta la frecuencia propia de la emisión.
track-info-overflow-loop = Girar
track-info-overflow-scroll = Recorrer
track-info-overflow-truncate = Truncar
track-info-queued-count = { $count } en cola
track-info-artist-scrobbles = Scrobbles del artista: { $count }
track-info-track-scrobbles = Scrobbles de la pista: { $count }
track-info-scrobble-style-text = Texto
track-info-scrobble-style-badge = Insignia
track-info-scrobble-style-labelled = Insignia con etiqueta
track-info-divider-stretch = Extendido
track-info-divider-bar = Barra
track-info-badge-artist = Artista
track-info-badge-track = Pista
track-info-row-size = Tamaño de la fila { $number }
track-info-speed = Velocidad
    .description = A qué velocidad recorre la línea
track-info-text-size = Tamaño del texto
track-info-preset-ticker = Cinta
track-info-preset-stacked = Apilado
track-info-preset-technical = Técnico
track-info-preset-rotating = Rotativo

## Playback failures

player-stream-refused = Pista omitida: { $reason }

## Seek panel
seek-ending = Final
    .description = Cuenta atrás el tiempo restante o muestra la duración completa
seek-ending-remaining = Restante
seek-ending-total = Total
seek-playhead = Cursor de reproducción
    .description = Ocupa toda la altura de la barra, cíñete a la línea u ocúltalo
seek-playhead-full = Completo
seek-playhead-line = Línea
seek-playhead-max-height = Altura máxima del cursor
    .description = Limita el cursor completo, centrado en la línea; 0 llena el panel
seek-playhead-off = Oculto
seek-playhead-width = Ancho del cursor
    .description = El ancho del marcador de posición que se mueve
seek-precise = Relojes precisos
    .description = Muestra los milisegundos en los relojes, para sincronizar letras a mano
seek-rounding = Redondeo
    .description = El radio de esquina de la línea, hasta una cápsula a la mitad del grosor
seek-scrobble-marker = Marca de scrobble
    .description = Una línea fina donde la pista cuenta como scrobbleada a Last.fm
seek-bookmarks = Marcadores
    .description = Los marcadores de la pista actual como flechas bajo la línea: clic para saltar, clic derecho para editar
seek-show-timings = Mostrar los tiempos
seek-dash-length = Largo del trazo
    .description = El largo de cada trazo de la entrada, con un hueco del mismo tamaño después
seek-lead-in = Entrada del búfer
    .description = Cómo se dibuja la parte del búfer de la emisora que aún no se ha llenado, el tramo a la izquierda de la cinta
seek-lead-in-dashed = Discontinua
seek-lead-in-faint = Tenue
seek-lead-in-hidden = Oculta
seek-lead-in-sweep = Barrido de la entrada
    .description = Un degradado que recorre la parte que aún no se ha llenado, como en una barra de carga
seek-section-live = En vivo
seek-thickness = Grosor
    .description = La altura de la línea de la pista
seek-preset-hairline = Línea fina
seek-preset-stacked = Apilado
seek-preset-pill = Píldora

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
volume-preset-fader = Fader
volume-preset-icon-only = Solo icono

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
transport-favourite-half-here = Favorito aquí, pero no en el servicio del plugin. Haz clic para añadirlo allí
transport-favourite-half-there = Favorito en el servicio del plugin, pero no aquí. Haz clic para añadirlo aquí
transport-favourite-nothing = No hay nada que marcar como favorito
transport-favourite-remove = Quitar de favoritos
transport-live = EN VIVO
transport-live-opening = Conectando con la emisora
transport-opening-track = Abriendo la pista
transport-live-reconnecting = Se cortó la emisión; reconectando
transport-live-dropped = La emisión se perdió
transport-live-jump = Ir al directo
transport-pieces = Piezas
    .description = Arrastra a lo largo de una fila para reordenar y entre filas para mover; la x y el más de una ficha ocultan y muestran

## Stragglers picked up in the final sweep

duplicates-scanning = Escaneando...
duplicates-section = Duplicados
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
panel-title-output = Salida
panel-title-overlay = Superposición
panel-title-playlists = Listas de reproducción
panel-title-queue = Cola
panel-title-queue-widget = Widget de cola
panel-title-canvas = Lienzo
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
unit-hz = Hz
unit-khz = kHz

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
workspace-shipped-aero-blurb = Una mañana soleada y azul cielo dibujada en un solo lienzo: paneles de cristal sobre nubes y colinas verdes, botones de gel, burbujas que suben con la música, una aurora que respira con los medios y un sol que destella con el pulso. El modo oscuro lo convierte en noche.
workspace-shipped-catrox-blurb = La skin de foobar2000 con la que empezó todo, reconstruida: una representación circular de la carátula como CD, los campos de metadatos por la izquierda, y pistas agrupadas por álbum con puntos de valoración.
workspace-shipped-critters-blurb = Toda la aplicación como una impresión de 1 bit: un dithering ordenado sobre cada superficie, tonos que se aplastan con los subgraves, y un muro de ruido que se retuerce con la canción. Inspirado en Critters for Sale.
workspace-shipped-diffuse-blurb = Solo el álbum en reproducción: la carátula y la tarjeta de reproducción como un único grupo que llena la ventana, superficies transparentes sobre el fondo, sin juntas. La biblioteca, la cola y las letras esperan en un cajón del borde derecho y salen deslizándose sobre la música cuando el cursor toca el tirador. Monocromo, así que el color lo ponen las carátulas.
workspace-shipped-foobar-blurb = La disposición con la que discute todo este proyecto. Paneles opacos, columnas de filtro por artista y álbum, una tabla de pistas densa, y la barra de menús justo donde estuvo siempre.
workspace-shipped-llama-winamp-blurb = Winamp tal como lo recuerdas y no tal como era. Tahoma, oscuro, sin marco, un espectro punteado arriba, y un modo reducido en la disposición mini.
workspace-shipped-metro-blurb = Paneles planos y filas holgadas en Segoe UI, con el tinte de carátula activado para que toda la paleta siga a la carátula que esté sonando.
workspace-shipped-phosphor-blurb = Todo en monoespaciada. Consolas, verde sobre negro, sin carátula en la reproducción rápida: un terminal que resulta que reproduce música.

## Milkdrop panel

panel-title-milkdrop = Milkdrop
milkdrop-presets = Presets
milkdrop-tuning = Ajustes
milkdrop-roots = Carpetas de presets
milkdrop-add-root = Añadir carpeta
milkdrop-remove-root = Quitar
milkdrop-folder = Dónde busca rox
milkdrop-filter-presets = Filtrar presets
milkdrop-preset-count = { $count } de { $total } presets
milkdrop-locked = Fijar preset
    .description = Quedarse en este: sin cambio por tiempo ni corte con un beat
milkdrop-duration = Duración del preset
    .description = Cuánto aguanta un preset antes de que llegue el siguiente
milkdrop-beat-sensitivity = Sensibilidad al beat
    .description = Con qué facilidad projectM cuenta algo como beat. Una mezcla suave necesita más que una fuerte
milkdrop-hard-cuts = Cortes duros
    .description = Dejar que un beat lo bastante fuerte corte directo al siguiente preset
milkdrop-fps = Fotogramas por segundo
    .description = Lo que busca el hilo de render. 30 reduce a la mitad lo que cuesta leer cada fotograma
milkdrop-scale = Escala de render
    .description = Renderizar por debajo del tamaño del panel y escalar. Cuesta nitidez y ahorra la misma fracción de la lectura
milkdrop-tint = Color del álbum
    .description = Gira los tonos del visual hacia la portada de la pista que suena. Cero deja los colores del preset tal cual
milkdrop-fade-duration = Duración del fundido
milkdrop-idle = En reposo
    .description = Qué hace el visual mientras la reproducción está en pausa o detenida. Mantener deja el último fotograma en pantalla. Fundir lo lleva al fondo del panel y lo trae de vuelta al reproducir
milkdrop-idle-hold = Mantener
milkdrop-idle-fade = Fundir
milkdrop-run-focused = Seguir con el foco
    .description = El visual sigue en movimiento mientras el panel tenga el foco, haya reproducción o no. Desactivado, un fotograma retenido sigue quieto aunque el panel tenga el foco
milkdrop-show-name = Mostrar el nombre del preset
    .description = Mostrar el nombre del preset en la esquina unos segundos cuando cambia
milkdrop-next = Preset siguiente
milkdrop-previous = Preset anterior
milkdrop-random = Preset al azar
milkdrop-open-folder = Abrir la carpeta de presets
milkdrop-rescan = Volver a analizar
milkdrop-rotation = Rotación
    .description = Qué presets recorren Siguiente, Anterior y el cambio por tiempo
milkdrop-rotation-all = Todos los presets
milkdrop-empty = No se encontraron presets.
    .description = Pon un paquete en la carpeta de abajo y pulsa Volver a analizar. Estos son los tres que valen la pena, todos en projectM-visualizer en GitHub.
milkdrop-packs-hint = Deja aquí los packs de presets. Los tres que valen la pena son presets-cream-of-the-crop, presets-milkdrop-original y presets-milkdrop-texture-pack, todos en GitHub bajo projectM-visualizer.
milkdrop-no-gl = Milkdrop no pudo arrancar.
milkdrop-still-starting = Milkdrop sigue arrancando.
milkdrop-still-starting-detail = Esperando un contexto OpenGL del controlador gráfico.
milkdrop-no-frames = Milkdrop está en marcha, pero no ha llegado ningún fotograma.
milkdrop-no-frames-detail = Renderizando en { $renderer }, { $version }. La consola tiene los detalles.
milkdrop-favorite = Preset favorito
milkdrop-favorite-short = Favorito
milkdrop-favorited = En favoritos
milkdrop-favorites-only = Solo favoritos
milkdrop-rotation-favorites = Favoritos ({ $count })
milkdrop-no-favorites = Aún no hay nada marcado, así que esto mezcla toda la biblioteca. Marca un preset desde la lista, el menú del panel o la fila Sonando ahora.
milkdrop-reveal = Mostrar preset en la carpeta
milkdrop-reveal-short = Mostrar
milkdrop-current = Sonando ahora
milkdrop-no-preset = Nada cargado todavía
milkdrop-color = Color
    .description = Preset conserva los colores del archivo. Tema los invierte hacia claro en el tema claro. Paleta los pinta con el fondo y el acento del tema y omite el color del álbum. Carátula los pinta con el color de la carátula en reproducción, con o sin tematización por canción
milkdrop-color-preset = Preset
milkdrop-color-theme = Tema
milkdrop-color-palette = Paleta
milkdrop-color-cover = Carátula
milkdrop-name-always = Mantener el nombre en pantalla
    .description = Mantener el nombre en pantalla todo el tiempo en vez de unos segundos
milkdrop-flip-horizontal = Voltear horizontalmente
    .description = Refleja el visual de izquierda a derecha
milkdrop-flip-vertical = Voltear verticalmente
    .description = Refleja el visual de arriba abajo
milkdrop-choose = Elegir preset
milkdrop-picker-title = Elegir preset
milkdrop-picker-backdrop = Fondo
milkdrop-picker-host = Eligiendo para { $host }
milkdrop-view-tree = Árbol
milkdrop-view-grid = Cuadrícula
milkdrop-nested = Anidado
milkdrop-locate-current = Ir al activo
milkdrop-load-preset = Cargar preset
milkdrop-unfavorite = Quitar de favoritos
milkdrop-reveal-folder = Mostrar carpeta
milkdrop-show-controls = Mostrar controles
    .description = Anterior, Siguiente, Aleatorio y la estrella de favorito sobre la visualización mientras el puntero está en el panel
milkdrop-preset-ambient = Ambiente
milkdrop-preset-party = Fiesta
milkdrop-preset-browse = Explorar
milkdrop-preset-low-power = Bajo consumo

## Source browser

panel-catalog-source-browser = Fuentes externas
panel-title-source-browser = Fuentes externas
source-browser-title = Fuentes externas
source-browser-search = Buscar
source-browser-home = Inicio
source-browser-show-home = Mostrar la página de inicio
    .description = Muestra la página de inicio de la fuente bajo sus colecciones y otros lugares, sin un clic más. Desactivado, queda como una entrada más de la lista.
source-browser-show-notice = Mostrar el aviso de la fuente
    .description = La línea que un plugin pone sobre sus páginas para decir algo de ellas. Desactivado la oculta. Un aviso que pide un ajuste, o que está solo en una página vacía, se muestra igualmente.
source-browser-hide-notice = Ocultar
source-browser-results = Resultados para { $query }
source-browser-empty = No hay nada aquí
source-browser-pick = Elige una fuente
source-browser-no-sources = No hay ninguna fuente de plugin en marcha.
source-browser-failed = { $source } no respondió
source-browser-plugins-off = Los plugins están desactivados
source-browser-switched-off = { $source } está desactivado
source-browser-changed = { $source } cambió en el disco. Vuelve a activarlo para aprobar el cambio
source-browser-missing = { $source } ya no está en la carpeta de plugins
source-browser-cant-load = { $source } no se puede cargar. La página Plugins dice por qué
source-browser-stopped = { $source } se detuvo tras fallar varias veces. Desactívalo y vuelve a activarlo
source-browser-open-plugins = Abrir Plugins
source-browser-open-link = Abrir enlace
source-browser-jump-to-playing = Ir a lo que suena
source-browser-go-to = Ir a
source-browser-go-to-album = Álbum
source-browser-go-to-artist = Artista
source-browser-similar-failed = No se pudo reproducir nada parecido a { $title }
source-browser-now-playing = Sonando ahora
source-browser-up-next = A continuación
source-browser-sorted-first = Se ordenaron las primeras { $count } filas
source-browser-node-empty = No contiene pistas
source-browser-pick-failed = No se pudieron preparar estas pistas para reproducir
source-browser-add-to-library = Añadir a la biblioteca
source-browser-remove-from-library = Quitar de la biblioteca
panel-stop-keeping = Dejar de conservar { $title }
panel-stop-keeping-failed = No se pudo dejar de conservar { $title }
source-browser-save-failed = No se pudieron añadir estas pistas a la biblioteca
source-browser-unsave-failed = No se pudieron quitar estas pistas de la biblioteca
source-browser-sync-failed = No se pudo sincronizar { $title }
source-browser-sync = Conservar en la biblioteca. Sus pistas se añaden a tu biblioteca y siguen sus cambios
source-browser-synced = Conservado en la biblioteca. Haz clic para dejar de conservarlo
source-browser-library = Biblioteca
source-browser-library-kept = Colecciones guardadas
source-browser-library-added = Añadidas una a una
source-browser-library-empty = Todavía no hay nada de { $source } en la biblioteca. Guarda una colección o añade pistas para verlas aquí
source-browser-track-save = Añadir a la biblioteca
source-browser-track-saved = En la biblioteca. Haz clic para quitarla
source-browser-track-kept = En la biblioteca como parte de una colección guardada
source-browser-members = { $count ->
    [one] { $count } pista
   *[other] { $count } pistas
}
source-browser-play = Reproducir
source-browser-play-count = Reproducir { $count } pistas
lastfm-import-blocked-running = Ya hay una importación en curso
lastfm-import-blocked-no-key = Esta compilación no tiene clave de API con la que preguntar
lastfm-import-blocked-no-account = Conecta primero una cuenta de Last.fm

## Canvas

canvas-panel-title = Lienzo
canvas-tree-grid = Cuadrícula, { $name }
canvas-image-missing = Falta la imagen: { $asset }
canvas-inspector-title = Lienzo
canvas-inspector-items = Elementos
canvas-add-grid = Añadir cuadrícula
canvas-add-group = Añadir grupo
canvas-add-image = Añadir imagen
canvas-add-file = Añadir archivo...
canvas-add-image-failed = No se pudo añadir la imagen
canvas-add-image-not-image = Ese archivo no es una imagen que rox pueda mostrar.
canvas-stage = Escenario
canvas-stage-size = Tamaño
canvas-stage-fit = Ajuste
canvas-fit-anchor = Anclado
canvas-fit-contain = Contener
canvas-fit-cover = Cubrir
canvas-camera = Cámara
canvas-camera-fov = Campo de visión
canvas-camera-parallax = Paralaje
canvas-signals = Señales
canvas-idle = Cuando para la música
    .description = En pausa, detenida o al final de la cola. Reposo devuelve cada elemento a sus propios ajustes. Mantener deja cada uno donde lo dejó la música.
canvas-idle-rest = Reposo
canvas-idle-hold = Mantener
canvas-item-anchor = Ancla
canvas-item-width = Ancho
canvas-item-height = Alto
canvas-length-units = Unidades
canvas-length-percent = Porcentaje
canvas-item-rotation-x = Rotación X
canvas-item-rotation-y = Rotación Y
canvas-item-rotation-z = Rotación Z
canvas-item-scale = Escala
canvas-item-opacity = Opacidad
canvas-item-flat-note = Los paneles en vivo siguen planos en esta versión.
canvas-bring-forward = Traer adelante
canvas-send-back = Enviar atrás
canvas-order = Orden
canvas-bring-to-front = Traer al frente
canvas-send-to-back = Enviar al fondo
canvas-align-to-stage = Alinear con el escenario
canvas-align-to-group = Alinear con el grupo
canvas-align-left = Alinear a la izquierda
canvas-align-center-x = Centrar en horizontal
canvas-align-right = Alinear a la derecha
canvas-align-top = Alinear arriba
canvas-align-center-y = Centrar en vertical
canvas-align-bottom = Alinear abajo
canvas-align-fill = Cubrir
canvas-align-fit = Contener
canvas-align-turned = Un elemento girado se alinea por su caja antes del giro.
canvas-group = Agrupar
canvas-ungroup = Desagrupar
canvas-delete = Eliminar
canvas-image-asset = Imagen
canvas-image-fit = Ajuste
canvas-image-fit-contain = Contener
canvas-image-fit-cover = Cubrir
canvas-image-fit-stretch = Estirar
canvas-key-undo = Deshacer
    .description = Deshace el último cambio en el lienzo, la cola o las listas de reproducción que tienen el foco
canvas-key-redo = Rehacer
    .description = Vuelve a aplicar el cambio que acabas de deshacer
canvas-key-delete = Eliminar elemento de lienzo
    .description = Quita el elemento seleccionado con todo lo que contiene
canvas-key-group = Agrupar elemento de lienzo
    .description = Envuelve el elemento seleccionado en un grupo nuevo
canvas-key-ungroup = Desagrupar elementos de lienzo
    .description = Saca los elementos de un grupo y quita el grupo
canvas-key-nudge-left = Mover elemento a la izquierda
    .description = Mueve el elemento seleccionado una unidad a la izquierda
canvas-key-nudge-right = Mover elemento a la derecha
    .description = Mueve el elemento seleccionado una unidad a la derecha
canvas-key-nudge-up = Mover elemento arriba
    .description = Mueve el elemento seleccionado una unidad hacia arriba
canvas-key-nudge-down = Mover elemento abajo
    .description = Mueve el elemento seleccionado una unidad hacia abajo
canvas-key-nudge-left-far = Mover elemento lejos a la izquierda
    .description = Mueve el elemento seleccionado diez unidades a la izquierda
canvas-key-nudge-right-far = Mover elemento lejos a la derecha
    .description = Mueve el elemento seleccionado diez unidades a la derecha
canvas-key-nudge-up-far = Mover elemento lejos arriba
    .description = Mueve el elemento seleccionado diez unidades hacia arriba
canvas-key-nudge-down-far = Mover elemento lejos abajo
    .description = Mueve el elemento seleccionado diez unidades hacia abajo
canvas-key-step-out = Salir de la selección
    .description = Selecciona el grupo que contiene el elemento y luego nada
canvas-add-shader = Añadir shader
canvas-shader-off = Shader apagado
canvas-shader-empty = Ningún shader elegido
canvas-shader-flat-note = Este shader lee la pantalla, una imagen o su fotograma anterior, o usa más de una pasada, así que queda plano.
canvas-item-position-x = Posición X
canvas-item-position-y = Posición Y
canvas-item-position-z = Posición Z
canvas-item-sway = Vaivén
    .units = Cuánto lo desplaza una señal completa, en unidades.
    .degrees = Cuánto lo gira una señal completa, en grados.
canvas-route-target = { $item } { $property }
canvas-route-sway-quiet = Dónde queda en silencio, como fracción del vaivén del elemento en este eje, contado desde el valor del propio deslizador
canvas-route-sway-loud = Hasta dónde llega a plena señal; 100% es el vaivén entero, por debajo de Suave oscila hacia el otro lado
canvas-add-text = Añadir texto
canvas-item-on-click = Al hacer clic
    .description = Se ejecuta al hacer clic en el elemento fuera del modo de diseño.
canvas-item-on-click-none = Nada
canvas-text = Texto
canvas-text-font = Fuente
canvas-text-size = Tamaño
canvas-text-color = Color
canvas-text-flat-note = El texto queda plano frente a la pantalla y se achica con la distancia.
canvas-add-objects = Objetos
canvas-objects-empty = No hay objetos instalados
canvas-export-object = Exportar objeto
canvas-object-export-title = Exportar "{ $name }" como objeto
canvas-object-export-hint = La ficha viaja en el archivo, así que la ve cualquiera con quien compartas el objeto
canvas-object-name = Nombre
canvas-object-name-placeholder = Cómo se llama
canvas-object-description-placeholder = Qué es
canvas-object-exported = { $name } exportado
canvas-object-export-failed = No se pudo exportar { $name }
canvas-object-place-title = ¿Colocar "{ $name }"?
canvas-object-place-body = Entra en el centro del escenario. Deshacer lo vuelve a quitar.
canvas-object-remove-body = Esto elimina el objeto instalado y deja como está cada lienzo donde se colocó.
canvas-object-heavy = Ocupa { $size } MB al desempaquetarse, más de lo que suele llevar un objeto
canvas-object-shaders-body = Aprobarlos los deja ejecutarse en este equipo. Sin aprobación, cada shader queda apagado y pide aprobación donde está.
canvas-object-place = Colocar
canvas-object-approve-place = Aprobar y colocar
canvas-object-place-failed = No se pudo colocar { $name }
canvas-object-add-failed = No se pudo añadir { $file }
canvas-object-needs-canvas = { $name } añadido. Los objetos van en un lienzo, y esta disposición aún no tiene ninguno.
canvas-weight-counts = Elementos { $items }, cuadrículas { $grids }, elementos shader { $shaders }, shaders de panel { $surfaces }, cuadrículas en movimiento { $moving }, anidamiento { $depth }
canvas-weight-many-grids = Elementos de cuadrícula: { $count }, más de los que se dibujan con fluidez
canvas-weight-many-shaders = Shaders de paneles y de la ventana: { $count }, más de los que se dibujan con fluidez
canvas-case-show = Mostrar
canvas-case-same-image = La misma imagen
canvas-case-same-color = El mismo color
canvas-case-preview = Ver este caso en el lienzo
canvas-add-target = Añadir destino de render
canvas-target-renderer = Renderizador
canvas-target-renderer-none = Ninguno elegido
canvas-target-not-installed = { $id }, no instalado
canvas-target-renderer-off = { $label }, desactivado
canvas-target-renderer-broken = { $label }, no puede ejecutarse
canvas-target-resolution = Resolución
canvas-target-pixels-wide = Ancho en píxeles
canvas-target-pixels-high = Alto en píxeles
canvas-target-rests = Pausar con la música
    .description = Mientras la música está en pausa o detenida, se queda el último fotograma y no se ejecuta nada hasta que vuelva a sonar
canvas-target-status = Estado
canvas-target-starting = Arrancando
canvas-target-retrying = No pudo arrancar, se vuelve a intentar: { $error }
canvas-target-running = En marcha
canvas-target-paused = En pausa mientras está oculto o fuera del escenario
canvas-target-resting = En pausa mientras la música está en pausa o detenida
canvas-target-stopped = Detenido
canvas-target-refused-missing = Detenido: no hay ningún renderizador instalado con este id
canvas-target-refused-broken = Detenido: el renderizador no carga. La página Renderizadores dice por qué
canvas-target-refused-not-renderer = Detenido: el paquete con este id no es un renderizador
canvas-target-refused-not-approved = Detenido: es nuevo o cambió en el disco, y hay que volver a activarlo
canvas-target-refused-off = Detenido: está desactivado
canvas-target-refused-stopped = Detenido tras cerrarse cinco veces en diez minutos
canvas-target-refused-size = Detenido: un renderizador dibuja desde { $min_width } por { $min_height } hasta { $max_width } por { $max_height } píxeles
canvas-target-refused-start = Detenido: no pudo arrancar. { $error }
canvas-target-start-again = Volver a arrancar
canvas-target-switch-on = Activar...
canvas-target-open-settings = Abrir ajustes de renderizadores
canvas-target-signals = Señales
canvas-target-signals-missing = Aún no están en el conjunto: { $signals }
canvas-target-signals-add = Añadir señales
canvas-target-signal-named = { $name } ({ $signal })
canvas-target-signals-title = ¿Añadir las señales que sigue { $name }?
canvas-target-signals-body = El conjunto aún no las tiene. Una vez añadidas, la ventana Señales puede cambiarlas o borrarlas.
canvas-target-signals-not-now = Ahora no
canvas-target-signals-confirm = Añadir
canvas-target-panel-empty = Ningún renderizador elegido
canvas-target-panel-pick = Elegir un renderizador
canvas-target-panel-none = No hay renderizadores instalados
canvas-target-panel-renderer = Renderizador
    .description = El programa que dibuja en este panel
canvas-target-panel-sizing = Tamaño
    .description = Una resolución fija o el tamaño del panel. Al seguir el panel, el renderizador vuelve a arrancar medio segundo después de un cambio de tamaño
canvas-target-panel-fixed = Fijo
canvas-target-panel-match = Igual que el panel
canvas-target-panel-size = Ancho y alto en píxeles
canvas-target-panel-scaling = Escala
    .description = Contener muestra toda la imagen con bandas alrededor. Cubrir llena el panel y recorta lo que no cabe
canvas-target-panel-fit = Contener
canvas-target-panel-fill = Cubrir
canvas-weight-targets = Destinos de render { $count }, { $megapixels } megapíxeles
canvas-weight-heavy-targets = Destinos de render: { $megapixels } megapíxeles, más de los que se dibujan con fluidez

## Catalog

catalog-window-title = Catálogo
menu-catalog = Catálogo de espacios de trabajo y plugins
keymap-open-catalog = Catálogo
    .description = Explora espacios de trabajo, plugins y objetos para instalar
catalog-placeholder = Buscar por nombre, autor, resumen o etiqueta
catalog-tab-workspaces = Espacios de trabajo
catalog-tab-plugins = Plugins
catalog-tab-objects = Objetos
catalog-tab-updates = Actualizaciones
catalog-kind-workspace = Espacio de trabajo
catalog-kind-plugin = Plugin
catalog-kind-object = Objeto
catalog-loading = Cargando el catálogo...
catalog-unreachable = No se pudo llegar al catálogo
catalog-saved-copy = No se pudo llegar al catálogo, así que esta es la copia guardada la última vez
catalog-retry = Reintentar
catalog-empty = Aún no hay nada aquí
catalog-none = Nada en el catálogo coincide con "{ $text }"
catalog-more = Se muestran los primeros { $shown }. Busca para acotarlos.
catalog-pick = Elige un paquete para verlo aquí
catalog-by = de { $author }
catalog-installed = Instalado
catalog-install = Instalar
catalog-update = Actualizar
catalog-update-available = Actualización disponible
catalog-cancel = Cancelar
catalog-downloading = { $done } de { $total }
catalog-busy = Primero se instala { $name }
catalog-install-failed = No se pudo instalar: { $reason }
catalog-installed-quiet = Instalado y guardado. No hay ninguna ventana de rox abierta donde ofrecerlo.
catalog-replace = Reemplazar
catalog-keep-both = Conservar ambos
catalog-update-choice-workspace = Reemplazar sobrescribe tu copia. Conservar ambos guarda esta junto a ella.
catalog-update-choice-object = Reemplazar sobrescribe tu copia. Conservar ambos guarda esta junto a ella. En los dos casos se ofrece a tu lienzo como una copia nueva, y las copias que ya están en él se quedan como están.
catalog-object-updated = Actualizado y ofrecido a tu lienzo como una copia nueva. Las copias que ya están en él se quedan como están.
catalog-replace-failed = No se pudo reemplazar, así que tu copia no ha cambiado: { $reason }
catalog-record-loading = Cargando los detalles...
catalog-record-failed = No se pudieron cargar los detalles: { $reason }
catalog-version = Versión
catalog-published = Publicado
catalog-license = Licencia
catalog-size = Tamaño
catalog-tags = Etiquetas
catalog-needs = Necesita
catalog-needs-missing = { $id } (no instalado)
catalog-homepage = Sitio web
catalog-notes = En esta versión
catalog-link-unknown = El catálogo no tiene ningún paquete llamado { $id }
catalog-link-too-new = { $name } necesita un rox más nuevo que este
catalog-why-no-connection = no hay conexión con el servidor del catálogo
catalog-why-timed-out = el servidor del catálogo tardó demasiado en responder
catalog-why-refused = el servidor del catálogo rechazó la solicitud ({ $status })
catalog-why-redirected = el servidor del catálogo envió una redirección, y rox no sigue ninguna aquí
catalog-why-too-large = la respuesta del catálogo es más grande de lo que rox acepta
catalog-why-wrong-size = la descarga no tiene el tamaño que indica el catálogo
catalog-why-wrong-checksum = la descarga no coincide con la suma de comprobación del catálogo
catalog-why-cancelled = se canceló la instalación
catalog-why-unreadable = la respuesta del catálogo no se puede leer ({ $detail })
catalog-why-no-index = el catálogo aún no se ha cargado
catalog-why-no-package = el catálogo no tiene ningún paquete llamado { $id }
catalog-why-too-new = necesita rox { $version } o posterior
catalog-why-busy = otra instalación sigue en curso
catalog-why-not-package = la descarga no es un paquete que rox pueda leer ({ $detail })
catalog-why-interrupted = la instalación se detuvo antes de terminar
catalog-updates-chip = { $count ->
    [one] { $count } actualización del catálogo
   *[other] { $count } actualizaciones del catálogo
}
catalog-updates-chip-tip = Versiones más nuevas de lo instalado desde el catálogo
