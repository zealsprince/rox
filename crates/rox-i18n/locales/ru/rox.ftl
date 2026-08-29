### Русский. Отражает en-CA/rox.ftl ключ в ключ; тест на паритет
### в rox-i18n за этим следит. Ключи в kebab-case с префиксом по
### поверхности; описание строки — это атрибут её сообщения.

## Shared widgets
tracking-title = Слежение
tracking-follow = Следовать за воспроизведением
tracking-resume = Возвращаться при простое
tracking-smooth = Плавная прокрутка
align-row = Выравнивание
    .description = Где размещается содержимое, когда у панели есть запас места
valign-row = Вертикальное выравнивание
    .description = Где размещается содержимое, когда у панели есть запас высоты
valign-top = Сверху
valign-middle = По центру
valign-bottom = Снизу
letter-rail-compact = Компактная полоса
    .description = Ограничить полосу одной строкой с прокруткой вместо переноса
letter-rail-side = Положение полосы
    .description = На какой стороне стены расположена полоса

## Panel source and search rows
source-track = Трек
    .description = Следовать за тем, что играет, или за выбранным в медиатеке
source-follow-playing = Следовать за воспроизведением
source-follow-selection = Следовать за выбором
source-playing = Играет
source-selected = Выбрано
query-search = Поиск
query-search-box = Поле поиска
    .description = Показывать поле поиска; запрос действует, только пока оно на экране
query-source = Источник поиска
    .description = Следовать общему поисковому запросу, фильтровать по собственному полю этой панели или показывать то, что выбрано в другой панели
query-source-shared = Общий
query-source-own = Собственный
query-source-selection = Выбор

## Signals and routes
signal-source = Источник
    .description = За чем следит сигнал: Полоса отслеживает один диапазон частот, Уровень отслеживает всю смесь, Атака пульсирует на каждом ударе в диапазоне, Триггер выдаёт импульс, когда диапазон достигает своего порога, Сумма накапливает другой сигнал со временем
signal-kind-band = Полоса
signal-kind-level = Уровень
signal-kind-onset = Атака
signal-kind-trigger = Триггер
signal-kind-total = Сумма
signal-response = Отклик
signal-response-pulse = Как долго звенит каждый импульс, прежде чем затухнуть
signal-response-drift = 0 идёт за музыкой вплотную, 100 тянется следом
signal-threshold = Порог
signal-threshold-trigger = Уровень, которого должен достичь диапазон, чтобы выдать импульс; снова он сработает только после того, как уровень опустится ниже метки на индикаторе выше
signal-threshold-gate = Ниже этого порога сигнал равен нулю, а выше выход снова растёт от нуля, так что тихие места не двигают регулятор. Метка на индикаторе выше показывает, где проходит порог
signal-low-bound = Нижняя граница
signal-high-bound = Верхняя граница
signal-adds-up = Что суммируется
    .description = Какой сигнал здесь накапливается; сумма растёт, пока тот показывает много, и замирает, пока он тихий
signal-aggregate-nothing = Нечему следовать
signal-aggregate-pick = Выбрать сигнал
signal-aggregate-alone = В пуле нет другого сигнала для суммирования, поэтому здесь ноль. Добавьте сигнал, и он появится в списке.
signal-aggregate-unpicked = Ничего не выбрано, поэтому сумма остаётся на нуле. Выберите сигнал выше.
signal-rate = Скорость
    .description = Оборотов в секунду при полном входе; после 1 значение сбрасывается в 0 и растёт дальше, а шейдер читает это как фазу
signal-reset-on-track = Сброс на новом треке
    .description = Плавно спускать значение к нулю, когда начинается новая песня, чтобы фаза не начиналась с суммы прошлой
signal-flush = Обнулить
signal-routes-in-panel = { $count ->
    [one] { $count } маршрут в этой панели
    [few] { $count } маршрута в этой панели
    [many] { $count } маршрутов в этой панели
   *[other] { $count } маршрута в этой панели
}
    .description = Вернуть к нулю прямо сейчас. Значение стекает за мгновение, а не обрывается, так что ничего следящего за ним не дёргается
route-header = Маршрут
route-signal = Сигнал
    .description = За каким общим сигналом идёт этот маршрут; настройка сигнала здесь настраивает каждый маршрут на нём
route-new-signal = Новый сигнал
route-shared-note = Общее для каждого маршрута на этом сигнале
route-signal-gone = Сигнал этого маршрута пропал; регулятор держит значение своего ползунка, пока выше не выбран другой.
route-range-note = Диапазон только для этого параметра
route-quiet = Тишина
    .description = Что показывает регулятор в тишине, как доля от собственной настройки
route-loud = Громко
    .description = Что он показывает при полном сигнале; 100% — это собственное значение ползунка, ниже Тишины модуляция идёт вниз
route-slot = Слот
    .description = Какой из шестнадцати сигнальных слотов шейдера заполняет этот маршрут
route-slot-quiet-description = Что показывает слот в тишине
route-slot-loud-description = Что он показывает при полном сигнале; ниже Тишины слот идёт задом наперёд
route-slot-signal-description = За каким общим сигналом идёт этот маршрут
route-slot-signal-gone = Сигнал этого маршрута пропал; слот показывает ноль, пока не выбран другой.
route-add = Добавить маршрут
route-unrouted = Без маршрута
route-pick-slot = Выбрать слот
route-pick-signal = Выбрать сигнал
route-no-signal = нет сигнала
route-no-signals-yet = Следить пока не за чем, сигналов нет. Создайте сигнал, и он появится здесь; до тех пор слот показывает ноль.
route-open-signals = Открыть сигналы
route-create-signal = Создать новый сигнал

## Panel settings window
panel-settings = Настройки панели
panel-menu-label = Панель
panel-save-as-preset = Сохранить как пресет
panel-rename = Переименовать
panel-rename-name = Название
panel-rename-note = Показывается как вкладка панели; пустое возвращает встроенное название
panel-rename-hint-after = чтобы переименовать
panel-was-closed = Панель была закрыта
panel-reset = Сбросить
panel-inverse = Инверсия
panel-apply-song-theme = Применить тему трека
panel-page-appearance = Внешний вид
panel-page-behavior = Поведение
panel-page-shader = Шейдер
panel-section-placement = Размещение
panel-section-size = Размер
panel-section-opacity = Непрозрачность
panel-section-frame = Рамка
panel-section-colors = Цвета
panel-section-font = Шрифт
panel-section-shader = Шейдер
panel-section-signals = Сигналы
panel-section-slots = Слоты
panel-awaiting-approval = Ожидает подтверждения
panel-size-off = Выкл
panel-locked = Закреплена
    .description = Закрепить панель на месте; её нельзя перетащить или переставить в доке
panel-drag-anchor = Область перетаскивания
    .description = Перетаскивание в любом месте панели двигает окно, а обычные клики по-прежнему попадают в её элементы; для макетов без оформления окна
panel-slot-controls = Управление слотами
    .description = Показывать угловые кнопки для замены и удаления панелей, которые размещены внутри этой. Если их скрыть, макет всё равно правится через дерево на странице «Рабочее пространство» в настройках
panel-min-width = Мин. ширина
    .description = Где изменение размера перестаёт сжимать панель. Берётся как написано, в том числе ниже собственного минимума панели, так что компактная полоса может стать уже стандартной; пустое поле оставляет минимум как есть
panel-max-width = Макс. ширина
    .description = Ограничить ширину панели, чтобы она не растягивалась, когда окно становится шире
panel-min-height = Мин. высота
    .description = Где изменение размера перестаёт сжимать панель по высоте. Берётся как написано, в том числе ниже собственного минимума панели, так что компактная полоса может стать ниже стандартной; пустое поле оставляет минимум как есть
panel-max-height = Макс. высота
    .description = Ограничить высоту панели, чтобы она не растягивалась, когда окно становится выше
panel-own-opacity = Своя непрозрачность поверхности
    .description = Дать этой панели собственную непрозрачность поверх подложки вместо общей для приложения
panel-surface-opacity = Непрозрачность поверхности
panel-margin = Внешний отступ
    .description = Втянуть панель внутрь её ячейки, чтобы в зазоре просвечивала подложка
panel-padding = Внутренний отступ
    .description = Место внутри края панели, залитое её собственным фоном
panel-rounding = Скругление
    .description = Скруглить углы панели, открывая подложку
panel-border = Граница
    .description = Линия по краю панели в цвете роли «Граница»; сторона с нулём не рисуется
panel-font = Шрифт
    .description = Гарнитура панели; по умолчанию следует шрифту приложения
panel-font-size = Размер шрифта
    .description = Размер текста панели относительно шрифта приложения; строки масштабируются вместе с ним
panel-surface-shader = Шейдер поверхности
    .description = Выполнять WGSL-шейдер по телу этой панели, под экранным шейдером приложения
panel-run-when-idle = Работать в простое
    .description = Продолжать рисовать кадры, пока звук молчит. Если выключить, шейдер замирает на последнем кадре и панель ничего не стоит
panel-shader-is-scene = Этот шейдер является сценой, поэтому он закрывает тело панели, а не рисует поверх него. Он пришёл из набора или из старой конфигурации; в списке выше только те шейдеры, которые оставляют панель читаемой.

## Shader picker and saving
shader-source = Источник
shader-pick-none = Нет
shader-reload = Перезагрузить
shader-edit-as-file = Править как файл
shader-make-private-copy = Сделать личную копию
shader-save-replace = Заменить
shader-save-to-workspace = Сохранить в рабочее пространство
shader-save-replaces = Заменяет шейдер, который в этом рабочем пространстве уже называется { $name }. Каждая панель с этим именем изменится вместе с ним
shader-save-adds = Добавляет его к шейдерам этого рабочего пространства под именем { $name }. Его может взять любая панель, и правка обновит их все
shader-group-examples = Примеры
shader-group-this-workspace = Это рабочее пространство
shader-group-scenes = Сцены
shader-group-workspace-scenes = Сцены рабочего пространства
shader-group-overlays = Наложения
shader-group-workspace-overlays = Наложения рабочего пространства

## Saving a panel preset
preset-save = Сохранить пресет
preset-save-name = Название пресета
preset-save-replaces = Заменяет пресет, который в этом рабочем пространстве уже называется { $name }
preset-save-hint-after = чтобы сохранить
preset-back-from = Вернуть его можно через
preset-back-add-panel = Добавить панель
preset-back-then = затем
preset-back-presets = Пресеты
preset-back-tail = в меню любой панели. Пресеты принадлежат только этому рабочему пространству; в другом их не будет.

## Keyboard hints
hint-press = Нажмите
hint-key-enter = Enter

## Settings: language
settings-language = Язык
    .description = Язык интерфейса. Системный сверяется со списком ОС и откатывается к английскому, когда ничего не совпало
    .keywords = перевод локаль язык интерфейс
settings-language-system = (Системный язык)
settings-language-search = Поиск языков
picker-no-matches = Нет совпадений
settings-search-no-matches = Нет совпадений для «{ $text }»

## Embed dialog
bake-window-title = rox - Встроить сохранённые метаданные
bake-title = Встроить сохранённые метаданные
bake-intro = Записывает сохранённые метаданные в сами файлы, чтобы их прочитал и другой плеер. Ничего не пересчитывается.
bake-formats = Только MP3 и FLAC; другие форматы и треки из CUE пропускаются
bake-source-lyrics = Тексты песен
bake-source-gain = ReplayGain
bake-source-acoustic = Акустические описания
bake-detail-nothing = встраивать нечего
bake-detail-only-skipped = записывать нечего, пропущено: { $skipped }
bake-detail-writes = { $count ->
    [one] { $count } файл к записи
    [few] { $count } файла к записи
    [many] { $count } файлов к записи
   *[other] { $count } файла к записи
}
bake-detail-writes-skipped = { $count ->
    [one] { $count } файл к записи, пропущено: { $skipped }
    [few] { $count } файла к записи, пропущено: { $skipped }
    [many] { $count } файлов к записи, пропущено: { $skipped }
   *[other] { $count } файла к записи, пропущено: { $skipped }
}
bake-error-read = Не удалось прочитать медиатеку: { $error }
bake-survey-counting = Просмотр медиатеки...
bake-survey-progress = Чтение тегов, { $done } из { $total }
bake-nothing-to-embed = Встраивать нечего: в файлах уже есть всё, что хранит rox
bake-rewrites = { $count ->
    [one] Будет перезаписан { $count } файл
    [few] Будет перезаписано { $count } файла
    [many] Будет перезаписано { $count } файлов
   *[other] Будет перезаписано { $count } файла
}
bake-hint-before = Нажмите
bake-hint-key = Enter
bake-hint-after = чтобы встроить
bake-embed = Встроить
bake-cancel = Отмена
bake-summary-files = { $count ->
    [one] { $count } файл
    [few] { $count } файла
    [many] { $count } файлов
   *[other] { $count } файла
}
bake-summary-updated = Обновлено { $files }
bake-summary-stopped = Остановлено, обновлено { $files }
bake-summary-skipped = , пропущено { $count }
bake-summary-failed = , ошибок: { $count }

## Arrange editors and header pieces
arrange-shown = Показано
arrange-hidden = Скрыто
tile-face-mosaic = Мозаика обложек
tile-face-tinted = Тонированная мозаика
tile-face-gradient = Градиентная карточка
tile-face-color = Цветная карточка
head-piece-artist = Исполнитель
head-piece-album = Альбом
head-piece-year = Год
head-piece-genre = Жанр
head-piece-quality = Качество
head-piece-tracks = Треки
head-piece-time = Время
head-piece-spacer = Распорка
head-piece-divider = Разделитель
head-piece-art = Обложка
head-unknown = Неизвестно
status-item-count = Количество
status-item-time = Время
status-item-albums = Альбомы
status-item-artists = Исполнители
status-item-plays = Прослушивания
volume-item-icon = Значок
volume-item-slider = Ползунок
volume-item-percent = Проценты

## Filter chips and search menus
filter-field-artist = Исполнитель
filter-field-album-artist = Исполнитель альбома
filter-field-album = Альбом
filter-field-genre = Жанр
filter-field-year = Год
filter-field-folder = Папка
filter-unknown = Неизвестно
filter-clear = Очистить
query-show-search-box = Показать поле поиска
query-own-query = Собственный запрос
query-shared-query = Общий запрос
headers-off = Выкл
headers-compact = Компактные
headers-expanded = Развёрнутые

## Panel context menu
panel-dock-back = Вернуть в док
panel-pop-out = Открыть в окне
panel-close = Закрыть
panel-duplicate = Дублировать
panel-reveal-in-browser = Показать в файловом менеджере
panel-play-next = Воспроизвести следующим
panel-add-to-queue = Добавить в очередь
panel-add-to-playlist = Добавить в плейлист
panel-favourite-add = Добавить в избранное
panel-favourite-remove = Убрать из избранного
shader-pick-missing = { $name } (отсутствует)
shader-pick-custom = Свой

## Shipped shader examples
shader-blurb-plasma = Плывущий цвет, взятый из одних только униформ, поэтому обходится в простой квад.
shader-blurb-trails = Размазывает собственный прошлый кадр, поэтому идёт на экранном проходе.
shader-blurb-sheen = Виньетка и плывущий блик, прозрачное наложение для панели, которая и так что-то рисует.
shader-blurb-shadow = Тень, которую отбрасывают собственный текст и элементы панели, считанные с маски.
shader-blurb-cover = Обложка играющего трека, вписанная в поля поверх заливки её же цветом.
shader-blurb-badge = Обложка как небольшая карточка в углу, со слотом, чтобы её подвинуть.
shader-blurb-lamp = Свет, который следует за курсором и отзывается на клики, прозрачное наложение.
shader-blurb-cube = Каркасный куб, кувыркающийся в псевдотрёхмерном пространстве, нарисованный как добавленный свет.
shader-blurb-bloom = Плывущие шары, засвеченные через второй проход половинного размера, вся цепочка в миниатюре.
shader-blurb-tube = Проигрывает панель под собой заново через изогнутое стекло ЭЛТ, со строчными полосами и всем прочим.

## Transport strip pieces
seek-item-elapsed = Прошло
seek-item-strip = Полоса
seek-item-ending = Остаток
seek-item-duration = Длительность
info-item-track-no = Номер трека
info-item-title = Название
info-item-duration = Длительность
info-item-next = Далее
info-item-queued = В очереди
info-item-output = Вывод
info-item-favourite = Избранное
info-item-rating = Оценка
playback-item-previous = Предыдущий
playback-item-seek-back = Перемотка назад
playback-item-play = Воспроизвести
playback-item-seek-forward = Перемотка вперёд
playback-item-next = Следующий
playback-item-stop = Стоп
playback-item-volume = Громкость
playback-item-loop = Повтор
playback-item-shuffle = Перемешивание
playback-item-continue = Продолжение
playback-item-crossfade = Кроссфейд
playback-item-random = Случайный
playback-item-stop-after = Стоп после
playback-item-favourite = Избранное
playback-item-rating = Оценка

## Dock chrome
dock-empty-tab = Пустая вкладка
dock-unnamed = Без названия
dock-tiles = Плитки
dock-zoom-in = Приблизить
dock-zoom-out = Отдалить
dock-collapse = Свернуть
dock-expand = Развернуть

## Shader picker notes
shader-note-empty = Выберите пример для начала или укажите rox файл .wgsl с фрагментной стадией, определяющей fs_user(uv)
shader-note-missing = { $name } больше нет среди шейдеров этого рабочего пространства, поэтому ничего не рисуется. Выберите здесь что-то другое, и у панели появится собственный источник.
shader-note-shared = Общий для всего рабочего пространства. Правка обновит каждую поверхность, которая его использует.
shader-note-file = { $path }. Ваши сохранения перезагружаются прямо во время рисования, а исходник хранится внутри макетов и наборов, так что он работает и на машине, где этого файла никогда не было.
shader-note-custom = Этот источник хранится внутри своего макета или набора, файла за ним нет. «Править как файл» запишет его наружу и подхватит ваши сохранения.

## Panel pages and shared sides
panel-page-layout = Макет
panel-page-view = Вид
panel-page-content = Содержимое
panel-page-source = Источник
panel-page-bindings = Привязки
panel-page-emitters = Эмиттеры
panel-page-forces = Силы
panel-page-scene = Сцена
side-left = Слева
side-right = Справа
genre-face-mosaic = Мозаика
genre-face-tinted = Тонированная
genre-face-gradient = Градиент
genre-face-color = Цвет

## Library panel
panel-title-library = Медиатека
library-play = Воспроизвести
library-play-album = Воспроизвести альбом
library-play-group = Воспроизвести группу
library-play-tracks = Воспроизвести треки: { $count }
library-play-similar = Воспроизвести похожее
library-filter-by-album = Фильтр по альбому
library-filter-by-artist = Фильтр по исполнителю
library-jump-to-playing = Перейти к играющему
library-menu-display = Отображение
library-disc = Диск { $number }
library-empty-title = Откройте папку с музыкой
library-empty-note = Она попадёт в медиатеку при сканировании (flac, mp3, wav)
library-headers = Заголовки
    .description = Разрывы групп поверх списка; сортировка сохраняет цельность серий, а при поиске список показывается плоским
library-group-by = Группировать по
    .description = По чему разбиваются заголовки; жанр и год пересортируют список
library-header-row = Строка заголовка
    .description = Что показывает однострочный заголовок, слева направо; распорка или разделитель разводит стороны
library-header-lines = Строки заголовка
    .description = Строки блока сверху вниз; пустая строка выпадает
library-follow-description = Прокручивать к играющей строке при каждой смене трека
library-resume-description = Прокручивать обратно к играющей строке, когда вы перестали листать
library-smooth-description = Плавно доезжать до строки, а не прыгать
library-columns = Столбцы
    .description = Какие столбцы показывать; перетаскивайте заголовки в панели, чтобы менять их порядок и ширину
library-column-headers = Заголовки столбцов
    .description = Строка сортируемых заголовков над списком; если её скрыть, столбцы сохранят порядок и ширину
library-compact-plays = Компактные прослушивания
    .description = Столбец прослушиваний как небольшое число с чёрточкой рядом
library-line-height = Высота строки заголовка
    .description = Одна строка заголовка; блоки занимают столько строк, сколько нужно, независимо от строк треков
library-text-size = Размер текста
    .description = Текст строк заголовка, независимо от высоты строки, так что обложка растёт сама по себе
library-flush-background = Фон заподлицо
    .description = Рисовать заголовки на фоне списка, а не на приподнятом оттенке; окраска по треку меняет их вместе
library-gap-above = Зазор сверху
    .description = Отрезается от верха блока; сквозь него виден список, а строки поджимаются, чтобы поместиться
library-gap-below = Зазор снизу
    .description = То же самое под блоком, перед его треками
library-section-rows = Строки
library-row-height = Высота строки
    .description = Строки треков; текст следует за ними, и оба масштабируются со шрифтом приложения
library-row-spacing = Интервал строк
    .description = Дополнительная высота каждой строки; воздух без укрупнения текста
library-stripes = Чередующаяся подсветка
    .description = Подкрашивать каждую вторую строку трека, чтобы длинный список читался
library-row-borders = Границы строк
    .description = Волосяная линия под каждой строкой трека
library-art-description = Плитка развёрнутых заголовков: обложка, портрет исполнителя или оформление жанра
library-art-rounding = Скругление обложки
    .description = Скруглить углы обложки
library-art-position = Положение обложки
    .description = С какой стороны блока размещается плитка развёрнутых заголовков
library-art-margin = Отступ обложки
    .description = Вписать плитку внутрь блока; она уменьшается, чтобы остаться квадратной
library-circular-portraits = Круглые портреты
    .description = При группировке по исполнителю скруглять плитки до полного круга, минуя регулятор скругления
library-genre-face = Оформление жанра
    .description = При группировке по жанру плитка показывает обложки, обложки в цвете жанра или цветную карточку под геометрией

## Album grid panel
panel-title-album-grid = Сетка альбомов
grid-menu-scroll = Прокрутка
grid-menu-sort = Сортировка
grid-sort-artist = Исполнитель
grid-sort-album = Альбом
grid-sort-year = Год
grid-sort-added = Недавно добавленные
grid-sort-plays = Самые прослушиваемые
grid-letter-rail = Алфавитная полоса
    .description = Инициалы вдоль края стены; клик переходит к первому альбому на эту букву
grid-vertical-scroll = Вертикальная прокрутка
grid-horizontal-scroll = Горизонтальная прокрутка
grid-jump-to-playing = Перейти к играющему
grid-library-empty = Медиатека пуста
grid-play-albums = Воспроизвести альбомы: { $count }
grid-vertical-layout = Вертикальный макет
    .description = Прокручивать стену вверх и вниз, строками по ширине; если выключено, прокрутка идёт влево и вправо, столбцами по высоте
grid-follow-description = Прокручивать к играющему альбому при каждой смене трека
grid-resume-description = Возвращаться к играющему альбому, когда вы перестали листать
grid-smooth-description = Плавно доезжать до альбома, а не прыгать
grid-section-dimming = Затемнение
grid-section-tiles = Плитки
grid-dim-while-playing = Затемнять при воспроизведении
    .description = Гасить все обложки, кроме играющего альбома; наведение снова зажигает плитку
grid-dim-amount = Сила затемнения
    .description = Насколько гаснут остальные обложки; 100% скрывает их
grid-desaturate = Обесцвечивать при воспроизведении
    .description = Сводить все обложки, кроме играющего альбома, в оттенки серого; наведение возвращает плитке цвет
grid-always = Всегда
    .description = Держать обложки притушенными, даже когда ничего не играет; в полную силу видна только плитка под курсором
grid-show-titles = Показывать подписи
    .description = Печатать альбом и исполнителя под каждой обложкой, как в iTunes, а не только при наведении
grid-title-alignment = Выравнивание подписей
    .description = Выровнять подписи под их обложками
grid-tile-size = Размер плитки
    .description = Длинная сторона плиток с обложками; столбцы делят ширину панели поровну
grid-gap = Зазор
    .description = Место между обложками; ноль укладывает их вплотную
grid-art-rounding-description = Скруглить углы каждой обложки; 100% даёт круг

## Settings: sidebar pages
settings-page-appearance = Внешний вид
settings-page-application = Приложение
settings-page-audio = Звук
settings-page-development = Разработка
settings-page-integrations = Интеграции
settings-page-keymap = Клавиши
settings-page-library = Медиатека
settings-page-mcp = MCP
settings-page-ml-models = ML-модели
settings-page-playback = Воспроизведение
settings-page-providers = Провайдеры
settings-page-shader = Шейдер
settings-page-storage = Хранилище
settings-page-workspace = Рабочее пространство

## Settings: appearance
settings-appearance-backdrop-all-windows = Все окна
    .description = Подкладывать подложку и под дочерние окна: настройки, редакторы, диалоги, отделённые панели. Если выключить, подложка и прозрачность останутся у окон рабочего пространства
settings-appearance-backdrop-strength = Сила подложки
    .description = Насколько сильно за ними проступает подложка из обложки
settings-appearance-border = Граница
    .description = Линия по краю каждой панели в цвете роли «Граница»; сторона с нулём не рисуется
settings-appearance-colors-locked-note = Окраска по треку включена, поэтому цвета задаёт играющий трек, и экспорт сохраняет именно их. Выключите её выше, чтобы править цвета вручную
settings-appearance-design-mode = Режим дизайна
    .description = Правка макета прямо на месте: пункты меню панели для добавления, переименования, дублирования, отделения и закрытия, элементы, которые контейнер накладывает на свои слоты, и перетаскивание вкладок. Если выключить, всё это скрыто; страница «Рабочее пространство» по-прежнему правит дерево
    .keywords = правка макет перестановка блокировка
settings-appearance-font = Шрифт
    .description = Гарнитура для всего приложения; панель может переопределить её в своих настройках
    .keywords = гарнитура шрифт текст
settings-appearance-font-size = Размер шрифта
    .description = Базовый размер текста, от которого масштабируется текст каждой панели; элементы и значки держат свой размер
settings-appearance-hide-menubar = Скрывать строку меню
    .description = Держать строку меню скрытой и выводить её над доком, пока зажат Alt. Двойное нажатие Alt оставляет её на экране, и тогда её кнопки нажимаются обычным кликом
settings-appearance-icons-intro = Набор — это папка с SVG, которая заменяет встроенные значки; переключение вступает в силу при следующем запуске
settings-appearance-icons-open-folder = Открыть папку
settings-appearance-inverse-from-dark = Инверсия из тёмной темы
settings-appearance-inverse-from-light = Инверсия из светлой темы
settings-appearance-keep-theme = Держать тему
    .description = Держать активную тему, даже когда яркость обложки перевернула бы её; окраска по треку всё равно подкрашивает цвет
settings-appearance-margin = Внешний отступ
    .description = Втянуть каждую панель внутрь её ячейки; панель может переопределить это в своих настройках
settings-appearance-new-pack = Новый набор
settings-appearance-os-decorations = Оформление окон ОС
    .description = Заголовок и рамки от ОС на основных окнах; если выключить, остаются кнопки окна и панели с областью перетаскивания
settings-appearance-pack-name-placeholder = Название набора
settings-appearance-padding = Внутренний отступ
    .description = Место внутри края каждой панели, залитое её собственным фоном
settings-appearance-palette-export = Экспорт
settings-appearance-palette-import = Импорт
settings-appearance-panel-seams = Швы панелей
    .description = Волосяная линия между плитками панелей; если выключить, манипуляторы размера станут невидимыми, но тянуть их всё равно можно
settings-appearance-resize-border = Рамка изменения размера
    .description = Изменение размера основных окон перетаскиванием их краёв; работает только при выключенном оформлении ОС, а если это выключить, размер меняется прикреплением к краям и Win+стрелка
settings-appearance-rounding = Скругление
    .description = Скруглить углы каждой панели, открывая подложку
settings-appearance-section-colors = Цвета
settings-appearance-section-frame = Рамка
settings-appearance-section-icons = Значки
settings-appearance-section-interface = Интерфейс
settings-appearance-section-theming = Окраска
settings-appearance-section-transparency = Прозрачность
settings-appearance-section-typography = Типографика
settings-appearance-song-theming = Окраска по треку
    .description = Подкрашивать палитру и фон окон обложкой играющего трека
settings-appearance-surface-opacity = Непрозрачность поверхности
    .description = Насколько плотными читаются поверхности приложения поверх подложки
settings-appearance-theme = Тема
    .description = Палитра, которой рисует приложение, и та, которую правит редактор цветов ниже; Системная следует светлому или тёмному режиму ОС
settings-appearance-theme-dark = Тёмная
settings-appearance-theme-light = Светлая
settings-appearance-theme-system = Системная

## Settings: application
settings-application-check-updates = Проверять обновления
    .description = Искать более новый выпуск раз в день при запуске rox; окно «О программе» проверяет прямо сейчас в любом случае
settings-application-download-updates = Скачивать обновления
    .description = Когда проверка находит более новый выпуск, скачивать и готовить его в фоне; следующий запуск пойдёт уже с ним
settings-application-enable-ai = Включить возможности ИИ
    .description = Разрешить ИИ-инструментам говорить с rox: добавляет поддержку MCP и загрузку ML-моделей, а их страницы появляются в боковой панели.
settings-application-lock-panel-resize = Запретить изменение размера панелей
    .description = Разделители панелей двигаются только при включённом режиме дизайна, чтобы перетаскивание у шва не сбило готовый макет
settings-application-portable-copying = Копирование данных...
settings-application-portable-mode = Портативный режим
    .description = Держать настройки, медиатеку и кеши в папке rox-data рядом с исполняемым файлом, чтобы плеер переезжал вместе со своими данными. Выключение возвращает системную папку и оставляет rox-data на месте
settings-application-portable-not-writable = Папка приложения недоступна для записи
settings-application-portable-restart-note = Применится при следующем запуске; этот сеанс останется на текущей папке
settings-application-remain-in-tray = Оставаться в трее
    .description = Не останавливать музыку, когда закрыто последнее окно; вернуться можно через значок в трее, на macOS через док
settings-application-section-ai = ИИ
settings-application-section-control-socket = Управляющий сокет
settings-application-section-data = Данные
settings-application-section-layout = Макет
settings-application-section-startup = Запуск
settings-application-section-window = Окно
settings-application-socket-path = Путь к сокету
    .description = Машинный интерфейс rox во время работы: JSON-RPC через локальный сокет, привязанный к этой папке данных. Прокси rox-mcp обслуживает через него MCP-клиентов

## Settings: audio
settings-audio-broadcast-bitrate = Битрейт
    .description = Сколько кодировщик MP3 тратит на секунду потока
settings-audio-broadcast-enable = Вещать на Icecast
    .description = Отдавать то, что играет rox, на сервер icecast в роли источника, с кодированием в MP3. Точка монтирования, слушатели и вся сетевая сторона принадлежат icecast; rox только подключается наружу, и недоступный сервер никогда не трогает локальное воспроизведение
settings-audio-broadcast-host-placeholder = хост icecast
settings-audio-broadcast-login = Логин источника
    .description = Учётные данные источника icecast, пользователь и пароль из его конфигурации
settings-audio-broadcast-mount = Точка монтирования
    .description = Точка монтирования, на которую настраиваются слушатели, и имя потока, которое она объявляет
settings-audio-broadcast-name-placeholder = Название потока
settings-audio-broadcast-password-placeholder = Пароль источника
settings-audio-broadcast-server = Сервер
    .description = Хост и порт сервера icecast; протокол источника идёт по обычному сокету
settings-audio-broadcast-user-placeholder = source
settings-audio-crossfade = Кроссфейд
    .description = Как долго трек перекрывается со следующим. Затухание нужно для перемешивания и перескоков, поэтому собственные стыки альбома остаются нетронутыми, если строка ниже не говорит иначе. Ноль выключает его
    .keywords = без пауз перекрытие переход затухание
settings-audio-equalizer-note = Десять октавных полос на выходе. Он открывается в отдельном окне, потому что его крутят по ходу музыки, а не настраивают один раз
settings-audio-exclusive-mode = Монопольный режим
    .description = Забрать устройство только для rox и работать на собственной частоте файла там, где железо это принимает; если выключить, звук пойдёт через системный микшер вместе со всем остальным на рабочем столе
settings-audio-fade-inside-albums = Затухание внутри альбомов
    .description = Перекрывать и треки, которые принадлежат одной записи. Если выключить, собственные склейки записи останутся ровно такими, какими их свели, а это как раз то место, где отсутствие пауз важнее всего
settings-audio-open-equalizer = Открыть эквалайзер
settings-audio-output-buffer = Буфер
    .description = Сколько звука карта держит за раз. Короче реагирует быстрее и раньше начинает трещать на загруженной машине; длиннее надёжнее и ленивее
settings-audio-output-buffer-default = По умолчанию (10 мс)
settings-audio-output-device = Устройство
    .description-default = Системное по умолчанию следует за тем, что выбрано в системе
    .description-linux = Монопольный режим забирает карту прямо у ядра, поэтому в списке звуковые карты, а не выходы рабочего стола. У Bluetooth и других устройств звукового сервера карты нет, и они видны только при выключенном монопольном режиме
    .description-other = Монопольный режим забирает устройство только для rox, поэтому ничто другое на рабочем столе не сможет через него звучать, пока режим включён
settings-audio-output-device-system-default = Системное по умолчанию
settings-audio-output-experimental-badge = Экспериментально
settings-audio-output-experimental-tooltip = Монопольный бэкенд для этой платформы написан по её документированному звуковому контракту, но разработчики ни разу не запускали его на реальном железе. Он должен либо забрать устройство, либо откатиться к общему режиму с объяснением причины, но не замолчать. Если он ведёт себя плохо, выключите его и опишите, что случилось, кнопкой рядом с этой пометкой.
settings-audio-output-format = Формат
    .description = Что rox отдаёт карте. Карта, которая не принимает выбранное, работает в самом широком доступном формате, и статус ниже показывает, в каком именно
settings-audio-output-format-f32 = 32 бита, с плавающей точкой
settings-audio-output-format-s16 = 16 бит, целые
settings-audio-output-format-s32 = 32 бита, целые
settings-audio-output-format-widest = Самый широкий доступный
settings-audio-output-issue-tooltip = Сообщить, как монопольный режим повёл себя на этой машине. Откроет issue на GitHub с уже заполненной платформой и согласованным потоком.
settings-audio-output-mode-exclusive = Монопольный
settings-audio-output-mode-shared = Общий
settings-audio-output-not-built = Для этой платформы пока не собрано
settings-audio-output-rate-follow = Следовать за файлом
settings-audio-output-sample-rate = Частота дискретизации
    .description = Следование заново открывает устройство на собственной частоте каждого файла, а это стоит паузы на стыке, где частота меняется; фиксированная частота никогда не платит эту цену и пересчитывает всё, что не совпало
settings-audio-output-status-error-hint = Выберите другое устройство или выключите монопольный режим
settings-audio-output-status-error-title = Нет вывода
settings-audio-output-status-idle-hint = Запустите трек, чтобы увидеть формат, на который согласилось устройство
settings-audio-output-status-idle-title = Ничего не играет
settings-audio-replaygain-level-by = Выравнивать по
    .description = Играть каждый трек на той громкости, которую измерили его теги ReplayGain, чтобы перемешивание перестало прыгать между мастерингами. Трек измеряет каждый файл сам по себе; Альбом берёт усиление записи по всем её трекам, и это оставляет собственные тихие и громкие места альбома там, где их поставили
    .keywords = нормализация громкость выравнивание уровень
settings-audio-replaygain-measure-missing-button = Измерить недостающее
settings-audio-replaygain-measure-new = Измерять новые файлы
    .description = Измерять то, что приносит наблюдатель за папками, по мере поступления, когда синхронизация улеглась, чтобы растущая медиатека сохраняла свои усиления без возврата на эту страницу. Числа сохраняются туда, куда указывает «Куда сохранять измеренное». При включении сначала будет предложено измерить то, чего уже не хватает; дальше обрабатываются только новые файлы
settings-audio-replaygain-measuring-progress = Измерено { $done } из { $total }
settings-audio-replaygain-measuring-start = Измерение: разбираемся, чего не хватает...
settings-audio-replaygain-mode-album = Альбом
settings-audio-replaygain-mode-off = Выкл
settings-audio-replaygain-mode-track = Трек
settings-audio-replaygain-preamp = Предусиление
    .description = Прибавляется к каждому усилению из тегов. Опорный уровень ReplayGain лежит ниже того, на котором сводят современные записи, поэтому выровненная медиатека играет тише той же медиатеки без обработки; здесь это возвращается. Прибавка никогда не клиппует: её ограничивает пиковое значение из тега
settings-audio-replaygain-save = Куда сохранять измеренное
    .description = Куда проход измерения записывает свои числа. База медиатеки оставляет ваши файлы нетронутыми; теги кладут те же значения туда, где их читает любой другой плеер, ценой перезаписи звуковых файлов
settings-audio-replaygain-status-measured = Усиление для выравнивания есть у всех просканированных треков ({ $total }), из них { $measured } измерил rox
settings-audio-replaygain-status-tagged = Теги ReplayGain есть у всех просканированных треков ({ $total })
settings-audio-replaygain-untagged = Файлы без тегов
    .description = На какой громкости играет файл без тегов ReplayGain. Его никто не мерил, так что это догадка вместо измерения; оставьте ноль, и треки без тегов будут играть как раньше
settings-audio-section-broadcast = Вещание
settings-audio-section-equalizer = Эквалайзер
settings-audio-section-output = Вывод
settings-audio-section-playback = Воспроизведение
settings-audio-section-replaygain = ReplayGain
settings-audio-transport = Транспорт
    .description = Запуск и остановка не выходя с этой страницы, ведь каждую настройку ниже судят на слух

## Settings: integrations
settings-integrations-discord-enable = Включить Rich Presence
    .description = Показывать активность rox в Discord во время воспроизведения
settings-integrations-discord-show-lastfm = Показывать кнопку Last.fm
    .description = Добавить в статус Discord кликабельную кнопку «Открыть на Last.fm»
settings-integrations-discord-show-youtube = Показывать кнопку YouTube
    .description = Добавить в статус Discord кликабельную кнопку «Искать на YouTube»
settings-integrations-ffmpeg-binary = Исполняемый файл FFmpeg
    .description = Какой ffmpeg выполняет преобразования; оставьте пустым, чтобы взять тот, что в PATH
settings-integrations-ffmpeg-fail-note = Преобразование останется скрытым, пока путь к ffmpeg не укажет на рабочий файл
settings-integrations-ffmpeg-fail-title = Этот ffmpeg не запустился
settings-integrations-ffmpeg-missing-note = Преобразование останется скрытым; установите ffmpeg или укажите путь к исполняемому файлу
settings-integrations-ffmpeg-missing-title = Рабочий ffmpeg не найден
settings-integrations-ffmpeg-ok-note = ffmpeg работает. Преобразование доступно.
settings-integrations-ffmpeg-test = Проверить
settings-integrations-lastfm-api-key-row = API-ключ
settings-integrations-lastfm-connect = Подключить
settings-integrations-lastfm-disconnect = Отключить
settings-integrations-lastfm-finish-connecting = Завершить подключение
settings-integrations-lastfm-hearts = { $n ->
    [one] { $n } сердечко
    [few] { $n } сердечка
    [many] { $n } сердечек
   *[other] { $n } сердечка
}
settings-integrations-lastfm-import-loved = Импортировать любимые треки
settings-integrations-lastfm-intro-builtin = Подключите свой аккаунт Last.fm: разрешите доступ rox в браузере, и прослушанные треки пойдут в скробблинг
settings-integrations-lastfm-intro-custom = В этой сборке нет api-идентичности, поэтому для скробблинга нужен свой api-аккаунт (Last.fm/api/account/create); вставьте его ключ и общий секрет, затем подключитесь
settings-integrations-lastfm-key-placeholder = API-ключ
settings-integrations-lastfm-love-failed = Последняя попытка не удалась: { $error }
settings-integrations-lastfm-love-pending = { $hearts } в очереди на отправку
settings-integrations-lastfm-love-pending-failed = { $hearts } в очереди на отправку, последняя попытка: { $error }
settings-integrations-lastfm-reconnect = Переподключить
settings-integrations-lastfm-secret-placeholder = Общий секрет
settings-integrations-lastfm-secret-row = Общий секрет
settings-integrations-lastfm-status-confirming = Подтверждение...
settings-integrations-lastfm-status-connected = Подключено как { $username }
settings-integrations-lastfm-status-elsewhere = Подключено на другой установке rox; каждая авторизуется под своей api-идентичностью, так что подключите и эту
settings-integrations-lastfm-status-failed = Подключение не удалось: { $error }
settings-integrations-lastfm-status-not-connected = Не подключено
settings-integrations-lastfm-status-rejected = Last.fm отклонил сессию, и она была сброшена. Подключитесь заново, чтобы скробблинг продолжился
settings-integrations-lastfm-status-requesting = Запрос токена...
settings-integrations-lastfm-status-waiting = Разрешите доступ rox в браузере, затем завершите подключение
settings-integrations-lastfm-working = Выполняется...
settings-integrations-love-favourites = Отправлять избранное как любимое
    .description = Отражать сердечки на Last.fm как любимые треки; снятое сердечко снимает отметку и там
settings-integrations-scrobble-threshold = Порог скробблинга
    .description = Какую часть трека надо проиграть, прежде чем он уйдёт в скробблинг; полоса перемотки и волновая форма умеют это отмечать
settings-integrations-scrobble-tracks = Скробблить треки
    .description = Отправлять прослушанные треки на Last.fm, как только они перешли порог
settings-integrations-section-conversion = Преобразование
settings-integrations-section-discord = Discord Rich Presence
settings-integrations-section-favourites = Избранное
settings-integrations-section-lastfm = Last.fm
settings-integrations-section-scrobbling = Скробблинг

## Settings: keymap
settings-keymap-clash = { $chord } — это ещё и { $other }; сработает только одно из двух
settings-keymap-not-bound = Не назначено
settings-keymap-recording = Нажмите клавиши
settings-keymap-restore = Восстановить
settings-keymap-restore-all = Восстановить все сочетания
    .description = Вернуть каждую команду на её штатные клавиши, включая те, для которых в этой сборке уже нет строки
settings-keymap-section-defaults = По умолчанию
settings-keymap-undo = Отменить
settings-keymap-undo-last = Отменить последний сброс
    .description = Вернуть сочетания, выброшенные последним сбросом, будь то одна строка или все

## Settings: library
settings-library-acoustic-all-described = { $label } описывает все просканированные треки ({ $total })
settings-library-acoustic-auto = Описывать новые файлы
    .description = Описывать то, что приносит наблюдатель за папками, по мере поступления, когда синхронизация улеглась, чтобы растущая медиатека сохраняла свои описания без возврата на эту страницу. Если выключить, новые файлы будут ждать кнопки «Проанализировать недостающее». При включении сначала будет предложено проанализировать то, чего уже не хватает; дальше обрабатываются только новые файлы
settings-library-acoustic-enable = Описывать, как звучат треки
    .description = Разбирать, как звучит каждый трек, чтобы медиатека умела находить музыку, похожую на играющую. Всё считается на этой машине, и описание большой медиатеки занимает время
    .keywords = похожее звучание отпечаток описание
settings-library-acoustic-extractor = Экстрактор
settings-library-acoustic-extractor-model = Модель
settings-library-acoustic-fallback = Анализ
settings-library-acoustic-partial = { $label } описывает { $done } из { $total } просканированных треков. «Проанализировать недостающее» разберёт остальные
settings-library-acoustic-progress = { $running }: { $done } из { $total }
settings-library-acoustic-progress-start = { $running }: разбираемся, чего не хватает...
settings-library-acoustic-save = Куда сохранять описания
    .description = Куда проход записывает то, что разобрал. Одна база оставляет ваши файлы нетронутыми; теги кладут копию ещё и в каждый файл, так что описания сохранятся при пересборке базы или переезде папки на другую машину, ценой перезаписи звуковых файлов. Теги работают только для MP3 и FLAC; у остальных форматов остаётся копия в базе
settings-library-add-folder = Добавить папку
settings-library-duplicates = Дубликаты...
settings-library-embed-button = Встроить сохранённые метаданные...
settings-library-folder-col-albums = Альбомы
settings-library-folder-col-folder = Папка
settings-library-folder-col-size = Размер
settings-library-folder-col-tracks = Треки
settings-library-folders-intro = Папки, просканированные в медиатеку; удаление папки убирает её треки из каталога и не трогает файлы
settings-library-genre-separator-nudge = Разделители изменились: просмотр подхватит это сразу. Списки жанров, сохранённые прошлыми сканированиями, останутся в прежнем виде, пока вы не нажмёте «Пересканировать» в шапке раздела «Папки»
settings-library-merge-case = Объединять варианты регистра
    .description = Считать значения, различающиеся только регистром, одним и тем же: Rock и rock становятся одним жанром, исполнителем и альбомом и показываются в том написании, которое встречается у большинства треков. В файлах теги остаются как есть
settings-library-no-folders = Пока нет папок
settings-library-repair-tags = Починить теги...
settings-library-section-folders = Папки
settings-library-section-stored-metadata = Сохранённые метаданные
settings-library-section-tempo = Анализ темпа
settings-library-split-genres = Делить жанры по запятым и слэшам
    .description = «Dubstep, Trap» и «Drum & Bass / Neurofunk» дают каждому значению отдельный жанр; точка с запятой делит всегда. Если выключить, имена со слэшем останутся целыми там, где они означают один жанр. В файлах теги остаются как есть
settings-library-tempo-auto = Считать темп новых файлов
    .description = Считать удары в том, что приносит наблюдатель за папками, по мере поступления, когда синхронизация улеглась, чтобы растущая медиатека сохраняла свои темпы без возврата на эту страницу. Если выключить, новые файлы будут ждать кнопки «Проанализировать недостающее». При включении сначала будет предложено посчитать то, чего уже не хватает; дальше обрабатываются только новые файлы
settings-library-tempo-enable = Определять темп треков
    .description = Считать удары в треках, у которых нет этого в тегах, чтобы медиатека умела показывать темп и сортировать по нему. Всё считается на этой машине, числа идут в базу медиатеки, а ваши файлы остаются нетронутыми
settings-library-tempo-progress = Определение темпа: { $done } из { $total }
settings-library-tempo-progress-start = Разбираемся, чего не хватает...
settings-library-tempo-status-measured = Темп есть у всех просканированных треков ({ $total }), из них { $measured } определил rox
settings-library-tempo-status-tagged = Тег темпа есть у всех просканированных треков ({ $total })
settings-library-watch-folders = Следить за папками
    .description = Подхватывать добавленные, изменённые и удалённые файлы в медиатеку по мере событий, без ручного пересканирования
settings-library-write-stored = Записать сохранённое в сами файлы
    .description = Три настройки сохранения действуют только на следующую запись, так что всё, сохранённое до переключения на «Теги», лежит только в rox. Здесь тексты, усиления и описания, которые rox уже хранит, записываются в сами файлы, чтобы их увидел другой плеер, читающий эту папку. Ничего не пересчитывается

## Settings: MCP
settings-mcp-client-config = Конфигурация клиента
    .description = Вставьте в список серверов MCP-клиента (Claude Code, Claude Desktop или любого другого), чтобы он мог спрашивать rox о медиатеке, о том, что играет, и о транспорте. rox должен быть запущен; инструменты работают через его управляющий сокет
settings-mcp-enable = Включить MCP-сервер
    .description = Отвечать на вызовы инструментов от подключённых MCP-клиентов. Прокси проверяет это на каждом вызове, так что пока выключено, клиенты получают отказ с причиной; конфигурацию ниже можно подготовить в любом случае

## Settings: ML models
settings-mlmodels-checking = Проверка...
settings-mlmodels-choose-file = Выбрать файл
settings-mlmodels-custom-description-empty = Укажите rox свой чекпоинт PANNs CNN10 в формате safetensors. Он читается на месте и именуется по своему хешу, так что второй чекпоинт описывает медиатеку отдельно, а не переиспользует координаты первого
settings-mlmodels-download-failed = Не удалось скачать { $label }: { $reason }
settings-mlmodels-downloading = Скачивание { $label }: { $done } из { $total }
settings-mlmodels-stopping = Остановка скачивания { $label }...
settings-mlmodels-fallback-model = модель
settings-mlmodels-fallback-the-model = Модель
settings-mlmodels-kind-custom = Своя
settings-mlmodels-kind-recommended = Рекомендуемая
settings-mlmodels-pass-stopped = Последний проход остановлен: { $reason }
settings-mlmodels-weights-file = Файл весов

## Settings: playback
settings-playback-continuation-continue = Продолжать
    .description = Идти дальше по списку, с которого вы начали, а затем по остальной медиатеке за ним. Запустите альбом из середины представления, и представление продолжится
settings-playback-continuation-off = Выкл
    .description = Очередь ничем не пополняется; воспроизведение останавливается в её конце
settings-playback-continuation-weighted = С весами
    .description = Тянуть из всей медиатеки: сначала то, что вы никогда не слушали, в конце то, что слушали недавно
settings-playback-keep-playing = Продолжать играть
    .description = Что играет, когда очередь кончилась. Всё, что выбрано здесь, дописывается на ленту как обычный контекст, то есть видно и удаляется, а не живёт скрытым состоянием. Если порядок выше выставлен на Похожие, rox продолжит искать треки, звучащие как играющий, при любом из этих вариантов
    .keywords = продолжение пополнение автовоспроизведение очередь
settings-playback-play-order = Порядок воспроизведения
    .description = Как расставлены уже стоящие в очереди треки, пока включено перемешивание. Кнопка перемешивания в транспорте включает и выключает его; здесь настраивается, что оно делает
settings-playback-rating-scale = Шкала оценок
    .description = Звёзды для быстрых кликов, 0-10 с половинными шагами для более точных рецензий
settings-playback-rating-scale-numeric = 0-10
settings-playback-rating-scale-stars = Звёзды
settings-playback-restore-last-session = Восстанавливать прошлый сеанс
    .description = Запускаться с той очередью, которую вы оставили, на паузе на игравшем треке и на том месте, где он остановился. Треки из очереди вне папок медиатеки восстановить нельзя, и они выпадают из порядка
settings-playback-section-queue = Очередь
settings-playback-section-ratings = Оценки
settings-playback-section-startup = Запуск
settings-playback-shuffle-random = Случайный
    .description = То самое перемешивание, которое все и имеют в виду. Дальше играет без всякого порядка
settings-playback-shuffle-similar = Похожие
    .description = Сначала ближайшее по звучанию. Дальнейшее сортируется по тому, насколько оно похоже на трек, который играл, когда вы это включили, и пересортируется на каждом перескоке. Нужна медиатека, описанная на странице «Медиатека»
settings-playback-unrated-dots = Точки без оценки
    .description = Отмечать незаполненные звёзды бледной точкой, а не оставлять их пустыми

## Settings: providers
settings-providers-artist = Last.fm
    .description = Получать биографии исполнителей, статистику и похожих исполнителей для панели биографии, с портретом из Deezer; всё складывается в папку данных и дальше читается офлайн
settings-providers-deezer = Deezer
    .description = Искать обложки в Deezer, до 1000 пикселей
settings-providers-itunes = iTunes
    .description = Искать обложки в iTunes; поиск в редакторе обложек показывает найденное, чтобы выбрать до установки
settings-providers-lastfm-art = Last.fm
    .description = Искать обложки на Last.fm
settings-providers-lrclib = LRCLIB
    .description = Получать недостающие тексты с lrclib.net, с синхронизацией, когда она там есть
settings-providers-lyrics-intro = Онлайн-запросы уходят только тогда, когда их просит действие в панели; воспроизведение и просмотр никогда не трогают сеть
settings-providers-musicbrainz = MusicBrainz
    .description = Искать теги на musicbrainz.org; поиск в панели метаданных показывает найденное, чтобы подтвердить поле за полем перед записью
settings-providers-save-lyrics = Куда сохранять найденные тексты
    .description = Куда сохраняется найденный текст: в собственную папку данных rox, оставляя медиатеку чистой, в файл .lrc рядом с треком или во встроенный тег
settings-providers-save-lyrics-data-folder = Папка данных
settings-providers-save-lyrics-sidecar = Файл рядом
settings-providers-save-lyrics-tag = Тег
settings-providers-section-artist = Исполнитель
settings-providers-section-cover-art = Обложки
settings-providers-section-lyrics = Тексты песен
settings-providers-section-metadata = Метаданные

## Settings: shader
settings-shader-backdrop-all-windows = Все окна
    .description = Затенять подложку каждого окна: настройки, редакторы, диалоги, отделённые панели. Если выключить, это останется у окон рабочего пространства
settings-shader-backdrop-enabled = Шейдер подложки
    .description = Выполнять реагирующий на музыку WGSL-шейдер поверх подложки из обложки, под всеми панелями. Часть рабочего пространства, так что путешествует вместе с внешним видом
settings-shader-backdrop-fallback-name = Подложка
settings-shader-backdrop-run-idle = Работать в простое
    .description = Продолжать рисовать, когда ничего не играет. Анимация в любом случае остаётся замороженной
settings-shader-compile-error-title = Этот шейдер не скомпилировался
settings-shader-legacy-note = Если ничего не назначено, пул заполняет слоты в собственном порядке: первый сигнал в слот 0, второй в слот 1 и так далее. Первый же добавленный маршрут берёт на себя всё распределение.
settings-shader-overlay-enabled = Шейдер наложения
    .description = Выполнять реагирующий на музыку WGSL-шейдер поверх всего окна. Предлагаются только шейдеры, которые оставляют приложение пригодным для работы
settings-shader-scene-covers-window = Этот шейдер является сценой, поэтому он закрывает окно, а не рисует поверх него. Он пришёл из набора или из старой конфигурации; в списке выше только те шейдеры, которые оставляют приложение пригодным для работы.
settings-shader-screen-all-windows = Все окна
    .description = Затенять и дочерние окна: настройки, статистику, эквалайзер, отделённые панели. Обратный отсчёт до отката в любом случае остаётся незатенённым
settings-shader-screen-fallback-name = Экран
settings-shader-screen-run-idle = Работать в простое
    .description = Продолжать рисовать, когда ничего не играет. Анимация в любом случае остаётся замороженной. Шейдер, который читает мышь, следует за курсором и с остановленной музыкой без этой настройки; он просто замирает через пару секунд после указателя
settings-shader-section-backdrop = Шейдер подложки
settings-shader-section-overlay = Шейдер наложения
settings-shader-signals-block = Сигналы
    .description = Какой общий сигнал читает каждый из шестнадцати слотов шейдера
settings-shader-slots-block = Слоты
    .description = Каждый слот в том виде, в каком его получает шейдер; слоты без маршрута — это регуляторы, выставленные вручную

## Settings: storage
settings-storage-artist-images = Изображения исполнителей
    .description = Портреты, баннеры и биографии, скачанные для представлений исполнителей (artists/); очищенное скачивается заново при следующем открытии представления
settings-storage-catalog = Каталог
    .description = Индекс треков, который строит сканирование: строка на трек с его тегами, деталями файла и любыми участками из cue, внутри library.db
settings-storage-cover-thumbnails = Миниатюры обложек
    .description = Маленькие обложки, сохранённые после первой отрисовки (thumbs.db); очищенные пересобираются, когда попадают в поле зрения
settings-storage-logs = Журналы
    .description = Что каждый запуск пишет для отчётов об ошибках (logs/rox.log), с ротацией по размеру, так что журнал не разрастается
settings-storage-looks-layouts = Внешний вид и макеты
    .description = Внешний вид, который сейчас использует приложение (workspace.json), рядом с сохранёнными рабочими пространствами, выгруженными файлами шейдеров и наборами значков. Места занимает мало, и каждый байт здесь настроен вами
settings-storage-lyrics = Тексты песен
    .description = Скачанные и отредактированные тексты в собственном хранилище приложения (lyrics/), чтобы папки медиатеки оставались чистыми
settings-storage-measured-tempos = Измеренные темпы
    .description = Темпы, которые rox насчитал из звука для треков, у которых их нет в тегах; собственные числа тегов не трогаются. Очистка вернёт эти треки в список «Проанализировать недостающее» на странице «Медиатека», так что улучшенный подсчёт ударов сможет заменить числа, записанные прошлым проходом
settings-storage-model-fallback-this = Эта модель
settings-storage-music-summary = { $tracks }, { $albums }, { $size }
settings-storage-model-weights = Веса моделей
    .description = Модели, скачанные для акустического анализа (models/). Их скачивают и удаляют на странице «ML-модели», по строке на модель
settings-storage-models-empty = Модели
    .description = Медиатеку ещё никто не описывал. Раздел заполнится, когда вы включите акустический анализ на странице «Медиатека»; каждая отработавшая модель получит здесь свою строку
settings-storage-music-files = Музыкальные файлы
    .description = Всё, что лежит в просканированных папках; файлы остаются на своих местах
settings-storage-none = Нет
settings-storage-playlists-history = Плейлисты и история
    .description = Ваши плейлисты и их состав, что вы слушали, и жанровые пометки медиатеки. На фоне остального library.db всё это мало весит
settings-storage-reclaimable = Освобождаемое место
    .description = Страницы внутри library.db, оставшиеся после удалений. Новые записи заполняют их снова, поэтому файл перестаёт расти раньше, чем начинает уменьшаться
    .keywords = vacuum сжатие уменьшение база данных
settings-storage-section-acoustic = Акустические описания
settings-storage-section-app-data = Данные приложения
settings-storage-section-caches = Кеши
settings-storage-section-diagnostics = Диагностика
settings-storage-section-library = Медиатека
settings-storage-section-tempo = Темп
settings-storage-vectors = Векторы
    .description = Сколько весит каждое описание внутри library.db. В медиатеке, через которую прошёл проход анализа, это большая часть файла: пара килобайт на трек против пары сотен байт тегов
settings-storage-waveforms = Волновые формы
    .description = Полоса пиков каждого трека, сохранённая после первого воспроизведения; очищенные декодируются заново при следующем

## Settings: workspace
settings-workspace-card-author = Автор
settings-workspace-card-author-placeholder = Кто это сделал
settings-workspace-card-created = Создано { $date }
settings-workspace-card-created-updated = Создано { $created }, обновлено { $updated }
settings-workspace-card-description = Описание
settings-workspace-card-description-placeholder = К чему стремится этот внешний вид
settings-workspace-card-empty = У этого рабочего пространства нет карточки
settings-workspace-card-hint = Карточка хранится в файле, так что её увидит тот, кому вы отдадите этот внешний вид
settings-workspace-card-license = Лицензия
settings-workspace-card-license-placeholder = Условия, на которых вы им делитесь
settings-workspace-card-save = Сохранить карточку
settings-workspace-card-updated = Обновлено { $date }
settings-workspace-card-version = Версия
settings-workspace-card-version-placeholder = Ваша собственная версия, в чём бы вы её ни считали
settings-workspace-card-website = Сайт
settings-workspace-card-website-placeholder = Где это живёт
settings-workspace-composition-closed = Окно рабочего пространства закрыто
settings-workspace-composition-hint = Панели окна так, как они расставлены по разделениям и группам вкладок; стрелки меняют порядок строки среди соседей, замок закрепляет панель на месте, а шестерёнка открывает её настройки
settings-workspace-empty = Пока нет рабочих пространств
settings-workspace-hint = Рабочее пространство — это целый внешний вид: макеты, палитра, оформление; применение заменяет все три
settings-workspace-layout-name-placeholder = Название макета
settings-workspace-layouts-empty = Пока нет макетов
settings-workspace-layouts-hint = Основной и мини — это те два, между которыми переключает кнопка мини-плеера в строке меню
settings-workspace-name-placeholder = Название рабочего пространства
settings-workspace-panel-preset-unknown-kind = Неизвестная панель
settings-workspace-panel-presets-empty = Пока нет пресетов панелей
settings-workspace-panel-presets-hint-after = в меню любой панели. Они принадлежат только этому рабочему пространству; в другом их не будет.
settings-workspace-panel-presets-hint-before = По одной настроенной панели в каждом, сохраняются из собственного меню панели и возвращаются через
settings-workspace-role-mini = Мини
settings-workspace-role-primary = Основной
settings-workspace-section-composition = Композиция
settings-workspace-section-layouts = Макеты
settings-workspace-section-panel-presets = Пресеты панелей
settings-workspace-section-workspaces = Рабочие пространства
settings-workspace-tree-empty-slot = Пустой слот
settings-workspace-tree-split-column = Разделение, друг над другом
settings-workspace-tree-split-row = Разделение, бок о бок
settings-workspace-tree-tabs = Вкладки

## Settings: development
settings-development-experimental-panels = Экспериментальные панели
    .description = Показывать ещё строящиеся панели в меню «Панели» и в лаунчере; они меняют форму от выпуска к выпуску, а макет, в котором такая панель уже есть, сохранит её после выключения этой настройки
settings-development-section-features = Возможности

## Settings: shared
settings-acoustic-analysis-heading = Акустический анализ
settings-analyze-nothing-scanned = Пока нечего анализировать, ничего не просканировано
settings-common-active = Активно
settings-common-analyze-missing = Проанализировать недостающее
settings-common-built-in = Встроенное
settings-common-clear = Очистить
settings-common-copy = Копировать
settings-common-database = База данных
settings-common-delete = Удалить
settings-common-download = Скачать
settings-common-rescan = Пересканировать
settings-common-reveal = Показать
settings-common-stop = Остановить
settings-common-stopping = Остановка...
settings-common-tags = Теги
settings-common-tracks-count = треков: { $count }
settings-common-use = Использовать
settings-confirm-apply-body = Это заменит ваши макеты, палитру и оформление на те, что в рабочем пространстве.
settings-confirm-apply-imported-body = Оно сохранено в ваши рабочие пространства. Применение прямо сейчас заменит ваши макеты, палитру и оформление на те, что в нём.
settings-confirm-clear = Очистить
settings-confirm-clear-embeddings-body = Описания уйдут, место вернётся. Чтобы получить их снова, придётся заново прогнать проход анализа по всем трекам медиатеки.
settings-confirm-clear-embeddings-title = Очистить то, что описала «{ $model }»?
settings-confirm-clear-measured-bpm-body = Каждый темп, который посчитал rox, вернётся в состояние «не измерено»; числа из тегов ваших файлов останутся. Чтобы получить их снова, придётся заново прогнать проход темпа по всем этим трекам.
settings-confirm-clear-measured-bpm-title = Очистить измеренные темпы?
settings-confirm-overwrite-workspace-body = Это заменит сохранённое рабочее пространство текущим состоянием.
settings-confirm-overwrite-workspace-title = Перезаписать рабочее пространство «{ $name }»?
settings-sidebar-data-folder = Папка данных
settings-sidebar-settings-file = Файл настроек

## Menubar
menu-about = О программе
menu-application = Приложение
menu-apply-layout = Применить макет
menu-apply-workspace = Применить рабочее пространство
menu-chat = Чат
menu-close = Закрыть
menu-console = Консоль
menu-design-mode = Режим дизайна
menu-discussions = Обсуждения
menu-empty-window = Пустое окно
menu-equalizer = Эквалайзер
menu-exit = Выход
menu-hide-menubar = Скрыть строку меню
menu-import-workspace = Импортировать рабочее пространство...
menu-new-ellipsis = Создать...
menu-new-window = Новое окно
menu-new-window-from-layout = Новое окно из макета
menu-new-window-from-panel = Новое окно из панели
menu-no-layouts = Нет макетов
menu-no-presets = Нет пресетов
menu-no-workspaces = Нет рабочих пространств
menu-os-decorations = Оформление окон ОС
menu-overlay-shader = Шейдер наложения
menu-panel-built-in = Встроенное
menu-panel-new = Создать...
menu-panel-no-layouts = Нет макетов
menu-panel-no-presets = Нет пресетов
menu-panel-no-workspaces = Нет рабочих пространств
menu-panel-title = Меню
menu-panels = Панели
menu-panels-presets = Пресеты
menu-pause = Пауза
menu-playback = Воспроизведение
menu-remain-in-tray = Оставаться в трее
menu-report-issue = Сообщить о проблеме
menu-save-layout = Сохранить макет
menu-save-workspace = Сохранить рабочее пространство
menu-section-add = Добавить
menu-section-app = Приложение
menu-section-interface = Интерфейс
menu-section-layouts = Макеты
menu-section-library = Медиатека
menu-section-session = Сеанс
menu-section-track = Трек
menu-section-tuning = Настройка
menu-settings = Настройки
menu-signals = Сигналы
menu-song-theming = Окраска по треку
menu-stats = Статистика
menu-tasks = Задачи
menu-update-available = Доступно обновление
menu-welcome = Приветствие
menu-window = Окно
menu-workspace = Рабочее пространство
menu-workspace-builtin-tag = Встроенное

## Workspaces
workspace-apply-body = Это заменит весь внешний вид: макеты, палитру, оформление.
workspace-apply-imported-body = Оно сохранено в ваши рабочие пространства. Применение прямо сейчас заменит весь внешний вид: макеты, палитру, оформление.
workspace-apply-imported-title = Импортировано «{ $name }»
workspace-apply-screen-shader-named = Накладывает шейдер { $name } поверх всего окна.
workspace-apply-screen-shader-plain = Накладывает шейдер наложения поверх всего окна.
workspace-apply-shader-count = { $count ->
    [one] Включает { $count } шейдер: { $names }
    [few] Включает { $count } шейдера: { $names }
    [many] Включает { $count } шейдеров: { $names }
   *[other] Включает { $count } шейдера: { $names }
}
workspace-apply-shaders-approve-body = Подтверждение позволит им работать на этой машине. Если применить без них, внешний вид останется голым, а шейдеры останутся в его пуле.
workspace-apply-shaders-plain-body = Если применить без них, внешний вид останется голым, а шейдеры останутся в его пуле.
workspace-byline-author = автор { $author }
workspace-byline-version = версия { $version }
workspace-context-add-panel = Добавить панель
workspace-dialog-apply = Применить
workspace-dialog-apply-title = Применить «{ $name }»?
workspace-dialog-approve-apply = Подтвердить и применить
workspace-dialog-cancel = Отмена
workspace-dialog-close = Закрыть
workspace-dialog-close-title = Закрыть «{ $name }»?
workspace-dialog-export = Экспорт
workspace-dialog-layout-name-placeholder = Название макета
workspace-dialog-not-now = Не сейчас
workspace-dialog-overwrite = Перезаписать
workspace-dialog-overwrite-title = Перезаписать «{ $name }»?
workspace-dialog-save = Сохранить
workspace-dialog-save-layout-title = Сохранить макет
workspace-dialog-save-workspace-title = Сохранить рабочее пространство
workspace-dialog-with-shaders = С шейдерами
workspace-dialog-without-shaders = Без шейдеров
workspace-dialog-workspace-name-placeholder = Название рабочего пространства
workspace-drop-add-queue = Добавить в очередь
workspace-drop-play-now = Воспроизвести сейчас
workspace-hint-or = или
workspace-hint-then = затем
workspace-import = Импорт
workspace-launcher-hint = Добавьте первую панель, чтобы начать сборку, или выберите готовое в меню «Рабочее пространство > Применить рабочее пространство»
workspace-launcher-need-help = Нужна помощь?
workspace-launcher-open-welcome = Открыть окно приветствия
workspace-launcher-title = Пустое окно
workspace-layout-apply-body = Это заменит текущий макет этого окна.
workspace-layout-overwrite-body = Это заменит сохранённый макет текущим.
workspace-layout-preset-restore-failed = Пресет макета этого окна не удалось восстановить, поэтому оно открылось пустым.
workspace-layout-restore-failed = Сохранённый макет не удалось восстановить, поэтому это окно открылось пустым.
workspace-mini-tip-back = Назад к полному макету
workspace-mini-tip-shrink = Свернуть до мини-плеера
workspace-overwrite-body = Это заменит сохранённое рабочее пространство текущим внешним видом.
workspace-panel-locked-close-body = Эта панель закреплена на месте. Закрытие уберёт её из макета.
workspace-save-current = Сохранить текущее
workspace-screen-shader-hint-before = Выключить в любой момент можно через
workspace-workspace-restore-failed = Макет рабочего пространства не удалось восстановить, поэтому это окно открылось пустым.

## Tasks window
tasks-acoustic-all-described = { $label } описывает все просканированные треки ({ $count })
tasks-acoustic-off = Описание того, как звучат треки, выключено в настройках, в разделе «Медиатека»
tasks-acoustic-partial = { $label } описывает { $embedded } из { $total } просканированных треков
tasks-analyzing = Анализ { $progress }
tasks-bake-writing = Запись тегов...
tasks-chip-count = задач: { $count }
tasks-convert-starting = Запуск ffmpeg...
tasks-converting = Преобразование { $progress }
tasks-count-of-total = { $done } из { $total }
tasks-embedding = Встраивание { $progress }
tasks-estimate-at = { $estimate } в { $workers }
tasks-import-failed = Последний импорт не удался: { $error }
tasks-import-reading = Чтение списка любимого...
tasks-import-unmatched = Без совпадений в этой медиатеке: { $count }
tasks-importing = Импорт { $progress }
tasks-job-acoustic = Акустический анализ
tasks-job-convert = Преобразование звука
tasks-job-loved-import = Любимые треки Last.fm
tasks-job-replaygain = ReplayGain
tasks-job-scan = Сканирование медиатеки
tasks-job-tempo = Анализ темпа
tasks-last-pass-stopped = Последний проход остановлен: { $reason }
tasks-last-run-finished = Последний запуск завершён, сделано: { $count }
tasks-last-run-stopped = Последний запуск остановлен после { $count }
tasks-library-busy = Медиатека занята
tasks-library-scanning = Идёт сканирование медиатеки
tasks-measuring = Измерение { $progress }
tasks-model-downloading = Модель ещё скачивается
tasks-no-library-window = Ни одного окна медиатеки не открыто, поэтому отсюда это не запустить
tasks-nothing-to-measure = Пока нечего измерять, ничего не просканировано
tasks-rg-all-gain = Усиление для воспроизведения есть у всех треков ({ $count })
tasks-rg-partial = У { $missing } из { $total } треков нет усиления
tasks-scan-folder-count = { $count ->
    [one] { $count } папка
    [few] { $count } папки
    [many] { $count } папок
   *[other] { $count } папки
}
tasks-scan-last-scanned = { $folders }, последнее сканирование { $ago } назад
tasks-scan-never-scanned = { $folders }, ни разу не сканировалось
tasks-scan-no-folders = Пока не добавлено ни одной папки. Добавьте её в настройках, в разделе «Медиатека»
tasks-start-analyze-missing = Проанализировать недостающее
tasks-start-measure-missing = Измерить недостающее
tasks-start-rescan = Пересканировать
tasks-stop = Остановить
tasks-stopping = Остановка...
tasks-tempo-all = Темп есть у всех треков ({ $count })
tasks-tempo-off = Определение темпа треков выключено в настройках, в разделе «Медиатека»
tasks-tempo-partial = У { $missing } из { $total } треков нет темпа
tasks-timing = Определение темпа { $progress }
tasks-tip = Открыть задачи медиатеки
tasks-window-title = rox - Задачи
tasks-working-out-missing = Разбираемся, чего не хватает...

## Stats window
stats-bucket-listens = { $count ->
    [one] { $count } прослушивание, { $ago }
    [few] { $count } прослушивания, { $ago }
    [many] { $count } прослушиваний, { $ago }
   *[other] { $count } прослушивания, { $ago }
}
stats-chart-start-all = Первое прослушивание
stats-chart-start-month = 30 дней назад
stats-chart-start-week = 7 дней назад
stats-chart-start-year = Год назад
stats-click-opens = Клик открывает статистику
stats-click-section = Клик
stats-count-menu = Счётчик
    .description = За какое скользящее окно число считает прослушивания; во всплывающем списке всегда есть все окна
stats-empty-all = Пока нет прослушиваний
stats-empty-range = В этом диапазоне нет прослушиваний
stats-now = Сейчас
stats-open = Открыть статистику
stats-open-on-click = Открывать статистику по клику
    .description = Клик по виджету открывает окно статистики, полную запись прослушиваний
stats-play-these-tracks = Воспроизвести эти треки
stats-play-this-track = Воспроизвести этот трек
stats-plays-count = { $count ->
    [one] { $count } прослушивание
    [few] { $count } прослушивания
    [many] { $count } прослушиваний
   *[other] { $count } прослушивания
}
stats-range-all = За всё время
stats-range-all-short = Всё
stats-range-day-short = День
stats-range-label = Диапазон
stats-range-month = В этом месяце
stats-range-month-short = Месяц
stats-range-today = Сегодня
stats-range-week = На этой неделе
stats-range-week-short = Неделя
stats-range-year = В этом году
stats-range-year-short = Год
stats-readout-section = Показания
stats-section-listens = Прослушивания
stats-section-listens-over-time = Прослушивания по времени
stats-section-recent-listens = Недавние прослушивания
stats-section-top-albums = Топ альбомов
stats-section-top-artists = Топ исполнителей
stats-section-top-genres = Топ жанров
stats-show-change = Показывать изменение
    .description = Добавить плашку с тем, как окно смотрится против предыдущего, вверх или вниз; у «За всё время» позади ничего нет
stats-show-number = Показывать число
    .description = Рисовать счётчик рядом со значком; если выключить, останется голый значок, а числа появятся при наведении
stats-title = Виджет статистики
stats-tooltip-listens = Прослушивания
stats-window-title = rox - Статистика

## About window
about-check-failed = Не удалось связаться с GitHub
about-check-for-updates = Проверить обновления
about-checking = Проверка...
about-download = Скачать
about-downloading = Скачивание... { $percent }%
about-get-it = Получить
about-license-lead = rox — это свободное ПО под GNU AGPLv3. Исходный код лежит на
about-notice-lead = Копия лицензии должна была прийти вместе с программой. Если нет, смотрите
about-release-notes = Заметки о выпуске
about-restart-now = Перезапустить сейчас
about-up-to-date = У вас последняя версия
about-update-failed = Обновление не удалось: { $error }
about-version = Версия { $version }
about-version-available = Доступна версия { $version }
about-version-ready = Версия { $version } готова
about-window-title = rox - О программе

## Welcome window
welcome-add-folder = Добавить папку
welcome-and = и
welcome-back = Назад
welcome-card-menubar-title = Строка меню
welcome-card-music-title = Музыка
welcome-card-panels-title = Панели
welcome-card-playback-title = Воспроизведение
welcome-card-rearranging-title = Перестановка
welcome-card-settings-title = Настройки
welcome-close = Закрыть
welcome-design-mode-note = Для перестановки нужен режим дизайна, он включён по умолчанию наверху того меню. Если его выключить, макет запирается, и готовую сборку не сдвинуть случайно.
welcome-done = Готово
welcome-drop-note = Бросьте на край панели, чтобы разделить её там, на середину, чтобы попасть в общую группу вкладок, или за пределы окна, чтобы сделать отдельное окно.
welcome-key-left-click = Левый клик
welcome-key-middle-mouse = Средняя кнопка
welcome-layout-note = Сохраните расстановку как макет; рабочее пространство складывает макеты и палитру в один внешний вид, которым можно поделиться.
welcome-menubar-after = дважды, чтобы она осталась.
welcome-menubar-before = Когда строка меню скрыта, зажмите
welcome-menubar-mid = чтобы вывести её над доком, или нажмите
welcome-music-note = rox просканирует её в медиатеку, а файлы останутся на месте. Другие папки добавляются в настройках, в разделе медиатеки.
welcome-next = Далее
welcome-or = или
welcome-panels-note = Каждая поверхность — это панель, а меню «Панели» в строке меню открывает новые.
welcome-playback-after = перематывают.
welcome-playback-before = переключает воспроизведение;
welcome-quickplay-after = и он играет.
welcome-quickplay-before = открывает быстрый запуск: наберите трек, нажмите
welcome-rearrange-after = в любом месте панели, чтобы её передвинуть.
welcome-rearrange-before = Перетащите вкладку или зажмите
welcome-settings-hint-after = открывает настройки: палитру, прозрачность и поведение.
welcome-shelf-caption = Выбор одного заменит внешний вид основного окна и закроет тур. Это окно всегда доступно через «Приложение > Приветствие».
welcome-stage-lead-quick-start = Выберите рабочее пространство, и основное окно переключится на него: макеты, палитра, весь внешний вид.
welcome-stage-lead-welcome = Foobar, если бы его сделали в 20XX.
welcome-stage-title-quick-start = Быстрый старт
welcome-stage-title-welcome = Добро пожаловать в rox
welcome-step-hint-after = , или кнопками ниже.
welcome-step-hint-before = Шагайте по нему с помощью
welcome-tile-by = автор { $author }
welcome-tour-intro = Короткий тур: откуда приходит музыка и где настраивается внешний вид. Он заканчивается полкой готовых рабочих пространств, каждое в один клик.
welcome-window-title = rox - Приветствие

## Console window
console-clear = Очистить
console-copy = Копировать
console-empty-filtered = Ничего на этих уровнях
console-empty-none = Пока ничего не записано
console-filter-error = Ошибки
console-filter-info = Инфо
console-filter-warn = Предупреждения
console-follow = Следовать
console-line-count = { $count ->
    [one] { $count } строка
    [few] { $count } строки
    [many] { $count } строк
   *[other] { $count } строки
}
console-open-button = Открыть консоль
console-reveal = Показать
console-window-title = rox - Консоль

## Signals window
signals-about-toggle = О сигналах
signals-blurb-marked = У панелей, отмеченных этим значком в меню, можно привязать большинство параметров: щёлкните правой кнопкой по параметру в настройках панели и выберите сигнал или добавьте новый оттуда же.
signals-blurb-shared = Настроенное здесь общее: изменение применяется к каждому параметру, направленному на этот сигнал, в каждой панели и каждом окне.
signals-blurb-total = Сумма — это четвёртый вид: она накапливает другой сигнал со временем и сбрасывается по достижении 1, поэтому растёт, пока музыка громкая, и замирает, пока нет. Пригодится, когда шейдеру нужна фаза, которая движется с песней, а не с часами.
signals-blurb-what = Сигнал превращает то, что играет, в одно число от 0 до 1: энергия в полосе частот, уровень всей смеси или импульс на каждом ударе внутри полосы. Отклик задаёт, как быстро он следует, Порог глушит его ниже выбранного уровня.
signals-no-library = Ни одного окна медиатеки не открыто, поэтому звука здесь нет. Правки всё равно сохраняются.
signals-window-title = rox - Сигналы

## Equaliser
eq-analyzer-bars = Полосы
eq-analyzer-off = Без анализатора
eq-analyzer-wave = Волна
eq-band-badge = Значок полос
    .description = Показывать на значке поверх иконки, сколько полос сдвинуто с нуля
eq-band-label = Полоса { $number }
eq-click-nothing = Ничего
eq-click-open = Открыть
eq-click-section = Клик
    .description = Что делает клик: открывает окно эквалайзера или включает и выключает всю кривую прямо на месте
eq-click-toggle = Переключить
eq-flatten = Выровнять
eq-freq-label = Частота
eq-gain-label = Усиление
eq-heading = Эквалайзер
eq-help-text = Тяните полосу, чтобы её сдвинуть, крутите колесо над ней, чтобы расширить или сузить. Обработка идёт до буфера, из которого читает звуковая карта, поэтому движение доходит до колонок примерно за полсекунды.
eq-hint-off = Клик выключает
eq-hint-on = Клик включает
eq-hint-open = Клик открывает эквалайзер
eq-open = Открыть эквалайзер
eq-readout-curve = Кривая
eq-readout-icon = Значок
eq-readout-section = Показания
    .description = Значок, кривая отклика как спарклайн или и то и другое. Кривой нужно около пятидесяти пикселей ширины, чтобы её можно было прочесть
eq-reset-bands = Сбросить полосы
eq-shape-active = { $count ->
    [one] { $count } полоса сдвинута с нуля, пик { $peak } дБ
    [few] { $count } полосы сдвинуты с нуля, пик { $peak } дБ
    [many] { $count } полос сдвинуто с нуля, пик { $peak } дБ
   *[other] { $count } полосы сдвинуты с нуля, пик { $peak } дБ
}
eq-shape-flat = Ровно, все полосы на 0 дБ
eq-status-off = Эквалайзер выключен
eq-status-on = Эквалайзер включён
eq-title = Виджет эквалайзера
eq-widget-section = Виджет
eq-width-label = Ширина
eq-window-title = rox - Эквалайзер

## Keymap
keymap-close-window = Закрыть окно
    .description = Закрыть то окно, что впереди. Назначено везде, включая отделённые панели
keymap-decrease-font-size = Уменьшить размер текста
    .description = Шаг вниз по размеру текста для всего приложения
keymap-focus-search = Фокус на поиск
    .description = Поставить курсор в поле поиска медиатеки
keymap-group-editing = Правка
keymap-group-playback = Воспроизведение
keymap-group-view = Вид
keymap-group-windows = Окна
keymap-increase-font-size = Увеличить размер текста
    .description = Шаг вверх по размеру текста для всего приложения
keymap-key-backspace = Backspace
keymap-key-delete = Del
keymap-key-down = Вниз
keymap-key-end = End
keymap-key-esc = Esc
keymap-key-home = Home
keymap-key-insert = Ins
keymap-key-left = Влево
keymap-key-page-down = PgDn
keymap-key-page-up = PgUp
keymap-key-right = Вправо
keymap-key-space = Пробел
keymap-key-tab = Tab
keymap-key-up = Вверх
keymap-mod-alt = Alt
keymap-mod-cmd = Cmd
keymap-mod-ctrl = Ctrl
keymap-mod-fn = Fn
keymap-mod-option = Option
keymap-mod-shift = Shift
keymap-mod-super = Super
keymap-mod-win = Win
keymap-open-quick-play = Быстрый запуск
    .description = Поднять окно поиска и запуска поверх окна
keymap-open-settings = Открыть настройки
    .description = Открыть это окно
keymap-open-stats = Открыть статистику
    .description = Открыть окно статистики прослушиваний
keymap-quit = Выход
    .description = Выйти из rox. Назначено везде, ведь нет окна, из которого это не должно работать
keymap-reset-font-size = Сбросить размер текста
    .description = Вернуть размер текста к штатному
keymap-seek-backward = Перемотать назад
    .description = Шаг назад по играющему треку
keymap-seek-forward = Перемотать вперёд
    .description = Шаг вперёд по играющему треку
keymap-stamp-line = Отметить строку текста
    .description = Записать текущую позицию воспроизведения в редактируемую строку текста
keymap-toggle-playback = Воспроизведение / Пауза
    .description = Запустить текущий трек или поставить его на паузу там, где он есть
keymap-toggle-post-shader = Переключить шейдер наложения
    .description = Выключить и включить экранный шейдер. Назначено везде: шейдер может закрыть собой те элементы, через которые его иначе выключают
keymap-toggle-zoom = Развернуть группу панелей
    .description = Заполнить док последней нажатой группой панелей или выйти обратно

## Panel catalog
panel-catalog-album-carousel = Карусель альбомов
panel-catalog-artist-grid = Сетка исполнителей
panel-catalog-biography = Биография
panel-catalog-cover-art = Обложка
panel-catalog-drawer = Выдвижная панель
panel-catalog-eq-widget = Виджет эквалайзера
panel-catalog-filter = Фильтр
panel-catalog-folder-tree = Дерево папок
panel-catalog-genre-grid = Сетка жанров
panel-catalog-group-application = Приложение
panel-catalog-group-arrangement = Расстановка
panel-catalog-group-catalogue = Каталог
panel-catalog-group-controls = Управление
panel-catalog-group-details = Детали
panel-catalog-group-experimental = Экспериментальные
panel-catalog-group-visualizers = Визуализация
panel-catalog-history = История
panel-catalog-menu = Меню
panel-catalog-metadata = Метаданные
panel-catalog-mini-toggle = Переключатель мини
panel-catalog-oscilloscope = Осциллограф
panel-catalog-overlay = Наложение
panel-catalog-particles = Частицы
panel-catalog-playlists = Плейлисты
panel-catalog-queue = Очередь
panel-catalog-queue-widget = Виджет очереди
panel-catalog-seek = Перемотка
panel-catalog-slide = Слайд
panel-catalog-spectrogram = Спектрограмма
panel-catalog-spectrum = Спектр
panel-catalog-stats-widget = Виджет статистики
panel-catalog-status = Статус
panel-catalog-theme-toggle = Переключатель темы
panel-catalog-track-info = Информация о треке
panel-catalog-vu-meter = VU-метр
panel-catalog-waveform = Волновая форма
panel-catalog-window-controls = Кнопки окна

## Updater
updater-already-latest = уже стоит последняя версия
updater-checksum-mismatch = контрольная сумма загрузки { $digest }, а не { $expected }, как заявлено в выпуске
updater-checksum-missing-entry = в { $sums } нет записи для { $name }; непроверяемая загрузка отклонена
updater-no-asset = в выпуске нет { $name }
updater-no-checksums = в выпуске нет { $sums }; непроверяемая загрузка отклонена
updater-no-release-build = для этой платформы нет сборки в выпуске
updater-overran = загрузка вышла за размер, заявленный в выпуске
updater-short = { $bytes ->
    [one] загрузка остановилась на { $done } из { $bytes } байта
    [few] загрузка остановилась на { $done } из { $bytes } байт
    [many] загрузка остановилась на { $done } из { $bytes } байт
   *[other] загрузка остановилась на { $done } из { $bytes } байта
}
updater-size-mismatch = сервер предложил { $claimed } байт, а выпуск заявляет { $bytes }

## Last.fm
lastfm-import-matching = Сверка с медиатекой
lastfm-import-read = Прочитано любимых треков: { $count }
lastfm-import-stopped = Остановлено, прочитано любимых треков: { $count }
lastfm-import-matched = , совпадений: { $count }
lastfm-import-added = , добавлено в избранное: { $count }

## Tag tools
tags-editor-clear-all = очистить всё
tags-editor-form-view = Форма
tags-editor-format-unsupported-all = Теги этого формата пока нельзя ни читать, ни писать.
tags-editor-format-unsupported-some = Часть этих файлов в формате, теги которого пока нельзя ни читать, ни писать.
tags-editor-guess-button = Разобрать
tags-editor-guess-folded = { $status }, ещё { $count } не показано
tags-editor-guess-help = { $placeholders }; / соответствует папке выше, %skip% отбрасывает
tags-editor-guess-match-count = совпадений { $hits } из { $total }
tags-editor-guess-no-match = нет совпадений
tags-editor-guess-pattern-label = шаблон
tags-editor-loading = Загрузка тегов...
tags-editor-look-up = Найти
tags-editor-clear-on-save = Очищается при сохранении
tags-editor-multiple-values = Несколько значений
tags-editor-other-tags = Прочие теги ({ $count })
tags-editor-remove = удалить
tags-editor-reveal = Показать
tags-editor-save-errors = Файлов с ошибкой: { $count }; { $error }
tags-editor-saving-progress = Сохранение { $done }/{ $total }...
tags-editor-table-view = Таблица
tags-editor-tags-section = Теги
tags-editor-unknown-partial = { $count } из { $total }
tags-editor-unread-count = Не удалось прочитать теги файлов: { $failed } из { $total }
tags-editor-will-clear = будет очищено
tags-editor-will-remove = будет удалено
tags-editor-window-title = rox - Редактор тегов
tags-guess-empty-segment = шаблон даёт пустое имя папки или файла
tags-guess-no-placeholders = нет подстановок
tags-guess-skip-renders-nothing = %skip% нечего подставить
tags-guess-unclosed = незакрытый %
tags-guess-unknown-placeholder = неизвестная подстановка %{ $name }%
tags-matcher-blocked-arm = Включите поле, чтобы применить
tags-matcher-blocked-no-match = Нечего применять, совпадений нет
tags-matcher-blocked-pick = Выберите совпадение
tags-matcher-blocked-writing = Запись тегов...
tags-matcher-match-count = { $count ->
    [one] { $count } совпадение
    [few] { $count } совпадения
    [many] { $count } совпадений
   *[other] { $count } совпадения
}
tags-matcher-no-matches = Совпадений не найдено
tags-matcher-pick-match = Выберите совпадение
tags-matcher-search-failed = Поиск не удался: { $error }
tags-matcher-searching = Поиск...
tags-matcher-tagging = Запись тегов: { $track }
tags-matcher-window-title = rox - Поиск метаданных
tags-rename-blocked-cue = трек из cue, своего файла нет
tags-rename-blocked-duplicate = два трека дают одно и то же имя
tags-rename-blocked-occupied = файл уже там
tags-rename-blocked-outside-roots = вне всех корней медиатеки
tags-rename-blocked-unresolved = ещё не в каталоге
tags-rename-move-error = { $name }: { $error }
tags-rename-move-errors = Файлов с ошибкой: { $count }; { $error }
tags-rename-moving = Перемещение { $done }/{ $total }...
tags-rename-nothing-to-move = Нечего перемещать
tags-rename-pattern-help = { $placeholders }; / создаёт папку, расширение берётся от файла
tags-rename-pattern-section = Шаблон
tags-rename-preview-section = Предпросмотр
tags-rename-unchanged = без изменений
tags-rename-will-move = будет перемещено { $count } из { $total }
tags-rename-window-title = rox - Переименование файлов
tags-repair-affected-files = Затронутые файлы
tags-repair-section = Починка
tags-repair-check-to-repair = Отметьте файл, чтобы починить его
tags-repair-count = { $count ->
    [one] { $count } файл
    [few] { $count } файла
    [many] { $count } файлов
   *[other] { $count } файла
}
tags-repair-count-so-far = пока { $count }
tags-repair-label-scope = область
tags-repair-no-affected = Затронутых файлов не найдено.
tags-repair-no-folder = Нет папки для сканирования; добавьте её в медиатеку или выберите вручную.
tags-repair-pick-folder = Выбрать папку...
tags-repair-progress = Починка { $done }/{ $total }...
tags-repair-repair-button = { $count ->
    [0] Починить
   *[other] Починить ({ $count })
}
tags-repair-result = { $count ->
    [one] Починен { $count } файл
    [few] Починено { $count } файла
    [many] Починено { $count } файлов
   *[other] Починено { $count } файла
}
tags-repair-result-failed = Починено { $count }, ошибок: { $failed }
tags-repair-scan-first = Сначала просканируйте
tags-repair-scan-hint = Просканируйте, чтобы найти файлы с повреждениями тегов, которые чинит перезапись.
tags-repair-select-all = Выбрать всё
tags-repair-select-none = Снять выбор
tags-repair-whole-library = Вся медиатека
tags-repair-window-title = rox - Починка тегов

## Convert
convert-arg-names-file = «{ $token }» называет файл; путь назначения берётся из папки и шаблона
convert-section-output = Вывод
convert-section-preview = Предпросмотр
convert-arg-not-flag-or-value = «{ $token }» не флаг и не значение для флага
convert-check-wrote-nothing = ffmpeg завершился чисто, но ничего не записал
convert-custom-ext-empty = Контейнер задаётся расширением, поэтому его нужно указать
convert-custom-ext-invalid = «{ $ext }» не имя контейнера; буквы и цифры, без точки
convert-dialog-browse = Обзор...
convert-dialog-check-passed = ffmpeg закодировал с этими параметрами мгновение тишины, значит они работают
convert-dialog-check-waiting = Проверяется через ffmpeg, как только вы перестанете печатать
convert-dialog-checking = Проверка через ffmpeg...
convert-dialog-choose-folder = Выберите папку для записи
convert-dialog-convert-button = Преобразовать
convert-dialog-custom-label = Свой
convert-dialog-custom-menu-item = Свой...
convert-dialog-custom-note = Аргументы делятся по пробелам, кавычек нет; встроенные обложки для своих форматов не копируются
convert-dialog-format-not-ready = Набранный формат ещё не прошёл через ffmpeg
convert-dialog-label-extension = расширение
convert-dialog-label-format = формат
convert-dialog-label-into = в
convert-dialog-label-named = с именем
convert-dialog-mirror = Повторить структуру папок медиатеки
convert-dialog-nothing-to-convert = Преобразовывать нечего: все строки пропущены
convert-dialog-pattern-help = { $placeholders }; / создаёт папку, расширение задаёт формат
convert-dialog-pick-folder = Укажите папку для записи
convert-dialog-span-note = { $count } вырезано из образа cue и снабжено тегами из медиатеки
convert-dialog-will-convert = будет преобразовано { $count } из { $total }
convert-dialog-window-title = rox - Преобразование
convert-ffmpeg-silent-failure = ffmpeg упал, не сказав почему
convert-flag-attach = -attach читает отдельный файл, а это здесь не разрешено
convert-flag-f = Контейнер задаётся расширением, поэтому -f задавать нельзя
convert-flag-i = Вход — это выбранный вами трек, так что -i задавать нельзя
convert-flag-n = -n уже стоит на каждом запуске
convert-flag-y = Здесь ничего не перезаписывается, поэтому -y недоступен; существующий файл назначения пропускается
convert-preset-flac = FLAC
convert-preset-mp3-320 = MP3 320 кбит/с
convert-preset-mp3-v0 = MP3 V0
convert-preset-opus-192 = Opus 192 кбит/с
convert-preset-wav = WAV
convert-skip-duplicate = два трека дают одно и то же имя
convert-skip-exists = уже там
convert-summary-failed = , ошибок: { $count }
convert-summary-files = { $count ->
    [one] { $count } файл
    [few] { $count } файла
    [many] { $count } файлов
   *[other] { $count } файла
}
convert-summary-line = { $files } в { $dest }
convert-summary-skipped = , пропущено { $count }
convert-summary-stopped = Остановлено, преобразовано { $files } в { $dest }
convert-version-answered = { $binary } запустился, но версию не сообщил

## Duplicates
duplicates-auto-select = Выбрать автоматически
duplicates-check-to-trash = Отметьте копии, чтобы отправить их в корзину
duplicates-copy-count = { $count ->
    [one] { $count } копия
    [few] { $count } копии
    [many] { $count } копий
   *[other] { $count } копии
}
duplicates-different-albums = разные альбомы
duplicates-filter-placeholder = Фильтр по названию, исполнителю или папке
duplicates-groups-summary = { $groups ->
    [one] { $groups } группа, лишних копий { $extras }
    [few] { $groups } группы, лишних копий { $extras }
    [many] { $groups } групп, лишних копий { $extras }
   *[other] { $groups } группы, лишних копий { $extras }
}
duplicates-library-loading = Медиатека ещё загружается; попробуйте чуть позже.
duplicates-no-duplicates = Дубликатов не найдено.
duplicates-no-filter-matches = Ни одна группа не подходит под фильтр.
duplicates-policy-newest = Оставить самое новое
duplicates-policy-oldest = Оставить самое старое
duplicates-policy-quality = Оставить лучшее качество
duplicates-scan-hint = Просканируйте медиатеку на треки, которые встречаются больше одного раза.
duplicates-select-none = Снять выбор
duplicates-selected-count = выбрано: { $count }
duplicates-trash-button = { $count ->
    [0] В корзину
   *[other] В корзину ({ $count })
}
duplicates-trash-error = { $name }: { $error }
duplicates-trash-result = { $count ->
    [one] В корзину отправлен { $count } файл
    [few] В корзину отправлено { $count } файла
    [many] В корзину отправлено { $count } файлов
   *[other] В корзину отправлено { $count } файла
}
duplicates-trash-result-failed = В корзину отправлено { $count }, ошибок: { $failed }
duplicates-trashing = Отправка в корзину { $done }/{ $total }...
duplicates-window-title = rox - Дубликаты

## Smart playlists
smart-playlist-descending = По убыванию
smart-playlist-edit-title = Правка умного плейлиста
smart-playlist-limit-label = Лимит
smart-playlist-limit-placeholder = Без лимита
smart-playlist-match-count = { $count ->
    [one] { $count } трек подходит
    [few] { $count } трека подходят
    [many] { $count } треков подходит
   *[other] { $count } трека подходят
}
smart-playlist-matched-tracks = Подошедшие треки
smart-playlist-new-title = Новый умный плейлист
smart-playlist-no-matches = Подходящих треков нет
smart-playlist-query-label = Запрос
smart-playlist-sort-default = Порядок по умолчанию
smart-playlist-sort-added = Добавлено
smart-playlist-sort-label = Сортировка
smart-playlist-unknown-field = «{ $field }:» — это не поле, поэтому оно ищется как обычный текст
smart-playlist-window-title = rox - { $verb }

## Playlist creation
playlist-create-not-savable = Назовите плейлист, чтобы сохранить его
playlist-create-placeholder = Название плейлиста
playlist-create-rename-title = Переименовать плейлист
playlist-create-title = Новый плейлист
playlist-create-window-title = rox - { $verb }

## Cover tools
cover-art-back = Задняя
cover-art-disc = Диск
cover-art-front = Передняя
cover-artwork = Изображение
    .description = Какую картинку показывать; слот, которого нет в файле, откатывается к передней обложке
cover-disc-style = Стиль диска
    .description = Оформить изображение как компакт-диск или как этикетку винила
cover-disc-off = Выкл
cover-disc-cd = CD
cover-disc-vinyl = Винил
cover-editor-choose-image = Выбрать изображение
cover-editor-multiple = Несколько
cover-editor-none = Нет
cover-editor-not-an-image = Этот файл не изображение, которое rox умеет встраивать
cover-editor-not-decoded = Это изображение не удалось декодировать
cover-editor-reading = Чтение текущей обложки...
cover-editor-remove = Удалить
cover-editor-replace = Заменить
cover-editor-revert = Откатить
cover-editor-save-errors = Файлов с ошибкой: { $count }; { $error }
cover-editor-saving-progress = Сохранение { $done }/{ $total }...
cover-editor-search-online = Искать в сети
cover-editor-section = Обложка
cover-editor-slot-back = Задняя обложка
cover-editor-slot-front = Передняя обложка
cover-editor-slot-media = Носитель
cover-editor-will-remove = Будет удалено
cover-editor-window-title = rox - Обложка
cover-matcher-blocked-fetching = Загрузка полного изображения...
cover-matcher-blocked-no-cover = Ставить нечего, обложек нет
cover-matcher-blocked-pick = Выберите обложку, чтобы поставить её
cover-matcher-cover-count = { $count ->
    [one] { $count } обложка
    [few] { $count } обложки
    [many] { $count } обложек
   *[other] { $count } обложки
}
cover-matcher-editor-closed = Редактор обложек был закрыт
cover-matcher-no-covers = Обложек не найдено
cover-matcher-search-failed = Поиск не удался: { $error }
cover-matcher-set-cover = Поставить обложку
cover-matcher-setting = Установка...
cover-matcher-tile-info = { $provider }  { $width }px
cover-matcher-unsupported-format = Неподдерживаемый формат изображения
cover-matcher-window-title = rox - Поиск обложек
cover-spin = Вращение
    .description = Крутить диск, пока играет трек; действует на слот диска или на стиль диска
cover-spin-disc = Крутить диск
cover-spin-ramp = Разгон вращения
    .description = Сколько диску нужно, чтобы выйти на полную скорость и накатом сбросить её обратно
cover-spin-speed = Скорость вращения
    .description = Полная скорость, в оборотах в минуту
cover-stretch = Растягивать
    .description = Заполнить панель, не считаясь с пропорциями изображения
cover-stretch-to-fill = Растянуть до заполнения
cover-title = Обложка

## Lyrics
lyrics-always-centered = Всегда по центру
    .description = Добить концы пустотой, чтобы первая и последняя строки тоже вставали по центру
lyrics-auto-search = Автопоиск
    .description = Искать в сети для трека без слов и сохранять уверенное совпадение, без выбора вручную
lyrics-bold = Жирный
lyrics-build-word-by-word = Собирать по словам
    .description = Открывать слова по мере того, как их поют, как в караоке; неспетые строки остаются скрытыми
lyrics-edge-bottom = Снизу
lyrics-edge-top = Сверху
lyrics-edit-hint-after-stamp = чтобы отметить
lyrics-edit-hint-or = или
lyrics-edit-loading = Загрузка текста...
lyrics-edit-lyrics = Править текст
lyrics-edit-saving = Сохранение...
lyrics-edit-section = Текст песни
lyrics-edit-stamp = Отметить
lyrics-edit-stamp-time = Отметить { $time }
lyrics-edit-window-title = rox - Правка текста
lyrics-fade-lines-in = Проявлять строки
    .description = Выводить строку из полутени, когда она становится активной
lyrics-falloff-edge = Край затухания
    .description = С какой стороны от активной строки затухание гасит текст
lyrics-find-online = Найти текст в сети...
lyrics-follow-playback = Следовать за воспроизведением
    .description = Плавно выводить активную строку к середине, пока играет синхронизированный текст
lyrics-font = Шрифт
    .description = Гарнитура текста песни; по умолчанию следует шрифту приложения
lyrics-gap-threshold = Порог паузы
    .description = Сколько должно длиться вступление или пауза, чтобы получить отдых
lyrics-lead-in-rest = Отдых перед вступлением
    .description = Показывать пустой отдых перед длинным вступлением, чтобы первая строка проявилась, когда до неё дойдёт
lyrics-line-falloff = Затухание строк
    .description = Насколько каждая строка тускнеет с каждым шагом от активной
lyrics-line-spacing = Межстрочный интервал
    .description = Расстояние между синхронизированными строками, кратное размеру текста
lyrics-look-again = Искать снова
lyrics-mark-dots = Точки
lyrics-mark-note = Нота
lyrics-marked-notice = Отмечено: без текста
lyrics-matcher-blocked-no-match = Нечего применять, совпадений нет
lyrics-matcher-blocked-pick = Выберите совпадение, чтобы применить
lyrics-matcher-blocked-saving = Сохранение слов...
lyrics-matcher-match-count = { $count ->
    [one] { $count } совпадение
    [few] { $count } совпадения
    [many] { $count } совпадений
   *[other] { $count } совпадения
}
lyrics-matcher-no-query = У этого трека нет исполнителя и названия, по которым можно искать
lyrics-matcher-pick-preview = Выберите совпадение для предпросмотра
lyrics-matcher-search-failed = Поиск не удался: { $error }
lyrics-matcher-synced-tag = { $provider }  синхронизировано
lyrics-matcher-window-title = rox - Поиск текстов
lyrics-no-lyrics-notice = Нет текста
lyrics-no-lyrics-track = Для этого трека нет текста
lyrics-rest-in-gaps = Отдых в паузах
    .description = Переходить на пустой отдых на длинном проигрыше вместо того, чтобы держать последнюю строку
lyrics-rest-marker = Знак отдыха
    .description = Что показывает строка без слов в синхронизированном тексте, в паузах и на пустых строках
lyrics-search-button = Кнопка поиска в сети
    .description = Показывать кнопку поиска на пустой панели; контекстное меню всё равно умеет искать текст
lyrics-search-online = Искать в сети
lyrics-show-song-name = Показывать название песни
    .description = Показывать название трека на пустой панели, над строкой об отсутствии текста
lyrics-text-size = Размер текста
    .description = Текст песни; высота синхронизированной строки следует за ним
lyrics-title = Текст песни
lyrics-title-unsynced = Название над несинхронизированным
    .description = Закрепить название трека над несинхронизированным текстом, чтобы его было видно и в короткой панели
lyrics-wipe-lyrics = Стереть текст

## Analysis passes
pass-acoustic-body = { $model } разбирает, как звучит каждый из них, чтобы медиатека умела находить музыку, похожую на играющую. Всё считается на этой машине, а уже описанное пропускается. { $lands }
pass-acoustic-lands-database = Результаты идут в базу медиатеки, а ваши файлы остаются нетронутыми.
pass-acoustic-lands-tags = Результаты идут в базу медиатеки, а для MP3 и FLAC ещё и в теги каждого файла, так что сохранятся при пересборке базы. У остальных форматов остаётся только копия в базе.
pass-acoustic-title = { $count ->
    [one] Проанализировать { $count } трек?
    [few] Проанализировать { $count } трека?
    [many] Проанализировать { $count } треков?
   *[other] Проанализировать { $count } трека?
}
pass-analyze = Проанализировать
pass-estimate-at = { $estimate } в { $workers_phrase }.
pass-estimate-button = Оценить
pass-estimating = Оценка...
pass-measure = Измерить
pass-no-estimate = На этой машине ещё ничего не запускалось, поэтому оценки нет. «Оценить» прогонит несколько треков и посчитает остальное по ним.
pass-replaygain-body = Каждый файл декодируется и измеряется, чтобы играть на той громкости, на которую его свели. Альбомы меряются целиком там, где усиления нет ни у одного их трека. { $lands }
pass-replaygain-lands-database = Числа идут в базу медиатеки, а ваши файлы остаются нетронутыми.
pass-replaygain-lands-tags = Числа записываются обратно в теги каждого файла, туда, где их читает любой другой плеер.
pass-replaygain-title = { $count ->
    [one] Измерить { $count } трек?
    [few] Измерить { $count } трека?
    [many] Измерить { $count } треков?
   *[other] Измерить { $count } трека?
}
pass-tempo-body = Из каждого файла декодируются два окна по полминуты и в них считаются удары, чтобы медиатека умела показать, на какой скорости идёт трек. Лучше всего работает на музыке, записанной под клик, и пропускает то, что измерить не выходит. Числа идут в базу медиатеки, а ваши файлы остаются нетронутыми.
pass-tempo-title = { $count ->
    [one] Определить темп { $count } трека?
    [few] Определить темп { $count } треков?
    [many] Определить темп { $count } треков?
   *[other] Определить темп { $count } треков?
}
pass-timing = Считаем темп на нескольких треках...
pass-timing-failed = Не удалось определить темп для этой медиатеки: { $error }
pass-workers = Потоки

## Quick play
quick-play-comfortable-rows = Просторные строки
    .description = Дать каждому результату больше высоты
quick-play-cover = Обложка
    .description = Показывать миниатюру обложки слева от каждого результата
quick-play-duration = Длительность
    .description = Показывать длину каждого результата справа
quick-play-narrow-by = Сузить по
quick-play-search-placeholder = Поиск по медиатеке
quick-play-subtitle = Подпись
    .description = Показывать исполнителя и альбом под каждым результатом
quick-play-tag-album = Альбом
quick-play-tag-artist = Исполнитель

## Drawer panel
drawer-add-tooltip = Добавить выдвижную панель
drawer-answers = Откликается на
    .description = Какие выборы открывают ящик: только его собственная основная панель или любая панель за её пределами
drawer-dim = Затемнение
    .description = Насколько сильно тускнеет основная панель за открытым ящиком
drawer-edge = Край
    .description = Край, к которому прижат ящик и от которого он выезжает
drawer-edge-bottom = Снизу
drawer-edge-top = Сверху
drawer-handle = Ручка
    .description = Показывать зацеп у края панели. Если скрыть, до выбора от ящика не видно ничего, а потом зацеп остаётся, пока держится выбор, так что закрывшийся ящик можно вытянуть обратно
drawer-open-on = Открывать по
    .description = Задержка на ручке открывает ящик всегда; выбор добавляет к этому клик в основной панели
drawer-pin-open = Закрепить открытым
drawer-reveal = Раскрытие
    .description = Какую часть панели закрывает открытый ящик
drawer-scope-elsewhere = В других панелях
drawer-scope-main = Основная панель
drawer-title = Выдвижная панель
drawer-trigger-hover = Наведение
drawer-trigger-selection = Выбор

## Mini player
mini-tip-back = Назад к полному макету
mini-tip-none = Мини-макет не назначен
mini-tip-shrink = Свернуть до мини-плеера
mini-title = Переключатель мини

## System tray
tray-open = Открыть
tray-pause = Пауза
tray-play = Воспроизвести
tray-quit = Выход

## Window controls
window-controls-mini-toggle = Переключатель мини
    .description = Ставить переключатель мини-макета первым; появляется, как только мини-макет назначен
window-controls-minimize = Свернуть
window-controls-style = Стиль
    .description = Плоские значки или светофор macOS
window-controls-style-icons = Значки
window-controls-title = Кнопки окна
window-controls-traffic-lights = Светофор

## Section names the audio panels share. Each one heads the same kind of
## rows in the spectrum, VU, oscilloscope, spectrogram and particles
## settings, so they're defined once rather than per panel.
viz-section-analysis = Анализ
viz-section-color = Цвет
viz-section-peaks = Пики
viz-section-playback = Воспроизведение
viz-section-scale = Шкала
viz-section-signal = Сигнал

## Particles panel
particles-add-emitter = Добавить эмиттер
particles-aim = Прицел
particles-aim-fixed = Фиксированный
particles-aim-outward = Наружу
particles-burst = Залп
particles-color = Цвет
particles-cone = Конус
particles-direction = Направление
    .description = Куда тянет; 0 вверх, 180 вниз
particles-drag = Сопротивление
    .description = Сколько скорости съедает воздух за секунду; ноль — это вакуум
particles-drift = Дрейф
    .description = Как быстро движется само поле, чтобы вихри не стояли на месте
particles-edit-emitters = Править эмиттеры
particles-emitter-label = Эмиттер { $index }
particles-emitter-target = Эмиттер { $index } { $target }
particles-emitters-empty = Пока нет эмиттеров. Добавьте один, чтобы запустить поле.
particles-glow = Свечение
    .description = Положить мягкий ореол за каждой частицей
particles-gravity = Гравитация
particles-gravity-strength = Сила
    .description = Постоянная тяга на всё, что в полёте
particles-height = Высота
particles-hold-on-pause = Держать на паузе
    .description = Заморозить поле на паузе, а не давать ему разлететься
particles-length = Длина
particles-lifetime = Время жизни
particles-position-x = Позиция X
particles-position-y = Позиция Y
particles-radius = Радиус
particles-rate = Частота
particles-rotation = Поворот
particles-round-particles = Круглые частицы
    .description = Рисовать точки вместо квадратов
particles-scale = Масштаб
    .description = Насколько широк один вихрь; маленький бурлит, большой перекатывается
particles-section-emitters = Эмиттеры
particles-section-medium = Среда
particles-section-particles = Частицы
particles-shape = Форма
particles-shape-box = Прямоугольник
particles-shape-line = Линия
particles-shape-point = Точка
particles-shape-ring = Кольцо
particles-size = Размер
particles-speed = Скорость
particles-trigger = Триггер
particles-trigger-continuous = Непрерывно
particles-turbulence = Турбулентность
particles-turbulence-drift = Дрейф турбулентности
particles-turbulence-scale = Масштаб турбулентности
particles-turbulence-strength = Сила
    .description = Насколько сильно поле толкает частицы; ноль выключает
particles-width = Ширина

## Spectrum panel
spectrum-axis-labels = Подписи осей
    .description = Отметить диапазон по ширине панели: октавы (C1, C2, ...) или частоты (100, 1k, 10k)
spectrum-bar-gap = Зазор между полосами
    .description = Место между полосами, чем шире зазор, тем меньше полос помещается
spectrum-bar-width = Ширина полосы
    .description = Насколько толстой рисуется каждая полоса, чем тоньше, тем больше полос помещается
spectrum-block-gap = Зазор между блоками
    .description = Шов между ячейками в столбце
spectrum-block-height = Высота блока
    .description = Насколько высокой рисуется каждая ячейка в столбце
spectrum-cap-gravity = Гравитация пиков
    .description = Насколько резко падают метки пиков, когда полоса опускается
spectrum-fft-size = Размер FFT
    .description = Окно анализа; короткое реагирует быстро, длинное различает точнее
spectrum-gradient-base-color = Базовый цвет
    .description = Тихий конец своей шкалы
spectrum-gradient-cover = Обложка
spectrum-gradient-mode = Градиент
    .description = Красить полосы по громкости: шкалой темы, цветами обложки при окраске по треку или своей парой цветов
spectrum-gradient-theme = Тема
spectrum-gradient-tip-color = Цвет вершины
    .description = Громкий конец своей шкалы
spectrum-high-bound-description = Самая высокая частота, которую анализируют полосы
spectrum-high-fft-size = Размер FFT сверху
    .description = Окно анализа для полос выше точки разделения
spectrum-hold-on-pause = Держать на паузе
    .description = Заморозить полосы на паузе, а не давать им упасть в тишину
spectrum-labels-frequency = Частота
spectrum-labels-pitch = Ноты
spectrum-low-bound-description = Самая низкая частота, которую анализируют полосы
spectrum-orientation = Ориентация
    .description = Край, от которого растут полосы
spectrum-outline-bars = Контурные полосы
    .description = Рисовать каждую полосу пустым контуром вместо заливки градиентом
spectrum-outline-width = Толщина контура
    .description = Толщина обводки пустых полос
spectrum-peak-caps = Метки пиков
    .description = Держать метку на недавнем пике каждой полосы
spectrum-section-bands = Полосы
spectrum-split-at = Делить на
    .description = Где встречаются зоны, с привязкой к ближайшей полосе
spectrum-split-zones = Разделить зоны
    .description = Анализировать ниже и выше частоты разделения окнами разного размера
spectrum-style = Стиль
    .description = Классические полосы, блоки в стиле LED или сплошная линия
spectrum-style-bars = Полосы
spectrum-style-blocks = Блоки
spectrum-style-line = Линия
spectrum-symmetry = Симметрия
    .description = Сложить спектр вокруг центра; прямая ставит низы по краям, обратная сводит их в середину
spectrum-symmetry-forward = Прямая
spectrum-symmetry-reverse = Обратная

## Waveform panel
waveform-bar-gap = Зазор между полосами
    .description = Место между полосами, ноль сливает их в сплошную фигуру
waveform-bar-width = Ширина полосы
    .description = Насколько толстой рисуется каждая полоса
waveform-outline = Контур
    .description = Обводить полосы вместо заливки; слитые полосы читаются как одна фигура
waveform-scrobble-marker = Метка скробблинга
    .description = Тонкая линия там, где трек засчитывается как отскробленный на Last.fm
waveform-split-channels = Разделить каналы
    .description = По строке на канал, левый над правым; моно-треки остаются одной строкой
waveform-unavailable = Волновая форма для этого трека недоступна

## VU panel
vu-ballistics = Баллистика
    .description = VU набирает громкость медленно; Пик вскакивает и плавно опадает
vu-ballistics-peak = Пик
vu-cap-gravity = Гравитация пиков
    .description = Насколько резко падают метки пиков, когда индикатор опускается
vu-channels = Каналы
    .description = Разделить стереопару или свести в один индикатор
vu-channels-mono = Моно
vu-channels-stereo = Стерео
vu-db-scale = Шкала дБ
    .description = Рисовать подписанную сетку по отметкам дБ за индикаторами
vu-gradient-mode = Градиент
    .description = Красить индикаторы по уровню: шкалой темы, цветами обложки при окраске по треку или своей парой цветов
vu-hold-on-pause = Держать на паузе
    .description = Заморозить индикаторы на паузе, а не давать им упасть в тишину
vu-orientation = Ориентация
    .description = Край, от которого растут индикаторы
vu-peak-caps = Метки пиков
    .description = Держать метку на недавнем пике каждого индикатора
vu-section-meter = Индикатор
vu-segment-gap = Зазор между сегментами
    .description = Шов между ячейками в столбце
vu-segment-height = Высота сегмента
    .description = Насколько высокой рисуется каждая ячейка в столбце
vu-style = Стиль
    .description = Сплошной столбец или сегменты в стиле LED
vu-style-continuous = Сплошной
vu-style-segments = Сегменты

## Spectrogram panel
spectrogram-ceiling = Потолок
    .description = Уровень, который отображается на светлый конец цветовой карты, так что всё громче него упирается туда
spectrogram-colormap = Цветовая карта
    .description = Как громкость переводится в цвет
spectrogram-colormap-cover = Обложка
spectrogram-colormap-grayscale = Оттенки серого
spectrogram-colormap-ice = Лёд
spectrogram-colormap-magma = Magma
spectrogram-colormap-theme = Тема
spectrogram-colormap-viridis = Viridis
spectrogram-direction = Направление
    .description = Край, с которого входят новые столбцы, что также определяет, идёт ли ось частот вверх по панели или поперёк неё
spectrogram-fft-size = Размер FFT
    .description = Размер окна анализа, компромисс между тем, как быстро столбец реагирует на переходный процесс, и тем, насколько хорошо он разделяет две низкие ноты
spectrogram-floor = Пол
    .description = Уровень, который отображается на тёмный конец цветовой карты, так что всё тише него читается как фон
spectrogram-grid = Сетка
    .description = Частотные линии поверх картинки
spectrogram-high-bound = Верхняя граница
    .description = Верх оси частот, ограниченный ниже частоты Найквиста, чтобы отбросить почти беззвучные верхние октавы
spectrogram-history = История
    .description = Сколько столбцов панель хранит, прежде чем самый старый уйдёт за край
spectrogram-hold-on-pause = Держать на паузе
    .description = Держать неподвижную картинку на паузе, а не давать тишине наползать на неё
spectrogram-labels = Подписи
    .description = Числа частот вдоль линейки, там, где на панели есть для них место
spectrogram-log-scale = Логарифмическая шкала
    .description = Дать каждой октаве одинаковое место, музыкальное чтение, вместо равномерного шага в Гц, как в лабораторном приборе
spectrogram-low-bound = Нижняя граница
    .description = Низ оси частот
spectrogram-section-picture = Изображение
spectrogram-speed = Скорость
    .description = Как быстро прокручивается картинка, в столбцах в секунду

## Oscilloscope panel

oscilloscope-channels = Каналы
    .description = Свести в одну кривую, наложить друг на друга, или сложить в стопку с отдельной рамкой для каждого
oscilloscope-channels-mono = Моно
oscilloscope-channels-overlay = Наложение
oscilloscope-channels-split = Раздельно
oscilloscope-fill = Заливка
    .description = Мягкая заливка между кривой и центральной линией
oscilloscope-gain = Усиление
    .description = Вертикальный масштаб, чтобы поднять тихий трек до читаемой кривой
oscilloscope-gradient-mode = Градиент
    .description = Красить кривую по размаху: шкалой темы, цветами обложки при окраске по треку или своей парой цветов
oscilloscope-grid = Сетка
    .description = Рисовать сетку за кривой
oscilloscope-hold-on-pause = Держать на паузе
    .description = Держать неподвижный кадр на паузе, а не давать кривой лечь ровной линией
oscilloscope-line-width = Толщина линии
    .description = Насколько толстой рисуется кривая
oscilloscope-persistence = Послесвечение
    .description = Как долго предыдущие кадры задерживаются за кривой, тот самый эффект послесвечения люминофора
oscilloscope-section-trace = Кривая
oscilloscope-trigger = Триггер
    .description = Начинать каждый кадр там, где сигнал пересекает уровень триггера, чтобы периодический материал стоял на месте
oscilloscope-trigger-falling = Спадающий
oscilloscope-trigger-level = Уровень триггера
    .description = Уровень, на котором ищется пересечение
oscilloscope-trigger-off = Выкл
oscilloscope-trigger-rising = Нарастающий
oscilloscope-window = Окно
    .description = Сколько времени охватывает кривая по ширине панели

## Shader panel
shader-panel-compile-error = Этот шейдер не скомпилировался:
shader-panel-compile-title = Этот шейдер не скомпилировался
shader-panel-enable = Включить
shader-panel-inspect = Осмотреть
shader-panel-note-empty-body = Выберите пример или укажите панели файл .wgsl, определяющий fs_user(uv).
shader-panel-note-empty-title = Шейдер не загружен.
shader-panel-note-missing-body = Эта панель ссылается на шейдер, которого в рабочем пространстве нет, так что выполнять нечего.
shader-panel-note-missing-title = { $name } нет среди шейдеров этого рабочего пространства.
shader-panel-note-off-body = Исходник и его привязки на месте, просто не выполняются.
shader-panel-note-off-title = Этот шейдер выключен.
shader-panel-note-pending-body = Он пришёл с макетом или рабочим пространством, а не с этой машины, поэтому остаётся выключенным, пока вы его не просмотрите.
shader-panel-note-pending-title = Этот шейдер ещё не прочитан.
shader-pending-origin-file = Утверждается, что он пришёл из { $path }
shader-pending-origin-inline = Файла за ним нет; исходник пришёл вместе с макетом
shader-pending-more-lines = { $count ->
    [one] ... ещё { $count } строка
    [few] ... ещё { $count } строки
    [many] ... ещё { $count } строк
   *[other] ... ещё { $count } строки
}
shader-eject-name-taken = { $count ->
    [one] У { $name } уже { $count } пронумерованная копия среди шейдеров этого рабочего пространства
    [few] У { $name } уже { $count } пронумерованные копии среди шейдеров этого рабочего пространства
    [many] У { $name } уже { $count } пронумерованных копий среди шейдеров этого рабочего пространства
   *[other] У { $name } уже { $count } пронумерованной копии среди шейдеров этого рабочего пространства
}
shader-eject-not-in-pool = { $name } нет среди шейдеров этого рабочего пространства
shader-eject-failed = запись наружу: { $error }
shader-panel-pick = Выбрать шейдер
shader-panel-run-shader = Выполнять шейдер
    .description = Если выключить, исходник, закладка и привязки останутся на месте, а рисоваться ничего не будет
shader-panel-section-routes = Маршруты

## Genre grid panel
genre-grid-clear-picked = Очистить выбранные жанры
genre-grid-desaturate = Обесцвечивать при воспроизведении
    .description = Сводить все плитки, кроме играющего жанра, в оттенки серого; наведение возвращает плитке цвет
genre-grid-dim-while-playing = Затемнять при воспроизведении
    .description = Гасить все плитки, кроме играющего жанра; наведение снова зажигает плитку
genre-grid-follow-description = Прокручивать к играющему жанру при каждой смене трека
genre-grid-merge-many = { $count ->
    [one] Объединить { $count } жанр в «{ $target }»
    [few] Объединить { $count } жанра в «{ $target }»
    [many] Объединить { $count } жанров в «{ $target }»
   *[other] Объединить { $count } жанра в «{ $target }»
}
genre-grid-merge-one = Объединить «{ $source }» с «{ $target }»
genre-grid-pick-filters = Выбор фильтрует медиатеку
    .description = Клик по жанру сужает до него каждую панель, следящую за общим поиском; если выключить, клик останется обычным выбором
genre-grid-play-genres = Воспроизвести жанры: { $count }
genre-grid-resume-description = Возвращаться к играющему жанру, когда вы перестали листать
genre-grid-show-names = Показывать названия
    .description = Печатать жанр под каждой плиткой, а не только при наведении
genre-grid-smooth-description = Плавно доезжать до жанра, а не прыгать
genre-grid-tally = { $albums ->
    [one] { $albums } альбом, треков: { $tracks }
    [few] { $albums } альбома, треков: { $tracks }
    [many] { $albums } альбомов, треков: { $tracks }
   *[other] { $albums } альбома, треков: { $tracks }
}
genre-grid-tile-face = Оформление плитки
    .description = Что показывает плитка: обложки альбомов жанра, обложки в его собственном цвете или плоскую цветную карточку с названием на ней
genre-grid-unmerge = { $count ->
    [one] Разъединить { $count } значение
    [few] Разъединить { $count } значения
    [many] Разъединить { $count } значений
   *[other] Разъединить { $count } значения
}

## Artist grid panel
artist-grid-clear-picked = Очистить выбранных исполнителей
artist-grid-desaturate = Обесцвечивать при воспроизведении
    .description = Сводить все плитки, кроме играющего исполнителя, в оттенки серого; наведение возвращает плитке цвет
artist-grid-dim-while-playing = Затемнять при воспроизведении
    .description = Гасить все плитки, кроме играющего исполнителя; наведение снова зажигает плитку
artist-grid-follow-description = Прокручивать к играющему исполнителю при каждой смене трека
artist-grid-group-mode = Одна плитка на
    .description = Указанный исполнитель альбома оставляет гостей записи за тем, кто её выпустил; исполнитель трека разводит каждое участие на свою плитку
artist-grid-pick-filters = Выбор фильтрует медиатеку
    .description = Клик по исполнителю сужает до него каждую панель, следящую за общим поиском; если выключить, клик останется обычным выбором
artist-grid-play-artists = Воспроизвести исполнителей: { $count }
artist-grid-portraits = Портреты исполнителей
    .description = Показывать собственное фото исполнителя, найденное один раз на имя и сохранённое на диск; если выключить, показывается обложка первого альбома
artist-grid-resume-description = Возвращаться к играющему исполнителю, когда вы перестали листать
artist-grid-section-grouping = Группировка
artist-grid-show-names = Показывать имена
    .description = Печатать исполнителя под каждой плиткой, а не только при наведении
artist-grid-smooth-description = Плавно доезжать до исполнителя, а не прыгать
artist-grid-tally = { $albums ->
    [one] { $albums } альбом, треков: { $tracks }
    [few] { $albums } альбома, треков: { $tracks }
    [many] { $albums } альбомов, треков: { $tracks }
   *[other] { $albums } альбома, треков: { $tracks }
}
artist-grid-track-artist = Исполнитель трека

## Wall panels
wall-dim-always = Всегда
    .description = Держать плитки притушенными, даже когда ничего не играет; в полную силу видна только плитка под курсором
wall-dim-amount = Сила затемнения
    .description = Насколько гаснут остальные плитки; 100% скрывает их
wall-gap = Зазор
    .description = Место между плитками
wall-name-alignment = Выравнивание имён
    .description = Выровнять подписи под их плитками
wall-rounding = Скругление
    .description = Скруглить углы каждой плитки; 100% даёт круг
wall-section-picking = Выбор
wall-show-counts = Показывать счётчики
    .description = Счёт альбомов и треков под каждым именем
wall-tile-size = Размер плитки
    .description = Длинная сторона плиток; столбцы делят ширину панели поровну

## Metadata panel
metadata-cover-background = Обложка фоном
    .description = Обложка трека за полями
metadata-display = Отображение
    .description = Лист с названием во главе или плоская таблица из полей и значений с самого верха
metadata-display-sheet = Лист
metadata-display-table = Таблица
metadata-edit-save = Сохранить
metadata-field-bit-depth = Разрядность
metadata-field-bitrate = Битрейт
metadata-field-codec = Кодек
metadata-field-comment = Комментарий
metadata-field-disc = Диск
metadata-field-file = Файл
metadata-field-sample-rate = Частота дискретизации
metadata-field-track = Трек
metadata-fields = Поля
    .description = Какие поля перечисляет лист; поле, которого нет у трека, остаётся скрытым
metadata-find-online = Найти метаданные в сети...
metadata-no-library = Нет медиатеки
metadata-row-borders-description = Волосяная линия под каждой строкой таблицы
metadata-source = Источник
    .description = Следовать за играющим или выбранным либо читать медиатеку целиком
metadata-stripes-description = Подкрашивать каждую вторую строку таблицы

## History panel
history-column-last-played = Последнее прослушивание
history-descending = По убыванию
    .description = Развернуть сортировку
history-empty-never = Каждый трек уже прослушан
history-empty-recent = Пока нет прослушиваний
history-headings = Разбивать недавний список на серии по альбомам; Развёрнутые добавляют обложку и статистику
history-sort-browse = Порядок просмотра
history-sort-date-added = Дата добавления
history-sort-menu = Сортировка
    .description = В каком порядке идут ни разу не прослушанные треки
history-title = История
history-view-most = Самые слушаемые
history-view-never = Ни разу не слушанные
history-view-recent = Недавно прослушанные
history-view-recent-short = Недавние
history-view-row = Вид
    .description = Какой срез записи прослушиваний показывает панель

## Folder tree panel
folder-tree-clear-scope = Очистить область папки
folder-tree-collapse-all = Свернуть всё
folder-tree-collapse-branch = Свернуть ветку
folder-tree-cover-art = Обложка
    .description = Показывать обложку вместо значка строки, на папках или на песнях
folder-tree-cover-folders = Папки
folder-tree-cover-songs = Песни
folder-tree-empty = В медиатеке пока нет папок
folder-tree-expand-branch = Развернуть ветку
folder-tree-follow-description = Раскрывать и прокручивать к играющему треку при каждой его смене
folder-tree-nonmatch-folders = Неподходящие папки
    .description = Скрывать папки без совпадений или оставлять их притушенными
folder-tree-nonmatch-songs = Неподходящие песни
    .description = Внутри подходящей папки притушить посторонние песни или скрыть их
folder-tree-play-folder = Воспроизвести папку
folder-tree-play-songs = { $count ->
    [one] Воспроизвести { $count } песню
    [few] Воспроизвести { $count } песни
    [many] Воспроизвести { $count } песен
   *[other] Воспроизвести { $count } песни
}
folder-tree-resume-description = Прокручивать обратно к играющему треку, когда вы перестали листать
folder-tree-scope-to-folder = Сузить фильтр до папки
folder-tree-smooth-description = Плавно доезжать до трека, а не прыгать
folder-tree-title = Дерево

## Art panel
art-always = Держать обложки притушенными, даже когда ничего не играет; в полную силу видна только обложка под курсором
art-convert = Преобразовать...
art-covers-section = Обложки
matcher-section-matches = Совпадения
art-desaturate = Сводить все обложки, кроме играющего альбома, в оттенки серого; наведение возвращает обложке цвет
art-dim-while-playing = Гасить все обложки, кроме играющего альбома; наведение снова зажигает обложку
art-disc-style = Стиль диска
    .description = Оформить каждую обложку как компакт-диск или как этикетку винила
art-edit-tags = Править теги...
art-fill-panel = Заполнять панель
    .description = Считать размер центральной обложки только по высоте панели, а в вертикальном режиме по ширине; боковые обложки при этом уходят за край, а не ужимают её
art-follow-description = Ставить играющий альбом в центр при каждой смене трека
art-glow = Свечение
    .description = Собрать акцентный цвет за центральной обложкой; при окраске по обложке берётся цвет играющего альбома
art-label-position = Положение подписи
    .description = Где стоит подпись альбома: сверху, под обложкой, у нижнего края или скрыта
art-letter-rail = Алфавитная полоса
    .description = Инициалы исполнителей вдоль края полки; клик переходит к первому альбому на эту букву
art-layout-section = Макет
art-perspective = Перспектива
    .description = Разворачивать боковые обложки в настоящем 3D вместо плоского сжатия
art-reflections = Отражения
    .description = Отражать каждую обложку в полу под полкой
art-resume-description = Снова ставить играющий альбом в центр, когда вы перестали листать
art-shadows = Тени
    .description = Мягкая тень под каждой обложкой
art-smooth-description = Плавно доезжать до альбома, а не прыгать
art-title = Карусель альбомов
art-vertical-layout = Вертикальный макет
    .description = Сложить полку в столбец, который прокручивается вверх и вниз, а не в строку

## Playlists panel
playlists-columns = Какие столбцы трека показывать рядом с названием
playlists-delete = Удалить плейлист
playlists-edit-query = Править запрос...
playlists-empty = Пока нет плейлистов, добавьте треки или воспользуйтесь пунктом «Новый плейлист»
playlists-headings = Разбивать треки каждого плейлиста на серии по альбомам; Развёрнутые добавляют обложку и статистику
playlists-import-tooltip = Импортировать плейлист
playlists-imported-fallback = Импортированный
playlists-new = Новый плейлист...
playlists-new-smart = Новый умный плейлист...
playlists-refuse-drag-out = Треки из умного плейлиста нельзя вытащить перетаскиванием
playlists-refuse-edit-query = Правьте запрос, чтобы изменить состав умного плейлиста
playlists-refuse-smart-source = Умный плейлист берёт свои треки из запроса
playlists-remove = { $count ->
    [one] Убрать { $count } трек из плейлиста
    [few] Убрать { $count } трека из плейлиста
    [many] Убрать { $count } треков из плейлиста
   *[other] Убрать { $count } трека из плейлиста
}
playlists-rename = Переименовать...
playlists-title = Плейлисты

## Queue panel
queue-clear = Очистить очередь
queue-empty = Очередь пуста
queue-headings = Разбивать очередь на серии по альбомам; Развёрнутые добавляют обложку и статистику
queue-play-now = Воспроизвести сейчас
queue-remove = { $count ->
    [one] Убрать { $count } трек из очереди
    [few] Убрать { $count } трека из очереди
    [many] Убрать { $count } треков из очереди
   *[other] Убрать { $count } трека из очереди
}
queue-title = Очередь
queue-widget-always-modal = Всегда открывать модальным окном
    .description = Каждый раз открывать очередь модальным окном, а не переходить к уже открытой панели очереди
queue-widget-clear-queue = Очистить очередь
queue-widget-more = +ещё { $count }
queue-widget-open-on-click = Открывать очередь по клику
    .description = Клик по виджету переходит к открытой панели очереди или открывает очередь в окне, если такой панели нет
queue-widget-section-click = Клик
queue-widget-title = Виджет очереди
queue-widget-up-next = Далее

## Biography panel
biography-background = Фон
    .description = Фанарт исполнителя за текстом, притушенный и растворяющийся книзу
biography-fill-width = На всю ширину
    .description = Дать высокой шапке занять всю ширину, а не держать её ограниченной по ширине и по центру
biography-from-lastfm = С Last.fm
biography-header-image = Изображение шапки
    .description = Широкий баннер исполнителя сверху или портрет, когда баннера нет
biography-keep-aspect = Сохранять пропорции
    .description = Показывать шапку в её собственных пропорциях, а не обрезать под полосу
biography-listeners-count = слушателей: { $count }
biography-looking-up = Поиск: { $name }
biography-no-artist-tag = Нет тега исполнителя
biography-no-text = Биографии нет
biography-not-found = Для { $name } ничего не найдено
biography-plays-count = прослушиваний: { $count }
biography-refresh = Обновить
biography-similar-artists = Похожие исполнители
    .description = Похожие исполнители по данным прослушиваний, внизу
biography-similar-heading = Похожие исполнители
biography-stats = Статистика
    .description = Слушатели и прослушивания на Last.fm, под именем
biography-tags = Теги
    .description = Жанровые теги строкой плашек
biography-title = Биография

## Status panel
status-count-albums = { $count ->
    [one] { $count } альбом
    [few] { $count } альбома
    [many] { $count } альбомов
   *[other] { $count } альбома
}
status-count-artists = { $count ->
    [one] { $count } исполнитель
    [few] { $count } исполнителя
    [many] { $count } исполнителей
   *[other] { $count } исполнителя
}
status-count-plays = { $count ->
    [one] { $count } прослушивание
    [few] { $count } прослушивания
    [many] { $count } прослушиваний
   *[other] { $count } прослушивания
}
status-count-selected = выбрано: { $count }
status-count-tracks = { $count ->
    [one] { $count } трек
    [few] { $count } трека
    [many] { $count } треков
   *[other] { $count } трека
}
status-readouts = Показания
    .description = Тяните вдоль полосы, чтобы менять порядок; тяните между строками либо жмите x и плюс на плашке, чтобы скрывать и показывать
status-scope-selection = Выбор
status-title = Статус

## Output panel
output-detail-badge = Значок
output-detail-compact = Компактно
output-detail-expanded = Развёрнуто
output-detail-label = Детализация
    .description = Значок сводит всё к одной плашке, остальное показывает при наведении; компактный даёт заголовку отдельную строку, для полосы вдоль края; развёрнутый добавляет причины рядом, а в узкой панели под ним
output-device-name = Имя устройства
    .description = Называть работающее устройство в заголовке; если выключить, в строке останутся режим, частота и формат
output-file-rate = Частота файла
    .description = Подтверждать собственную частоту играющего файла, когда её ничто не преобразует. Про преобразование сказано в любом случае, ведь предупреждение именно о нём
output-mode-exclusive = Монопольный
output-mode-shared = Общий
output-no-output = Нет вывода
output-nothing-playing = Ничего не играет
output-pick-another-device = Выберите другое устройство или выключите монопольный режим
output-headline-numbers = { $rate } Гц, { $channels } кан., { $format }
output-headline = { $mode }, { output-headline-numbers }
output-headline-device = { $mode } на { $device }, { output-headline-numbers }
output-fell-back-to-shared = Монопольный откатился к общему: { $why }
output-replaygain-levelling = ReplayGain выравнивает этот файл на { $db } дБ
output-replaygain-short = ReplayGain { $db } дБ
output-rate-resampled = Играющий файл на { $rate } Гц, преобразован для устройства
output-rate-resampled-short = Файл { $rate } Гц преобразован
output-rate-native = Играющий файл на { $rate } Гц, поэтому ничего не преобразуется
output-rate-native-short = Файл { $rate } Гц, без преобразования
output-start-track-hint = Запустите трек, чтобы увидеть формат, на который согласилось устройство
output-title = Вывод

## Track columns
columns-bits = Биты
columns-bpm = BPM
columns-codec = Кодек
columns-cover = Обложка
columns-fav = Изб.
columns-gain = Усиление
columns-kbps = Кбит/с
columns-khz = кГц
columns-name = Название
columns-number = Номер
columns-scanned = Просканировано
columns-similar = Похожесть

## Filter panel
filter-add-column = Добавить столбец
filter-add-column-tooltip = Добавить столбец
filter-all = Все
filter-clear-filters = Очистить фильтры
filter-clear-selection = Снять выбор
filter-empty = Выберите поле, чтобы начать фильтровать
filter-remove-column = Удалить столбец

## Search panel
search-chips-below = Снизу
search-chips-inline = В строке
search-filter-chips = Плашки фильтров
search-placeholder = Поиск по медиатеке

## Playback panel
playback-buttons = Кнопки
    .description = Тяните вдоль полосы, чтобы менять порядок; тяните между строками либо жмите x и плюс на плашке, чтобы скрывать и показывать
playback-continue-down-list = Продолжать играть дальше по списку
playback-continue-off = Продолжение выключено
playback-continue-weighted = Продолжать играть, сначала ни разу не слушанное
playback-crossfade-inside-albums = Внутри альбомов
playback-crossfade-off = Кроссфейд выключен
playback-crossfade-tip = Кроссфейд { $length }
playback-highlight-circle = Круг
playback-highlight-square = Квадрат
playback-hold-draw = { $tip }. Удерживайте, чтобы выбрать подсветку
playback-hold-length = { $tip }. Удерживайте, чтобы выбрать длительность
playback-hold-order = { $tip }. Удерживайте, чтобы выбрать порядок
playback-loop-off = Повтор выключен
playback-loop-queue = Повторять очередь
playback-loop-track = Повторять этот трек
playback-menu-continue = Кнопка продолжения
playback-menu-crossfade = Кнопка кроссфейда
playback-menu-favourite = Кнопка избранного
playback-menu-random = Кнопка случайного
playback-menu-rating = Звёзды оценки
playback-menu-stop = Кнопка «Стоп»
playback-menu-stop-after = Кнопка «Стоп после»
playback-menu-volume = Кнопка громкости
playback-pause = Пауза
playback-play-highlight = Подсветка воспроизведения
    .description = Акцентная заливка кнопки воспроизведения: круг, мягкий квадрат или ничего
playback-random-tip-random = Воспроизвести случайный трек
playback-random-tip-similar = Воспроизвести трек, похожий на этот
playback-seek-back-tip = Назад на 10 секунд
playback-seek-forward-tip = Вперёд на 10 секунд
playback-shuffle-off = Перемешивание выключено
playback-shuffle-on = Перемешивание включено, порядок: { $order }
playback-stop-after-armed = Стоп после этого трека, взведено
playback-stop-after-tip = Стоп после этого трека
playback-stop-tip = Остановить и выгрузить трек
playback-volume-tip-muted = Включить звук, { $percent }%. Правый клик даёт ползунок
playback-volume-tip-unmuted = Выключить звук, { $percent }%. Правый клик даёт ползунок

## Track info panel
track-info-color-output-chip = Цветная плашка вывода
    .description = Разрешить плашке окрашиваться в предупреждающие цвета, когда вывод откатывается или пересчитывает частоту. Если выключить, она всегда останется в одном приглушённом тоне, а подсказка при наведении всё равно объяснит состояние
track-info-cycle-every = Менять каждые
    .description = Сколько каждая строка держится до затухания
track-info-cycle-rows = Чередовать строки
    .description = Показывать строки расстановки по одной в единственной строке, растворяя их друг в друге; одна строка читается сама по себе
track-info-delay = Задержка
    .description = Сколько строка стоит на каждом краю, прежде чем двинуться дальше
track-info-marquee = Бегущая строка
    .description = Что делает строка, которая не влезает в панель: ползёт и возвращается или идёт по кругу без конца
track-info-menu-overflow = Переполнение
track-info-next = Далее: { $line }
track-info-opening = открываем...
track-info-output-fallback = Устройство отказало в монопольном выводе, поэтому воспроизведение идёт через общий микшер. Устройство сообщило: { $reason }
track-info-output-resample-exclusive = Этот файл на { $source } кГц, а карта взяла { $device } кГц, так что каждый сэмпл преобразуется по пути наружу. Устройство отказалось работать на собственной частоте файла.
track-info-output-resample-mixer = Этот файл на { $source } кГц, а микшер работает на { $device } кГц, так что каждый сэмпл преобразуется по пути наружу. Монопольный режим отдал бы карте собственную частоту файла.
track-info-overflow-loop = По кругу
track-info-overflow-scroll = Прокрутка
track-info-overflow-truncate = Обрезать
track-info-queued-count = в очереди: { $count }
track-info-row-size = Размер строки { $number }
track-info-speed = Скорость
    .description = Как быстро ползёт строка
track-info-text-size = Размер текста

## Seek panel
seek-ending = Остаток
    .description = Отсчитывать оставшееся время или показывать полную длину
seek-ending-remaining = Осталось
seek-ending-total = Всего
seek-playhead = Указатель
    .description = Растянуть на всю высоту полосы или прижать к линии
seek-playhead-full = На всю высоту
seek-playhead-line = По линии
seek-playhead-max-height = Макс. высота указателя
    .description = Ограничить полный указатель, по центру относительно линии; 0 заполняет панель
seek-playhead-width = Ширина указателя
    .description = Ширина движущейся метки позиции
seek-rounding = Скругление
    .description = Радиус углов линии, вплоть до пилюли при половине толщины
seek-scrobble-marker = Метка скробблинга
    .description = Тонкая линия там, где трек засчитывается как отскробленный на Last.fm
seek-show-timings = Показывать время
seek-thickness = Толщина
    .description = Высота линии трека

## Volume panel
volume-pieces = Части
    .description = Тяните вдоль полосы, чтобы менять порядок; тяните между строками либо жмите x и плюс на плашке, чтобы скрывать и показывать. Если проценты скрыты, их показывает подсказка динамика
volume-readout = Показания
    .description = Показывать уровень в процентах или в децибелах усиления, которое он даёт
volume-readout-decibels = Децибелы
volume-readout-percent = Проценты
volume-stretch = Растягивать
    .description = Дать ползунку заполнить панель вместо ограничения по ширине
volume-tip-mute = Выключить звук
volume-tip-mute-level = Выключить звук, { $level }
volume-tip-unmute = Включить звук
volume-tip-unmute-level = Включить звук, { $level }

## Shared panel content
content-filter = Фильтр
content-no-track = Нет трека
content-total-genres = Жанры
content-total-time = Общее время

## Shared panel chrome
panel-columns-description = Какие столбцы трека показывать
panel-headings = Заголовки
panel-jump-to-playing = Перейти к играющему
panel-menu-display = Отображение
panel-title-artists = Исполнители
panel-title-genres = Жанры
panel-title-oscilloscope = Осциллограф
panel-title-particles = Частицы
panel-title-playback = Воспроизведение
panel-title-seek = Перемотка
panel-title-shader = Шейдер
panel-title-spectrogram = Спектрограмма
panel-title-spectrum = Спектр
panel-title-theme-toggle = Переключатель темы
panel-title-track-info = Информация о треке
panel-title-volume = Громкость
panel-title-vu = VU-метр
panel-title-waveform = Волновая форма

## Everything else
choice-both = Оба
choice-dim = Затемнить
choice-hide = Скрыть
composite-add-panel = Добавить панель
composite-host-settings = Настройки: { $host }
composite-move-left = Сдвинуть влево
composite-move-right = Сдвинуть вправо
composite-remove = Удалить
composite-replace = Заменить
group-panel-add-slot = Добавить слот
group-panel-move-down = Сдвинуть вниз
group-panel-move-up = Сдвинуть вверх
group-panel-remove-slot = Удалить слот
group-panel-split-side-by-side = Разделить бок о бок
group-panel-split-stacked = Разделить друг над другом
group-panel-swap-panels = Поменять панели местами
group-panel-title = Группа
overlay-dim = Затемнение
    .description = Насколько сильно тускнеет основная панель под раскрытым наложением
overlay-title = Наложение
overlay-toggle = Переключить наложение
shader-confirm-hint-after = переключает шейдер откуда угодно.
shader-confirm-hint-before = Шейдер может сделать окна неудобными. Откатите или закройте это окно, чтобы вернуть всё как было.
shader-confirm-keep = Оставить
shader-confirm-question = Оставить этот экранный шейдер?
shader-confirm-revert = Откатить
shader-confirm-window-title = rox - Шейдер наложения
slide-add = Добавить слайд
slide-next = Следующий слайд
slide-previous = Предыдущий слайд
slide-title = Слайд
theme-toggle-to-dark = Переключить на тёмную тему
theme-toggle-to-light = Переключить на светлую тему
transport-favourite-add = Добавить в избранное
transport-favourite-nothing = Нечего добавить в избранное
transport-favourite-remove = Убрать из избранного
transport-pieces = Части
    .description = Тяните вдоль строки, чтобы менять порядок, и между строками, чтобы переносить; x и плюс на плашке скрывают и показывают

## Stragglers picked up in the final sweep
duplicates-scanning = Сканирование...
about-copyright = Copyright © 2026
signal-name-placeholder = Название сигнала
signals-empty = Пока нет сигналов. Добавьте один или щёлкните правой кнопкой по любому привязываемому регулятору.
signal-add = Добавить сигнал
panel-approve = Подтвердить
panel-turn-off = Выключить
shader-from-file = Из файла...
arrange-add-row = Добавить строку
smart-playlist-name-placeholder = Название плейлиста
smart-playlist-name-to-save = Назовите плейлист, чтобы сохранить его
panel-new-playlist = Новый плейлист...
panel-edit-tags = Править теги...
panel-edit-cover = Править обложку...
panel-rename-files = Переименовать файлы...
panel-convert = Преобразовать...
panel-catalog-drag-anchor = Область перетаскивания
panel-catalog-spacer = Распорка

## Duration and worker phrasing
pace-under-a-minute = меньше минуты
pace-minutes = { $count ->
    [one] около { $count } минуты
    [few] около { $count } минут
    [many] около { $count } минут
   *[other] около { $count } минут
}
pace-hours = { $count ->
    [one] около { $count } часа
    [few] около { $count } часов
    [many] около { $count } часов
   *[other] около { $count } часов
}
pace-half-hours = около { $value } часа
pace-days = { $count ->
    [one] около { $count } дня
    [few] около { $count } дней
    [many] около { $count } дней
   *[other] около { $count } дней
}
pace-workers = { $count ->
    [one] { $count } поток
    [few] { $count } потока
    [many] { $count } потоков
   *[other] { $count } потока
}
tasks-rest-takes = , остальное займёт { $estimate }
tasks-measuring-takes = , их измерение займёт { $estimate }
tasks-working-out-takes = , их расчёт займёт { $estimate }
tasks-time-left = , осталось { $left }
tasks-failed-suffix = (ошибок: { $count })
tasks-file-suffix = - { $file }
tasks-no-beat-suffix = (без чёткого ритма: { $count })
tasks-estimate-at-workers = ({ tasks-estimate-at })

## Panel vanity names
panel-title-art-view = Просмотр обложек
panel-title-artist-grid = Сетка исполнителей
panel-title-genre-grid = Сетка жанров
panel-title-biography = Биография
panel-title-cover-art = Обложка
panel-title-drag-anchor = Область перетаскивания
panel-title-drawer = Выдвижная панель
panel-title-eq-widget = Виджет эквалайзера
panel-title-filter = Фильтр
panel-title-folder-tree = Дерево папок
panel-title-group = Группа
panel-title-history = История
panel-title-lyrics = Текст песни
panel-title-menu = Меню
panel-title-metadata = Метаданные
panel-title-mini-toggle = Переключатель мини
panel-title-output = Вывод
panel-title-overlay = Наложение
panel-title-playlists = Плейлисты
panel-title-queue = Очередь
panel-title-queue-widget = Виджет очереди
panel-title-search = Поиск
panel-title-slide = Слайд
panel-title-spacer = Распорка
panel-title-stats-widget = Виджет статистики
panel-title-vu-meter = VU-метр
panel-title-window-controls = Кнопки окна

## Relative time and the output headline
ago-just-now = только что
ago-minutes = { $count } мин назад
ago-hours = { $count } ч назад
ago-days = { $count } д назад
ago-weeks = { $count } нед назад
ago-years = { $count } г назад

## Long spans spelled out, for the library totals. The short clocks stop
## meaning much past a day, so these carry the noun with them.
span-seconds = { $count ->
    [one] { $count } секунда
    [few] { $count } секунды
    [many] { $count } секунд
   *[other] { $count } секунды
}
span-minutes = { $count ->
    [one] { $count } минута
    [few] { $count } минуты
    [many] { $count } минут
   *[other] { $count } минуты
}
span-hours = { $count ->
    [one] { $count } час
    [few] { $count } часа
    [many] { $count } часов
   *[other] { $count } часа
}
span-days = { $count ->
    [one] { $count } день
    [few] { $count } дня
    [many] { $count } дней
   *[other] { $count } дня
}
span-weeks = { $count ->
    [one] { $count } неделя
    [few] { $count } недели
    [many] { $count } недель
   *[other] { $count } недели
}
span-years = { $count ->
    [one] { $count } год
    [few] { $count } года
    [many] { $count } лет
   *[other] { $count } года
}

## How a span joins its second unit: "3 weeks, 2 days".
span-pair = { $first }, { $second }

## A percentage. The space before the sign is a locale question, not a
## notation one, so each locale spells the whole thing out.
unit-percent = { $value }%

settings-audio-output-headline = { $mode }{ $note } на { $device }, { $rate } Гц, каналов: { $channels }, { $format }
settings-audio-output-experimental =  (экспериментально)

## ML model catalog
settings-mlmodels-description = { $summary }. { $dim } значений на трек. { $licence }
settings-mlmodels-on-disk = , { $size } на диске
settings-mlmodels-to-download = , { $size } к загрузке
model-summary-dsp-timbre-1 = Встроено, скачивать нечего. Сводка по энергии логарифмических полос, спектральной форме и частоте атак каждого трека. Грубо на фоне обученной сети, но ей ничего не нужно и она работает везде
model-summary-panns-cnn10 = Свёрточная сеть, обученная на AudioSet распознавать, что за звук перед ней. Её описание трека из 512 значений намного богаче встроенного наброска, ценой загрузки в 24 МБ и более медленного прохода анализа

## Shipped workspaces
workspace-shipped-default = (По умолчанию)
workspace-shipped-default-blurb = Как rox выглядит из коробки: полупрозрачные поверхности над рабочим столом, без рамки окна, окраска по обложке выключена. Точка отсчёта, от которой отходит каждый другой внешний вид здесь.
workspace-shipped-catrox-blurb = Скин foobar2000, с которого всё началось, собранный заново: круглая отрисовка обложки как компакт-диска, поля метаданных слева и сгруппированные по альбомам треки с точками оценки.
workspace-shipped-critters-blurb = Всё приложение как однобитная печать: упорядоченный дизеринг по каждой поверхности, тона, которые схлопываются с сабом, и стена шума, которая извивается вместе с песней. По мотивам Critters for Sale.
workspace-shipped-diffuse-blurb = Только играющий альбом: обложка и карточка воспроизведения одной группой на всё окно, прозрачные поверхности над подложкой, без швов. Медиатека, очередь и текст песни ждут в ящике у правого края и выезжают поверх музыки, когда на ручку наводят курсор. Монохром, поэтому цвет идёт от обложек.
workspace-shipped-foobar-blurb = Тот самый макет, с которым спорит весь этот проект. Непрозрачные панели, столбцы фильтров по исполнителю и альбому, плотная таблица треков и строка меню ровно там, где она всегда была.
workspace-shipped-llama-winamp-blurb = Winamp таким, каким вы его помните, а не каким он был. Tahoma, тёмный, без рамки, точечный спектр по верху и режим свёртки в мини-макете.
workspace-shipped-metro-blurb = Плоские панели и просторные строки в Segoe UI, с включённой окраской по обложке, так что вся палитра следует за играющей обложкой.
workspace-shipped-phosphor-blurb = Всё моноширинным. Consolas, зелёное на чёрном, без обложки в быстром запуске: терминал, который по случайности играет музыку.
