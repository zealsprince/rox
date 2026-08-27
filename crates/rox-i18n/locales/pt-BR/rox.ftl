### Português do Brasil. Espelha en-CA/rox.ftl chave por chave; o
### teste de paridade em rox-i18n garante isso. As chaves são
### kebab-case com prefixo de superfície; a descrição de uma linha é
### um atributo da mensagem do rótulo.

## Shared widgets
tracking-title = Acompanhamento
tracking-follow = Seguir reprodução
tracking-resume = Retomar quando ocioso
tracking-smooth = Rolagem suave
align-row = Alinhamento
    .description = Onde o conteúdo fica quando o painel tem espaço de sobra
valign-row = Alinhamento vertical
    .description = Onde o conteúdo fica quando o painel tem altura de sobra
valign-top = Topo
valign-middle = Meio
valign-bottom = Base

## Panel source and search rows
source-track = Faixa
    .description = Seguir o que está tocando, ou o que está selecionado na biblioteca
source-follow-playing = Seguir reprodução
source-follow-selection = Seguir seleção
source-playing = Tocando
source-selected = Selecionado
query-search = Busca
query-search-box = Campo de busca
    .description = Mostrar o campo de busca; a consulta só vale enquanto ele aparece
query-source = Fonte da busca
    .description = Seguir a busca compartilhada, filtrar pelo campo do próprio painel, ou mostrar o que outro painel tem selecionado
query-source-shared = Compartilhada
query-source-own = Própria
query-source-selection = Seleção

## Signals and routes
signal-source = Fonte
    .description = O que o sinal segue: Banda acompanha uma faixa de frequência, Nível a mistura inteira, Onset pulsa a cada batida na faixa, Gatilho dispara um pulso quando a faixa atinge seu limiar, Total soma outro sinal ao longo do tempo
signal-kind-band = Banda
signal-kind-level = Nível
signal-kind-onset = Onset
signal-kind-trigger = Gatilho
signal-kind-total = Total
signal-response = Resposta
signal-response-pulse = Quanto tempo cada pulso ressoa antes de se apagar
signal-response-drift = 0 gruda na música, 100 vem arrastado atrás dela
signal-threshold = Limiar
signal-threshold-trigger = O nível que a faixa precisa alcançar para disparar o pulso; ele só dispara de novo depois que o nível cai abaixo da marca no medidor acima
signal-threshold-gate = Abaixo disso o sinal é lido como zero, e acima dele a saída volta a subir a partir do zero, para que os trechos quietos não mexam no controle. A marca no medidor acima mostra onde ele está
signal-low-bound = Limite inferior
signal-high-bound = Limite superior
signal-adds-up = Soma
    .description = Qual sinal este total acumula; ele sobe enquanto aquele está alto e empaca enquanto está quieto
signal-aggregate-nothing = Nada para seguir
signal-aggregate-pick = Escolha um sinal
signal-aggregate-alone = Não há outro sinal no conjunto para somar, então este fica em zero. Adicione um e ele aparece na lista.
signal-aggregate-unpicked = Nada escolhido, então este total fica em zero. Escolha um sinal acima.
signal-rate = Taxa
    .description = Voltas por segundo com a entrada no máximo; passa de 1, volta para 0 e continua subindo, o que um shader lê como fase
signal-reset-on-track = Zerar a cada faixa
    .description = Voltar a zero quando uma música nova começa, para que uma fase não comece a partir do total da anterior
signal-flush = Esvaziar
signal-routes-in-panel = { $count ->
    [one] { $count } rota neste painel
   *[other] { $count } rotas neste painel
}
    .description = Mandar de volta a zero agora. Ele escoa aos poucos em vez de saltar, para que nada que o siga dê um pulo
route-header = Rota
route-signal = Sinal
    .description = Qual sinal compartilhado esta rota segue; ajustar aqui ajusta todas as rotas nele
route-new-signal = Novo sinal
route-shared-note = Compartilhado por todas as rotas neste sinal
route-signal-gone = O sinal desta rota sumiu; o controle mantém o valor da barra até que outro seja escolhido acima.
route-range-note = Faixa apenas para este parâmetro
route-quiet = Quieto
    .description = O que o controle lê no silêncio, como fração do próprio ajuste
route-loud = Alto
    .description = O que ele lê com o sinal cheio; 100% é o valor da própria barra, abaixo de Quieto modula para baixo
route-slot = Slot
    .description = Qual dos dezesseis slots de sinal do shader esta rota preenche
route-slot-quiet-description = O que o slot lê no silêncio
route-slot-loud-description = O que ele lê com o sinal cheio; abaixo de Quieto o slot roda ao contrário
route-slot-signal-description = Qual sinal compartilhado esta rota segue
route-slot-signal-gone = O sinal desta rota sumiu; o slot lê zero até que outro seja escolhido.
route-add = Adicionar rota
route-unrouted = Sem rota
route-pick-slot = Escolha um slot
route-pick-signal = Escolha um sinal
route-no-signal = sem sinal
route-no-signals-yet = Ainda não há sinais para seguir. Crie um e ele aparece aqui; até lá o slot lê zero.
route-open-signals = Abrir sinais
route-create-signal = Criar novo sinal

## Panel settings window
panel-settings = Configurações do painel
panel-menu-label = Painel
panel-save-as-preset = Salvar como predefinição
panel-rename = Renomear
panel-rename-name = Nome
panel-rename-note = Aparece como a aba do painel; vazio volta ao nome original
panel-rename-hint-after = para renomear
panel-was-closed = O painel foi fechado
panel-reset = Redefinir
panel-inverse = Inverter
panel-apply-song-theme = Aplicar tema da música
panel-page-appearance = Aparência
panel-page-behavior = Comportamento
panel-page-shader = Shader
panel-section-placement = Posicionamento
panel-section-size = Tamanho
panel-section-opacity = Opacidade
panel-section-frame = Moldura
panel-section-colors = Cores
panel-section-font = Fonte
panel-section-shader = Shader
panel-section-signals = Sinais
panel-section-slots = Slots
panel-awaiting-approval = Aguardando aprovação
panel-size-off = Desligado
panel-locked = Travado
    .description = Fixar o painel no lugar; ele não pode ser arrastado nem reorganizado no dock
panel-drag-anchor = Âncora de arraste
    .description = Arrastar em qualquer ponto do painel move a janela, enquanto os cliques simples continuam caindo nos controles dele; para layouts sem decoração de janela
panel-slot-controls = Controles de slot
    .description = Mostrar os botões de canto para trocar e remover os painéis que este hospeda. Ocultos, o layout ainda é editado pela árvore na página Espaço de trabalho das Configurações
panel-min-width = Largura mínima
    .description = Onde um redimensionamento para de espremer o painel. Vale como está escrito, inclusive abaixo do piso do próprio painel, então uma tira compacta pode ficar mais estreita que o padrão; vazio deixa o piso em paz
panel-max-width = Largura máxima
    .description = Limitar a largura do painel para que ele não estique quando a janela alarga
panel-min-height = Altura mínima
    .description = Onde um redimensionamento para de encurtar o painel. Vale como está escrito, inclusive abaixo do piso do próprio painel, então uma tira compacta pode ficar mais apertada que o padrão; vazio deixa o piso em paz
panel-max-height = Altura máxima
    .description = Limitar a altura do painel para que ele não estique quando a janela cresce
panel-own-opacity = Opacidade própria da superfície
    .description = Dar a este painel uma opacidade própria sobre o fundo, em vez da do aplicativo
panel-surface-opacity = Opacidade da superfície
panel-margin = Margem
    .description = Recuar o painel dentro da célula dele, com o fundo aparecendo pela fresta
panel-padding = Espaçamento interno
    .description = Espaço dentro da borda do painel, mantido no fundo dele mesmo
panel-rounding = Arredondamento
    .description = Arredondar os cantos do painel para dentro do fundo
panel-border = Borda
    .description = Uma linha em volta do painel, na cor definida para a Borda; um lado em zero não desenha nada
panel-font = Fonte
    .description = A tipografia do painel; o padrão segue a fonte do aplicativo
panel-font-size = Tamanho da fonte
    .description = O tamanho do texto do painel em relação à fonte do aplicativo; as linhas escalam junto
panel-surface-shader = Shader de superfície
    .description = Rodar um shader WGSL sobre o corpo deste painel, abaixo do shader de tela do aplicativo
panel-run-when-idle = Executar em silêncio
    .description = Continuar desenhando quadros enquanto o áudio está em silêncio. Desligado, o shader congela no último quadro e o painel não custa nada
panel-shader-is-scene = Este shader é uma cena, então ele cobre o corpo do painel em vez de desenhar por cima. Veio de um pacote ou de uma configuração antiga; a lista acima só oferece shaders que deixam o painel legível.

## Shader picker and saving
shader-source = Fonte
shader-pick-none = Nenhum
shader-reload = Recarregar
shader-edit-as-file = Editar como arquivo
shader-make-private-copy = Fazer cópia privada
shader-save-replace = Substituir
shader-save-to-workspace = Salvar no espaço de trabalho
shader-save-replaces = Substitui o shader que este espaço de trabalho já chama de { $name }. Todo painel que usa esse nome muda junto
shader-save-adds = Adiciona aos shaders deste espaço de trabalho sob { $name }. Qualquer painel pode usar, e editar atualiza todos
shader-group-examples = Exemplos
shader-group-this-workspace = Este espaço de trabalho
shader-group-scenes = Cenas
shader-group-workspace-scenes = Cenas do espaço de trabalho
shader-group-overlays = Overlays
shader-group-workspace-overlays = Overlays do espaço de trabalho

## Saving a panel preset
preset-save = Salvar predefinição
preset-save-name = Nome da predefinição
preset-save-replaces = Substitui a predefinição que este espaço de trabalho já chama de { $name }
preset-save-hint-after = para salvar
preset-back-from = Traga de volta por
preset-back-add-panel = Adicionar painel
preset-back-then = depois
preset-back-presets = Predefinições
preset-back-tail = em qualquer menu de painel. As predefinições pertencem só a este espaço de trabalho; outro não vai tê-las.

## Keyboard hints
hint-press = Pressione
hint-key-enter = Enter

## Settings: language
settings-language = Idioma
    .description = O idioma da interface. Sistema compara com a lista do sistema operacional e cai no inglês quando nada bate
    .keywords = idioma traducao localizacao lingua
settings-language-system = (Idioma do sistema)
settings-language-search = Buscar idiomas
picker-no-matches = Nenhum resultado
settings-search-no-matches = Nada corresponde a "{ $text }"

## Embed dialog
bake-window-title = rox - Incorporar metadados salvos
bake-title = Incorporar metadados salvos
bake-intro = Escreve os metadados salvos nos próprios arquivos, para que outro player também os leia. Nada é recalculado.
bake-formats = Só MP3 e FLAC; outros formatos e faixas de CUE são pulados
bake-source-lyrics = Letras
bake-source-gain = ReplayGain
bake-source-acoustic = Descrições acústicas
bake-detail-nothing = nada salvo para incorporar
bake-detail-only-skipped = nada a escrever, { $skipped } a pular
bake-detail-writes = { $count ->
    [one] { $count } arquivo a escrever
   *[other] { $count } arquivos a escrever
}
bake-detail-writes-skipped = { $count ->
    [one] { $count } arquivo a escrever, { $skipped } a pular
   *[other] { $count } arquivos a escrever, { $skipped } a pular
}
bake-error-read = Não foi possível ler a biblioteca: { $error }
bake-survey-counting = Vasculhando a biblioteca...
bake-survey-progress = Lendo tags, { $done } de { $total }
bake-nothing-to-embed = Nada para incorporar: os arquivos já têm tudo que o rox guardou
bake-rewrites = { $count ->
    [one] { $count } arquivo será reescrito
   *[other] { $count } arquivos serão reescritos
}
bake-hint-before = Pressione
bake-hint-key = Enter
bake-hint-after = para incorporar
bake-embed = Incorporar
bake-cancel = Cancelar
bake-summary-files = { $count ->
    [0] { $count } arquivos
    [one] 1 arquivo
   *[other] { $count } arquivos
}
bake-summary-updated = Atualizou { $files }
bake-summary-stopped = Parou depois de atualizar { $files }
bake-summary-skipped = { $count ->
    [0] , { $count } pulados
    [one] , { $count } pulado
   *[other] , { $count } pulados
}
bake-summary-failed = , { $count } com falha

## Arrange editors and header pieces
arrange-shown = Visível
arrange-hidden = Oculto
tile-face-mosaic = Mosaico de capas
tile-face-tinted = Mosaico tingido
tile-face-gradient = Cartão gradiente
tile-face-color = Cartão de cor
head-piece-artist = Artista
head-piece-album = Álbum
head-piece-year = Ano
head-piece-genre = Gênero
head-piece-quality = Qualidade
head-piece-tracks = Faixas
head-piece-time = Tempo
head-piece-spacer = Espaçador
head-piece-divider = Divisor
head-piece-art = Capa
head-unknown = Desconhecido
status-item-count = Contagem
status-item-time = Tempo
status-item-albums = Álbuns
status-item-artists = Artistas
status-item-plays = Reproduções
volume-item-icon = Ícone
volume-item-slider = Barra
volume-item-percent = Porcentagem

## Filter chips and search menus
filter-field-artist = Artista
filter-field-album-artist = Artista do álbum
filter-field-album = Álbum
filter-field-genre = Gênero
filter-field-year = Ano
filter-field-folder = Pasta
filter-unknown = Desconhecido
filter-clear = Limpar
query-show-search-box = Mostrar campo de busca
query-own-query = Busca própria
query-shared-query = Busca compartilhada
headers-off = Desligado
headers-compact = Compacto
headers-expanded = Expandido

## Panel context menu
panel-dock-back = Reacoplar
panel-pop-out = Destacar
panel-close = Fechar
panel-duplicate = Duplicar
panel-reveal-in-browser = Mostrar no gerenciador de arquivos
panel-play-next = Tocar em seguida
panel-add-to-queue = Adicionar à fila
panel-add-to-playlist = Adicionar à playlist
panel-favourite-add = Adicionar aos favoritos
panel-favourite-remove = Remover dos favoritos
shader-pick-missing = { $name } (ausente)
shader-pick-custom = Personalizado

## Shipped shader examples
shader-blurb-plasma = Cor à deriva tirada só dos seus uniforms, então custa um quad simples.
shader-blurb-trails = Borra o próprio quadro anterior, então roda no passe de tela.
shader-blurb-sheen = Uma vinheta e um brilho que passeia, overlay transparente para um painel que já desenha.
shader-blurb-shadow = Uma sombra projetada pelo próprio texto e pelos controles do painel, lida da captura da máscara.
shader-blurb-cover = A capa da faixa que está tocando, em letterbox sobre um banho da própria cor dela.
shader-blurb-badge = A capa como um cartãozinho parado num canto, com um slot para movê-la de lugar.
shader-blurb-lamp = Uma luz que segue o cursor e responde a cliques, overlay transparente.
shader-blurb-cube = Um cubo em wireframe cambaleando num 3D falso, desenhado como luz aditiva.
shader-blurb-bloom = Orbes à deriva com bloom por um segundo passe pela metade do tamanho, a cadeia em miniatura.
shader-blurb-tube = Reproduz o painel abaixo através de uma tela curva de CRT, scanlines e tudo.

## Transport strip pieces
seek-item-elapsed = Decorrido
seek-item-strip = Barra
seek-item-ending = Restante
seek-item-duration = Duração
info-item-track-no = Nº da faixa
info-item-title = Título
info-item-duration = Duração
info-item-next = Próxima
info-item-queued = Na fila
info-item-output = Saída
info-item-favourite = Favorito
info-item-rating = Avaliação
playback-item-previous = Anterior
playback-item-seek-back = Retroceder
playback-item-play = Reproduzir
playback-item-seek-forward = Avançar
playback-item-next = Próxima
playback-item-stop = Parar
playback-item-volume = Volume
playback-item-loop = Repetir
playback-item-shuffle = Embaralhar
playback-item-continue = Continuar
playback-item-crossfade = Crossfade
playback-item-random = Aleatório
playback-item-stop-after = Parar depois
playback-item-favourite = Favorito
playback-item-rating = Avaliação

## Dock chrome
dock-empty-tab = Aba vazia
dock-unnamed = Sem nome
dock-tiles = Blocos
dock-zoom-in = Aproximar
dock-zoom-out = Afastar
dock-collapse = Recolher
dock-expand = Expandir

## Shader picker notes
shader-note-empty = Escolha um exemplo para começar, ou aponte o rox para um arquivo .wgsl com um estágio de fragmento definindo fs_user(uv)
shader-note-missing = { $name } não está mais nos shaders deste espaço de trabalho, então nada é pintado. Escolha outra coisa aqui e este painel ganha uma fonte própria.
shader-note-shared = Compartilhado por este espaço de trabalho. Editar atualiza todas as superfícies que o usam.
shader-note-file = { $path }. O que você salva é recarregado enquanto o shader desenha, e a fonte fica guardada dentro dos layouts e pacotes, então ela continua funcionando numa máquina que nunca teve o arquivo.
shader-note-custom = Esta fonte fica guardada dentro do layout ou pacote dela, sem arquivo por trás. Editar como arquivo escreve a fonte para fora e passa a acompanhar o que você salvar.

## Panel pages and shared sides
panel-page-layout = Layout
panel-page-view = Visualização
panel-page-content = Conteúdo
panel-page-source = Fonte
panel-page-bindings = Vínculos
panel-page-emitters = Emissores
panel-page-forces = Forças
panel-page-scene = Cena
side-left = Esquerda
side-right = Direita
genre-face-mosaic = Mosaico
genre-face-tinted = Tingido
genre-face-gradient = Gradiente
genre-face-color = Cor

## Library panel
panel-title-library = Biblioteca
library-play = Reproduzir
library-play-album = Reproduzir álbum
library-play-group = Reproduzir grupo
library-play-tracks = Reproduzir { $count } faixas
library-play-similar = Reproduzir similares
library-filter-by-album = Filtrar por álbum
library-filter-by-artist = Filtrar por artista
library-jump-to-playing = Ir para a faixa tocando
library-menu-display = Exibição
library-disc = Disco { $number }
library-empty-title = Abra uma pasta de música
library-empty-note = Ela é escaneada para a biblioteca (flac, mp3, wav)
library-headers = Cabeçalhos
    .description = Quebras de grupo sobre a lista; uma ordenação mantém juntas as sequências, e a busca mostra tudo plano
library-group-by = Agrupar por
    .description = No que os cabeçalhos quebram; gênero e ano reordenam a lista
library-header-row = Linha de cabeçalho
    .description = O que os cabeçalhos de uma linha mostram, da esquerda para a direita; um espaçador ou divisor separa os lados
library-header-lines = Linhas do cabeçalho
    .description = As linhas do bloco, de cima para baixo; uma linha vazia some
library-follow-description = Rolar até a linha que está tocando sempre que a faixa muda
library-resume-description = Rolar de volta para a linha que está tocando depois que você para de navegar
library-smooth-description = Deslizar até a linha em vez de saltar
library-columns = Colunas
    .description = Quais colunas aparecem; arraste os cabeçalhos no painel para reordenar e redimensionar
library-column-headers = Cabeçalhos de coluna
    .description = A linha de cabeçalho ordenável sobre a lista; oculta, as colunas mantêm a ordem e a largura
library-compact-plays = Reproduções compactas
    .description = A coluna de reproduções como um número pequeno com um traço ao lado
library-line-height = Altura da linha
    .description = Uma linha de cabeçalho; os blocos ocupam as linhas de que precisam, independentes das linhas de faixa
library-text-size = Tamanho do texto
    .description = O texto das linhas de cabeçalho, independente da altura da linha, para que a capa cresça sozinha
library-flush-background = Fundo rente
    .description = Colocar os cabeçalhos no fundo da lista em vez do tom elevado; o tema da música move os dois juntos
library-gap-above = Espaço acima
    .description = Recortado do topo do bloco; a lista aparece por ali, e as linhas se apertam para caber
library-gap-below = Espaço abaixo
    .description = O mesmo abaixo do bloco, antes das faixas dele
library-section-rows = Linhas
library-row-height = Altura da linha
    .description = As linhas de faixa; o texto acompanha, e os dois escalam com a fonte do aplicativo
library-row-spacing = Espaçamento entre linhas
    .description = Altura extra que cada linha ocupa; folga sem aumentar o texto
library-stripes = Destaque alternado
    .description = Tingir uma linha de faixa sim, outra não, para que uma lista longa se leia melhor
library-row-borders = Bordas de linha
    .description = O fio de cabelo abaixo de cada linha de faixa
library-art-description = A miniatura dos cabeçalhos expandidos: a capa, o retrato do artista ou a face do gênero
library-art-rounding = Arredondamento da capa
    .description = Arredondar os cantos da capa
library-art-position = Posição da capa
    .description = De que lado do bloco fica a miniatura dos cabeçalhos expandidos
library-art-margin = Margem da capa
    .description = Recuar a miniatura dentro do bloco; ela encolhe para continuar quadrada
library-circular-portraits = Retratos circulares
    .description = Agrupado por artista, arredondar as miniaturas até o círculo completo do mural em vez de usar o controle de arredondamento
library-genre-face = Face do gênero
    .description = Agrupado por gênero, o que a miniatura mostra: as capas, as capas banhadas na cor do gênero, ou um cartão de cor sob a geometria dele

## Album grid panel
panel-title-album-grid = Grade de álbuns
grid-menu-scroll = Rolagem
grid-vertical-scroll = Rolagem vertical
grid-horizontal-scroll = Rolagem horizontal
grid-jump-to-playing = Ir para o álbum tocando
grid-library-empty = A biblioteca está vazia
grid-play-albums = Reproduzir { $count } álbuns
grid-vertical-layout = Layout vertical
    .description = Rolar o mural para cima e para baixo, com as linhas preenchendo a largura; desligado, ele rola para os lados, com as colunas preenchendo a altura
grid-follow-description = Rolar até o álbum que está tocando sempre que a faixa muda
grid-resume-description = Deslizar de volta para o álbum que está tocando depois que você para de navegar
grid-smooth-description = Deslizar até o álbum em vez de saltar
grid-section-dimming = Escurecimento
grid-section-tiles = Blocos
grid-dim-while-playing = Escurecer durante a reprodução
    .description = Apagar todas as capas menos a do álbum tocando; passar o mouse acende um bloco de novo
grid-dim-amount = Intensidade
    .description = Quanto as outras capas apagam; 100% as esconde
grid-desaturate = Dessaturar durante a reprodução
    .description = Deixar todas as capas menos a do álbum tocando em tons de cinza; passar o mouse traz a cor de um bloco de volta
grid-always = Sempre
    .description = Manter as capas recuadas mesmo quando nada toca; só um bloco sob o mouse aparece por inteiro
grid-show-titles = Mostrar títulos
    .description = Imprimir o álbum e o artista sob cada capa, no estilo iTunes, em vez de só ao passar o mouse
grid-title-alignment = Alinhamento dos títulos
    .description = Alinhar as legendas sob suas capas
grid-tile-size = Tamanho do bloco
    .description = A maior aresta dos blocos de capa; as colunas dividem a largura do painel por igual
grid-gap = Espaço
    .description = Espaço entre as capas; zero as encosta umas nas outras
grid-art-rounding-description = Arredondar os cantos de cada capa; 100% é um círculo

## Settings: sidebar pages
settings-page-appearance = Aparência
settings-page-application = Aplicativo
settings-page-audio = Áudio
settings-page-development = Desenvolvimento
settings-page-integrations = Integrações
settings-page-keymap = Atalhos
settings-page-library = Biblioteca
settings-page-mcp = MCP
settings-page-ml-models = Modelos de ML
settings-page-playback = Reprodução
settings-page-providers = Provedores
settings-page-shader = Shader
settings-page-storage = Armazenamento
settings-page-workspace = Espaço de trabalho

## Settings: appearance
settings-appearance-backdrop-all-windows = Todas as janelas
    .description = Colocar o fundo também nas janelas filhas: configurações, editores, diálogos, painéis destacados. Desligado, o fundo e a transparência ficam só nas janelas do espaço de trabalho
settings-appearance-backdrop-strength = Intensidade do fundo
    .description = O quanto o fundo com a capa aparece atrás delas
settings-appearance-border = Borda
    .description = Uma linha em volta de cada painel, na cor definida para a Borda; um lado em zero não desenha nada
settings-appearance-colors-locked-note = O tema da música está ligado, então a faixa que toca comanda estas cores e a exportação as salva. Desligue acima para editá-las
settings-appearance-design-mode = Modo de design
    .description = Editar o layout onde ele está: as opções de adicionar, renomear, duplicar, destacar e fechar nos menus dos painéis, os controles que um contêiner flutua sobre seus slots, e o arraste das abas. Desligado, tudo isso fica oculto; a página Espaço de trabalho continua editando a árvore
    .keywords = editar layout reorganizar bloquear design
settings-appearance-font = Fonte
    .description = A tipografia de todo o aplicativo; os painéis podem sobrescrevê-la nas configurações deles
    .keywords = fonte tipografia tipo texto
settings-appearance-font-size = Tamanho da fonte
    .description = O tamanho base a partir do qual o texto de cada painel escala; controles e ícones mantêm o tamanho
settings-appearance-hide-menubar = Ocultar a barra de menus
    .description = Manter a barra de menus oculta, flutuando sobre o dock enquanto o alt estiver pressionado. Dois toques no alt a deixam à mostra, para que os botões aceitem um clique simples
settings-appearance-icons-intro = Um pacote é uma pasta de SVGs que substitui os ícones internos; a troca vale no próximo início
settings-appearance-icons-open-folder = Abrir pasta
settings-appearance-inverse-from-dark = Inverter a partir do tema escuro
settings-appearance-inverse-from-light = Inverter a partir do tema claro
settings-appearance-keep-theme = Manter o tema
    .description = Segurar o tema ativo mesmo quando o brilho de uma capa o viraria; o tema da música continua tingindo a cor
settings-appearance-margin = Margem
    .description = Recuar cada painel dentro da célula dele; um painel pode sobrescrever isso nas configurações dele
settings-appearance-new-pack = Novo pacote
settings-appearance-os-decorations = Decorações do sistema
    .description = A barra de título e as bordas do sistema nas janelas principais; desligado, tudo depende dos controles de janela e dos painéis com âncora de arraste
settings-appearance-pack-name-placeholder = Nome do pacote
settings-appearance-padding = Espaçamento interno
    .description = Espaço dentro da borda de cada painel, mantido no fundo dele mesmo
settings-appearance-palette-export = Exportar
settings-appearance-palette-import = Importar
settings-appearance-panel-seams = Costuras dos painéis
    .description = O fio de cabelo entre os blocos de painel; desligado, as alças de redimensionamento ficam invisíveis, mas continuam arrastáveis
settings-appearance-resize-border = Borda de redimensionamento
    .description = Redimensionar as janelas principais arrastando as bordas; só vale com as Decorações do sistema desligadas, e desligar isso deixa o encaixe e o Win+seta como o jeito de redimensionar
settings-appearance-rounding = Arredondamento
    .description = Arredondar os cantos de cada painel para dentro do fundo
settings-appearance-section-colors = Cores
settings-appearance-section-frame = Moldura
settings-appearance-section-icons = Ícones
settings-appearance-section-interface = Interface
settings-appearance-section-theming = Tematização
settings-appearance-section-transparency = Transparência
settings-appearance-section-typography = Tipografia
settings-appearance-song-theming = Tema da música
    .description = Tingir a paleta e colocar a capa da faixa que toca como fundo das janelas
settings-appearance-surface-opacity = Opacidade da superfície
    .description = O quanto as superfícies do aplicativo cobrem o fundo
settings-appearance-theme = Tema
    .description = A paleta que o aplicativo desenha e a que o editor de cores abaixo mira; Sistema segue a preferência clara ou escura do sistema operacional
settings-appearance-theme-dark = Escuro
settings-appearance-theme-light = Claro
settings-appearance-theme-system = Sistema

## Settings: application
settings-application-check-updates = Verificar atualizações
    .description = Procurar uma versão mais nova uma vez por dia quando o rox inicia; a janela Sobre verifica na hora de qualquer jeito
settings-application-download-updates = Baixar atualizações
    .description = Quando uma verificação encontra uma versão mais nova, baixar e deixar pronta em segundo plano; o próximo início a executa
settings-application-enable-ai = Ativar recursos de IA
    .description = Deixar ferramentas de IA conversarem com o rox: adiciona suporte a MCP e os downloads de modelos de ML, com as páginas deles entrando na barra lateral.
settings-application-lock-panel-resize = Travar o redimensionamento dos painéis
    .description = As divisões dos painéis só mudam de tamanho com o Modo de design ligado, para que um arraste perto de uma costura não desloque um layout pronto
settings-application-portable-copying = Copiando dados...
settings-application-portable-mode = Modo portátil
    .description = Manter configurações, biblioteca e caches numa pasta rox-data ao lado do executável, para que o player viaje com os dados dele. Desligar volta para a pasta do sistema e deixa a rox-data onde está
settings-application-portable-not-writable = A pasta do aplicativo não permite escrita
settings-application-portable-restart-note = Vale a partir do próximo início; esta execução continua na pasta atual
settings-application-remain-in-tray = Continuar na bandeja
    .description = Manter a música tocando quando a última janela fecha, com o ícone da bandeja (o dock no macOS) como o caminho de volta
settings-application-section-ai = IA
settings-application-section-control-socket = Socket de controle
settings-application-section-data = Dados
settings-application-section-layout = Layout
settings-application-section-startup = Início
settings-application-section-window = Janela
settings-application-socket-path = Caminho do socket
    .description = A interface de máquina do rox enquanto ele roda: JSON-RPC sobre um socket local, atrelado a esta pasta de dados. O roxctl a controla pelo terminal, e o proxy rox-mcp atende clientes MCP por ela

## Settings: audio
settings-audio-broadcast-bitrate = Bitrate
    .description = O que o codificador MP3 gasta por segundo de transmissão
settings-audio-broadcast-enable = Transmitir para o Icecast
    .description = Empurrar o que o rox toca para um servidor icecast como cliente de origem, codificado em MP3. O mount, os ouvintes e a cara pública são todos do icecast; o rox só conecta para fora, e um servidor inalcançável nunca encosta na reprodução local
settings-audio-broadcast-host-placeholder = host do icecast
settings-audio-broadcast-login = Login de origem
    .description = As credenciais de origem do icecast, o usuário e a senha que a configuração dele define
settings-audio-broadcast-mount = Mount
    .description = O mount que os ouvintes sintonizam, e o nome da transmissão que ele anuncia
settings-audio-broadcast-name-placeholder = Nome da transmissão
settings-audio-broadcast-password-placeholder = Senha de origem
settings-audio-broadcast-server = Servidor
    .description = O host e a porta do servidor icecast; o protocolo de origem roda sobre um socket simples
settings-audio-broadcast-user-placeholder = source
settings-audio-crossfade = Crossfade
    .description = Quanto tempo uma faixa se sobrepõe à seguinte. O crossfade é para o embaralhar e os pulos, então as transições do próprio álbum ficam intactas a menos que a linha abaixo diga o contrário. Zero desliga
    .keywords = crossfade transicao sobreposicao sem intervalo
settings-audio-equalizer-note = Dez bandas de oitava sobre a saída. Ele abre numa janela própria, já que se mexe nele com a música tocando em vez de ajustar uma vez só
settings-audio-exclusive-mode = Modo exclusivo
    .description = Tomar o dispositivo só para o rox e rodá-lo na taxa do próprio arquivo onde o hardware aceitar; desligado, ele divide o mixer do sistema com todo o resto do desktop
settings-audio-fade-inside-albums = Cruzar dentro dos álbuns
    .description = Sobrepor também as faixas que pertencem ao mesmo disco. Desligado, as emendas de um disco ficam exatamente como foram masterizadas, que é onde a reprodução sem intervalo mais importa
settings-audio-open-equalizer = Abrir equalizador
settings-audio-output-buffer = Buffer
    .description = Quanto áudio a placa segura de cada vez. Mais curto reage mais rápido e estala mais cedo numa máquina ocupada; mais longo é mais seguro e mais preguiçoso
settings-audio-output-buffer-default = Padrão (10 ms)
settings-audio-output-device = Dispositivo
    .description-default = O padrão do sistema segue o que o desktop estiver usando
    .description-linux = O exclusivo toma uma placa direto do kernel, então a lista traz placas de som em vez das saídas do desktop. Bluetooth e outros dispositivos de servidor de som não têm placa para tomar e só aparecem com o exclusivo desligado
    .description-other = O exclusivo toma o dispositivo só para o rox, então nada mais no desktop consegue soar por ele até o modo ser desligado
settings-audio-output-device-system-default = Padrão do sistema
settings-audio-output-experimental-badge = Experimental
settings-audio-output-experimental-tooltip = O backend exclusivo desta plataforma foi escrito a partir do contrato de áudio documentado dela, mas nunca rodou em hardware real pelos desenvolvedores. Ele deve tomar o dispositivo ou cair para o compartilhado com um motivo, nunca ficar mudo. Se der problema, desligue e conte o que aconteceu pelo botão ao lado deste selo.
settings-audio-output-format = Formato
    .description = O que o rox entrega à placa. Uma placa que não aceita a escolha roda o formato mais largo que tem, e o status abaixo mostra qual
settings-audio-output-format-f32 = Ponto flutuante de 32 bits
settings-audio-output-format-s16 = Inteiro de 16 bits
settings-audio-output-format-s32 = Inteiro de 32 bits
settings-audio-output-format-widest = O mais largo disponível
settings-audio-output-issue-tooltip = Relate como o modo exclusivo se comportou nesta máquina. Abre uma issue no GitHub com a plataforma e o stream negociado já preenchidos.
settings-audio-output-mode-exclusive = Exclusivo
settings-audio-output-mode-shared = Compartilhado
settings-audio-output-not-built = Ainda não compilado para esta plataforma
settings-audio-output-rate-follow = Seguir o arquivo
settings-audio-output-sample-rate = Taxa de amostragem
    .description = Seguir reabre o dispositivo na taxa de cada arquivo, o que custa uma pausa na fronteira onde a taxa muda; fixar uma taxa nunca paga isso e reamostra tudo que não bate
settings-audio-output-status-error-hint = Escolha outro dispositivo, ou desligue o exclusivo
settings-audio-output-status-error-title = Sem saída
settings-audio-output-status-idle-hint = Comece uma faixa para ver o formato que o dispositivo aceitou
settings-audio-output-status-idle-title = Nada tocando
settings-audio-replaygain-level-by = Nivelar por
    .description = Tocar cada faixa na sonoridade que as tags de ReplayGain mediram, para que o aleatório pare de pular entre masterizações. Faixa nivela cada arquivo por conta própria; Álbum usa o ganho do disco em todas as faixas dele, o que mantém as passagens quietas e altas do álbum onde foram colocadas
    .keywords = normalizacao volume nivelamento sonoridade
settings-audio-replaygain-measure-missing-button = Medir o que falta
settings-audio-replaygain-measure-new = Medir arquivos novos
    .description = Medir o que a monitoração traz assim que chega, depois que a sincronização assenta, para que uma biblioteca que cresce mantenha os ganhos sem você voltar aqui. Os números vão para onde Salvar ganhos medidos apontar. Ligar isso oferece medir primeiro o que já está faltando; depois disso ele só vê arquivos recém-chegados
settings-audio-replaygain-measuring-progress = Medindo { $done } de { $total }
settings-audio-replaygain-measuring-start = Medindo: descobrindo o que falta...
settings-audio-replaygain-mode-album = Álbum
settings-audio-replaygain-mode-off = Desligado
settings-audio-replaygain-mode-track = Faixa
settings-audio-replaygain-preamp = Pré-amplificação
    .description = Somado a todo ganho marcado nas tags. A referência do ReplayGain fica abaixo de onde os discos modernos são cortados, então uma biblioteca nivelada toca mais baixo que a mesma biblioteca crua; é aqui que isso volta. Um reforço nunca satura: o pico marcado nas tags o limita
settings-audio-replaygain-save = Salvar ganhos medidos
    .description = Onde a passagem de medição coloca os números dela. O banco de dados da biblioteca deixa seus arquivos intactos; as tags colocam os mesmos valores onde todo outro player os lê, ao custo de reescrever os arquivos de áudio
settings-audio-replaygain-status-measured = Todas as { $total } faixas escaneadas têm um ganho para nivelar, e o rox mediu { $measured } delas
settings-audio-replaygain-status-tagged = Todas as { $total } faixas escaneadas têm tags de ReplayGain
settings-audio-replaygain-untagged = Arquivos sem tags
    .description = Com que ganho toca um arquivo sem tags de ReplayGain. Nada o mediu, então isto é um palpite no lugar de uma medição. Deixe em zero e as faixas sem tags tocam como sempre tocaram
settings-audio-section-broadcast = Transmissão
settings-audio-section-equalizer = Equalizador
settings-audio-section-output = Saída
settings-audio-section-playback = Reprodução
settings-audio-section-replaygain = ReplayGain
settings-audio-transport = Transporte
    .description = Iniciar e parar sem sair desta página, já que toda configuração abaixo é julgada de ouvido

## Settings: integrations
settings-integrations-discord-enable = Ativar Rich Presence
    .description = Mostrar a atividade do rox no Discord enquanto a música toca
settings-integrations-discord-show-lastfm = Mostrar botão do Last.fm
    .description = Incluir um botão clicável 'Ver no Last.fm' no status do Discord
settings-integrations-discord-show-youtube = Mostrar botão do YouTube
    .description = Incluir um botão clicável 'Buscar no YouTube' no status do Discord
settings-integrations-ffmpeg-binary = Binário do FFmpeg
    .description = Qual ffmpeg roda as conversões; deixe vazio para o que está no PATH
settings-integrations-ffmpeg-fail-note = Converter fica escondido até o ffmpeg apontar para um binário que funcione
settings-integrations-ffmpeg-fail-title = Este ffmpeg não rodou
settings-integrations-ffmpeg-missing-note = Converter fica escondido; instale o ffmpeg ou aponte o caminho para um binário
settings-integrations-ffmpeg-missing-title = Nenhum ffmpeg funcional encontrado
settings-integrations-ffmpeg-ok-note = O ffmpeg funciona. Converter está disponível.
settings-integrations-ffmpeg-test = Testar
settings-integrations-lastfm-api-key-row = Chave de API
settings-integrations-lastfm-connect = Conectar
settings-integrations-lastfm-disconnect = Desconectar
settings-integrations-lastfm-finish-connecting = Concluir a conexão
settings-integrations-lastfm-hearts = { $n ->
    [one] { $n } curtida
   *[other] { $n } curtidas
}
settings-integrations-lastfm-import-loved = Importar faixas curtidas
settings-integrations-lastfm-intro-builtin = Conecte sua conta do Last.fm: autorize o rox no navegador e as faixas tocadas viram scrobbles lá
settings-integrations-lastfm-intro-custom = Esta build não traz identidade de api, então o scrobbling precisa da sua própria conta de api (Last.fm/api/account/create); cole a chave e o segredo compartilhado, depois conecte
settings-integrations-lastfm-key-placeholder = Chave de API
settings-integrations-lastfm-love-failed = A última falhou: { $error }
settings-integrations-lastfm-love-pending = { $hearts } esperando para enviar
settings-integrations-lastfm-love-pending-failed = { $hearts } esperando para enviar, última tentativa: { $error }
settings-integrations-lastfm-reconnect = Reconectar
settings-integrations-lastfm-secret-placeholder = Segredo compartilhado
settings-integrations-lastfm-secret-row = Segredo compartilhado
settings-integrations-lastfm-status-confirming = Confirmando...
settings-integrations-lastfm-status-connected = Conectado como { $username }
settings-integrations-lastfm-status-elsewhere = Conectado em outra instalação do rox; cada uma autoriza sob a própria identidade de api, então conecte esta também
settings-integrations-lastfm-status-failed = A conexão falhou: { $error }
settings-integrations-lastfm-status-not-connected = Não conectado
settings-integrations-lastfm-status-rejected = O Last.fm rejeitou a sessão e ela foi descartada. Conecte de novo para continuar fazendo scrobble
settings-integrations-lastfm-status-requesting = Pedindo um token...
settings-integrations-lastfm-status-waiting = Autorize o rox no navegador, depois conclua a conexão
settings-integrations-lastfm-working = Trabalhando...
settings-integrations-love-favourites = Curtir os favoritos
    .description = Espelhar as curtidas no Last.fm como faixas curtidas; tirar uma curtida a remove de lá também
settings-integrations-scrobble-threshold = Limiar de scrobble
    .description = Quanto de uma faixa precisa tocar antes de virar scrobble; a barra de posição e a forma de onda podem marcar isso
settings-integrations-scrobble-tracks = Fazer scrobble das faixas
    .description = Enviar as faixas tocadas para o Last.fm assim que passarem do limiar
settings-integrations-section-conversion = Conversão
settings-integrations-section-discord = Discord Rich Presence
settings-integrations-section-favourites = Favoritos
settings-integrations-section-lastfm = Last.fm
settings-integrations-section-scrobbling = Scrobbling

## Settings: keymap
settings-keymap-clash = { $chord } também é { $other }; só um dos dois vai disparar
settings-keymap-not-bound = Sem atalho
settings-keymap-recording = Pressione as teclas
settings-keymap-restore = Restaurar
settings-keymap-restore-all = Restaurar todos os atalhos
    .description = Devolver cada comando às teclas com que ele vem de fábrica, inclusive as que esta build não tem mais linha para mostrar
settings-keymap-section-defaults = Padrões
settings-keymap-undo = Desfazer
settings-keymap-undo-last = Desfazer a última restauração
    .description = Trazer de volta os atalhos que a última restauração descartou, de uma linha ou de todas

## Settings: library
settings-library-acoustic-all-described = Todas as { $total } faixas escaneadas estão descritas por { $label }
settings-library-acoustic-auto = Descrever arquivos novos
    .description = Descrever o que a monitoração traz assim que chega, depois que a sincronização assenta, para que uma biblioteca que cresce mantenha as descrições sem você voltar aqui. Desligado, os arquivos novos esperam pelo botão Analisar o que falta. Ligar isso oferece analisar primeiro o que já está faltando; depois disso ele só vê arquivos recém-chegados
settings-library-acoustic-enable = Descrever como as faixas soam
    .description = Descobrir como cada faixa soa, para que a biblioteca consiga achar música parecida com o que está tocando. Tudo roda nesta máquina, e descrever uma biblioteca grande demora um bom tempo
    .keywords = similar som impressao descrever
settings-library-acoustic-extractor = Extrator
settings-library-acoustic-extractor-model = Modelo
settings-library-acoustic-fallback = Analisando
settings-library-acoustic-partial = { $label } descreve { $done } de { $total } faixas escaneadas. Analisar o que falta cuida do resto
settings-library-acoustic-progress = { $running } está em { $done } de { $total }
settings-library-acoustic-progress-start = { $running }: descobrindo o que falta...
settings-library-acoustic-save = Salvar descrições
    .description = Onde a passagem coloca o que descobre. Só o banco de dados deixa seus arquivos intactos; as tags colocam uma cópia dentro de cada arquivo também, para que as descrições sejam mantidas se a biblioteca for reconstruída ou a pasta for parar em outra máquina, ao custo de reescrever os arquivos de áudio. As tags só valem para MP3 e FLAC; todo outro formato fica com a cópia no banco de dados
settings-library-add-folder = Adicionar pasta
settings-library-duplicates = Duplicatas...
settings-library-embed-button = Incorporar metadados salvos...
settings-library-folder-col-albums = Álbuns
settings-library-folder-col-folder = Pasta
settings-library-folder-col-size = Tamanho
settings-library-folder-col-tracks = Faixas
settings-library-folders-intro = Pastas escaneadas para a biblioteca; remover uma tira as faixas dela do catálogo e deixa os arquivos em paz
settings-library-genre-separator-nudge = Separadores mudaram: a navegação acompanha na hora. As listas de gênero guardadas por varreduras anteriores mantêm a forma antiga até você apertar Reescanear no cabeçalho Pastas lá em cima
settings-library-merge-case = Unificar variações de caixa
    .description = Tratar valores que só diferem em maiúsculas e minúsculas como um só: Rock e rock viram o mesmo gênero, artista e álbum, mostrados na grafia que a maioria das faixas usa. Os arquivos mantêm as tags como estão escritas
settings-library-no-folders = Nenhuma pasta ainda
settings-library-repair-tags = Reparar tags...
settings-library-section-folders = Pastas
settings-library-section-stored-metadata = Metadados salvos
settings-library-section-tempo = Análise de andamento
settings-library-split-genres = Separar gêneros em vírgulas e barras
    .description = "Dubstep, Trap" e "Drum & Bass / Neurofunk" contam cada valor como um gênero próprio; ponto e vírgula sempre separa. Desligado, os nomes com barra ficam inteiros, para as tags em que eles significam um gênero só. Os arquivos mantêm as tags como estão escritas
settings-library-tempo-auto = Medir arquivos novos
    .description = Contar as batidas do que a monitoração traz assim que chega, depois que a sincronização assenta, para que uma biblioteca que cresce mantenha os andamentos sem você voltar aqui. Desligado, os arquivos novos esperam pelo botão Analisar o que falta. Ligar isso oferece medir primeiro o que já está faltando; depois disso ele só vê arquivos recém-chegados
settings-library-tempo-enable = Descobrir a que velocidade as faixas correm
    .description = Contar as batidas das faixas cujas tags não dizem, para que a biblioteca possa mostrar e ordenar por andamento. Tudo roda nesta máquina, os números vão para o banco de dados da biblioteca, e seus arquivos ficam intactos
settings-library-tempo-progress = Medindo o andamento de { $done } de { $total }
settings-library-tempo-progress-start = Descobrindo o que falta...
settings-library-tempo-status-measured = Todas as { $total } faixas escaneadas têm andamento, e o rox descobriu { $measured } delas
settings-library-tempo-status-tagged = Todas as { $total } faixas escaneadas têm uma tag de andamento
settings-library-watch-folders = Monitorar pastas
    .description = Absorver na biblioteca os arquivos adicionados, editados e apagados conforme acontecem, sem uma nova varredura manual
settings-library-write-stored = Escrever o que está salvo dentro dos arquivos
    .description = As três configurações de salvamento só valem para a próxima escrita, então tudo que foi salvo antes de uma delas virar Tags ainda está só no rox. Isto escreve as letras, os ganhos e as descrições que o rox já tem dentro dos próprios arquivos, para que outro player que leia a pasta os veja. Nada é recalculado

## Settings: MCP
settings-mcp-client-config = Configuração do cliente
    .description = Cole na lista de servidores de um cliente MCP (Claude Code, Claude Desktop, ou qualquer outro) para deixá-lo consultar o rox sobre a biblioteca, o que está tocando e o transporte. O rox precisa estar rodando; as ferramentas passam pelo socket de controle dele
settings-mcp-enable = Ativar o servidor MCP
    .description = Responder a chamadas de ferramenta dos clientes MCP conectados. O proxy verifica isso a cada chamada, então enquanto estiver desligado os clientes são recusados com o motivo; a configuração abaixo pode ser feita de qualquer jeito

## Settings: ML models
settings-mlmodels-checking = Verificando...
settings-mlmodels-choose-file = Escolher arquivo
settings-mlmodels-custom-description-empty = Aponte o rox para um checkpoint PANNs CNN10 seu, em safetensors. Ele é lido onde está e nomeado pelo hash, então um segundo checkpoint descreve a biblioteca separadamente em vez de reaproveitar as coordenadas do primeiro
settings-mlmodels-download-failed = Não foi possível baixar { $label }: { $reason }
settings-mlmodels-downloading = Baixando { $label }: { $done } de { $total }
settings-mlmodels-stopping = Parando o download de { $label }...
settings-mlmodels-fallback-model = modelo
settings-mlmodels-fallback-the-model = O modelo
settings-mlmodels-kind-custom = Personalizado
settings-mlmodels-kind-recommended = Recomendado
settings-mlmodels-pass-stopped = A última passagem parou: { $reason }
settings-mlmodels-weights-file = Arquivo de pesos

## Settings: playback
settings-playback-continuation-continue = Continuar
    .description = Seguir descendo a lista de onde você começou, depois o resto da biblioteca atrás dela. Toque um álbum do meio de uma visualização e a visualização continua
settings-playback-continuation-off = Desligado
    .description = Nada reabastece a fila; a reprodução para no fim dela
settings-playback-continuation-weighted = Ponderado
    .description = Sortear da biblioteca inteira, o que você nunca tocou primeiro e o que ouviu há pouco por último
settings-playback-keep-playing = Continuar tocando
    .description = O que toca quando a fila acaba. O que quer que isso escolha é acrescentado à linha do tempo como contexto comum, então fica visível e removível em vez de virar estado escondido. Com a ordem acima em Similar, ele continua achando faixas parecidas com a que está tocando, qualquer que seja a opção escolhida
    .keywords = continuacao reabastecer reproducao automatica fila
settings-playback-play-order = Ordem de reprodução
    .description = Como as faixas já enfileiradas ficam arrumadas enquanto o embaralhar está ligado. O botão de embaralhar do transporte liga e desliga; isto é o que ele faz depois de ligado
settings-playback-rating-scale = Escala de avaliação
    .description = Estrelas para cliques rápidos, 0-10 em meios passos para notas de resenha mais finas
settings-playback-rating-scale-numeric = 0-10
settings-playback-rating-scale-stars = Estrelas
settings-playback-restore-last-session = Restaurar a última sessão
    .description = Iniciar com a fila como você deixou, pausada na faixa que estava tocando e no ponto onde parou. Faixas enfileiradas fora das pastas da sua biblioteca não podem ser restauradas e caem da ordem
settings-playback-section-queue = Fila
settings-playback-section-ratings = Avaliações
settings-playback-section-startup = Início
settings-playback-shuffle-random = Aleatória
    .description = O embaralhar que todo mundo quer dizer com a palavra. O que vem toca em nenhuma ordem específica
settings-playback-shuffle-similar = Similar
    .description = O mais próximo primeiro, pelo som. O que vem é ordenado por quanto se parece com a faixa que estava tocando quando você ligou isso, e reordenado a cada pulo. Precisa da biblioteca descrita na página Biblioteca
settings-playback-unrated-dots = Pontos para o não avaliado
    .description = Marcar as estrelas vazias com um ponto fraco em vez de deixá-las em branco

## Settings: providers
settings-providers-artist = Last.fm
    .description = Buscar biografias, estatísticas e artistas parecidos para o painel de biografia, com um retrato do Deezer; tudo fica na pasta de dados e é lido offline depois
settings-providers-deezer = Deezer
    .description = Buscar capas no Deezer, até 1000 pixels
settings-providers-itunes = iTunes
    .description = Buscar capas no iTunes; a busca do editor de capas mostra os resultados para escolher antes de aplicar
settings-providers-lastfm-art = Last.fm
    .description = Buscar capas no Last.fm
settings-providers-lrclib = LRCLIB
    .description = Buscar letras que faltam no lrclib.net, sincronizadas quando existirem
settings-providers-lyrics-intro = As consultas online só rodam quando uma ação de painel pede uma; a reprodução e a navegação nunca encostam na rede
settings-providers-musicbrainz = MusicBrainz
    .description = Consultar tags no musicbrainz.org; a busca do painel de metadados mostra os resultados para confirmar campo por campo antes de escrever
settings-providers-save-lyrics = Salvar letras buscadas
    .description = Onde uma letra buscada vai parar: na pasta de dados do próprio rox, mantendo a biblioteca limpa, num .lrc ao lado da faixa, ou na tag incorporada
settings-providers-save-lyrics-data-folder = Pasta de dados
settings-providers-save-lyrics-sidecar = Arquivo ao lado
settings-providers-save-lyrics-tag = Tag
settings-providers-section-artist = Artista
settings-providers-section-cover-art = Capas
settings-providers-section-lyrics = Letras
settings-providers-section-metadata = Metadados

## Settings: shader
settings-shader-backdrop-all-windows = Todas as janelas
    .description = Sombrear o fundo de todas as janelas: configurações, editores, diálogos, painéis destacados. Desligado, fica só nas janelas do espaço de trabalho
settings-shader-backdrop-enabled = Shader de fundo
    .description = Rodar um shader WGSL reativo à música sobre o fundo com a capa do álbum, abaixo de todos os painéis. Faz parte do espaço de trabalho, então viaja junto com o visual
settings-shader-backdrop-fallback-name = Fundo
settings-shader-backdrop-run-idle = Executar em silêncio
    .description = Continuar desenhando com nada tocando. A animação fica parada de qualquer jeito
settings-shader-compile-error-title = Este shader não compilou
settings-shader-legacy-note = Sem nada roteado, o conjunto preenche os slots na ordem dele: o primeiro sinal no slot 0, o segundo no slot 1, e assim por diante. A primeira rota que você adicionar assume o mapeamento inteiro.
settings-shader-overlay-enabled = Shader de overlay
    .description = Rodar um shader WGSL reativo à música sobre a janela inteira. Só são oferecidos shaders que deixam o aplicativo utilizável por baixo
settings-shader-scene-covers-window = Este shader é uma cena, então ele cobre a janela em vez de desenhar por cima. Veio de um pacote ou de uma configuração antiga; a lista acima só oferece shaders que deixam o aplicativo utilizável.
settings-shader-screen-all-windows = Todas as janelas
    .description = Sombrear as janelas filhas também: configurações, estatísticas, equalizador, painéis destacados. A contagem regressiva para reverter fica sem sombra de qualquer jeito
settings-shader-screen-fallback-name = Tela
settings-shader-screen-run-idle = Executar em silêncio
    .description = Continuar desenhando com nada tocando. A animação fica parada de qualquer jeito. Um shader que lê o mouse segue o cursor com a música parada mesmo sem isto; ele só para uns dois segundos depois do ponteiro
settings-shader-section-backdrop = Shader de fundo
settings-shader-section-overlay = Shader de overlay
settings-shader-signals-block = Sinais
    .description = Qual sinal compartilhado cada um dos dezesseis slots do shader lê
settings-shader-slots-block = Slots
    .description = Cada slot como ele chega ao shader; os slots sem rota são controles ajustados na mão

## Settings: storage
settings-storage-artist-images = Imagens de artistas
    .description = Retratos, banners e biografias buscados para as visualizações de artista (artists/); os que forem limpos são buscados de novo na próxima vez que uma visualização abrir
settings-storage-catalog = Catálogo
    .description = O índice de faixas que as varreduras constroem: uma linha por faixa com as tags, os detalhes do arquivo e os trechos de cue, dentro de library.db
settings-storage-cover-thumbnails = Miniaturas de capas
    .description = Capas pequenas guardadas depois da primeira renderização (thumbs.db); as que forem limpas se refazem conforme entram na tela
settings-storage-logs = Logs
    .description = O que cada execução escreve para relatórios de bug (logs/rox.log), rotacionado num limite de tamanho para nunca crescer demais
settings-storage-looks-layouts = Visuais e layouts
    .description = O visual que o aplicativo está usando (workspace.json), com seus espaços de trabalho salvos, os arquivos de shader exportados e os pacotes de ícones ao lado. Pequeno, e cada byte dele é algo que você montou
settings-storage-lyrics = Letras
    .description = Letras buscadas e editadas mantidas no armazenamento do próprio aplicativo (lyrics/), para que as pastas da biblioteca fiquem limpas
settings-storage-measured-tempos = Andamentos medidos
    .description = Os andamentos que o rox contou a partir do áudio, para faixas cujas tags não trazem nenhum; os números das próprias tags ficam intactos. Limpar devolve essas faixas à lista do Analisar o que falta na página Biblioteca, para que uma contagem de batidas melhorada possa substituir números escritos por uma passagem antiga
settings-storage-model-fallback-this = Este modelo
settings-storage-music-summary = { $tracks }, { $albums }, { $size }
settings-storage-model-weights = Pesos dos modelos
    .description = Os modelos baixados para a análise acústica (models/). A página Modelos de ML é onde eles são buscados e apagados, uma linha por modelo
settings-storage-models-empty = Modelos
    .description = Nada descreveu a biblioteca ainda. Ligar a análise acústica na página Biblioteca é o que preenche isto, e todo modelo que rodou ganha uma linha aqui
settings-storage-music-files = Arquivos de música
    .description = O que as pastas escaneadas guardam; os arquivos ficam onde estão
settings-storage-none = Nenhum
settings-storage-playlists-history = Playlists e histórico
    .description = Suas playlists e o que há nelas, o que você tocou, e as anotações de gênero da biblioteca. Tudo pequeno ao lado do resto do library.db
settings-storage-reclaimable = Espaço recuperável
    .description = Páginas dentro do library.db que as exclusões deixaram para trás. As escritas novas as preenchem de novo, então o arquivo para de crescer antes de começar a encolher
    .keywords = vacuum compactar reduzir banco de dados
settings-storage-section-acoustic = Descrições acústicas
settings-storage-section-app-data = Dados do aplicativo
settings-storage-section-caches = Caches
settings-storage-section-diagnostics = Diagnóstico
settings-storage-section-library = Biblioteca
settings-storage-section-tempo = Andamento
settings-storage-vectors = Vetores
    .description = O que cada descrição pesa dentro do library.db. Numa biblioteca por onde a passagem de análise já correu, isto é a maior parte do arquivo, alguns kilobytes por faixa contra algumas centenas de bytes de tags
settings-storage-waveforms = Formas de onda
    .description = A tira de picos de cada faixa, guardada depois da primeira reprodução; as que forem limpas são decodificadas de novo na próxima

## Settings: workspace
settings-workspace-card-author = Autor
settings-workspace-card-author-placeholder = Quem fez
settings-workspace-card-created = Criado em { $date }
settings-workspace-card-created-updated = Criado em { $created }, atualizado em { $updated }
settings-workspace-card-description = Descrição
settings-workspace-card-description-placeholder = Aonde o visual quer chegar
settings-workspace-card-empty = Este espaço de trabalho não tem cartão
settings-workspace-card-hint = O cartão fica guardado no arquivo, então quem receber este visual o vê
settings-workspace-card-license = Licença
settings-workspace-card-license-placeholder = Os termos sob os quais você compartilha
settings-workspace-card-save = Salvar cartão
settings-workspace-card-updated = Atualizado em { $date }
settings-workspace-card-version = Versão
settings-workspace-card-version-placeholder = Sua própria versão, do jeito que você contar
settings-workspace-card-website = Site
settings-workspace-card-website-placeholder = Onde ele fica
settings-workspace-composition-closed = A janela do espaço de trabalho está fechada
settings-workspace-composition-hint = Os painéis da janela como estão nas divisões e nos grupos de abas; as setas reordenam uma linha entre as irmãs, o cadeado fixa um painel no lugar, e a engrenagem abre as configurações dele
settings-workspace-empty = Nenhum espaço de trabalho ainda
settings-workspace-hint = Um espaço de trabalho é um visual inteiro: layouts, paleta, aparência. Aplicar um substitui os três
settings-workspace-layout-name-placeholder = Nome do layout
settings-workspace-layouts-empty = Nenhum layout ainda
settings-workspace-layouts-hint = Principal e mini são os dois entre os quais o botão de mini player da barra de menus alterna
settings-workspace-name-placeholder = Nome do espaço de trabalho
settings-workspace-panel-preset-unknown-kind = Painel desconhecido
settings-workspace-panel-presets-empty = Nenhuma predefinição de painel ainda
settings-workspace-panel-presets-hint-after = em qualquer menu de painel. Elas pertencem só a este espaço de trabalho; outro não vai tê-las.
settings-workspace-panel-presets-hint-before = Um painel configurado em cada uma, salva pelo menu do próprio painel e trazida de volta por
settings-workspace-role-mini = Mini
settings-workspace-role-primary = Principal
settings-workspace-section-composition = Composição
settings-workspace-section-layouts = Layouts
settings-workspace-section-panel-presets = Predefinições de painel
settings-workspace-section-workspaces = Espaços de trabalho
settings-workspace-tree-empty-slot = Slot vazio
settings-workspace-tree-split-column = Dividido, empilhado
settings-workspace-tree-split-row = Dividido, lado a lado
settings-workspace-tree-tabs = Abas

## Settings: development
settings-development-experimental-panels = Painéis experimentais
    .description = Mostrar os painéis ainda em construção no menu Painéis e no lançador; eles mudam de forma entre as versões, e um layout que já tem um o mantém quando isto voltar a ficar desligado
settings-development-section-features = Recursos

## Settings: shared
settings-acoustic-analysis-heading = Análise acústica
settings-analyze-nothing-scanned = Nada escaneado para analisar ainda
settings-common-active = Ativo
settings-common-analyze-missing = Analisar o que falta
settings-common-built-in = Interno
settings-common-clear = Limpar
settings-common-copy = Copiar
settings-common-database = Banco de dados
settings-common-delete = Excluir
settings-common-download = Baixar
settings-common-rescan = Reescanear
settings-common-reveal = Mostrar
settings-common-stop = Parar
settings-common-stopping = Parando...
settings-common-tags = Tags
settings-common-tracks-count = { $count ->
    [0] { $count } faixas
    [one] { $count } faixa
   *[other] { $count } faixas
}
settings-common-use = Usar
settings-confirm-apply-body = Isto substitui seus layouts, sua paleta e sua aparência pelos do espaço de trabalho.
settings-confirm-apply-imported-body = Ele está salvo nos seus espaços de trabalho. Aplicá-lo agora substitui seus layouts, sua paleta e sua aparência pelos do espaço de trabalho.
settings-confirm-clear = Limpar
settings-confirm-clear-embeddings-body = As descrições vão embora e o espaço volta. Tê-las de novo significa rodar a passagem de análise sobre cada faixa da biblioteca outra vez.
settings-confirm-clear-embeddings-title = Limpar o que "{ $model }" descreveu?
settings-confirm-clear-measured-bpm-body = Todo andamento que o rox descobriu volta a não medido; os números das tags dos seus arquivos ficam. Tê-los de novo significa rodar a passagem de andamento sobre cada uma dessas faixas outra vez.
settings-confirm-clear-measured-bpm-title = Limpar os andamentos medidos?
settings-confirm-overwrite-workspace-body = Isto substitui o espaço de trabalho salvo pelo estado atual.
settings-confirm-overwrite-workspace-title = Sobrescrever o espaço de trabalho "{ $name }"?
settings-sidebar-data-folder = Pasta de dados
settings-sidebar-settings-file = Arquivo de configurações

## Menubar
menu-about = Sobre
menu-application = Aplicativo
menu-apply-layout = Aplicar layout
menu-apply-workspace = Aplicar espaço de trabalho
menu-chat = Bate-papo
menu-close = Fechar
menu-console = Console
menu-design-mode = Modo de design
menu-discussions = Discussões
menu-empty-window = Janela vazia
menu-equalizer = Equalizador
menu-exit = Sair
menu-hide-menubar = Ocultar a barra de menus
menu-import-workspace = Importar espaço de trabalho...
menu-new-ellipsis = Novo...
menu-new-window = Nova janela
menu-new-window-from-layout = Nova janela a partir de um layout
menu-new-window-from-panel = Nova janela a partir de um painel
menu-no-layouts = Nenhum layout
menu-no-presets = Nenhuma predefinição
menu-no-workspaces = Nenhum espaço de trabalho
menu-os-decorations = Decorações do sistema
menu-overlay-shader = Shader de overlay
menu-panel-built-in = Interno
menu-panel-new = Novo...
menu-panel-no-layouts = Nenhum layout
menu-panel-no-presets = Nenhuma predefinição
menu-panel-no-workspaces = Nenhum espaço de trabalho
menu-panel-title = Menu
menu-panels = Painéis
menu-panels-presets = Predefinições
menu-pause = Pausar
menu-playback = Reprodução
menu-remain-in-tray = Continuar na bandeja
menu-report-issue = Relatar um problema
menu-save-layout = Salvar layout
menu-save-workspace = Salvar espaço de trabalho
menu-section-add = Adicionar
menu-section-app = Aplicativo
menu-section-interface = Interface
menu-section-layouts = Layouts
menu-section-library = Biblioteca
menu-section-session = Sessão
menu-section-track = Faixa
menu-section-tuning = Ajustes
menu-settings = Configurações
menu-signals = Sinais
menu-song-theming = Tema da música
menu-stats = Estatísticas
menu-tasks = Tarefas
menu-welcome = Boas-vindas
menu-window = Janela
menu-workspace = Espaço de trabalho
menu-workspace-builtin-tag = Interno

## Workspaces
workspace-apply-body = Isto substitui o visual inteiro: layouts, paleta, aparência.
workspace-apply-imported-body = Ele está salvo nos seus espaços de trabalho. Aplicá-lo agora substitui o visual inteiro: layouts, paleta, aparência.
workspace-apply-imported-title = "{ $name }" importado
workspace-apply-screen-shader-named = Aplica o shader de overlay { $name } sobre a janela inteira.
workspace-apply-screen-shader-plain = Aplica um shader de overlay sobre a janela inteira.
workspace-apply-shader-count = { $count ->
    [one] Inclui { $count } shader: { $names }
   *[other] Inclui { $count } shaders: { $names }
}
workspace-apply-shaders-approve-body = Aprovar deixa que eles rodem nesta máquina. Aplicar sem eles deixa o visual pelado, com os shaders ainda no conjunto dele.
workspace-apply-shaders-plain-body = Aplicar sem eles deixa o visual pelado, com os shaders ainda no conjunto dele.
workspace-byline-author = por { $author }
workspace-byline-version = versão { $version }
workspace-context-add-panel = Adicionar painel
workspace-dialog-apply = Aplicar
workspace-dialog-apply-title = Aplicar "{ $name }"?
workspace-dialog-approve-apply = Aprovar e aplicar
workspace-dialog-cancel = Cancelar
workspace-dialog-close = Fechar
workspace-dialog-close-title = Fechar "{ $name }"?
workspace-dialog-export = Exportar
workspace-dialog-layout-name-placeholder = Nome do layout
workspace-dialog-not-now = Agora não
workspace-dialog-overwrite = Sobrescrever
workspace-dialog-overwrite-title = Sobrescrever "{ $name }"?
workspace-dialog-save = Salvar
workspace-dialog-save-layout-title = Salvar layout
workspace-dialog-save-workspace-title = Salvar espaço de trabalho
workspace-dialog-with-shaders = Com shaders
workspace-dialog-without-shaders = Sem shaders
workspace-dialog-workspace-name-placeholder = Nome do espaço de trabalho
workspace-drop-add-queue = Adicionar à fila
workspace-drop-play-now = Tocar agora
workspace-hint-or = ou
workspace-hint-then = depois
workspace-import = Importar
workspace-launcher-hint = Adicione seu primeiro painel para começar a montar, ou escolha uma predefinição em Espaço de trabalho > Aplicar espaço de trabalho
workspace-launcher-need-help = Precisa de ajuda?
workspace-launcher-open-welcome = Abrir a janela de boas-vindas
workspace-launcher-title = Uma janela vazia
workspace-layout-apply-body = Isto substitui o layout atual desta janela.
workspace-layout-overwrite-body = Isto substitui o layout salvo pelo atual.
workspace-layout-preset-restore-failed = A predefinição de layout desta janela não pôde ser restaurada, então ela começa vazia.
workspace-layout-restore-failed = O layout salvo não pôde ser restaurado, então esta janela começa vazia.
workspace-mini-tip-back = Voltar para o layout completo
workspace-mini-tip-shrink = Encolher para o mini player
workspace-overwrite-body = Isto substitui o espaço de trabalho salvo pelo visual atual.
workspace-panel-locked-close-body = Este painel está fixado no lugar. Fechá-lo o tira do layout.
workspace-save-current = Salvar o atual
workspace-screen-shader-hint-before = Desligue quando quiser com
workspace-workspace-restore-failed = O layout do espaço de trabalho não pôde ser restaurado, então esta janela começa vazia.

## Tasks window
tasks-acoustic-all-described = Todas as { $count } faixas escaneadas estão descritas por { $label }
tasks-acoustic-off = Descrever como as faixas soam está desligado nas Configurações, em Biblioteca
tasks-acoustic-partial = { $label } descreve { $embedded } de { $total } faixas escaneadas
tasks-analyzing = Analisando { $progress }
tasks-bake-writing = Escrevendo tags...
tasks-chip-count = { $count } tarefas
tasks-convert-starting = Iniciando o ffmpeg...
tasks-converting = Convertendo { $progress }
tasks-count-of-total = { $done } de { $total }
tasks-embedding = Incorporando { $progress }
tasks-estimate-at = { $estimate } com { $workers }
tasks-import-failed = A última importação falhou: { $error }
tasks-import-reading = Lendo a lista de curtidas...
tasks-import-unmatched = { $count } sem correspondência nesta biblioteca
tasks-importing = Importando { $progress }
tasks-job-acoustic = Análise acústica
tasks-job-convert = Converter áudio
tasks-job-loved-import = Faixas curtidas do Last.fm
tasks-job-replaygain = ReplayGain
tasks-job-scan = Varredura da biblioteca
tasks-job-tempo = Análise de andamento
tasks-last-pass-stopped = A última passagem parou: { $reason }
tasks-last-run-finished = { $count ->
    [0] Última execução concluída, { $count } prontas
    [one] Última execução concluída, { $count } pronta
   *[other] Última execução concluída, { $count } prontas
}
tasks-last-run-stopped = Última execução parou depois de { $count }
tasks-library-busy = A biblioteca está ocupada
tasks-library-scanning = A biblioteca está escaneando
tasks-measuring = Medindo { $progress }
tasks-model-downloading = Um modelo ainda está baixando
tasks-no-library-window = Nenhuma janela de biblioteca está aberta, então isto não pode ser iniciado daqui
tasks-nothing-to-measure = Nada escaneado para medir ainda
tasks-rg-all-gain = Todas as { $count } faixas têm um ganho para tocar
tasks-rg-partial = { $missing } de { $total } faixas não têm ganho
tasks-scan-folder-count = { $count ->
    [one] { $count } pasta
   *[other] { $count } pastas
}
tasks-scan-last-scanned = { $folders }, escaneadas pela última vez há { $ago }
tasks-scan-never-scanned = { $folders }, nunca escaneadas
tasks-scan-no-folders = Nenhuma pasta adicionada ainda. Adicione uma nas Configurações, em Biblioteca
tasks-start-analyze-missing = Analisar o que falta
tasks-start-measure-missing = Medir o que falta
tasks-start-rescan = Reescanear
tasks-stop = Parar
tasks-stopping = Parando...
tasks-tempo-all = Todas as { $count } faixas têm andamento
tasks-tempo-off = Descobrir a que velocidade as faixas correm está desligado nas Configurações, em Biblioteca
tasks-tempo-partial = { $missing } de { $total } faixas não têm andamento
tasks-timing = Medindo o andamento de { $progress }
tasks-tip = Abrir as tarefas da biblioteca
tasks-window-title = rox - Tarefas
tasks-working-out-missing = Descobrindo o que falta...

## Stats window
stats-bucket-listens = { $count ->
    [one] { $count } audição, { $ago }
   *[other] { $count } audições, { $ago }
}
stats-chart-start-all = Primeira audição
stats-chart-start-month = Há 30 dias
stats-chart-start-week = Há 7 dias
stats-chart-start-year = Há um ano
stats-click-opens = Clique abre as estatísticas
stats-click-section = Clique
stats-count-menu = Contagem
    .description = Sobre qual janela recente o número conta as audições; a lista que aparece ao passar o mouse sempre mostra todas
stats-empty-all = Nenhuma audição ainda
stats-empty-range = Nenhuma audição neste período
stats-now = Agora
stats-open = Abrir estatísticas
stats-open-on-click = Abrir estatísticas ao clicar
    .description = Clicar no widget para abrir a janela de estatísticas, o registro completo de audições
stats-play-these-tracks = Reproduzir estas faixas
stats-play-this-track = Reproduzir esta faixa
stats-plays-count = { $count ->
    [one] { $count } reprodução
   *[other] { $count } reproduções
}
stats-range-all = Desde sempre
stats-range-all-short = Tudo
stats-range-day-short = Dia
stats-range-label = Período
stats-range-month = Este mês
stats-range-month-short = Mês
stats-range-today = Hoje
stats-range-week = Esta semana
stats-range-week-short = Semana
stats-range-year = Este ano
stats-range-year-short = Ano
stats-readout-section = Leitura
stats-section-listens = Audições
stats-section-listens-over-time = Audições ao longo do tempo
stats-section-recent-listens = Audições recentes
stats-section-top-albums = Top álbuns
stats-section-top-artists = Top artistas
stats-section-top-genres = Top gêneros
stats-show-change = Mostrar a variação
    .description = Adicionar um selo com a comparação entre este período e o anterior, para cima ou para baixo; Desde sempre não tem nada atrás
stats-show-number = Mostrar o número
    .description = Desenhar a contagem ao lado do ícone; desligado, fica só o ícone e as contagens vêm ao passar o mouse
stats-title = Widget de estatísticas
stats-tooltip-listens = Audições
stats-window-title = rox - Estatísticas

## About window
about-check-failed = Não foi possível alcançar o GitHub
about-check-for-updates = Verificar atualizações
about-checking = Verificando...
about-download = Baixar
about-downloading = Baixando... { $percent }%
about-get-it = Obter
about-license-lead = O rox é software livre sob a GNU AGPLv3. O código está em
about-notice-lead = Você deveria ter recebido uma cópia da licença com este programa. Se não, veja
about-release-notes = Notas da versão
about-restart-now = Reiniciar agora
about-up-to-date = Você está na versão mais recente
about-update-failed = A atualização falhou: { $error }
about-version = Versão { $version }
about-version-available = A versão { $version } está disponível
about-version-ready = A versão { $version } está pronta
about-window-title = rox - Sobre

## Welcome window
welcome-add-folder = Adicionar pasta
welcome-and = e
welcome-back = Voltar
welcome-card-menubar-title = Barra de menus
welcome-card-music-title = Música
welcome-card-panels-title = Painéis
welcome-card-playback-title = Reprodução
welcome-card-rearranging-title = Reorganizar
welcome-card-settings-title = Configurações
welcome-close = Fechar
welcome-design-mode-note = Reorganizar precisa do Modo de design, ligado por padrão no topo daquele menu. Desligado, ele trava o layout, para que um arranjo pronto não saia do lugar.
welcome-done = Pronto
welcome-drop-note = Solte na borda de um painel para dividir ali, no meio para dividir um grupo de abas, ou fora da janela para virar uma janela própria.
welcome-key-left-click = Clique esquerdo
welcome-key-middle-mouse = Botão do meio
welcome-layout-note = Salve um arranjo como layout; um espaço de trabalho junta layouts e paleta num visual que dá para compartilhar.
welcome-menubar-after = duas vezes para deixá-la à mostra.
welcome-menubar-before = Com a barra de menus oculta, segure
welcome-menubar-mid = para trazê-la de volta sobre o dock, ou toque
welcome-music-note = O rox escaneia a pasta para a biblioteca e os arquivos ficam onde estão. Mais pastas você adiciona nas configurações, em biblioteca.
welcome-next = Avançar
welcome-or = ou
welcome-panels-note = Toda superfície é um painel, e o menu Painéis da barra de menus abre mais deles.
welcome-playback-after = avançam e retrocedem.
welcome-playback-before = alterna a reprodução;
welcome-quickplay-after = e ela toca.
welcome-quickplay-before = abre a reprodução rápida: digite uma faixa, aperte
welcome-rearrange-after = em qualquer ponto de um painel para movê-lo.
welcome-rearrange-before = Arraste uma aba, ou segure
welcome-settings-hint-after = abre as configurações: a paleta, a transparência e o comportamento.
welcome-shelf-caption = Escolher um substitui o visual da janela principal e fecha o tour. Esta janela está sempre em Aplicativo > Boas-vindas.
welcome-stage-lead-quick-start = Escolha um espaço de trabalho e a janela principal muda para ele: layouts, paleta, o visual inteiro.
welcome-stage-lead-welcome = O Foobar se ele tivesse sido feito em 20XX.
welcome-stage-title-quick-start = Início rápido
welcome-stage-title-welcome = Bem-vindo ao rox
welcome-step-hint-after = , ou os botões abaixo.
welcome-step-hint-before = Percorra com
welcome-tile-by = por { $author }
welcome-tour-intro = Um tour rápido por onde a música entra e onde o visual é definido. Ele termina na prateleira de espaços de trabalho que vêm junto, um clique cada.
welcome-window-title = rox - Boas-vindas

## Console window
console-clear = Limpar
console-copy = Copiar
console-empty-filtered = Nada nestes níveis
console-empty-none = Nada registrado ainda
console-filter-error = Erro
console-filter-info = Info
console-filter-warn = Aviso
console-follow = Acompanhar
console-line-count = { $count ->
    [one] { $count } linha
   *[other] { $count } linhas
}
console-open-button = Abrir console
console-reveal = Mostrar
console-window-title = rox - Console

## Signals window
signals-about-toggle = Sobre os sinais
signals-blurb-marked = Os painéis marcados com isto nos menus podem ter a maior parte dos parâmetros vinculada: clique com o botão direito num parâmetro nas configurações do painel e escolha um sinal, ou adicione um dali mesmo.
signals-blurb-shared = O que é ajustado aqui é compartilhado: uma mudança se aplica a todo parâmetro roteado para aquele sinal, em todo painel e em toda janela.
signals-blurb-total = Um Total é o quarto tipo: ele soma outro sinal ao longo do tempo e dá a volta em 1, então sobe enquanto a música está alta e empaca enquanto não está. Use quando um shader precisa de uma fase que anda com a música em vez de com o relógio.
signals-blurb-what = Um sinal transforma o que está tocando num número entre 0 e 1: a energia numa banda de frequência, o nível da mistura inteira, ou um pulso a cada batida dentro de uma banda. Resposta define a rapidez com que ele segue, Limiar o silencia abaixo de um nível que você escolhe.
signals-no-library = Nenhuma janela de biblioteca está aberta, então estes não mostram áudio. As edições continuam sendo salvas.
signals-window-title = rox - Sinais

## Equaliser
eq-analyzer-bars = Barras
eq-analyzer-off = Sem analisador
eq-analyzer-wave = Onda
eq-band-badge = Selo de bandas
    .description = Mostrar quantas bandas estão fora do plano, num selo sobre o ícone
eq-band-label = Banda { $number }
eq-click-nothing = Nada
eq-click-open = Abrir
eq-click-section = Clique
    .description = O que um clique faz: abre a janela do equalizador, ou liga e desliga a curva inteira onde ela está
eq-click-toggle = Alternar
eq-flatten = Aplanar
eq-freq-label = Freq
eq-gain-label = Ganho
eq-heading = Equalizador
eq-help-text = Arraste uma banda para movê-la, role o mouse sobre ela para alargar ou estreitar. O processamento roda antes do buffer que alimenta a placa de som, então uma mudança leva até meio segundo para chegar às caixas.
eq-hint-off = Clique para desligar
eq-hint-on = Clique para ligar
eq-hint-open = Clique para abrir o equalizador
eq-open = Abrir equalizador
eq-readout-curve = Curva
eq-readout-icon = Ícone
eq-readout-section = Leitura
    .description = O ícone, a curva de resposta como um minigráfico, ou os dois. A curva precisa de uns cinquenta pixels de largura para ser legível
eq-reset-bands = Redefinir bandas
eq-shape-active = { $count ->
    [one] { $count } banda fora do plano, pico { $peak } dB
   *[other] { $count } bandas fora do plano, pico { $peak } dB
}
eq-shape-flat = Plano, todas as bandas em 0 dB
eq-status-off = Equalizador desligado
eq-status-on = Equalizador ligado
eq-title = Widget de EQ
eq-widget-section = Widget
eq-width-label = Largura
eq-window-title = rox - Equalizador

## Keymap
keymap-close-window = Fechar janela
    .description = Fechar a janela que estiver na frente. Vale em todo lugar, painéis destacados inclusive
keymap-decrease-font-size = Diminuir o texto
    .description = Baixar um passo o tamanho do texto de todo o aplicativo
keymap-focus-search = Focar a busca
    .description = Colocar o cursor no campo de busca da biblioteca
keymap-group-editing = Edição
keymap-group-playback = Reprodução
keymap-group-view = Visualização
keymap-group-windows = Janelas
keymap-increase-font-size = Aumentar o texto
    .description = Subir um passo o tamanho do texto de todo o aplicativo
keymap-key-backspace = Backspace
keymap-key-delete = Delete
keymap-key-down = Baixo
keymap-key-end = End
keymap-key-esc = Esc
keymap-key-home = Home
keymap-key-insert = Insert
keymap-key-left = Esquerda
keymap-key-page-down = Page Down
keymap-key-page-up = Page Up
keymap-key-right = Direita
keymap-key-space = Espaço
keymap-key-tab = Tab
keymap-key-up = Cima
keymap-mod-alt = Alt
keymap-mod-cmd = Cmd
keymap-mod-ctrl = Ctrl
keymap-mod-fn = Fn
keymap-mod-option = Option
keymap-mod-shift = Shift
keymap-mod-super = Super
keymap-mod-win = Win
keymap-open-quick-play = Reprodução rápida
    .description = Levantar o campo de buscar e tocar sobre a janela
keymap-open-settings = Abrir configurações
    .description = Abrir esta janela
keymap-open-stats = Abrir estatísticas
    .description = Abrir a janela de estatísticas de audição
keymap-quit = Sair
    .description = Sair do rox. Vale em todo lugar, já que não existe janela de onde não deveria funcionar
keymap-reset-font-size = Redefinir o texto
    .description = Voltar o tamanho do texto ao padrão
keymap-seek-backward = Retroceder
    .description = Voltar um passo dentro da faixa que toca
keymap-seek-forward = Avançar
    .description = Avançar um passo dentro da faixa que toca
keymap-stamp-line = Marcar linha da letra
    .description = Escrever a posição de reprodução na linha da letra que está sendo editada
keymap-toggle-playback = Reproduzir / Pausar
    .description = Iniciar a faixa atual, ou pausá-la onde está
keymap-toggle-post-shader = Alternar o shader de overlay
    .description = Ligar e desligar o shader de tela. Vale em todo lugar, já que um shader pode encobrir os controles que você usaria para desligá-lo
keymap-toggle-zoom = Ampliar o grupo de painéis
    .description = Preencher o dock com o último grupo de painéis clicado, ou sair dele

## Panel catalog
panel-catalog-album-carousel = Carrossel de álbuns
panel-catalog-artist-grid = Grade de artistas
panel-catalog-biography = Biografia
panel-catalog-cover-art = Capa
panel-catalog-drawer = Gaveta
panel-catalog-eq-widget = Widget de EQ
panel-catalog-filter = Filtro
panel-catalog-folder-tree = Árvore de pastas
panel-catalog-genre-grid = Grade de gêneros
panel-catalog-group-application = Aplicativo
panel-catalog-group-arrangement = Arranjo
panel-catalog-group-catalogue = Catálogo
panel-catalog-group-controls = Controles
panel-catalog-group-details = Detalhes
panel-catalog-group-experimental = Experimental
panel-catalog-group-visualizers = Visualizações
panel-catalog-history = Histórico
panel-catalog-menu = Menu
panel-catalog-metadata = Metadados
panel-catalog-mini-toggle = Alternar mini
panel-catalog-oscilloscope = Osciloscópio
panel-catalog-overlay = Overlay
panel-catalog-particles = Partículas
panel-catalog-playlists = Playlists
panel-catalog-queue = Fila
panel-catalog-queue-widget = Widget de fila
panel-catalog-seek = Posição
panel-catalog-slide = Slide
panel-catalog-spectrogram = Espectrograma
panel-catalog-spectrum = Espectro
panel-catalog-stats-widget = Widget de estatísticas
panel-catalog-status = Status
panel-catalog-theme-toggle = Alternar tema
panel-catalog-track-info = Info da faixa
panel-catalog-vu-meter = Medidor VU
panel-catalog-waveform = Forma de onda
panel-catalog-window-controls = Controles de janela

## Updater
updater-already-latest = já está na versão mais recente
updater-checksum-mismatch = o checksum do download é { $digest }, não o { $expected } que a versão informa
updater-checksum-missing-entry = { $sums } não tem entrada para { $name }; recusando um download que não dá para verificar
updater-no-asset = a versão não tem { $name }
updater-no-checksums = a versão não tem { $sums }; recusando um download que não dá para verificar
updater-no-release-build = nenhuma build de lançamento para esta plataforma
updater-overran = o download passou do tamanho que a versão informa
updater-short = o download parou em { $done } de { $bytes } bytes
updater-size-mismatch = o servidor ofereceu { $claimed } bytes, a versão informa { $bytes }

## Last.fm
lastfm-import-matching = Comparando com a biblioteca
lastfm-import-read = { $count ->
    [0] { $count } faixas curtidas lidas
    [one] { $count } faixa curtida lida
   *[other] { $count } faixas curtidas lidas
}
lastfm-import-stopped = { $count ->
    [0] Parou depois de { $count } faixas curtidas
    [one] Parou depois de { $count } faixa curtida
   *[other] Parou depois de { $count } faixas curtidas
}
lastfm-import-matched = , { $count } com correspondência
lastfm-import-added = { $count ->
    [0] , { $count } adicionadas aos favoritos
    [one] , { $count } adicionada aos favoritos
   *[other] , { $count } adicionadas aos favoritos
}

## Tag tools
tags-editor-clear-all = limpar tudo
tags-editor-form-view = Formulário
tags-editor-format-unsupported-all = As tags deste formato ainda não podem ser lidas nem escritas.
tags-editor-format-unsupported-some = Alguns destes arquivos estão num formato cujas tags ainda não podem ser lidas nem escritas.
tags-editor-guess-button = Deduzir
tags-editor-guess-folded = { $count ->
    [one] { $status }, mais { $count } não mostrado
   *[other] { $status }, mais { $count } não mostrados
}
tags-editor-guess-help = { $placeholders }; / casa com a pasta acima, %skip% descarta
tags-editor-guess-match-count = { $hits } de { $total } com correspondência
tags-editor-guess-no-match = nenhuma correspondência
tags-editor-guess-pattern-label = padrão
tags-editor-loading = Carregando as tags...
tags-editor-look-up = Consultar
tags-editor-multiple-values = Vários valores
tags-editor-clear-on-save = Limpar ao salvar
tags-editor-other-tags = Outras tags ({ $count })
tags-editor-remove = remover
tags-editor-reveal = Mostrar
tags-editor-save-errors = { $count ->
    [one] { $count } arquivo falhou; { $error }
   *[other] { $count } arquivos falharam; { $error }
}
tags-editor-saving-progress = Salvando { $done }/{ $total }...
tags-editor-table-view = Tabela
tags-editor-tags-section = Tags
tags-editor-unknown-partial = { $count } de { $total }
tags-editor-unread-count = { $total ->
    [one] Não foi possível ler as tags deste arquivo
   *[other] Não foi possível ler as tags de { $failed } de { $total } arquivos
}
tags-editor-will-clear = vai limpar
tags-editor-will-remove = vai remover
tags-editor-window-title = rox - Editor de tags
tags-guess-empty-segment = o padrão gera um nome de pasta ou arquivo vazio
tags-guess-no-placeholders = sem marcadores
tags-guess-skip-renders-nothing = %skip% não tem nada para gerar
tags-guess-unclosed = % sem fechamento
tags-guess-unknown-placeholder = marcador desconhecido %{ $name }%
tags-matcher-blocked-arm = Ative um campo para aplicar
tags-matcher-blocked-no-match = Nenhuma correspondência para aplicar
tags-matcher-blocked-pick = Escolha uma correspondência
tags-matcher-blocked-writing = Escrevendo as tags...
tags-matcher-match-count = { $count ->
    [one] 1 correspondência
   *[other] { $count } correspondências
}
tags-matcher-no-matches = Nenhuma correspondência encontrada
tags-matcher-pick-match = Escolha uma correspondência
tags-matcher-search-failed = A busca falhou: { $error }
tags-matcher-searching = Buscando...
tags-matcher-tagging = Marcando { $track }
tags-matcher-window-title = rox - Encontrar metadados
tags-rename-blocked-cue = faixa de cue, sem arquivo próprio
tags-rename-blocked-duplicate = duas faixas dão neste nome
tags-rename-blocked-occupied = já existe um arquivo ali
tags-rename-blocked-outside-roots = fora de toda raiz da biblioteca
tags-rename-blocked-unresolved = ainda não está no catálogo
tags-rename-move-error = { $name }: { $error }
tags-rename-move-errors = { $count ->
    [one] { $count } arquivo falhou; { $error }
   *[other] { $count } arquivos falharam; { $error }
}
tags-rename-moving = Movendo { $done }/{ $total }...
tags-rename-nothing-to-move = Nada para mover
tags-rename-pattern-help = { $placeholders }; / cria uma pasta, a extensão segue o arquivo
tags-rename-pattern-section = Padrão
tags-rename-preview-section = Prévia
tags-rename-unchanged = sem alteração
tags-rename-will-move = Vai mover { $count } de { $total }
tags-rename-window-title = rox - Renomear arquivos
tags-repair-affected-files = Arquivos afetados
tags-repair-section = Reparo
tags-repair-check-to-repair = Marque um arquivo para repará-lo
tags-repair-count = { $count ->
    [one] 1 arquivo
   *[other] { $count } arquivos
}
tags-repair-count-so-far = { $count } até agora
tags-repair-label-scope = escopo
tags-repair-no-affected = Nenhum arquivo afetado encontrado.
tags-repair-no-folder = Nenhuma pasta para escanear; adicione uma à biblioteca ou escolha uma.
tags-repair-pick-folder = Escolher uma pasta...
tags-repair-progress = Reparando { $done }/{ $total }...
tags-repair-repair-button = { $count ->
    [0] Reparar
   *[other] Reparar ({ $count })
}
tags-repair-result = { $count ->
    [one] 1 arquivo reparado
   *[other] { $count } arquivos reparados
}
tags-repair-result-failed = Reparou { $count }, { $failed } com falha
tags-repair-scan-first = Escaneie primeiro
tags-repair-scan-hint = Escaneie para achar arquivos com danos nas tags que uma reescrita conserta.
tags-repair-select-all = Selecionar tudo
tags-repair-select-none = Não selecionar nada
tags-repair-whole-library = Biblioteca inteira
tags-repair-window-title = rox - Reparo de tags

## Convert
convert-arg-names-file = "{ $token }" nomeia um arquivo; o destino vem da pasta e do padrão
convert-section-output = Saída
convert-section-preview = Prévia
convert-arg-not-flag-or-value = "{ $token }" não é uma flag nem um valor para uma
convert-check-wrote-nothing = o ffmpeg saiu limpo mas não escreveu nada
convert-custom-ext-empty = A extensão escolhe o contêiner, então ela é obrigatória
convert-custom-ext-invalid = "{ $ext }" não é um nome de contêiner; letras e dígitos, sem ponto
convert-dialog-browse = Procurar...
convert-dialog-check-passed = o ffmpeg codificou um instante de silêncio com estes, então eles funcionam
convert-dialog-check-waiting = Verificado com o ffmpeg quando você parar de digitar
convert-dialog-checking = Verificando com o ffmpeg...
convert-dialog-choose-folder = Escolha uma pasta para escrever
convert-dialog-convert-button = Converter
convert-dialog-custom-label = Personalizado
convert-dialog-custom-menu-item = Personalizado...
convert-dialog-custom-note = Os argumentos são separados por espaços, então nada de aspas; a capa incorporada não é copiada em formatos personalizados
convert-dialog-format-not-ready = O formato digitado ainda não passou pelo ffmpeg
convert-dialog-label-extension = extensão
convert-dialog-label-format = formato
convert-dialog-label-into = para
convert-dialog-label-named = com o nome
convert-dialog-mirror = Espelhar as pastas da biblioteca
convert-dialog-nothing-to-convert = Nada para converter: todas as linhas foram puladas
convert-dialog-pattern-help = { $placeholders }; / cria uma pasta, o formato define a extensão
convert-dialog-pick-folder = Escolha uma pasta para escrever
convert-dialog-span-note = { $count ->
    [one] { $count } recortado de uma imagem de cue e marcado pela biblioteca
   *[other] { $count } recortados de uma imagem de cue e marcados pela biblioteca
}
convert-dialog-will-convert = Vai converter { $count } de { $total }
convert-dialog-window-title = rox - Converter
convert-ffmpeg-silent-failure = o ffmpeg falhou sem dizer por quê
convert-flag-attach = -attach lê um arquivo próprio, o que aqui não é permitido
convert-flag-f = A extensão escolhe o contêiner, então -f não é seu para definir
convert-flag-i = A entrada é a faixa que você escolheu, então -i não é seu para definir
convert-flag-n = -n já está em toda execução
convert-flag-y = Nada aqui sobrescreve, então -y não está disponível; um destino que já existe é pulado
convert-preset-flac = FLAC
convert-preset-mp3-320 = MP3 320 kbps
convert-preset-mp3-v0 = MP3 V0
convert-preset-opus-192 = Opus 192 kbps
convert-preset-wav = WAV
convert-skip-duplicate = duas faixas dão neste nome
convert-skip-exists = já existe
convert-summary-failed = , { $count } com falha
convert-summary-files = { $count ->
    [0] { $count } arquivos
    [one] 1 arquivo
   *[other] { $count } arquivos
}
convert-summary-line = { $files } para { $dest }
convert-summary-skipped = { $count ->
    [0] , { $count } pulados
    [one] , { $count } pulado
   *[other] , { $count } pulados
}
convert-summary-stopped = Parou depois de { $files } para { $dest }
convert-version-answered = { $binary } rodou, mas não informou a versão

## Duplicates
duplicates-auto-select = Selecionar automaticamente
duplicates-check-to-trash = Marque as cópias para mandá-las para a lixeira
duplicates-copy-count = { $count ->
    [one] 2 cópias
   *[other] { $count } cópias
}
duplicates-different-albums = álbuns diferentes
duplicates-filter-placeholder = Filtrar por título, artista ou pasta
duplicates-groups-summary = { $groups ->
    [one] 1 grupo, { $extras ->
        [one] { $extras } cópia extra
       *[other] { $extras } cópias extras
    }
   *[other] { $groups } grupos, { $extras } cópias extras
}
duplicates-library-loading = A biblioteca ainda está carregando; tente de novo daqui a pouco.
duplicates-no-duplicates = Nenhuma duplicata encontrada.
duplicates-no-filter-matches = Nenhum grupo bate com o filtro.
duplicates-policy-newest = Manter a mais nova
duplicates-policy-oldest = Manter a mais antiga
duplicates-policy-quality = Manter a de melhor qualidade
duplicates-scan-hint = Escaneie a biblioteca em busca de faixas que aparecem mais de uma vez.
duplicates-select-none = Não selecionar nada
duplicates-selected-count = { $count ->
    [one] { $count } selecionado
   *[other] { $count } selecionados
}
duplicates-trash-button = { $count ->
    [0] Lixeira
   *[other] Lixeira ({ $count })
}
duplicates-trash-error = { $name }: { $error }
duplicates-trash-result = { $count ->
    [one] 1 arquivo movido para a lixeira
   *[other] { $count } arquivos movidos para a lixeira
}
duplicates-trash-result-failed = Moveu { $count } para a lixeira, { $failed } com falha
duplicates-trashing = Mandando { $done }/{ $total } para a lixeira...
duplicates-window-title = rox - Duplicatas

## Smart playlists
smart-playlist-descending = Decrescente
smart-playlist-edit-title = Editar playlist inteligente
smart-playlist-limit-label = Limite
smart-playlist-limit-placeholder = Sem limite
smart-playlist-match-count = { $count ->
    [0] { $count } faixas correspondem
    [one] 1 faixa corresponde
   *[other] { $count } faixas correspondem
}
smart-playlist-matched-tracks = Faixas correspondentes
smart-playlist-new-title = Nova playlist inteligente
smart-playlist-no-matches = Nenhuma faixa corresponde
smart-playlist-query-label = Consulta
smart-playlist-sort-default = Ordem padrão
smart-playlist-sort-added = Adicionada
smart-playlist-sort-label = Ordenação
smart-playlist-unknown-field = "{ $field }:" não é um campo, então o termo casa como texto simples
smart-playlist-window-title = rox - { $verb }

## Playlist creation
playlist-create-not-savable = Dê um nome à playlist para salvá-la
playlist-create-placeholder = Nome da playlist
playlist-create-rename-title = Renomear playlist
playlist-create-title = Nova playlist
playlist-create-window-title = rox - { $verb }

## Cover tools
cover-art-back = Verso
cover-art-disc = Disco
cover-art-front = Frente
cover-artwork = Imagem
    .description = Qual imagem mostrar; um slot que o arquivo não tem volta para a capa da frente
cover-disc-style = Estilo de disco
    .description = Estilizar a imagem como um CD ou como o rótulo de um disco de vinil
cover-disc-off = Desligado
cover-disc-cd = CD
cover-disc-vinyl = Vinil
cover-editor-choose-image = Escolher imagem
cover-editor-multiple = Várias
cover-editor-none = Nenhuma
cover-editor-not-an-image = Esse arquivo não é uma imagem que o rox consiga incorporar
cover-editor-not-decoded = Não foi possível decodificar essa imagem
cover-editor-reading = Lendo a capa atual...
cover-editor-remove = Remover
cover-editor-replace = Substituir
cover-editor-revert = Reverter
cover-editor-save-errors = { $count ->
    [one] { $count } arquivo falhou; { $error }
   *[other] { $count } arquivos falharam; { $error }
}
cover-editor-saving-progress = Salvando { $done }/{ $total }...
cover-editor-search-online = Buscar online
cover-editor-section = Capa
cover-editor-slot-back = Capa de trás
cover-editor-slot-front = Capa da frente
cover-editor-slot-media = Mídia
cover-editor-will-remove = Vai remover
cover-editor-window-title = rox - Capa
cover-matcher-blocked-fetching = Buscando a imagem completa...
cover-matcher-blocked-no-cover = Nenhuma capa para aplicar
cover-matcher-blocked-pick = Escolha uma capa para aplicá-la
cover-matcher-cover-count = { $count ->
    [one] 1 capa
   *[other] { $count } capas
}
cover-matcher-editor-closed = O editor de capas foi fechado
cover-matcher-no-covers = Nenhuma capa encontrada
cover-matcher-search-failed = A busca falhou: { $error }
cover-matcher-set-cover = Aplicar capa
cover-matcher-setting = Aplicando...
cover-matcher-tile-info = { $provider }  { $width }px
cover-matcher-unsupported-format = Formato de imagem não suportado
cover-matcher-window-title = rox - Encontrar capa
cover-spin = Girar
    .description = Girar o disco enquanto uma faixa toca; vale para o espaço do disco ou para um estilo de disco
cover-spin-disc = Girar o disco
cover-spin-ramp = Aceleração
    .description = Quanto o disco leva para chegar à velocidade cheia, e para desacelerar de volta
cover-spin-speed = Velocidade de giro
    .description = Velocidade cheia, em rotações por minuto
cover-stretch = Esticar
    .description = Preencher o painel, ignorando a proporção da imagem
cover-stretch-to-fill = Esticar para preencher
cover-title = Capa

## Lyrics
lyrics-always-centered = Sempre centralizado
    .description = Acolchoar as pontas para que a primeira e a última linha também possam ficar no centro
lyrics-auto-search = Busca automática
    .description = Buscar online numa faixa sem letra e salvar uma correspondência confiável, sem escolha manual
lyrics-bold = Negrito
lyrics-build-word-by-word = Montar palavra por palavra
    .description = Revelar as palavras conforme são cantadas, no estilo karaokê; as linhas ainda não cantadas ficam ocultas
lyrics-edge-bottom = Base
lyrics-edge-top = Topo
lyrics-edit-hint-after-stamp = para marcar
lyrics-edit-hint-or = ou
lyrics-edit-loading = Carregando a letra...
lyrics-edit-lyrics = Editar letra
lyrics-edit-saving = Salvando...
lyrics-edit-section = Letra
lyrics-edit-stamp = Marcar
lyrics-edit-stamp-time = Marcar { $time }
lyrics-edit-window-title = rox - Editar letra
lyrics-fade-lines-in = Revelar as linhas
    .description = Trazer uma linha do escuro conforme ela vira a linha ativa
lyrics-falloff-edge = Lado do esmaecimento
    .description = De que lado da linha ativa o esmaecimento escurece
lyrics-find-online = Encontrar letra online...
lyrics-follow-playback = Seguir a reprodução
    .description = Deslizar a linha ativa até o meio enquanto uma letra sincronizada toca
lyrics-font = Fonte
    .description = A tipografia da letra; o padrão segue a fonte do aplicativo
lyrics-gap-threshold = Limiar de intervalo
    .description = Quanto uma introdução ou um intervalo precisa durar para ganhar uma pausa
lyrics-lead-in-rest = Pausa de entrada
    .description = Mostrar uma pausa em branco antes de uma introdução longa, para que a primeira linha apareça suavemente quando chegar
lyrics-line-falloff = Esmaecimento das linhas
    .description = Quanto cada linha escurece por passo de distância da linha ativa
lyrics-line-spacing = Espaçamento das linhas
    .description = A distância entre as linhas sincronizadas, como múltiplo do tamanho do texto
lyrics-mark-dots = Pontos
lyrics-mark-note = Nota
lyrics-matcher-blocked-no-match = Nenhuma correspondência para aplicar
lyrics-matcher-blocked-pick = Escolha uma correspondência para aplicar
lyrics-matcher-blocked-saving = Salvando a letra...
lyrics-matcher-match-count = { $count ->
    [one] 1 correspondência
   *[other] { $count } correspondências
}
lyrics-matcher-no-query = Esta faixa não tem artista e título para comparar
lyrics-matcher-pick-preview = Escolha uma correspondência para ver a prévia
lyrics-matcher-search-failed = A busca falhou: { $error }
lyrics-matcher-synced-tag = { $provider }  sincronizada
lyrics-matcher-window-title = rox - Encontrar letra
lyrics-no-lyrics-notice = Sem letra
lyrics-no-lyrics-track = Sem letra para esta faixa
lyrics-rest-in-gaps = Pausa nos intervalos
    .description = Ir para uma pausa em branco num intervalo instrumental longo em vez de segurar a última linha
lyrics-rest-marker = Marca de pausa
    .description = O que uma linha sem palavras mostra numa letra sincronizada, os intervalos e as linhas em branco
lyrics-search-button = Botão de busca online
    .description = Mostrar o botão de busca na face vazia; o menu do botão direito continua encontrando letras
lyrics-search-online = Buscar online
lyrics-show-song-name = Mostrar o nome da música
    .description = Mostrar o nome da faixa na face vazia, acima da linha de sem letra
lyrics-text-size = Tamanho do texto
    .description = O texto da letra; a altura da linha sincronizada acompanha
lyrics-title = Letra
lyrics-title-unsynced = Título na não sincronizada
    .description = Fixar o título da faixa acima de uma letra não sincronizada, para que um painel curto ainda o mostre
lyrics-wipe-lyrics = Apagar a letra

## Analysis passes
pass-acoustic-body = { $model } descobre como cada uma soa, para que a biblioteca consiga achar música parecida com o que está tocando. Tudo roda nesta máquina, e o que já foi descrito é pulado. { $lands }
pass-acoustic-lands-database = Os resultados vão para o banco de dados da biblioteca e seus arquivos ficam em paz.
pass-acoustic-lands-tags = Os resultados vão para o banco de dados da biblioteca e, no caso de MP3 e FLAC, também para as tags de cada arquivo, para que sejam mantidos se o banco for reconstruído. Os outros formatos ficam só com a cópia no banco de dados.
pass-acoustic-title = { $count ->
    [one] Analisar 1 faixa?
   *[other] Analisar { $count } faixas?
}
pass-analyze = Analisar
pass-estimate-at = { $estimate } com { $workers_phrase }.
pass-estimate-button = Estimar
pass-estimating = Estimando...
pass-measure = Medir
pass-no-estimate = Nada rodou nesta máquina ainda, então não há estimativa. Estimar mede algumas faixas e calcula o resto a partir dali.
pass-replaygain-body = Cada arquivo é decodificado e medido para poder tocar na sonoridade em que foi masterizado. Os álbuns são medidos inteiros quando todas as faixas deles estão sem ganho. { $lands }
pass-replaygain-lands-database = Os números vão para o banco de dados da biblioteca e seus arquivos ficam em paz.
pass-replaygain-lands-tags = Os números são escritos de volta nas tags de cada arquivo, onde todo outro player os lê.
pass-replaygain-title = { $count ->
    [one] Medir 1 faixa?
   *[other] Medir { $count } faixas?
}
pass-tempo-body = Duas janelas de meio minuto de cada arquivo são decodificadas e as batidas contadas, para que a biblioteca possa mostrar a que velocidade uma faixa corre. Funciona melhor com música gravada no clique e pula o que não consegue medir. Os números vão para o banco de dados da biblioteca e seus arquivos ficam intactos.
pass-tempo-title = { $count ->
    [one] Descobrir o andamento de 1 faixa?
   *[other] Descobrir o andamento de { $count } faixas?
}
pass-timing = Medindo algumas faixas...
pass-timing-failed = Não foi possível medir esta biblioteca: { $error }
pass-workers = Processos

## Quick play
quick-play-comfortable-rows = Linhas folgadas
    .description = Dar mais altura a cada resultado
quick-play-cover = Capa
    .description = Mostrar uma miniatura da capa à esquerda de cada resultado
quick-play-duration = Duração
    .description = Mostrar a duração de cada resultado à direita
quick-play-narrow-by = Restringir por
quick-play-search-placeholder = Buscar na biblioteca
quick-play-subtitle = Subtítulo
    .description = Mostrar o artista e o álbum abaixo de cada resultado
quick-play-tag-album = Álbum
quick-play-tag-artist = Artista

## Drawer panel
drawer-add-tooltip = Adicionar painel gaveta
drawer-answers = Responde a
    .description = Quais escolhas abrem a gaveta: só o painel principal dela, ou qualquer painel fora
drawer-dim = Escurecer
    .description = O quanto o painel principal escurece atrás da gaveta aberta
drawer-edge = Borda
    .description = A borda em que a gaveta descansa e de onde ela desliza
drawer-edge-bottom = Base
drawer-edge-top = Topo
drawer-handle = Alça
    .description = Mostrar a alça na borda do painel. Oculta, nada da gaveta aparece até uma escolha, e a alça então fica enquanto a seleção durar, para que uma gaveta que fechou possa ser puxada de novo
drawer-open-on = Abrir com
    .description = Parar sobre a alça sempre abre a gaveta; seleção adiciona uma escolha no painel principal
drawer-pin-open = Fixar aberta
drawer-reveal = Abertura
    .description = O quanto do painel a gaveta aberta cobre
drawer-scope-elsewhere = Em outro lugar
drawer-scope-main = Painel principal
drawer-title = Gaveta
drawer-trigger-hover = Mouse em cima
drawer-trigger-selection = Seleção

## Mini player
mini-tip-back = Voltar para o layout completo
mini-tip-none = Nenhum layout mini atribuído
mini-tip-shrink = Encolher para o mini player
mini-title = Alternar mini

## System tray
tray-open = Abrir
tray-pause = Pausar
tray-play = Reproduzir
tray-quit = Sair

## Window controls
window-controls-mini-toggle = Alternar mini
    .description = Começar pelo botão de alternar o layout mini; aparece assim que um layout mini for atribuído
window-controls-minimize = Minimizar
window-controls-style = Estilo
    .description = Ícones planos, ou o semáforo do macOS
window-controls-style-icons = Ícones
window-controls-title = Controles de janela
window-controls-traffic-lights = Semáforo

## Particles panel
particles-add-emitter = Adicionar emissor
particles-aim = Mira
particles-aim-fixed = Fixa
particles-aim-outward = Para fora
particles-burst = Rajada
particles-color = Cor
particles-cone = Cone
particles-direction = Direção
    .description = Para onde ele puxa; 0 é para cima, 180 é para baixo
particles-drag = Arrasto
    .description = Quanta velocidade o ar come por segundo; zero é vácuo
particles-drift = Deriva
    .description = A que velocidade o próprio campo se move, para que os redemoinhos não fiquem parados
particles-edit-emitters = Editar emissores
particles-emitter-label = Emissor { $index }
particles-emitter-target = Emissor { $index } { $target }
particles-emitters-empty = Nenhum emissor ainda. Adicione um para começar o campo.
particles-glow = Brilho
    .description = Colocar um halo suave atrás de cada partícula
particles-gravity = Gravidade
particles-gravity-strength = Força
    .description = Puxão constante sobre tudo que estiver no ar
particles-height = Altura
particles-hold-on-pause = Segurar na pausa
    .description = Congelar o campo enquanto está pausado em vez de deixá-lo se dispersar
particles-length = Comprimento
particles-lifetime = Tempo de vida
particles-position-x = Posição X
particles-position-y = Posição Y
particles-radius = Raio
particles-rate = Taxa
particles-rotation = Rotação
particles-round-particles = Partículas redondas
    .description = Desenhar pontos em vez de quadrados
particles-scale = Escala
    .description = Quão largo um redemoinho corre; pequeno agita, grande rola
particles-section-emitters = Emissores
particles-section-medium = Meio
particles-section-particles = Partículas
particles-section-playback = Reprodução
particles-shape = Forma
particles-shape-box = Retângulo
particles-shape-line = Linha
particles-shape-point = Ponto
particles-shape-ring = Anel
particles-size = Tamanho
particles-speed = Velocidade
particles-trigger = Gatilho
particles-trigger-continuous = Contínuo
particles-turbulence = Turbulência
particles-turbulence-drift = Deriva da turbulência
particles-turbulence-scale = Escala da turbulência
particles-turbulence-strength = Força
    .description = Com que força o campo empurra as partículas; zero é desligado
particles-width = Largura

## Spectrum panel
spectrum-axis-labels = Rótulos do eixo
    .description = Marcar a faixa ao longo do painel: oitavas (C1, C2, ...) ou frequências (100, 1k, 10k)
spectrum-bar-gap = Espaço entre barras
    .description = Espaço entre as barras, espaços maiores cabem menos barras
spectrum-bar-width = Largura das barras
    .description = Quão grossa cada barra é desenhada, barras mais finas cabem mais bandas
spectrum-block-gap = Espaço entre blocos
    .description = A costura entre as células de uma pilha
spectrum-block-height = Altura dos blocos
    .description = Quão alta cada célula de uma pilha é desenhada
spectrum-cap-gravity = Gravidade das marcas
    .description = Com que força as marcas de pico caem quando a banda se afasta
spectrum-fft-size = Tamanho da FFT
    .description = Janela de análise; curta reage rápido, longa resolve mais fino
spectrum-gradient-base-color = Cor de base
    .description = A ponta quieta do gradiente personalizado
spectrum-gradient-cover = Capa
spectrum-gradient-mode = Gradiente
    .description = Colorir as bandas pela intensidade: o gradiente do tema, as cores da capa com o tema da música, ou um par personalizado
spectrum-gradient-theme = Tema
spectrum-gradient-tip-color = Cor da ponta
    .description = A ponta alta do gradiente personalizado
spectrum-high-bound-description = Frequência mais alta que as barras analisam
spectrum-high-fft-size = Tamanho da FFT alta
    .description = Janela de análise para as bandas acima da divisão
spectrum-hold-on-pause = Segurar na pausa
    .description = Congelar as barras enquanto está pausado em vez de deixá-las cair até o silêncio
spectrum-labels-frequency = Frequência
spectrum-labels-pitch = Altura
spectrum-low-bound-description = Frequência mais baixa que as barras analisam
spectrum-orientation = Orientação
    .description = A borda de onde as bandas crescem
spectrum-outline-bars = Contornar as barras
    .description = Desenhar cada barra como um contorno vazado em vez de um gradiente preenchido
spectrum-outline-width = Largura do contorno
    .description = Espessura do traço das barras vazadas
spectrum-peak-caps = Marcas de pico
    .description = Segurar uma marca no pico recente de cada banda
spectrum-split-at = Dividir em
    .description = Onde as zonas se encontram, encaixado na barra mais próxima
spectrum-split-zones = Dividir zonas
    .description = Analisar abaixo e acima de uma frequência de corte com tamanhos de janela diferentes
spectrum-style = Estilo
    .description = Barras clássicas, blocos no estilo LED, ou uma linha sólida
spectrum-style-bars = Barras
spectrum-style-blocks = Blocos
spectrum-style-line = Linha
spectrum-symmetry = Simetria
    .description = Dobrar o espectro em torno do centro; para a frente coloca os graves nas pontas, ao contrário os junta no meio
spectrum-symmetry-forward = Para a frente
spectrum-symmetry-reverse = Ao contrário

## Waveform panel
waveform-bar-gap = Espaço entre barras
    .description = Espaço entre as barras, zero as funde numa forma sólida
waveform-bar-width = Largura das barras
    .description = Quão grossa cada barra é desenhada
waveform-outline = Contorno
    .description = Traçar as barras em vez de preenchê-las; barras fundidas se leem como uma forma só
waveform-scrobble-marker = Marca de scrobble
    .description = Uma linha fina onde a faixa conta como scrobble no Last.fm
waveform-split-channels = Separar os canais
    .description = Uma linha por canal, esquerdo acima do direito; faixas mono continuam numa linha só
waveform-unavailable = Forma de onda indisponível para esta faixa

## VU panel
vu-ballistics = Balística
    .description = VU integra a sonoridade devagar; Pico salta para cima e desce suave
vu-ballistics-peak = Pico
vu-cap-gravity = Gravidade das marcas
    .description = Com que força as marcas de pico caem quando o medidor se afasta
vu-channels = Canais
    .description = Separar o par estéreo, ou juntar num medidor só
vu-channels-mono = Mono
vu-channels-stereo = Estéreo
vu-db-scale = Escala em dB
    .description = Desenhar linhas de grade rotuladas nas marcas de dB atrás dos medidores
vu-gradient-mode = Gradiente
    .description = Colorir os medidores pelo nível: o gradiente do tema, as cores da capa com o tema da música, ou um par personalizado
vu-hold-on-pause = Segurar na pausa
    .description = Congelar os medidores enquanto está pausado em vez de deixá-los cair até o silêncio
vu-orientation = Orientação
    .description = A borda de onde os medidores crescem
vu-peak-caps = Marcas de pico
    .description = Segurar uma marca no pico recente de cada medidor
vu-segment-gap = Espaço entre segmentos
    .description = A costura entre as células de uma pilha
vu-segment-height = Altura dos segmentos
    .description = Quão alta cada célula de uma pilha é desenhada
vu-style = Estilo
    .description = Uma coluna sólida, ou segmentos no estilo LED
vu-style-continuous = Contínuo
vu-style-segments = Segmentos

## Spectrogram panel
spectrogram-ceiling = Teto
    .description = Nível que corresponde à ponta clara do mapa de cores, então tudo que for mais alto fica preso ali
spectrogram-colormap = Mapa de cores
    .description = Como o volume se converte em cor
spectrogram-colormap-cover = Capa
spectrogram-colormap-grayscale = Tons de cinza
spectrogram-colormap-ice = Gelo
spectrogram-colormap-magma = Magma
spectrogram-colormap-theme = Tema
spectrogram-colormap-viridis = Viridis
spectrogram-direction = Direção
    .description = A borda por onde as novas colunas entram, o que também decide se o eixo de frequência sobe pelo painel ou o atravessa
spectrogram-fft-size = Tamanho da FFT
    .description = Tamanho da janela de análise, um equilíbrio entre a rapidez com que uma coluna acompanha um transiente e o quanto ela separa duas notas graves
spectrogram-floor = Piso
    .description = Nível que corresponde à ponta escura do mapa de cores, então tudo que for mais baixo se lê como fundo
spectrogram-grid = Grade
    .description = Linhas de frequência sobre a imagem
spectrogram-high-bound = Limite superior
    .description = Topo do eixo de frequência, limitado abaixo de Nyquist para descartar as oitavas mais altas, quase silenciosas
spectrogram-history = Histórico
    .description = Quantas colunas o painel guarda antes que a mais antiga role para fora
spectrogram-hold-on-pause = Segurar na pausa
    .description = Manter a imagem parada na pausa em vez de deixar silêncio rolar para dentro dela
spectrogram-labels = Rótulos
    .description = Os números de frequência ao longo da régua, onde o painel tem espaço para eles
spectrogram-log-scale = Escala log
    .description = Dar o mesmo espaço a cada oitava, a leitura musical, em vez do espaçamento uniforme em Hz de um instrumento de laboratório
spectrogram-low-bound = Limite inferior
    .description = Base do eixo de frequência
spectrogram-speed = Velocidade
    .description = Com que rapidez a imagem rola, em colunas por segundo

## Oscilloscope panel

oscilloscope-channels = Canais
    .description = Juntar em um só traço, sobrepor os dois, ou empilhar um quadro para cada
oscilloscope-channels-mono = Mono
oscilloscope-channels-overlay = Overlay
oscilloscope-channels-split = Separado
oscilloscope-fill = Preenchimento
    .description = Um preenchimento suave entre o traço e a linha central
oscilloscope-gain = Ganho
    .description = Escala vertical, para levar uma faixa baixa a um traço legível
oscilloscope-gradient-mode = Gradiente
    .description = Colorir o traço pela excursão: o gradiente do tema, as cores da capa com o tema da música, ou um par personalizado
oscilloscope-grid = Grade
    .description = Desenhar a retícula atrás do traço
oscilloscope-hold-on-pause = Segurar na pausa
    .description = Manter o quadro parado na pausa em vez de deixar o traço achatar
oscilloscope-line-width = Largura da linha
    .description = Com que largura o traço é desenhado
oscilloscope-persistence = Persistência
    .description = Por quanto tempo os quadros anteriores ficam visíveis atrás do traço, o efeito de persistência fosforescente
oscilloscope-trigger = Gatilho
    .description = Começar cada quadro onde o sinal cruza o nível do gatilho, para que material periódico fique parado
oscilloscope-trigger-falling = Descida
oscilloscope-trigger-level = Nível do gatilho
    .description = O nível em que o cruzamento é procurado
oscilloscope-trigger-off = Desligado
oscilloscope-trigger-rising = Subida
oscilloscope-window = Janela
    .description = Quanto tempo o traço cobre ao longo do painel

## Shader panel
shader-panel-compile-error = Este shader não compilou:
shader-panel-compile-title = Este shader não compilou
shader-panel-enable = Ativar
shader-panel-inspect = Inspecionar
shader-panel-note-empty-body = Escolha um exemplo, ou aponte o painel para um arquivo .wgsl definindo fs_user(uv).
shader-panel-note-empty-title = Nenhum shader carregado.
shader-panel-note-missing-body = Este painel aponta para um shader que o espaço de trabalho não tem, então não há nada para rodar.
shader-panel-note-missing-title = { $name } não está nos shaders deste espaço de trabalho.
shader-panel-note-off-body = A fonte e os vínculos dela continuam aqui, só não estão rodando.
shader-panel-note-off-title = Este shader está desligado.
shader-panel-note-pending-body = Ele chegou com um layout ou um espaço de trabalho em vez de vir desta máquina, então fica desligado até você revisá-lo.
shader-panel-note-pending-title = Este shader ainda não foi lido.
shader-pending-origin-file = Diz ter vindo de { $path }
shader-pending-origin-inline = Não há arquivo por trás; a fonte veio com o layout
shader-pending-more-lines = { $count ->
    [one] ... mais { $count } linha
   *[other] ... mais { $count } linhas
}
shader-eject-name-taken = { $name } já tem { $count } cópias numeradas nos shaders deste espaço de trabalho
shader-eject-not-in-pool = { $name } não está nos shaders deste espaço de trabalho
shader-eject-failed = exportando: { $error }
shader-panel-pick = Escolher um shader
shader-panel-run-shader = Executar o shader
    .description = Desligado, a fonte, o marcador e os vínculos ficam no lugar e nada é pintado
shader-panel-section-routes = Rotas

## Genre grid panel
genre-grid-clear-picked = Limpar os gêneros escolhidos
genre-grid-desaturate = Dessaturar durante a reprodução
    .description = Deixar todos os blocos menos o do gênero tocando em tons de cinza; passar o mouse traz a cor de um bloco de volta
genre-grid-dim-while-playing = Escurecer durante a reprodução
    .description = Apagar todos os blocos menos o do gênero tocando; passar o mouse acende um bloco de novo
genre-grid-follow-description = Rolar até o gênero que está tocando sempre que a faixa muda
genre-grid-merge-many = Unificar { $count } gêneros em "{ $target }"
genre-grid-merge-one = Unificar "{ $source }" em "{ $target }"
genre-grid-pick-filters = Escolher filtra a biblioteca
    .description = Clicar num gênero restringe a ele todo painel que segue a busca compartilhada; desligado, o clique fica sendo uma seleção simples
genre-grid-play-genres = Reproduzir { $count } gêneros
genre-grid-resume-description = Deslizar de volta para o gênero que está tocando depois que você para de navegar
genre-grid-show-names = Mostrar nomes
    .description = Imprimir o gênero sob cada bloco em vez de só ao passar o mouse
genre-grid-smooth-description = Deslizar até o gênero em vez de saltar
genre-grid-tally = { $albums ->
    [one] { $albums } álbum, { $tracks } faixa(s)
   *[other] { $albums } álbuns, { $tracks } faixa(s)
}
genre-grid-tile-face = Face do bloco
    .description = O que um bloco mostra: as capas dos álbuns do gênero, as capas banhadas na cor do próprio gênero, ou um cartão de cor chapada com o nome escrito nele
genre-grid-unmerge = { $count ->
    [one] Desfazer a unificação de { $count } valor
   *[other] Desfazer a unificação de { $count } valores
}

## Artist grid panel
artist-grid-clear-picked = Limpar os artistas escolhidos
artist-grid-desaturate = Dessaturar durante a reprodução
    .description = Deixar todos os blocos menos o do artista tocando em tons de cinza; passar o mouse traz a cor de um bloco de volta
artist-grid-dim-while-playing = Escurecer durante a reprodução
    .description = Apagar todos os blocos menos o do artista tocando; passar o mouse acende um bloco de novo
artist-grid-follow-description = Rolar até o artista que está tocando sempre que a faixa muda
artist-grid-group-mode = Um bloco por
    .description = O artista do álbum creditado mantém os convidados de um disco no ato que o lançou; o artista da faixa dá a cada participação um bloco próprio
artist-grid-pick-filters = Escolher filtra a biblioteca
    .description = Clicar num artista restringe a ele todo painel que segue a busca compartilhada; desligado, o clique fica sendo uma seleção simples
artist-grid-play-artists = Reproduzir { $count } artistas
artist-grid-portraits = Retratos dos artistas
    .description = Mostrar a foto de cada artista, buscada uma vez por nome e guardada no disco; desligado, aparece a capa do primeiro álbum
artist-grid-resume-description = Deslizar de volta para o artista que está tocando depois que você para de navegar
artist-grid-section-grouping = Agrupamento
artist-grid-show-names = Mostrar nomes
    .description = Imprimir o artista sob cada bloco em vez de só ao passar o mouse
artist-grid-smooth-description = Deslizar até o artista em vez de saltar
artist-grid-tally = { $albums ->
    [one] { $albums } álbum, { $tracks } faixa(s)
   *[other] { $albums } álbuns, { $tracks } faixa(s)
}
artist-grid-track-artist = Artista da faixa

## Wall panels
wall-dim-always = Sempre
    .description = Manter os blocos recuados mesmo quando nada toca; só um bloco sob o mouse aparece por inteiro
wall-dim-amount = Intensidade
    .description = Quanto os outros blocos apagam; 100% os esconde
wall-gap = Espaço
    .description = Espaço entre os blocos
wall-name-alignment = Alinhamento dos nomes
    .description = Alinhar as legendas sob seus blocos
wall-rounding = Arredondamento
    .description = Arredondar os cantos de cada bloco; 100% é um círculo
wall-section-picking = Escolha
wall-show-counts = Mostrar as contagens
    .description = A contagem de álbuns e faixas sob cada nome
wall-tile-size = Tamanho do bloco
    .description = A maior aresta dos blocos; as colunas dividem a largura do painel por igual

## Metadata panel
metadata-cover-background = Capa ao fundo
    .description = A capa da faixa atrás dos campos
metadata-display = Exibição
    .description = A ficha com o título na frente, ou uma tabela plana de rótulo e valor a partir do topo
metadata-display-sheet = Ficha
metadata-display-table = Tabela
metadata-edit-save = Salvar
metadata-field-bit-depth = Profundidade de bits
metadata-field-bitrate = Bitrate
metadata-field-codec = Codec
metadata-field-comment = Comentário
metadata-field-disc = Disco
metadata-field-file = Arquivo
metadata-field-sample-rate = Taxa de amostragem
metadata-field-track = Faixa
metadata-fields = Campos
    .description = Quais campos a ficha lista; um campo que a faixa não tem fica oculto
metadata-find-online = Encontrar metadados online...
metadata-no-library = Sem biblioteca
metadata-row-borders-description = O fio de cabelo abaixo de cada linha da tabela
metadata-source = Fonte
    .description = Seguir o que está tocando ou selecionado, ou ler a biblioteca como um todo
metadata-stripes-description = Tingir uma linha da tabela sim, outra não

## History panel
history-column-last-played = Tocada por último
history-descending = Decrescente
    .description = Rodar a ordenação ao contrário
history-empty-never = Todas as faixas já foram tocadas
history-empty-recent = Nenhuma audição ainda
history-headings = Quebrar a lista recente em sequências de álbum; Expandido acrescenta a capa e os números
history-sort-browse = Ordem de navegação
history-sort-date-added = Data de inclusão
history-sort-menu = Ordenar
    .description = Como as faixas nunca tocadas ficam ordenadas
history-title = Histórico
history-view-most = Mais tocadas
history-view-never = Nunca tocadas
history-view-recent = Tocadas recentemente
history-view-recent-short = Recentes
history-view-row = Visualização
    .description = Que recorte do registro de audições o painel mostra

## Folder tree panel
folder-tree-clear-scope = Limpar o escopo de pasta
folder-tree-collapse-all = Recolher tudo
folder-tree-cover-art = Capa
    .description = Mostrar a capa do álbum no lugar do ícone da linha, em pastas ou em músicas
folder-tree-cover-folders = Pastas
folder-tree-cover-songs = Músicas
folder-tree-empty = Nenhuma pasta na biblioteca ainda
folder-tree-follow-description = Revelar a faixa que está tocando e rolar até ela sempre que muda
folder-tree-nonmatch-folders = Pastas sem correspondência
    .description = Esconder as pastas sem correspondência, ou deixá-las escurecidas
folder-tree-nonmatch-songs = Músicas sem correspondência
    .description = Dentro de uma pasta que corresponde, escurecer as músicas soltas ou escondê-las
folder-tree-play-folder = Reproduzir a pasta
folder-tree-play-songs = { $count ->
    [one] Reproduzir
   *[other] Reproduzir { $count } músicas
}
folder-tree-resume-description = Rolar de volta para a faixa que está tocando depois que você para de navegar
folder-tree-scope-to-folder = Restringir o filtro à pasta
folder-tree-smooth-description = Deslizar até a faixa em vez de saltar
folder-tree-title = Árvore

## Art panel
art-always = Manter as capas recuadas mesmo quando nada toca; só uma capa sob o mouse aparece por inteiro
art-convert = Converter...
art-covers-section = Capas
matcher-section-matches = Correspondências
art-desaturate = Deixar todas as capas menos a do álbum tocando em tons de cinza; passar o mouse traz a cor de uma capa de volta
art-dim-while-playing = Apagar todas as capas menos a do álbum tocando; passar o mouse acende uma capa de novo
art-disc-style = Estilo de disco
    .description = Estilizar cada capa como um CD ou como o rótulo de um disco de vinil
art-edit-tags = Editar tags...
art-fill-panel = Preencher o painel
    .description = Dimensionar a capa central só pela altura do painel (pela largura quando vertical); as capas laterais passam da borda em vez de encolhê-la
art-follow-description = Centralizar o álbum que está tocando sempre que a faixa muda
art-glow = Brilho
    .description = Juntar a cor de destaque atrás da capa central; com a tintura da capa ligada, ela pega a cor do álbum que toca
art-layout-section = Layout
art-perspective = Perspectiva
    .description = Girar as capas laterais em 3D de verdade em vez do achatamento plano
art-reflections = Reflexos
    .description = Espelhar cada capa no chão abaixo da prateleira
art-resume-description = Centralizar o álbum que está tocando de novo depois que você para de navegar
art-shadows = Sombras
    .description = Uma sombra suave abaixo de cada capa
art-smooth-description = Deslizar até o álbum em vez de saltar
art-title = Carrossel de álbuns
art-vertical-layout = Layout vertical
    .description = Empilhar a prateleira como uma coluna que rola para cima e para baixo em vez de uma linha

## Playlists panel
playlists-columns = Quais colunas de faixa aparecem ao lado do título
playlists-delete = Excluir playlist
playlists-edit-query = Editar consulta...
playlists-empty = Nenhuma playlist ainda, adicione faixas ou use Nova playlist
playlists-headings = Quebrar as faixas de cada playlist em sequências de álbum; Expandido acrescenta a capa e os números
playlists-import-tooltip = Importar playlist
playlists-imported-fallback = Importada
playlists-new = Nova playlist...
playlists-new-smart = Nova playlist inteligente...
playlists-refuse-drag-out = Faixas de uma playlist inteligente não podem ser arrastadas para fora
playlists-refuse-edit-query = Edite a consulta para mudar o que uma playlist inteligente contém
playlists-refuse-smart-source = Uma playlist inteligente tira as faixas dela da consulta
playlists-remove = { $count ->
    [one] Remover da playlist
   *[other] Remover { $count } da playlist
}
playlists-rename = Renomear...
playlists-title = Playlists

## Queue panel
queue-clear = Limpar a fila
queue-empty = A fila está vazia
queue-headings = Quebrar a fila em sequências de álbum; Expandido acrescenta a capa e os números
queue-play-now = Tocar agora
queue-remove = { $count ->
    [one] Remover da fila
   *[other] Remover { $count } da fila
}
queue-title = Fila
queue-widget-always-modal = Sempre abrir como modal
    .description = Abrir a fila num modal toda vez, em vez de pular para um painel de fila que já esteja aberto
queue-widget-clear-queue = Limpar a fila
queue-widget-more = +{ $count } a mais
queue-widget-open-on-click = Abrir a fila ao clicar
    .description = Clicar no widget para pular para um painel de fila aberto, ou abrir a fila numa janela quando não houver nenhum
queue-widget-section-click = Clique
queue-widget-title = Widget de fila
queue-widget-up-next = A seguir

## Biography panel
biography-background = Fundo
    .description = A fanart do artista atrás do texto, escurecida e esmaecendo em direção à base
biography-fill-width = Preencher a largura
    .description = Deixar um cabeçalho alto ocupar a largura inteira em vez de ficar limitado e centralizado
biography-from-lastfm = Do Last.fm
biography-header-image = Imagem do cabeçalho
    .description = O banner largo do artista no topo, ou o retrato quando não há banner
biography-keep-aspect = Manter a proporção
    .description = Mostrar o cabeçalho nas proporções dele em vez de cortá-lo para preencher uma faixa
biography-listeners-count = { $count } ouvintes
biography-looking-up = Consultando { $name }
biography-no-artist-tag = Sem tag de artista
biography-no-text = Nenhuma biografia registrada
biography-not-found = Nada encontrado para { $name }
biography-plays-count = { $count } reproduções
biography-refresh = Atualizar
biography-similar-artists = Artistas similares
    .description = Artistas relacionados pelos dados de audição, no rodapé
biography-similar-heading = Artistas similares
biography-stats = Números
    .description = Ouvintes e reproduções no Last.fm, abaixo do nome
biography-tags = Tags
    .description = As tags de gênero como uma fileira de chips
biography-title = Biografia

## Status panel
status-count-albums = { $count ->
    [0] { $count } álbuns
    [one] 1 álbum
   *[other] { $count } álbuns
}
status-count-artists = { $count ->
    [0] { $count } artistas
    [one] 1 artista
   *[other] { $count } artistas
}
status-count-plays = { $count ->
    [0] { $count } reproduções
    [one] 1 reprodução
   *[other] { $count } reproduções
}
status-count-selected = { $count ->
    [one] { $count } selecionada
   *[other] { $count } selecionadas
}
status-count-tracks = { $count ->
    [0] { $count } faixas
    [one] 1 faixa
   *[other] { $count } faixas
}
status-readouts = Leituras
    .description = Arraste ao longo da barra para reordenar; arraste entre as linhas, ou use o x e o mais de um chip, para esconder e mostrar
status-scope-selection = Seleção
status-title = Status

## Output panel
output-detail-badge = Selo
output-detail-compact = Compacto
output-detail-expanded = Expandido
output-detail-label = Detalhe
    .description = Selo deixa tudo num chip com o resto ao passar o mouse; compacto dá ao destaque uma linha própria, para uma tira ao longo de uma borda; expandido acrescenta os motivos ao lado, ou abaixo quando o painel é estreito demais
output-device-name = Nome do dispositivo
    .description = Nomear o dispositivo em uso no destaque; desligado, a linha fica com o modo, a taxa e o formato
output-file-rate = Taxa do arquivo
    .description = Confirmar a taxa do próprio arquivo que toca quando nada a está convertendo. Uma conversão é sinalizada de qualquer jeito, já que é disso que o alerta trata
output-mode-exclusive = Exclusivo
output-mode-shared = Compartilhado
output-no-output = Sem saída
output-nothing-playing = Nada tocando
output-pick-another-device = Escolha outro dispositivo, ou desligue o exclusivo
output-headline-numbers = { $rate } Hz, { $channels } can., { $format }
output-headline = { $mode }, { output-headline-numbers }
output-headline-device = { $mode } em { $device }, { output-headline-numbers }
output-fell-back-to-shared = Exclusivo caiu para Compartilhado: { $why }
output-replaygain-levelling = O ReplayGain está nivelando este arquivo em { $db } dB
output-replaygain-short = ReplayGain { $db } dB
output-rate-resampled = O arquivo em reprodução é de { $rate } Hz, reamostrado para chegar ao dispositivo
output-rate-resampled-short = Arquivo de { $rate } Hz reamostrado
output-rate-native = O arquivo em reprodução é de { $rate } Hz, então nada está reamostrando
output-rate-native-short = Arquivo de { $rate } Hz, sem reamostragem
output-start-track-hint = Comece uma faixa para ver o formato que o dispositivo aceitou
output-title = Saída

## Track columns
columns-bits = Bits
columns-bpm = BPM
columns-codec = Codec
columns-cover = Capa
columns-fav = Fav
columns-gain = Ganho
columns-kbps = Kbps
columns-khz = kHz
columns-name = Nome
columns-number = Número
columns-scanned = Escaneado
columns-similar = Similar

## Filter panel
filter-add-column = Adicionar coluna
filter-add-column-tooltip = Adicionar coluna
filter-all = Todos
filter-clear-filters = Limpar filtros
filter-clear-selection = Limpar seleção
filter-empty = Escolha um campo para começar a filtrar
filter-remove-column = Remover coluna

## Search panel
search-chips-below = Abaixo
search-chips-inline = Na linha
search-filter-chips = Chips de filtro
search-placeholder = Buscar na biblioteca

## Playback panel
playback-buttons = Botões
    .description = Arraste ao longo da barra para reordenar; arraste entre as linhas, ou use o x e o mais de um chip, para esconder e mostrar
playback-continue-down-list = Continuar tocando, descendo a lista
playback-continue-off = Continuar tocando desligado
playback-continue-weighted = Continuar tocando, nunca tocadas primeiro
playback-crossfade-inside-albums = Dentro dos álbuns
playback-crossfade-off = Crossfade desligado
playback-crossfade-tip = Crossfade { $length }
playback-highlight-circle = Círculo
playback-highlight-square = Quadrado
playback-hold-draw = { $tip }. Segure para escolher o sorteio
playback-hold-length = { $tip }. Segure para escolher uma duração
playback-hold-order = { $tip }. Segure para escolher uma ordem
playback-loop-off = Repetição desligada
playback-loop-queue = Repetir a fila
playback-loop-track = Repetir esta faixa
playback-menu-continue = Botão continuar
playback-menu-crossfade = Botão crossfade
playback-menu-favourite = Botão favorito
playback-menu-random = Botão aleatório
playback-menu-rating = Estrelas de avaliação
playback-menu-stop = Botão parar
playback-menu-stop-after = Botão parar depois
playback-menu-volume = Botão volume
playback-pause = Pausar
playback-play-highlight = Destaque do play
    .description = O preenchimento de destaque do botão de play: um círculo, um quadrado suave, ou nenhum
playback-random-tip-random = Tocar uma faixa aleatória
playback-random-tip-similar = Tocar uma faixa parecida com esta
playback-seek-back-tip = 10 segundos para trás
playback-seek-forward-tip = 10 segundos para a frente
playback-shuffle-off = Embaralhar desligado
playback-shuffle-on = Embaralhar ligado, ordem { $order }
playback-stop-after-armed = Parar depois desta faixa, armado
playback-stop-after-tip = Parar depois desta faixa
playback-stop-tip = Parar e descarregar a faixa
playback-volume-tip-muted = Tirar o mudo, { $percent }%. Botão direito para a barra
playback-volume-tip-unmuted = Mudo, { $percent }%. Botão direito para a barra

## Track info panel
track-info-color-output-chip = Colorir o chip da saída
    .description = Deixar o chip assumir cores de alerta quando a saída cai para o compartilhado ou reamostra. Desligado, ele fica sempre no mesmo tom discreto, e a nota ao passar o mouse continua explicando o estado
track-info-cycle-every = Alternar a cada
    .description = Quanto tempo cada linha fica antes da transição
track-info-cycle-rows = Alternar as linhas
    .description = Mostrar as linhas do arranjo uma de cada vez numa linha só, com transição entre elas; uma linha sozinha se lê como ela mesma
track-info-delay = Atraso
    .description = Quanto a linha descansa em cada ponta antes de se mover de novo
track-info-marquee = Letreiro
    .description = O que uma linha longa demais para o painel faz: rastejar e voltar, ou girar sem fim
track-info-menu-overflow = Transbordo
track-info-next = A seguir: { $line }
track-info-opening = abrindo...
track-info-output-fallback = O dispositivo recusou a saída exclusiva, então a reprodução está passando pelo mixer compartilhado. O dispositivo informou: { $reason }
track-info-output-resample-exclusive = Este arquivo é de { $source } kHz e a placa aceitou { $device } kHz, então cada amostra está sendo convertida na saída. O dispositivo não quis rodar na taxa do próprio arquivo.
track-info-output-resample-mixer = Este arquivo é de { $source } kHz e o mixer está rodando a { $device } kHz, então cada amostra está sendo convertida na saída. O modo exclusivo entregaria à placa a taxa do próprio arquivo.
track-info-overflow-loop = Girar
track-info-overflow-scroll = Rolar
track-info-overflow-truncate = Cortar
track-info-queued-count = { $count } na fila
track-info-row-size = Tamanho da linha { $number }
track-info-speed = Velocidade
    .description = A que velocidade a linha rasteja
track-info-text-size = Tamanho do texto

## Seek panel
seek-ending = Fim
    .description = Contar o tempo que falta ou mostrar a duração completa
seek-ending-remaining = Restante
seek-ending-total = Total
seek-playhead = Cursor
    .description = Ocupar a altura inteira da barra ou se colar à linha
seek-playhead-full = Cheio
seek-playhead-line = Linha
seek-playhead-max-height = Altura máxima do cursor
    .description = Limitar o cursor cheio, centralizado na linha; 0 preenche o painel
seek-playhead-width = Largura do cursor
    .description = A largura da marca de posição que se move
seek-rounding = Arredondamento
    .description = O raio dos cantos da linha, até virar uma pílula na metade da espessura
seek-scrobble-marker = Marca de scrobble
    .description = Uma linha fina onde a faixa conta como scrobble no Last.fm
seek-show-timings = Mostrar os tempos
seek-thickness = Espessura
    .description = A altura da linha da faixa

## Volume panel
volume-pieces = Peças
    .description = Arraste ao longo da barra para reordenar; arraste entre as linhas, ou use o x e o mais de um chip, para esconder e mostrar. Com a porcentagem escondida, a dica do alto-falante a mostra
volume-readout = Leitura
    .description = Mostrar o nível como porcentagem ou como o ganho em decibéis que ele aplica
volume-readout-decibels = Decibéis
volume-readout-percent = Porcentagem
volume-stretch = Esticar
    .description = Deixar a barra preencher o painel em vez de limitar a largura dela
volume-tip-mute = Mudo
volume-tip-mute-level = Mudo, { $level }
volume-tip-unmute = Tirar o mudo
volume-tip-unmute-level = Tirar o mudo, { $level }

## Shared panel content
content-filter = Filtro
content-no-track = Nenhuma faixa
content-total-genres = Gêneros
content-total-time = Tempo total

## Shared panel chrome
panel-columns-description = Quais colunas de faixa aparecem
panel-headings = Títulos
panel-jump-to-playing = Ir para a faixa tocando
panel-menu-display = Exibição
panel-title-artists = Artistas
panel-title-genres = Gêneros
panel-title-oscilloscope = Osciloscópio
panel-title-particles = Partículas
panel-title-playback = Reprodução
panel-title-seek = Posição
panel-title-shader = Shader
panel-title-spectrogram = Espectrograma
panel-title-spectrum = Espectro
panel-title-theme-toggle = Alternar tema
panel-title-track-info = Info da faixa
panel-title-volume = Volume
panel-title-vu = Medidor VU
panel-title-waveform = Forma de onda

## Everything else
choice-both = Ambos
choice-dim = Escurecer
choice-hide = Ocultar
composite-add-panel = Adicionar painel
composite-host-settings = Configurações de { $host }
composite-move-left = Mover para a esquerda
composite-move-right = Mover para a direita
composite-remove = Remover
composite-replace = Substituir
group-panel-add-slot = Adicionar slot
group-panel-move-down = Mover para baixo
group-panel-move-up = Mover para cima
group-panel-remove-slot = Remover slot
group-panel-split-side-by-side = Dividir lado a lado
group-panel-split-stacked = Dividir empilhado
group-panel-swap-panels = Trocar os painéis
group-panel-title = Grupo
overlay-dim = Escurecer
    .description = O quanto o painel principal escurece sob o overlay revelado
overlay-title = Overlay
overlay-toggle = Alternar o overlay
shader-confirm-hint-after = alterna o shader de qualquer lugar.
shader-confirm-hint-before = Um shader pode deixar as janelas difíceis de usar. Reverter ou fechar esta janela devolve tudo como estava.
shader-confirm-keep = Manter
shader-confirm-question = Manter este shader de tela?
shader-confirm-revert = Reverter
shader-confirm-window-title = rox - Shader de overlay
slide-add = Adicionar slide
slide-next = Próximo slide
slide-previous = Slide anterior
slide-title = Slide
theme-toggle-to-dark = Mudar para o tema escuro
theme-toggle-to-light = Mudar para o tema claro
transport-favourite-add = Adicionar aos favoritos
transport-favourite-nothing = Nada para favoritar
transport-favourite-remove = Remover dos favoritos
transport-pieces = Peças
    .description = Arraste ao longo de uma linha para reordenar e entre as linhas para mover; o x e o mais de um chip escondem e mostram

## Stragglers picked up in the final sweep

duplicates-scanning = Escaneando...
about-copyright = Copyright © 2026
signal-name-placeholder = Nome do sinal
signals-empty = Nenhum sinal ainda. Adicione um, ou clique com o botão direito em qualquer controle vinculável.
signal-add = Adicionar sinal
panel-approve = Aprovar
panel-turn-off = Desligar
shader-from-file = De um arquivo...
arrange-add-row = Adicionar linha
smart-playlist-name-placeholder = Nome da playlist
smart-playlist-name-to-save = Dê um nome à playlist para salvá-la
panel-new-playlist = Nova playlist...
panel-edit-tags = Editar tags...
panel-edit-cover = Editar capa...
panel-rename-files = Renomear arquivos...
panel-convert = Converter...
panel-catalog-drag-anchor = Âncora de arraste
panel-catalog-spacer = Espaçador

## Duration and worker phrasing

pace-under-a-minute = menos de um minuto
pace-minutes = { $count ->
    [one] cerca de um minuto
   *[other] cerca de { $count } minutos
}
pace-hours = { $count ->
    [one] cerca de uma hora
   *[other] cerca de { $count } horas
}
pace-half-hours = cerca de { $value } horas
pace-days = { $count ->
    [one] cerca de um dia
   *[other] cerca de { $count } dias
}
pace-workers = { $count ->
    [one] { $count } processo
   *[other] { $count } processos
}
tasks-rest-takes = , o resto leva { $estimate }
tasks-measuring-takes = , medi-las leva { $estimate }
tasks-working-out-takes = , descobri-las leva { $estimate }
tasks-time-left = , faltando { $left }
tasks-failed-suffix = ({ $count } com falha)
tasks-file-suffix = - { $file }
tasks-no-beat-suffix = ({ $count } sem batida clara)
tasks-estimate-at-workers = ({ tasks-estimate-at })

## Panel vanity names

panel-title-art-view = Vista das capas
panel-title-artist-grid = Grade de artistas
panel-title-genre-grid = Grade de gêneros
panel-title-biography = Biografia
panel-title-cover-art = Capa
panel-title-drag-anchor = Âncora de arraste
panel-title-drawer = Gaveta
panel-title-eq-widget = Widget de EQ
panel-title-filter = Filtro
panel-title-folder-tree = Árvore de pastas
panel-title-group = Grupo
panel-title-history = Histórico
panel-title-lyrics = Letra
panel-title-menu = Menu
panel-title-metadata = Metadados
panel-title-mini-toggle = Alternar mini
panel-title-output = Saída
panel-title-overlay = Overlay
panel-title-playlists = Playlists
panel-title-queue = Fila
panel-title-queue-widget = Widget de fila
panel-title-search = Busca
panel-title-slide = Slide
panel-title-spacer = Espaçador
panel-title-stats-widget = Widget de estatísticas
panel-title-vu-meter = Medidor VU
panel-title-window-controls = Controles de janela

## Relative time and the output headline

ago-just-now = agora mesmo
ago-minutes = há { $count } min
ago-hours = há { $count } h
ago-days = há { $count } d
ago-weeks = há { $count } sem
ago-years = há { $count } a

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
    [one] { $count } dia
   *[other] { $count } dias
}
span-weeks = { $count ->
    [one] { $count } semana
   *[other] { $count } semanas
}
span-years = { $count ->
    [one] { $count } ano
   *[other] { $count } anos
}
span-pair = { $first }, { $second }
unit-percent = { $value }%

settings-audio-output-headline = { $mode }{ $note } em { $device }, { $rate } Hz, { $channels } can., { $format }
settings-audio-output-experimental =  (experimental)

## ML model catalog

settings-mlmodels-description = { $summary }. { $dim } valores por faixa. { $licence }
settings-mlmodels-on-disk = , { $size } no disco
settings-mlmodels-to-download = , { $size } para baixar
model-summary-dsp-timbre-1 = Interno, sem download. Um resumo da energia por banda, da forma espectral e da taxa de ataques de cada faixa. Grosseiro ao lado de uma rede treinada, mas não precisa de nada e roda em qualquer lugar
model-summary-panns-cnn10 = Uma rede convolucional treinada no AudioSet para reconhecer o que um som é. A descrição de 512 valores que ela faz de uma faixa é muito mais rica que o esboço interno, ao custo de um download de 24 MB e de uma análise mais lenta

## Shipped workspaces

workspace-shipped-default = (Padrão)
workspace-shipped-default-blurb = A cara do rox recém-instalado: superfícies translúcidas sobre a área de trabalho, sem moldura de janela, sem tintura pelas capas. O ponto de partida do qual todo outro visual aqui se afasta.
workspace-shipped-catrox-blurb = O skin de foobar2000 que começou tudo, reconstruído: uma renderização circular da capa como CD, os campos de metadados descendo à esquerda e faixas agrupadas por álbum com pontos de avaliação.
workspace-shipped-critters-blurb = O aplicativo inteiro como uma impressão de 1 bit: um dither ordenado sobre cada superfície, tons que esmagam com o sub-grave, e uma parede de ruído que se contorce com a música. Inspirado em Critters for Sale.
workspace-shipped-diffuse-blurb = Só o álbum que está tocando: a capa e o cartão de reprodução como um grupo só preenchendo a janela, superfícies transparentes sobre o fundo, sem costuras. A biblioteca, a fila e a letra esperam numa gaveta na borda direita, deslizando sobre a música quando o mouse passa pela alça. Monocromático, então a cor vem das capas.
workspace-shipped-foobar-blurb = O layout com o qual este projeto inteiro discute. Painéis opacos, colunas de filtro por artista e álbum, uma tabela de faixas densa, e a barra de menus exatamente onde ela sempre esteve.
workspace-shipped-llama-winamp-blurb = O Winamp do jeito que você lembra, não do jeito que ele era. Tahoma, escuro, sem moldura, um espectro pontilhado no topo e um modo recolhido no layout mini.
workspace-shipped-metro-blurb = Painéis planos e linhas folgadas em Segoe UI, com a tintura pelas capas ligada para que a paleta inteira siga a capa que estiver tocando.
workspace-shipped-phosphor-blurb = Monoespaçado em tudo. Consolas, verde no preto, sem capa na reprodução rápida: um terminal que por acaso toca música.
