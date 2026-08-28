### 简体中文。与 en-CA/rox.ftl 逐键对应，rox-i18n 里的一致性测试
### 会检查这一点。键名是按界面区域加前缀的 kebab-case；每一行的说明
### 是标签消息上的一个属性。

## Shared widgets
tracking-title = 跟随
tracking-follow = 跟随播放
tracking-resume = 空闲时恢复跟随
tracking-smooth = 平滑滚动
align-row = 对齐
    .description = 面板有多余空间时，内容放在哪里
valign-row = 垂直对齐
    .description = 面板有多余高度时，内容放在哪里
valign-top = 顶部
valign-middle = 居中
valign-bottom = 底部

## Panel source and search rows
source-track = 曲目
    .description = 跟随正在播放的曲目，或媒体库里选中的曲目
source-follow-playing = 跟随播放
source-follow-selection = 跟随选中
source-playing = 播放中
source-selected = 选中
query-search = 搜索
query-search-box = 搜索框
    .description = 显示搜索框；搜索词只在它显示时生效
query-source = 搜索来源
    .description = 跟随共享的搜索词、按本面板自己的搜索框筛选，或显示另一个面板选中的内容
query-source-shared = 共享
query-source-own = 自有
query-source-selection = 选中

## Signals and routes
signal-source = 来源
    .description = 信号跟随什么：频段跟一段频率范围，电平跟整个混音，起音在这段范围内每次击打时脉冲一下，触发在范围达到阈值时发出脉冲，累加把另一个信号随时间加起来
signal-kind-band = 频段
signal-kind-level = 电平
signal-kind-onset = 起音
signal-kind-trigger = 触发
signal-kind-total = 累加
signal-response = 响应
signal-response-pulse = 每次脉冲衰减前会响多久
signal-response-drift = 0 紧贴音乐，100 拖在后面
signal-threshold = 阈值
signal-threshold-trigger = 这段范围要达到多高才触发脉冲；在电平回落到上方表盘的刻度以下之前，它不会再触发
signal-threshold-gate = 低于此值信号读作零，高于它输出重新从零开始上升，安静的段落就推不动旋钮。上方表盘上的刻度标出了它的位置
signal-low-bound = 低端
signal-high-bound = 高端
signal-adds-up = 累加对象
    .description = 这里累加哪个信号；那个信号读数高时它往上升，安静时它停住
signal-aggregate-nothing = 没有可跟随的信号
signal-aggregate-pick = 选一个信号
signal-aggregate-alone = 信号池里没有别的信号可以累加，所以这里保持为零。加一个，它就会出现在列表里。
signal-aggregate-unpicked = 什么都没选，所以这个累加值保持为零。在上面选一个信号。
signal-rate = 速率
    .description = 满输入时每秒绕多少圈；过 1 就回到 0 继续上升，着色器把它读作相位
signal-reset-on-track = 换曲时重置
    .description = 新歌开始时回落到零，相位就不会从上一首的累加值接着算
signal-flush = 清空
signal-routes-in-panel = { $count ->
   *[other] 这个面板里有 { $count } 条路由
}
    .description = 立刻回到零。它会花一小会儿排空而不是直接跳，跟着它的东西就不会抖
route-header = 路由
route-signal = 信号
    .description = 这条路由跟随哪个共享信号；在这里调它，就是调这个信号上的每一条路由
route-new-signal = 新建信号
route-shared-note = 这个信号上的每条路由共用
route-signal-gone = 这条路由的信号没了；在上面另选一个之前，旋钮保持滑块上的值。
route-range-note = 仅对这个参数的范围
route-quiet = 安静
    .description = 静音时旋钮的读数，按它自身设定值的比例算
route-loud = 响亮
    .description = 满信号时它的读数；100% 就是滑块自身的值，低于“安静”则向下调制
route-slot = 槽位
    .description = 这条路由填着色器十六个信号槽位中的哪一个
route-slot-quiet-description = 静音时槽位的读数
route-slot-loud-description = 满信号时它的读数；低于“安静”会让槽位倒着跑
route-slot-signal-description = 这条路由跟随哪个共享信号
route-slot-signal-gone = 这条路由的信号没了；另选一个之前槽位读数为零。
route-add = 添加路由
route-unrouted = 未路由
route-pick-slot = 选一个槽位
route-pick-signal = 选一个信号
route-no-signal = 无信号
route-no-signals-yet = 还没有可跟随的信号。建一个它就会出现在这里；在那之前槽位读数为零。
route-open-signals = 打开信号
route-create-signal = 新建信号

## Panel settings window
panel-settings = 面板设置
panel-menu-label = 面板
panel-save-as-preset = 存为预设
panel-rename = 重命名
panel-rename-name = 名称
panel-rename-note = 显示为面板的标签页名；留空则回到内置名称
panel-rename-hint-after = 重命名
panel-was-closed = 面板已关闭
panel-reset = 重置
panel-inverse = 反相
panel-apply-song-theme = 应用歌曲配色
panel-page-appearance = 外观
panel-page-behavior = 行为
panel-page-shader = 着色器
panel-section-placement = 位置
panel-section-size = 尺寸
panel-section-opacity = 不透明度
panel-section-frame = 边框
panel-section-colors = 颜色
panel-section-font = 字体
panel-section-shader = 着色器
panel-section-signals = 信号
panel-section-slots = 槽位
panel-awaiting-approval = 等待批准
panel-size-off = 关
panel-locked = 锁定
    .description = 把面板钉在原地；它在停靠区里既不能拖动，也不能重排
panel-drag-anchor = 拖动锚点
    .description = 在面板任意处拖动都会移动窗口，普通点击仍落在它的控件上；给关掉窗口装饰的布局用
panel-slot-controls = 槽位控件
    .description = 显示角上用来交换和移除内嵌面板的按钮。隐藏后仍可在设置里“工作区”页的树上编辑布局
panel-min-width = 最小宽度
    .description = 调整大小时到哪里就不再把面板挤窄。写多少算多少，低于面板自身的下限也照办，紧凑条就能比出厂更窄；留空则不动下限
panel-max-width = 最大宽度
    .description = 给面板宽度设上限，窗口变宽时它就不会跟着拉伸
panel-min-height = 最小高度
    .description = 调整大小时到哪里就不再把面板挤矮。写多少算多少，低于面板自身的下限也照办，紧凑条就能比出厂更矮；留空则不动下限
panel-max-height = 最大高度
    .description = 给面板高度设上限，窗口变高时它就不会跟着拉伸
panel-own-opacity = 自定表面不透明度
    .description = 让这个面板在背景之上用自己的不透明度，而不是跟随应用的设置
panel-surface-opacity = 表面不透明度
panel-margin = 外边距
    .description = 把面板从它的格子里往里收，背景从缝隙透出来
panel-padding = 内边距
    .description = 面板边缘以内的留白，仍算在它自己的背景里
panel-rounding = 圆角
    .description = 把面板的角磨圆，露出背景
panel-border = 边框
    .description = 面板边缘的一圈线，用“边框”角色的颜色；某一边为零就不画
panel-font = 字体
    .description = 面板的字体；默认跟随应用字体
panel-font-size = 字号
    .description = 面板文字相对应用字体的大小；各行随之缩放
panel-surface-shader = 表面着色器
    .description = 在这个面板的主体上跑一个 WGSL 着色器，位于应用的屏幕着色器之下
panel-run-when-idle = 空闲时继续运行
    .description = 音频静默时也继续画帧。关掉后着色器停在最后一帧，面板不占任何开销
panel-shader-is-scene = 这个着色器是场景，所以它盖住面板主体，而不是画在上面。它来自某个包或旧配置；上面的列表只提供不遮住面板内容的着色器。

## Shader picker and saving
shader-source = 来源
shader-pick-none = 无
shader-reload = 重新加载
shader-edit-as-file = 作为文件编辑
shader-make-private-copy = 建立私有副本
shader-save-replace = 替换
shader-save-to-workspace = 保存到工作区
shader-save-replaces = 替换这个工作区里已经叫 { $name } 的着色器。用这个名字的每个面板都会跟着变
shader-save-adds = 以 { $name } 加进这个工作区的着色器里。任何面板都能用它，改一次全都跟着变
shader-group-examples = 示例
shader-group-this-workspace = 本工作区
shader-group-scenes = 场景
shader-group-workspace-scenes = 工作区场景
shader-group-overlays = 覆盖层
shader-group-workspace-overlays = 工作区覆盖层

## Saving a panel preset
preset-save = 保存预设
preset-save-name = 预设名称
preset-save-replaces = 替换这个工作区里已经叫 { $name } 的预设
preset-save-hint-after = 保存
preset-back-from = 可以从
preset-back-add-panel = 添加面板
preset-back-then = 然后
preset-back-presets = 预设
preset-back-tail = 加回来，任意面板菜单里都行。预设只属于这个工作区，别的工作区没有。

## Keyboard hints
hint-press = 按
hint-key-enter = Enter

## Settings: language
settings-language = 语言
    .description = 界面语言。“系统”会与操作系统的语言列表匹配，没有匹配项时回退到英语
    .keywords = 语言 翻译 本地化 区域 yuyan fanyi language locale
settings-language-system = （系统语言）
settings-language-search = 搜索语言
picker-no-matches = 无匹配项
settings-search-no-matches = 没找到匹配“{ $text }”的项

## Embed dialog
bake-window-title = rox - 嵌入已存储的元数据
bake-title = 嵌入已存储的元数据
bake-intro = 把已存储的元数据写进文件本身，别的播放器也能读到。不会重新计算任何内容。
bake-formats = 仅限 MP3 和 FLAC；其他格式和 CUE 曲目会跳过
bake-source-lyrics = 歌词
bake-source-gain = ReplayGain
bake-source-acoustic = 声学描述
bake-detail-nothing = 没有可嵌入的存储内容
bake-detail-only-skipped = 无内容可写，跳过 { $skipped } 个
bake-detail-writes = { $count ->
   *[other] 待写入 { $count } 个文件
}
bake-detail-writes-skipped = { $count ->
   *[other] 待写入 { $count } 个文件，跳过 { $skipped } 个
}
bake-error-read = 媒体库读不出来：{ $error }
bake-survey-counting = 正在查看媒体库…
bake-survey-progress = 正在读取标签，{ $done } / { $total }
bake-nothing-to-embed = 没有可嵌入的内容：文件里已经有 rox 存下的全部内容
bake-rewrites = { $count ->
   *[other] 将重写 { $count } 个文件
}
bake-hint-before = 按
bake-hint-key = Enter
bake-hint-after = 嵌入
bake-embed = 嵌入
bake-cancel = 取消
bake-summary-files = { $count ->
    [one] { $count } 个文件
   *[other] { $count } 个文件
}
bake-summary-updated = 已更新 { $files }
bake-summary-stopped = 在更新 { $files } 之后停下
bake-summary-skipped = ，跳过 { $count } 个
bake-summary-failed = ，{ $count } 个失败

## Arrange editors and header pieces
arrange-shown = 显示
arrange-hidden = 隐藏
tile-face-mosaic = 封面拼贴
tile-face-tinted = 染色拼贴
tile-face-gradient = 渐变卡片
tile-face-color = 纯色卡片
head-piece-artist = 艺术家
head-piece-album = 专辑
head-piece-year = 年份
head-piece-genre = 流派
head-piece-quality = 音质
head-piece-tracks = 曲目
head-piece-time = 时长
head-piece-spacer = 间隔
head-piece-divider = 分隔线
head-piece-art = 封面
head-unknown = 未知
status-item-count = 数量
status-item-time = 时长
status-item-albums = 专辑
status-item-artists = 艺术家
status-item-plays = 播放次数
volume-item-icon = 图标
volume-item-slider = 滑块
volume-item-percent = 百分比

## Filter chips and search menus
filter-field-artist = 艺术家
filter-field-album-artist = 专辑艺术家
filter-field-album = 专辑
filter-field-genre = 流派
filter-field-year = 年份
filter-field-folder = 文件夹
filter-unknown = 未知
filter-clear = 清除
query-show-search-box = 显示搜索框
query-own-query = 自有搜索词
query-shared-query = 共享搜索词
headers-off = 关
headers-compact = 紧凑
headers-expanded = 展开

## Panel context menu
panel-dock-back = 停靠回去
panel-pop-out = 弹出为窗口
panel-close = 关闭
panel-duplicate = 复制
panel-reveal-in-browser = 在文件管理器中显示
panel-play-next = 下一首播放
panel-add-to-queue = 加入队列
panel-add-to-playlist = 添加到播放列表
panel-favourite-add = 加入收藏
panel-favourite-remove = 从收藏移除
shader-pick-missing = { $name }（缺失）
shader-pick-custom = 自定义

## Shipped shader examples
shader-blurb-plasma = 只靠自己的 uniform 画出的流动色彩，代价就是一个普通四边形。
shader-blurb-trails = 把自己上一帧抹开，所以它跑在屏幕通道上。
shader-blurb-sheen = 一圈暗角加一道游走的光泽，给本来就有内容的面板用的透明覆盖层。
shader-blurb-shadow = 面板自己的文字和控件投下的阴影，从遮罩捕获里读出来。
shader-blurb-cover = 正在播放曲目的封面，加黑边铺在它自身颜色的底色上。
shader-blurb-badge = 封面缩成一张停在角落的小卡片，配一个槽位来挪动它的位置。
shader-blurb-lamp = 一盏跟着光标走、对点击有反应的灯，透明覆盖层。
shader-blurb-cube = 一个在伪 3D 里翻滚的线框立方体，按加色光绘制。
shader-blurb-bloom = 流动的光球经过半分辨率的第二遍泛光，整条链的缩微版。
shader-blurb-tube = 把下面的面板放在弯曲的 CRT 屏面上重放，扫描线一应俱全。

## Transport strip pieces
seek-item-elapsed = 已播
seek-item-strip = 进度条
seek-item-ending = 结尾
seek-item-duration = 时长
info-item-track-no = 曲目号
info-item-title = 标题
info-item-duration = 时长
info-item-next = 下一首
info-item-queued = 队列中
info-item-output = 输出
info-item-favourite = 收藏
info-item-rating = 评分
playback-item-previous = 上一首
playback-item-seek-back = 快退
playback-item-play = 播放
playback-item-seek-forward = 快进
playback-item-next = 下一首
playback-item-stop = 停止
playback-item-volume = 音量
playback-item-loop = 循环
playback-item-shuffle = 随机
playback-item-continue = 续播
playback-item-crossfade = 交叉淡化
playback-item-random = 随机一首
playback-item-stop-after = 播完即停
playback-item-favourite = 收藏
playback-item-rating = 评分

## Dock chrome
dock-empty-tab = 空标签页
dock-unnamed = 未命名
dock-tiles = 平铺
dock-zoom-in = 放大
dock-zoom-out = 缩小
dock-collapse = 折叠
dock-expand = 展开

## Shader picker notes
shader-note-empty = 选一个示例开始，或者给 rox 指一个带片元阶段、定义了 fs_user(uv) 的 .wgsl 文件
shader-note-missing = { $name } 已经不在这个工作区的着色器里了，所以什么都不画。在这里另选一个，这个面板就会有自己的来源。
shader-note-shared = 在这个工作区里共享。改一次，用到它的每个表面都跟着变。
shader-note-file = { $path }。你保存的改动会在着色器绘制时热加载，源码也存在布局和包里，所以换到从没有这个文件的机器上它照样能用。
shader-note-custom = 这份源码存在它的布局或包里，背后没有文件。“作为文件编辑”会把它写回去，并接上你之后的保存。

## Panel pages and shared sides
panel-page-layout = 布局
panel-page-view = 视图
panel-page-content = 内容
panel-page-source = 来源
panel-page-bindings = 绑定
panel-page-emitters = 发射器
panel-page-forces = 力场
panel-page-scene = 场景
side-left = 左
side-right = 右
genre-face-mosaic = 拼贴
genre-face-tinted = 染色
genre-face-gradient = 渐变
genre-face-color = 纯色

## Library panel
panel-title-library = 媒体库
library-play = 播放
library-play-album = 播放专辑
library-play-group = 播放分组
library-play-tracks = 播放 { $count } 首曲目
library-play-similar = 播放相似曲目
library-filter-by-album = 按专辑筛选
library-filter-by-artist = 按艺术家筛选
library-jump-to-playing = 跳到正在播放
library-menu-display = 显示
library-disc = 碟 { $number }
library-empty-title = 打开音乐文件夹
library-empty-note = 它会被扫描进媒体库（flac、mp3、wav）
library-headers = 分组标题
    .description = 列表上的分组断点；排序会把连续的同组曲目聚在一起，搜索时平铺显示
library-group-by = 分组依据
    .description = 分组标题按什么断开；流派和年份会重排列表
library-header-row = 标题行
    .description = 单行标题从左到右显示什么；间隔或分隔线把两侧分开
library-header-lines = 标题行内容
    .description = 标题块的各行，自上而下；空行会去掉
library-follow-description = 每次换曲都滚动到正在播放的那一行
library-resume-description = 停止浏览后滚回正在播放的那一行
library-smooth-description = 滑到那一行，而不是直接跳
library-columns = 列
    .description = 显示哪些列；在面板里拖动表头可以调整顺序和宽度
library-column-headers = 列表头
    .description = 列表上方可排序的表头行；隐藏后各列仍保持顺序和宽度
library-compact-plays = 紧凑播放次数
    .description = 播放次数列显示为一个小数字加一道横杠
library-line-height = 行高
    .description = 一条标题行的高度；标题块按需要占用行数，与曲目行无关
library-text-size = 文字大小
    .description = 标题各行的文字，与行高无关，这样封面可以单独变大
library-flush-background = 平齐背景
    .description = 把标题放在列表背景上，而不是抬高的色块上；歌曲配色会让两者一起变
library-gap-above = 上方留白
    .description = 从标题块顶部切出来的；列表从这里透出，各行会收紧以适应
library-gap-below = 下方留白
    .description = 标题块下方同理，在它的曲目之前
library-section-rows = 行
library-row-height = 行高
    .description = 曲目行的高度；文字随之变化，两者都随应用字体缩放
library-row-spacing = 行间距
    .description = 每行多占的高度；不放大文字也能透气
library-stripes = 隔行高亮
    .description = 隔一行给曲目行上色，长列表更好扫
library-row-borders = 行分隔线
    .description = 每个曲目行下方的细线
library-art-description = 展开标题的图块：封面、艺术家肖像，或流派图面
library-art-rounding = 封面圆角
    .description = 把封面的角磨圆
library-art-position = 封面位置
    .description = 展开标题的图块放在标题块的哪一侧
library-art-margin = 封面边距
    .description = 把图块在标题块里往内收；它会缩小以保持正方
library-circular-portraits = 圆形肖像
    .description = 按艺术家分组时，把图块磨成整圆，而不是用圆角旋钮
library-genre-face = 流派图面
    .description = 按流派分组时图块显示什么：封面、按流派颜色染过的封面，或几何图形下的纯色卡片

## Album grid panel
panel-title-album-grid = 专辑墙
grid-menu-scroll = 滚动
grid-vertical-scroll = 纵向滚动
grid-horizontal-scroll = 横向滚动
grid-jump-to-playing = 跳到正在播放
grid-library-empty = 媒体库是空的
grid-play-albums = 播放 { $count } 张专辑
grid-vertical-layout = 纵向布局
    .description = 上下滚动这面墙，每行填满宽度；关掉就左右滚动，每列填满高度
grid-follow-description = 每次换曲都滚动到正在播放的专辑
grid-resume-description = 停止浏览后滑回正在播放的专辑
grid-smooth-description = 滑到那张专辑，而不是直接跳
grid-section-dimming = 变暗
grid-section-tiles = 图块
grid-dim-while-playing = 播放时变暗
    .description = 让除正在播放的专辑外每张封面都淡下去；悬停能把某块重新点亮
grid-dim-amount = 变暗程度
    .description = 其他封面淡到什么程度；100% 就是藏起来
grid-desaturate = 播放时去色
    .description = 让除正在播放的专辑外每张封面都变灰；悬停能让某块恢复颜色
grid-always = 始终
    .description = 什么都没播时也让封面退到后面；只有悬停的那块完整显示
grid-show-titles = 显示标题
    .description = 像 iTunes 那样把专辑和艺术家印在每张封面下面，而不是只在悬停时显示
grid-title-alignment = 标题对齐
    .description = 让说明文字在封面下方对齐
grid-tile-size = 图块大小
    .description = 封面图块的最长边；各列均分面板宽度
grid-gap = 间距
    .description = 封面之间的空隙；为零就边挨边排满
grid-art-rounding-description = 把每张封面的角磨圆；100% 是圆形

## Settings: sidebar pages
settings-page-appearance = 外观
settings-page-application = 应用
settings-page-audio = 音频
settings-page-development = 开发
settings-page-integrations = 集成
settings-page-keymap = 快捷键
settings-page-library = 媒体库
settings-page-mcp = MCP
settings-page-ml-models = ML 模型
settings-page-playback = 播放
settings-page-providers = 数据源
settings-page-shader = 着色器
settings-page-storage = 存储
settings-page-workspace = 工作区

## Settings: appearance
settings-appearance-backdrop-all-windows = 所有窗口
    .description = 子窗口也铺背景：设置、编辑器、对话框、弹出的面板。关掉则背景和透明只用在工作区窗口上
settings-appearance-backdrop-strength = 背景强度
    .description = 封面背景在它们后面透出多少
settings-appearance-border = 边框
    .description = 每个面板边缘的一圈线，用“边框”角色的颜色；某一边为零就不画
settings-appearance-colors-locked-note = 歌曲配色开着，所以这些颜色由正在播放的曲目决定，导出也会保存它们。要编辑就在上面关掉
settings-appearance-design-mode = 设计模式
    .description = 就地编辑布局：面板菜单里的添加、重命名、复制、弹出和关闭各项，容器浮在槽位上的控件，以及标签页拖动。关掉后这些全都隐藏；“工作区”页仍然能编辑布局树
    .keywords = 编辑 布局 重排 锁定 bianji buju layout
settings-appearance-font = 字体
    .description = 全应用的字体；面板可以在自己的设置里覆盖
    .keywords = 字体 字型 文字 ziti wenzi font
settings-appearance-font-size = 字号
    .description = 每个面板文字缩放的基准大小；控件和图标保持原尺寸
settings-appearance-hide-menubar = 隐藏菜单栏
    .description = 一直藏着菜单栏，按住 Alt 时让它浮在停靠区上方。连按两下 Alt 让它留着，这样它的按钮能接普通点击
settings-appearance-icons-intro = 一个图标包就是一个装满 SVG 的文件夹，用来替换内置图标；切换在下次启动时生效
settings-appearance-icons-open-folder = 打开文件夹
settings-appearance-inverse-from-dark = 从深色主题反相
settings-appearance-inverse-from-light = 从浅色主题反相
settings-appearance-keep-theme = 保持主题
    .description = 即使封面亮度会把主题翻过去也保持当前主题；歌曲配色照样染色
settings-appearance-margin = 外边距
    .description = 把每个面板从它的格子里往里收；面板可以在自己的设置里覆盖
settings-appearance-new-pack = 新建图标包
settings-appearance-os-decorations = 系统窗口装饰
    .description = 主窗口上的系统标题栏和边框；关掉后就靠“窗口按钮”和“拖动锚点”面板
settings-appearance-pack-name-placeholder = 图标包名称
settings-appearance-padding = 内边距
    .description = 每个面板边缘以内的留白，仍算在它自己的背景里
settings-appearance-palette-export = 导出
settings-appearance-palette-import = 导入
settings-appearance-panel-seams = 面板接缝
    .description = 面板图块之间的细线；关掉后拖动手柄看不见，但照样能拖
settings-appearance-resize-border = 调整大小边框
    .description = 拖主窗口的边缘来改大小；只在关掉系统窗口装饰时生效，关掉它之后就只剩贴边和 Win+方向键这条路
settings-appearance-rounding = 圆角
    .description = 把每个面板的角磨圆，露出背景
settings-appearance-section-colors = 颜色
settings-appearance-section-frame = 边框
settings-appearance-section-icons = 图标
settings-appearance-section-interface = 界面
settings-appearance-section-theming = 配色
settings-appearance-section-transparency = 透明度
settings-appearance-section-typography = 字体排印
settings-appearance-song-theming = 歌曲配色
    .description = 用正在播放曲目的封面给调色板染色，并铺到窗口背景上
settings-appearance-surface-opacity = 表面不透明度
    .description = 应用的各个表面在背景之上有多不透明
settings-appearance-theme = 主题
    .description = 应用绘制用的调色板，也是下面颜色编辑器针对的那一套；“系统”跟随操作系统的浅色或深色偏好
settings-appearance-theme-dark = 深色
settings-appearance-theme-light = 浅色
settings-appearance-theme-system = 系统

## Settings: application
settings-application-check-updates = 检查更新
    .description = rox 启动时每天查一次有没有新版本；不管开不开，“关于”窗口都会当场检查
settings-application-download-updates = 下载更新
    .description = 检查到新版本时在后台下载并准备好；下次启动就用它
settings-application-enable-ai = 启用 AI 功能
    .description = 让 AI 工具跟 rox 对话：加上 MCP 支持和 ML 模型下载，对应的页面也会出现在侧栏。
settings-application-lock-panel-resize = 锁定面板大小
    .description = 只有开着设计模式时面板分隔才能调整大小，靠近接缝的拖动就动不了已经排好的布局
settings-application-portable-copying = 正在复制数据…
settings-application-portable-mode = 便携模式
    .description = 把设置、媒体库和缓存放在可执行文件旁边的 rox-data 文件夹里，播放器就能带着数据一起搬。关掉会回到系统文件夹，rox-data 留在原处
settings-application-portable-not-writable = 应用所在的文件夹不可写
settings-application-portable-restart-note = 下次启动生效；这次运行仍用当前文件夹
settings-application-remain-in-tray = 留在托盘
    .description = 最后一个窗口关闭时音乐继续播，托盘图标（macOS 上是程序坞）是回去的路
settings-application-section-ai = AI
settings-application-section-control-socket = 控制套接字
settings-application-section-data = 数据
settings-application-section-layout = 布局
settings-application-section-startup = 启动
settings-application-section-window = 窗口
settings-application-socket-path = 套接字路径
    .description = rox 运行时的机器接口：本地套接字上的 JSON-RPC，绑定到这个数据文件夹。rox-mcp 代理在它之上服务 MCP 客户端

## Settings: audio
settings-audio-broadcast-bitrate = 比特率
    .description = MP3 编码器每秒流花掉多少
settings-audio-broadcast-enable = 推流到 Icecast
    .description = 把 rox 正在播的内容当作源客户端推给 icecast 服务器，编码为 MP3。挂载点、听众和对外的网络那一面都归 icecast 管；rox 只往外连，服务器连不上也不会碰本地播放
settings-audio-broadcast-host-placeholder = icecast 主机
settings-audio-broadcast-login = 源登录
    .description = icecast 的源凭据，也就是它配置里写的用户名和密码
settings-audio-broadcast-mount = 挂载点
    .description = 听众连过来的挂载点，以及它对外报的流名称
settings-audio-broadcast-name-placeholder = 流名称
settings-audio-broadcast-password-placeholder = 源密码
settings-audio-broadcast-server = 服务器
    .description = icecast 服务器的主机和端口；源协议跑在普通套接字上
settings-audio-broadcast-user-placeholder = source
settings-audio-crossfade = 交叉淡化
    .description = 一首曲目和下一首重叠多久。淡化是给随机播放和跳曲用的，所以除非下面那行另有说明，专辑自身的曲目边界保持不变。为零就是关掉
    .keywords = 无缝 重叠 过渡 淡化 wufeng guodu gapless
settings-audio-equalizer-note = 输出之上的十个倍频程频段。它开在自己的窗口里，因为这是边听边调的东西，不是设一次就完
settings-audio-exclusive-mode = 独占模式
    .description = 把设备独占给 rox，并在硬件接受的情况下按文件自身的采样率跑；关掉则和桌面上其他一切共用系统混音器
settings-audio-fade-inside-albums = 专辑内也淡化
    .description = 同一张唱片里的曲目之间也重叠。关掉则唱片自身的接缝保持母带里的样子，而那正是无缝播放最要紧的地方
settings-audio-open-equalizer = 打开均衡器
settings-audio-output-buffer = 缓冲
    .description = 声卡一次握住多少音频。短一点反应更快，机器一忙就更早爆音；长一点更稳也更迟钝
settings-audio-output-buffer-default = 默认（10 毫秒）
settings-audio-output-device = 设备
    .description-default = 系统默认跟随桌面的设置
    .description-linux = 独占会直接从内核拿一张声卡，所以列表里是声卡而不是桌面的输出。蓝牙和其他声音服务器上的设备没有声卡可拿，只在关掉独占时出现
    .description-other = 独占把设备拿给 rox 一个用，所以在关掉这个模式之前，桌面上别的东西都发不出声
settings-audio-output-device-system-default = 系统默认
settings-audio-output-experimental-badge = 实验性
settings-audio-output-experimental-tooltip = 这个平台的独占后端是照着平台文档里的音频约定写的，但开发者从没在真实硬件上跑过。它应该拿到设备，或者给出理由后回退到共享，而不该没声音。要是它表现异常，关掉它，用这个标记旁边的按钮报告发生了什么。
settings-audio-output-format = 格式
    .description = rox 交给声卡的格式。接受不了所选格式的声卡会用它最宽的格式，下面的状态会显示是哪个
settings-audio-output-format-f32 = 32 位浮点
settings-audio-output-format-s16 = 16 位整数
settings-audio-output-format-s32 = 32 位整数
settings-audio-output-format-widest = 可用的最宽格式
settings-audio-output-issue-tooltip = 报告独占模式在这台机器上的表现。会打开一个已经填好平台和协商结果的 GitHub issue。
settings-audio-output-mode-exclusive = 独占
settings-audio-output-mode-shared = 共享
settings-audio-output-not-built = 这个平台还没有构建
settings-audio-output-rate-follow = 跟随文件
settings-audio-output-sample-rate = 采样率
    .description = 跟随会按每个文件自身的采样率重开设备，在采样率变化的边界上要付一小段间隙；锁定一个采样率不用付这个代价，但会把对不上的内容重采样
settings-audio-output-status-error-hint = 换个设备，或者关掉独占
settings-audio-output-status-error-title = 没有输出
settings-audio-output-status-idle-hint = 播一首曲目就能看到设备接受的格式
settings-audio-output-status-idle-title = 没在播放
settings-audio-replaygain-level-by = 电平依据
    .description = 让每首曲目按 ReplayGain 标签量到的响度播放，随机播放时就不会在不同母带之间蹦。“曲目”给每个文件单独定电平；“专辑”用整张唱片的增益覆盖它所有曲目，唱片自身的强弱段落就留在原处
    .keywords = 归一化 响度 音量均衡 guiyihua xiangdu normalization
settings-audio-replaygain-measure-missing-button = 测量缺失项
settings-audio-replaygain-measure-new = 测量新文件
    .description = 同步稳定之后，监视器带进来什么就测什么，媒体库不断增长也不用再回这里一趟。数字保存到“保存测得增益”指定的位置。打开这一项时会先问要不要把已经缺失的补上；之后它只处理新加进来的文件
settings-audio-replaygain-measuring-progress = 正在测量 { $done } / { $total }
settings-audio-replaygain-measuring-start = 正在测量：先算出缺哪些…
settings-audio-replaygain-mode-album = 专辑
settings-audio-replaygain-mode-off = 关
settings-audio-replaygain-mode-track = 曲目
settings-audio-replaygain-preamp = 前置增益
    .description = 加在每个标签增益上。ReplayGain 的参考值低于现在唱片的制作电平，所以做过电平均衡的媒体库放起来比原始音量更小；这里就是把它补回来。提升不会削波：标签里的峰值会给它封顶
settings-audio-replaygain-save = 保存测得增益
    .description = 测量结果放在哪里。媒体库数据库不动你的文件；标签则把同样的数值放到其他播放器都能读的地方，代价是重写音频文件
settings-audio-replaygain-status-measured = 扫描到的 { $total } 首曲目全都有可用的增益，其中 { $measured } 首由 rox 测得
settings-audio-replaygain-status-tagged = 扫描到的 { $total } 首曲目都有 ReplayGain 标签
settings-audio-replaygain-untagged = 无标签文件
    .description = 没有 ReplayGain 标签的文件按什么电平播放。没有测量结果，所以这只是个顶替用的估计值。保持为零，无标签的曲目就照旧播放
settings-audio-section-broadcast = 广播
settings-audio-section-equalizer = 均衡器
settings-audio-section-output = 输出
settings-audio-section-playback = 播放
settings-audio-section-replaygain = ReplayGain
settings-audio-transport = 播放控制
    .description = 不用离开这一页就能起停，因为下面每一项设置都得用耳朵判断

## Settings: integrations
settings-integrations-discord-enable = 启用 Rich Presence
    .description = 播放音乐时在 Discord 上显示 rox 的活动
settings-integrations-discord-show-lastfm = 显示 Last.fm 按钮
    .description = 在 Discord 状态里加一个可点击的“在 Last.fm 查看”按钮
settings-integrations-discord-show-youtube = 显示 YouTube 按钮
    .description = 在 Discord 状态里加一个可点击的“在 YouTube 搜索”按钮
settings-integrations-ffmpeg-binary = FFmpeg 可执行文件
    .description = 用哪个 ffmpeg 做转换；留空就用 PATH 上的那个
settings-integrations-ffmpeg-fail-note = 在把 ffmpeg 指向一个能用的可执行文件之前，“转换”一直藏着
settings-integrations-ffmpeg-fail-title = 这个 ffmpeg 跑不起来
settings-integrations-ffmpeg-missing-note = “转换”还藏着；装个 ffmpeg，或者把路径指向一个可执行文件
settings-integrations-ffmpeg-missing-title = 没找到能用的 ffmpeg
settings-integrations-ffmpeg-ok-note = ffmpeg 能用。“转换”已可用。
settings-integrations-ffmpeg-test = 测试
settings-integrations-lastfm-api-key-row = API 密钥
settings-integrations-lastfm-connect = 连接
settings-integrations-lastfm-disconnect = 断开连接
settings-integrations-lastfm-finish-connecting = 完成连接
settings-integrations-lastfm-hearts = { $n ->
   *[other] { $n } 个红心
}
settings-integrations-lastfm-import-loved = 导入喜爱的曲目
settings-integrations-lastfm-intro-builtin = 连接你的 Last.fm 账号：在浏览器里给 rox 授权，播过的曲目就会 scrobble 过去
settings-integrations-lastfm-intro-custom = 这个构建没有内置 api 身份，所以 scrobble 需要你自己的 api 账号（Last.fm/api/account/create）；把密钥和共享密钥贴进来，然后连接
settings-integrations-lastfm-key-placeholder = API 密钥
settings-integrations-lastfm-love-failed = 上一次失败了：{ $error }
settings-integrations-lastfm-love-pending = { $hearts } 等着发送
settings-integrations-lastfm-love-pending-failed = { $hearts } 等着发送，上次尝试：{ $error }
settings-integrations-lastfm-reconnect = 重新连接
settings-integrations-lastfm-secret-placeholder = 共享密钥
settings-integrations-lastfm-secret-row = 共享密钥
settings-integrations-lastfm-status-confirming = 正在确认…
settings-integrations-lastfm-status-connected = 已连接为 { $username }
settings-integrations-lastfm-status-elsewhere = 已经在另一个 rox 上连接过了；每一个都用自己的 api 身份授权，所以这一个也要连
settings-integrations-lastfm-status-failed = 连接失败：{ $error }
settings-integrations-lastfm-status-not-connected = 未连接
settings-integrations-lastfm-status-rejected = Last.fm 拒绝了这个会话，已经丢弃。重新连接才能继续 scrobble
settings-integrations-lastfm-status-requesting = 正在请求令牌…
settings-integrations-lastfm-status-waiting = 在浏览器里给 rox 授权，然后完成连接
settings-integrations-lastfm-working = 处理中…
settings-integrations-love-favourites = 收藏同步为喜爱
    .description = 把红心同步到 Last.fm 作为喜爱的曲目；取消红心那边也会取消
settings-integrations-scrobble-threshold = Scrobble 阈值
    .description = 一首曲目播到多少才 scrobble；进度条和波形可以标出这个位置
settings-integrations-scrobble-tracks = Scrobble 曲目
    .description = 播过的曲目越过阈值后发给 Last.fm
settings-integrations-section-conversion = 转换
settings-integrations-section-discord = Discord Rich Presence
settings-integrations-section-favourites = 收藏
settings-integrations-section-lastfm = Last.fm
settings-integrations-section-scrobbling = Scrobble

## Settings: keymap
settings-keymap-clash = { $chord } 也绑给了{ $other }；只有一个会触发
settings-keymap-not-bound = 未绑定
settings-keymap-recording = 按下按键
settings-keymap-restore = 恢复
settings-keymap-restore-all = 恢复全部快捷键
    .description = 把每个命令放回出厂的按键上，包括这个构建里已经没有对应行的那些
settings-keymap-section-defaults = 默认值
settings-keymap-undo = 撤销
settings-keymap-undo-last = 撤销上次重置
    .description = 把上次重置扔掉的快捷键找回来，单行或全部都行

## Settings: library
settings-library-acoustic-all-described = 扫描到的 { $total } 首曲目全都由 { $label } 描述过
settings-library-acoustic-auto = 描述新文件
    .description = 同步稳定之后，监视器带进来什么就描述什么，媒体库不断增长也不用再回这里一趟。关掉则新文件等着“分析缺失项”按钮。打开这一项时会先问要不要把已经缺失的补上；之后它只处理新加进来的文件
settings-library-acoustic-enable = 描述曲目听起来是什么样
    .description = 算出每首曲目听起来是什么样，媒体库就能找到和正在播放的相像的音乐。全部在这台机器上跑，描述一个大媒体库要花些时间
    .keywords = 相似 声音 指纹 描述 xiangsi zhiwen similar
settings-library-acoustic-extractor = 提取器
settings-library-acoustic-extractor-model = 模型
settings-library-acoustic-fallback = 分析中
settings-library-acoustic-partial = { $label } 描述了扫描到的 { $total } 首曲目中的 { $done } 首。“分析缺失项”会把剩下的做完
settings-library-acoustic-progress = { $running }：{ $done } / { $total }
settings-library-acoustic-progress-start = { $running }：先算出缺哪些…
settings-library-acoustic-save = 保存描述
    .description = 这一轮算出来的结果放在哪里。只用数据库不动你的文件；标签则在每个文件里也放一份，媒体库重建或文件夹换到别的机器上时描述还在，代价是重写音频文件。标签只支持 MP3 和 FLAC，其他格式一律只保留数据库那一份
settings-library-add-folder = 添加文件夹
settings-library-duplicates = 重复曲目…
settings-library-embed-button = 嵌入已存储的元数据…
settings-library-folder-col-albums = 专辑
settings-library-folder-col-folder = 文件夹
settings-library-folder-col-size = 大小
settings-library-folder-col-tracks = 曲目
settings-library-folders-intro = 扫描进媒体库的文件夹；移除一个会把它的曲目从目录里去掉，文件不动
settings-library-genre-separator-nudge = 分隔符已改：浏览立刻跟上。早先扫描存下的流派列表会保持原样，直到你按上面“文件夹”标题里的“重新扫描”
settings-library-merge-case = 合并大小写变体
    .description = 只有大小写不同的值算作同一个：Rock 和 rock 变成同一个流派、艺术家和专辑，按多数曲目的写法显示。文件里的标签保持原样
settings-library-no-folders = 还没有文件夹
settings-library-repair-tags = 修复标签…
settings-library-section-folders = 文件夹
settings-library-section-stored-metadata = 已存储的元数据
settings-library-section-tempo = 速度分析
settings-library-split-genres = 按逗号和斜杠拆分流派
    .description = “Dubstep, Trap”和“Drum & Bass / Neurofunk”里的每个值各算一个流派；分号一律拆分。关掉则保留带斜杠的完整名称，适合那些斜杠本就属于一个流派名的标签。文件里的标签保持原样
settings-library-tempo-auto = 给新文件测速
    .description = 同步稳定之后，监视器带进来什么就数它的拍子，媒体库不断增长也不用再回这里一趟。关掉则新文件等着“分析缺失项”按钮。打开这一项时会先问要不要把已经缺失的补上；之后它只处理新加进来的文件
settings-library-tempo-enable = 算出曲目跑多快
    .description = 给标签里没写速度的曲目数拍子，媒体库就能显示速度并按它排序。全部在这台机器上跑，数字进媒体库数据库，你的文件不动
settings-library-tempo-progress = 正在测速 { $done } / { $total }
settings-library-tempo-progress-start = 正在算出缺哪些…
settings-library-tempo-status-measured = 扫描到的 { $total } 首曲目全都有速度，其中 { $measured } 首由 rox 算出
settings-library-tempo-status-tagged = 扫描到的 { $total } 首曲目都有速度标签
settings-library-watch-folders = 监视文件夹
    .description = 文件的添加、修改和删除随时并进媒体库，不用手动重新扫描
settings-library-write-stored = 把已存储的内容写进文件
    .description = 三个保存设置只对下一次写入生效，所以在切到“标签”之前保存的东西还只在 rox 里。这一步会把 rox 里已有的歌词、增益和描述写进文件本身，别的播放器读这个文件夹时也就能看到。不会重新计算任何内容

## Settings: MCP
settings-mcp-client-config = 客户端配置
    .description = 贴进 MCP 客户端的服务器列表（Claude Code、Claude Desktop 或别的），它就能向 rox 查询媒体库、正在播放的内容和播放控制。rox 必须在运行；这些工具跑在它的控制套接字上
settings-mcp-enable = 启用 MCP 服务器
    .description = 响应已连接 MCP 客户端的工具调用。代理每次调用都会查这一项，所以关着时客户端会收到带理由的拒绝；下面的配置无论开关都能设好

## Settings: ML models
settings-mlmodels-checking = 正在检查…
settings-mlmodels-choose-file = 选择文件
settings-mlmodels-custom-description-empty = 给 rox 指一个你自己的 PANNs CNN10 检查点，safetensors 格式。它就地读取并按哈希命名，所以第二个检查点会单独描述媒体库，而不是沿用第一个的坐标系
settings-mlmodels-download-failed = { $label } 下载失败：{ $reason }
settings-mlmodels-downloading = 正在下载 { $label }：{ $done } / { $total }
settings-mlmodels-stopping = 正在停止 { $label } 的下载…
settings-mlmodels-fallback-model = 模型
settings-mlmodels-fallback-the-model = 这个模型
settings-mlmodels-kind-custom = 自定义
settings-mlmodels-kind-recommended = 推荐
settings-mlmodels-pass-stopped = 上一轮停下了：{ $reason }
settings-mlmodels-weights-file = 权重文件

## Settings: playback
settings-playback-continuation-continue = 续播
    .description = 顺着你起播的那个列表往下走，之后接媒体库剩下的。从某个视图中间播一张专辑，这个视图会继续下去
settings-playback-continuation-off = 关
    .description = 没有东西续上队列；播到底就停
settings-playback-continuation-weighted = 加权
    .description = 从整个媒体库里抽，没听过的排前面，最近听过的排最后
settings-playback-keep-playing = 继续播放
    .description = 队列播完之后放什么。选出来的内容会作为普通上下文追加到时间线上，看得见也删得掉，不是藏起来的状态。上面的顺序设成“相似”时，无论这里选哪一个，它都会继续找和正在播放的听起来像的曲目
    .keywords = 续播 自动播放 队列 xubo duilie autoplay
settings-playback-play-order = 播放顺序
    .description = 随机开着时已入队的曲目按什么顺序排。播放控制上的随机按钮负责开关；这里决定开了之后怎么排
settings-playback-rating-scale = 评分刻度
    .description = 星星适合随手点，0-10 半档适合细致打分
settings-playback-rating-scale-numeric = 0-10
settings-playback-rating-scale-stars = 星星
settings-playback-restore-last-session = 恢复上次会话
    .description = 启动时带上你离开时的播放队列，暂停在当时那首曲目和那个位置。媒体库文件夹之外的入队曲目恢复不了，会从顺序里掉出去
settings-playback-section-queue = 队列
settings-playback-section-ratings = 评分
settings-playback-section-startup = 启动
settings-playback-shuffle-random = 随机
    .description = 大家说随机时想的就是这个。接下来的曲目没有特定顺序
settings-playback-shuffle-similar = 相似
    .description = 按声音由近及远。接下来的曲目按和你打开它时正在播放的那首有多像来排，每次跳曲都重排。需要先在“媒体库”页把媒体库描述过
settings-playback-unrated-dots = 未评分小点
    .description = 用一个淡点标出没填的星位，而不是留空

## Settings: providers
settings-providers-artist = Last.fm
    .description = 为简介面板抓取艺术家小传、统计和相似艺术家，肖像来自 Deezer；全部存在数据文件夹里，之后离线也能读
settings-providers-deezer = Deezer
    .description = 在 Deezer 搜封面，最大 1000 像素
settings-providers-itunes = iTunes
    .description = 在 iTunes 搜封面；封面编辑器的搜索会列出候选，先挑再设
settings-providers-lastfm-art = Last.fm
    .description = 在 Last.fm 搜封面
settings-providers-lrclib = LRCLIB
    .description = 从 lrclib.net 抓缺失的歌词，有同步版就拿同步版
settings-providers-lyrics-intro = 只有面板动作要求时才会联网查询；播放和浏览从不碰网络
settings-providers-musicbrainz = MusicBrainz
    .description = 在 musicbrainz.org 上查标签；元数据面板的搜索会列出候选，写入前逐个字段确认
settings-providers-save-lyrics = 保存抓取的歌词
    .description = 抓来的歌词保存在哪里：rox 自己的数据文件夹（媒体库保持干净）、曲目旁边的 .lrc，或者内嵌标签
settings-providers-save-lyrics-data-folder = 数据文件夹
settings-providers-save-lyrics-sidecar = 同名文件
settings-providers-save-lyrics-tag = 标签
settings-providers-section-artist = 艺术家
settings-providers-section-cover-art = 封面
settings-providers-section-lyrics = 歌词
settings-providers-section-metadata = 元数据

## Settings: shader
settings-shader-backdrop-all-windows = 所有窗口
    .description = 给每个窗口的背景加着色：设置、编辑器、对话框、弹出的面板。关掉则只用在工作区窗口上
settings-shader-backdrop-enabled = 背景着色器
    .description = 在专辑封面背景上跑一个随音乐反应的 WGSL 着色器，位于所有面板之下。它属于工作区，所以会跟着这套外观一起走
settings-shader-backdrop-fallback-name = 背景
settings-shader-backdrop-run-idle = 空闲时运行
    .description = 什么都没播时也继续绘制。动画不论开关都保持静止
settings-shader-compile-error-title = 这个着色器没编译过
settings-shader-legacy-note = 没有任何路由时，信号池按自己的顺序填充各个槽位：第一个信号进槽位 0，第二个进槽位 1，以此类推。你加的第一条路由会接管整套映射。
settings-shader-overlay-enabled = 覆盖着色器
    .description = 在整个窗口上跑一个随音乐反应的 WGSL 着色器。只提供那些不会让底下的应用没法用的着色器
settings-shader-scene-covers-window = 这个着色器是场景，所以它盖住窗口，而不是画在上面。它来自某个包或旧配置；上面的列表只提供不会让应用没法用的着色器。
settings-shader-screen-all-windows = 所有窗口
    .description = 子窗口也加着色：设置、统计、均衡器、弹出的面板。还原倒计时无论如何都不加着色
settings-shader-screen-fallback-name = 屏幕
settings-shader-screen-run-idle = 空闲时运行
    .description = 什么都没播时也继续绘制。动画不论开关都保持静止。会读鼠标的着色器不开这一项也能在音乐停下时跟着光标走；只是指针停下两秒左右后它也跟着停
settings-shader-section-backdrop = 背景着色器
settings-shader-section-overlay = 覆盖着色器
settings-shader-signals-block = 信号
    .description = 着色器的十六个槽位各读哪个共享信号
settings-shader-slots-block = 槽位
    .description = 每个槽位送到着色器时的样子；没有路由的槽位就是手动旋钮

## Settings: storage
settings-storage-artist-images = 艺术家图片
    .description = 为艺术家视图抓取的肖像、横幅和小传（artists/）；清掉的会在下次打开视图时重新抓
settings-storage-catalog = 目录
    .description = 扫描建起来的曲目索引：一行一首曲目，带它的标签、文件细节和任何 cue 区段，存在 library.db 里
settings-storage-cover-thumbnails = 封面缩略图
    .description = 首次渲染后留下的小封面（thumbs.db）；清掉的会在滚动到时重建
settings-storage-logs = 日志
    .description = 每次运行为错误报告写下的内容（logs/rox.log），按大小上限滚动，不会长得太大
settings-storage-looks-layouts = 外观和布局
    .description = 应用当前使用的这套外观（workspace.json），连同你保存的工作区、导出的着色器文件和图标包。很小，而且每一个字节都是你亲手设的
settings-storage-lyrics = 歌词
    .description = 抓来的和编辑过的歌词存在应用自己的库里（lyrics/），媒体库文件夹就保持干净
settings-storage-measured-tempos = 测得的速度
    .description = rox 从音频里数出来的速度，用于标签里没写的曲目；标签自身的数字不动。清掉会把那些曲目放回“媒体库”页“分析缺失项”的名单里，改进过的节拍检测就能替换掉旧的那一轮写下的数字
settings-storage-model-fallback-this = 这个模型
settings-storage-music-summary = { $tracks }，{ $albums }，{ $size }
settings-storage-model-weights = 模型权重
    .description = 为声学分析下载的模型（models/）。“ML 模型”页负责下载和删除，一行一个模型
settings-storage-models-empty = 模型
    .description = 还没有模型描述过媒体库。在“媒体库”页打开声学分析就会填上这里，跑过的每个模型都会占一行
settings-storage-music-files = 音乐文件
    .description = 扫描到的文件夹里装的东西；文件留在原处
settings-storage-none = 无
settings-storage-playlists-history = 播放列表和历史
    .description = 你的播放列表和其中的成员、你听过什么，以及媒体库的流派记录。和 library.db 的其他部分比全都很小
settings-storage-reclaimable = 可回收空间
    .description = library.db 里删除留下的空页。新的写入会重新填上，所以文件是先停止增长，然后才开始变小
    .keywords = 整理 压缩 收缩 数据库 zhengli yasuo vacuum
settings-storage-section-acoustic = 声学描述
settings-storage-section-app-data = 应用数据
settings-storage-section-caches = 缓存
settings-storage-section-diagnostics = 诊断
settings-storage-section-library = 媒体库
settings-storage-section-tempo = 速度
settings-storage-vectors = 向量
    .description = 每条描述在 library.db 里占多少。在分析跑过一遍的媒体库上，这就是文件的大头，一首曲目几千字节，对比标签的几百字节
settings-storage-waveforms = 波形
    .description = 每首曲目首次播放后留下的峰值条；清掉的会在下次播放时重新解码

## Settings: workspace
settings-workspace-card-author = 作者
settings-workspace-card-author-placeholder = 谁做的
settings-workspace-card-created = 创建于 { $date }
settings-workspace-card-created-updated = 创建于 { $created }，更新于 { $updated }
settings-workspace-card-description = 说明
settings-workspace-card-description-placeholder = 这套外观想做什么
settings-workspace-card-empty = 这个工作区没有卡片
settings-workspace-card-hint = 卡片存在文件里，所以你把这套外观分享给谁，谁就看得到
settings-workspace-card-license = 许可
settings-workspace-card-license-placeholder = 你按什么条款分享
settings-workspace-card-save = 保存卡片
settings-workspace-card-updated = 更新于 { $date }
settings-workspace-card-version = 版本
settings-workspace-card-version-placeholder = 你自己的版本号，怎么数都行
settings-workspace-card-website = 网站
settings-workspace-card-website-placeholder = 它在哪儿能找到
settings-workspace-composition-closed = 工作区窗口已关闭
settings-workspace-composition-hint = 窗口里的面板在分隔和标签组里的排布；箭头把某一行在同级中重排，锁把面板钉在原地，齿轮打开它的设置
settings-workspace-empty = 还没有工作区
settings-workspace-hint = 一个工作区就是一整套外观：布局、调色板、外观设置。应用一个会把这三样全换掉
settings-workspace-layout-name-placeholder = 布局名称
settings-workspace-layouts-empty = 还没有布局
settings-workspace-layouts-hint = 主布局和迷你布局，就是菜单栏迷你播放器按钮来回切换的那两个
settings-workspace-name-placeholder = 工作区名称
settings-workspace-panel-preset-unknown-kind = 未知面板
settings-workspace-panel-presets-empty = 还没有面板预设
settings-workspace-panel-presets-hint-after = 加回来，任意面板菜单里都行。它们只属于这个工作区，别的工作区没有。
settings-workspace-panel-presets-hint-before = 一个预设一个配置好的面板，从面板自己的菜单里保存，再从
settings-workspace-role-mini = 迷你
settings-workspace-role-primary = 主
settings-workspace-section-composition = 构成
settings-workspace-section-layouts = 布局
settings-workspace-section-panel-presets = 面板预设
settings-workspace-section-workspaces = 工作区
settings-workspace-tree-empty-slot = 空槽位
settings-workspace-tree-split-column = 分隔，上下叠放
settings-workspace-tree-split-row = 分隔，左右并排
settings-workspace-tree-tabs = 标签页

## Settings: development
settings-development-experimental-panels = 实验性面板
    .description = 在“面板”菜单和启动器里显示还在做的面板；它们在版本之间会变形，已经放了一个的布局在这项关掉后仍然留着它
settings-development-section-features = 功能

## Settings: shared
settings-acoustic-analysis-heading = 声学分析
settings-analyze-nothing-scanned = 还没扫描到可分析的内容
settings-common-active = 使用中
settings-common-analyze-missing = 分析缺失项
settings-common-built-in = 内置
settings-common-clear = 清除
settings-common-copy = 复制
settings-common-database = 数据库
settings-common-delete = 删除
settings-common-download = 下载
settings-common-rescan = 重新扫描
settings-common-reveal = 在文件管理器中显示
settings-common-stop = 停止
settings-common-stopping = 正在停止…
settings-common-tags = 标签
settings-common-tracks-count = { $count } 首曲目
settings-common-use = 使用
settings-confirm-apply-body = 这会用这个工作区的内容替换你的布局、调色板和外观设置。
settings-confirm-apply-imported-body = 它已经保存到你的工作区里。现在应用会用它的内容替换你的布局、调色板和外观设置。
settings-confirm-clear = 清除
settings-confirm-clear-embeddings-body = 描述会消失，空间会回来。想再要一次，就得对媒体库里每首曲目重跑一遍分析。
settings-confirm-clear-embeddings-title = 清除“{ $model }”描述的内容？
settings-confirm-clear-measured-bpm-body = rox 算出来的每个速度都回到未测状态；来自你文件自身标签的数字保留。想再要一次，就得对那些曲目重跑一遍速度分析。
settings-confirm-clear-measured-bpm-title = 清除测得的速度？
settings-confirm-overwrite-workspace-body = 这会用当前状态替换已保存的工作区。
settings-confirm-overwrite-workspace-title = 覆盖工作区“{ $name }”？
settings-sidebar-data-folder = 数据文件夹
settings-sidebar-settings-file = 设置文件

## Menubar
menu-about = 关于
menu-application = 应用
menu-apply-layout = 应用布局
menu-apply-workspace = 应用工作区
menu-chat = 聊天
menu-close = 关闭
menu-console = 控制台
menu-design-mode = 设计模式
menu-discussions = 讨论
menu-empty-window = 空窗口
menu-equalizer = 均衡器
menu-exit = 退出
menu-hide-menubar = 隐藏菜单栏
menu-import-workspace = 导入工作区…
menu-new-ellipsis = 新建…
menu-new-window = 新建窗口
menu-new-window-from-layout = 从布局新建窗口
menu-new-window-from-panel = 从面板新建窗口
menu-no-layouts = 没有布局
menu-no-presets = 没有预设
menu-no-workspaces = 没有工作区
menu-os-decorations = 系统窗口装饰
menu-overlay-shader = 覆盖着色器
menu-panel-built-in = 内置
menu-panel-new = 新建…
menu-panel-no-layouts = 没有布局
menu-panel-no-presets = 没有预设
menu-panel-no-workspaces = 没有工作区
menu-panel-title = 菜单
menu-panels = 面板
menu-panels-presets = 预设
menu-pause = 暂停
menu-playback = 播放
menu-remain-in-tray = 留在托盘
menu-report-issue = 报告问题
menu-save-layout = 保存布局
menu-save-workspace = 保存工作区
menu-section-add = 添加
menu-section-app = 应用
menu-section-interface = 界面
menu-section-layouts = 布局
menu-section-library = 媒体库
menu-section-session = 会话
menu-section-track = 曲目
menu-section-tuning = 调校
menu-settings = 设置
menu-signals = 信号
menu-song-theming = 歌曲配色
menu-stats = 统计
menu-tasks = 任务
menu-welcome = 欢迎
menu-window = 窗口
menu-workspace = 工作区
menu-workspace-builtin-tag = 内置

## Workspaces
workspace-apply-body = 这会替换整套外观：布局、调色板、外观设置。
workspace-apply-imported-body = 它已经保存到你的工作区里。现在应用会替换整套外观：布局、调色板、外观设置。
workspace-apply-imported-title = 已导入“{ $name }”
workspace-apply-screen-shader-named = 会在整个窗口上应用 { $name } 覆盖着色器。
workspace-apply-screen-shader-plain = 会在整个窗口上应用一个覆盖着色器。
workspace-apply-shader-count = { $count ->
   *[other] 包含 { $count } 个着色器：{ $names }
}
workspace-apply-shaders-approve-body = 批准之后它们就能在这台机器上运行。不带它们应用，这套外观会是光秃秃的，着色器仍留在它的池子里。
workspace-apply-shaders-plain-body = 不带它们应用，这套外观会是光秃秃的，着色器仍留在它的池子里。
workspace-byline-author = 作者 { $author }
workspace-byline-version = 版本 { $version }
workspace-context-add-panel = 添加面板
workspace-dialog-apply = 应用
workspace-dialog-apply-title = 应用“{ $name }”？
workspace-dialog-approve-apply = 批准并应用
workspace-dialog-cancel = 取消
workspace-dialog-close = 关闭
workspace-dialog-close-title = 关闭“{ $name }”？
workspace-dialog-export = 导出
workspace-dialog-layout-name-placeholder = 布局名称
workspace-dialog-not-now = 暂不
workspace-dialog-overwrite = 覆盖
workspace-dialog-overwrite-title = 覆盖“{ $name }”？
workspace-dialog-save = 保存
workspace-dialog-save-layout-title = 保存布局
workspace-dialog-save-workspace-title = 保存工作区
workspace-dialog-with-shaders = 带着色器
workspace-dialog-without-shaders = 不带着色器
workspace-dialog-workspace-name-placeholder = 工作区名称
workspace-drop-add-queue = 加入队列
workspace-drop-play-now = 立即播放
workspace-hint-or = 或
workspace-hint-then = 然后
workspace-import = 导入
workspace-launcher-hint = 添加第一个面板开始搭建，或者在“工作区 > 应用工作区”里挑一个预设
workspace-launcher-need-help = 需要帮助？
workspace-launcher-open-welcome = 打开欢迎窗口
workspace-launcher-title = 一个空窗口
workspace-layout-apply-body = 这会替换这个窗口当前的布局。
workspace-layout-overwrite-body = 这会用当前布局替换已保存的布局。
workspace-layout-preset-restore-failed = 这个窗口的布局预设恢复不了，所以它是空的。
workspace-layout-restore-failed = 保存的布局恢复不了，所以这个窗口是空的。
workspace-mini-tip-back = 回到完整布局
workspace-mini-tip-shrink = 收成迷你播放器
workspace-overwrite-body = 这会用当前外观替换已保存的工作区。
workspace-panel-locked-close-body = 这个面板被钉住了。关掉它会把它从布局里去掉。
workspace-save-current = 保存当前
workspace-screen-shader-hint-before = 随时可以关掉它，用
workspace-workspace-restore-failed = 这个工作区的布局恢复不了，所以这个窗口是空的。

## Tasks window
tasks-acoustic-all-described = 扫描到的 { $count } 首曲目全都由 { $label } 描述过
tasks-acoustic-off = “描述曲目听起来是什么样”在设置的“媒体库”里关着
tasks-acoustic-partial = { $label } 描述了扫描到的 { $total } 首曲目中的 { $embedded } 首
tasks-analyzing = 正在分析 { $progress }
tasks-bake-writing = 正在写入标签…
tasks-chip-count = { $count } 个任务
tasks-convert-starting = 正在启动 ffmpeg…
tasks-converting = 正在转换 { $progress }
tasks-count-of-total = { $done } / { $total }
tasks-embedding = 正在嵌入 { $progress }
tasks-estimate-at = { $estimate }，用 { $workers }
tasks-import-failed = 上次导入失败了：{ $error }
tasks-import-reading = 正在读取喜爱列表…
tasks-import-unmatched = { $count } 首在这个媒体库里没有对应
tasks-importing = 正在导入 { $progress }
tasks-job-acoustic = 声学分析
tasks-job-convert = 转换音频
tasks-job-loved-import = Last.fm 喜爱的曲目
tasks-job-replaygain = ReplayGain
tasks-job-scan = 媒体库扫描
tasks-job-tempo = 速度分析
tasks-last-pass-stopped = 上一轮停下了：{ $reason }
tasks-last-run-finished = 上次运行完成，做了 { $count } 个
tasks-last-run-stopped = 上次运行在 { $count } 个之后停下
tasks-library-busy = 媒体库正忙
tasks-library-scanning = 媒体库正在扫描
tasks-measuring = 正在测量 { $progress }
tasks-model-downloading = 还有模型在下载
tasks-no-library-window = 没有打开的媒体库窗口，所以这些没法从这里启动
tasks-nothing-to-measure = 还没扫描到可测量的内容
tasks-rg-all-gain = { $count } 首曲目全都有播放用的增益
tasks-rg-partial = { $total } 首曲目里有 { $missing } 首没有增益
tasks-scan-folder-count = { $count ->
   *[other] { $count } 个文件夹
}
tasks-scan-last-scanned = { $folders }，上次扫描在 { $ago }前
tasks-scan-never-scanned = { $folders }，从未扫描
tasks-scan-no-folders = 还没添加文件夹。在设置的“媒体库”里加一个
tasks-start-analyze-missing = 分析缺失项
tasks-start-measure-missing = 测量缺失项
tasks-start-rescan = 重新扫描
tasks-stop = 停止
tasks-stopping = 正在停止…
tasks-tempo-all = { $count } 首曲目全都有速度
tasks-tempo-off = “算出曲目跑多快”在设置的“媒体库”里关着
tasks-tempo-partial = { $total } 首曲目里有 { $missing } 首没有速度
tasks-timing = 正在测速 { $progress }
tasks-tip = 打开媒体库任务
tasks-window-title = rox - 任务
tasks-working-out-missing = 正在算出缺哪些…

## Stats window
stats-bucket-listens = { $count ->
   *[other] { $count } 次收听，{ $ago }
}
stats-chart-start-all = 首次收听
stats-chart-start-month = 30 天前
stats-chart-start-week = 7 天前
stats-chart-start-year = 一年前
stats-click-opens = 点击打开统计
stats-click-section = 点击
stats-count-menu = 计数
    .description = 这个数字统计最近哪一段时间的收听；悬停列表里始终全都有
stats-empty-all = 还没有收听记录
stats-empty-range = 这个区间没有收听记录
stats-now = 现在
stats-open = 打开统计
stats-open-on-click = 点击打开统计
    .description = 点这个部件打开统计窗口，也就是完整的收听记录
stats-play-these-tracks = 播放这些曲目
stats-play-this-track = 播放这首曲目
stats-plays-count = { $count ->
   *[other] { $count } 次播放
}
stats-range-all = 全部时间
stats-range-all-short = 全部
stats-range-day-short = 日
stats-range-label = 区间
stats-range-month = 本月
stats-range-month-short = 月
stats-range-today = 今天
stats-range-week = 本周
stats-range-week-short = 周
stats-range-year = 今年
stats-range-year-short = 年
stats-readout-section = 读数
stats-section-listens = 收听
stats-section-listens-over-time = 收听随时间变化
stats-section-recent-listens = 最近收听
stats-section-top-albums = 专辑排行
stats-section-top-artists = 艺术家排行
stats-section-top-genres = 流派排行
stats-show-change = 显示变化
    .description = 加一个小标签，显示这一段和上一段比是升还是降；“全部时间”后面没有可比的
stats-show-number = 显示数字
    .description = 在图标旁边画出计数；关掉就只剩图标，计数在悬停时显示
stats-title = 统计部件
stats-tooltip-listens = 收听
stats-window-title = rox - 统计

## About window
about-check-failed = 连不上 GitHub
about-check-for-updates = 检查更新
about-checking = 正在检查…
about-download = 下载
about-downloading = 正在下载… { $percent }%
about-get-it = 去下载
about-license-lead = rox 是 GNU AGPLv3 下的自由软件。源码在
about-notice-lead = 你应该随本程序收到一份许可证副本。如果没有，请见
about-release-notes = 发行说明
about-restart-now = 立即重启
about-up-to-date = 你用的已经是最新版本
about-update-failed = 更新失败：{ $error }
about-version = 版本 { $version }
about-version-available = 有新版本 { $version }
about-version-ready = 版本 { $version } 已就绪
about-window-title = rox - 关于

## Welcome window
welcome-add-folder = 添加文件夹
welcome-and = 和
welcome-back = 上一步
welcome-card-menubar-title = 菜单栏
welcome-card-music-title = 音乐
welcome-card-panels-title = 面板
welcome-card-playback-title = 播放
welcome-card-rearranging-title = 重新排布
welcome-card-settings-title = 设置
welcome-close = 关闭
welcome-design-mode-note = 重新排布需要设计模式，默认在那个菜单顶部开着。关掉会锁住布局，排好的界面就碰不歪了。
welcome-done = 完成
welcome-drop-note = 拖到某个面板的边缘就在那里分隔，拖到中间就并进同一个标签组，拖到窗口外就变成独立窗口。
welcome-key-left-click = 左键
welcome-key-middle-mouse = 中键
welcome-layout-note = 把一套排布存成布局；一个工作区把布局和调色板打包成一套可以分享的外观。
welcome-menubar-after = 两下让它留着。
welcome-menubar-before = 菜单栏藏起来时，按住
welcome-menubar-mid = 让它浮回停靠区上方，或者点
welcome-music-note = rox 把它扫描进媒体库，文件留在原处。更多文件夹在设置的媒体库里加。
welcome-next = 下一步
welcome-or = 或
welcome-panels-note = 每一块表面都是一个面板，菜单栏的“面板”菜单里还有更多。
welcome-playback-after = 快进快退。
welcome-playback-before = 切换播放；
welcome-quickplay-after = 就播了。
welcome-quickplay-before = 打开快速播放：输入一首曲目，按
welcome-rearrange-after = 面板里任意处，就能移动它。
welcome-rearrange-before = 拖动标签页，或者按住
welcome-settings-hint-after = 打开设置：调色板、透明度和行为。
welcome-shelf-caption = 选一个就会替换主窗口的外观并结束导览。这个窗口随时能从“应用 > 欢迎”打开。
welcome-stage-lead-quick-start = 挑一个工作区，主窗口就会切换过去：布局、调色板、整套外观。
welcome-stage-lead-welcome = 如果 Foobar2000 是 20XX 年做的。
welcome-stage-title-quick-start = 快速上手
welcome-stage-title-welcome = 欢迎使用 rox
welcome-step-hint-after = ，或者用下面的按钮。
welcome-step-hint-before = 一步步走完，用
welcome-tile-by = 作者 { $author }
welcome-tour-intro = 快速看一遍音乐从哪儿进来、外观在哪儿设置。最后停在随附工作区的陈列架上，每一个点一下就能用。
welcome-window-title = rox - 欢迎

## Console window
console-clear = 清空
console-copy = 复制
console-empty-filtered = 这些级别下没有内容
console-empty-none = 还没有日志
console-filter-error = 错误
console-filter-info = 信息
console-filter-warn = 警告
console-follow = 跟随
console-line-count = { $count ->
   *[other] { $count } 行
}
console-open-button = 打开控制台
console-reveal = 在文件管理器中显示
console-window-title = rox - 控制台

## Signals window
signals-about-toggle = 关于信号
signals-blurb-marked = 菜单里带这个标记的面板，大部分参数都能绑定：在面板设置里右键某个参数，选一个信号，或者直接从那里加一个。
signals-blurb-shared = 在这里调的东西是共享的：一处改动会作用到路由到那个信号的每个参数上，跨面板、跨窗口都一样。
signals-blurb-total = 累加是第四种：它把另一个信号随时间加起来，到 1 就回绕，所以音乐响时它往上升，不响时它停住。着色器需要一个跟着歌走而不是跟着时钟走的相位时，就用它。
signals-blurb-what = 一个信号把正在播放的内容变成 0 到 1 之间的一个数：某个频段的能量、整个混音的电平，或者某个频段里每次击打的脉冲。“响应”决定它跟得多快，“阈值”让它在你挑的电平以下保持安静。
signals-no-library = 没有打开的媒体库窗口，所以这些看不到音频。编辑照样保存。
signals-window-title = rox - 信号

## Equaliser
eq-analyzer-bars = 柱状
eq-analyzer-off = 无分析器
eq-analyzer-wave = 波形
eq-band-badge = 频段角标
    .description = 在图标上的角标里显示有多少个频段偏离平直
eq-band-label = 频段 { $number }
eq-click-nothing = 无
eq-click-open = 打开
eq-click-section = 点击
    .description = 点击做什么：打开均衡器窗口，或者就地把整条曲线开关一下
eq-click-toggle = 开关
eq-flatten = 拉平
eq-freq-label = 频率
eq-gain-label = 增益
eq-heading = 均衡器
eq-help-text = 拖动某个频段来移动它，在它上面滚动可以变宽或变窄。处理发生在送往声卡的缓冲之前，所以一次调整最多要半秒才传到音箱。
eq-hint-off = 点击关闭
eq-hint-on = 点击开启
eq-hint-open = 点击打开均衡器
eq-open = 打开均衡器
eq-readout-curve = 曲线
eq-readout-icon = 图标
eq-readout-section = 读数
    .description = 图标、作为迷你折线的响应曲线，或者两者都要。曲线大约要五十像素宽才看得清
eq-reset-bands = 重置频段
eq-shape-active = { $count ->
   *[other] { $count } 个频段偏离平直，峰值 { $peak } dB
}
eq-shape-flat = 平直，每个频段都在 0 dB
eq-status-off = 均衡器已关
eq-status-on = 均衡器已开
eq-title = 均衡器部件
eq-widget-section = 部件
eq-width-label = 宽度
eq-window-title = rox - 均衡器

## Keymap
keymap-close-window = 关闭窗口
    .description = 关掉最前面的那个窗口。到处都绑着，弹出的面板也算
keymap-decrease-font-size = 减小文字
    .description = 把全应用的文字调小一档
keymap-focus-search = 聚焦搜索
    .description = 把光标放进媒体库搜索框
keymap-group-editing = 编辑
keymap-group-playback = 播放
keymap-group-view = 视图
keymap-group-windows = 窗口
keymap-increase-font-size = 放大文字
    .description = 把全应用的文字调大一档
keymap-key-backspace = 退格
keymap-key-delete = Delete
keymap-key-down = 下
keymap-key-end = End
keymap-key-esc = Esc
keymap-key-home = Home
keymap-key-insert = Insert
keymap-key-left = 左
keymap-key-page-down = Page Down
keymap-key-page-up = Page Up
keymap-key-right = 右
keymap-key-space = 空格
keymap-key-tab = Tab
keymap-key-up = 上
keymap-mod-alt = Alt
keymap-mod-cmd = Cmd
keymap-mod-ctrl = Ctrl
keymap-mod-fn = Fn
keymap-mod-option = Option
keymap-mod-shift = Shift
keymap-mod-super = Super
keymap-mod-win = Win
keymap-open-quick-play = 快速播放
    .description = 在窗口上方唤出搜索即播的输入框
keymap-open-settings = 打开设置
    .description = 打开这个窗口
keymap-open-stats = 打开统计
    .description = 打开收听统计窗口
keymap-quit = 退出
    .description = 离开 rox。到处都绑着，因为没有哪个窗口不该能用它
keymap-reset-font-size = 重置文字大小
    .description = 把文字大小拉回出厂值
keymap-seek-backward = 快退
    .description = 在正在播放的曲目里往回走一步
keymap-seek-forward = 快进
    .description = 在正在播放的曲目里往前走一步
keymap-stamp-line = 标记歌词行时间
    .description = 把当前播放位置写到正在编辑的那一行歌词上
keymap-toggle-playback = 播放 / 暂停
    .description = 起播当前曲目，或者就地暂停
keymap-toggle-post-shader = 切换覆盖着色器
    .description = 关掉和打开屏幕着色器。到处都绑着，因为着色器可能把你用来关掉它的控件全都盖住
keymap-toggle-zoom = 缩放面板组
    .description = 让最后点过的面板组填满停靠区，或者退出来

## Panel catalog
panel-catalog-album-carousel = 专辑转盘
panel-catalog-artist-grid = 艺术家墙
panel-catalog-biography = 简介
panel-catalog-cover-art = 封面
panel-catalog-drawer = 抽屉
panel-catalog-eq-widget = 均衡器部件
panel-catalog-filter = 筛选
panel-catalog-folder-tree = 文件夹树
panel-catalog-genre-grid = 流派墙
panel-catalog-group-application = 应用
panel-catalog-group-arrangement = 排布
panel-catalog-group-catalogue = 目录
panel-catalog-group-controls = 控件
panel-catalog-group-details = 详情
panel-catalog-group-experimental = 实验性
panel-catalog-group-visualizers = 可视化
panel-catalog-history = 播放历史
panel-catalog-menu = 菜单
panel-catalog-metadata = 元数据
panel-catalog-mini-toggle = 迷你切换
panel-catalog-oscilloscope = 示波器
panel-catalog-overlay = 覆盖层
panel-catalog-particles = 粒子
panel-catalog-playlists = 播放列表
panel-catalog-queue = 队列
panel-catalog-queue-widget = 队列部件
panel-catalog-seek = 进度
panel-catalog-slide = 幻灯片
panel-catalog-spectrogram = 频谱图
panel-catalog-spectrum = 频谱
panel-catalog-stats-widget = 统计部件
panel-catalog-status = 状态
panel-catalog-theme-toggle = 主题切换
panel-catalog-track-info = 曲目信息
panel-catalog-vu-meter = VU 表
panel-catalog-waveform = 波形
panel-catalog-window-controls = 窗口按钮

## Updater
updater-already-latest = 已经是最新版本
updater-checksum-mismatch = 下载文件的校验和是 { $digest }，不是这个版本写明的 { $expected }
updater-checksum-missing-entry = { $sums } 里没有 { $name } 的条目；拒绝无法校验的下载
updater-no-asset = 这个版本没有 { $name }
updater-no-checksums = 这个版本没有 { $sums }；拒绝无法校验的下载
updater-no-release-build = 这个平台没有发行版构建
updater-overran = 下载超过了这个版本写明的大小
updater-short = 下载停在了 { $bytes } 字节中的 { $done }
updater-size-mismatch = 服务器给的是 { $claimed } 字节，这个版本写明的是 { $bytes }

## Last.fm
lastfm-import-matching = 正在与媒体库比对
lastfm-import-read = 已读取 { $count } 首喜爱的曲目
lastfm-import-stopped = 在 { $count } 首喜爱的曲目之后停下
lastfm-import-matched = ，匹配 { $count } 首
lastfm-import-added = ，加入收藏 { $count } 首

## Tag tools
tags-editor-clear-all = 全部清空
tags-editor-form-view = 表单
tags-editor-format-unsupported-all = 这种格式的标签还读不了也写不了。
tags-editor-format-unsupported-some = 这些文件里有一部分的格式，标签还读不了也写不了。
tags-editor-guess-button = 猜测
tags-editor-guess-folded = { $status }，另有 { $count } 条未显示
tags-editor-guess-help = { $placeholders }；/ 对应上一层文件夹，%skip% 丢弃
tags-editor-guess-match-count = { $total } 个里匹配 { $hits } 个
tags-editor-guess-no-match = 无匹配
tags-editor-guess-pattern-label = 模式
tags-editor-loading = 正在加载标签…
tags-editor-look-up = 查询
tags-editor-multiple-values = 多个值
tags-editor-clear-on-save = 保存时清空
tags-editor-other-tags = 其他标签（{ $count }）
tags-editor-remove = 移除
tags-editor-reveal = 在文件管理器中显示
tags-editor-save-errors = { $count } 个文件失败；{ $error }
tags-editor-saving-progress = 正在保存 { $done }/{ $total }…
tags-editor-table-view = 表格
tags-editor-tags-section = 标签
tags-editor-unknown-partial = { $total } 个里 { $count } 个
tags-editor-unread-count = { $total } 个文件里有 { $failed } 个的标签读不出来
tags-editor-will-clear = 将清空
tags-editor-will-remove = 将移除
tags-editor-window-title = rox - 标签编辑器
tags-guess-empty-segment = 模式渲染出空的文件夹名或文件名
tags-guess-no-placeholders = 没有占位符
tags-guess-skip-renders-nothing = %skip% 没有可渲染的内容
tags-guess-unclosed = % 未闭合
tags-guess-unknown-placeholder = 未知占位符 %{ $name }%
tags-matcher-blocked-arm = 先启用一个字段才能应用
tags-matcher-blocked-no-match = 没有可应用的匹配
tags-matcher-blocked-pick = 选一个匹配
tags-matcher-blocked-writing = 正在写入标签…
tags-matcher-match-count = { $count ->
   *[other] { $count } 个匹配
}
tags-matcher-no-matches = 没找到匹配
tags-matcher-pick-match = 选一个匹配
tags-matcher-search-failed = 搜索失败：{ $error }
tags-matcher-searching = 正在搜索…
tags-matcher-tagging = 正在写入 { $track }
tags-matcher-window-title = rox - 查找元数据
tags-rename-blocked-cue = cue 曲目，没有自己的文件
tags-rename-blocked-duplicate = 两首曲目映射到同一个名字
tags-rename-blocked-occupied = 那里已经有文件了
tags-rename-blocked-outside-roots = 在所有媒体库根目录之外
tags-rename-blocked-unresolved = 还不在目录里
tags-rename-move-error = { $name }：{ $error }
tags-rename-move-errors = { $count } 个文件失败；{ $error }
tags-rename-moving = 正在移动 { $done }/{ $total }…
tags-rename-nothing-to-move = 没有可移动的内容
tags-rename-pattern-help = { $placeholders }；/ 建一层文件夹，扩展名沿用原文件的
tags-rename-pattern-section = 模式
tags-rename-preview-section = 预览
tags-rename-unchanged = 不变
tags-rename-will-move = { $total } 个里将移动 { $count } 个
tags-rename-window-title = rox - 重命名文件
tags-repair-affected-files = 受影响的文件
tags-repair-section = 修复
tags-repair-check-to-repair = 勾选文件来修复它
tags-repair-count = { $count ->
   *[other] { $count } 个文件
}
tags-repair-count-so-far = 目前 { $count } 个
tags-repair-label-scope = 范围
tags-repair-no-affected = 没找到受影响的文件。
tags-repair-no-folder = 没有可扫描的文件夹；往媒体库里加一个，或者挑一个。
tags-repair-pick-folder = 选一个文件夹…
tags-repair-progress = 正在修复 { $done }/{ $total }…
tags-repair-repair-button = { $count ->
    [0] 修复
   *[other] 修复（{ $count }）
}
tags-repair-result = { $count ->
   *[other] 已修复 { $count } 个文件
}
tags-repair-result-failed = 修复了 { $count } 个，{ $failed } 个失败
tags-repair-scan-first = 先扫描
tags-repair-scan-hint = 扫描一遍，找出标签有损、重写就能修好的文件。
tags-repair-select-all = 全选
tags-repair-select-none = 全不选
tags-repair-whole-library = 整个媒体库
tags-repair-window-title = rox - 标签修复

## Convert
convert-arg-names-file = “{ $token }”指的是一个文件；目标位置由文件夹和模式决定
convert-section-output = 输出
convert-section-preview = 预览
convert-arg-not-flag-or-value = “{ $token }”既不是选项，也不是某个选项的值
convert-check-wrote-nothing = ffmpeg 干净退出，但什么都没写
convert-custom-ext-empty = 容器由扩展名决定，所以必须填一个
convert-custom-ext-invalid = “{ $ext }”不是容器名；只能用字母和数字，不带点
convert-dialog-browse = 浏览…
convert-dialog-check-passed = ffmpeg 用这些参数编码了一小段静音，所以它们能跑
convert-dialog-check-waiting = 你停下打字后会拿去给 ffmpeg 检查
convert-dialog-checking = 正在用 ffmpeg 检查…
convert-dialog-choose-folder = 选一个写进去的文件夹
convert-dialog-convert-button = 转换
convert-dialog-custom-label = 自定义
convert-dialog-custom-menu-item = 自定义…
convert-dialog-custom-note = 参数按空格拆分，所以不能加引号；自定义格式不会复制内嵌封面
convert-dialog-format-not-ready = 输入的格式还没通过 ffmpeg 检查
convert-dialog-label-extension = 扩展名
convert-dialog-label-format = 格式
convert-dialog-label-into = 写入
convert-dialog-label-named = 命名为
convert-dialog-mirror = 照搬媒体库的文件夹结构
convert-dialog-nothing-to-convert = 没有可转换的内容：每一行都被跳过了
convert-dialog-pattern-help = { $placeholders }；/ 建一层文件夹，扩展名由格式决定
convert-dialog-pick-folder = 挑一个写进去的文件夹
convert-dialog-span-note = { $count } 首从 cue 整轨里截出来，并从媒体库取标签
convert-dialog-will-convert = { $total } 个里将转换 { $count } 个
convert-dialog-window-title = rox - 转换
convert-ffmpeg-silent-failure = ffmpeg 失败了，也没说为什么
convert-flag-attach = -attach 要读它自己的文件，这里不允许
convert-flag-f = 容器由扩展名决定，所以 -f 不归你设
convert-flag-i = 输入就是你选的曲目，所以 -i 不归你设
convert-flag-n = 每次运行都已经带上 -n
convert-flag-y = 这里什么都不会覆盖，所以没有 -y；目标已经存在就跳过
convert-preset-flac = FLAC
convert-preset-mp3-320 = MP3 320 kbps
convert-preset-mp3-v0 = MP3 V0
convert-preset-opus-192 = Opus 192 kbps
convert-preset-wav = WAV
convert-skip-duplicate = 两首曲目映射到同一个名字
convert-skip-exists = 已经在那里了
convert-summary-failed = ，{ $count } 个失败
convert-summary-files = { $count ->
   *[other] { $count } 个文件
}
convert-summary-line = { $files } 到 { $dest }
convert-summary-skipped = ，跳过 { $count } 个
convert-summary-stopped = 在 { $files } 到 { $dest } 之后停下
convert-version-answered = { $binary } 能运行，但没报告版本号

## Duplicates
duplicates-auto-select = 自动选择
duplicates-check-to-trash = 勾选副本把它们扔进回收站
duplicates-copy-count = { $count ->
   *[other] { $count } 份
}
duplicates-different-albums = 不同专辑
duplicates-filter-placeholder = 按标题、艺术家或文件夹筛选
duplicates-groups-summary = { $groups ->
   *[other] { $groups } 组，多出 { $extras } 份
}
duplicates-library-loading = 媒体库还在加载；稍后再试。
duplicates-no-duplicates = 没找到重复项。
duplicates-no-filter-matches = 没有分组匹配这个筛选。
duplicates-policy-newest = 保留最新
duplicates-policy-oldest = 保留最旧
duplicates-policy-quality = 保留音质最好的
duplicates-scan-hint = 扫描媒体库，找出出现不止一次的曲目。
duplicates-select-none = 全不选
duplicates-selected-count = 已选 { $count } 项
duplicates-trash-button = { $count ->
    [0] 移入回收站
   *[other] 移入回收站（{ $count }）
}
duplicates-trash-error = { $name }：{ $error }
duplicates-trash-result = { $count ->
   *[other] 已把 { $count } 个文件移入回收站
}
duplicates-trash-result-failed = 已把 { $count } 个移入回收站，{ $failed } 个失败
duplicates-trashing = 正在移入回收站 { $done }/{ $total }…
duplicates-window-title = rox - 重复曲目

## Smart playlists
smart-playlist-descending = 降序
smart-playlist-edit-title = 编辑智能播放列表
smart-playlist-limit-label = 上限
smart-playlist-limit-placeholder = 不限
smart-playlist-match-count = { $count ->
   *[other] { $count } 首曲目匹配
}
smart-playlist-matched-tracks = 匹配的曲目
smart-playlist-new-title = 新建智能播放列表
smart-playlist-no-matches = 没有匹配的曲目
smart-playlist-query-label = 查询
smart-playlist-sort-default = 默认顺序
smart-playlist-sort-added = 添加日期
smart-playlist-sort-label = 排序
smart-playlist-unknown-field = “{ $field }:”不是字段，所以这一项按纯文本匹配
smart-playlist-window-title = rox - { $verb }

## Playlist creation
playlist-create-not-savable = 给播放列表起个名字才能保存
playlist-create-placeholder = 播放列表名称
playlist-create-rename-title = 重命名播放列表
playlist-create-title = 新建播放列表
playlist-create-window-title = rox - { $verb }

## Cover tools
cover-art-back = 封底
cover-art-disc = 碟面
cover-art-front = 封面
cover-artwork = 图片
    .description = 显示哪张图；文件里没有的槽位会回退到正面封面
cover-disc-style = 碟片样式
    .description = 把图片做成 CD 或黑胶唱片标签的样式
cover-disc-off = 关
cover-disc-cd = CD
cover-disc-vinyl = 黑胶唱片
cover-editor-choose-image = 选择图片
cover-editor-multiple = 多个
cover-editor-none = 无
cover-editor-not-an-image = 那个文件不是 rox 能嵌入的图片
cover-editor-not-decoded = 那张图片解码不了
cover-editor-reading = 正在读取当前图片…
cover-editor-remove = 移除
cover-editor-replace = 替换
cover-editor-revert = 还原
cover-editor-save-errors = { $count } 个文件失败；{ $error }
cover-editor-saving-progress = 正在保存 { $done }/{ $total }…
cover-editor-search-online = 在线搜索
cover-editor-section = 封面
cover-editor-slot-back = 封底
cover-editor-slot-front = 正面封面
cover-editor-slot-media = 碟面
cover-editor-will-remove = 将移除
cover-editor-window-title = rox - 封面
cover-matcher-blocked-fetching = 正在获取完整图片…
cover-matcher-blocked-no-cover = 没有可设置的封面
cover-matcher-blocked-pick = 选一张封面来设置
cover-matcher-cover-count = { $count ->
   *[other] { $count } 张封面
}
cover-matcher-editor-closed = 封面编辑器已关闭
cover-matcher-no-covers = 没找到封面
cover-matcher-search-failed = 搜索失败：{ $error }
cover-matcher-set-cover = 设为封面
cover-matcher-setting = 正在设置…
cover-matcher-tile-info = { $provider }  { $width }px
cover-matcher-unsupported-format = 不支持的图片格式
cover-matcher-window-title = rox - 查找封面
cover-spin = 旋转
    .description = 播放时让碟片转起来；对碟面槽位或碟片样式生效
cover-spin-disc = 旋转碟片
cover-spin-ramp = 旋转加速
    .description = 碟片加到全速要多久，滑回停下又要多久
cover-spin-speed = 旋转速度
    .description = 全速转速，每分钟多少圈
cover-stretch = 拉伸
    .description = 填满面板，不管图片的宽高比
cover-stretch-to-fill = 拉伸填满
cover-title = 封面

## Lyrics
lyrics-always-centered = 始终居中
    .description = 两端补空，第一行和最后一行也能居中
lyrics-auto-search = 自动搜索
    .description = 曲目没有歌词时在线搜索，把握大的直接保存，不弹选择器
lyrics-bold = 粗体
lyrics-build-word-by-word = 逐词显现
    .description = 唱到哪个词就显示哪个词，卡拉 OK 式；没唱到的行保持隐藏
lyrics-edge-bottom = 底部
lyrics-edge-top = 顶部
lyrics-edit-hint-after-stamp = 打时间戳
lyrics-edit-hint-or = 或
lyrics-edit-loading = 正在加载歌词…
lyrics-edit-lyrics = 编辑歌词
lyrics-edit-saving = 正在保存…
lyrics-edit-section = 歌词
lyrics-edit-stamp = 打时间戳
lyrics-edit-stamp-time = 打上 { $time }
lyrics-edit-window-title = rox - 编辑歌词
lyrics-fade-lines-in = 淡入行
    .description = 某一行成为当前行时，从暗淡里淡上来
lyrics-falloff-edge = 衰减方向
    .description = 当前行的哪一侧被衰减压暗
lyrics-find-online = 在线查找歌词…
lyrics-follow-playback = 跟随播放
    .description = 同步歌词播放时，把当前行滑到中间
lyrics-font = 字体
    .description = 歌词字体；默认跟随应用字体
lyrics-gap-threshold = 间隙阈值
    .description = 前奏或间奏要多长才给它一个休止
lyrics-lead-in-rest = 前奏休止
    .description = 长前奏之前显示一个空白休止，第一行到来时正好淡入
lyrics-line-falloff = 逐行衰减
    .description = 离当前行每远一行暗多少
lyrics-line-spacing = 行间距
    .description = 同步歌词各行隔多远，按文字大小的倍数算
lyrics-mark-dots = 圆点
lyrics-mark-note = 音符
lyrics-matcher-blocked-no-match = 没有可应用的匹配
lyrics-matcher-blocked-pick = 选一个匹配来应用
lyrics-matcher-blocked-saving = 正在保存歌词…
lyrics-matcher-match-count = { $count ->
   *[other] { $count } 个匹配
}
lyrics-matcher-no-query = 这首曲目没有艺术家和标题，没法拿去匹配
lyrics-matcher-pick-preview = 选一个匹配来预览
lyrics-matcher-search-failed = 搜索失败：{ $error }
lyrics-matcher-synced-tag = { $provider }  已同步
lyrics-matcher-window-title = rox - 查找歌词
lyrics-no-lyrics-notice = 没有歌词
lyrics-no-lyrics-track = 这首曲目没有歌词
lyrics-rest-in-gaps = 间奏休止
    .description = 长的器乐间奏里切到空白休止，而不是一直停在上一行
lyrics-rest-marker = 休止标记
    .description = 同步歌词里没有词的那一行显示什么，也就是间隙和空行
lyrics-search-button = 在线搜索按钮
    .description = 在空白面上显示搜索按钮；右键菜单照样能找歌词
lyrics-search-online = 在线搜索
lyrics-show-song-name = 显示歌名
    .description = 在空白面上显示曲目名，压在“没有歌词”那一行上方
lyrics-text-size = 文字大小
    .description = 歌词文字的大小；同步歌词的行高随它变化
lyrics-title = 歌词
lyrics-title-unsynced = 非同步时显示标题
    .description = 把曲目标题钉在非同步歌词上方，面板矮的时候也看得到
lyrics-wipe-lyrics = 清除歌词

## Analysis passes
pass-acoustic-body = { $model } 会算出每一首听起来是什么样，媒体库就能找到和正在播放的相像的音乐。全部在这台机器上跑，已经描述过的会跳过。{ $lands }
pass-acoustic-lands-database = 结果进媒体库数据库，你的文件不动。
pass-acoustic-lands-tags = 结果保存到媒体库数据库；MP3 和 FLAC 还会写进各自文件的标签，数据库重建后它们也还在。其他格式只保留数据库那一份。
pass-acoustic-title = { $count ->
   *[other] 分析 { $count } 首曲目？
}
pass-analyze = 分析
pass-estimate-at = { $estimate }，用 { $workers_phrase }。
pass-estimate-button = 估算
pass-estimating = 正在估算…
pass-measure = 测量
pass-no-estimate = 这台机器上还没跑过，所以没有估算值。“估算”会先跑几首，再据此推出剩下的。
pass-replaygain-body = 每个文件都会解码并计量，好按它母带的响度播放。整张专辑的曲目全都缺增益时，会按整张来测。{ $lands }
pass-replaygain-lands-database = 数字进媒体库数据库，你的文件不动。
pass-replaygain-lands-tags = 数字写回每个文件的标签里，其他播放器都从那里读。
pass-replaygain-title = { $count ->
   *[other] 测量 { $count } 首曲目？
}
pass-tempo-body = 每个文件解码两段半分钟的窗口并数拍子，媒体库就能显示曲目跑多快。它在跟点录制的音乐上最准，测不出来的会跳过。数字进媒体库数据库，你的文件不动。
pass-tempo-title = { $count ->
   *[other] 找出 { $count } 首曲目的速度？
}
pass-timing = 正在给几首曲目测速…
pass-timing-failed = 这个媒体库测速失败：{ $error }
pass-workers = 并发数

## Quick play
quick-play-comfortable-rows = 宽松行距
    .description = 给每条结果多一点高度
quick-play-cover = 封面
    .description = 在每条结果左边显示封面缩略图
quick-play-duration = 时长
    .description = 在右边显示每条结果的长度
quick-play-narrow-by = 缩小范围
quick-play-search-placeholder = 搜索媒体库
quick-play-subtitle = 副标题
    .description = 在每条结果下面显示艺术家和专辑
quick-play-tag-album = 专辑
quick-play-tag-artist = 艺术家

## Drawer panel
drawer-add-tooltip = 添加抽屉面板
drawer-answers = 响应对象
    .description = 哪些选中会打开抽屉：只有它自己的主面板，还是它之外的任意面板
drawer-dim = 变暗
    .description = 抽屉打开时主面板在后面暗到什么程度
drawer-edge = 边缘
    .description = 抽屉靠着哪一边，又从那里滑出来
drawer-edge-bottom = 底部
drawer-edge-top = 顶部
drawer-handle = 把手
    .description = 显示面板边缘的把手。隐藏后在选中之前抽屉什么都不露；之后只要选中还在，把手就一直留着，收起来的抽屉还能再拉出来
drawer-open-on = 打开方式
    .description = 把指针停在把手上总能打开抽屉；“选中”再加上主面板里的一次选中
drawer-pin-open = 钉住不收
drawer-reveal = 露出
    .description = 打开的抽屉盖住面板的多少
drawer-scope-elsewhere = 别处
drawer-scope-main = 主面板
drawer-title = 抽屉
drawer-trigger-hover = 悬停
drawer-trigger-selection = 选中

## Mini player
mini-tip-back = 回到完整布局
mini-tip-none = 没有指定迷你布局
mini-tip-shrink = 收成迷你播放器
mini-title = 迷你切换

## System tray
tray-open = 打开
tray-pause = 暂停
tray-play = 播放
tray-quit = 退出

## Window controls
window-controls-mini-toggle = 迷你切换
    .description = 把迷你布局切换放在最前面；指定了迷你布局后才显示
window-controls-minimize = 最小化
window-controls-style = 样式
    .description = 扁平图标，或者 macOS 的红黄绿圆点
window-controls-style-icons = 图标
window-controls-title = 窗口按钮
window-controls-traffic-lights = 红黄绿圆点

## Section names the audio panels share. Each one heads the same kind of
## rows in the spectrum, VU, oscilloscope, spectrogram and particles
## settings, so they're defined once rather than per panel.
viz-section-analysis = 分析
viz-section-color = 颜色
viz-section-peaks = 峰值
viz-section-playback = 播放
viz-section-scale = 刻度
viz-section-signal = 信号

## Particles panel
particles-add-emitter = 添加发射器
particles-aim = 朝向
particles-aim-fixed = 固定
particles-aim-outward = 向外
particles-burst = 爆发
particles-color = 颜色
particles-cone = 锥角
particles-direction = 方向
    .description = 往哪边拉；0 是上，180 是下
particles-drag = 阻力
    .description = 空气每秒吃掉多少速度；为零就是真空
particles-drift = 漂移
    .description = 场本身移动得多快，让漩涡不会杵着不动
particles-edit-emitters = 编辑发射器
particles-emitter-label = 发射器 { $index }
particles-emitter-target = 发射器 { $index } { $target }
particles-emitters-empty = 还没有发射器。加一个就能启动这个场。
particles-glow = 辉光
    .description = 在每个粒子后面铺一圈柔光
particles-gravity = 重力
particles-gravity-strength = 强度
    .description = 对所有飞行中粒子的恒定拉力
particles-height = 高度
particles-hold-on-pause = 暂停时保持
    .description = 暂停时冻住这个场，而不是让它飘散
particles-length = 长度
particles-lifetime = 存活时间
particles-position-x = 位置 X
particles-position-y = 位置 Y
particles-radius = 半径
particles-rate = 速率
particles-rotation = 旋转
particles-round-particles = 圆形粒子
    .description = 画圆点而不是方块
particles-scale = 尺度
    .description = 一个漩涡有多宽；小的翻搅，大的滚动
particles-section-emitters = 发射器
particles-section-medium = 介质
particles-section-particles = 粒子
particles-shape = 形状
particles-shape-box = 矩形
particles-shape-line = 线
particles-shape-point = 点
particles-shape-ring = 环
particles-size = 大小
particles-speed = 速度
particles-trigger = 触发
particles-trigger-continuous = 持续
particles-turbulence = 湍流
particles-turbulence-drift = 湍流漂移
particles-turbulence-scale = 湍流尺度
particles-turbulence-strength = 强度
    .description = 场把粒子推得多用力；为零就是关掉
particles-width = 宽度

## Spectrum panel
spectrum-axis-labels = 坐标标签
    .description = 在面板上标出范围：倍频程（C1、C2……）或频率（100、1k、10k）
spectrum-bar-gap = 柱间距
    .description = 柱子之间的空隙，间距越宽装下的柱子越少
spectrum-bar-width = 柱宽
    .description = 每根柱子画多粗，越细装下的频段越多
spectrum-block-gap = 块间距
    .description = 一叠里各格之间的接缝
spectrum-block-height = 块高
    .description = 一叠里每一格画多高
spectrum-cap-gravity = 峰帽重力
    .description = 频段落下后峰值标记掉得多快
spectrum-fft-size = FFT 大小
    .description = 分析窗口；短的反应快，长的分辨细
spectrum-gradient-base-color = 基础色
    .description = 自定义渐变里安静的那一端
spectrum-gradient-cover = 封面
spectrum-gradient-mode = 渐变
    .description = 按响度给频段上色：主题的渐变、歌曲配色下封面的颜色，或者一对自定义颜色
spectrum-gradient-theme = 主题
spectrum-gradient-tip-color = 顶端色
    .description = 自定义渐变里响亮的那一端
spectrum-high-bound-description = 柱子分析的最高频率
spectrum-high-fft-size = 高频 FFT 大小
    .description = 分割点以上频段的分析窗口
spectrum-hold-on-pause = 暂停时保持
    .description = 暂停时冻住柱子，而不是让它们落到静默
spectrum-labels-frequency = 频率
spectrum-labels-pitch = 音高
spectrum-low-bound-description = 柱子分析的最低频率
spectrum-orientation = 方向
    .description = 频段从哪条边长出来
spectrum-outline-bars = 描边柱
    .description = 把每根柱子画成空心描边，而不是填充渐变
spectrum-outline-width = 描边宽度
    .description = 空心柱的线条粗细
spectrum-peak-caps = 峰值帽
    .description = 在每个频段最近的峰值上留一个标记
spectrum-section-bands = 频带
spectrum-split-at = 分割点
    .description = 两个区在哪里交界，会对齐到最近的一根柱子
spectrum-split-zones = 分区分析
    .description = 分割频率上下两段用不同的窗口大小分析
spectrum-style = 样式
    .description = 经典柱状、LED 式方块，或者一条实线
spectrum-style-bars = 柱状
spectrum-style-blocks = 方块
spectrum-style-line = 线条
spectrum-symmetry = 对称
    .description = 把频谱绕中心对折；正向把低频放在两侧，反向让它们在中间会合
spectrum-symmetry-forward = 正向
spectrum-symmetry-reverse = 反向

## Waveform panel
waveform-bar-gap = 柱间距
    .description = 柱子之间的空隙，为零就并成一整块
waveform-bar-width = 柱宽
    .description = 每根柱子画多粗
waveform-outline = 描边
    .description = 只描柱子的轮廓而不填充；并在一起的柱子读作一个形状
waveform-scrobble-marker = Scrobble 标记
    .description = 一条细线，标出曲目算作已 scrobble 到 Last.fm 的位置
waveform-split-channels = 分离声道
    .description = 一个声道一行，左上右下；单声道曲目仍是一行
waveform-unavailable = 这首曲目没有波形

## VU panel
vu-ballistics = 表针特性
    .description = VU 慢慢积分响度；峰值猛冲上去、缓缓落下
vu-ballistics-peak = 峰值
vu-cap-gravity = 峰帽重力
    .description = 表针落下后峰值标记掉得多快
vu-channels = 声道
    .description = 分开立体声两路，或者并成一个表
vu-channels-mono = 单声道
vu-channels-stereo = 立体声
vu-db-scale = dB 刻度
    .description = 在表后面按 dB 刻度画带标签的网格线
vu-gradient-mode = 渐变
    .description = 按电平给表上色：主题的渐变、歌曲配色下封面的颜色，或者一对自定义颜色
vu-hold-on-pause = 暂停时保持
    .description = 暂停时冻住表，而不是让它们落到静默
vu-orientation = 方向
    .description = 表从哪条边长出来
vu-peak-caps = 峰值帽
    .description = 在每个表最近的峰值上留一个标记
vu-section-meter = 仪表
vu-segment-gap = 段间距
    .description = 一叠里各格之间的接缝
vu-segment-height = 段高
    .description = 一叠里每一格画多高
vu-style = 样式
    .description = 一整条实心柱，或者 LED 式分段
vu-style-continuous = 连续
vu-style-segments = 分段

## Spectrogram panel
spectrogram-ceiling = 上限
    .description = 映射到颜色映射亮端的电平，比这更响的声音都会顶在这里
spectrogram-colormap = 颜色映射
    .description = 响度怎么映射成颜色
spectrogram-colormap-cover = 封面
spectrogram-colormap-grayscale = 灰度
spectrogram-colormap-ice = 冰
spectrogram-colormap-magma = Magma
spectrogram-colormap-theme = 主题
spectrogram-colormap-viridis = Viridis
spectrogram-direction = 流向
    .description = 新的一列从哪条边进入，这也决定了频率轴是沿面板往上走，还是横着走
spectrogram-fft-size = FFT 大小
    .description = 分析用的窗口大小，在一列能多快跟上瞬态，和它能把两个低音分得多开之间取舍
spectrogram-floor = 下限
    .description = 映射到颜色映射暗端的电平，比这更轻的声音都会读作背景
spectrogram-grid = 网格
    .description = 画在图像上的频率分隔线
spectrogram-high-bound = 高端
    .description = 频率轴的顶端，限制在奈奎斯特频率以下，去掉几乎无声的最高几个倍频程
spectrogram-history = 历史
    .description = 最旧的一列滚出画面之前，面板保留多少列
spectrogram-hold-on-pause = 暂停时保持
    .description = 暂停时保持静止的画面，而不是让静音滚进去
spectrogram-labels = 标签
    .description = 沿着标尺显示的频率数字，画在面板留得出空间的地方
spectrogram-log-scale = 对数刻度
    .description = 给每个倍频程同样的空间，这是音乐化的读法，而不是像实验室仪器那样按 Hz 均匀分布
spectrogram-low-bound = 低端
    .description = 频率轴的底端
spectrogram-section-picture = 图像
spectrogram-speed = 速度
    .description = 画面滚动的快慢，以每秒多少列计

## Oscilloscope panel

oscilloscope-channels = 声道
    .description = 合并成一条轨迹，两条叠在一起，或者各自堆叠出一个框
oscilloscope-channels-mono = 单声道
oscilloscope-channels-overlay = 叠加
oscilloscope-channels-split = 分离
oscilloscope-fill = 填充
    .description = 轨迹和中心线之间的柔和填充
oscilloscope-gain = 增益
    .description = 垂直比例，把安静的曲目拉到能看清的轨迹
oscilloscope-gradient-mode = 渐变
    .description = 按摆幅给轨迹上色：主题的渐变、歌曲配色下封面的颜色，或者一对自定义颜色
oscilloscope-grid = 网格
    .description = 在轨迹后面画出网格线
oscilloscope-hold-on-pause = 暂停时保持
    .description = 暂停时保持静止的一帧，而不是让轨迹变成一条平线
oscilloscope-line-width = 线宽
    .description = 轨迹画得多粗
oscilloscope-persistence = 余辉
    .description = 之前的帧在轨迹后面残留多久，也就是荧光屏余辉的效果
oscilloscope-section-trace = 波形
oscilloscope-trigger = 触发
    .description = 让每一帧都从信号穿过触发电平的地方开始，这样周期性的内容就能稳定不动
oscilloscope-trigger-falling = 下降
oscilloscope-trigger-level = 触发电平
    .description = 寻找信号穿越的电平
oscilloscope-trigger-off = 关
oscilloscope-trigger-rising = 上升
oscilloscope-window = 窗口
    .description = 轨迹在面板上跨越的时长

## Shader panel
shader-panel-compile-error = 这个着色器没编译过：
shader-panel-compile-title = 这个着色器没编译过
shader-panel-enable = 启用
shader-panel-inspect = 查看
shader-panel-note-empty-body = 选一个示例，或者给面板指一个定义了 fs_user(uv) 的 .wgsl 文件。
shader-panel-note-empty-title = 没有加载着色器。
shader-panel-note-missing-body = 这个面板引用了工作区里没有的着色器，所以没什么可跑的。
shader-panel-note-missing-title = { $name } 不在这个工作区的着色器里。
shader-panel-note-off-body = 源码和它的绑定都还在，只是没在运行。
shader-panel-note-off-title = 这个着色器已关闭。
shader-panel-note-pending-body = 它是随布局或工作区来的，不是这台机器上的，所以在你查看确认之前保持关闭。
shader-panel-note-pending-title = 这个着色器还没被读过。
shader-pending-origin-file = 据称来自 { $path }
shader-pending-origin-inline = 背后没有文件；源码是随布局来的
shader-pending-more-lines = …还有 { $count } 行
shader-eject-name-taken = { $name } 在这个工作区的着色器里已经有 { $count } 个编号副本
shader-eject-not-in-pool = { $name } 不在这个工作区的着色器里
shader-eject-failed = 导出：{ $error }
shader-panel-pick = 选一个着色器
shader-panel-run-shader = 运行着色器
    .description = 关掉会保留源码、书签和绑定，什么都不画
shader-panel-section-routes = 路由

## Genre grid panel
genre-grid-clear-picked = 清除已选流派
genre-grid-desaturate = 播放时去色
    .description = 让除正在播放的流派外每个图块都变灰；悬停能让某块恢复颜色
genre-grid-dim-while-playing = 播放时变暗
    .description = 让除正在播放的流派外每个图块都淡下去；悬停能把某块重新点亮
genre-grid-follow-description = 每次换曲都滚动到正在播放的流派
genre-grid-merge-many = 把 { $count } 个流派合并进“{ $target }”
genre-grid-merge-one = 把“{ $source }”合并进“{ $target }”
genre-grid-pick-filters = 选中即筛选媒体库
    .description = 点一个流派会把跟随共享搜索的每个面板都收窄到它；关掉则点击只是普通选中
genre-grid-play-genres = 播放 { $count } 个流派
genre-grid-resume-description = 停止浏览后滑回正在播放的流派
genre-grid-show-names = 显示名称
    .description = 把流派印在每个图块下面，而不是只在悬停时显示
genre-grid-smooth-description = 滑到那个流派，而不是直接跳
genre-grid-tally = { $albums ->
   *[other] { $albums } 张专辑，{ $tracks } 首曲目
}
genre-grid-tile-face = 图块外观
    .description = 图块显示什么：这个流派的专辑封面、按流派自身颜色染过的封面，或者一张写着名字的纯色卡片
genre-grid-unmerge = { $count ->
   *[other] 取消合并 { $count } 个值
}

## Artist grid panel
artist-grid-clear-picked = 清除已选艺术家
artist-grid-desaturate = 播放时去色
    .description = 让除正在播放的艺术家外每个图块都变灰；悬停能让某块恢复颜色
artist-grid-dim-while-playing = 播放时变暗
    .description = 让除正在播放的艺术家外每个图块都淡下去；悬停能把某块重新点亮
artist-grid-follow-description = 每次换曲都滚动到正在播放的艺术家
artist-grid-group-mode = 每个图块代表
    .description = 用署名的专辑艺术家，唱片上的客串就归到发行它的那一位名下；用曲目艺术家，每个客串都会拆成自己的图块
artist-grid-pick-filters = 选中即筛选媒体库
    .description = 点一位艺术家会把跟随共享搜索的每个面板都收窄到这位艺术家；关掉则点击只是普通选中
artist-grid-play-artists = 播放 { $count } 位艺术家
artist-grid-portraits = 艺术家肖像
    .description = 显示每位艺术家自己的照片，按名字查一次并存在本地；关掉则显示第一张专辑的封面
artist-grid-resume-description = 停止浏览后滑回正在播放的艺术家
artist-grid-section-grouping = 分组
artist-grid-show-names = 显示名称
    .description = 把艺术家印在每个图块下面，而不是只在悬停时显示
artist-grid-smooth-description = 滑到那位艺术家，而不是直接跳
artist-grid-tally = { $albums ->
   *[other] { $albums } 张专辑，{ $tracks } 首曲目
}
artist-grid-track-artist = 曲目艺术家

## Wall panels
wall-dim-always = 始终
    .description = 什么都没播时也让图块退到后面；只有悬停的那块完整显示
wall-dim-amount = 变暗程度
    .description = 其他图块淡到什么程度；100% 就是藏起来
wall-gap = 间距
    .description = 图块之间的空隙
wall-name-alignment = 名称对齐
    .description = 让说明文字在图块下方对齐
wall-rounding = 圆角
    .description = 把每个图块的角磨圆；100% 是圆形
wall-section-picking = 选中
wall-show-counts = 显示数量
    .description = 每个名称下面的专辑和曲目数
wall-tile-size = 图块大小
    .description = 图块的最长边；各列均分面板宽度

## Metadata panel
metadata-cover-background = 封面背景
    .description = 在字段后面放这首曲目的封面
metadata-display = 显示
    .description = 以标题开头的信息卡，或者从顶部开始的扁平标签值表格
metadata-display-sheet = 信息卡
metadata-display-table = 表格
metadata-edit-save = 保存
metadata-field-bit-depth = 位深
metadata-field-bitrate = 比特率
metadata-field-codec = 编码
metadata-field-comment = 注释
metadata-field-disc = 碟片
metadata-field-file = 文件
metadata-field-sample-rate = 采样率
metadata-field-track = 曲目
metadata-fields = 字段
    .description = 信息卡列出哪些字段；曲目没有的字段保持隐藏
metadata-find-online = 在线查找元数据…
metadata-no-library = 没有媒体库
metadata-row-borders-description = 表格里每一行下方的细线
metadata-source = 来源
    .description = 跟随正在播放或选中的，或者读整个媒体库
metadata-stripes-description = 给表格隔行上色

## History panel
history-column-last-played = 上次播放
history-descending = 降序
    .description = 把排序反过来
history-empty-never = 每首曲目都播过了
history-empty-recent = 还没有收听记录
history-headings = 把最近列表按连续的同一专辑分段；“展开”还会加上封面和统计
history-sort-browse = 浏览顺序
history-sort-date-added = 添加日期
history-sort-menu = 排序
    .description = 从没播过的曲目怎么排
history-title = 播放历史
history-view-most = 播放最多
history-view-never = 从未播放
history-view-recent = 最近播放
history-view-recent-short = 最近
history-view-row = 视图
    .description = 面板显示收听记录的哪一面

## Folder tree panel
folder-tree-clear-scope = 清除文件夹范围
folder-tree-collapse-all = 全部折叠
folder-tree-cover-art = 封面
    .description = 用专辑封面代替行图标，用在文件夹或歌曲上
folder-tree-cover-folders = 文件夹
folder-tree-cover-songs = 歌曲
folder-tree-empty = 媒体库里还没有文件夹
folder-tree-follow-description = 每次换曲都展开并滚动到正在播放的曲目
folder-tree-nonmatch-folders = 不匹配的文件夹
    .description = 把没有匹配的文件夹藏起来，或者让它们保持暗淡
folder-tree-nonmatch-songs = 不匹配的歌曲
    .description = 在匹配的文件夹里，把零散的歌曲压暗或者藏起来
folder-tree-play-folder = 播放文件夹
folder-tree-play-songs = { $count ->
   *[other] 播放 { $count } 首歌曲
}
folder-tree-resume-description = 停止浏览后滚回正在播放的曲目
folder-tree-scope-to-folder = 把筛选限定到文件夹
folder-tree-smooth-description = 滑到那首曲目，而不是直接跳
folder-tree-title = 树

## Art panel
art-always = 什么都没播时也让封面退到后面；只有悬停的那张完整显示
art-convert = 转换…
art-covers-section = 封面
matcher-section-matches = 匹配
art-desaturate = 让除正在播放的专辑外每张封面都变灰；悬停能让某张恢复颜色
art-dim-while-playing = 让除正在播放的专辑外每张封面都淡下去；悬停能把某张重新点亮
art-disc-style = 碟片样式
    .description = 把每张封面都做成 CD 或黑胶唱片标签的样式
art-edit-tags = 编辑标签…
art-fill-panel = 填满面板
    .description = 只按面板高度定居中封面的大小（纵向时按宽度）；两侧的封面跑出边缘，而不是把它挤小
art-follow-description = 每次换曲都把正在播放的专辑居中
art-glow = 辉光
    .description = 在居中封面后面铺一层强调色；开着封面染色时它会取正在播放专辑的颜色
art-layout-section = 布局
art-perspective = 透视
    .description = 用真正的 3D 转动两侧的封面，而不是扁平压缩
art-reflections = 倒影
    .description = 把每张封面映到架子下面的地板上
art-resume-description = 停止浏览后重新把正在播放的专辑居中
art-shadows = 阴影
    .description = 每张封面下面一层柔和的影子
art-smooth-description = 滑到那张专辑，而不是直接跳
art-title = 专辑转盘
art-vertical-layout = 纵向布局
    .description = 把架子叠成一列上下滚动，而不是排成一行

## Playlists panel
playlists-columns = 标题旁边显示哪些曲目列
playlists-delete = 删除播放列表
playlists-edit-query = 编辑查询…
playlists-empty = 还没有播放列表，加些曲目或者用“新建播放列表”
playlists-headings = 把每个播放列表的曲目按连续的同一专辑分段；“展开”还会加上封面和统计
playlists-import-tooltip = 导入播放列表
playlists-imported-fallback = 已导入
playlists-new = 新建播放列表…
playlists-new-smart = 新建智能播放列表…
playlists-refuse-drag-out = 智能播放列表里的曲目拖不出来
playlists-refuse-edit-query = 编辑查询才能改变智能播放列表的内容
playlists-refuse-smart-source = 智能播放列表的曲目来自它的查询
playlists-remove = { $count ->
   *[other] 从播放列表移除 { $count } 首
}
playlists-rename = 重命名…
playlists-title = 播放列表

## Queue panel
queue-clear = 清空队列
queue-empty = 队列是空的
queue-headings = 把队列按连续的同一专辑分段；“展开”还会加上封面和统计
queue-play-now = 立即播放
queue-remove = { $count ->
   *[other] 从队列移除 { $count } 首
}
queue-title = 队列
queue-widget-always-modal = 总是以浮层打开
    .description = 每次都在浮层里打开队列，而不是跳到已经开着的队列面板
queue-widget-clear-queue = 清空队列
queue-widget-more = +{ $count } 首
queue-widget-open-on-click = 点击打开队列
    .description = 点这个部件跳到开着的队列面板；一个都没有就在窗口里打开队列
queue-widget-section-click = 点击
queue-widget-title = 队列部件
queue-widget-up-next = 接下来

## Biography panel
biography-background = 背景
    .description = 文字后面的艺术家画面，压暗并向底部淡出
biography-fill-width = 填满宽度
    .description = 让高的头图铺满整个宽度，而不是限宽居中
biography-from-lastfm = 来自 Last.fm
biography-header-image = 头图
    .description = 顶部横跨的宽幅艺术家横幅；没有横幅时用肖像
biography-keep-aspect = 保持宽高比
    .description = 按头图自身的比例显示，而不是裁成一条
biography-listeners-count = { $count } 位听众
biography-looking-up = 正在查找 { $name }
biography-no-artist-tag = 没有艺术家标签
biography-no-text = 没有存下的简介
biography-not-found = 没找到关于 { $name } 的内容
biography-plays-count = { $count } 次播放
biography-refresh = 刷新
biography-similar-artists = 相似艺术家
    .description = 按收听数据得出的相关艺术家，放在最底下
biography-similar-heading = 相似艺术家
biography-stats = 统计
    .description = Last.fm 上的听众数和播放数，放在名字下面
biography-tags = 标签
    .description = 流派标签排成一行小标签
biography-title = 简介

## Status panel
status-count-albums = { $count ->
   *[other] { $count } 张专辑
}
status-count-artists = { $count ->
   *[other] { $count } 位艺术家
}
status-count-plays = { $count ->
   *[other] { $count } 次播放
}
status-count-selected = 已选 { $count } 项
status-count-tracks = { $count ->
   *[other] { $count } 首曲目
}
status-readouts = 读数
    .description = 沿着这条栏拖动可以重排；在两行之间拖动，或者用小标签上的 x 和 +，来隐藏和显示
status-scope-selection = 选中
status-title = 状态

## Output panel
output-detail-badge = 角标
output-detail-compact = 紧凑
output-detail-expanded = 展开
output-detail-label = 详略
    .description = 角标只留一个小标签，其余悬停时显示；紧凑给标题行单独一行，适合贴边的窄条；展开在旁边加上原因，面板太窄时放到下面
output-device-name = 设备名称
    .description = 在标题行里写出正在用的设备；关掉则只留模式、采样率和格式
output-file-rate = 文件采样率
    .description = 没有转换时也标出播放文件自身的采样率。有转换时无论如何都会提示，因为警告说的就是这件事
output-mode-exclusive = 独占
output-mode-shared = 共享
output-no-output = 没有输出
output-nothing-playing = 没在播放
output-pick-another-device = 换个设备，或者关掉独占
output-headline-numbers = { $rate } Hz，{ $channels } 声道，{ $format }
output-headline = { $mode }，{ output-headline-numbers }
output-headline-device = { $mode }于 { $device }，{ output-headline-numbers }
output-fell-back-to-shared = 独占回退到了共享：{ $why }
output-replaygain-levelling = ReplayGain 正在把这个文件的电平调平 { $db } dB
output-replaygain-short = ReplayGain { $db } dB
output-rate-resampled = 正在播放的文件是 { $rate } Hz，为了适配这台设备已被重新采样
output-rate-resampled-short = { $rate } Hz 文件已重新采样
output-rate-native = 正在播放的文件是 { $rate } Hz，所以不用重新采样
output-rate-native-short = { $rate } Hz 文件，未重新采样
output-start-track-hint = 播一首曲目就能看到设备接受的格式
output-title = 输出

## Track columns
columns-bits = 位深
columns-bpm = BPM
columns-codec = 编码
columns-cover = 封面
columns-fav = 收藏
columns-gain = 增益
columns-kbps = Kbps
columns-khz = kHz
columns-name = 名称
columns-number = 序号
columns-scanned = 扫描时间
columns-similar = 相似度

## Filter panel
filter-add-column = 添加列
filter-add-column-tooltip = 添加列
filter-all = 全部
filter-clear-filters = 清除筛选
filter-clear-selection = 清除选中
filter-empty = 选一个字段开始筛选
filter-remove-column = 移除列

## Search panel
search-chips-below = 下方
search-chips-inline = 同行
search-filter-chips = 筛选标签
search-placeholder = 搜索媒体库

## Playback panel
playback-buttons = 按钮
    .description = 沿着这条栏拖动可以重排；在两行之间拖动，或者用小标签上的 x 和 +，来隐藏和显示
playback-continue-down-list = 继续播放，顺着列表往下
playback-continue-off = 继续播放已关
playback-continue-weighted = 继续播放，没播过的优先
playback-crossfade-inside-albums = 专辑内
playback-crossfade-off = 交叉淡化已关
playback-crossfade-tip = 交叉淡化 { $length }
playback-highlight-circle = 圆形
playback-highlight-square = 方形
playback-hold-draw = { $tip }。按住可以选抽取方式
playback-hold-length = { $tip }。按住可以选时长
playback-hold-order = { $tip }。按住可以选顺序
playback-loop-off = 循环已关
playback-loop-queue = 循环整个队列
playback-loop-track = 循环这首曲目
playback-menu-continue = 续播按钮
playback-menu-crossfade = 交叉淡化按钮
playback-menu-favourite = 收藏按钮
playback-menu-random = 随机按钮
playback-menu-rating = 评分星星
playback-menu-stop = 停止按钮
playback-menu-stop-after = 播完即停按钮
playback-menu-volume = 音量按钮
playback-pause = 暂停
playback-play-highlight = 播放键高亮
    .description = 播放按钮的强调填充：圆形、柔和方形，或者不要
playback-random-tip-random = 随机播一首
playback-random-tip-similar = 播一首和这首像的
playback-seek-back-tip = 后退 10 秒
playback-seek-forward-tip = 前进 10 秒
playback-shuffle-off = 随机已关
playback-shuffle-on = 随机已开，{ $order }顺序
playback-stop-after-armed = 播完这首就停，已就绪
playback-stop-after-tip = 播完这首就停
playback-stop-tip = 停止并卸载曲目
playback-volume-tip-muted = 取消静音，{ $percent }%。右键出滑块
playback-volume-tip-unmuted = 静音，{ $percent }%。右键出滑块

## Track info panel
track-info-color-output-chip = 给输出标签上色
    .description = 输出回退或重采样时，让这个小标签变成警告色。关掉则始终是同样的柔和色，悬停说明照样解释状态
track-info-cycle-every = 每隔
    .description = 每一行停多久才淡出
track-info-cycle-rows = 轮播行
    .description = 把排布里的各行在一行里依次显示，彼此之间淡入淡出；只有一行时就是它自己
track-info-delay = 延迟
    .description = 这行文字在两端各停多久才继续滚动
track-info-marquee = 跑马灯
    .description = 一行文字超出面板宽度时怎么办：滚过去再回来，还是无尽循环
track-info-menu-overflow = 溢出
track-info-next = 下一首：{ $line }
track-info-opening = 正在打开…
track-info-output-fallback = 设备拒绝了独占输出，所以播放走的是共享混音器。设备报告：{ $reason }
track-info-output-resample-exclusive = 这个文件是 { $source } kHz，声卡接的是 { $device } kHz，所以每一个采样都在出去的路上被转换。设备跑不了文件自身的采样率。
track-info-output-resample-mixer = 这个文件是 { $source } kHz，混音器跑在 { $device } kHz，所以每一个采样都在出去的路上被转换。独占模式会直接把文件自身的采样率交给声卡。
track-info-overflow-loop = 循环
track-info-overflow-scroll = 滚动
track-info-overflow-truncate = 截断
track-info-queued-count = { $count } 首在队列
track-info-row-size = 第 { $number } 行大小
track-info-speed = 速度
    .description = 这行文字滚动得多快
track-info-text-size = 文字大小

## Seek panel
seek-ending = 结尾
    .description = 倒数剩余时间，或者显示总长度
seek-ending-remaining = 剩余
seek-ending-total = 总长
seek-playhead = 播放头
    .description = 铺满进度条的整个高度，或者只贴着那条线
seek-playhead-full = 整条
seek-playhead-line = 线
seek-playhead-max-height = 播放头最大高度
    .description = 给整条播放头封顶，以线为中心；0 就填满面板
seek-playhead-width = 播放头宽度
    .description = 移动的位置标记有多宽
seek-rounding = 圆角
    .description = 这条线的圆角半径，最多到半个粗细，成为胶囊
seek-scrobble-marker = Scrobble 标记
    .description = 一条细线，标出曲目算作已 scrobble 到 Last.fm 的位置
seek-show-timings = 显示时间
seek-thickness = 粗细
    .description = 曲目进度线的高度

## Volume panel
volume-pieces = 组件
    .description = 沿着这条栏拖动可以重排；在两行之间拖动，或者用小标签上的 x 和 +，来隐藏和显示。百分比隐藏时，喇叭的悬停提示里会显示它
volume-readout = 读数
    .description = 把电平显示为百分比，或者它施加的分贝增益
volume-readout-decibels = 分贝
volume-readout-percent = 百分比
volume-stretch = 拉伸
    .description = 让滑块填满面板，而不是限制宽度
volume-tip-mute = 静音
volume-tip-mute-level = 静音，{ $level }
volume-tip-unmute = 取消静音
volume-tip-unmute-level = 取消静音，{ $level }

## Shared panel content
content-filter = 筛选
content-no-track = 没有曲目
content-total-genres = 流派
content-total-time = 总时长

## Shared panel chrome
panel-columns-description = 显示哪些曲目列
panel-headings = 分组标题
panel-jump-to-playing = 跳到正在播放
panel-menu-display = 显示
panel-title-artists = 艺术家
panel-title-genres = 流派
panel-title-oscilloscope = 示波器
panel-title-particles = 粒子
panel-title-playback = 播放
panel-title-seek = 进度
panel-title-shader = 着色器
panel-title-spectrogram = 频谱图
panel-title-spectrum = 频谱
panel-title-theme-toggle = 主题切换
panel-title-track-info = 曲目信息
panel-title-volume = 音量
panel-title-vu = VU 表
panel-title-waveform = 波形

## Everything else
choice-both = 两者都要
choice-dim = 变暗
choice-hide = 隐藏
composite-add-panel = 添加面板
composite-host-settings = { $host } 设置
composite-move-left = 左移
composite-move-right = 右移
composite-remove = 移除
composite-replace = 替换
group-panel-add-slot = 添加槽位
group-panel-move-down = 下移
group-panel-move-up = 上移
group-panel-remove-slot = 移除槽位
group-panel-split-side-by-side = 左右分隔
group-panel-split-stacked = 上下分隔
group-panel-swap-panels = 交换面板
group-panel-title = 分组
overlay-dim = 变暗
    .description = 覆盖层露出来时主面板在下面暗到什么程度
overlay-title = 覆盖层
overlay-toggle = 切换覆盖层
shader-confirm-hint-after = 可以在任何地方切换着色器。
shader-confirm-hint-before = 着色器可能让窗口变得难用。还原或者关掉这个窗口就能回到原样。
shader-confirm-keep = 保留
shader-confirm-question = 保留这个屏幕着色器？
shader-confirm-revert = 还原
shader-confirm-window-title = rox - 覆盖着色器
slide-add = 添加幻灯片
slide-next = 下一张
slide-previous = 上一张
slide-title = 幻灯片
theme-toggle-to-dark = 切换到深色主题
theme-toggle-to-light = 切换到浅色主题
transport-favourite-add = 加入收藏
transport-favourite-nothing = 没有可收藏的
transport-favourite-remove = 从收藏移除
transport-pieces = 组件
    .description = 沿着一行拖动可以重排，在行与行之间拖动可以移动；小标签上的 x 和 + 负责隐藏和显示

## Stragglers picked up in the final sweep
duplicates-scanning = 正在扫描…
about-copyright = 版权所有 © 2026
signal-name-placeholder = 信号名称
signals-empty = 还没有信号。加一个，或者右键任意可绑定的旋钮。
signal-add = 添加信号
panel-approve = 批准
panel-turn-off = 关闭
shader-from-file = 从文件…
arrange-add-row = 添加行
smart-playlist-name-placeholder = 播放列表名称
smart-playlist-name-to-save = 给播放列表起个名字才能保存
panel-new-playlist = 新建播放列表…
panel-edit-tags = 编辑标签…
panel-edit-cover = 编辑封面…
panel-rename-files = 重命名文件…
panel-convert = 转换…
panel-catalog-drag-anchor = 拖动锚点
panel-catalog-spacer = 间隔

## Duration and worker phrasing
pace-under-a-minute = 不到一分钟
pace-minutes = { $count ->
   *[other] 大约 { $count } 分钟
}
pace-hours = { $count ->
   *[other] 大约 { $count } 小时
}
pace-half-hours = 大约 { $value } 小时
pace-days = { $count ->
   *[other] 大约 { $count } 天
}
pace-workers = { $count ->
   *[other] { $count } 个并发
}
tasks-rest-takes = ，剩下的要 { $estimate }
tasks-measuring-takes = ，测量它们要 { $estimate }
tasks-working-out-takes = ，算出它们要 { $estimate }
tasks-time-left = ，还剩 { $left }
tasks-failed-suffix = （{ $count } 个失败）
tasks-file-suffix = - { $file }
tasks-no-beat-suffix = （找不到明显节拍 { $count } 个）
tasks-estimate-at-workers = （{ tasks-estimate-at }）

## Panel vanity names
panel-title-art-view = 封面视图
panel-title-artist-grid = 艺术家墙
panel-title-genre-grid = 流派墙
panel-title-biography = 简介
panel-title-cover-art = 封面
panel-title-drag-anchor = 拖动锚点
panel-title-drawer = 抽屉
panel-title-eq-widget = 均衡器部件
panel-title-filter = 筛选
panel-title-folder-tree = 文件夹树
panel-title-group = 分组
panel-title-history = 播放历史
panel-title-lyrics = 歌词
panel-title-menu = 菜单
panel-title-metadata = 元数据
panel-title-mini-toggle = 迷你切换
panel-title-output = 输出
panel-title-overlay = 覆盖层
panel-title-playlists = 播放列表
panel-title-queue = 队列
panel-title-queue-widget = 队列部件
panel-title-search = 搜索
panel-title-slide = 幻灯片
panel-title-spacer = 间隔
panel-title-stats-widget = 统计部件
panel-title-vu-meter = VU 表
panel-title-window-controls = 窗口按钮

## Relative time and the output headline
ago-just-now = 刚刚
ago-minutes = { $count } 分钟前
ago-hours = { $count } 小时前
ago-days = { $count } 天前
ago-weeks = { $count } 周前
ago-years = { $count } 年前

span-seconds = { $count ->
   *[other] { $count } 秒
}
span-minutes = { $count ->
   *[other] { $count } 分钟
}
span-hours = { $count ->
   *[other] { $count } 小时
}
span-days = { $count ->
   *[other] { $count } 天
}
span-weeks = { $count ->
   *[other] { $count } 周
}
span-years = { $count ->
   *[other] { $count } 年
}
span-pair = { $first } { $second }
unit-percent = { $value }%

settings-audio-output-headline = { $mode }{ $note }，设备 { $device }，{ $rate } Hz，{ $channels } 声道，{ $format }
settings-audio-output-experimental =  （实验性）

## ML model catalog
settings-mlmodels-description = { $summary }。每首曲目 { $dim } 个值。{ $licence }
settings-mlmodels-on-disk = ，占用 { $size }
settings-mlmodels-to-download = ，需下载 { $size }
model-summary-dsp-timbre-1 = 内置，无需下载。把每首曲目的对数频段能量、频谱形状和起音率概括一遍。和训练过的网络比是粗了，但它什么都不用装，哪儿都能跑
model-summary-panns-cnn10 = 一个在 AudioSet 上训练的卷积网络，用来识别声音是什么。它对一首曲目的 512 维描述远比内置的草图丰富，代价是 24 MB 的下载和更慢的分析

## Shipped workspaces
workspace-shipped-default = （默认）
workspace-shipped-default-blurb = rox 开箱时的样子：桌面之上的半透明表面、没有窗口装饰、封面染色关着。这里其他每一套外观都是从这个起点走出去的。
workspace-shipped-catrox-blurb = 一切的起点、那套 foobar2000 皮肤的重制：封面渲染成圆形 CD、元数据字段沿左侧排下来，曲目按专辑分组并带评分小点。
workspace-shipped-critters-blurb = 整个应用变成 1 位印刷品：每一块表面上的有序抖动、跟着超低频压碎的色调，还有一面随歌扭动的噪点墙。致敬 Critters for Sale。
workspace-shipped-diffuse-blurb = 只有正在播放的专辑：封面和播放卡片作为一组填满窗口，透明表面浮在背景上，没有接缝。媒体库、队列和歌词在右边缘的抽屉里候着，把手一悬停就滑出来盖在音乐上。单色设计，颜色交给封面。
workspace-shipped-foobar-blurb = 整个项目都在跟它较劲的那套布局。不透明的面板、艺术家和专辑筛选列、密集的曲目表格，还有菜单栏一直待着的那个位置。
workspace-shipped-llama-winamp-blurb = 你记忆里的 Winamp，而不是它当年真实的样子。Tahoma、深色、无窗口装饰、顶部一条点阵频谱，迷你布局上还有一个卷帘模式。
workspace-shipped-metro-blurb = Segoe UI 下的扁平面板和宽松行距，封面染色开着，整套调色板跟着正在播放的封面走。
workspace-shipped-phosphor-blurb = 全等宽。Consolas、黑底绿字、快速播放里不放封面：一个碰巧能放音乐的终端。
