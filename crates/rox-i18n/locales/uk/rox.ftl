### Українська. Дзеркалить en-CA/rox.ftl ключ у ключ; тест на паритет
### у rox-i18n за цим стежить. Ключі - kebab-case із префіксом поверхні;
### опис рядка задається атрибутом повідомлення-мітки.

## Shared widgets
tracking-title = Стеження
tracking-follow = Стежити за відтворенням
tracking-resume = Повертатися при простої
tracking-smooth = Плавне прокручування
align-row = Вирівнювання
    .description = Де розміщується вміст, коли в панелі є запас місця
valign-row = Вертикальне вирівнювання
    .description = Де розміщується вміст, коли в панелі є запас висоти
valign-top = Згори
valign-middle = По центру
valign-bottom = Знизу
letter-rail-compact = Компактна смуга
    .description = Обмежити смугу одним рядком із прокручуванням замість перенесення
letter-rail-side = Положення смуги
    .description = На якому боці стіни розташована смуга

## Panel source and search rows
source-track = Трек
    .description = Стежити за тим, що грає, або за вибраним у медіатеці
source-follow-playing = Стежити за відтворенням
source-follow-selection = Стежити за вибором
source-playing = Грає
source-selected = Вибрано
query-search = Пошук
query-search-box = Поле пошуку
    .description = Показувати поле пошуку; запит діє, лише поки воно на екрані
query-source = Джерело пошуку
    .description = Стежити за спільним пошуковим запитом, фільтрувати за власним полем цієї панелі або показувати те, що вибрано в іншій панелі
query-source-shared = Спільний
query-source-own = Власний
query-source-selection = Вибір

## Signals and routes
signal-source = Джерело
    .description = За чим стежить сигнал: Смуга - за одним діапазоном частот, Рівень - за всім міксом, Атака пульсує на кожному ударі в діапазоні, Тригер видає імпульс, коли діапазон досягає свого порога, Сума накопичує інший сигнал із часом
signal-kind-band = Смуга
signal-kind-level = Рівень
signal-kind-onset = Атака
signal-kind-trigger = Тригер
signal-kind-total = Сума
signal-response = Відгук
signal-response-pulse = Як довго дзвенить кожен імпульс, перш ніж згаснути
signal-response-drift = 0 тримається музики впритул, 100 тягнеться за нею
signal-threshold = Поріг
signal-threshold-trigger = Рівень, якого має досягти діапазон, щоб видати імпульс; знову він спрацює лише після того, як рівень опуститься під позначку на індикаторі вище
signal-threshold-gate = Нижче за це сигнал читається як нуль, а вище вихід знову росте від нуля, тож тихі місця не рухають регулятор. Позначка на індикаторі вище показує, де саме проходить поріг
signal-low-bound = Нижня межа
signal-high-bound = Верхня межа
signal-adds-up = Що підсумовується
    .description = Який сигнал тут накопичується; сума росте, поки той читається високо, і завмирає, поки він тихий
signal-aggregate-nothing = Немає за чим стежити
signal-aggregate-pick = Вибрати сигнал
signal-aggregate-alone = У пулі немає іншого сигналу, щоб його підсумувати, тож тут нуль. Додайте сигнал, і він з'явиться в списку.
signal-aggregate-unpicked = Нічого не вибрано, тож ця сума стоїть на нулі. Виберіть сигнал вище.
signal-rate = Швидкість
    .description = Обертів на секунду при повному вході; після 1 значення скидається в 0 і росте далі, а шейдер читає це як фазу
signal-reset-on-track = Скидати на новому треку
    .description = Повертатися до нуля, коли починається нова пісня, щоб фаза не починалася з суми попередньої
signal-flush = Обнулити
signal-routes-in-panel = { $count ->
    [one] { $count } маршрут у цій панелі
    [few] { $count } маршрути в цій панелі
    [many] { $count } маршрутів у цій панелі
   *[other] { $count } маршруту в цій панелі
}
    .description = Повернути до нуля просто зараз. Значення стікає за мить, а не обривається, тож ніщо з того, що за ним стежить, не смикається
route-header = Маршрут
route-signal = Сигнал
    .description = За яким спільним сигналом іде цей маршрут; налаштування сигналу тут налаштовує кожен маршрут на ньому
route-new-signal = Новий сигнал
route-shared-note = Спільне для кожного маршруту на цьому сигналі
route-signal-gone = Сигнал цього маршруту зник; регулятор тримає значення свого повзунка, поки вище не вибрано інший.
route-range-note = Діапазон лише для цього параметра
route-quiet = Тиша
    .description = Що регулятор читає в тиші, як частка його власного налаштування
route-loud = Гучно
    .description = Що він читає при повному сигналі; 100% - це власне значення повзунка, нижче за Тишу модулює вниз
route-slot = Слот
    .description = Який із шістнадцяти сигнальних слотів шейдера заповнює цей маршрут
route-slot-quiet-description = Що слот читає в тиші
route-slot-loud-description = Що він читає при повному сигналі; нижче за Тишу пускає слот назад
route-slot-signal-description = За яким спільним сигналом іде цей маршрут
route-slot-signal-gone = Сигнал цього маршруту зник; слот читає нуль, поки не вибрано інший.
route-add = Додати маршрут
route-unrouted = Без маршруту
route-pick-slot = Вибрати слот
route-pick-signal = Вибрати сигнал
route-no-signal = немає сигналу
route-no-signals-yet = Ще немає сигналів, за якими можна стежити. Створіть сигнал, і він з'явиться тут; доти слот читає нуль.
route-open-signals = Відкрити сигнали
route-create-signal = Створити новий сигнал

## Panel settings window
panel-settings = Налаштування панелі
panel-menu-label = Панель
panel-save-as-preset = Зберегти як пресет
panel-rename = Перейменувати
panel-rename-name = Назва
panel-rename-note = Показується як вкладка панелі; порожнє повертає вбудовану назву
panel-rename-hint-after = щоб перейменувати
panel-was-closed = Панель було закрито
panel-reset = Скинути
panel-inverse = Інверсія
panel-apply-song-theme = Застосувати тему пісні
panel-page-appearance = Оформлення
panel-page-behavior = Поведінка
panel-page-shader = Шейдер
panel-section-placement = Розміщення
panel-section-size = Розмір
panel-section-opacity = Непрозорість
panel-section-frame = Рамка
panel-section-colors = Кольори
panel-section-font = Шрифт
panel-section-shader = Шейдер
panel-section-signals = Сигнали
panel-section-slots = Слоти
panel-awaiting-approval = Очікує підтвердження
panel-size-off = Вимк.
panel-locked = Закріплено
    .description = Зафіксувати панель на місці; у доку її не перетягнути й не переставити
panel-drag-anchor = Якір перетягування
    .description = Перетягування будь-де по панелі рухає вікно, а звичайні кліки й далі потрапляють на її елементи; для розкладок без рамок вікна
panel-slot-controls = Кнопки слотів
    .description = Показувати кутові кнопки для заміни й прибирання панелей, які ця панель тримає. Приховані, розкладка все одно редагується з дерева на сторінці Робочий простір у Налаштуваннях
panel-min-width = Мін. ширина
    .description = Де зміна розміру перестає стискати панель вужче. Береться як написано, зокрема й нижче за власну межу панелі, тож компактна смуга може стати тіснішою за стандарт; порожнє лишає межу як є
panel-max-width = Макс. ширина
    .description = Обмежити ширину панелі, щоб вона не розтягувалася, коли вікно ширшає
panel-min-height = Мін. висота
    .description = Де зміна розміру перестає стискати панель нижче. Береться як написано, зокрема й нижче за власну межу панелі, тож компактна смуга може стати тіснішою за стандарт; порожнє лишає межу як є
panel-max-height = Макс. висота
    .description = Обмежити висоту панелі, щоб вона не розтягувалася, коли вікно вищає
panel-own-opacity = Власна непрозорість поверхні
    .description = Дати цій панелі власну непрозорість над тлом замість застосункової
panel-surface-opacity = Непрозорість поверхні
panel-margin = Зовнішній відступ
    .description = Підтягнути панель усередину її комірки, щоб у проміжку просвічувало тло
panel-padding = Внутрішній відступ
    .description = Місце всередині краю панелі, лишається в її власному тлі
panel-rounding = Заокруглення
    .description = Заокруглити кути панелі в тло
panel-border = Рамка
    .description = Лінія по краю панелі, кольором ролі Рамка; сторона на нулі не малюється
panel-font = Шрифт
    .description = Гарнітура панелі; типова йде за шрифтом застосунку
panel-font-size = Розмір шрифту
    .description = Розмір тексту панелі відносно шрифту застосунку; рядки масштабуються разом із ним
panel-surface-shader = Шейдер поверхні
    .description = Пустити шейдер WGSL по тілу цієї панелі, під екранним шейдером застосунку
panel-run-when-idle = Працювати в простої
    .description = Малювати кадри й далі, поки звук мовчить. Вимкнено, шейдер завмирає на останньому кадрі, і панель нічого не коштує
panel-shader-is-scene = Цей шейдер - сцена, тож він накриває тіло панелі, а не малює поверх нього. Він прийшов із набору або зі старішого конфігу; список вище пропонує лише шейдери, які лишають панель читабельною.

## Shader picker and saving
shader-source = Джерело
shader-pick-none = Немає
shader-reload = Перезавантажити
shader-edit-as-file = Редагувати як файл
shader-make-private-copy = Зробити власну копію
shader-save-replace = Замінити
shader-save-to-workspace = Зберегти в робочий простір
shader-save-replaces = Замінює шейдер, який цей робочий простір уже зве { $name }. Кожна панель, що бере цю назву, зміниться разом із ним
shader-save-adds = Додає його до шейдерів цього робочого простору під назвою { $name }. Його може взяти будь-яка панель, і правка оновить їх усі
shader-group-examples = Приклади
shader-group-this-workspace = Цей робочий простір
shader-group-scenes = Сцени
shader-group-workspace-scenes = Сцени робочого простору
shader-group-overlays = Накладки
shader-group-workspace-overlays = Накладки робочого простору

## Saving a panel preset
preset-save = Зберегти пресет
preset-save-name = Назва пресету
preset-save-replaces = Замінює пресет, який цей робочий простір уже зве { $name }
preset-save-hint-after = щоб зберегти
preset-back-from = Поверніть його через
preset-back-add-panel = Додати панель
preset-back-then = потім
preset-back-presets = Пресети
preset-back-tail = у меню будь-якої панелі. Пресети належать лише цьому робочому простору; в іншому їх не буде.

## Keyboard hints
hint-press = Натисніть
hint-key-enter = Enter

## Settings: language
settings-language = Мова
    .description = Мова інтерфейсу. Системна звіряється зі списком ОС і відкочується до англійської, коли нічого не збіглося
    .keywords = переклад локаль мова інтерфейс
settings-language-system = (Системна мова)
settings-language-search = Пошук мов
picker-no-matches = Збігів немає
settings-search-no-matches = Немає збігів для «{ $text }»

## Embed dialog
bake-window-title = rox - Вписати збережені метадані
bake-title = Вписати збережені метадані
bake-intro = Записує збережені метадані в самі файли, щоб їх прочитав і інший програвач. Нічого не перераховується.
bake-formats = Лише MP3 і FLAC; інші формати та треки CUE пропускаються
bake-source-lyrics = Текст пісні
bake-source-gain = ReplayGain
bake-source-acoustic = Акустичні описи
bake-detail-nothing = немає нічого збереженого, щоб вписати
bake-detail-only-skipped = нічого записувати, пропущено: { $skipped }
bake-detail-writes = { $count ->
    [one] { $count } файл до запису
    [few] { $count } файли до запису
    [many] { $count } файлів до запису
   *[other] { $count } файлу до запису
}
bake-detail-writes-skipped = { $count ->
    [one] { $count } файл до запису, пропущено: { $skipped }
    [few] { $count } файли до запису, пропущено: { $skipped }
    [many] { $count } файлів до запису, пропущено: { $skipped }
   *[other] { $count } файлу до запису, пропущено: { $skipped }
}
bake-error-read = Не вдалося прочитати медіатеку: { $error }
bake-survey-counting = Переглядаємо медіатеку...
bake-survey-progress = Читаємо теги, { $done } з { $total }
bake-nothing-to-embed = Вписувати нічого: у файлах уже є все, що зберіг rox
bake-rewrites = { $count ->
    [one] Буде перезаписано { $count } файл
    [few] Буде перезаписано { $count } файли
    [many] Буде перезаписано { $count } файлів
   *[other] Буде перезаписано { $count } файлу
}
bake-hint-before = Натисніть
bake-hint-key = Enter
bake-hint-after = щоб вписати
bake-embed = Вписати
bake-cancel = Скасувати
## Звіт одним рядком після вписування. Показуються всі три числа, нулі теж,
## бо пропуски найбільше цікавлять опісля. Голова рядка збирається першою,
## а два хвости дописуються за нею, тож кожне число сидить у власному
## повідомленні й мова, яка навколо нього відмінюється, може вибирати
## саме за ним, не чіпаючи інших.
bake-summary-files = { $count ->
    [one] { $count } файл
    [few] { $count } файли
    [many] { $count } файлів
   *[other] { $count } файла
}
bake-summary-updated = Оновлено { $files }
bake-summary-stopped = Спинилося, оновлено { $files }
bake-summary-skipped = , пропущено { $count }
bake-summary-failed = , не вдалося { $count }

## Arrange editors and header pieces
arrange-shown = Показано
arrange-hidden = Приховано
tile-face-mosaic = Мозаїка обкладинок
tile-face-tinted = Тонована мозаїка
tile-face-gradient = Градієнтна картка
tile-face-color = Кольорова картка
head-piece-artist = Виконавець
head-piece-album = Альбом
head-piece-year = Рік
head-piece-genre = Жанр
head-piece-quality = Якість
head-piece-tracks = Треки
head-piece-time = Час
head-piece-spacer = Проміжок
head-piece-divider = Роздільник
head-piece-art = Обкладинка
head-unknown = Невідомо
status-item-count = Кількість
status-item-time = Час
status-item-albums = Альбоми
status-item-artists = Виконавці
status-item-plays = Прослуховування
volume-item-icon = Значок
volume-item-slider = Повзунок
volume-item-percent = Відсоток

## Filter chips and search menus
filter-field-artist = Виконавець
filter-field-album-artist = Виконавець альбому
filter-field-album = Альбом
filter-field-genre = Жанр
filter-field-year = Рік
filter-field-folder = Тека
filter-unknown = Невідомо
filter-clear = Очистити
query-show-search-box = Показати поле пошуку
query-own-query = Власний запит
query-shared-query = Спільний запит
headers-off = Вимк.
headers-compact = Компактні
headers-expanded = Розгорнуті

## Panel context menu
panel-dock-back = Повернути в док
panel-pop-out = Відділити
panel-close = Закрити
panel-duplicate = Дублювати
panel-reveal-in-browser = Показати у файловому менеджері
panel-play-next = Відтворити наступним
panel-add-to-queue = Додати в чергу
panel-add-to-playlist = Додати до списку відтворення
panel-favourite-add = Додати в улюблене
panel-favourite-remove = Прибрати з улюбленого
panel-copy = Копіювати
panel-copy-title = Копіювати назву
panel-copy-artist = Копіювати виконавця
panel-copy-album = Копіювати альбом
panel-copy-filename = Копіювати ім'я файлу
panel-copy-path = Копіювати шлях
shader-pick-missing = { $name } (немає)
shader-pick-custom = Власний

## Shipped shader examples
shader-blurb-plasma = Пливучий колір, зібраний лише з власних уніформ, тож коштує як звичайний квад.
shader-blurb-trails = Розмазує свій попередній кадр, тож іде на екранному проході.
shader-blurb-sheen = Віньєтка й пливучий полиск, прозора накладка для панелі, яка вже щось малює.
shader-blurb-shadow = Тінь, яку відкидають власний текст і елементи панелі, знята з маски.
shader-blurb-cover = Обкладинка треку, що грає, у леттербоксі поверх заливки її ж кольором.
shader-blurb-badge = Обкладинка як маленька картка, припаркована в кутку, зі слотом, щоб її пересувати.
shader-blurb-lamp = Світло, яке йде за курсором і відгукується на кліки, прозора накладка.
shader-blurb-cube = Каркасний куб, що перевертається в підробленому 3D, намальований доданим світлом.
shader-blurb-bloom = Пливучі кулі, розмиті другим проходом удвічі меншого розміру, увесь ланцюг у мініатюрі.
shader-blurb-tube = Переграє панель під собою крізь вигнутий екран ЕПТ, разом зі рядками розгортки.

## Transport strip pieces
seek-item-elapsed = Минуло
seek-item-strip = Смуга
seek-item-ending = Лишилось
seek-item-duration = Тривалість
info-item-track-no = Номер треку
info-item-title = Назва
info-item-duration = Тривалість
info-item-next = Далі
info-item-queued = У черзі
info-item-output = Вихід
info-item-favourite = Улюблене
info-item-rating = Оцінка
playback-item-previous = Попередній
playback-item-seek-back = Перемотати назад
playback-item-play = Відтворити
playback-item-seek-forward = Перемотати вперед
playback-item-next = Наступний
playback-item-stop = Зупинити
playback-item-volume = Гучність
playback-item-loop = Повтор
playback-item-shuffle = Перемішати
playback-item-continue = Продовження
playback-item-crossfade = Кросфейд
playback-item-random = Випадково
playback-item-stop-after = Зупинити після
playback-item-favourite = Улюблене
playback-item-rating = Оцінка

## Dock chrome
dock-empty-tab = Порожня вкладка
dock-unnamed = Без назви
dock-tiles = Плитки
dock-zoom-in = Наблизити
dock-zoom-out = Віддалити
dock-collapse = Згорнути
dock-expand = Розгорнути

## Shader picker notes
shader-note-empty = Виберіть приклад для початку або вкажіть rox файл .wgsl із фрагментною стадією, що визначає fs_user(uv)
shader-note-missing = { $name } більше немає серед шейдерів цього робочого простору, тож нічого не малюється. Виберіть тут щось інше, і ця панель дістане власне джерело.
shader-note-shared = Спільний для всього робочого простору. Правка оновить кожну поверхню, яка його бере.
shader-note-file = { $path }. Ваші збереження перезавантажуються, поки шейдер малює, а джерело зберігається всередині розкладок і наборів, тож він працює й на машині, де цього файлу ніколи не було.
shader-note-custom = Це джерело зберігається всередині своєї розкладки чи набору, файлу за ним немає. Редагувати як файл випише його назовні й підхопить ваші збереження.

## Panel pages and shared sides
panel-page-layout = Розкладка
panel-page-view = Вигляд
panel-page-content = Вміст
panel-page-source = Джерело
panel-page-bindings = Прив'язки
panel-page-emitters = Емітери
panel-page-forces = Сили
panel-page-scene = Сцена
side-left = Ліворуч
side-right = Праворуч
genre-face-mosaic = Мозаїка
genre-face-tinted = Тонована
genre-face-gradient = Градієнт
genre-face-color = Колір

## Library panel
panel-title-library = Медіатека
library-play = Відтворити
library-play-album = Відтворити альбом
library-play-group = Відтворити групу
library-play-tracks = Відтворити треки: { $count }
library-play-similar = Відтворити схоже
library-filter-by-album = Фільтрувати за альбомом
library-filter-by-artist = Фільтрувати за виконавцем
library-jump-to-playing = Перейти до того, що грає
library-menu-display = Показ
library-disc = Диск { $number }
library-empty-title = Відкрийте музичну теку
library-empty-note = Вона потрапить у медіатеку при скануванні (flac, mp3, wav)
library-headers = Заголовки
    .description = Розриви груп над списком; сортування тримає разом усі наявні послідовності, а пошук показує список рівним
library-group-by = Групувати за
    .description = За чим ламаються заголовки; жанр і рік пересортовують список
library-header-row = Рядок заголовка
    .description = Що показують однорядкові заголовки, зліва направо; проміжок або роздільник ділить боки
library-header-lines = Рядки заголовка
    .description = Рядки блока, згори вниз; порожній рядок випадає
library-follow-description = Прокручувати до рядка, що грає, щоразу коли змінюється трек
library-resume-description = Прокручувати назад до рядка, що грає, коли ви перестали гортати
library-smooth-description = Плавно ковзати до рядка замість стрибка
library-columns = Стовпці
    .description = Які стовпці показані; перетягніть заголовки в панелі, щоб змінити їхній порядок і ширину
library-column-headers = Заголовки стовпців
    .description = Рядок заголовків над списком, за якими сортують; приховайте його, і стовпці збережуть порядок та ширину
library-column-rename = Перейменувати...
library-column-rename-reset = Скинути назву
library-column-rename-name = Заголовок
library-column-rename-note = Показується замість вбудованого заголовка; порожнє поле повертає його, а один пробіл лишає заголовок порожнім
library-sort-on-click = Сортування кліком
    .description = Сортує кліком будь-де в заголовку, а не на його значку; щоб переставити стовпець, потрібні Alt і перетягування
library-compact-plays = Компактні прослуховування
    .description = Стовпець прослуховувань як маленьке число з рискою поруч
library-line-height = Висота рядка
    .description = Один рядок заголовка; блоки беруть стільки рядків, скільки треба, незалежно від рядків треків
library-text-size = Розмір тексту
    .description = Текст рядків заголовка, незалежно від висоти рядка, тож обкладинка росте сама
library-flush-background = Урівень із тлом
    .description = Показувати заголовки на тлі списку замість піднятого відтінку; тема пісні змінює їх разом
library-gap-above = Проміжок згори
    .description = Відрізаний від верху блока; крізь нього видно список, а рядки стискаються, щоб влізти
library-gap-below = Проміжок знизу
    .description = Те саме під блоком, перед його треками
library-section-rows = Рядки
library-row-height = Висота рядка
    .description = Рядки треків; текст іде за ними, і обидва масштабуються зі шрифтом застосунку
library-row-spacing = Інтервал рядків
    .description = Додаткова висота, яку добирає кожен рядок; вільніше, без збільшення тексту
library-stripes = Смуги через рядок
    .description = Тонувати кожен другий рядок треку, щоб довгий список читався
library-row-borders = Лінії рядків
    .description = Волосяна лінія під кожним рядком треку
library-art-description = Плитка розгорнутих заголовків: обкладинка, портрет виконавця або обличчя жанру
library-art-rounding = Заокруглення обкладинки
    .description = Заокруглити кути обкладинки
library-art-position = Розташування обкладинки
    .description = З якого боку блока розміщується плитка розгорнутих заголовків
library-art-margin = Відступ обкладинки
    .description = Втиснути плитку всередину блока; вона зменшується, щоб лишитися квадратом
library-circular-portraits = Круглі портрети
    .description = При групуванні за виконавцем заокруглювати плитки до повного кола стіни, а не за регулятором заокруглення
library-genre-face = Обличчя жанру
    .description = При групуванні за жанром - що показує плитка: обкладинки, обкладинки, залиті кольором жанру, або кольорову картку під його геометрією

## Album grid panel
panel-title-album-grid = Сітка альбомів
grid-menu-scroll = Прокручування
grid-menu-sort = Сортування
grid-sort-artist = Виконавець
grid-sort-album = Альбом
grid-sort-year = Рік
grid-sort-added = Нещодавно додані
grid-sort-plays = Найчастіше прослухані
grid-letter-rail = Алфавітна смуга
    .description = Ініціали вздовж краю стіни; клік переходить до першого альбому на цю літеру
grid-vertical-scroll = Вертикальне прокручування
grid-horizontal-scroll = Горизонтальне прокручування
grid-jump-to-playing = Перейти до того, що грає
grid-library-empty = Медіатека порожня
grid-play-albums = Відтворити альбоми: { $count }
grid-vertical-layout = Вертикальна розкладка
    .description = Гортати стіну вгору й вниз, рядами по ширині; вимкнено гортає її ліворуч і праворуч, стовпцями по висоті
grid-follow-description = Прокручувати до альбому, що грає, щоразу коли змінюється трек
grid-resume-description = Ковзати назад до альбому, що грає, коли ви перестали гортати
grid-smooth-description = Плавно ковзати до альбому замість стрибка
grid-section-dimming = Притлумлення
grid-section-tiles = Плитки
grid-dim-while-playing = Тьмяніти під час відтворення
    .description = Пригасити кожну обкладинку, крім альбому, що грає; наведення повертає плитці світло
grid-dim-amount = Сила притлумлення
    .description = Наскільки гаснуть інші обкладинки; 100% ховає їх
grid-desaturate = Знебарвлювати під час відтворення
    .description = Злити колір з кожної обкладинки, крім альбому, що грає; наведення повертає плитці колір
grid-always = Завжди
    .description = Тримати обкладинки притлумленими, навіть коли нічого не грає; на повну показується лише плитка під курсором
grid-show-titles = Показувати назви
    .description = Друкувати альбом і виконавця під кожною обкладинкою, як в iTunes, а не лише при наведенні
grid-title-alignment = Вирівнювання назв
    .description = Вирівняти підписи під їхніми обкладинками
grid-tile-size = Розмір плитки
    .description = Найдовший бік плиток з обкладинками; стовпці ділять ширину панелі порівну
grid-gap = Проміжок
    .description = Місце між обкладинками; нуль пакує їх впритул
grid-art-rounding-description = Заокруглити кути кожної обкладинки; 100% - це коло

## Settings: sidebar pages
settings-page-appearance = Оформлення
settings-page-application = Застосунок
settings-page-audio = Звук
settings-page-development = Розробка
settings-page-integrations = Інтеграції
settings-page-keymap = Клавіші
settings-page-library = Медіатека
settings-page-mcp = MCP
settings-page-ml-models = Моделі ML
settings-page-playback = Відтворення
settings-page-providers = Постачальники
settings-page-shader = Шейдер
settings-page-storage = Сховище
settings-page-workspace = Робочий простір

## Settings: appearance
settings-appearance-backdrop-all-windows = Усі вікна
    .description = Підкладати тло й під дочірні вікна: налаштування, редактори, діалоги, відділені панелі. Вимкнено лишає тло й прозорість вікнам робочого простору
settings-appearance-backdrop-strength = Сила тла
    .description = Наскільки сильно тло з обкладинки проступає за ними
settings-appearance-border = Рамка
    .description = Лінія по краю кожної панелі, кольором ролі Рамка; сторона на нулі не малюється
settings-appearance-colors-locked-note = Тема пісні увімкнена, тож ці кольори задає трек, що грає, і експорт зберігає саме їх. Вимкніть її вище, щоб редагувати
settings-appearance-design-mode = Режим дизайну
    .description = Правка розкладки прямо на місці: пункти меню панелі для додавання, перейменування, дублювання, відділення й закриття, елементи, які контейнер накладає на свої слоти, і перетягування вкладок. Вимкнено ховає все це; сторінка Робочий простір усе одно править дерево
    .keywords = правка розкладка перестановка блокування
settings-appearance-font = Шрифт
    .description = Гарнітура для всього застосунку; панелі можуть перекрити її у власних налаштуваннях
    .keywords = гарнітура шрифт текст
settings-appearance-font-size = Розмір шрифту
    .description = Базовий розмір тексту, від якого масштабується текст кожної панелі; елементи й значки тримають свій розмір
settings-appearance-hide-menubar = Ховати смугу меню
    .description = Тримати смугу меню прихованою, показуючи її над доком, поки затиснуто alt. Подвійне натискання alt лишає її на екрані, тож її кнопки беруть звичайний клік
settings-appearance-icons-intro = Набір - це тека зі SVG, яка замінює вбудовані значки; перемикання спрацює при наступному запуску
settings-appearance-icons-open-folder = Відкрити теку
settings-appearance-inverse-from-dark = Інверсія з темної теми
settings-appearance-inverse-from-light = Інверсія зі світлої теми
settings-appearance-keep-theme = Тримати тему
    .description = Тримати активну тему, навіть коли яскравість обкладинки її перекинула б; тема пісні все одно тонує колір
settings-appearance-margin = Зовнішній відступ
    .description = Підтягнути кожну панель усередину її комірки; панель може перекрити це у власних налаштуваннях
settings-appearance-new-pack = Новий набір
settings-appearance-os-decorations = Оформлення ОС
    .description = Заголовок вікна й рамки ОС на головних вікнах; вимкнено покладається на кнопки вікна й панелі з якорем перетягування
settings-appearance-pack-name-placeholder = Назва набору
settings-appearance-padding = Внутрішній відступ
    .description = Місце всередині краю кожної панелі, лишається в її власному тлі
settings-appearance-palette-export = Експорт
settings-appearance-palette-import = Імпорт
settings-appearance-panel-seams = Шви панелей
    .description = Волосяна лінія між плитками панелей; вимкнено лишає межі для зміни розміру невидимими, але їх усе одно можна тягнути
settings-appearance-resize-border = Рамка зміни розміру
    .description = Зміна розміру головних вікон перетягуванням за краї; діє лише з вимкненим Оформленням ОС, а якщо це вимкнути, лишаються прилипання і Win+стрілки
settings-appearance-rounding = Заокруглення
    .description = Заокруглити кути кожної панелі в тло
settings-appearance-section-colors = Кольори
settings-appearance-section-frame = Рамка
settings-appearance-section-icons = Значки
settings-appearance-section-interface = Інтерфейс
settings-appearance-section-theming = Теми
settings-appearance-section-transparency = Прозорість
settings-appearance-section-typography = Типографіка
settings-appearance-song-theming = Тема пісні
    .description = Тонувати палітру й підкладати тло під вікна обкладинкою треку, що грає
settings-appearance-surface-opacity = Непрозорість поверхні
    .description = Наскільки непрозорими читаються поверхні застосунку над тлом
settings-appearance-theme = Тема
    .description = Палітра, якою малює застосунок, і та, яку править редактор кольорів нижче; Системна йде за світлим чи темним налаштуванням ОС
settings-appearance-theme-dark = Темна
settings-appearance-theme-light = Світла
settings-appearance-theme-system = Системна

## Settings: application
settings-application-check-updates = Перевіряти оновлення
    .description = Шукати новіший випуск раз на день при запуску rox; вікно Про програму перевіряє зараз у будь-якому разі
settings-application-download-updates = Завантажувати оновлення
    .description = Коли перевірка знаходить новіший випуск, завантажити й підготувати його у фоні; наступний запуск його застосує
settings-application-enable-ai = Увімкнути функції ШІ
    .description = Дозволити інструментам ШІ говорити з rox: додає підтримку MCP і завантаження моделей ML, а їхні сторінки з'являються на бічній панелі.
settings-application-lock-panel-resize = Заблокувати зміну розміру панелей
    .description = Розділювачі панелей рухаються лише в Режимі дизайну, тож перетягування біля шва не зрушить готову розкладку
settings-application-portable-copying = Копіюємо дані...
settings-application-portable-mode = Портативний режим
    .description = Тримати налаштування, медіатеку й кеші в теці rox-data поруч із виконуваним файлом, щоб програвач переїжджав разом зі своїми даними. Вимкнення повертає системну теку й лишає rox-data на місці
settings-application-portable-not-writable = Тека застосунку недоступна для запису
settings-application-portable-restart-note = Застосується при наступному запуску; цей сеанс лишається на поточній теці
settings-application-remain-in-tray = Лишатися в лотку
    .description = Не спиняти музику, коли закрито останнє вікно; значок у лотку (док на macOS) - шлях назад
settings-application-section-ai = ШІ
settings-application-section-control-socket = Керувальний сокет
settings-application-section-data = Дані
settings-application-section-layout = Розкладка
settings-application-section-startup = Запуск
settings-application-section-window = Вікно
settings-application-socket-path = Шлях до сокета
    .description = Машинний інтерфейс rox, поки він працює: JSON-RPC через локальний сокет, прив'язаний до цієї теки даних. Проксі rox-mcp обслуговує через нього клієнтів MCP

## Settings: audio
settings-audio-broadcast-bitrate = Бітрейт
    .description = Скільки кодувальник MP3 витрачає на секунду потоку
settings-audio-broadcast-enable = Транслювати на Icecast
    .description = Надсилати те, що грає rox, на сервер icecast як клієнт-джерело, кодуючи в MP3. Точка монтування, слухачі й мережевий бік належать icecast; rox лише під'єднується назовні, а недоступний сервер ніколи не чіпає локальне відтворення
settings-audio-broadcast-host-placeholder = хост icecast
settings-audio-broadcast-login = Вхід джерела
    .description = Облікові дані джерела icecast, користувач і пароль, які називає його конфіг
settings-audio-broadcast-mount = Точка монтування
    .description = Точка, на яку налаштовуються слухачі, і назва потоку, яку вона оголошує
settings-audio-broadcast-name-placeholder = Назва потоку
settings-audio-broadcast-password-placeholder = Пароль джерела
settings-audio-broadcast-server = Сервер
    .description = Хост і порт сервера icecast; протокол джерела йде звичайним сокетом
settings-audio-broadcast-user-placeholder = source
settings-audio-crossfade = Кросфейд
    .description = Як довго трек накладається на наступний. Згасання потрібне саме для перемішування й перемикань, тож власні межі альбому лишаються недоторканими, поки рядок нижче не скаже інакше. Нуль вимикає це
    .keywords = без пауз накладання перехід згасання
settings-audio-equalizer-note = Десять октавних смуг на виході. Відкривається у власному вікні, бо з ним працюють під музику, а не налаштовують один раз
settings-audio-exclusive-mode = Ексклюзивний режим
    .description = Забрати пристрій під сам лише rox і пустити його на власній частоті файлу там, де залізо це приймає; вимкнено ділить системний мікшер з усім іншим на робочому столі
settings-audio-fade-inside-albums = Згасання всередині альбомів
    .description = Накладати й треки, що належать одному запису. Вимкнено лишає власні склейки запису точно такими, як їх зведено, а саме там безперервність між треками важить найбільше
settings-audio-open-equalizer = Відкрити еквалайзер
settings-audio-output-buffer = Буфер
    .description = Скільки звуку карта тримає за раз. Коротший реагує швидше й раніше тріщить на завантаженій машині; довший безпечніший і лінивіший
settings-audio-output-buffer-default = Типовий (10 мс)
settings-audio-output-device = Пристрій
    .description-default = Системний типовий іде за тим, що виставлено на робочому столі
    .description-linux = Ексклюзивний забирає карту просто з ядра, тож у списку звукові карти, а не виходи робочого столу. Bluetooth та інші пристрої звукового сервера не мають карти, яку можна забрати, і показуються лише з вимкненим ексклюзивним режимом
    .description-other = Ексклюзивний забирає пристрій під сам лише rox, тож ніщо інше на робочому столі не звучатиме крізь нього, поки режим не вимкнути
settings-audio-output-device-system-default = Системний типовий
settings-audio-output-experimental-badge = Експериментальне
settings-audio-output-experimental-tooltip = Ексклюзивний бекенд для цієї платформи написано за задокументованим звуковим контрактом платформи, але розробники ніколи не ганяли його на справжньому залізі. Він має або забрати пристрій, або відкотитися до спільного режиму з поясненням, і ніколи не мовчати. Якщо він поводиться дивно, вимкніть його й розкажіть, що сталося, кнопкою поруч із цією позначкою.
settings-audio-output-format = Формат
    .description = Що rox передає карті. Карта, яка не бере вибране, працює в найширшому форматі, який має, а статус нижче показує, у якому саме
settings-audio-output-format-f32 = 32-бітний float
settings-audio-output-format-s16 = 16-бітний integer
settings-audio-output-format-s32 = 32-бітний integer
settings-audio-output-format-widest = Найширший доступний
settings-audio-output-issue-tooltip = Розкажіть, як ексклюзивний режим повівся на цій машині. Відкриває issue на GitHub із заповненою платформою й узгодженим потоком.
settings-audio-output-mode-exclusive = Ексклюзивний
settings-audio-output-mode-shared = Спільний
settings-audio-output-not-built = Ще не зібрано для цієї платформи
settings-audio-output-rate-follow = Іти за файлом
settings-audio-output-sample-rate = Частота дискретизації
    .description = Режим Іти за файлом перевідкриває пристрій на власній частоті кожного файлу, що коштує паузи на межі, де частота змінюється; закріплена частота цього не коштує й передискретизує все, що не збігається
settings-audio-output-status-error-hint = Виберіть інший пристрій або вимкніть ексклюзивний режим
settings-audio-output-status-error-title = Немає виходу
settings-audio-output-status-idle-hint = Запустіть трек, щоб побачити формат, який прийняв пристрій
settings-audio-output-status-idle-title = Нічого не грає
settings-audio-replaygain-level-by = Вирівнювати за
    .description = Грати кожен трек із гучністю, яку виміряли його теги ReplayGain, щоб перемішування перестало стрибати між зведеннями. Трек міряє кожен файл окремо; Альбом бере підсилення запису на всі його треки, що лишає тихі й гучні місця альбому там, де їх поставили
    .keywords = нормалізація гучність вирівнювання рівень
settings-audio-replaygain-measure-missing-button = Виміряти те, чого бракує
settings-audio-replaygain-measure-new = Міряти нові файли
    .description = Міряти те, що приносить спостерігач, щойно воно з'явиться і синхронізація вляжеться, щоб медіатека, яка росте, тримала свої підсилення без повернення сюди. Числа лягають туди, куди вказує Зберігати виміряні підсилення. Увімкнення спершу запропонує виміряти те, чого вже бракує; після цього воно бачить лише щойно додані файли
settings-audio-replaygain-measuring-progress = Міряємо { $done } з { $total }
settings-audio-replaygain-measuring-start = Міряємо: з'ясовуємо, чого бракує...
settings-audio-replaygain-mode-album = Альбом
settings-audio-replaygain-mode-off = Вимк.
settings-audio-replaygain-mode-track = Трек
settings-audio-replaygain-preamp = Попереднє підсилення
    .description = Додається до кожного підсилення з тегів. Опорний рівень ReplayGain нижчий за той, на якому зводять сучасні записи, тож вирівняна медіатека грає тихіше за ту саму медіатеку без обробки; тут це повертається. Підйом ніколи не кліпує: виміряний пік його обмежує
settings-audio-replaygain-save = Зберігати виміряні підсилення
    .description = Куди прохід вимірювання кладе свої числа. База медіатеки лишає ваші файли недоторканими; теги кладуть ті самі значення туди, звідки їх читає кожен інший програвач, ціною перезапису звукових файлів
settings-audio-replaygain-status-measured = Усі скановані треки ({ $total }) мають підсилення для вирівнювання, з них { $measured } виміряв rox
settings-audio-replaygain-status-tagged = Усі скановані треки ({ $total }) мають теги ReplayGain
settings-audio-replaygain-untagged = Файли без тегів
    .description = З яким рівнем грає файл без тегів ReplayGain. Його ніхто не міряв, тож це здогад замість виміру. Лишіть нуль, і треки без тегів гратимуть, як завжди
settings-audio-section-broadcast = Мовлення
settings-audio-section-equalizer = Еквалайзер
settings-audio-section-output = Вихід
settings-audio-section-playback = Відтворення
settings-audio-section-replaygain = ReplayGain
settings-audio-transport = Керування
    .description = Пуск і зупинка, не покидаючи цієї сторінки, бо кожне налаштування нижче оцінюють на слух

## Settings: integrations
settings-integrations-discord-enable = Увімкнути Rich Presence
    .description = Показувати активність rox у Discord, коли грає музика
settings-integrations-discord-show-lastfm = Показувати кнопку Last.fm
    .description = Додати в статус Discord клікабельну кнопку «Переглянути на Last.fm»
settings-integrations-discord-show-youtube = Показувати кнопку YouTube
    .description = Додати в статус Discord клікабельну кнопку «Шукати на YouTube»
settings-integrations-ffmpeg-binary = Виконуваний файл FFmpeg
    .description = Який ffmpeg виконує конвертації; лишіть порожнім, щоб узяти той, що в PATH
settings-integrations-ffmpeg-fail-note = Конвертація лишається прихованою, поки ffmpeg не вкаже на робочий виконуваний файл
settings-integrations-ffmpeg-fail-title = Цей ffmpeg не запустився
settings-integrations-ffmpeg-missing-note = Конвертація лишається прихованою; установіть ffmpeg або вкажіть шлях до виконуваного файлу
settings-integrations-ffmpeg-missing-title = Робочого ffmpeg не знайдено
settings-integrations-ffmpeg-ok-note = ffmpeg працює. Конвертація доступна.
settings-integrations-ffmpeg-test = Перевірити
settings-integrations-lastfm-api-key-row = Ключ API
settings-integrations-lastfm-connect = Під'єднати
settings-integrations-lastfm-disconnect = Від'єднати
settings-integrations-lastfm-finish-connecting = Завершити під'єднання
settings-integrations-lastfm-hearts = { $n ->
    [one] { $n } сердечко
    [few] { $n } сердечка
    [many] { $n } сердечок
   *[other] { $n } сердечка
}
settings-integrations-lastfm-import-loved = Імпортувати улюблені треки
settings-integrations-lastfm-intro-builtin = Під'єднайте свій акаунт Last.fm: авторизуйте rox у браузері, і прослухані треки підуть у скробл
settings-integrations-lastfm-intro-custom = У цій збірці немає власної api-ідентичності, тож для скроблу потрібен ваш власний акаунт api (Last.fm/api/account/create); вставте його ключ і спільний секрет, а потім під'єднайтеся
settings-integrations-lastfm-key-placeholder = Ключ API
settings-integrations-lastfm-love-failed = Остання спроба не вдалася: { $error }
settings-integrations-lastfm-love-pending = У черзі на надсилання: { $hearts }
settings-integrations-lastfm-love-pending-failed = У черзі на надсилання: { $hearts }, остання спроба: { $error }
settings-integrations-lastfm-reconnect = Під'єднатися знову
settings-integrations-lastfm-secret-placeholder = Спільний секрет
settings-integrations-lastfm-secret-row = Спільний секрет
settings-integrations-lastfm-status-confirming = Підтверджуємо...
settings-integrations-lastfm-status-connected = Під'єднано як { $username }
settings-integrations-lastfm-status-elsewhere = Під'єднано в іншій копії rox; кожна авторизується під власною api-ідентичністю, тож під'єднайте й цю
settings-integrations-lastfm-status-failed = Не вдалося під'єднатися: { $error }
settings-integrations-lastfm-status-not-connected = Не під'єднано
settings-integrations-lastfm-status-rejected = Last.fm відхилив сеанс, і його скинуто. Під'єднайтеся знову, щоб скробл тривав
settings-integrations-lastfm-status-requesting = Запитуємо токен...
settings-integrations-lastfm-status-waiting = Авторизуйте rox у браузері, а потім завершіть під'єднання
settings-integrations-lastfm-working = Працюємо...
settings-integrations-love-favourites = Улюблені як Loved
    .description = Дзеркалити сердечка на Last.fm як loved-треки; зняте сердечко знімає його й там
settings-integrations-scrobble-threshold = Поріг скроблу
    .description = Скільки треку має відіграти, перш ніж він піде у скробл; смуга перемотки й хвиля можуть це позначити
settings-integrations-scrobble-tracks = Скроблити треки
    .description = Надсилати прослухані треки на Last.fm, щойно вони перетнуть поріг
settings-integrations-section-conversion = Конвертація
settings-integrations-section-discord = Discord Rich Presence
settings-integrations-section-favourites = Улюблене
settings-integrations-section-lastfm = Last.fm
settings-integrations-section-scrobbling = Скробл

## Settings: keymap
settings-keymap-clash = { $chord } - це також { $other }; спрацює лише одне з них
settings-keymap-not-bound = Не призначено
settings-keymap-recording = Натисніть клавіші
settings-keymap-restore = Відновити
settings-keymap-restore-all = Відновити всі поєднання
    .description = Повернути кожну команду на клавіші, з якими вона постачається, зокрема й ті, для яких у цій збірці вже немає рядка
settings-keymap-section-defaults = Типові
settings-keymap-undo = Скасувати
settings-keymap-undo-last = Скасувати останнє скидання
    .description = Повернути поєднання, які викинуло останнє скидання, чи то рядка, чи то всіх

## Settings: library
settings-library-acoustic-all-described = Усі скановані треки ({ $total }) описано моделлю { $label }
settings-library-acoustic-auto = Описувати нові файли
    .description = Описувати те, що приносить спостерігач, щойно воно з'явиться і синхронізація вляжеться, щоб медіатека, яка росте, тримала свої описи без повернення сюди. Вимкнено, нові файли чекають на кнопку Проаналізувати те, чого бракує. Увімкнення спершу запропонує проаналізувати те, чого вже бракує; після цього воно бачить лише щойно додані файли
settings-library-acoustic-enable = Описувати, як звучать треки
    .description = З'ясувати, як звучить кожен трек, щоб медіатека могла знаходити музику, схожу на ту, що грає. Усе працює на цій машині, а опис великої медіатеки триває довго
    .keywords = схоже звучання відбиток опис
settings-library-acoustic-extractor = Екстрактор
settings-library-acoustic-extractor-model = Модель
settings-library-acoustic-fallback = Аналіз
settings-library-acoustic-partial = { $label } описує { $done } з { $total } сканованих треків. Проаналізувати те, чого бракує, візьметься за решту
settings-library-acoustic-progress = { $running } на { $done } з { $total }
settings-library-acoustic-progress-start = { $running }: з'ясовуємо, чого бракує...
settings-library-acoustic-save = Зберігати описи
    .description = Куди прохід кладе те, що з'ясував. Сама база лишає ваші файли недоторканими; теги кладуть копію ще й у кожен файл, тож описи переживуть перебудову медіатеки чи переїзд теки на іншу машину, ціною перезапису звукових файлів. Теги дістають лише MP3 і FLAC; усі інші формати лишаються з копією в базі
settings-library-add-folder = Додати теку
settings-library-duplicates = Дублікати...
settings-library-embed-button = Вписати збережені метадані...
settings-library-folder-col-albums = Альбоми
settings-library-folder-col-folder = Тека
settings-library-folder-col-size = Розмір
settings-library-folder-col-tracks = Треки
settings-library-folders-intro = Теки, скановані в медіатеку; прибрана тека прибирає свої треки з каталогу, а файли лишає на місці
settings-library-genre-separator-nudge = Роздільники змінилися: перегляд підхопить це одразу. Списки жанрів, збережені попередніми скануваннями, тримають стару форму, поки ви не натиснете Пересканувати вгорі, у заголовку Теки
settings-library-merge-case = Зливати варіанти регістру
    .description = Вважати значення, що різняться лише регістром, одним: Rock і rock стають тим самим жанром, виконавцем і альбомом, а показуються в тому написанні, яким його пише більшість треків. Файли тримають свої теги як написано
settings-library-no-folders = Ще немає тек
settings-library-repair-tags = Полагодити теги...
settings-library-section-folders = Теки
settings-library-section-stored-metadata = Збережені метадані
settings-library-section-tempo = Аналіз темпу
settings-library-split-genres = Ділити жанри по комах і скісних
    .description = «Dubstep, Trap» і «Drum & Bass / Neurofunk» рахують кожне значення окремим жанром; крапка з комою ділить завжди. Вимкнено лишає назви зі скісною цілими для тегів, де вони означають один жанр. Файли тримають свої теги як написано
settings-library-tempo-auto = Міряти час нових файлів
    .description = Рахувати біти в тому, що приносить спостерігач, щойно воно з'явиться і синхронізація вляжеться, щоб медіатека, яка росте, тримала свої темпи без повернення сюди. Вимкнено, нові файли чекають на кнопку Проаналізувати те, чого бракує. Увімкнення спершу запропонує зміряти те, чого вже бракує; після цього воно бачить лише щойно додані файли
settings-library-tempo-enable = З'ясовувати, як швидко йдуть треки
    .description = Рахувати біти в треках, чиї теги про це мовчать, щоб медіатека могла показувати темп і сортувати за ним. Усе працює на цій машині, числа лягають у базу медіатеки, а ваші файли лишаються недоторканими
settings-library-tempo-progress = З'ясовуємо темп, { $done } з { $total }
settings-library-tempo-progress-start = З'ясовуємо, чого бракує...
settings-library-tempo-refused = . Треків, у яких rox не розчув ритму: { $count }. «Проаналізувати те, чого бракує» їх не чіпає
settings-library-tempo-retry = Повторити відхилені
settings-library-tempo-status-measured = Усі скановані треки ({ $total }) мають темп, з них { $measured } з'ясував rox
settings-library-tempo-status-measured-some = Темп мають { $covered } зі { $total } сканованих треків, з них { $measured } з'ясував rox
settings-library-tempo-status-none = Жоден зі сканованих треків ({ $total }) не каже, як швидко він грає. «Проаналізувати те, чого бракує» це з'ясує
settings-library-tempo-status-partial = Темп мають { $covered } зі { $total } сканованих треків, з них { $measured } з'ясував rox. «Проаналізувати те, чого бракує» візьметься за решту ({ $missing })
settings-library-tempo-status-tagged = Усі скановані треки ({ $total }) мають тег темпу
settings-library-tempo-status-tagged-some = Тег темпу мають { $covered } зі { $total } сканованих треків
settings-library-watch-folders = Стежити за теками
    .description = Вносити додані, змінені й видалені файли в медіатеку в міру того, як це стається, без ручного пересканування
settings-library-write-stored = Записати збережене у файли
    .description = Три налаштування збереження діють лише на наступний запис, тож усе, збережене до того, як котресь перемкнули на Теги, лишається тільки в rox. Це записує тексти пісень, підсилення й описи, які rox уже тримає, у самі файли, щоб їх бачив інший програвач, який читає цю теку. Нічого не перераховується
settings-show-readings = Показувати читання
    .description = Ставити латинське читання після імені, записаного письмом, яке ця абетка не прочитає: 秋ノ風 (Aki no kaze). Читання береться з імені для сортування, яке значення вже має, тож імʼя без нього нічого не показує, а латинське імʼя читання не отримує

## Settings: MCP
settings-mcp-client-config = Конфіг клієнта
    .description = Вставте в список серверів MCP-клієнта (Claude Code, Claude Desktop чи будь-якого іншого), щоб він міг питати rox про медіатеку, те, що грає, і керування відтворенням. rox має бути запущеним; інструменти працюють через його керувальний сокет
settings-mcp-enable = Увімкнути сервер MCP
    .description = Відповідати на виклики інструментів від під'єднаних MCP-клієнтів. Проксі перевіряє це на кожному виклику, тож поки вимкнено, клієнти дістають відмову з поясненням; конфіг нижче можна налаштувати в будь-якому разі

## Settings: ML models
settings-mlmodels-checking = Перевіряємо...
settings-mlmodels-choose-file = Вибрати файл
settings-mlmodels-custom-description-empty = Укажіть rox власний чекпоінт PANNs CNN10 у форматі safetensors. Він читається на місці й називається за своїм хешем, тож другий чекпоінт описує медіатеку окремо, а не перевикористовує координати першого
settings-mlmodels-download-failed = Не вдалося завантажити { $label }: { $reason }
settings-mlmodels-downloading = Завантажуємо { $label }: { $done } з { $total }
settings-mlmodels-stopping = Спиняємо завантаження { $label }...
settings-dictionary-description = { $summary }. { $licence }
settings-dictionary-download-failed = Не вдалося завантажити словник: { $reason }
settings-dictionary-downloading = Завантажуємо словник: { $done } з { $total }
settings-dictionary-heading = Романізація
settings-dictionary-stopping = Спиняємо завантаження словника...
settings-mlmodels-fallback-model = модель
settings-mlmodels-fallback-the-model = Модель
settings-mlmodels-kind-custom = Власна
settings-mlmodels-kind-recommended = Рекомендована
settings-mlmodels-pass-stopped = Останній прохід спинився: { $reason }
settings-mlmodels-weights-file = Файл ваг

## Settings: playback
settings-playback-continuation-continue = Продовжувати
    .description = Іти далі списком, з якого ви почали, а потім рештою медіатеки за ним. Запустіть альбом із середини вигляду, і вигляд піде далі
settings-playback-continuation-off = Вимк.
    .description = Ніщо не поповнює чергу; відтворення спиняється в її кінці
settings-playback-continuation-weighted = Зважено
    .description = Брати з усієї медіатеки: спершу те, чого ви ніколи не грали, наостанок те, що чули нещодавно
settings-playback-keep-playing = Далі грати
    .description = Що грає, коли черга скінчилася. Усе, що воно вибере, дописується в стрічку як звичайний контекст, тож його видно й можна прибрати, а не сховано десь у стані. Коли порядок вище стоїть на Схоже, воно й далі шукає треки, що звучать як той, що грає, хоч би що з цього було вибрано
    .keywords = продовження поповнення автовідтворення черга
settings-playback-play-order = Порядок відтворення
    .description = Як розставлені вже поставлені в чергу треки, поки ввімкнено перемішування. Кнопка перемішування в керуванні вмикає й вимикає його; тут задається, що саме воно робить, коли ввімкнене
settings-playback-rating-scale = Шкала оцінок
    .description = Зірки для швидких кліків, 0-10 із половинними кроками для точніших рецензій
settings-playback-rating-scale-numeric = 0-10
settings-playback-rating-scale-stars = Зірки
settings-playback-restore-last-session = Відновлювати останній сеанс
    .description = Запускатися з чергою в тому вигляді, в якому ви її лишили, на паузі на треку, що грав, і там, де він спинився. Треки з черги поза теками вашої медіатеки відновити не вийде, і вони випадають із порядку
settings-playback-section-queue = Черга
settings-playback-section-ratings = Оцінки
settings-playback-section-startup = Запуск
settings-playback-shuffle-random = Випадково
    .description = Те перемішування, яке всі й мають на увазі. Те, що попереду, грає без жодного порядку
settings-playback-shuffle-similar = Схоже
    .description = Спершу найближче за звучанням. Те, що попереду, відсортовано за схожістю на трек, який грав, коли ви це ввімкнули, і пересортовується на кожному перемиканні. Потребує описаної медіатеки на сторінці Медіатека
settings-playback-unrated-dots = Крапки замість порожніх зірок
    .description = Позначати незаповнені місця під зірки блідою крапкою, а не лишати їх порожніми

## Settings: providers
settings-providers-artist = Last.fm
    .description = Тягнути біографії виконавців, статистику й схожих виконавців для панелі біографії, а портрет - з Deezer; усе лягає в теку даних і потім читається офлайн
settings-providers-deezer = Deezer
    .description = Шукати обкладинки на Deezer, до 1000 пікселів
settings-providers-itunes = iTunes
    .description = Шукати обкладинки в iTunes; пошук у редакторі обкладинок показує збіги, з яких можна вибрати, перш ніж ставити
settings-providers-lastfm-art = Last.fm
    .description = Шукати обкладинки на Last.fm
settings-providers-lrclib = LRCLIB
    .description = Тягнути тексти пісень, яких бракує, з lrclib.net, синхронізовані аркуші там, де вони є
settings-providers-lyrics-intro = Онлайн-пошук іде лише тоді, коли його просить дія в панелі; відтворення й перегляд ніколи не чіпають мережу
settings-providers-musicbrainz = MusicBrainz
    .description = Шукати теги на musicbrainz.org; пошук у панелі метаданих показує збіги, які можна підтвердити поле за полем перед записом
settings-providers-save-lyrics = Зберігати завантажені тексти
    .description = Куди зберігається завантажений аркуш: у власну теку даних rox, лишаючи медіатеку чистою, у файл .lrc поруч із треком або у вбудований тег
settings-providers-save-lyrics-data-folder = Тека даних
settings-providers-save-lyrics-sidecar = Файл поруч
settings-providers-save-lyrics-tag = Тег
settings-providers-section-artist = Виконавець
settings-providers-section-cover-art = Обкладинки
settings-providers-section-lyrics = Тексти пісень
settings-providers-section-metadata = Метадані

## Settings: shader
settings-shader-backdrop-all-windows = Усі вікна
    .description = Затінювати тло кожного вікна: налаштування, редактори, діалоги, відділені панелі. Вимкнено лишає це вікнам робочого простору
settings-shader-backdrop-enabled = Шейдер тла
    .description = Пустити музично-реактивний шейдер WGSL по тлу з обкладинки, під усіма панелями. Частина робочого простору, тож він мандрує разом із виглядом
settings-shader-backdrop-fallback-name = Тло
settings-shader-backdrop-run-idle = Працювати в простої
    .description = Малювати й далі, коли нічого не грає. Анімація в будь-якому разі лишається на місці
settings-shader-compile-error-title = Цей шейдер не скомпілювався
settings-shader-legacy-note = Коли нічого не змаршрутовано, пул заповнює слоти у власному порядку: перший сигнал у слот 0, другий у слот 1 і так далі. Перший доданий вами маршрут перебирає на себе все зіставлення.
settings-shader-overlay-enabled = Шейдер накладки
    .description = Пустити музично-реактивний шейдер WGSL по всьому вікну. Пропонуються лише шейдери, які лишають застосунок під собою придатним до роботи
settings-shader-scene-covers-window = Цей шейдер - сцена, тож він накриває вікно, а не малює поверх нього. Він прийшов із набору або зі старішого конфігу; список вище пропонує лише шейдери, які лишають застосунок придатним до роботи.
settings-shader-screen-all-windows = Усі вікна
    .description = Затінювати й дочірні вікна: налаштування, статистику, еквалайзер, відділені панелі. Зворотний відлік до відкату в будь-якому разі лишається незатіненим
settings-shader-screen-fallback-name = Екран
settings-shader-screen-run-idle = Працювати в простої
    .description = Малювати й далі, коли нічого не грає. Анімація в будь-якому разі лишається на місці. Шейдер, який читає мишу, іде за курсором і зі спиненою музикою без цього; він просто спиняється за пару секунд після вказівника
settings-shader-section-backdrop = Шейдер тла
settings-shader-section-overlay = Шейдер накладки
settings-shader-signals-block = Сигнали
    .description = Який спільний сигнал читає кожен із шістнадцяти слотів шейдера
settings-shader-slots-block = Слоти
    .description = Кожен слот таким, яким він доходить до шейдера; слоти без маршруту - це регулятори, виставлені вручну

## Settings: storage
settings-storage-artist-images = Зображення виконавців
    .description = Портрети, банери й біографії, завантажені для виглядів виконавців (artists/); очищені завантажаться знову, коли вигляд відкриється наступного разу
settings-storage-catalog = Каталог
    .description = Індекс треків, який будують сканування: рядок - трек із його тегами, подробицями файлу й будь-якими проміжками з cue, усередині library.db
settings-storage-cover-thumbnails = Мініатюри обкладинок
    .description = Маленькі обкладинки, збережені після першого малювання (thumbs.db); очищені перебудуються, коли доскролите до них
settings-storage-logs = Журнали
    .description = Те, що кожен запуск пише для звітів про вади (logs/rox.log), із перекиданням за розміром, тож файл ніколи не розростається
settings-storage-looks-layouts = Вигляд і розкладки
    .description = Вигляд, яким зараз користується застосунок (workspace.json), а поруч ваші збережені робочі простори, вивантажені файли шейдерів і набори значків. Мало місця, і кожен байт цього ви налаштували самі
settings-storage-lyrics = Тексти пісень
    .description = Завантажені й відредаговані аркуші, збережені у власному сховищі застосунку (lyrics/), тож теки медіатеки лишаються чистими
settings-storage-measured-tempos = Виміряні темпи
    .description = Темпи, які rox нарахував зі звуку, для треків, у чиїх тегах їх немає; власні числа тегів не чіпаються. Очищення повертає ці треки в список кнопки Проаналізувати те, чого бракує, на сторінці Медіатека, щоб покращений підрахунок бітів міг замінити числа, які записав старіший прохід
settings-storage-model-fallback-this = Ця модель
settings-storage-music-summary = { $tracks }, { $albums }, { $size }
settings-storage-model-weights = Ваги моделей
    .description = Моделі, завантажені для акустичного аналізу (models/). Тягнуть і видаляють їх на сторінці Моделі ML, рядок на модель
settings-storage-models-empty = Моделі
    .description = Медіатеку ще ніщо не описувало. Це заповниться, коли ввімкнути акустичний аналіз на сторінці Медіатека, і кожна модель, яка відпрацювала, дістане тут свій рядок
settings-storage-music-files = Музичні файли
    .description = Те, що тримають скановані теки; файли лишаються там, де вони є
settings-storage-none = Немає
settings-storage-playlists-history = Списки відтворення й історія
    .description = Ваші списки відтворення з їхнім вмістом, те, що ви грали, і жанрові нотатки медіатеки. Усе це дрібниця поруч із рештою library.db
settings-storage-reclaimable = Місце, яке можна повернути
    .description = Сторінки всередині library.db, які лишили по собі видалення. Нові записи заповнять їх знову, тож файл перестає рости раніше, ніж починає меншати
    .keywords = vacuum стиснення зменшення база даних
settings-storage-section-acoustic = Акустичні описи
settings-storage-section-app-data = Дані застосунку
settings-storage-section-caches = Кеші
settings-storage-section-diagnostics = Діагностика
settings-storage-section-library = Медіатека
settings-storage-section-tempo = Темп
settings-storage-vectors = Вектори
    .description = Скільки важить кожен опис усередині library.db. На медіатеці, якою пройшовся аналіз, це більша частина файлу: пара кілобайтів на трек проти кількох сотень байтів тегів
settings-storage-waveforms = Форми хвилі
    .description = Смуга піків кожного треку, збережена після першого відтворення; очищені декодуються знову при наступному

## Settings: workspace
settings-workspace-card-author = Автор
settings-workspace-card-author-placeholder = Хто це зробив
settings-workspace-card-created = Створено { $date }
settings-workspace-card-created-updated = Створено { $created }, оновлено { $updated }
settings-workspace-card-description = Опис
settings-workspace-card-description-placeholder = До чого прагне цей вигляд
settings-workspace-card-empty = У цього робочого простору немає картки
settings-workspace-card-hint = Картка зберігається у файлі, тож її побачить кожен, з ким ви поділитеся цим виглядом
settings-workspace-card-license = Ліцензія
settings-workspace-card-license-placeholder = Умови, на яких ви цим ділитеся
settings-workspace-card-save = Зберегти картку
settings-workspace-card-updated = Оновлено { $date }
settings-workspace-card-version = Версія
settings-workspace-card-version-placeholder = Ваша власна версія, у чому б ви її не рахували
settings-workspace-card-website = Сайт
settings-workspace-card-website-placeholder = Де це живе
settings-workspace-composition-closed = Вікно робочого простору закрито
settings-workspace-composition-hint = Панелі вікна так, як вони розставлені в поділах і групах вкладок; стрілки міняють порядок рядка серед сусідів, замок фіксує панель на місці, а шестірня відкриває її налаштування
settings-workspace-empty = Ще немає робочих просторів
settings-workspace-hint = Робочий простір - це цілий вигляд: розкладки, палітра, оформлення. Застосування замінює всі три
settings-workspace-layout-name-placeholder = Назва розкладки
settings-workspace-layouts-empty = Ще немає розкладок
settings-workspace-layouts-hint = Основна й міні - це ті дві, між якими перемикає кнопка міні-програвача на смузі меню
settings-workspace-name-placeholder = Назва робочого простору
settings-workspace-panel-preset-unknown-kind = Невідома панель
settings-workspace-panel-presets-empty = Ще немає пресетів панелей
settings-workspace-panel-presets-hint-after = у меню будь-якої панелі. Вони належать лише цьому робочому простору; в іншому їх не буде.
settings-workspace-panel-presets-hint-before = По одній налаштованій панелі, збережені з меню самої панелі; повернути їх можна через
settings-workspace-role-mini = Міні
settings-workspace-role-primary = Основна
settings-workspace-section-composition = Композиція
settings-workspace-section-layouts = Розкладки
settings-workspace-section-panel-presets = Пресети панелей
settings-workspace-section-workspaces = Робочі простори
settings-workspace-tree-empty-slot = Порожній слот
settings-workspace-tree-split-column = Поділ, один над одним
settings-workspace-tree-split-row = Поділ, поруч
settings-workspace-tree-tabs = Вкладки

## Settings: development
settings-development-experimental-panels = Експериментальні панелі
    .description = Показувати панелі, які ще будуються, у меню Панелі й у стартовому вікні; вони міняють форму між випусками, а розкладка, яка вже тримає таку панель, лишить її при вимкненні цього
settings-development-section-features = Можливості

## Settings: shared
settings-acoustic-analysis-heading = Акустичний аналіз
settings-analyze-nothing-scanned = Ще нічого не скановано для аналізу
settings-common-active = Активна
settings-common-analyze-missing = Проаналізувати те, чого бракує
settings-common-built-in = Вбудоване
settings-common-cancel = Скасувати
settings-common-clear = Очистити
settings-common-copy = Копіювати
settings-common-database = База даних
settings-common-delete = Видалити
settings-common-download = Завантажити
settings-common-rescan = Пересканувати
settings-common-reveal = Показати
settings-common-stop = Спинити
settings-common-stopping = Спиняємо...
settings-common-tags = Теги
settings-common-tracks-count = { $count ->
    [one] { $count } трек
    [few] { $count } треки
    [many] { $count } треків
   *[other] { $count } трека
}
settings-common-use = Узяти
settings-confirm-apply-body = Це замінить ваші розкладки, палітру й оформлення на ті, що в робочому просторі.
settings-confirm-apply-imported-body = Його збережено до ваших робочих просторів. Застосування зараз замінить ваші розкладки, палітру й оформлення на ті, що в ньому.
settings-confirm-clear = Очистити
settings-confirm-clear-embeddings-body = Описи зникнуть, а місце повернеться. Щоб мати їх знову, доведеться прогнати аналіз по кожному треку в медіатеці.
settings-confirm-clear-embeddings-title = Очистити те, що описала «{ $model }»?
settings-confirm-clear-measured-bpm-body = Кожен темп, який з'ясував rox, стане невиміряним; числа з власних тегів ваших файлів лишаться. Щоб мати їх знову, доведеться прогнати прохід темпу по кожному з цих треків.
settings-confirm-clear-measured-bpm-title = Очистити виміряні темпи?
settings-confirm-overwrite-workspace-body = Це замінить збережений робочий простір поточним станом.
settings-confirm-overwrite-workspace-title = Перезаписати робочий простір «{ $name }»?
settings-sidebar-data-folder = Тека даних
settings-sidebar-settings-file = Файл налаштувань

## Menubar
menu-about = Про програму
menu-analyze-tempo = Проаналізувати темп...
menu-application = Застосунок
menu-apply-layout = Застосувати розкладку
menu-apply-workspace = Застосувати робочий простір
menu-build-acoustic = Побудувати акустичні вектори...
menu-chat = Чат
menu-close = Закрити
menu-console = Консоль
menu-design-mode = Режим дизайну
menu-discussions = Обговорення
menu-empty-window = Порожнє вікно
menu-equalizer = Еквалайзер
menu-exit = Вийти
menu-fill-sort-names = Заповнити імена для сортування...
menu-romanize-library = Романізувати медіатеку...
menu-find-duplicates = Знайти дублікати...
menu-tag-genres = Проставити жанри...
menu-health = Стан бібліотеки
menu-power-search = Розширений пошук
menu-hide-menubar = Ховати смугу меню
menu-import-workspace = Імпортувати робочий простір...
menu-library = Медіатека
menu-measure-replaygain = Виміряти ReplayGain...
menu-new-ellipsis = Новий...
menu-new-window = Нове вікно
menu-new-window-from-layout = Нове вікно з розкладки
menu-new-window-from-panel = Нове вікно з панелі
menu-no-layouts = Немає розкладок
menu-no-presets = Немає пресетів
menu-no-workspaces = Немає робочих просторів
menu-os-decorations = Оформлення ОС
menu-overlay-shader = Шейдер накладки
menu-panel-built-in = Вбудовані
menu-panel-new = Нова...
menu-panel-no-layouts = Немає розкладок
menu-panel-no-presets = Немає пресетів
menu-panel-no-workspaces = Немає робочих просторів
menu-panel-title = Меню
menu-panels = Панелі
menu-panels-presets = Пресети
menu-pause = Пауза
menu-playback = Відтворення
menu-remain-in-tray = Лишатися в лотку
menu-report-issue = Повідомити про проблему
menu-rescan-library = Пересканувати медіатеку
menu-save-layout = Зберегти розкладку
menu-save-workspace = Зберегти робочий простір
menu-section-add = Додати
menu-section-analyze = Аналіз
menu-section-app = Застосунок
menu-section-interface = Інтерфейс
menu-section-layouts = Розкладки
menu-section-listening = Прослуховування
menu-section-maintain = Обслуговування
menu-section-session = Сеанс
menu-section-track = Трек
menu-section-tuning = Налаштування
menu-settings = Налаштування
menu-signals = Сигнали
menu-song-theming = Тема пісні
menu-stats = Статистика
menu-tasks = Завдання
menu-update-available = Доступне оновлення
menu-welcome = Вітання
menu-window = Вікно
menu-workspace = Робочий простір
menu-workspace-builtin-tag = Вбудований

## Workspaces
workspace-apply-body = Це замінить увесь вигляд: розкладки, палітру, оформлення.
workspace-apply-imported-body = Його збережено до ваших робочих просторів. Застосування зараз замінить увесь вигляд: розкладки, палітру, оформлення.
workspace-apply-imported-title = Імпортовано «{ $name }»
workspace-apply-screen-shader-named = Застосовує шейдер накладки { $name } на все вікно.
workspace-apply-screen-shader-plain = Застосовує шейдер накладки на все вікно.
workspace-apply-shader-count = { $count ->
    [one] Містить { $count } шейдер: { $names }
    [few] Містить { $count } шейдери: { $names }
    [many] Містить { $count } шейдерів: { $names }
   *[other] Містить { $count } шейдера: { $names }
}
workspace-apply-shaders-approve-body = Підтвердження дозволяє їм працювати на цій машині. Застосування без них лишає вигляд голим, а шейдери - в його пулі.
workspace-apply-shaders-plain-body = Застосування без них лишає вигляд голим, а шейдери - в його пулі.
workspace-byline-author = від { $author }
workspace-byline-version = версія { $version }
workspace-context-add-panel = Додати панель
workspace-dialog-apply = Застосувати
workspace-dialog-apply-title = Застосувати «{ $name }»?
workspace-dialog-approve-apply = Підтвердити й застосувати
workspace-dialog-cancel = Скасувати
workspace-dialog-close = Закрити
workspace-dialog-close-title = Закрити «{ $name }»?
workspace-dialog-export = Експорт
workspace-dialog-layout-name-placeholder = Назва розкладки
workspace-dialog-not-now = Не зараз
workspace-dialog-overwrite = Перезаписати
workspace-dialog-overwrite-title = Перезаписати «{ $name }»?
workspace-dialog-save = Зберегти
workspace-dialog-save-layout-title = Зберегти розкладку
workspace-dialog-save-workspace-title = Зберегти робочий простір
workspace-dialog-with-shaders = Із шейдерами
workspace-dialog-without-shaders = Без шейдерів
workspace-dialog-workspace-name-placeholder = Назва робочого простору
workspace-drop-add-queue = Додати в чергу
workspace-drop-play-now = Відтворити зараз
workspace-hint-or = або
workspace-hint-then = потім
workspace-import = Імпорт
workspace-launcher-hint = Додайте першу панель, щоб почати збирати, або виберіть готовий вигляд у Робочий простір > Застосувати робочий простір
workspace-launcher-need-help = Потрібна допомога?
workspace-launcher-open-welcome = Відкрити вікно вітання
workspace-launcher-title = Порожнє вікно
workspace-layout-apply-body = Це замінить поточну розкладку цього вікна.
workspace-layout-overwrite-body = Це замінить збережену розкладку поточною.
workspace-layout-preset-restore-failed = Не вдалося відновити пресет розкладки цього вікна, тож воно починає порожнім.
workspace-layout-restore-failed = Не вдалося відновити збережену розкладку, тож це вікно починає порожнім.
workspace-mini-tip-back = Назад до повної розкладки
workspace-mini-tip-shrink = Стиснути до міні-програвача
workspace-overwrite-body = Це замінить збережений робочий простір поточним виглядом.
workspace-panel-locked-close-body = Цю панель зафіксовано на місці. Закриття прибирає її з розкладки.
workspace-save-current = Зберегти поточний
workspace-screen-shader-hint-before = Вимкнути можна будь-коли через
workspace-workspace-restore-failed = Не вдалося відновити розкладку робочого простору, тож це вікно починає порожнім.

## Tasks window
tasks-acoustic-all-described = Усі скановані треки ({ $count }) описано моделлю { $label }
tasks-acoustic-off = Опис того, як звучать треки, вимкнено в Налаштуваннях, у розділі Медіатека
tasks-acoustic-partial = { $label } описує { $embedded } з { $total } сканованих треків
tasks-analyzing = Аналізуємо { $progress }
tasks-bake-writing = Записуємо теги...
tasks-chip-count = { $count ->
    [one] { $count } завдання
    [few] { $count } завдання
    [many] { $count } завдань
   *[other] { $count } завдання
}
tasks-convert-starting = Запускаємо ffmpeg...
tasks-converting = Конвертуємо { $progress }
tasks-count-of-total = { $done } з { $total }
tasks-embedding = Вписуємо { $progress }
tasks-estimate-at = { $estimate } при { $workers }
tasks-import-failed = Останній імпорт не вдався: { $error }
tasks-import-reading = Читаємо список loved...
tasks-import-unmatched = Без пари в цій медіатеці: { $count }
tasks-importing = Імпортуємо { $progress }
tasks-job-acoustic = Акустичний аналіз
tasks-job-convert = Конвертація звуку
tasks-job-loved-import = Loved-треки Last.fm
tasks-job-replaygain = ReplayGain
tasks-job-scan = Сканування медіатеки
tasks-job-tempo = Аналіз темпу
tasks-last-pass-stopped = Останній прохід спинився: { $reason }
tasks-last-run-finished = Останній запуск завершено, зроблено { $count }
tasks-last-run-stopped = Останній запуск спинився після { $count }
tasks-library-busy = Медіатека зайнята
tasks-library-scanning = Медіатека сканується
tasks-measuring = Міряємо { $progress }
tasks-model-downloading = Модель ще завантажується
tasks-no-library-window = Жодного вікна медіатеки не відкрито, тож звідси це не запустити
tasks-nothing-to-measure = Ще нічого не скановано для вимірювання
tasks-rg-all-gain = Усі треки ({ $count }) мають підсилення, з яким грати
tasks-rg-partial = { $missing } з { $total } треків без підсилення
tasks-scan-folder-count = { $count ->
    [one] { $count } тека
    [few] { $count } теки
    [many] { $count } тек
   *[other] { $count } теки
}
tasks-scan-last-scanned = { $folders }, скановано { $ago } тому
tasks-scan-never-scanned = { $folders }, жодного разу не скановано
tasks-scan-no-folders = Ще не додано жодної теки. Додайте її в Налаштуваннях, у розділі Медіатека
tasks-start-analyze-missing = Проаналізувати те, чого бракує
tasks-start-measure-missing = Виміряти те, чого бракує
tasks-start-rescan = Пересканувати
tasks-stop = Спинити
tasks-stopping = Спиняємо...
tasks-tempo-all = Усі треки ({ $count }) мають темп
tasks-tempo-counted = Треків із темпом: { $count }
tasks-tempo-off = З'ясування того, як швидко йдуть треки, вимкнено в Налаштуваннях, у розділі Медіатека
tasks-tempo-partial = { $missing } з { $total } треків без темпу
tasks-tempo-refused = Треків, у яких rox не розчув ритму: { $count }
tasks-timing = Міряємо час { $progress }
tasks-filling = Заповнення { $progress }
tasks-job-sortnames = Імена для сортування
tasks-sortnames-all = Усі виконавці ({ $count }) мають ім'я для сортування
tasks-sortnames-non-latin = , з них не латиницею: { $count }, { $estimate }
tasks-sortnames-nothing = Ще нічого не скановано для пошуку
tasks-sortnames-partial = { $missing } з { $total } виконавців без імені для сортування
tasks-start-fill-missing = Заповнити те, чого бракує
tasks-job-romanize = Романізація
tasks-reading-takes = , їх читання займе { $estimate }
tasks-romanize-all = Імена для сортування є в усіх { $count } назв, альбомів та виконавців
tasks-romanize-nothing = Ще нічого не скановано для читання
tasks-romanize-partial = { $missing } з { $total } назв, альбомів та виконавців без імені для сортування
tasks-romanizing = Читання { $progress }
tasks-romanize-skipped = Пропущено без японського словника: { $count }
tasks-romanize-skipping = Із них { $kanji } — кандзі, їм потрібен японський словник із Налаштування > Медіатека
tasks-start-romanize = Романізувати
tasks-tip = Відкрити завдання медіатеки
tasks-window-title = rox - Завдання
tasks-working-out-missing = З'ясовуємо, чого бракує...

## Stats window
stats-bars-daily = Стовпці по днях, клацніть один, щоб відкрити день
stats-bars-days = Стовпці по { $days } дн., клацніть один, щоб відкрити період
stats-bars-hourly = Стовпці по годинах, ближче не наблизити
stats-bars-hours = Стовпці по { $hours } год, клацніть один, щоб відкрити його день
stats-bars-weekly = Стовпці по тижнях, клацніть один, щоб відкрити тиждень
stats-bucket-listens = { $count ->
    [one] { $count } прослуховування, { $ago } ({ $date })
    [few] { $count } прослуховування, { $ago } ({ $date })
    [many] { $count } прослуховувань, { $ago } ({ $date })
   *[other] { $count } прослуховування, { $ago } ({ $date })
}
stats-chart-end-day = Північ
stats-chart-start-all = Перше прослуховування
stats-chart-start-month = 30 днів тому
stats-chart-start-week = 7 днів тому
stats-chart-start-year = Рік тому
stats-click-opens = Клік відкриває статистику
stats-click-section = Клік
stats-count-menu = Підрахунок
    .description = За яким останнім проміжком число рахує прослуховування; список при наведенні завжди показує їх усі
stats-empty-all = Ще немає прослуховувань
stats-empty-range = У цьому проміжку немає прослуховувань
stats-library-held = { $tracks } треків, { $size } у пам'яті
stats-now = Зараз
stats-open = Відкрити статистику
stats-open-on-click = Відкривати статистику кліком
    .description = Клікніть віджет, щоб відкрити вікно статистики, повний запис прослуханого
stats-play-these-tracks = Відтворити ці треки
stats-play-this-track = Відтворити цей трек
stats-plays-count = { $count ->
    [one] { $count } прослуховування
    [few] { $count } прослуховування
    [many] { $count } прослуховувань
   *[other] { $count } прослуховування
}
stats-range-all = За весь час
stats-range-all-short = Усе
stats-range-day-short = День
stats-range-label = Проміжок
stats-range-month = Цього місяця
stats-range-month-short = Місяць
stats-range-span = З { $from } по { $to }
stats-range-today = Сьогодні
stats-range-week = Цього тижня
stats-range-week-short = Тиждень
stats-range-year = Цього року
stats-range-year-short = Рік
stats-readout-section = Показник
stats-section-listens = Прослуховування
stats-section-listens-over-time = Прослуховування з часом
stats-section-recent-listens = Недавні прослуховування
stats-section-top-albums = Топ альбомів
stats-section-top-artists = Топ виконавців
stats-section-top-genres = Топ жанрів
stats-show-change = Показувати зміну
    .description = Додати позначку про те, як проміжок виглядає проти попереднього, вгору чи вниз; за весь час позаду нічого немає
stats-show-number = Показувати число
    .description = Малювати число поруч зі значком; вимкнено лишає голий значок, а числа показує при наведенні
stats-title = Віджет статистики
stats-tooltip-listens = Прослуховування
stats-window-title = rox - Статистика

## Library health window

health-caption-art = { $albums } з { $total }, { $tracks }
health-caption-duplicates = { $groups } на { $tracks }
health-caption-formats = { $unwritable } з { $total }
health-caption-gaps = { $albums } з { $total }
health-caption-missing = { $missing } відсутні з { $total }
health-caption-sort = Виконавці альбому { $album_artists }, альбоми { $albums }, назви { $titles }
health-caption-split = { $tagged } з тегами, { $measured } виміряно, { $missing } відсутні
health-caption-split-refused = { $tagged } з тегами, { $measured } виміряно, { $missing } відсутні, { $refused } без ритму
health-checks-menu = Теги, які рахуються
    .description = Які з п'яти основних тегів рахує показник; список при наведенні завжди показує їх усі
health-click-opens = Клік відкриває стан бібліотеки
health-click-section = Клік
health-complete = Пропусків немає
health-count-groups = { $count ->
    [one] { $count } група
    [few] { $count } групи
    [many] { $count } груп
   *[other] { $count } групи
}
health-desc-acoustic = Треки без акустичного відбитка: схоже за звучанням для них не підібрати.
health-desc-art = Альбоми без обкладинки: ні всередині файлів, ні картинкою поруч із ними.
health-desc-duplicates = Групи треків з однаковим виконавцем і назвою та приблизно однаковою тривалістю.
health-desc-gaps = Альбоми, у нумерації треків яких пропущено номер або трек не має номера взагалі.
health-desc-genre = Треки, у файлах яких не вказано жанр.
health-desc-rating = Треки, яким ви ще не поставили оцінку.
health-desc-replaygain = Треки без заміру гучності: вони звучать гучніше або тихіше за решту.
health-desc-sort-names = Скільки імен мають ім'я для сортування, тобто написання, яке визначає місце в алфавітному порядку.
health-desc-tempo = Треки без темпу, і саме його читають сортування та добір за BPM.
health-desc-writable = Треки у форматах, які rox читає, але не може записати в них теги. Фрагментовані файли MP4 теж відмовляють у записі, і тут вони не враховані.
health-desc-year = Треки без року випуску.
health-drill = Показати ці
health-fix-analyze = Проаналізувати те, чого бракує
health-fix-duplicates = Відкрити дублікати
health-fix-genres = Проставити жанри
health-fix-measure = Виміряти те, чого бракує
health-fix-fill = Заповнити те, чого бракує
health-measuring-art = Перевіряємо обкладинки, { $done } з { $total }
health-measuring-duplicates = Шукаємо збіги дублікатів
health-measuring-formats = Читаємо формати файлів
health-measuring-gaps = Перевіряємо номери треків
health-open = Відкрити стан бібліотеки
health-open-on-click = Відкривати стан бібліотеки кліком
    .description = Клікніть віджет, щоб відкрити вікно стану бібліотеки, де розкладено покриття
health-overview-complete = { $complete } з { $total } повністю з тегами
health-overview-missing = { $missing } відсутні
health-readout-section = Показник
health-running = Виконується
health-section-audio = Аудіо
health-section-files = Файли та структура
health-section-overview = Огляд
health-section-tags = Теги
health-show-percent = Показувати відсоток
    .description = Малювати покриття поруч зі значком; вимкнено лишає голий значок, а числа показує при наведенні
health-tile-acoustic = Акустичні вектори
health-tile-album = Альбом
health-tile-art = Обкладинка альбому
health-tile-artist = Виконавець
health-tile-duplicates = Дублікати
health-tile-gaps = Пропуски в альбомах
health-tile-genre = Жанр
health-tile-rating = Оцінка
health-tile-replaygain = ReplayGain
health-tile-sort-names = Імена для сортування
health-tile-tempo = Темп
health-tile-writable = Без підтримки
health-tile-year = Рік
health-tile-title = Назва
health-tooltip-missing = Відсутні теги
health-waiting = Очікування
health-widget-title = Віджет стану
health-window-title = rox - Стан бібліотеки

## Power search window

search-seed-caption = { $source }: { $count }
search-window-title = rox - Розширений пошук

## About window
about-check-failed = Не вдалося дістатися GitHub
about-check-for-updates = Перевірити оновлення
about-checking = Перевіряємо...
about-download = Завантажити
about-downloading = Завантажуємо... { $percent }%
about-get-it = Отримати
about-license-lead = rox - вільне програмне забезпечення під GNU AGPLv3. Вихідний код на
about-notice-lead = Разом із програмою ви мали отримати копію ліцензії. Якщо ні, дивіться
about-release-notes = Нотатки випуску
about-restart-now = Перезапустити зараз
about-up-to-date = У вас найновіша версія
about-update-failed = Оновлення не вдалося: { $error }
about-version = Версія { $version }
about-version-available = Доступна версія { $version }
about-version-ready = Версія { $version } готова
about-window-title = rox - Про програму

## Welcome window
welcome-add-folder = Додати теку
welcome-and = і
welcome-back = Назад
welcome-card-menubar-title = Смуга меню
welcome-card-music-title = Музика
welcome-card-panels-title = Панелі
welcome-card-playback-title = Відтворення
welcome-card-rearranging-title = Перестановка
welcome-card-settings-title = Налаштування
welcome-close = Закрити
welcome-design-mode-note = Для перестановки потрібен Режим дизайну, типово ввімкнений угорі того меню. Вимкнений замикає розкладку, тож готового налаштування нічим не зрушити.
welcome-done = Готово
welcome-drop-note = Киньте її на край панелі, щоб поділити там, на середину, щоб стати в одну групу вкладок, або поза вікно, щоб зробити з неї власне вікно.
welcome-key-left-click = Лівий клік
welcome-key-middle-mouse = Середня кнопка
welcome-layout-note = Збережіть розстановку як розкладку; робочий простір збирає розкладки й палітру в один вигляд, яким можна поділитися.
welcome-menubar-after = двічі, щоб вона лишилася.
welcome-menubar-before = Коли смугу меню приховано, затисніть
welcome-menubar-mid = щоб підняти її над доком, або натисніть
welcome-music-note = rox сканує її в медіатеку, а файли лишаються там, де вони є. Більше тек - у налаштуваннях, у розділі медіатеки.
welcome-next = Далі
welcome-or = або
welcome-panels-note = Кожна поверхня - це панель, а меню Панелі на смузі меню відкриває їх більше.
welcome-playback-after = перемотують.
welcome-playback-before = перемикає відтворення;
welcome-quickplay-after = і він заграє.
welcome-quickplay-before = відкриває швидкий запуск: наберіть трек, натисніть
welcome-rearrange-after = будь-де в панелі, щоб її пересунути.
welcome-rearrange-before = Перетягніть вкладку або затисніть
welcome-settings-hint-after = відкриває налаштування: палітру, прозорість і поведінку.
welcome-shelf-caption = Вибір одного замінює вигляд головного вікна й закриває тур. Це вікно доступне будь-коли через Застосунок > Вітання.
welcome-stage-lead-quick-start = Виберіть робочий простір, і головне вікно перемкнеться на нього: розкладки, палітра, увесь вигляд.
welcome-stage-lead-welcome = Foobar, якби його зробили у 20XX.
welcome-stage-title-quick-start = Швидкий старт
welcome-stage-title-welcome = Вітаємо в rox
welcome-step-hint-after = , або кнопками нижче.
welcome-step-hint-before = Крокуйте по ньому через
welcome-tile-by = від { $author }
welcome-tour-intro = Короткий тур по тому, звідки береться музика і де налаштовується вигляд. Він закінчується полицею з готовими робочими просторами, по одному кліку кожен.
welcome-window-title = rox - Вітання

## Console window
console-clear = Очистити
console-copy = Копіювати
console-empty-filtered = Нічого на цих рівнях
console-empty-none = Ще нічого не записано
console-filter-error = Помилки
console-filter-info = Інфо
console-filter-warn = Попередження
console-follow = Стежити
console-line-count = { $count ->
    [one] { $count } рядок
    [few] { $count } рядки
    [many] { $count } рядків
   *[other] { $count } рядка
}
console-open-button = Відкрити консоль
console-reveal = Показати
console-window-title = rox - Консоль

## Signals window
signals-about-toggle = Про сигнали
signals-blurb-marked = Панелі, позначені цим у меню, можуть прив'язати більшість своїх параметрів: клікніть параметр у налаштуваннях панелі правою кнопкою й виберіть сигнал або додайте його звідти.
signals-blurb-shared = Налаштоване тут - спільне: зміна діє на кожен параметр, змаршрутований на цей сигнал, у кожній панелі й кожному вікні.
signals-blurb-total = Сума - це четвертий вид: вона накопичує інший сигнал із часом і перекидається через 1, тож росте, поки музика гучна, і завмирає, поки ні. Беріть її, коли шейдеру потрібна фаза, яка йде за піснею, а не за годинником.
signals-blurb-what = Сигнал перетворює те, що грає, на одне число між 0 і 1: енергію в смузі частот, рівень усього міксу або імпульс на кожному ударі всередині смуги. Відгук задає, як швидко він іде слідом, Поріг глушить його нижче за вибраний вами рівень.
signals-no-library = Жодного вікна медіатеки не відкрито, тож тут немає звуку. Правки все одно зберігаються.
signals-window-title = rox - Сигнали

## Equaliser
eq-analyzer-bars = Смуги
eq-analyzer-off = Без аналізатора
eq-analyzer-wave = Хвиля
eq-band-badge = Позначка смуг
    .description = Показувати, скільки смуг зсунуто з нуля, на позначці над значком
eq-band-label = Смуга { $number }
eq-click-nothing = Нічого
eq-click-open = Відкрити
eq-click-section = Клік
    .description = Що робить клік: відкриває вікно еквалайзера або вмикає й вимикає всю криву просто на місці
eq-click-toggle = Перемкнути
eq-flatten = Вирівняти
eq-freq-label = Частота
eq-gain-label = Підсилення
eq-heading = Еквалайзер
eq-help-text = Тягніть смугу, щоб її пересунути, крутіть колесо над нею, щоб розширити чи звузити. Обробка працює перед буфером, який подає звук на карту, тож рухові треба до пів секунди, щоб дійти до колонок.
eq-hint-off = Клікніть, щоб вимкнути
eq-hint-on = Клікніть, щоб увімкнути
eq-hint-open = Клікніть, щоб відкрити еквалайзер
eq-open = Відкрити еквалайзер
eq-readout-curve = Крива
eq-readout-icon = Значок
eq-readout-section = Показник
    .description = Значок, крива відгуку як спарклайн або обидва. Кривій треба близько п'ятдесяти пікселів ширини, щоб її можна було читати
eq-reset-bands = Скинути смуги
eq-shape-active = { $count ->
    [one] { $count } смуга не на нулі, пік { $peak } дБ
    [few] { $count } смуги не на нулі, пік { $peak } дБ
    [many] { $count } смуг не на нулі, пік { $peak } дБ
   *[other] { $count } смуги не на нулі, пік { $peak } дБ
}
eq-shape-flat = Рівно, кожна смуга на 0 дБ
eq-status-off = Еквалайзер вимкнено
eq-status-on = Еквалайзер увімкнено
eq-title = Віджет еквалайзера
eq-widget-section = Віджет
eq-width-label = Ширина
eq-window-title = rox - Еквалайзер

## Keymap
keymap-close-window = Закрити вікно
    .description = Закрити те вікно, що спереду. Прив'язано всюди, зокрема й до відділених панелей
keymap-decrease-font-size = Зменшити розмір тексту
    .description = Зсунути розмір тексту всього застосунку на крок вниз
keymap-focus-search = Фокус на пошук
    .description = Поставити курсор у поле пошуку медіатеки
keymap-group-browsing = Навігація
keymap-group-editing = Редагування
keymap-group-library = Медіатека
keymap-group-playback = Відтворення
keymap-group-view = Вигляд
keymap-group-windows = Вікна
keymap-increase-font-size = Збільшити розмір тексту
    .description = Зсунути розмір тексту всього застосунку на крок вгору
keymap-key-backspace = Backspace
keymap-key-delete = Delete
keymap-key-down = Вниз
keymap-key-end = End
keymap-key-esc = Esc
keymap-key-home = Home
keymap-key-insert = Insert
keymap-key-left = Ліворуч
keymap-key-page-down = Page Down
keymap-key-page-up = Page Up
keymap-key-right = Праворуч
keymap-key-space = Пробіл
keymap-key-tab = Tab
keymap-key-up = Вгору
keymap-mod-alt = Alt
keymap-mod-cmd = Cmd
keymap-mod-ctrl = Ctrl
keymap-mod-fn = Fn
keymap-mod-option = Option
keymap-mod-shift = Shift
keymap-mod-super = Super
keymap-mod-win = Win
keymap-new-window = Нове вікно
    .description = Відкрити ще одне робоче вікно зі збереженою розкладкою
keymap-next-track = Наступний трек
    .description = Перейти до наступного треку в черзі
keymap-open-about = Про програму
    .description = Показати версію та подяки
keymap-open-console = Консоль
    .description = Відкрити вікно журналу
keymap-open-equalizer = Еквалайзер
    .description = Відкрити вікно еквалайзера
keymap-open-quick-play = Швидкий запуск
    .description = Підняти над вікном рядок пошуку й відтворення
keymap-open-settings = Відкрити налаштування
    .description = Відкрити це вікно
keymap-open-panel-settings = Налаштування панелі
    .description = Відкриває вікно налаштувань активної панелі
keymap-open-health = Стан бібліотеки
    .description = Відкрити вікно стану бібліотеки, де підраховуються охоплення тегів і структурні проблеми
keymap-open-power-search = Розширений пошук
    .description = Відкрити вікно пошуку з власним запитом, щоб пошук тут не змінював робочий простір
keymap-open-stats = Відкрити статистику
    .description = Відкрити вікно статистики прослуханого
keymap-open-tasks = Завдання
    .description = Показати, чим rox зайнятий у фоні
keymap-open-welcome = Вітання
    .description = Знову відкрити вікно вітання
keymap-play-random = Випадковий трек
    .description = Узяти випадковий трек із фонотеки й увімкнути його
keymap-previous-track = Попередній трек
    .description = Повернутися до попереднього треку
keymap-quit = Вийти
    .description = Покинути rox. Прив'язано всюди, бо немає вікна, з якого це не мало б працювати
keymap-reset-font-size = Скинути розмір тексту
    .description = Повернути розмір тексту до стандартного
keymap-seek-backward = Перемотати назад
    .description = Крок назад по треку, що грає
keymap-seek-forward = Перемотати вперед
    .description = Крок вперед по треку, що грає
keymap-stamp-line = Позначити рядок тексту
    .description = Записати позицію відтворення в рядок тексту, який редагується
keymap-stop-playback = Зупинити
    .description = Зупинити відтворення і звільнити трек
keymap-toggle-playback = Відтворити / Пауза
    .description = Запустити поточний трек або спинити його там, де він є
keymap-toggle-post-shader = Перемкнути шейдер накладки
    .description = Вимкнути й увімкнути екранний шейдер. Прив'язано всюди, бо шейдер може сховати під собою ті елементи, якими його інакше було б вимкнути
keymap-toggle-zoom = Збільшити групу панелей
    .description = Заповнити док групою панелей, яку клікнули останньою, або вийти з неї
keymap-type-ahead-next = Наступний збіг
    .description = Перейти до наступного рядка, що збігається з набраним
keymap-type-ahead-prev = Попередній збіг
    .description = Повернутися до попереднього збігу з набраним
keymap-next-tab = Наступна вкладка
    .description = Показує наступну вкладку в активній групі панелей
keymap-prev-tab = Попередня вкладка
    .description = Показує попередню вкладку в активній групі панелей
keymap-toggle-mute = Вимкнути звук
    .description = Вимикає вивід, не скидаючи рівень. Натисніть ще раз, щоб повернути його
keymap-toggle-shuffle = Перемкнути перемішування
    .description = Вмикає або вимикає перемішування черги
keymap-cycle-loop = Перемкнути повтор
    .description = Перемикає повтор з вимкненого на всі, на один і знову по колу
keymap-toggle-stop-after = Зупинити після
    .description = Дає поточному треку дограти, потім ставить паузу. Натисніть ще раз, щоб скасувати
keymap-volume-up = Гучніше
    .description = Підвищує гучність на один крок
keymap-volume-down = Тихіше
    .description = Знижує гучність на один крок
keymap-close-panel = Закрити панель
    .description = Закриває активну панель в активній групі панелей
keymap-new-empty-window = Порожнє вікно
    .description = Відкриває вікно робочого простору без вмісту
keymap-open-signals = Сигнали
    .description = Відкриває вікно сигналів, спільний пул за маршрутами кожної панелі
keymap-import-workspace = Імпортувати робочий простір
    .description = Вибирає файл робочого простору та додає його до колекції
keymap-toggle-quit-to-tray = Перемкнути «Лишатися в лотку»
    .description = Перемикає, чи лишається rox у лотку після закриття останнього вікна
keymap-toggle-design-mode = Перемкнути режим дизайну
    .description = Перемикає, чи можна переставляти панелі прямо в розкладці
keymap-toggle-theme = Перемкнути світлу / темну
    .description = Перемикає на інший бік палітри. Діє всюди, бо всі вікна ділять одну тему
keymap-toggle-resize-lock = Перемкнути блокування зміни розміру панелей
    .description = Перемикає, чи доступна зміна розміру панелей лише в режимі дизайну
keymap-toggle-menubar = Перемкнути приховування смуги меню
    .description = Показує смугу меню у вікні або ховає її, поки не утримується Alt
keymap-toggle-decorations = Перемкнути оформлення ОС
    .description = Перемикає вікна робочого простору між рамкою ОС і власною рамкою rox
keymap-toggle-art-theming = Перемкнути тему пісні
    .description = Перемикає, чи обкладинка поточного треку забарвлює палітру
keymap-rescan-library = Пересканувати медіатеку
    .description = Заново просканувати всі запам’ятовані теки медіатеки
keymap-measure-replaygain = Виміряти ReplayGain
    .description = Відкрити діалог, який вимірює гучність треків без неї
keymap-analyze-tempo = Проаналізувати темп
    .description = Відкрити діалог, який слухає біт у треках без BPM
keymap-build-acoustic = Побудувати акустичні вектори
    .description = Відкрити діалог, який будує вектори для акустичного пошуку
keymap-fill-sort-names = Заповнити імена для сортування
    .description = Відкрити діалог, який запитує в MusicBrainz імена для сортування, яких немає у файлах
keymap-romanize-library = Романізувати медіатеку
    .description = Відкрити діалог, який читає латинкою нелатинські назви, альбоми та імена виконавців
keymap-find-duplicates = Знайти дублікати
    .description = Відкрити пошук дублікатів у медіатеці
keymap-tag-genres = Проставити жанри
    .description = Відкрити простановку жанрів за треками без жанру

## Panel catalog
panel-catalog-album-carousel = Карусель альбомів
panel-catalog-artist-grid = Сітка виконавців
panel-catalog-biography = Біографія
panel-catalog-cover-art = Обкладинка
panel-catalog-drawer = Шухляда
panel-catalog-eq-widget = Віджет еквалайзера
panel-catalog-filter = Фільтр
panel-catalog-folder-tree = Дерево тек
panel-catalog-genre-grid = Сітка жанрів
panel-catalog-health-widget = Віджет стану
panel-catalog-group-application = Застосунок
panel-catalog-group-arrangement = Розстановка
panel-catalog-group-catalogue = Каталог
panel-catalog-group-controls = Керування
panel-catalog-group-details = Подробиці
panel-catalog-group-experimental = Експериментальні
panel-catalog-group-visualizers = Візуалізації
panel-catalog-history = Історія
panel-catalog-menu = Меню
panel-catalog-metadata = Метадані
panel-catalog-mini-toggle = Перемикач міні
panel-catalog-oscilloscope = Осцилограф
panel-catalog-overlay = Накладка
panel-catalog-particles = Частинки
panel-catalog-playlists = Списки відтворення
panel-catalog-queue = Черга
panel-catalog-queue-widget = Віджет черги
panel-catalog-seek = Перемотка
panel-catalog-slide = Слайд
panel-catalog-spectrogram = Спектрограма
panel-catalog-spectrum = Спектр
panel-catalog-stats-widget = Віджет статистики
panel-catalog-status = Статус
panel-catalog-theme-toggle = Перемикач теми
panel-catalog-track-info = Про трек
panel-catalog-vu-meter = Індикатор VU
panel-catalog-waveform = Форма хвилі
panel-catalog-window-controls = Кнопки вікна

## Updater
updater-already-latest = уже найновіша версія
updater-checksum-mismatch = контрольна сума завантаженого - { $digest }, а не { $expected }, яку заявляє випуск
updater-checksum-missing-entry = у { $sums } немає запису для { $name }; відмовляємося від завантаження, яке не перевірити
updater-no-asset = у випуску немає { $name }
updater-no-checksums = у випуску немає { $sums }; відмовляємося від завантаження, яке не перевірити
updater-no-release-build = для цієї платформи немає збірки випуску
updater-overran = завантаження вийшло за розмір, який заявляє випуск
updater-short = завантаження спинилося на { $done } з { $bytes } байтів
updater-size-mismatch = сервер запропонував { $claimed } байтів, випуск заявляє { $bytes }

## Last.fm
lastfm-import-matching = Звіряємо з медіатекою
lastfm-import-read = Прочитано loved-треків: { $count }
lastfm-import-stopped = Спинилося, прочитано loved-треків: { $count }
lastfm-import-matched = , зіставлено: { $count }
lastfm-import-added = , додано в улюблене: { $count }

## Tag tools
tags-editor-add-tag = Додати
tags-editor-clear-all = очистити все
tags-editor-form-view = Форма
tags-editor-format-unsupported-all = Теги цього формату поки що не читаються й не записуються.
tags-editor-format-unsupported-some = Частина цих файлів у форматі, чиї теги поки що не читаються й не записуються.
tags-editor-guess-button = Вгадати
tags-editor-guess-folded = { $status }, ще { $count } не показано
tags-editor-guess-help = { $placeholders }; / збігається з текою вище, %skip% відкидає
tags-editor-guess-match-count = Збігів: { $hits } з { $total }
tags-editor-guess-no-match = немає збігу
tags-editor-guess-pattern-label = Шаблон
tags-editor-loading = Читаємо теги...
tags-editor-look-up = Знайти
tags-editor-multiple-values = Кілька значень
tags-editor-clear-on-save = Очиститься при збереженні
tags-editor-additional-tags = Додаткові теги ({ $count })
tags-editor-remove = прибрати
tags-editor-reveal = Показати
tags-editor-save-errors = Файлів з помилкою: { $count }; { $error }
tags-editor-saving-progress = Зберігаємо { $done }/{ $total }...
tags-editor-sort-names = Імена для сортування
tags-editor-table-view = Таблиця
tags-editor-tag-columns = Додаткові теги
tags-editor-tag-field-conflict = поле { $field } записує цей тег
tags-editor-tag-key-placeholder = Назва тега
tags-editor-tag-value-placeholder = Значення
tags-editor-tags-section = Теги
tags-editor-unknown-partial = { $count } з { $total }
tags-editor-unread-count = Не вдалося прочитати теги { $failed } з { $total } файлів
tags-editor-will-clear = буде очищено
tags-editor-will-remove = буде прибрано
tags-editor-window-title = rox - Редактор тегів
tags-guess-empty-segment = шаблон дає порожню назву теки або файлу
tags-guess-no-placeholders = немає підстановок
tags-guess-skip-renders-nothing = %skip% нічого не дає
tags-guess-unclosed = незакритий %
tags-guess-unknown-placeholder = невідома підстановка %{ $name }%
tags-matcher-blocked-arm = Увімкніть поле, щоб застосувати
tags-matcher-blocked-no-match = Немає збігу, який застосувати
tags-matcher-blocked-pick = Виберіть збіг
tags-matcher-blocked-writing = Записуємо теги...
tags-matcher-match-count = { $count ->
    [one] { $count } збіг
    [few] { $count } збіги
    [many] { $count } збігів
   *[other] { $count } збіга
}
tags-matcher-no-matches = Збігів не знайдено
tags-matcher-pick-match = Виберіть збіг
tags-matcher-search-failed = Пошук не вдався: { $error }
tags-matcher-searching = Шукаємо...
tags-matcher-tagging = Теги для { $track }
tags-matcher-window-title = rox - Пошук метаданих
tags-rename-blocked-cue = трек із cue, власного файлу немає
tags-rename-blocked-duplicate = два треки лягають на цю назву
tags-rename-blocked-occupied = там уже є файл
tags-rename-blocked-outside-roots = поза всіма коренями медіатеки
tags-rename-blocked-unresolved = ще не в каталозі
tags-rename-move-error = { $name }: { $error }
tags-rename-move-errors = Файлів з помилкою: { $count }; { $error }
tags-rename-moving = Переносимо { $done }/{ $total }...
tags-rename-nothing-to-move = Немає чого переносити
tags-rename-pattern-help = { $placeholders }; / робить теку, розширення йде за файлом
tags-rename-pattern-section = Шаблон
tags-rename-preview-section = Перегляд
tags-rename-unchanged = без змін
tags-rename-will-move = { $count } з { $total } буде перенесено
tags-rename-window-title = rox - Перейменування файлів
tags-repair-affected-files = Зачеплені файли
tags-repair-section = Лагодження
tags-repair-check-to-repair = Позначте файл, щоб його полагодити
tags-repair-count = { $count ->
    [one] { $count } файл
    [few] { $count } файли
    [many] { $count } файлів
   *[other] { $count } файла
}
tags-repair-count-so-far = { $count } поки що
tags-repair-label-scope = обсяг
tags-repair-no-affected = Зачеплених файлів не знайдено.
tags-repair-no-folder = Немає теки для сканування; додайте її в медіатеку або виберіть.
tags-repair-pick-folder = Вибрати теку...
tags-repair-progress = Лагодимо { $done }/{ $total }...
tags-repair-repair-button = { $count ->
    [0] Полагодити
   *[other] Полагодити ({ $count })
}
tags-repair-result = { $count ->
    [one] Полагоджено { $count } файл
    [few] Полагоджено { $count } файли
    [many] Полагоджено { $count } файлів
   *[other] Полагоджено { $count } файла
}
tags-repair-result-failed = Полагоджено { $count }, не вдалося { $failed }
tags-repair-scan-first = Спершу скануйте
tags-repair-scan-hint = Скануйте, щоб знайти файли з пошкодженими тегами, які лагодить перезапис.
tags-repair-select-all = Вибрати все
tags-repair-select-none = Зняти вибір
tags-repair-whole-library = Уся медіатека
tags-repair-window-title = rox - Лагодження тегів

## Convert
convert-arg-names-file = «{ $token }» називає файл; призначення береться з теки й шаблону
convert-section-output = Вихід
convert-section-preview = Перегляд
convert-arg-not-flag-or-value = «{ $token }» - це не прапорець і не значення до нього
convert-check-wrote-nothing = ffmpeg вийшов чисто, але нічого не записав
convert-custom-ext-empty = Саме розширення задає контейнер, тож воно потрібне
convert-custom-ext-invalid = «{ $ext }» - не назва контейнера; літери й цифри, без крапки
convert-dialog-browse = Огляд...
convert-dialog-check-passed = ffmpeg закодував із цим мить тиші, тож воно працює
convert-dialog-check-waiting = Перевіримо через ffmpeg, щойно ви перестанете набирати
convert-dialog-checking = Перевіряємо через ffmpeg...
convert-dialog-choose-folder = Виберіть теку для запису
convert-dialog-convert-button = Конвертувати
convert-dialog-custom-label = Власний
convert-dialog-custom-menu-item = Власний...
convert-dialog-custom-note = Аргументи діляться по пробілах, тож без лапок; вбудовані обкладинки для власних форматів не копіюються
convert-dialog-format-not-ready = Набраний формат ще не пройшов через ffmpeg
convert-dialog-label-extension = розширення
convert-dialog-label-format = формат
convert-dialog-label-into = у
convert-dialog-label-named = з назвою
convert-dialog-mirror = Дзеркалити теки медіатеки
convert-dialog-nothing-to-convert = Немає чого конвертувати: кожен рядок пропущено
convert-dialog-pattern-help = { $placeholders }; / робить теку, розширення задає формат
convert-dialog-pick-folder = Оберіть теку для запису
convert-dialog-span-note = { $count } вирізано з cue-образу й протеговано з медіатеки
convert-dialog-will-convert = { $count } з { $total } буде сконвертовано
convert-dialog-window-title = rox - Конвертація
convert-ffmpeg-silent-failure = ffmpeg упав, не сказавши чому
convert-flag-attach = -attach читає власний файл, а це тут не дозволено
convert-flag-f = Розширення задає контейнер, тож -f виставляти не вам
convert-flag-i = Вхід - це вибраний вами трек, тож -i виставляти не вам
convert-flag-n = -n і так стоїть на кожному запуску
convert-flag-y = Тут ніщо не перезаписує, тож -y недоступний; наявне призначення пропускається
convert-preset-flac = FLAC
convert-preset-mp3-320 = MP3 320 кбіт/с
convert-preset-mp3-v0 = MP3 V0
convert-preset-opus-192 = Opus 192 кбіт/с
convert-preset-wav = WAV
convert-skip-duplicate = два треки лягають на цю назву
convert-skip-exists = уже є
convert-summary-failed = , не вдалося { $count }
convert-summary-files = { $count ->
    [one] { $count } файл
    [few] { $count } файли
    [many] { $count } файлів
   *[other] { $count } файла
}
convert-summary-line = { $files } у { $dest }
convert-summary-skipped = , пропущено { $count }
convert-summary-stopped = Спинилося, сконвертовано { $files } у { $dest }
convert-version-answered = { $binary } запустився, але не повідомив версію

## Duplicates
duplicates-auto-select = Вибрати автоматично
duplicates-check-to-trash = Позначте копії, щоб викинути їх у смітник
duplicates-copy-count = { $count ->
    [one] { $count } копія
    [few] { $count } копії
    [many] { $count } копій
   *[other] { $count } копії
}
duplicates-different-albums = різні альбоми
duplicates-filter-placeholder = Фільтр за назвою, виконавцем або текою
duplicates-groups-summary = { $groups ->
    [one] { $groups } група, зайвих копій: { $extras }
    [few] { $groups } групи, зайвих копій: { $extras }
    [many] { $groups } груп, зайвих копій: { $extras }
   *[other] { $groups } групи, зайвих копій: { $extras }
}
duplicates-library-loading = Медіатека ще завантажується; спробуйте трохи згодом.
duplicates-no-duplicates = Дублікатів не знайдено.
duplicates-no-filter-matches = Жодна група не збігається з фільтром.
duplicates-policy-newest = Лишати найновіші
duplicates-policy-oldest = Лишати найстаріші
duplicates-policy-quality = Лишати найкращі за якістю
duplicates-scan-hint = Проскануйте медіатеку на треки, які трапляються більше ніж раз.
duplicates-select-none = Зняти вибір
duplicates-selected-count = вибрано: { $count }
duplicates-trash-button = { $count ->
    [0] У смітник
   *[other] У смітник ({ $count })
}
duplicates-trash-error = { $name }: { $error }
duplicates-trash-result = { $count ->
    [one] Перенесено в смітник { $count } файл
    [few] Перенесено в смітник { $count } файли
    [many] Перенесено в смітник { $count } файлів
   *[other] Перенесено в смітник { $count } файла
}
duplicates-trash-result-failed = Перенесено в смітник { $count }, не вдалося { $failed }
duplicates-trashing = Переносимо в смітник { $done }/{ $total }...
duplicates-window-title = rox - Дублікати

## Простановка жанрів

tag-genres-empty = У кожного треку є жанр. Увімкніть щось, щоб позначити заново.
tag-genres-heading = Проставити жанри
tag-genres-input-placeholder = Введіть жанр
tag-genres-keys-hint = 1-8 обирають рядок, Shift+1-8 додають його в поле, Ctrl+1-8 застосовують його до всього альбому, Enter застосовує набране, L запитує Last.fm, S пропускає, Ctrl+Z скасовує
tag-genres-library-loading = Медіатека ще завантажується; спробуйте трохи згодом.
tag-genres-no-file = У медіатеці немає файлу для цього треку.
tag-genres-no-suggestions = Немає що запропонувати; введіть жанр.
tag-genres-progress = { $at } з { $total } без жанру
tag-genres-skip = Пропустити
tag-genres-thinking = Читаємо сусідів...
tag-genres-undo = Скасувати
tag-genres-unwritable = Цей трек лежить усередині спільного cue-образу, тож його жанр не записати. Пропустіть його.
tag-genres-window-title = rox - Проставити жанри
tag-genres-looking-up = Питаємо Last.fm...
tag-genres-lookup = Знайти на Last.fm
tag-genres-auto-lookup = Автоматично
tag-genres-lookup-found = Last.fm позначає { $artist } як: { $tags }
tag-genres-lookup-none = У Last.fm немає тегів для { $artist }.
tag-genres-lookup-off = Онлайн-пошук виконавців вимкнено в налаштуваннях.
tag-genres-why-acoustic = { $count ->
    [one] { $count } трек зі схожим звучанням
    [few] { $count } треки зі схожим звучанням
    [many] { $count } треків зі схожим звучанням
   *[other] { $count } треки зі схожим звучанням
}
tag-genres-why-album = { $count ->
    [one] { $count } трек у цьому альбомі
    [few] { $count } треки у цьому альбомі
    [many] { $count } треків у цьому альбомі
   *[other] { $count } треки у цьому альбомі
}
tag-genres-why-artist = { $count ->
    [one] { $count } трек цього виконавця
    [few] { $count } треки цього виконавця
    [many] { $count } треків цього виконавця
   *[other] { $count } треки цього виконавця
}
tag-genres-why-lookup = Last.fm
tag-genres-album-too = { $count ->
    [one] Позначити весь альбом треку і { $count } сусідній трек
    [few] Позначити весь альбом треку і { $count } сусідні треки
    [many] Позначити весь альбом треку і { $count } сусідніх треків
   *[other] Позначити весь альбом треку і { $count } сусіднього треку
}
tag-genres-apply = Застосувати
tag-genres-begin = Запустити чергу
tag-genres-col-genre = Жанр
tag-genres-col-match = Збіг
tag-genres-col-why = Чому
tag-genres-current-genre = Жанр: { $genre }
tag-genres-idle = Нічого не грає. Запустіть чергу, щоб пройти треки без жанру, або увімкніть щось, щоб позначити заново.
tag-genres-no-genre = Жанру ще немає
tag-genres-stop = Зупинити чергу
tag-genres-untagged-count = { $count ->
    [one] { $count } трек без жанру
    [few] { $count } треки без жанру
    [many] { $count } треків без жанру
   *[other] { $count } треку без жанру
}
tag-genres-write-error = { $name }: { $error }
tag-genres-writing = Запис { $done } з { $total }...

## Smart playlists
smart-playlist-descending = За спаданням
smart-playlist-edit-title = Редагувати розумний список
smart-playlist-limit-label = Обмеження
smart-playlist-limit-placeholder = Без обмеження
smart-playlist-match-count = { $count ->
    [one] { $count } трек збігається
    [few] { $count } треки збігаються
    [many] { $count } треків збігається
   *[other] { $count } трека збігається
}
smart-playlist-matched-tracks = Треки, що збіглися
smart-playlist-new-title = Новий розумний список
smart-playlist-no-matches = Жоден трек не збігається
smart-playlist-query-label = Запит
smart-playlist-sort-default = Типовий порядок
smart-playlist-sort-added = Додано
smart-playlist-sort-label = Сортування
smart-playlist-unknown-field = «{ $field }:» - це не поле, тож термін збігається як звичайний текст
smart-playlist-window-title = rox - { $verb }

## Playlist creation
playlist-create-not-savable = Назвіть список, щоб його зберегти
playlist-create-placeholder = Назва списку
playlist-create-rename-title = Перейменувати список
playlist-create-title = Новий список відтворення
playlist-create-window-title = rox - { $verb }

## Cover tools
cover-art-back = Зворот
cover-art-disc = Диск
cover-art-front = Лице
cover-artwork = Зображення
    .description = Яку картинку показувати; слот, якого у файлі немає, відкочується до лицьової обкладинки
cover-disc-style = Стиль диска
    .description = Оформити зображення як CD або як етикетку вінілової платівки
cover-disc-off = Вимк.
cover-disc-cd = CD
cover-disc-vinyl = Вініл
cover-editor-choose-image = Вибрати зображення
cover-editor-multiple = Кілька
cover-editor-none = Немає
cover-editor-not-an-image = Цей файл - не зображення, яке rox може вбудувати
cover-editor-not-decoded = Це зображення не вдалося декодувати
cover-editor-reading = Читаємо поточну обкладинку...
cover-editor-remove = Прибрати
cover-editor-replace = Замінити
cover-editor-revert = Повернути як було
cover-editor-save-errors = Файлів з помилкою: { $count }; { $error }
cover-editor-saving-progress = Зберігаємо { $done }/{ $total }...
cover-editor-search-online = Шукати онлайн
cover-editor-section = Обкладинка
cover-editor-slot-back = Задня обкладинка
cover-editor-slot-front = Лицьова обкладинка
cover-editor-slot-media = Носій
cover-editor-will-remove = Буде прибрано
cover-editor-window-title = rox - Обкладинка
cover-matcher-blocked-fetching = Тягнемо повне зображення...
cover-matcher-blocked-no-cover = Немає обкладинки, яку поставити
cover-matcher-blocked-pick = Виберіть обкладинку, щоб її поставити
cover-matcher-cover-count = { $count ->
    [one] { $count } обкладинка
    [few] { $count } обкладинки
    [many] { $count } обкладинок
   *[other] { $count } обкладинки
}
cover-matcher-editor-closed = Редактор обкладинок було закрито
cover-matcher-no-covers = Обкладинок не знайдено
cover-matcher-search-failed = Пошук не вдався: { $error }
cover-matcher-set-cover = Поставити обкладинку
cover-matcher-setting = Ставимо...
cover-matcher-tile-info = { $provider }  { $width }px
cover-matcher-unsupported-format = Формат зображення не підтримується
cover-matcher-window-title = rox - Пошук обкладинок
cover-spin = Обертання
    .description = Крутити диск, поки грає трек; діє для слота диска або для стилю диска
cover-spin-disc = Крутити диск
cover-spin-ramp = Розгін
    .description = Скільки диску треба, щоб набрати повну швидкість і щоб зупинитися
cover-spin-speed = Швидкість обертання
    .description = Повна швидкість, обертів на хвилину
cover-stretch = Розтягнути
    .description = Заповнити панель, не зважаючи на співвідношення сторін зображення
cover-stretch-to-fill = Розтягнути на всю
cover-title = Обкладинка

## Lyrics
lyrics-always-centered = Завжди по центру
    .description = Додати відступи з країв, щоб перший і останній рядки теж могли стати по центру
lyrics-auto-search = Автопошук
    .description = Шукати онлайн для треку без слів і зберігати впевнений збіг, без вибору
lyrics-bold = Жирний
lyrics-build-word-by-word = Збирати слово за словом
    .description = Відкривати слова в міру того, як їх співають, як у караоке; неспівані рядки лишаються прихованими
lyrics-edge-bottom = Знизу
lyrics-edge-top = Згори
lyrics-edit-hint-after-stamp = щоб позначити
lyrics-edit-hint-or = або
lyrics-edit-loading = Читаємо аркуш...
lyrics-edit-lyrics = Редагувати текст
lyrics-edit-saving = Зберігаємо...
lyrics-edit-section = Текст пісні
lyrics-edit-stamp = Позначити
lyrics-edit-stamp-time = Позначити { $time }
lyrics-edit-window-title = rox - Редагування тексту
lyrics-fade-lines-in = Проявляти рядки
    .description = Виводити рядок із тьмяного, коли він стає активним
lyrics-falloff-edge = Бік згасання
    .description = З якого боку від активного рядка згасання тьмянить
lyrics-find-online = Знайти текст онлайн...
lyrics-follow-playback = Стежити за відтворенням
    .description = Плавно вести активний рядок до середини, поки грає синхронізований аркуш
lyrics-font = Шрифт
    .description = Гарнітура тексту пісні; типова йде за шрифтом застосунку
lyrics-gap-threshold = Поріг паузи
    .description = Скільки має тривати вступ чи пауза, перш ніж вона дістане перепочинок
lyrics-lead-in-rest = Перепочинок на вступі
    .description = Показувати порожній перепочинок перед довгим вступом, щоб перший рядок проявився, коли він настане
lyrics-line-falloff = Згасання рядків
    .description = Наскільки кожен рядок тьмяніє з кожним кроком від активного
lyrics-line-spacing = Міжрядковий інтервал
    .description = Наскільки далеко стоять один від одного синхронізовані рядки, кратно розміру тексту
lyrics-look-again = Шукати знову
lyrics-mark-dots = Крапки
lyrics-mark-note = Нота
lyrics-marked-notice = Позначено: без тексту
lyrics-matcher-blocked-no-match = Немає збігу, який застосувати
lyrics-matcher-blocked-pick = Виберіть збіг, щоб застосувати
lyrics-matcher-blocked-saving = Зберігаємо слова...
lyrics-matcher-match-count = { $count ->
    [one] { $count } збіг
    [few] { $count } збіги
    [many] { $count } збігів
   *[other] { $count } збіга
}
lyrics-matcher-no-query = У цього треку немає виконавця й назви, за якими шукати збіг
lyrics-matcher-pick-preview = Виберіть збіг, щоб переглянути
lyrics-matcher-search-failed = Пошук не вдався: { $error }
lyrics-matcher-synced-tag = { $provider }  синхронізовано
lyrics-matcher-window-title = rox - Пошук тексту
lyrics-no-lyrics-notice = Немає тексту
lyrics-no-lyrics-track = Для цього треку немає тексту
lyrics-rest-in-gaps = Перепочинок у паузах
    .description = Переходити на порожній перепочинок у довгому інструментальному програші замість того, щоб тримати останній рядок
lyrics-rest-marker = Позначка перепочинку
    .description = Що показує безслівний рядок у синхронізованому аркуші: паузи й порожні рядки
lyrics-search-button = Кнопка пошуку онлайн
    .description = Показувати кнопку пошуку на порожній панелі; меню правої кнопки все одно знаходить текст
lyrics-search-online = Шукати онлайн
lyrics-show-song-name = Показувати назву пісні
    .description = Показувати назву треку на порожній панелі, над рядком про відсутній текст
lyrics-text-size = Розмір тексту
    .description = Текст пісні; висота синхронізованого рядка йде за ним
lyrics-title = Текст пісні
lyrics-title-unsynced = Назва над несинхронізованим
    .description = Закріпити назву треку над несинхронізованим аркушем, щоб її було видно й на низькій панелі
lyrics-wipe-lyrics = Стерти текст

## Analysis passes
pass-acoustic-body = { $model } з'ясовує, як звучить кожен із них, щоб медіатека могла знаходити музику, схожу на ту, що грає. Усе працює на цій машині, а вже описане пропускається. { $lands }
pass-acoustic-lands-database = Результати лягають у базу медіатеки, а ваші файли лишаються недоторканими.
pass-acoustic-lands-tags = Результати лягають у базу медіатеки, а для MP3 і FLAC ще й у власні теги кожного файлу, тож вони збережуться, якщо базу перебудують. Інші формати лишаються з копією в базі.
pass-acoustic-title = { $count ->
    [one] Проаналізувати { $count } трек?
    [few] Проаналізувати { $count } треки?
    [many] Проаналізувати { $count } треків?
   *[other] Проаналізувати { $count } трека?
}
pass-analyze = Проаналізувати
pass-estimate-at = { $estimate } при { $workers_phrase }.
pass-estimate-button = Оцінити
pass-estimating = Оцінюємо...
pass-measure = Виміряти
pass-no-estimate = На цій машині ще нічого не працювало, тож оцінки немає. Кнопка Оцінити прожене кілька треків і порахує решту звідти.
pass-replaygain-body = Кожен файл декодується й міряється, щоб він грав із тією гучністю, під яку його зводили. Альбоми міряються цілком там, де жоден із їхніх треків не має підсилення. { $lands }
pass-replaygain-lands-database = Числа лягають у базу медіатеки, а ваші файли лишаються недоторканими.
pass-replaygain-lands-tags = Числа записуються назад у теги кожного файлу, звідки їх читає кожен інший програвач.
pass-replaygain-title = { $count ->
    [one] Виміряти { $count } трек?
    [few] Виміряти { $count } треки?
    [many] Виміряти { $count } треків?
   *[other] Виміряти { $count } трека?
}
pass-tempo-body = З кожного файлу декодуються два вікна по пів хвилини, і в них рахуються біти, щоб медіатека могла показати, з якою швидкістю йде трек. Найкраще це працює на музиці, записаній під клік, а те, що зміряти не вдається, пропускається. Числа лягають у базу медіатеки, а ваші файли лишаються недоторканими.
pass-tempo-retry-body = Ці треки попередній прохід уже прослухав і ритму в них не знайшов. Повтор знову декодує кожен із них, тож сенс у ньому з'являється лише після того, як рахування ударів стало кращим.
pass-tempo-retry-title = Прослухати заново відхилені треки ({ $count })?
pass-tempo-title = { $count ->
    [one] Знайти темп { $count } треку?
    [few] Знайти темп { $count } треків?
    [many] Знайти темп { $count } треків?
   *[other] Знайти темп { $count } трека?
}
pass-timing = Міряємо час кількох треків...
pass-timing-failed = Не вдалося зміряти цю медіатеку: { $error }
pass-fill = Заповнити
pass-sortnames-body = Кожного виконавця шукаємо на MusicBrainz заради латинського написання, за яким він сортується, щоб 米津玄師 потрапив на літеру Y. Сервіс дозволяє один запит на секунду, і саме це задає темп. Відповіді потрапляють у базу медіатеки, файли не чіпаються ніколи.
pass-sortnames-scope-all = Шукати й ті імена, які вже сортуються латиницею
pass-romanize = Романізувати
pass-romanize-body = Кожну назву, альбом і виконавця без імені для сортування читаємо латинкою, щоб レモン знаходилося за запитом "lemon". Корейській та китайській більше нічого не потрібно. Японським кандзі потрібен словник із Налаштування > Медіатека, а IPADIC помиляється в іменах досить часто, щоб редактор тегів був на місці для виправлення. Відповіді потрапляють у базу медіатеки, файли не чіпаються ніколи.
pass-romanize-title = Прочитати латинкою назв: { $count }?
pass-romanize-skips-kanji = { $kanji } із { $total } значень — це кандзі, їх буде пропущено, доки не встановлено японський словник. Візьміть його в Налаштування > Медіатека.
pass-sortnames-title = Знайти виконавців: { $count }?
pass-workers = Потоки

## Quick play
quick-play-comfortable-rows = Просторі рядки
    .description = Дати кожному результату більше висоти
quick-play-cover = Обкладинка
    .description = Показувати мініатюру обкладинки ліворуч від кожного результату
quick-play-duration = Тривалість
    .description = Показувати довжину кожного результату праворуч
quick-play-search-placeholder = Пошук у медіатеці
quick-play-subtitle = Підпис
    .description = Показувати виконавця й альбом під кожним результатом
quick-play-syntax-absent = Рядки взагалі без значення
    .example = -year
quick-play-syntax-exclude = Усе, крім збігів
    .example = -genre:rock
quick-play-syntax-field = Закріпити одне поле, значення з пробілами в лапках
    .example = artist:"Daft Punk"
quick-play-syntax-free = Збігається з назвою, виконавцем, виконавцем альбому, альбомом або жанром
    .example = daft punk
quick-play-syntax-numeric = Порівняння числа; plays:0 і added:<90d працюють так само
    .example = rating:>=4
quick-play-syntax-title = Синтаксис пошуку
quick-play-syntax-year = Це цифри, тож префікс бере ціле десятиліття
    .example = year:199
quick-play-tag-album = Альбом
quick-play-tag-artist = Виконавець

## Drawer panel
drawer-add-tooltip = Додати панель-шухляду
drawer-answers = Відгукується на
    .description = Які вибори відкривають шухляду: лише її власна головна панель чи будь-яка панель поза нею
drawer-dim = Притлумлення
    .description = Наскільки сильно головна панель тьмяніє за відкритою шухлядою
drawer-edge = Край
    .description = Край, до якого шухляда тулиться і з якого висувається
drawer-edge-bottom = Знизу
drawer-edge-top = Згори
drawer-handle = Ручка
    .description = Показувати ручку на краю панелі. Прихована, від шухляди нічого не видно до вибору, а далі ручка лишається, поки тримається вибір, тож складену шухляду можна витягти назад
drawer-open-on = Відкривати за
    .description = Затримка на ручці відкриває шухляду завжди; вибір додає до цього вибір у головній панелі
drawer-pin-open = Тримати відкритою
drawer-reveal = Розкриття
    .description = Яку частину панелі накриває відкрита шухляда
drawer-scope-elsewhere = Деінде
drawer-scope-main = Головна панель
drawer-title = Шухляда
drawer-trigger-hover = Наведення
drawer-trigger-selection = Вибір

## Mini player
mini-tip-back = Назад до повної розкладки
mini-tip-none = Міні-розкладку не призначено
mini-tip-shrink = Стиснути до міні-програвача
mini-title = Перемикач міні

## System tray
tray-open = Відкрити
tray-pause = Пауза
tray-play = Відтворити
tray-quit = Вийти

## Window controls
window-controls-mini-toggle = Перемикач міні
    .description = Ставити перемикач міні-розкладки першим; показується, щойно призначено міні-розкладку
window-controls-minimize = Згорнути
window-controls-style = Стиль
    .description = Плоскі значки або світлофори macOS
window-controls-style-icons = Значки
window-controls-title = Кнопки вікна
window-controls-traffic-lights = Світлофори

## Section names the audio panels share. Each one heads the same kind of
## rows in the spectrum, VU, oscilloscope, spectrogram and particles
## settings, so they're defined once rather than per panel.
viz-section-analysis = Аналіз
viz-section-color = Колір
viz-section-peaks = Піки
viz-section-playback = Відтворення
viz-section-scale = Шкала
viz-section-signal = Сигнал

## Particles panel
particles-add-emitter = Додати емітер
particles-aim = Приціл
particles-aim-fixed = Фіксований
particles-aim-outward = Назовні
particles-burst = Сплеск
particles-color = Колір
particles-cone = Конус
particles-direction = Напрямок
    .description = Куди тягне; 0 - вгору, 180 - вниз
particles-drag = Опір
    .description = Скільки швидкості з'їдає повітря за секунду; нуль - це вакуум
particles-drift = Дрейф
    .description = Як швидко рухається саме поле, щоб вихори не стояли на місці
particles-edit-emitters = Редагувати емітери
particles-emitter-label = Емітер { $index }
particles-emitter-target = Емітер { $index } { $target }
particles-emitters-empty = Ще немає емітерів. Додайте один, щоб запустити поле.
particles-glow = Сяйво
    .description = Покласти за кожною частинкою м'який ореол
particles-gravity = Гравітація
particles-gravity-strength = Сила
    .description = Постійне тяжіння на все, що в польоті
particles-height = Висота
particles-hold-on-pause = Тримати на паузі
    .description = Заморозити поле на паузі замість того, щоб дати йому розвіятися
particles-length = Довжина
particles-lifetime = Час життя
particles-position-x = Позиція X
particles-position-y = Позиція Y
particles-radius = Радіус
particles-rate = Темп
particles-rotation = Обертання
particles-round-particles = Круглі частинки
    .description = Малювати крапки замість квадратів
particles-scale = Масштаб
    .description = Наскільки широкий один вихор; малий вирує, великий котиться
particles-section-emitters = Емітери
particles-section-medium = Середовище
particles-section-particles = Частинки
particles-shape = Форма
particles-shape-box = Прямокутник
particles-shape-line = Лінія
particles-shape-point = Точка
particles-shape-ring = Кільце
particles-size = Розмір
particles-speed = Швидкість
particles-trigger = Тригер
particles-trigger-continuous = Постійно
particles-turbulence = Турбулентність
particles-turbulence-drift = Дрейф турбулентності
particles-turbulence-scale = Масштаб турбулентності
particles-turbulence-strength = Сила
    .description = Наскільки сильно поле штовхає частинки; нуль вимикає
particles-width = Ширина

## Spectrum panel
spectrum-axis-labels = Підписи осі
    .description = Позначати діапазон поперек панелі: октави (C1, C2, ...) або частоти (100, 1k, 10k)
spectrum-bar-gap = Проміжок між смугами
    .description = Місце між смугами, ширші проміжки вміщують менше смуг
spectrum-bar-width = Ширина смуги
    .description = Наскільки товстою малюється кожна смуга, тонші вміщують більше діапазонів
spectrum-block-gap = Проміжок між блоками
    .description = Шов між комірками в стовпчику
spectrum-block-height = Висота блока
    .description = Наскільки високою малюється кожна комірка в стовпчику
spectrum-cap-gravity = Тяжіння позначок
    .description = Наскільки різко падають позначки піків, коли смуга опускається
spectrum-fft-size = Розмір FFT
    .description = Вікно аналізу; коротке реагує швидко, довге розрізняє тонше
spectrum-gradient-base-color = Основний колір
    .description = Тихий кінець власної шкали
spectrum-gradient-cover = Обкладинка
spectrum-gradient-mode = Градієнт
    .description = Забарвлювати смуги за гучністю: шкалою теми, кольорами обкладинки при темі пісні або власною парою
spectrum-gradient-theme = Тема
spectrum-gradient-tip-color = Колір вершини
    .description = Гучний кінець власної шкали
spectrum-high-bound-description = Найвища частота, яку аналізують смуги
spectrum-high-fft-size = Високий розмір FFT
    .description = Вікно аналізу для смуг вище за розділ
spectrum-hold-on-pause = Тримати на паузі
    .description = Заморозити смуги на паузі замість того, щоб дати їм упасти в тишу
spectrum-labels-frequency = Частота
spectrum-labels-pitch = Висота тону
spectrum-low-bound-description = Найнижча частота, яку аналізують смуги
spectrum-orientation = Орієнтація
    .description = Край, від якого ростуть смуги
spectrum-outline-bars = Контурні смуги
    .description = Малювати кожну смугу порожнім контуром замість заливки
spectrum-outline-width = Товщина контуру
    .description = Товщина обведення порожніх смуг
spectrum-peak-caps = Позначки піків
    .description = Тримати позначку на недавньому піку кожної смуги
spectrum-section-bands = Смуги
spectrum-split-at = Розділ на
    .description = Де сходяться зони, прив'язано до найближчої смуги
spectrum-split-zones = Розділити зони
    .description = Аналізувати нижче й вище за частоту розділу з різними розмірами вікна
spectrum-style = Стиль
    .description = Класичні смуги, блоки в стилі LED або суцільна лінія
spectrum-style-bars = Смуги
spectrum-style-blocks = Блоки
spectrum-style-line = Лінія
spectrum-symmetry = Симетрія
    .description = Скласти спектр навколо центру; вперед ставить низи по краях, назад зводить їх посередині
spectrum-symmetry-forward = Вперед
spectrum-symmetry-reverse = Назад

## Waveform panel
waveform-bar-gap = Проміжок між смугами
    .description = Місце між смугами, нуль зливає їх у суцільну форму
waveform-bar-width = Ширина смуги
    .description = Наскільки товстою малюється кожна смуга
waveform-outline = Контур
    .description = Обводити смуги замість того, щоб їх заливати; злиті смуги читаються як одна форма
waveform-scrobble-marker = Позначка скроблу
    .description = Тонка лінія там, де трек зараховується у скробл на Last.fm
waveform-split-channels = Розділити канали
    .description = По рядку на канал, лівий над правим; моно-треки лишаються одним рядком
waveform-unavailable = Для цього треку форма хвилі недоступна

## VU panel
vu-ballistics = Балістика
    .description = VU інтегрує гучність повільно; Пік підскакує вгору й плавно спадає
vu-ballistics-peak = Пік
vu-cap-gravity = Тяжіння позначок
    .description = Наскільки різко падають позначки піків, коли індикатор опускається
vu-channels = Канали
    .description = Розділити стереопару або скласти в один індикатор
vu-channels-mono = Моно
vu-channels-stereo = Стерео
vu-db-scale = Шкала дБ
    .description = Малювати за індикаторами підписані лінії сітки на позначках дБ
vu-gradient-mode = Градієнт
    .description = Забарвлювати індикатори за рівнем: шкалою теми, кольорами обкладинки при темі пісні або власною парою
vu-hold-on-pause = Тримати на паузі
    .description = Заморозити індикатори на паузі замість того, щоб дати їм упасти в тишу
vu-orientation = Орієнтація
    .description = Край, від якого ростуть індикатори
vu-peak-caps = Позначки піків
    .description = Тримати позначку на недавньому піку кожного індикатора
vu-section-meter = Індикатор
vu-segment-gap = Проміжок між сегментами
    .description = Шов між комірками в стовпчику
vu-segment-height = Висота сегмента
    .description = Наскільки високою малюється кожна комірка в стовпчику
vu-style = Стиль
    .description = Суцільний стовпчик або сегменти в стилі LED
vu-style-continuous = Суцільний
vu-style-segments = Сегменти

## Spectrogram panel
spectrogram-ceiling = Стеля
    .description = Рівень, що відповідає світлому краю кольорової карти, тож усе гучніше за нього впирається туди
spectrogram-colormap = Кольорова карта
    .description = Як гучність перетворюється на колір
spectrogram-colormap-cover = Обкладинка
spectrogram-colormap-grayscale = Відтінки сірого
spectrogram-colormap-ice = Лід
spectrogram-colormap-magma = Magma
spectrogram-colormap-theme = Тема
spectrogram-colormap-viridis = Viridis
spectrogram-direction = Напрямок
    .description = Край, з якого заходять нові стовпці, що також визначає, чи вісь частот іде вгору панеллю, чи впоперек неї
spectrogram-fft-size = Розмір FFT
    .description = Розмір вікна аналізу, компроміс між тим, наскільки швидко стовпець реагує на перехідний процес, і тим, наскільки добре він розділяє дві низькі ноти
spectrogram-floor = Підлога
    .description = Рівень, що відповідає темному краю кольорової карти, тож усе тихіше за нього читається як фон
spectrogram-grid = Сітка
    .description = Лінії частот поверх зображення
spectrogram-high-bound = Верхня межа
    .description = Верх осі частот, обмежений нижче частоти Найквіста, щоб відкинути майже беззвучні найвищі октави
spectrogram-history = Історія
    .description = Скільки стовпців панель тримає, перш ніж найстаріший піде за край
spectrogram-hold-on-pause = Тримати на паузі
    .description = Тримати нерухоме зображення на паузі, а не давати тиші наповзати на нього
spectrogram-labels = Підписи
    .description = Числа частот уздовж лінійки, там, де на панелі є для них місце
spectrogram-log-scale = Логарифмічна шкала
    .description = Давати кожній октаві однакове місце, музичне читання, замість рівномірного кроку в Гц, як у лабораторному приладі
spectrogram-low-bound = Нижня межа
    .description = Низ осі частот
spectrogram-section-picture = Зображення
spectrogram-speed = Швидкість
    .description = Наскільки швидко прокручується зображення, у стовпцях за секунду

## Oscilloscope panel
oscilloscope-channels = Канали
    .description = Звести в одну криву, накласти одну на одну, або скласти в окрему рамку для кожного
oscilloscope-channels-mono = Моно
oscilloscope-channels-overlay = Накладання
oscilloscope-channels-split = Окремо
oscilloscope-fill = Заливка
    .description = М'яка заливка між кривою та центральною лінією
oscilloscope-gain = Підсилення
    .description = Вертикальний масштаб, щоб підняти тихий трек до читабельної кривої
oscilloscope-gradient-mode = Градієнт
    .description = Забарвлювати криву за розмахом: шкалою теми, кольорами обкладинки при темі пісні або власною парою
oscilloscope-grid = Сітка
    .description = Малювати сітку за кривою
oscilloscope-hold-on-pause = Тримати на паузі
    .description = Тримати нерухомий кадр на паузі, а не давати кривій лягти рівною лінією
oscilloscope-line-width = Товщина лінії
    .description = Наскільки товстою малюється крива
oscilloscope-persistence = Післясвітіння
    .description = Як довго попередні кадри затримуються за кривою, той самий ефект післясвітіння люмінофору
oscilloscope-section-trace = Крива
oscilloscope-trigger = Тригер
    .description = Починати кожен кадр там, де сигнал перетинає рівень тригера, щоб періодичний матеріал стояв на місці
oscilloscope-trigger-falling = Спадний
oscilloscope-trigger-level = Рівень тригера
    .description = Рівень, на якому шукається перетин
oscilloscope-trigger-off = Вимк.
oscilloscope-trigger-rising = Висхідний
oscilloscope-window = Вікно
    .description = Скільки часу охоплює крива по ширині панелі

## Shader panel
shader-panel-compile-error = Цей шейдер не скомпілювався:
shader-panel-compile-title = Цей шейдер не скомпілювався
shader-panel-enable = Увімкнути
shader-panel-inspect = Оглянути
shader-panel-note-empty-body = Виберіть приклад або вкажіть панелі файл .wgsl, який визначає fs_user(uv).
shader-panel-note-empty-title = Шейдер не завантажено.
shader-panel-note-missing-body = Ця панель посилається на шейдер, якого в робочому просторі немає, тож запускати нічого.
shader-panel-note-missing-title = { $name } немає серед шейдерів цього робочого простору.
shader-panel-note-off-body = Джерело і його прив'язки лишаються на місці, просто не працюють.
shader-panel-note-off-title = Цей шейдер вимкнено.
shader-panel-note-pending-body = Він прийшов із розкладкою чи робочим простором, а не з цієї машини, тож лишається вимкненим, поки ви його не переглянете.
shader-panel-note-pending-title = Цей шейдер ще не читали.
## Картка перегляду шейдера, який чекає: звідки прийшло джерело і як
## виглядає обрізаний хвіст лістингу, коли той не влазить у рамку.
shader-pending-origin-file = Нібито прийшов із { $path }
shader-pending-origin-inline = Файлу за ним немає; джерело прийшло з розкладкою
shader-pending-more-lines = { $count ->
    [one] ... ще { $count } рядок
    [few] ... ще { $count } рядки
    [many] ... ще { $count } рядків
   *[other] ... ще { $count } рядка
}
## Виписування шейдера назад у файл.
shader-eject-name-taken = { $count ->
    [one] { $name } уже має { $count } пронумеровану копію серед шейдерів цього робочого простору
    [few] { $name } уже має { $count } пронумеровані копії серед шейдерів цього робочого простору
    [many] { $name } уже має { $count } пронумерованих копій серед шейдерів цього робочого простору
   *[other] { $name } уже має { $count } пронумерованої копії серед шейдерів цього робочого простору
}
shader-eject-not-in-pool = { $name } немає серед шейдерів цього робочого простору
shader-eject-failed = виписування: { $error }
shader-panel-pick = Вибрати шейдер
shader-panel-run-shader = Запустити шейдер
    .description = Вимкнено лишає джерело, закладку й прив'язки на місці й нічого не малює
shader-panel-section-routes = Маршрути

## Редактор шейдерів

shader-edit-here = Редагувати
shader-editor-window-title = rox - Редактор шейдерів
shader-editor-target-screen = Шейдер екрана
shader-editor-target-backdrop = Шейдер тла
shader-editor-origin-pool = Шейдер робочого простору: застосування доходить до кожної поверхні, що його використовує
shader-editor-origin-pool-file = Шейдер робочого простору, робоча копія в { $path }
shader-editor-origin-file = Прив'язаний до { $path }, при застосуванні файл теж записується
shader-editor-origin-inline = Власний код цієї поверхні
shader-editor-apply = Застосувати
shader-editor-revert = Відкотити
shader-editor-close = Закрити
shader-editor-hint-press = Натисніть
shader-editor-hint-apply = щоб застосувати
shader-editor-status-unchecked = Поки нічого перевіряти
shader-editor-status-ok = Компілюється
shader-editor-status-error = Цей шейдер не скомпілювався:
shader-editor-section-uniforms = Uniform-змінні
shader-editor-section-textures = Текстури
shader-editor-section-slots = Слоти
shader-editor-section-signals = Сигнали
shader-editor-slot-unnamed = Слот { $n }
shader-editor-signals-empty = У пулі ще немає сигналів. Додайте їх у вікні «Сигнали», і вони з'являться тут із живими індикаторами.
shader-editor-uniform-time = Секунди роботи шейдера, завмирають разом із потоком
shader-editor-uniform-delta = Секунди з його минулого кадру, 0 на першому
shader-editor-uniform-resolution = Поверхня в пікселях пристрою
shader-editor-uniform-mouse = xy курсор у пікселях пристрою, z і w кнопки
shader-editor-uniform-meta-0 = x гучність, y позиція в треку, z відтворення, w довжина треку в секундах
shader-editor-uniform-meta-1 = x яскравість сторінки, y світла тема, z присутність курсора, w форма вмісту
shader-editor-texture-screen = Те, що під поверхнею в цьому кадрі
shader-editor-texture-prev = Минулий кадр цієї поверхні, для шлейфів

## Genre grid panel
genre-grid-clear-picked = Очистити вибрані жанри
genre-grid-desaturate = Знебарвлювати під час відтворення
    .description = Злити колір з кожної плитки, крім жанру, що грає; наведення повертає плитці колір
genre-grid-dim-while-playing = Тьмяніти під час відтворення
    .description = Пригасити кожну плитку, крім жанру, що грає; наведення повертає плитці світло
genre-grid-follow-description = Прокручувати до жанру, що грає, щоразу коли змінюється трек
genre-grid-merge-many = { $count ->
    [one] Злити { $count } жанр у «{ $target }»
    [few] Злити { $count } жанри у «{ $target }»
    [many] Злити { $count } жанрів у «{ $target }»
   *[other] Злити { $count } жанру у «{ $target }»
}
genre-grid-merge-one = Злити «{ $source }» у «{ $target }»
genre-grid-pick-filters = Вибір фільтрує медіатеку
    .description = Клік по жанру звужує до нього кожну панель, що йде за спільним пошуком; вимкнено лишає клік звичайним вибором
genre-grid-play-genres = Відтворити жанри: { $count }
genre-grid-resume-description = Ковзати назад до жанру, що грає, коли ви перестали гортати
genre-grid-show-names = Показувати назви жанрів
    .description = Друкувати жанр під кожною плиткою, а не лише при наведенні
genre-grid-smooth-description = Плавно ковзати до жанру замість стрибка
genre-grid-tally = { $albums ->
    [one] { $albums } альбом, треків: { $tracks }
    [few] { $albums } альбоми, треків: { $tracks }
    [many] { $albums } альбомів, треків: { $tracks }
   *[other] { $albums } альбому, треків: { $tracks }
}
genre-grid-tile-face = Обличчя плитки
    .description = Що показує плитка: обкладинки альбомів жанру, обкладинки, залиті власним кольором жанру, або рівну кольорову картку з назвою на ній
genre-grid-unmerge = { $count ->
    [one] Роз'єднати { $count } значення
    [few] Роз'єднати { $count } значення
    [many] Роз'єднати { $count } значень
   *[other] Роз'єднати { $count } значення
}

## Artist grid panel
artist-grid-clear-picked = Очистити вибраних виконавців
artist-grid-desaturate = Знебарвлювати під час відтворення
    .description = Злити колір з кожної плитки, крім виконавця, що грає; наведення повертає плитці колір
artist-grid-dim-while-playing = Тьмяніти під час відтворення
    .description = Пригасити кожну плитку, крім виконавця, що грає; наведення повертає плитці світло
artist-grid-follow-description = Прокручувати до виконавця, що грає, щоразу коли змінюється трек
artist-grid-group-mode = Одна плитка на
    .description = Зазначений виконавець альбому тримає гостей запису на тому, хто його випустив; виконавець треку розводить кожну участь на окрему плитку
artist-grid-pick-filters = Вибір фільтрує медіатеку
    .description = Клік по виконавцю звужує до нього кожну панель, що йде за спільним пошуком; вимкнено лишає клік звичайним вибором
artist-grid-play-artists = Відтворити виконавців: { $count }
artist-grid-portraits = Портрети виконавців
    .description = Показувати власне фото кожного виконавця, знайдене один раз на ім'я й збережене на диску; вимкнено показує обкладинку першого альбому
artist-grid-resume-description = Ковзати назад до виконавця, що грає, коли ви перестали гортати
artist-grid-section-grouping = Групування
artist-grid-show-names = Показувати імена
    .description = Друкувати виконавця під кожною плиткою, а не лише при наведенні
artist-grid-smooth-description = Плавно ковзати до виконавця замість стрибка
artist-grid-tally = { $albums ->
    [one] { $albums } альбом, треків: { $tracks }
    [few] { $albums } альбоми, треків: { $tracks }
    [many] { $albums } альбомів, треків: { $tracks }
   *[other] { $albums } альбому, треків: { $tracks }
}
artist-grid-track-artist = Виконавець треку

## Wall panels
wall-dim-always = Завжди
    .description = Тримати плитки притлумленими, навіть коли нічого не грає; на повну показується лише плитка під курсором
wall-dim-amount = Сила притлумлення
    .description = Наскільки гаснуть інші плитки; 100% ховає їх
wall-gap = Проміжок
    .description = Місце між плитками
wall-name-alignment = Вирівнювання імен
    .description = Вирівняти підписи під їхніми плитками
wall-rounding = Заокруглення
    .description = Заокруглити кути кожної плитки; 100% - це коло
wall-section-picking = Вибір
wall-show-counts = Показувати кількість
    .description = Підсумок альбомів і треків під кожним іменем
wall-tile-size = Розмір плитки
    .description = Найдовший бік плиток; стовпці ділять ширину панелі порівну

## Metadata panel
metadata-copy-field = Копіювати { $field }
metadata-cover-background = Обкладинка на тлі
    .description = Обкладинка треку за полями
metadata-display = Показ
    .description = Аркуш, який починається з назви, або рівна таблиця підписів і значень від самого верху
metadata-display-sheet = Аркуш
metadata-display-table = Таблиця
metadata-edit-save = Зберегти
metadata-field-album-artist-sort = Сортування виконавця альбому
metadata-field-album-sort = Сортування альбому
metadata-field-artist-sort = Сортування виконавця
metadata-field-bit-depth = Глибина біт
metadata-field-bitrate = Бітрейт
metadata-field-bpm-measured = { $bpm } (виміряно rox)
metadata-field-codec = Кодек
metadata-field-comment = Коментар
metadata-field-disc = Диск
metadata-field-file = Файл
metadata-field-gain-album = Підсилення альбому
metadata-field-gain-track = Підсилення треку
metadata-field-sample-rate = Частота дискретизації
metadata-field-title-sort = Сортування назви
metadata-field-track = Трек
metadata-fields = Поля
    .description = Які поля перелічує аркуш; поле, якого в треку немає, лишається прихованим
metadata-find-online = Знайти метадані онлайн...
metadata-no-library = Немає медіатеки
metadata-romanize = Романізувати
metadata-romanize-needs-dictionary = Для назви з кандзі потрібен японський словник. Візьміть його в Налаштування > Медіатека.
metadata-romanize-sort-names = Романізувати імена для сортування
metadata-row-borders-description = Волосяна лінія під кожним рядком таблиці
metadata-source = Джерело
    .description = Стежити за тим, що грає чи вибрано, або читати медіатеку загалом
metadata-stripes-description = Тонувати кожен другий рядок таблиці

## History panel
history-column-last-played = Востаннє грав
history-descending = За спаданням
    .description = Пустити сортування навпаки
history-empty-never = Кожен трек уже грав
history-empty-recent = Ще немає прослуховувань
history-headings = Ламати недавній список на ряди альбомів; Розгорнуті додають обкладинку й статистику
history-sort-browse = Порядок перегляду
history-sort-date-added = Дата додавання
history-sort-menu = Сортування
    .description = Як упорядковані треки, які жодного разу не грали
history-title = Історія
history-view-most = Найчастіше грали
history-view-never = Жодного разу не грали
history-view-recent = Недавно грали
history-view-recent-short = Недавнє
history-view-row = Вигляд
    .description = Який зріз запису прослуханого показує панель

## Folder tree panel
folder-tree-clear-scope = Очистити обсяг теки
folder-tree-collapse-all = Згорнути все
folder-tree-collapse-branch = Згорнути гілку
folder-tree-cover-art = Обкладинка
    .description = Показувати обкладинку альбому замість значка рядка, на теках або на піснях
folder-tree-cover-folders = Теки
folder-tree-cover-songs = Пісні
folder-tree-empty = У медіатеці ще немає тек
folder-tree-expand-branch = Розгорнути гілку
folder-tree-follow-description = Розкривати й прокручувати до треку, що грає, щоразу коли він змінюється
folder-tree-nonmatch-folders = Теки без збігу
    .description = Ховати теки без збігу або тримати їх тьмяними
folder-tree-nonmatch-songs = Пісні без збігу
    .description = Усередині теки, яка збіглася, тьмянити чужі пісні або ховати їх
folder-tree-play-folder = Відтворити теку
folder-tree-play-songs = { $count ->
    [one] Відтворити { $count } пісню
    [few] Відтворити { $count } пісні
    [many] Відтворити { $count } пісень
   *[other] Відтворити { $count } пісні
}
folder-tree-resume-description = Прокручувати назад до треку, що грає, коли ви перестали гортати
folder-tree-scope-to-folder = Звузити фільтр до теки
folder-tree-smooth-description = Плавно ковзати до треку замість стрибка
folder-tree-title = Дерево

## Art panel
art-always = Тримати обкладинки притлумленими, навіть коли нічого не грає; на повну показується лише обкладинка під курсором
art-convert = Конвертувати...
art-covers-section = Обкладинки
matcher-section-matches = Збіги
art-desaturate = Злити колір з кожної обкладинки, крім альбому, що грає; наведення повертає обкладинці колір
art-dim-while-playing = Пригасити кожну обкладинку, крім альбому, що грає; наведення повертає обкладинці світло
art-disc-style = Стиль диска
    .description = Оформити кожну обкладинку як CD або як етикетку вінілової платівки
art-edit-tags = Редагувати теги...
art-fill-panel = Заповнити панель
    .description = Рахувати розмір центральної обкладинки лише з висоти панелі (з ширини, коли вона вертикальна); бічні обкладинки тоді йдуть за край, а не стискають її
art-follow-description = Ставити альбом, що грає, у центр щоразу коли змінюється трек
art-glow = Сяйво
    .description = Розлити акцентний колір за центральною обкладинкою; при тонуванні обкладинкою бере колір альбому, що грає
art-label-position = Розташування підпису
    .description = Де стоїть підпис альбому: зверху, під обкладинкою, біля нижнього краю або прихований
art-letter-rail = Алфавітна смуга
    .description = Ініціали виконавців уздовж краю полиці; клік переходить до першого альбому на цю літеру
art-layout-section = Розкладка
art-perspective = Перспектива
    .description = Повертати бічні обкладинки на кут нижче; вимкнено вони лишаються плоскими та квадратними, єдиний режим, де працює заокруглення обкладинок
art-recede = Яскравість у глибині
    .description = Наскільки освітлена найдальша обкладинка стосу; обкладинки між нею та центром ділять відстань порівну
art-spacing = Відстань між обкладинками
    .description = Наскільки далеко від центральної обкладинки стоїть перша з кожного боку; після половини вона відходить від центральної та лишає їй місце
art-stride = Крок стосу
    .description = Наскільки далеко одна від одної стоять обкладинки за першою; це ж задає, скільки проходить перетягування на одну обкладинку
art-visible = Показувати обкладинок
    .description = Скільки обкладинок стоїть з кожного боку від центральної; остання згасає на виході
art-tilt = Кут повороту
    .description = Наскільки бічні обкладинки відвертаються від вас
art-reflections = Відбиття
    .description = Віддзеркалювати кожну обкладинку в підлогу під полицею
art-resume-description = Знову ставити альбом, що грає, у центр, коли ви перестали гортати
art-shadows = Тіні
    .description = М'яка тінь під кожною обкладинкою
art-smooth-description = Плавно ковзати до альбому замість стрибка
art-title = Карусель альбомів
art-vertical-layout = Вертикальна розкладка
    .description = Скласти полицю стовпцем, що гортається вгору й вниз, замість ряду

## Playlists panel
playlists-art-description = The expanded headings' cover tile
playlists-line-height-description = One heading line; it draws inside the rows the block already has, so shrinking a line opens space instead of growing the block
playlists-meta-line = Meta Line
playlists-meta-line-description = What the second row under it shows, the same way
playlists-name-line = Name Line
playlists-name-line-description = What the heading's first row shows, left to right; a spacer or divider splits the sides
playlists-columns = Які стовпці треку показані поруч із назвою
playlists-delete = Видалити список
playlists-edit-query = Редагувати запит...
playlists-empty = Ще немає списків, додайте треки або скористайтеся Новим списком
playlists-headings = Ламати треки кожного списку на ряди альбомів; Розгорнуті додають обкладинку й статистику
playlists-import-tooltip = Імпортувати список
playlists-imported-fallback = Імпортовано
playlists-new = Новий список...
playlists-new-smart = Новий розумний список...
playlists-refuse-drag-out = Треки в розумному списку не витягнути перетягуванням
playlists-refuse-edit-query = Щоб змінити вміст розумного списку, відредагуйте запит
playlists-refuse-smart-source = Розумний список бере свої треки зі свого запиту
playlists-remove = { $count ->
    [one] Прибрати { $count } трек зі списку
    [few] Прибрати { $count } треки зі списку
    [many] Прибрати { $count } треків зі списку
   *[other] Прибрати { $count } трека зі списку
}
playlists-rename = Перейменувати...
playlists-title = Списки відтворення

## Queue panel
queue-clear = Очистити чергу
queue-empty = Черга порожня
queue-headings = Ламати чергу на ряди альбомів; Розгорнуті додають обкладинку й статистику
queue-play-now = Відтворити зараз
queue-remove = { $count ->
    [one] Прибрати { $count } трек із черги
    [few] Прибрати { $count } треки з черги
    [many] Прибрати { $count } треків із черги
   *[other] Прибрати { $count } трека з черги
}
queue-title = Черга
queue-widget-always-modal = Завжди відкривати модально
    .description = Відкривати чергу в модальному вікні щоразу, замість переходу до вже відкритої панелі черги
queue-widget-clear-queue = Очистити чергу
queue-widget-more = +{ $count } ще
queue-widget-open-on-click = Відкривати чергу кліком
    .description = Клікніть віджет, щоб перейти до відкритої панелі черги, або відкрити чергу у вікні, коли жодної немає
queue-widget-section-click = Клік
queue-widget-title = Віджет черги
queue-widget-up-next = Далі в черзі

## Biography panel
biography-background = Тло
    .description = Фанарт виконавця за текстом, притлумлений і згасає донизу
biography-fill-width = На всю ширину
    .description = Дати високій шапці зайняти всю ширину замість того, щоб стояти обмеженою й по центру
biography-from-lastfm = З Last.fm
biography-header-image = Зображення шапки
    .description = Широкий банер виконавця вгорі або портрет, коли банера немає
biography-keep-aspect = Тримати співвідношення сторін
    .description = Показувати шапку в її власних пропорціях замість того, щоб обрізати її під смугу
biography-listeners-count = { $count ->
    [one] { $count } слухач
    [few] { $count } слухачі
    [many] { $count } слухачів
   *[other] { $count } слухача
}
biography-looking-up = Шукаємо { $name }
biography-no-artist-tag = Немає тегу виконавця
biography-no-text = Біографії немає
biography-not-found = Нічого не знайдено для { $name }
biography-plays-count = { $count ->
    [one] { $count } прослуховування
    [few] { $count } прослуховування
    [many] { $count } прослуховувань
   *[other] { $count } прослуховування
}
biography-refresh = Оновити
biography-similar-artists = Схожі виконавці
    .description = Споріднені виконавці за даними прослуховувань, унизу
biography-similar-heading = Схожі виконавці
biography-stats = Статистика
    .description = Слухачі й прослуховування на Last.fm, під іменем
biography-tags = Теги
    .description = Жанрові теги як ряд позначок
biography-title = Біографія

## Status panel
status-count-albums = { $count ->
    [one] { $count } альбом
    [few] { $count } альбоми
    [many] { $count } альбомів
   *[other] { $count } альбому
}
status-count-artists = { $count ->
    [one] { $count } виконавець
    [few] { $count } виконавці
    [many] { $count } виконавців
   *[other] { $count } виконавця
}
status-count-plays = { $count ->
    [one] { $count } прослуховування
    [few] { $count } прослуховування
    [many] { $count } прослуховувань
   *[other] { $count } прослуховування
}
status-count-selected = вибрано: { $count }
status-count-tracks = { $count ->
    [one] { $count } трек
    [few] { $count } треки
    [many] { $count } треків
   *[other] { $count } трека
}
status-readouts = Показники
    .description = Тягніть уздовж смуги, щоб змінити порядок; тягніть між рядками або беріть x і плюс на позначці, щоб ховати й показувати
status-scope-selection = Вибір
status-title = Статус

## Output panel
output-detail-badge = Позначка
output-detail-compact = Компактно
output-detail-expanded = Розгорнуто
output-detail-label = Подробиці
    .description = Позначка тримає все в одній мітці, а решту показує при наведенні; компактно дає заголовку власний рядок, для смуги вздовж краю; розгорнуто додає причини поруч, або під ним, коли панель завузька
output-device-name = Назва пристрою
    .description = Називати робочий пристрій у заголовку; вимкнено лишає в рядку режим, частоту й формат
output-file-rate = Частота файлу
    .description = Підтверджувати власну частоту файлу, що грає, коли ніщо її не перетворює. Про перетворення сказано в будь-якому разі, бо саме про це попередження
output-mode-exclusive = Ексклюзивний
output-mode-shared = Спільний
output-no-output = Немає виходу
output-nothing-playing = Нічого не грає
output-pick-another-device = Виберіть інший пристрій або вимкніть ексклюзивний режим
output-headline-numbers = { $rate } Гц, { $channels } кан., { $format }
output-headline = { $mode }, { output-headline-numbers }
output-headline-device = { $mode } на { $device }, { output-headline-numbers }
output-fell-back-to-shared = Ексклюзивний відкотився до спільного: { $why }
output-replaygain-levelling = ReplayGain вирівнює цей файл на { $db } дБ
output-replaygain-short = ReplayGain { $db } дБ
output-rate-resampled = Файл, що грає, має { $rate } Гц, передискретизовано, щоб дійти до пристрою
output-rate-resampled-short = файл { $rate } Гц передискретизовано
output-rate-native = Файл, що грає, має { $rate } Гц, тож ніщо його не передискретизує
output-rate-native-short = файл { $rate } Гц, без передискретизації
output-start-track-hint = Запустіть трек, щоб побачити формат, який прийняв пристрій
output-title = Вихід

## Track columns
columns-album-artist-sort = Сортування виконавця альбому
columns-album-sort = Сортування альбому
columns-artist-sort = Сортування виконавця
columns-bits = Біти
columns-bpm = BPM
columns-codec = Кодек
columns-cover = Обкладинка
columns-fav = Улюб.
columns-gain = Підсил.
columns-kbps = Кбіт/с
columns-khz = кГц
columns-name = Назва
columns-number = Номер
columns-scanned = Скановано
columns-similar = Схожість
columns-title-sort = Сортування назви

## Filter panel
filter-add-column = Додати стовпець
filter-add-column-tooltip = Додати стовпець
filter-all = Усі
filter-clear-filters = Очистити фільтри
filter-clear-selection = Зняти вибір
filter-empty = Виберіть поле, щоб почати фільтрувати
filter-over-cap = Ще { $count }, уточніть пошуком
filter-remove-column = Прибрати стовпець

## Search panel
search-chips-below = Знизу
search-chips-inline = У рядку
search-filter-chips = Позначки фільтрів
search-placeholder = Пошук у медіатеці

## Playback panel
playback-buttons = Кнопки
    .description = Тягніть уздовж смуги, щоб змінити порядок; тягніть між рядками або беріть x і плюс на позначці, щоб ховати й показувати
playback-continue-down-list = Грати далі, вниз по списку
playback-continue-off = Грати далі вимкнено
playback-continue-weighted = Грати далі, спершу те, що не грало
playback-crossfade-inside-albums = Усередині альбомів
playback-crossfade-off = Кросфейд вимкнено
playback-crossfade-tip = Кросфейд { $length }
playback-highlight-circle = Коло
playback-highlight-square = Квадрат
playback-hold-draw = { $tip }. Затисніть, щоб вибрати вигляд
playback-hold-length = { $tip }. Затисніть, щоб вибрати довжину
playback-hold-order = { $tip }. Затисніть, щоб вибрати порядок
playback-loop-off = Повтор вимкнено
playback-loop-queue = Повторювати чергу
playback-loop-track = Повторювати цей трек
playback-menu-continue = Кнопка продовження
playback-menu-crossfade = Кнопка кросфейду
playback-menu-favourite = Кнопка улюбленого
playback-menu-random = Кнопка випадкового
playback-menu-rating = Зірки оцінки
playback-menu-stop = Кнопка зупинки
playback-menu-stop-after = Кнопка зупинки після
playback-menu-volume = Кнопка гучності
playback-pause = Пауза
playback-play-highlight = Підсвітка відтворення
    .description = Акцентна заливка кнопки відтворення: коло, м'який квадрат або нічого
playback-random-tip-random = Відтворити випадковий трек
playback-random-tip-similar = Відтворити трек, схожий на цей
playback-seek-back-tip = Назад на 10 секунд
playback-seek-forward-tip = Вперед на 10 секунд
playback-shuffle-off = Перемішування вимкнено
playback-shuffle-on = Перемішування ввімкнено, порядок { $order }
playback-stop-after-armed = Зупинитися після цього треку, зведено
playback-stop-after-tip = Зупинитися після цього треку
playback-stop-tip = Зупинити й вивантажити трек
playback-volume-tip-muted = Увімкнути звук, { $percent }%. Права кнопка дає повзунок
playback-volume-tip-unmuted = Вимкнути звук, { $percent }%. Права кнопка дає повзунок

## Track info panel
track-info-color-output-chip = Кольорова позначка виходу
    .description = Дати позначці набирати попереджувальних кольорів, коли вихід відкочується чи передискретизує. Вимкнено лишає її в тому самому приглушеному тоні завжди, а підказка при наведенні все одно пояснює стан
track-info-cycle-every = Міняти кожні
    .description = Скільки часу тримається кожен рядок перед згасанням
track-info-cycle-rows = Міняти рядки
    .description = Показувати рядки розстановки по одному в одному рядку, згасаючи між ними; один рядок сам по собі читається як він є
track-info-delay = Затримка
    .description = Скільки рядок стоїть на кожному краю, перш ніж рушити знову
track-info-marquee = Біжучий рядок
    .description = Що робить рядок, задовгий для панелі: повзе й вертається або крутиться без кінця
track-info-menu-overflow = Переповнення
track-info-next = Далі: { $line }
track-info-opening = відкриваємо...
track-info-output-fallback = Пристрій відмовив ексклюзивному виходу, тож відтворення йде через спільний мікшер. Пристрій повідомив: { $reason }
track-info-output-resample-exclusive = Цей файл має { $source } кГц, а карта взяла { $device } кГц, тож кожен семпл перетворюється на виході. Пристрій не погодився працювати на власній частоті файлу.
track-info-output-resample-mixer = Цей файл має { $source } кГц, а мікшер працює на { $device } кГц, тож кожен семпл перетворюється на виході. Ексклюзивний режим віддав би карті власну частоту файлу.
track-info-overflow-loop = Кільце
track-info-overflow-scroll = Прокручування
track-info-overflow-truncate = Обрізати
track-info-queued-count = у черзі: { $count }
track-info-row-size = Розмір рядка { $number }
track-info-speed = Швидкість
    .description = Як швидко повзе рядок
track-info-text-size = Розмір тексту

## Seek panel
seek-ending = Кінець
    .description = Відлічувати час, що лишився, або показувати повну довжину
seek-ending-remaining = Лишилось
seek-ending-total = Усього
seek-playhead = Позиція
    .description = Займати всю висоту смуги або тулитися до лінії
seek-playhead-full = На всю
seek-playhead-line = Лінія
seek-playhead-max-height = Макс. висота позиції
    .description = Обмежити повну позицію, по центру лінії; 0 заповнює панель
seek-playhead-width = Ширина позиції
    .description = Ширина рухомої позначки позиції
seek-rounding = Заокруглення
    .description = Радіус кутів лінії, аж до пігулки на половині товщини
seek-scrobble-marker = Позначка скроблу
    .description = Тонка лінія там, де трек зараховується у скробл на Last.fm
seek-show-timings = Показувати час
seek-thickness = Товщина
    .description = Висота лінії треку

## Volume panel
volume-pieces = Частини
    .description = Тягніть уздовж смуги, щоб змінити порядок; тягніть між рядками або беріть x і плюс на позначці, щоб ховати й показувати. Коли відсоток приховано, його показує підказка динаміка
volume-readout = Показник
    .description = Показувати рівень як відсоток або як підсилення в децибелах, яке він дає
volume-readout-decibels = Децибели
volume-readout-percent = Відсоток
volume-stretch = Розтягнути
    .description = Дати повзунку заповнити панель замість того, щоб обмежувати його ширину
volume-tip-mute = Вимкнути звук
volume-tip-mute-level = Вимкнути звук, { $level }
volume-tip-unmute = Увімкнути звук
volume-tip-unmute-level = Увімкнути звук, { $level }

## Shared panel content
content-filter = Фільтр
content-no-track = Немає треку
content-total-genres = Жанри
content-total-time = Загальний час

## Shared panel chrome
panel-columns-description = Які стовпці треку показані
panel-headings = Заголовки
panel-jump-to-playing = Перейти до того, що грає
panel-menu-display = Показ
panel-title-artists = Виконавці
panel-title-genres = Жанри
panel-title-oscilloscope = Осцилограф
panel-title-particles = Частинки
panel-title-playback = Відтворення
panel-title-seek = Перемотка
panel-title-shader = Шейдер
panel-title-spectrogram = Спектрограма
panel-title-spectrum = Спектр
panel-title-theme-toggle = Перемикач теми
panel-title-track-info = Про трек
panel-title-volume = Гучність
panel-title-vu = Індикатор VU
panel-title-waveform = Форма хвилі

## Everything else
choice-both = Обидва
choice-dim = Притлумити
choice-hide = Приховати
composite-add-panel = Додати панель
composite-host-settings = Налаштування { $host }
composite-move-left = Пересунути ліворуч
composite-move-right = Пересунути праворуч
composite-remove = Прибрати
composite-replace = Замінити
group-panel-add-slot = Додати слот
group-panel-move-down = Пересунути вниз
group-panel-move-up = Пересунути вгору
group-panel-remove-slot = Прибрати слот
group-panel-split-side-by-side = Поділити поруч
group-panel-split-stacked = Поділити один над одним
group-panel-swap-panels = Поміняти панелі місцями
group-panel-title = Група
overlay-dim = Притлумлення
    .description = Наскільки сильно головна панель тьмяніє під розкритою накладкою
overlay-title = Накладка
overlay-toggle = Перемкнути накладку
shader-confirm-hint-after = перемикає шейдер звідусіль.
shader-confirm-hint-before = Шейдер може зробити вікна незручними. Поверніть як було або закрийте це вікно, щоб повернутися до попереднього стану.
shader-confirm-keep = Лишити
shader-confirm-question = Лишити цей екранний шейдер?
shader-confirm-revert = Повернути як було
shader-confirm-window-title = rox - Шейдер накладки
slide-add = Додати слайд
slide-next = Наступний слайд
slide-previous = Попередній слайд
slide-title = Слайд
theme-toggle-to-dark = Перемкнути на темну тему
theme-toggle-to-light = Перемкнути на світлу тему
transport-favourite-add = Додати в улюблене
transport-favourite-nothing = Немає чого додати в улюблене
transport-favourite-remove = Прибрати з улюбленого
transport-pieces = Частини
    .description = Тягніть уздовж рядка, щоб змінити порядок, і між рядками, щоб пересунути; x і плюс на позначці ховають і показують

## Stragglers picked up in the final sweep
duplicates-scanning = Скануємо...
about-copyright = Copyright © 2026
signal-name-placeholder = Назва сигналу
signals-empty = Ще немає сигналів. Додайте один або клікніть правою кнопкою будь-який регулятор, який можна прив'язати.
signal-add = Додати сигнал
panel-approve = Підтвердити
panel-turn-off = Вимкнути
shader-from-file = З файлу...
arrange-add-row = Додати рядок
smart-playlist-name-placeholder = Назва списку
smart-playlist-name-to-save = Назвіть список, щоб його зберегти
panel-new-playlist = Новий список відтворення...
panel-edit-tags = Редагувати теги...
panel-edit-cover = Редагувати обкладинку...
panel-rename-files = Перейменувати файли...
panel-convert = Конвертувати...
panel-catalog-drag-anchor = Якір перетягування
panel-catalog-spacer = Проміжок

## Duration and worker phrasing
pace-under-a-minute = менше за хвилину
pace-minutes = { $count ->
    [one] близько { $count } хвилини
    [few] близько { $count } хвилин
    [many] близько { $count } хвилин
   *[other] близько { $count } хвилини
}
pace-hours = { $count ->
    [one] близько { $count } години
    [few] близько { $count } годин
    [many] близько { $count } годин
   *[other] близько { $count } години
}
pace-half-hours = близько { $value } години
pace-days = { $count ->
    [one] близько { $count } дня
    [few] близько { $count } днів
    [many] близько { $count } днів
   *[other] близько { $count } дня
}
pace-workers = { $count ->
    [one] { $count } потоці
    [few] { $count } потоках
    [many] { $count } потоках
   *[other] { $count } потоках
}
tasks-rest-takes = , решта займе { $estimate }
tasks-measuring-takes = , зміряти їх займе { $estimate }
tasks-working-out-takes = , з'ясувати їх займе { $estimate }
tasks-time-left = , лишилось { $left }
tasks-failed-suffix = (не вдалося { $count })
tasks-file-suffix = - { $file }
tasks-no-beat-suffix = ({ $count } без чіткого біту)
tasks-estimate-at-workers = ({ tasks-estimate-at })

## Panel vanity names
panel-title-art-view = Вигляд обкладинок
panel-title-artist-grid = Сітка виконавців
panel-title-genre-grid = Сітка жанрів
panel-title-biography = Біографія
panel-title-cover-art = Обкладинка
panel-title-drag-anchor = Якір перетягування
panel-title-drawer = Шухляда
panel-title-eq-widget = Віджет еквалайзера
panel-title-filter = Фільтр
panel-title-folder-tree = Дерево тек
panel-title-group = Група
panel-title-history = Історія
panel-title-lyrics = Текст пісні
panel-title-menu = Меню
panel-title-metadata = Метадані
panel-title-mini-toggle = Перемикач міні
panel-title-output = Вихід
panel-title-overlay = Накладка
panel-title-playlists = Списки відтворення
panel-title-queue = Черга
panel-title-queue-widget = Віджет черги
panel-title-search = Пошук
panel-title-slide = Слайд
panel-title-spacer = Проміжок
panel-title-stats-widget = Віджет статистики
panel-title-vu-meter = Індикатор VU
panel-title-window-controls = Кнопки вікна

## Relative time and the output headline
ago-just-now = щойно
ago-minutes = { $count } хв тому
ago-hours = { $count } год тому
ago-days = { $count } д тому
ago-weeks = { $count } тиж тому
ago-years = { $count } р тому

## Long spans spelled out, for the library totals. The short clocks stop
## meaning much past a day, so these carry the noun with them.
span-seconds = { $count ->
    [one] { $count } секунда
    [few] { $count } секунди
    [many] { $count } секунд
   *[other] { $count } секунди
}
span-minutes = { $count ->
    [one] { $count } хвилина
    [few] { $count } хвилини
    [many] { $count } хвилин
   *[other] { $count } хвилини
}
span-hours = { $count ->
    [one] { $count } година
    [few] { $count } години
    [many] { $count } годин
   *[other] { $count } години
}
span-days = { $count ->
    [one] { $count } день
    [few] { $count } дні
    [many] { $count } днів
   *[other] { $count } дня
}
span-weeks = { $count ->
    [one] { $count } тиждень
    [few] { $count } тижні
    [many] { $count } тижнів
   *[other] { $count } тижня
}
span-years = { $count ->
    [one] { $count } рік
    [few] { $count } роки
    [many] { $count } років
   *[other] { $count } року
}

## How a span joins its second unit: "3 weeks, 2 days".
span-pair = { $first }, { $second }

## A percentage. The space before the sign is a locale question, not a
## notation one, so each locale spells the whole thing out.
unit-percent = { $value }%

settings-audio-output-headline = { $mode }{ $note } на { $device }, { $rate } Гц, { $channels } кан., { $format }
settings-audio-output-experimental =  (експериментальний)

## ML model catalog
settings-mlmodels-description = { $summary }. { $dim } значень на трек. { $licence }
settings-mlmodels-on-disk = , { $size } на диску
settings-mlmodels-to-download = , { $size } до завантаження
model-summary-dsp-timbre-1 = Вбудована, без завантаження. Зведення енергії по логарифмічних смугах, спектральної форми й частоти атак кожного треку. Грубо порівняно з навченою мережею, але їй нічого не треба і вона працює всюди
model-summary-panns-cnn10 = Згорткова мережа, навчена на AudioSet розпізнавати, що це за звук. Її опис треку з 512 значень набагато багатший за вбудований ескіз, ціною завантаження на 24 МБ і повільнішого проходу аналізу
dictionary-summary-lindera-ipadic = Японський словник, на якому тримаються читання кандзі. Без нього кана й хангиль усе одно романізуються, а китайська усе одно читається піньїнем, але назва в кандзі пропускається

## Shipped workspaces
workspace-shipped-default = (Типовий)
workspace-shipped-default-blurb = Який вигляд має rox із коробки: напівпрозорі поверхні над робочим столом, без рамок вікна, тонування обкладинкою вимкнено. Відправна точка, від якої відходить кожен інший вигляд тут.
workspace-shipped-catrox-blurb = Той самий скін для foobar2000, з якого все почалося, зібраний наново: круглий рендер обкладинки як CD, поля метаданих ліворуч і треки, згруповані за альбомами, з крапками оцінок.
workspace-shipped-critters-blurb = Увесь застосунок як 1-бітний друк: упорядкований дизеринг по кожній поверхні, тони, що стискаються з саббасом, і стіна шуму, яка звивається під пісню. За мотивами Critters for Sale.
workspace-shipped-diffuse-blurb = Лише альбом, що грає: обкладинка й картка відтворення однією групою на все вікно, прозорі поверхні над тлом, без швів. Медіатека, черга й текст пісні чекають у шухляді біля правого краю й висуваються над музикою, коли навести на ручку. Монохром, тож колір дають обкладинки.
workspace-shipped-foobar-blurb = Та розкладка, з якою сперечається весь цей проєкт. Непрозорі панелі, стовпці фільтрів за виконавцем і альбомом, щільна таблиця треків і смуга меню рівно там, де вона завжди була.
workspace-shipped-llama-winamp-blurb = Winamp таким, яким ви його пам'ятаєте, а не таким, яким він був. Tahoma, темно, без рамок, крапковий спектр угорі й режим згортання на міні-розкладці.
workspace-shipped-metro-blurb = Плоскі панелі й просторі рядки в Segoe UI, з увімкненим тонуванням обкладинкою, тож уся палітра йде за тим, яка обкладинка грає.
workspace-shipped-phosphor-blurb = Усе моноширинним. Consolas, зелене на чорному, без обкладинки у швидкому запуску: термінал, який випадково грає музику.
