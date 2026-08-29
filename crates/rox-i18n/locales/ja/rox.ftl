### 日本語。en-CA/rox.ftl をキーごとにそのまま写したもの。
### rox-i18n のパリティテストがそれを守っている。
### キーは画面ごとの接頭辞付き kebab-case。行の説明文はラベルの
### メッセージに付く属性。

## Shared widgets
tracking-title = 追従
tracking-follow = 再生中を追う
tracking-resume = 操作をやめたら再開
tracking-smooth = スムーススクロール
align-row = 配置
    .description = パネルに余裕があるとき、中身をどこに置くか
valign-row = 垂直方向の配置
    .description = パネルに高さの余裕があるとき、中身をどこに置くか
valign-top = 上
valign-middle = 中央
valign-bottom = 下
letter-rail-compact = コンパクトな文字インデックス
    .description = インデックスを折り返さず、1行に収めてスクロールする
letter-rail-side = インデックスの位置
    .description = 壁のどちらの端にインデックスを配置するか

## Panel source and search rows
source-track = トラック
    .description = 再生中を追うか、ライブラリの選択を追うか
source-follow-playing = 再生中を追う
source-follow-selection = 選択を追う
source-playing = 再生中
source-selected = 選択中
query-search = 検索
query-search-box = 検索ボックス
    .description = 検索ボックスを表示する。検索語は表示されているあいだだけ効く
query-source = 検索ソース
    .description = 共有の検索語を追うか、このパネル自身のボックスで絞るか、他のパネルが選んだものを表示するか
query-source-shared = 共有
query-source-own = 自前
query-source-selection = 選択

## Signals and routes
signal-source = ソース
    .description = シグナルが何を追うか。バンドは一つの周波数帯、レベルはミックス全体、オンセットはその帯域の打点ごとにパルスを出し、トリガーは帯域がしきい値に達したときにパルスを出し、トータルは別のシグナルを時間で積み上げる
signal-kind-band = バンド
signal-kind-level = レベル
signal-kind-onset = オンセット
signal-kind-trigger = トリガー
signal-kind-total = トータル
signal-response = 応答
signal-response-pulse = 各パルスが消えるまで鳴り残る長さ
signal-response-drift = 0 は音に張り付き、100 は遅れて追う
signal-threshold = しきい値
signal-threshold-trigger = パルスを出すために帯域が達しなければならないレベル。上のメーターの目印より下に戻るまで再発火しない
signal-threshold-gate = これより下ではシグナルはゼロ扱い、上では出力がまたゼロから登るので、静かな部分でつまみが動かない。上のメーターの目印がその位置
signal-low-bound = 下限
signal-high-bound = 上限
signal-adds-up = 積み上げ元
    .description = どのシグナルを合算するか。そのシグナルが高いあいだ登り、静かなあいだは止まる
signal-aggregate-nothing = 追えるシグナルがない
signal-aggregate-pick = シグナルを選ぶ
signal-aggregate-alone = プールに合算できる他のシグナルがないので、ここはゼロのままです。追加すればリストに出てきます。
signal-aggregate-unpicked = 何も選ばれていないので、このトータルはゼロのままです。上でシグナルを選んでください。
signal-rate = レート
    .description = 入力が最大のときの毎秒の周回数。1 を超えると 0 に戻ってまた登るので、シェーダーはこれを位相として読む
signal-reset-on-track = 曲頭でリセット
    .description = 新しい曲が始まったらゼロまで戻す。位相が前の曲の合計から始まらないように
signal-flush = ゼロに戻す
signal-routes-in-panel = { $count ->
   *[other] このパネル内の { $count } 個のルート
}
    .description = 今すぐゼロに戻す。急に飛ばずに少しかけて抜けるので、これを追うものが跳ねない
route-header = ルート
route-signal = シグナル
    .description = このルートがどの共有シグナルを追うか。ここでの調整はそのシグナル上のすべてのルートに効く
route-new-signal = 新規シグナル
route-shared-note = このシグナル上のすべてのルートで共有
route-signal-gone = このルートのシグナルが失われています。上で別のものを選ぶまで、つまみはスライダーの値を保ちます。
route-range-note = このパラメーターだけの範囲
route-quiet = 無音時
    .description = 無音のときにつまみが示す値。自身の設定値に対する割合
route-loud = 最大時
    .description = シグナルが最大のときに示す値。100% はスライダー自身の値、無音時より下なら下向きに変調する
route-slot = スロット
    .description = シェーダーの 16 個のシグナルスロットのうち、このルートが埋めるもの
route-slot-quiet-description = 無音のときスロットが示す値
route-slot-loud-description = シグナルが最大のとき示す値。無音時より下だとスロットが逆走する
route-slot-signal-description = このルートがどの共有シグナルを追うか
route-slot-signal-gone = このルートのシグナルが失われています。別のものを選ぶまでスロットはゼロを返します。
route-add = ルートを追加
route-unrouted = ルートなし
route-pick-slot = スロットを選ぶ
route-pick-signal = シグナルを選ぶ
route-no-signal = シグナルなし
route-no-signals-yet = 追えるシグナルがまだありません。作ればここに出てきます。それまでスロットはゼロを返します。
route-open-signals = シグナルを開く
route-create-signal = シグナルを新規作成

## Panel settings window
panel-settings = パネル設定
panel-menu-label = パネル
panel-save-as-preset = プリセットとして保存
panel-rename = 名前を変更
panel-rename-name = 名前
panel-rename-note = パネルのタブとして表示される。空にすると組み込みの名前に戻る
panel-rename-hint-after = で名前を変更
panel-was-closed = パネルは閉じられました
panel-reset = リセット
panel-inverse = 反転
panel-apply-song-theme = 曲テーマを適用
panel-page-appearance = 外観
panel-page-behavior = 動作
panel-page-shader = シェーダー
panel-section-placement = 配置
panel-section-size = サイズ
panel-section-opacity = 不透明度
panel-section-frame = 枠
panel-section-colors = 色
panel-section-font = フォント
panel-section-shader = シェーダー
panel-section-signals = シグナル
panel-section-slots = スロット
panel-awaiting-approval = 承認待ち
panel-size-off = オフ
panel-locked = ロック
    .description = パネルをその場に固定する。ドックでのドラッグも並べ替えもできなくなる
panel-drag-anchor = ドラッグ領域
    .description = パネル上のどこをドラッグしてもウィンドウが動き、普通のクリックはコントロールに届く。ウィンドウ枠なしのレイアウト向け
panel-slot-controls = スロット操作
    .description = このパネルの中に置いたパネルを入れ替え・削除する隅のボタンを表示する。隠しても、レイアウトは設定のワークスペースページのツリーから編集できる
panel-min-width = 最小幅
    .description = リサイズがパネルを狭めるのをやめる位置。書いたとおりに効き、パネル自身の下限より下も許すので、細いストリップは既定より詰められる。空なら下限はそのまま
panel-max-width = 最大幅
    .description = パネルの幅に上限を設け、ウィンドウが広がっても伸びないようにする
panel-min-height = 最小高さ
    .description = リサイズがパネルを縮めるのをやめる位置。書いたとおりに効き、パネル自身の下限より下も許すので、細いストリップは既定より詰められる。空なら下限はそのまま
panel-max-height = 最大高さ
    .description = パネルの高さに上限を設け、ウィンドウが高くなっても伸びないようにする
panel-own-opacity = 独自の面の不透明度
    .description = このパネルに、アプリ共通ではなく独自の不透明度を持たせる
panel-surface-opacity = 面の不透明度
panel-margin = 外側の余白
    .description = パネルをセルの内側に寄せ、隙間から背景を見せる
panel-padding = 内側の余白
    .description = パネルの縁の内側の余白。パネル自身の背景のまま
panel-rounding = 角の丸み
    .description = パネルの角を丸めて背景になじませる
panel-border = 枠線
    .description = パネルの縁を囲む線。枠線ロールの色で描かれる。0 の辺には引かれない
panel-font = フォント
    .description = パネルの書体。既定はアプリのフォントに従う
panel-font-size = フォントサイズ
    .description = アプリフォントに対するパネルの文字サイズ。行もこれに合わせて拡縮する
panel-surface-shader = 面のシェーダー
    .description = このパネルの本体に WGSL シェーダーを走らせる。アプリの画面シェーダーの下に入る
panel-run-when-idle = 無音時も動かす
    .description = 音が止まっていてもフレームを描き続ける。オフならシェーダーは最後のフレームで止まり、パネルは負荷ゼロになる
panel-shader-is-scene = このシェーダーはシーンなので、パネル本体の上に重ねるのではなく覆ってしまいます。バンドルか古い設定から来たものです。上のリストには、パネルが読めるまま残るシェーダーだけが並びます。

## Shader picker and saving
shader-source = ソース
shader-pick-none = なし
shader-reload = 再読み込み
shader-edit-as-file = ファイルとして編集
shader-make-private-copy = 専用のコピーを作る
shader-save-replace = 置き換える
shader-save-to-workspace = ワークスペースに保存
shader-save-replaces = このワークスペースが既に { $name } と呼んでいるシェーダーを置き換えます。その名前を使うすべてのパネルが一緒に変わります
shader-save-adds = このワークスペースのシェーダーに { $name } として追加します。どのパネルからも使え、編集すればすべてに反映されます
shader-group-examples = サンプル
shader-group-this-workspace = このワークスペース
shader-group-scenes = シーン
shader-group-workspace-scenes = ワークスペースのシーン
shader-group-overlays = オーバーレイ
shader-group-workspace-overlays = ワークスペースのオーバーレイ

## Saving a panel preset
preset-save = プリセットを保存
preset-save-name = プリセット名
preset-save-replaces = このワークスペースが既に { $name } と呼んでいるプリセットを置き換えます
preset-save-hint-after = で保存
preset-back-from = 戻すときは
preset-back-add-panel = パネルを追加
preset-back-then = そのあと
preset-back-presets = プリセット
preset-back-tail = をどのパネルメニューからでも。プリセットはこのワークスペース専用で、他のワークスペースにはありません。

## Keyboard hints
hint-press = 押す
hint-key-enter = Enter

## Settings: language
settings-language = 言語
    .description = 画面の表示言語。システムは OS の言語リストと照合し、一致しなければ英語に戻る
    .keywords = 言語 げんご gengo 翻訳 ほんやく honyaku ロケール locale language translation
settings-language-system = (システムの言語)
settings-language-search = 言語を検索
picker-no-matches = 一致なし
settings-search-no-matches = "{ $text }" に一致するものがありません

## Embed dialog
bake-window-title = rox - 保存済みメタデータの埋め込み
bake-title = 保存済みメタデータの埋め込み
bake-intro = 保存済みのメタデータをファイル自体に書き込み、他のプレイヤーからも読めるようにします。再計算はしません。
bake-formats = MP3 と FLAC のみ。他の形式と CUE のトラックはスキップされます
bake-source-lyrics = 歌詞
bake-source-gain = ReplayGain
bake-source-acoustic = 音響的な記述
bake-detail-nothing = 埋め込む保存データがない
bake-detail-only-skipped = 書き込むものなし、{ $skipped } 件スキップ
bake-detail-writes = { $count ->
   *[other] { $count } 件のファイルに書き込み
}
bake-detail-writes-skipped = { $count ->
   *[other] { $count } 件のファイルに書き込み、{ $skipped } 件スキップ
}
bake-error-read = ライブラリを読めませんでした: { $error }
bake-survey-counting = ライブラリを調べています...
bake-survey-progress = タグを読み込み中、{ $total } 件中 { $done } 件
bake-nothing-to-embed = 埋め込むものがありません。rox が持っているものは既にファイル側にあります
bake-rewrites = { $count ->
   *[other] { $count } 件のファイルが書き直されます
}
bake-hint-before = 埋め込むには
bake-hint-key = Enter
bake-hint-after = を押す
bake-embed = 埋め込む
bake-cancel = キャンセル
bake-summary-files = { $count ->
   *[other] { $count } 件のファイル
}
bake-summary-updated = { $files } を更新
bake-summary-stopped = { $files } を更新したところで停止
bake-summary-skipped = 、{ $count } 件をスキップ
bake-summary-failed = 、{ $count } 件が失敗

## Arrange editors and header pieces
arrange-shown = 表示
arrange-hidden = 非表示
tile-face-mosaic = カバーのモザイク
tile-face-tinted = 色付きモザイク
tile-face-gradient = グラデーションカード
tile-face-color = カラーカード
head-piece-artist = アーティスト
head-piece-album = アルバム
head-piece-year = 年
head-piece-genre = ジャンル
head-piece-quality = 音質
head-piece-tracks = トラック
head-piece-time = 時間
head-piece-spacer = スペーサー
head-piece-divider = 区切り線
head-piece-art = アート
head-unknown = 不明
status-item-count = 件数
status-item-time = 時間
status-item-albums = アルバム
status-item-artists = アーティスト
status-item-plays = 再生回数
volume-item-icon = アイコン
volume-item-slider = スライダー
volume-item-percent = パーセント

## Filter chips and search menus
filter-field-artist = アーティスト
filter-field-album-artist = アルバムアーティスト
filter-field-album = アルバム
filter-field-genre = ジャンル
filter-field-year = 年
filter-field-folder = フォルダー
filter-unknown = 不明
filter-clear = クリア
query-show-search-box = 検索ボックスを表示
query-own-query = 自前の検索
query-shared-query = 共有の検索
headers-off = オフ
headers-compact = コンパクト
headers-expanded = 拡張

## Panel context menu
panel-dock-back = ドックに戻す
panel-pop-out = 切り離す
panel-close = 閉じる
panel-duplicate = 複製
panel-reveal-in-browser = ファイルマネージャーで表示
panel-play-next = 次に再生
panel-add-to-queue = キューに追加
panel-add-to-playlist = プレイリストに追加
panel-favourite-add = お気に入りに追加
panel-favourite-remove = お気に入りから削除
shader-pick-missing = { $name } (見つかりません)
shader-pick-custom = カスタム

## Shipped shader examples
shader-blurb-plasma = ユニフォームだけから流れる色を描くので、ただの四角一枚分しかかからない。
shader-blurb-trails = 自分の前フレームを引き伸ばすので、画面パスで走る。
shader-blurb-sheen = ビネットと流れる光沢。既に描いているパネルに重ねる透過オーバーレイ。
shader-blurb-shadow = パネル自身の文字とコントロールが落とすドロップシャドウ。マスクキャプチャから拾う。
shader-blurb-cover = 再生中の曲のアート。自身の色のにじみの上にレターボックスで置く。
shader-blurb-badge = カバーを小さなカードとして隅に置く。動かすためのスロット付き。
shader-blurb-lamp = カーソルを追い、クリックに反応する光。透過オーバーレイ。
shader-blurb-cube = 疑似 3D で転がるワイヤーフレームの立方体。加算光として描く。
shader-blurb-bloom = 半分のサイズの second pass でブルームをかけた流れる球。チェーンの縮小版。
shader-blurb-tube = 下のパネルを湾曲した CRT 面に映し直す。走査線付き。

## Transport strip pieces
seek-item-elapsed = 経過
seek-item-strip = バー
seek-item-ending = 残り
seek-item-duration = 長さ
info-item-track-no = トラック番号
info-item-title = タイトル
info-item-duration = 長さ
info-item-next = 次
info-item-queued = キュー
info-item-output = 出力
info-item-favourite = お気に入り
info-item-rating = レーティング
playback-item-previous = 前へ
playback-item-seek-back = 巻き戻し
playback-item-play = 再生
playback-item-seek-forward = 早送り
playback-item-next = 次へ
playback-item-stop = 停止
playback-item-volume = 音量
playback-item-loop = リピート
playback-item-shuffle = シャッフル
playback-item-continue = 継続再生
playback-item-crossfade = クロスフェード
playback-item-random = ランダム
playback-item-stop-after = この曲で停止
playback-item-favourite = お気に入り
playback-item-rating = レーティング

## Dock chrome
dock-empty-tab = 空のタブ
dock-unnamed = 名前なし
dock-tiles = タイル
dock-zoom-in = 拡大
dock-zoom-out = 縮小
dock-collapse = 折りたたむ
dock-expand = 広げる

## Shader picker notes
shader-note-empty = まずサンプルを選ぶか、fs_user(uv) を定義したフラグメントステージを持つ .wgsl ファイルを rox に指定してください
shader-note-missing = { $name } はこのワークスペースのシェーダーにもう無いので、何も描かれません。ここで別のものを選べば、このパネルは自前のソースを持ちます。
shader-note-shared = このワークスペース内で共有。編集すると、これを使うすべての面に反映されます。
shader-note-file = { $path }。保存するたびシェーダーを描いたまま読み込み直します。ソースはレイアウトやバンドルの中にも保存されるので、そのファイルを持たないマシンでも動きます。
shader-note-custom = このソースはレイアウトやバンドルの中に保存されていて、裏にファイルはありません。ファイルとして編集すると書き出され、以後の保存を拾います。

## Panel pages and shared sides
panel-page-layout = レイアウト
panel-page-view = 表示
panel-page-content = 内容
panel-page-source = ソース
panel-page-bindings = バインド
panel-page-emitters = エミッター
panel-page-forces = 力
panel-page-scene = シーン
side-left = 左
side-right = 右
genre-face-mosaic = モザイク
genre-face-tinted = 色付き
genre-face-gradient = グラデーション
genre-face-color = 単色

## Library panel
panel-title-library = ライブラリ
library-play = 再生
library-play-album = アルバムを再生
library-play-group = グループを再生
library-play-tracks = { $count } 曲を再生
library-play-similar = 似た曲を再生
library-filter-by-album = アルバムで絞る
library-filter-by-artist = アーティストで絞る
library-jump-to-playing = 再生中へ移動
library-menu-display = 表示
library-disc = ディスク { $number }
library-empty-title = 音楽フォルダーを開く
library-empty-note = ライブラリに読み込まれます (flac, mp3, wav)
library-headers = 見出し
    .description = リスト上のグループ区切り。並べ替えても連続した固まりは保たれ、検索中はフラットに表示される
library-group-by = グループ化
    .description = 見出しが何で区切るか。ジャンルと年はリストを並べ替える
library-header-row = 見出し行
    .description = 一行見出しが左から右へ何を出すか。スペーサーか区切り線で左右に分かれる
library-header-lines = 見出しの行
    .description = ブロックの行を上から下へ。空の行は消える
library-follow-description = 曲が変わるたびに再生中の行までスクロールする
library-resume-description = 眺めるのをやめたら再生中の行へ戻る
library-smooth-description = 飛ばずに行までなめらかに移動する
library-columns = 列
    .description = どの列を表示するか。パネル内の見出しをドラッグして並べ替えと幅の調整ができる
library-column-headers = 列見出し
    .description = リスト上の並べ替え用の見出し行。隠しても列の順序と幅は保たれる
library-compact-plays = 再生回数を詰めて表示
    .description = 再生回数の列を、横にダッシュを添えた小さな数字にする
library-line-height = 行送り
    .description = 見出しの一行分。ブロックは必要な行数を取り、トラックの行とは無関係
library-text-size = 文字サイズ
    .description = 見出しの行の文字。行送りとは独立なので、アートだけを大きくできる
library-flush-background = 背景をそろえる
    .description = 見出しを浮いた色付き背景ではなくリストの背景に置く。曲テーマは両方まとめて動かす
library-gap-above = 上の間隔
    .description = ブロックの上から削る分。リストが透けて見え、行は詰まって収まる
library-gap-below = 下の間隔
    .description = ブロックの下、そのトラックの手前でも同じ
library-section-rows = 行
library-row-height = 行の高さ
    .description = トラックの行。文字も追随し、どちらもアプリフォントに合わせて拡縮する
library-row-spacing = 行間
    .description = 各行が余分に取る高さ。文字を大きくせずに余裕を作る
library-stripes = 交互の背景色
    .description = 一行おきにトラック行を色づけし、長いリストを追いやすくする
library-row-borders = 行の境界線
    .description = 各トラック行の下の細い線
library-art-description = 拡張見出しのタイル。カバー、アーティストの写真、ジャンルの表示のいずれか
library-art-rounding = アートの角の丸み
    .description = アートの角を丸める
library-art-position = アートの位置
    .description = 拡張見出しのタイルをブロックのどちら側に置くか
library-art-margin = アートの余白
    .description = タイルをブロックの内側に寄せる。正方形を保つように縮む
library-circular-portraits = 円形の写真
    .description = アーティストでグループ化したとき、角の丸みの設定ではなくウォール全体を円に切り抜く
library-genre-face = ジャンルの表示
    .description = ジャンルでグループ化したとき、タイルに何を出すか。カバー、ジャンルの色をかけたカバー、または図形を敷いたカラーカード

## Album grid panel
panel-title-album-grid = アルバムグリッド
grid-menu-scroll = スクロール
grid-menu-sort = 並べ替え
grid-sort-artist = アーティスト
grid-sort-album = アルバム
grid-sort-year = 年
grid-sort-added = 最近追加した順
grid-sort-plays = 再生回数順
grid-letter-rail = 文字インデックス
    .description = 壁の端に頭文字を並べる。クリックでその文字の最初のアルバムへ移動
grid-vertical-scroll = 縦スクロール
grid-horizontal-scroll = 横スクロール
grid-jump-to-playing = 再生中へ移動
grid-library-empty = ライブラリが空です
grid-play-albums = { $count } 枚のアルバムを再生
grid-vertical-layout = 縦レイアウト
    .description = ウォールを上下にスクロールし、行が幅を埋める。オフなら左右にスクロールし、列が高さを埋める
grid-follow-description = 曲が変わるたびに再生中のアルバムまでスクロールする
grid-resume-description = 眺めるのをやめたら再生中のアルバムへ戻る
grid-smooth-description = 飛ばずにアルバムまでなめらかに移動する
grid-section-dimming = 減光
grid-section-tiles = タイル
grid-dim-while-playing = 再生中は暗くする
    .description = 再生中のアルバム以外のカバーを暗くする。カーソルを乗せたタイルは戻る
grid-dim-amount = 減光量
    .description = 他のカバーをどこまで暗くするか。100% で見えなくなる
grid-desaturate = 再生中は彩度を落とす
    .description = 再生中のアルバム以外のカバーをグレースケールにする。カーソルを乗せたタイルは色が戻る
grid-always = 常に
    .description = 何も再生していなくてもカバーを引っ込めたままにする。カーソルを乗せたタイルだけがはっきり見える
grid-show-titles = タイトルを表示
    .description = カーソルを乗せたときだけでなく、iTunes 風に各カバーの下へアルバムとアーティストを出す
grid-title-alignment = タイトルの配置
    .description = キャプションをカバーの下でそろえる
grid-tile-size = タイルサイズ
    .description = カバータイルの長辺。列はパネルの幅を均等に割る
grid-gap = 間隔
    .description = カバーどうしの隙間。0 で隙間なく敷き詰める
grid-art-rounding-description = 各カバーの角を丸める。100% で円になる

## Settings: sidebar pages
settings-page-appearance = 外観
settings-page-application = アプリケーション
settings-page-audio = オーディオ
settings-page-development = 開発
settings-page-integrations = 連携
settings-page-keymap = キー割り当て
settings-page-library = ライブラリ
settings-page-mcp = MCP
settings-page-ml-models = ML モデル
settings-page-playback = 再生
settings-page-providers = プロバイダー
settings-page-shader = シェーダー
settings-page-storage = ストレージ
settings-page-workspace = ワークスペース

## Settings: appearance
settings-appearance-backdrop-all-windows = すべてのウィンドウ
    .description = 子ウィンドウにも背景を敷く。設定、エディター、ダイアログ、切り離したパネル。オフなら背景と透過はワークスペースのウィンドウだけに効く
settings-appearance-backdrop-strength = 背景の強さ
    .description = カバーの背景をどれくらい濃く後ろに出すか
settings-appearance-border = 枠線
    .description = すべてのパネルの縁を囲む線。枠線ロールの色で描かれる。0 の辺には引かれない
settings-appearance-colors-locked-note = 曲テーマがオンなので、再生中の曲がこれらの色を決め、エクスポートもその色を保存します。編集するには上でオフにしてください
settings-appearance-design-mode = デザインモード
    .description = レイアウトをその場で編集する。パネルメニューの追加・名前変更・複製・切り離し・閉じる、コンテナがスロットの上に浮かべるコントロール、タブのドラッグ。オフだとそれらは隠れるが、ワークスペースページからはツリーを編集できる
    .keywords = レイアウト 編集 並べ替え ロック layout edit
settings-appearance-font = フォント
    .description = アプリ全体の書体。パネル側の設定で個別に上書きできる
    .keywords = 書体 フォント 文字 font typeface
settings-appearance-font-size = フォントサイズ
    .description = すべてのパネルの文字が基準にするサイズ。コントロールとアイコンの大きさは変わらない
settings-appearance-hide-menubar = メニューバーを隠す
    .description = メニューバーを隠したままにし、Alt を押しているあいだドックの上に浮かべる。Alt を二度押すと出したままになり、ボタンを普通にクリックできる
settings-appearance-icons-intro = パックは組み込みアイコンを差し替える SVG のフォルダーです。切り替えは次回起動時から効きます
settings-appearance-icons-open-folder = フォルダーを開く
settings-appearance-inverse-from-dark = ダークテーマから反転
settings-appearance-inverse-from-light = ライトテーマから反転
settings-appearance-keep-theme = テーマを固定
    .description = カバーの明るさで切り替わるはずのところでも今のテーマを保つ。曲テーマによる色づけは効いたまま
settings-appearance-margin = 外側の余白
    .description = すべてのパネルをセルの内側に寄せる。パネル側の設定で個別に上書きできる
settings-appearance-new-pack = 新しいパック
settings-appearance-os-decorations = OS のウィンドウ枠
    .description = メインウィンドウの OS タイトルバーと枠。オフのときはウィンドウコントロールとドラッグ領域パネルに任せる
settings-appearance-pack-name-placeholder = パック名
settings-appearance-padding = 内側の余白
    .description = すべてのパネルの縁の内側の余白。パネル自身の背景のまま
settings-appearance-palette-export = エクスポート
settings-appearance-palette-import = インポート
settings-appearance-panel-seams = パネルの継ぎ目
    .description = パネルのタイル間の細い線。オフだとリサイズのつまみは見えなくなるが、ドラッグはできる
settings-appearance-resize-border = リサイズ用の縁
    .description = メインウィンドウの縁をドラッグしてリサイズする。OS のウィンドウ枠がオフのときだけ効き、これをオフにするとスナップと Win+方向キーがリサイズの手段になる
settings-appearance-rounding = 角の丸み
    .description = すべてのパネルの角を丸めて背景になじませる
settings-appearance-section-colors = 色
settings-appearance-section-frame = 枠
settings-appearance-section-icons = アイコン
settings-appearance-section-interface = インターフェース
settings-appearance-section-theming = テーマ
settings-appearance-section-transparency = 透過
settings-appearance-section-typography = タイポグラフィ
settings-appearance-song-theming = 曲テーマ
    .description = 再生中の曲のカバーアートでパレットを色づけし、ウィンドウの背景にも使う
settings-appearance-surface-opacity = 面の不透明度
    .description = アプリの面が背景の上でどれくらい不透明に見えるか
settings-appearance-theme = テーマ
    .description = アプリが描くパレットと、下の色エディターが対象にするパレット。システムは OS のライト・ダーク設定に従う
settings-appearance-theme-dark = ダーク
settings-appearance-theme-light = ライト
settings-appearance-theme-system = システム

## Settings: application
settings-application-check-updates = 更新を確認
    .description = rox の起動時に一日一度、新しいリリースを探す。バージョン情報のウィンドウはどちらにせよその場で確認する
settings-application-download-updates = 更新をダウンロード
    .description = 新しいリリースが見つかったら、裏でダウンロードして用意しておく。次回の起動でそれが動く
settings-application-enable-ai = AI 機能を有効にする
    .description = AI ツールから rox を触れるようにする。MCP 対応と ML モデルのダウンロードが加わり、そのページがサイドバーに並ぶ。
settings-application-lock-panel-resize = パネルのリサイズをロック
    .description = パネルの分割はデザインモードがオンのときだけリサイズできる。継ぎ目の近くをドラッグしても完成したレイアウトが動かない
settings-application-portable-copying = データをコピー中...
settings-application-portable-mode = ポータブルモード
    .description = 設定・ライブラリ・キャッシュを実行ファイルの隣の rox-data フォルダーに置き、プレイヤーがデータごと移動できるようにする。オフに戻すとシステムのフォルダーを使い、rox-data はそのまま残る
settings-application-portable-not-writable = アプリのフォルダーに書き込めません
settings-application-portable-restart-note = 次回の起動から効きます。今回の実行は今のフォルダーのままです
settings-application-remain-in-tray = トレイに残す
    .description = 最後のウィンドウを閉じても音楽を止めず、トレイアイコン (macOS では Dock) から戻れるようにする
settings-application-section-ai = AI
settings-application-section-control-socket = 制御ソケット
settings-application-section-data = データ
settings-application-section-layout = レイアウト
settings-application-section-startup = 起動
settings-application-section-window = ウィンドウ
settings-application-socket-path = ソケットのパス
    .description = 実行中の rox のマシン向けインターフェース。ローカルソケット上の JSON-RPC で、このデータフォルダーに紐づく。rox-mcp プロキシがこれを通して MCP クライアントに応える

## Settings: audio
settings-audio-broadcast-bitrate = ビットレート
    .description = MP3 エンコーダーがストリーム 1 秒あたりに使う量
settings-audio-broadcast-enable = Icecast に配信
    .description = rox が再生しているものをソースクライアントとして icecast サーバーへ MP3 で送る。マウント、リスナー、外向きの見せ方はすべて icecast 側の話で、rox は接続しに行くだけ。サーバーに届かなくてもローカルの再生には触れない
settings-audio-broadcast-host-placeholder = icecast のホスト
settings-audio-broadcast-login = ソースのログイン
    .description = icecast のソース認証情報。設定ファイルで指定したユーザーとパスワード
settings-audio-broadcast-mount = マウント
    .description = リスナーが合わせるマウントと、そこで名乗るストリーム名
settings-audio-broadcast-name-placeholder = ストリーム名
settings-audio-broadcast-password-placeholder = ソースのパスワード
settings-audio-broadcast-server = サーバー
    .description = icecast サーバーのホストとポート。ソースプロトコルは素のソケット上で動く
settings-audio-broadcast-user-placeholder = source
settings-audio-crossfade = クロスフェード
    .description = 次の曲とどれくらい重ねるか。フェードはシャッフルとスキップのためのものなので、アルバム内の曲間は下の行で指定しない限り触らない。0 でオフ
    .keywords = ギャップレス 重ね フェード 曲間 gapless crossfade
settings-audio-equalizer-note = 出力にかかる 10 バンドのオクターブイコライザー。一度決めて終わりではなく音を聴きながら触るものなので、専用のウィンドウで開きます
settings-audio-exclusive-mode = 排他モード
    .description = デバイスを rox 専用にし、ハードウェアが受けるならファイル自身のレートで鳴らす。オフならデスクトップの他のものとシステムミキサーを共有する
settings-audio-fade-inside-albums = アルバム内でもフェード
    .description = 同じ盤に属する曲どうしも重ねる。オフなら盤のつなぎ目はマスタリングされたまま残る。ギャップレスが一番効くのはそこ
settings-audio-open-equalizer = イコライザーを開く
settings-audio-output-buffer = バッファ
    .description = サウンドカードが一度に抱える音の量。短いほど反応は速いが、負荷の高いマシンでは早く音が途切れる。長いほど安全でのんびりする
settings-audio-output-buffer-default = 既定 (10 ms)
settings-audio-output-device = デバイス
    .description-default = システムの既定は、デスクトップ側で設定されているものに従う
    .description-linux = 排他モードはカーネルから直接カードを掴むので、一覧はデスクトップの出力ではなくサウンドカードになる。Bluetooth などサウンドサーバー経由のデバイスは掴めるカードがなく、排他をオフにしたときだけ出る
    .description-other = 排他モードはデバイスを rox 専用にするので、モードをオフにするまでデスクトップの他のものはそこから音を出せない
settings-audio-output-device-system-default = システムの既定
settings-audio-output-experimental-badge = 実験的
settings-audio-output-experimental-tooltip = このプラットフォームの排他バックエンドは、公開されているオーディオ仕様から書かれていますが、開発者が実機で動かしたことはありません。デバイスを掴むか、理由を示して共有モードに落ちるかのどちらかで、無音になることはないはずです。おかしな挙動をしたらオフにして、このバッジの横のボタンから何が起きたか報告してください。
settings-audio-output-format = フォーマット
    .description = rox がカードに渡すフォーマット。選んだフォーマットを受けないカードは持っている中で一番広いもので動き、どれになったかは下のステータスに出る
settings-audio-output-format-f32 = 32 ビット浮動小数点
settings-audio-output-format-s16 = 16 ビット整数
settings-audio-output-format-s32 = 32 ビット整数
settings-audio-output-format-widest = 利用できる最も広いフォーマット
settings-audio-output-issue-tooltip = このマシンで排他モードがどう振る舞ったかを報告します。プラットフォームと合意されたストリームを埋めた GitHub の issue を開きます。
settings-audio-output-mode-exclusive = 排他
settings-audio-output-mode-shared = 共有
settings-audio-output-not-built = このプラットフォーム向けにはまだビルドされていません
settings-audio-output-rate-follow = ファイルに従う
settings-audio-output-sample-rate = サンプルレート
    .description = 追従するとファイルごとのレートでデバイスを開き直すので、レートが変わる境目で無音が入る。一つのレートに固定すればそれは起きず、合わないものはリサンプリングされる
settings-audio-output-status-error-hint = 別のデバイスを選ぶか、排他をオフにしてください
settings-audio-output-status-error-title = 出力なし
settings-audio-output-status-idle-hint = 曲を再生すると、デバイスが受け入れたフォーマットが出ます
settings-audio-output-status-idle-title = 再生していません
settings-audio-replaygain-level-by = 基準
    .description = すべての曲を ReplayGain タグが測った音量で鳴らし、シャッフルがマスターごとに跳ねないようにする。トラックはファイル単位、アルバムは盤全体のゲインを全曲に使うので、アルバム内の静と動はそのまま残る
    .keywords = ノーマライズ 音量 ラウドネス 音量統一 normalization loudness
settings-audio-replaygain-measure-missing-button = 未測定を測る
settings-audio-replaygain-measure-new = 新しいファイルを測る
    .description = 監視が拾ってきたものを、同期が落ち着いた時点で測る。ライブラリが増えてもここに戻らずゲインが揃う。数値は「測定したゲインの保存先」に従う。これをオンにすると、まず未測定のものを測るか尋ねる。その後は新しく入ったファイルだけを見る
settings-audio-replaygain-measuring-progress = { $total } 件中 { $done } 件を測定中
settings-audio-replaygain-measuring-start = 測定中: 何が未測定か調べています...
settings-audio-replaygain-mode-album = アルバム
settings-audio-replaygain-mode-off = オフ
settings-audio-replaygain-mode-track = トラック
settings-audio-replaygain-preamp = プリアンプ
    .description = タグのゲインすべてに加算される。ReplayGain の基準は最近の盤の音圧より下にあるので、揃えたライブラリは素のときより静かになる。その分をここで戻す。ブーストがクリップすることはない。タグのピーク値が頭を押さえる
settings-audio-replaygain-save = 測定したゲインの保存先
    .description = 測定パスが数値をどこに置くか。ライブラリのデータベースならファイルには触らない。タグなら他のプレイヤーが読む場所に同じ値が入るが、音声ファイルを書き直すことになる
settings-audio-replaygain-status-measured = スキャン済み { $total } 曲すべてに基準となるゲインがあり、うち { $measured } 曲は rox が測定しました
settings-audio-replaygain-status-tagged = スキャン済み { $total } 曲すべてに ReplayGain タグがあります
settings-audio-replaygain-untagged = タグのないファイル
    .description = ReplayGain タグを持たないファイルをどの音量で鳴らすか。誰も測っていないので、これは測定の代わりの当て推量。0 のままにすれば、タグのない曲は今までどおり鳴る
settings-audio-section-broadcast = 配信
settings-audio-section-equalizer = イコライザー
settings-audio-section-output = 出力
settings-audio-section-playback = 再生
settings-audio-section-replaygain = ReplayGain
settings-audio-transport = 再生操作
    .description = 下の設定はどれも耳で判断するものなので、このページを離れずに再生と停止ができる

## Settings: integrations
settings-integrations-discord-enable = リッチプレゼンスを有効にする
    .description = 再生中に rox の状況を Discord に表示する
settings-integrations-discord-show-lastfm = Last.fm ボタンを表示
    .description = Discord のステータスに「Last.fm で見る」ボタンを載せる
settings-integrations-discord-show-youtube = YouTube ボタンを表示
    .description = Discord のステータスに「YouTube で検索」ボタンを載せる
settings-integrations-ffmpeg-binary = FFmpeg の実行ファイル
    .description = 変換に使う ffmpeg。空なら PATH 上のものを使う
settings-integrations-ffmpeg-fail-note = 動く ffmpeg を指定するまで「変換」は隠れたままです
settings-integrations-ffmpeg-fail-title = この ffmpeg は動きませんでした
settings-integrations-ffmpeg-missing-note = 「変換」は隠れたままです。ffmpeg を入れるか、パスを実行ファイルに向けてください
settings-integrations-ffmpeg-missing-title = 動く ffmpeg が見つかりません
settings-integrations-ffmpeg-ok-note = ffmpeg は動きます。「変換」が使えます。
settings-integrations-ffmpeg-test = テスト
settings-integrations-lastfm-api-key-row = API キー
settings-integrations-lastfm-connect = 接続
settings-integrations-lastfm-disconnect = 切断
settings-integrations-lastfm-finish-connecting = 接続を完了する
settings-integrations-lastfm-hearts = { $n ->
   *[other] { $n } 件のハート
}
settings-integrations-lastfm-import-loved = お気に入り登録した曲をインポート
settings-integrations-lastfm-intro-builtin = Last.fm アカウントに接続します。ブラウザーで rox を承認すると、再生した曲がスクロブルされます
settings-integrations-lastfm-intro-custom = このビルドには api の識別情報が入っていないので、スクロブルには自分の api アカウントが要ります (Last.fm/api/account/create)。そのキーと共有シークレットを貼り付けてから接続してください
settings-integrations-lastfm-key-placeholder = API キー
settings-integrations-lastfm-love-failed = 直近の送信に失敗: { $error }
settings-integrations-lastfm-love-pending = { $hearts } 件が送信待ち
settings-integrations-lastfm-love-pending-failed = { $hearts } 件が送信待ち、直近の試行: { $error }
settings-integrations-lastfm-reconnect = 再接続
settings-integrations-lastfm-secret-placeholder = 共有シークレット
settings-integrations-lastfm-secret-row = 共有シークレット
settings-integrations-lastfm-status-confirming = 確認中...
settings-integrations-lastfm-status-connected = { $username } として接続中
settings-integrations-lastfm-status-elsewhere = 別の rox から接続されています。承認はインストールごとの api 識別情報で行うので、こちらも接続してください
settings-integrations-lastfm-status-failed = 接続に失敗: { $error }
settings-integrations-lastfm-status-not-connected = 未接続
settings-integrations-lastfm-status-rejected = Last.fm がセッションを拒否したため破棄しました。スクロブルを続けるには接続し直してください
settings-integrations-lastfm-status-requesting = トークンを要求中...
settings-integrations-lastfm-status-waiting = ブラウザーで rox を承認してから、接続を完了してください
settings-integrations-lastfm-working = 処理中...
settings-integrations-love-favourites = お気に入りを Love に反映
    .description = ハートを Last.fm の Loved Tracks に反映する。ハートを外すと向こうでも外れる
settings-integrations-scrobble-threshold = スクロブルのしきい値
    .description = 曲のどれくらいを再生したらスクロブルするか。シークバーと波形に印を出せる
settings-integrations-scrobble-tracks = 曲をスクロブル
    .description = しきい値を超えた曲を Last.fm に送る
settings-integrations-section-conversion = 変換
settings-integrations-section-discord = Discord リッチプレゼンス
settings-integrations-section-favourites = お気に入り
settings-integrations-section-lastfm = Last.fm
settings-integrations-section-scrobbling = スクロブル

## Settings: keymap
settings-keymap-clash = { $chord } は { $other } にも割り当てられています。どちらか一方しか発火しません
settings-keymap-not-bound = 未割り当て
settings-keymap-recording = キーを押してください
settings-keymap-restore = 元に戻す
settings-keymap-restore-all = すべてのキーを元に戻す
    .description = すべてのコマンドを出荷時のキーに戻す。このビルドに行が無いものも含む
settings-keymap-section-defaults = 既定
settings-keymap-undo = 取り消す
settings-keymap-undo-last = 直前のリセットを取り消す
    .description = 直前のリセットで捨てられたキーを戻す。行単位でも全体でも

## Settings: library
settings-library-acoustic-all-described = スキャン済み { $total } 曲すべてを { $label } が記述しています
settings-library-acoustic-auto = 新しいファイルを記述する
    .description = 監視が拾ってきたものを、同期が落ち着いた時点で記述する。ライブラリが増えてもここに戻らず記述が揃う。オフだと新しいファイルは「未処理を解析」を待つ。これをオンにすると、まず未処理のものを解析するか尋ねる。その後は新しく入ったファイルだけを見る
settings-library-acoustic-enable = 曲の響きを記述する
    .description = 各曲がどう聴こえるかを割り出し、再生中の曲に似た音楽をライブラリから探せるようにする。処理はすべてこのマシンで走り、大きなライブラリの記述には時間がかかる
    .keywords = 類似 音 指紋 記述 similar sound fingerprint
settings-library-acoustic-extractor = 抽出器
settings-library-acoustic-extractor-model = モデル
settings-library-acoustic-fallback = 解析中
settings-library-acoustic-partial = { $label } はスキャン済み { $total } 曲のうち { $done } 曲を記述しています。残りは「未処理を解析」が進めます
settings-library-acoustic-progress = { $running }: { $total } 件中 { $done } 件
settings-library-acoustic-progress-start = { $running }: 何が未処理か調べています...
settings-library-acoustic-save = 記述の保存先
    .description = 解析結果をどこに置くか。データベースだけならファイルには触らない。タグなら各ファイルにも写しが入るので、ライブラリを作り直しても、フォルダーを別のマシンに移しても記述が残る。ただし音声ファイルを書き直すことになる。タグに書けるのは MP3 と FLAC だけで、他の形式はデータベースの写しだけになる
settings-library-add-folder = フォルダーを追加
settings-library-duplicates = 重複...
settings-library-embed-button = 保存済みメタデータを埋め込む...
settings-library-folder-col-albums = アルバム
settings-library-folder-col-folder = フォルダー
settings-library-folder-col-size = サイズ
settings-library-folder-col-tracks = トラック
settings-library-folders-intro = ライブラリに読み込むフォルダーです。外すとそのトラックはカタログから消えますが、ファイルはそのまま残ります
settings-library-genre-separator-nudge = 区切り文字が変わりました。表示にはすぐ効きます。以前のスキャンで保存されたジャンルのリストは、上のフォルダー見出しから再スキャンするまで古い形のままです
settings-library-merge-case = 大文字小文字の違いをまとめる
    .description = 大文字小文字だけが違う値を同じものとして扱う。Rock と rock は同じジャンル、アーティスト、アルバムになり、曲数の多い表記で表示される。ファイルのタグは書かれたまま
settings-library-no-folders = フォルダーがまだありません
settings-library-repair-tags = タグを修復...
settings-library-section-folders = フォルダー
settings-library-section-stored-metadata = 保存済みメタデータ
settings-library-section-tempo = テンポ解析
settings-library-split-genres = カンマとスラッシュでジャンルを分ける
    .description = "Dubstep, Trap" や "Drum & Bass / Neurofunk" をそれぞれ別のジャンルとして数える。セミコロンは常に分割する。オフなら、スラッシュ入りで一つのジャンルを表すタグをそのまま残す。ファイルのタグは書かれたまま
settings-library-tempo-auto = 新しいファイルのテンポを測る
    .description = 監視が拾ってきたもののビートを、同期が落ち着いた時点で数える。ライブラリが増えてもここに戻らずテンポが揃う。オフだと新しいファイルは「未処理を解析」を待つ。これをオンにすると、まず未処理のものを測るか尋ねる。その後は新しく入ったファイルだけを見る
settings-library-tempo-enable = 曲の速さを割り出す
    .description = タグにテンポが無い曲のビートを数え、ライブラリでテンポの表示と並べ替えができるようにする。処理はすべてこのマシンで走り、数値はライブラリのデータベースに入り、ファイルには触らない
settings-library-tempo-progress = { $total } 件中 { $done } 件のテンポを解析中
settings-library-tempo-progress-start = 何が未処理か調べています...
settings-library-tempo-status-measured = スキャン済み { $total } 曲すべてにテンポがあり、うち { $measured } 曲は rox が割り出しました
settings-library-tempo-status-tagged = スキャン済み { $total } 曲すべてにテンポのタグがあります
settings-library-watch-folders = フォルダーを監視
    .description = 追加・編集・削除されたファイルを、手動の再スキャンなしでその場でライブラリに取り込む
settings-library-write-stored = 保存済みの内容をファイルに書き込む
    .description = 3 つの保存先設定は次回の書き込みからしか効かないので、タグに切り替える前に保存したものは rox の中だけにある。これは rox が既に持っている歌詞・ゲイン・記述をファイル自体に書き込み、そのフォルダーを読む他のプレイヤーにも見えるようにする。再計算はしない

## Settings: MCP
settings-mcp-client-config = クライアント設定
    .description = MCP クライアント (Claude Code、Claude Desktop、その他何でも) のサーバー一覧に貼り付けると、ライブラリ・再生中の曲・再生操作について rox に問い合わせられる。rox が起動している必要があり、ツールは制御ソケット経由で動く
settings-mcp-enable = MCP サーバーを有効にする
    .description = 接続した MCP クライアントからのツール呼び出しに応じる。プロキシは呼び出しごとにこれを見るので、オフのあいだクライアントは理由付きで拒否される。下の設定はどちらの状態でも用意できる

## Settings: ML models
settings-mlmodels-checking = 確認中...
settings-mlmodels-choose-file = ファイルを選択
settings-mlmodels-custom-description-empty = 自前の PANNs CNN10 チェックポイントを safetensors 形式で rox に指定してください。その場で読み込まれ、ハッシュで名前が付くので、二つ目のチェックポイントは一つ目の座標に混ざらず別々にライブラリを記述します
settings-mlmodels-download-failed = { $label } をダウンロードできませんでした: { $reason }
settings-mlmodels-downloading = { $label } をダウンロード中: { $done } / { $total }
settings-mlmodels-stopping = { $label } のダウンロードを停止中...
settings-mlmodels-fallback-model = モデル
settings-mlmodels-fallback-the-model = このモデル
settings-mlmodels-kind-custom = カスタム
settings-mlmodels-kind-recommended = 推奨
settings-mlmodels-pass-stopped = 直近の処理が止まりました: { $reason }
settings-mlmodels-weights-file = 重みファイル

## Settings: playback
settings-playback-continuation-continue = 継続
    .description = 再生を始めたリストの続きを流し、そのあとはライブラリの残りへ。ビューの途中からアルバムを再生しても、そのビューが続く
settings-playback-continuation-off = オフ
    .description = キューは補充されず、最後で再生が止まる
settings-playback-continuation-weighted = 重み付き
    .description = ライブラリ全体から引く。一度も再生していないものが先、最近聴いたものが後
settings-playback-keep-playing = 再生を続ける
    .description = キューが尽きたときに何を鳴らすか。選ばれたものは隠れた状態ではなく普通のコンテキストとしてタイムラインに足されるので、見えるし外せる。上の順序が「似た曲」なら、ここで何を選んでも再生中の曲に似た曲を探し続ける
    .keywords = 継続 補充 自動再生 キュー autoplay queue
settings-playback-play-order = 再生順
    .description = シャッフルがオンのあいだ、既にキューにある曲をどう並べるか。オンオフは再生バーのシャッフルボタンで、これはオンのときの中身
settings-playback-rating-scale = レーティングの段階
    .description = さっと付けるなら星、細かく採点するなら 0-10 の 0.5 刻み
settings-playback-rating-scale-numeric = 0-10
settings-playback-rating-scale-stars = 星
settings-playback-restore-last-session = 前回のセッションを復元
    .description = 終了時のままの再生キューで起動し、再生していた曲のその位置で一時停止する。ライブラリのフォルダーの外にあるキューの曲は復元できず、順序から外れる
settings-playback-section-queue = キュー
settings-playback-section-ratings = レーティング
settings-playback-section-startup = 起動
settings-playback-shuffle-random = ランダム
    .description = 誰もがシャッフルと呼ぶあれ。この先は順不同で流れる
settings-playback-shuffle-similar = 似た曲
    .description = 音の近いものから順に。オンにした時点で再生していた曲にどれくらい似ているかで並び、スキップのたびに並べ直す。ライブラリページで記述を済ませておく必要がある
settings-playback-unrated-dots = 未評価の点
    .description = 埋まっていない星の位置を空白にせず、薄い点で示す

## Settings: providers
settings-providers-artist = Last.fm
    .description = バイオグラフィパネル用に、アーティストの略歴・統計・似たアーティストを取得し、写真は Deezer から。すべてデータフォルダーに保存され、以後はオフラインで読める
settings-providers-deezer = Deezer
    .description = カバーアートを Deezer で検索する。最大 1000 ピクセル
settings-providers-itunes = iTunes
    .description = カバーアートを iTunes で検索する。カバーエディターの検索では、設定する前に候補を選べる
settings-providers-lastfm-art = Last.fm
    .description = カバーアートを Last.fm で検索する
settings-providers-lrclib = LRCLIB
    .description = 足りない歌詞を lrclib.net から取得する。同期歌詞があればそちらを使う
settings-providers-lyrics-intro = オンラインの検索はパネルの操作から求められたときだけ走ります。再生と閲覧がネットワークに触れることはありません
settings-providers-musicbrainz = MusicBrainz
    .description = タグを musicbrainz.org で調べる。メタデータパネルの検索では、書き込む前に項目ごとに候補を確認できる
settings-providers-save-lyrics = 取得した歌詞の保存先
    .description = 取得した歌詞をどこに置くか。ライブラリを汚さない rox 自身のデータフォルダー、曲の隣の .lrc、または埋め込みタグ
settings-providers-save-lyrics-data-folder = データフォルダー
settings-providers-save-lyrics-sidecar = 隣に置く
settings-providers-save-lyrics-tag = タグ
settings-providers-section-artist = アーティスト
settings-providers-section-cover-art = カバーアート
settings-providers-section-lyrics = 歌詞
settings-providers-section-metadata = メタデータ

## Settings: shader
settings-shader-backdrop-all-windows = すべてのウィンドウ
    .description = すべてのウィンドウの背景にかける。設定、エディター、ダイアログ、切り離したパネル。オフならワークスペースのウィンドウだけ
settings-shader-backdrop-enabled = 背景シェーダー
    .description = アルバムアートの背景に、すべてのパネルの下で音に反応する WGSL シェーダーを走らせる。ワークスペースの一部なので見た目と一緒に持ち運べる
settings-shader-backdrop-fallback-name = 背景
settings-shader-backdrop-run-idle = 無音時も動かす
    .description = 何も再生していなくても描き続ける。アニメーションはどちらにせよ止まったまま
settings-shader-compile-error-title = このシェーダーはコンパイルできませんでした
settings-shader-legacy-note = 何もルーティングしていないと、プールが独自の順でスロットに流し込みます。最初のシグナルがスロット 0、二つ目がスロット 1、以下同様。ルートを一つ追加した時点で、その配分は完全にルート側に移ります。
settings-shader-overlay-enabled = オーバーレイシェーダー
    .description = ウィンドウ全体に音に反応する WGSL シェーダーを走らせる。下のアプリが使えるまま残るシェーダーだけが並ぶ
settings-shader-scene-covers-window = このシェーダーはシーンなので、ウィンドウの上に重ねるのではなく覆ってしまいます。バンドルか古い設定から来たものです。上のリストには、アプリが使えるまま残るシェーダーだけが並びます。
settings-shader-screen-all-windows = すべてのウィンドウ
    .description = 子ウィンドウにもかける。設定、統計、イコライザー、切り離したパネル。取り消しのカウントダウンはどちらにせよかからない
settings-shader-screen-fallback-name = 画面
settings-shader-screen-run-idle = 無音時も動かす
    .description = 何も再生していなくても描き続ける。アニメーションはどちらにせよ止まったまま。マウスを読むシェーダーは、これが無くても音を止めた状態でカーソルを追う。ポインターが止まってから数秒で止まるだけ
settings-shader-section-backdrop = 背景シェーダー
settings-shader-section-overlay = オーバーレイシェーダー
settings-shader-signals-block = シグナル
    .description = シェーダーの 16 個のスロットがそれぞれどの共有シグナルを読むか
settings-shader-slots-block = スロット
    .description = シェーダーに届く時点での各スロット。ルートの無いスロットは手で決めるつまみ

## Settings: storage
settings-storage-artist-images = アーティストの画像
    .description = アーティスト表示のために取得した写真・バナー・略歴 (artists/)。消しても、次にその表示を開いたときにまた取得される
settings-storage-catalog = カタログ
    .description = スキャンが作るトラックの索引。1 行が 1 トラックで、タグ・ファイル情報・CUE の区間が入る。library.db の中
settings-storage-cover-thumbnails = カバーのサムネイル
    .description = 一度描いたあと取っておく小さなカバー (thumbs.db)。消しても、スクロールして見えたときに作り直される
settings-storage-logs = ログ
    .description = 不具合報告のために各実行が書き出すもの (logs/rox.log)。サイズ上限で切り替えるので大きくならない
settings-storage-looks-layouts = 見た目とレイアウト
    .description = 今アプリが使っている見た目 (workspace.json) と、保存したワークスペース、書き出したシェーダーファイル、アイコンパック。小さく、その一バイト一バイトが自分で作ったもの
settings-storage-lyrics = 歌詞
    .description = 取得・編集した歌詞をアプリ自身の保管場所 (lyrics/) に置く。ライブラリのフォルダーは汚れない
settings-storage-measured-tempos = 測定したテンポ
    .description = タグにテンポが無い曲について rox が音から数えた値。タグ自身の数値には触れない。消すと、それらの曲はライブラリページの「未処理を解析」の対象に戻るので、改良したビート検出で古いパスが書いた数値を置き換えられる
settings-storage-model-fallback-this = このモデル
settings-storage-music-summary = { $tracks }、{ $albums }、{ $size }
settings-storage-model-weights = モデルの重み
    .description = 音響解析のためにダウンロードしたモデル (models/)。取得と削除は ML モデルのページで、1 行が 1 モデル
settings-storage-models-empty = モデル
    .description = まだ何もライブラリを記述していない。ライブラリページで音響解析をオンにするとここが埋まり、走ったモデルごとに行ができる
settings-storage-music-files = 音楽ファイル
    .description = スキャンしたフォルダーの中身。ファイルはその場から動かない
settings-storage-none = なし
settings-storage-playlists-history = プレイリストと履歴
    .description = プレイリストとその中身、再生した記録、ライブラリのジャンルのメモ。library.db の他の部分に比べればどれも小さい
settings-storage-reclaimable = 回収できる領域
    .description = 削除が library.db の中に残した空きページ。新しい書き込みがまた埋めるので、ファイルは縮む前にまず膨らむのをやめる
    .keywords = vacuum 圧縮 縮小 データベース compact shrink
settings-storage-section-acoustic = 音響的な記述
settings-storage-section-app-data = アプリのデータ
settings-storage-section-caches = キャッシュ
settings-storage-section-diagnostics = 診断
settings-storage-section-library = ライブラリ
settings-storage-section-tempo = テンポ
settings-storage-vectors = ベクトル
    .description = 各記述が library.db の中で占める量。解析パスを通したライブラリではこれがファイルの大半で、タグの数百バイトに対して 1 曲あたり数キロバイト
settings-storage-waveforms = 波形
    .description = 各曲のピーク波形。最初の再生のあと取っておく。消すと次の再生でデコードし直す

## Settings: workspace
settings-workspace-card-author = 作者
settings-workspace-card-author-placeholder = 作った人
settings-workspace-card-created = 作成 { $date }
settings-workspace-card-created-updated = 作成 { $created }、更新 { $updated }
settings-workspace-card-description = 説明
settings-workspace-card-description-placeholder = この見た目が目指しているもの
settings-workspace-card-empty = このワークスペースにはカードがありません
settings-workspace-card-hint = カードはファイルの中に保存されるので、この見た目を渡した相手にも見えます
settings-workspace-card-license = ライセンス
settings-workspace-card-license-placeholder = 配布の条件
settings-workspace-card-save = カードを保存
settings-workspace-card-updated = 更新 { $date }
settings-workspace-card-version = バージョン
settings-workspace-card-version-placeholder = 自分のバージョン。数え方は自由
settings-workspace-card-website = ウェブサイト
settings-workspace-card-website-placeholder = 置いてある場所
settings-workspace-composition-closed = ワークスペースのウィンドウは閉じられています
settings-workspace-composition-hint = ウィンドウのパネルが分割とタブグループの中でどう並んでいるか。矢印は同じ階層での並べ替え、鍵はパネルの固定、歯車は設定を開きます
settings-workspace-empty = ワークスペースがまだありません
settings-workspace-hint = ワークスペースは見た目のすべてです。レイアウト、パレット、外観。適用するとこの 3 つが入れ替わります
settings-workspace-layout-name-placeholder = レイアウト名
settings-workspace-layouts-empty = レイアウトがまだありません
settings-workspace-layouts-hint = メニューバーのミニプレイヤーボタンが行き来するのは、プライマリとミニの 2 つです
settings-workspace-name-placeholder = ワークスペース名
settings-workspace-panel-preset-unknown-kind = 不明なパネル
settings-workspace-panel-presets-empty = パネルプリセットがまだありません
settings-workspace-panel-presets-hint-after = をどのパネルメニューからでも。これらはこのワークスペース専用で、他のワークスペースにはありません。
settings-workspace-panel-presets-hint-before = 設定済みのパネル 1 つずつ。パネル自身のメニューから保存し、戻すときは
settings-workspace-role-mini = ミニ
settings-workspace-role-primary = プライマリ
settings-workspace-section-composition = 構成
settings-workspace-section-layouts = レイアウト
settings-workspace-section-panel-presets = パネルプリセット
settings-workspace-section-workspaces = ワークスペース
settings-workspace-tree-empty-slot = 空のスロット
settings-workspace-tree-split-column = 分割、上下
settings-workspace-tree-split-row = 分割、左右
settings-workspace-tree-tabs = タブ

## Settings: development
settings-development-experimental-panels = 実験的なパネル
    .description = まだ作りかけのパネルをパネルメニューとランチャーに出す。リリースごとに形が変わるが、これをオフに戻しても既に置いてあるレイアウトからは消えない
settings-development-section-features = 機能

## Settings: shared
settings-acoustic-analysis-heading = 音響解析
settings-analyze-nothing-scanned = 解析できるスキャン済みのものがまだありません
settings-common-active = 使用中
settings-common-analyze-missing = 未処理を解析
settings-common-built-in = 組み込み
settings-common-clear = クリア
settings-common-copy = コピー
settings-common-database = データベース
settings-common-delete = 削除
settings-common-download = ダウンロード
settings-common-rescan = 再スキャン
settings-common-reveal = 場所を開く
settings-common-stop = 停止
settings-common-stopping = 停止中...
settings-common-tags = タグ
settings-common-tracks-count = { $count } 曲
settings-common-use = 使う
settings-confirm-apply-body = レイアウト、パレット、外観がワークスペースのものに置き換わります。
settings-confirm-apply-imported-body = ワークスペースに保存されました。今これを適用すると、レイアウト、パレット、外観がワークスペースのものに置き換わります。
settings-confirm-clear = クリア
settings-confirm-clear-embeddings-body = 記述が消え、その分の領域が戻ります。もう一度そろえるには、ライブラリの全曲に解析パスをかけ直すことになります。
settings-confirm-clear-embeddings-title = "{ $model }" が記述した内容を消しますか?
settings-confirm-clear-measured-bpm-body = rox が割り出したテンポはすべて未測定に戻ります。ファイル自身のタグの数値は残ります。もう一度そろえるには、それらの曲にテンポのパスをかけ直すことになります。
settings-confirm-clear-measured-bpm-title = 測定したテンポを消しますか?
settings-confirm-overwrite-workspace-body = 保存済みのワークスペースが現在の状態に置き換わります。
settings-confirm-overwrite-workspace-title = ワークスペース "{ $name }" を上書きしますか?
settings-sidebar-data-folder = データフォルダー
settings-sidebar-settings-file = 設定ファイル

## Menubar
menu-about = rox について
menu-application = アプリケーション
menu-apply-layout = レイアウトを適用
menu-apply-workspace = ワークスペースを適用
menu-chat = チャット
menu-close = 閉じる
menu-console = コンソール
menu-design-mode = デザインモード
menu-discussions = ディスカッション
menu-empty-window = 空のウィンドウ
menu-equalizer = イコライザー
menu-exit = 終了
menu-hide-menubar = メニューバーを隠す
menu-import-workspace = ワークスペースをインポート...
menu-new-ellipsis = 新規...
menu-new-window = 新しいウィンドウ
menu-new-window-from-layout = レイアウトから新しいウィンドウ
menu-new-window-from-panel = パネルから新しいウィンドウ
menu-no-layouts = レイアウトなし
menu-no-presets = プリセットなし
menu-no-workspaces = ワークスペースなし
menu-os-decorations = OS のウィンドウ枠
menu-overlay-shader = オーバーレイシェーダー
menu-panel-built-in = 組み込み
menu-panel-new = 新規...
menu-panel-no-layouts = レイアウトなし
menu-panel-no-presets = プリセットなし
menu-panel-no-workspaces = ワークスペースなし
menu-panel-title = メニュー
menu-panels = パネル
menu-panels-presets = プリセット
menu-pause = 一時停止
menu-playback = 再生
menu-remain-in-tray = トレイに残す
menu-report-issue = 問題を報告
menu-save-layout = レイアウトを保存
menu-save-workspace = ワークスペースを保存
menu-section-add = 追加
menu-section-app = アプリ
menu-section-interface = インターフェース
menu-section-layouts = レイアウト
menu-section-library = ライブラリ
menu-section-session = セッション
menu-section-track = トラック
menu-section-tuning = 調整
menu-settings = 設定
menu-signals = シグナル
menu-song-theming = 曲テーマ
menu-stats = 統計
menu-tasks = タスク
menu-update-available = 更新があります
menu-welcome = ようこそ
menu-window = ウィンドウ
menu-workspace = ワークスペース
menu-workspace-builtin-tag = 組み込み

## Workspaces
workspace-apply-body = 見た目がまるごと置き換わります。レイアウト、パレット、外観。
workspace-apply-imported-body = ワークスペースに保存されました。今これを適用すると、見た目がまるごと置き換わります。レイアウト、パレット、外観。
workspace-apply-imported-title = "{ $name }" をインポートしました
workspace-apply-screen-shader-named = ウィンドウ全体に { $name } オーバーレイシェーダーがかかります。
workspace-apply-screen-shader-plain = ウィンドウ全体にオーバーレイシェーダーがかかります。
workspace-apply-shader-count = { $count ->
   *[other] シェーダー { $count } 個を含みます: { $names }
}
workspace-apply-shaders-approve-body = 承認するとこのマシンで動くようになります。承認せずに適用すると見た目は素のままで、シェーダーはプールに残ります。
workspace-apply-shaders-plain-body = 承認せずに適用すると見た目は素のままで、シェーダーはプールに残ります。
workspace-byline-author = 作者 { $author }
workspace-byline-version = バージョン { $version }
workspace-context-add-panel = パネルを追加
workspace-dialog-apply = 適用
workspace-dialog-apply-title = "{ $name }" を適用しますか?
workspace-dialog-approve-apply = 承認して適用
workspace-dialog-cancel = キャンセル
workspace-dialog-close = 閉じる
workspace-dialog-close-title = "{ $name }" を閉じますか?
workspace-dialog-export = エクスポート
workspace-dialog-layout-name-placeholder = レイアウト名
workspace-dialog-not-now = 今はしない
workspace-dialog-overwrite = 上書き
workspace-dialog-overwrite-title = "{ $name }" を上書きしますか?
workspace-dialog-save = 保存
workspace-dialog-save-layout-title = レイアウトを保存
workspace-dialog-save-workspace-title = ワークスペースを保存
workspace-dialog-with-shaders = シェーダーごと
workspace-dialog-without-shaders = シェーダーなし
workspace-dialog-workspace-name-placeholder = ワークスペース名
workspace-drop-add-queue = キューに追加
workspace-drop-play-now = すぐ再生
workspace-hint-or = または
workspace-hint-then = そのあと
workspace-import = インポート
workspace-launcher-hint = 最初のパネルを追加して組み立てを始めてください。または「ワークスペース > ワークスペースを適用」からプリセットを選んでください
workspace-launcher-need-help = 手助けが要りますか?
workspace-launcher-open-welcome = ようこそウィンドウを開く
workspace-launcher-title = 空のウィンドウ
workspace-layout-apply-body = このウィンドウの今のレイアウトが置き換わります。
workspace-layout-overwrite-body = 保存済みのレイアウトが今のものに置き換わります。
workspace-layout-preset-restore-failed = このウィンドウのレイアウトプリセットを復元できなかったので、空の状態で始まります。
workspace-layout-restore-failed = 保存済みのレイアウトを復元できなかったので、このウィンドウは空の状態で始まります。
workspace-mini-tip-back = 通常のレイアウトに戻る
workspace-mini-tip-shrink = ミニプレイヤーに縮める
workspace-overwrite-body = 保存済みのワークスペースが今の見た目に置き換わります。
workspace-panel-locked-close-body = このパネルはその場に固定されています。閉じるとレイアウトから外れます。
workspace-save-current = 現在の状態を保存
workspace-screen-shader-hint-before = オフにするのはいつでも
workspace-workspace-restore-failed = ワークスペースのレイアウトを復元できなかったので、このウィンドウは空の状態で始まります。

## Tasks window
tasks-acoustic-all-described = スキャン済み { $count } 曲すべてを { $label } が記述しています
tasks-acoustic-off = 曲の響きの記述は、設定のライブラリでオフになっています
tasks-acoustic-partial = { $label } はスキャン済み { $total } 曲のうち { $embedded } 曲を記述しています
tasks-analyzing = 解析中 { $progress }
tasks-bake-writing = タグを書き込み中...
tasks-chip-count = { $count } 件のタスク
tasks-convert-starting = ffmpeg を起動中...
tasks-converting = 変換中 { $progress }
tasks-count-of-total = { $total } 件中 { $done } 件
tasks-embedding = 埋め込み中 { $progress }
tasks-estimate-at = { $workers } で { $estimate }
tasks-import-failed = 直前のインポートに失敗しました: { $error }
tasks-import-reading = お気に入りの一覧を読み込み中...
tasks-import-unmatched = { $count } 件はこのライブラリに該当がありませんでした
tasks-importing = インポート中 { $progress }
tasks-job-acoustic = 音響解析
tasks-job-convert = 音声の変換
tasks-job-loved-import = Last.fm のお気に入り
tasks-job-replaygain = ReplayGain
tasks-job-scan = ライブラリのスキャン
tasks-job-tempo = テンポ解析
tasks-last-pass-stopped = 直近の処理が止まりました: { $reason }
tasks-last-run-finished = 直近の実行が完了、{ $count } 件
tasks-last-run-stopped = 直近の実行は { $count } 件で止まりました
tasks-library-busy = ライブラリが処理中です
tasks-library-scanning = ライブラリをスキャン中です
tasks-measuring = 測定中 { $progress }
tasks-model-downloading = モデルをまだダウンロード中です
tasks-no-library-window = ライブラリのウィンドウが開いていないので、ここからは始められません
tasks-nothing-to-measure = 測定できるスキャン済みのものがまだありません
tasks-rg-all-gain = { $count } 曲すべてに再生時のゲインがあります
tasks-rg-partial = { $total } 曲中 { $missing } 曲にゲインがありません
tasks-scan-folder-count = { $count ->
   *[other] { $count } 個のフォルダー
}
tasks-scan-last-scanned = { $folders }、最後のスキャンは { $ago } 前
tasks-scan-never-scanned = { $folders }、未スキャン
tasks-scan-no-folders = フォルダーがまだ追加されていません。設定のライブラリで追加してください
tasks-start-analyze-missing = 未処理を解析
tasks-start-measure-missing = 未測定を測る
tasks-start-rescan = 再スキャン
tasks-stop = 停止
tasks-stopping = 停止中...
tasks-tempo-all = { $count } 曲すべてにテンポがあります
tasks-tempo-off = 曲の速さの割り出しは、設定のライブラリでオフになっています
tasks-tempo-partial = { $total } 曲中 { $missing } 曲にテンポがありません
tasks-timing = 計測中 { $progress }
tasks-tip = ライブラリのタスクを開く
tasks-window-title = rox - タスク
tasks-working-out-missing = 何が未処理か調べています...

## Stats window
stats-bucket-listens = { $count ->
   *[other] { $count } 回再生、{ $ago }
}
stats-chart-start-all = 最初の再生
stats-chart-start-month = 30 日前
stats-chart-start-week = 7 日前
stats-chart-start-year = 1 年前
stats-click-opens = クリックで統計を開く
stats-click-section = クリック
stats-count-menu = 集計
    .description = 数字がどの期間の再生数を数えるか。ホバーの一覧には常に全期間が出る
stats-empty-all = 再生の記録がまだありません
stats-empty-range = この期間の再生はありません
stats-now = 現在
stats-open = 統計を開く
stats-open-on-click = クリックで統計を開く
    .description = ウィジェットをクリックすると、再生記録の全体を見る統計ウィンドウが開く
stats-play-these-tracks = これらの曲を再生
stats-play-this-track = この曲を再生
stats-plays-count = { $count ->
   *[other] { $count } 回再生
}
stats-range-all = 全期間
stats-range-all-short = 全体
stats-range-day-short = 日
stats-range-label = 期間
stats-range-month = 今月
stats-range-month-short = 月
stats-range-today = 今日
stats-range-week = 今週
stats-range-week-short = 週
stats-range-year = 今年
stats-range-year-short = 年
stats-readout-section = 表示
stats-section-listens = 再生数
stats-section-listens-over-time = 再生数の推移
stats-section-recent-listens = 最近の再生
stats-section-top-albums = よく聴くアルバム
stats-section-top-artists = よく聴くアーティスト
stats-section-top-genres = よく聴くジャンル
stats-show-change = 増減を表示
    .description = その期間が一つ前の期間と比べて増えたか減ったかのチップを添える。全期間には比べる相手がない
stats-show-number = 数字を表示
    .description = アイコンの横に件数を描く。オフならアイコンだけになり、件数はホバーで出る
stats-title = 統計ウィジェット
stats-tooltip-listens = 再生数
stats-window-title = rox - 統計

## About window
about-check-failed = GitHub に接続できませんでした
about-check-for-updates = 更新を確認
about-checking = 確認中...
about-download = ダウンロード
about-downloading = ダウンロード中... { $percent }%
about-get-it = 入手する
about-license-lead = rox は GNU AGPLv3 のもとで自由に使えるソフトウェアです。ソースは
about-notice-lead = このプログラムにはライセンスの写しが同梱されているはずです。無ければこちらを
about-release-notes = リリースノート
about-restart-now = 今すぐ再起動
about-up-to-date = 最新のバージョンです
about-update-failed = 更新に失敗しました: { $error }
about-version = バージョン { $version }
about-version-available = バージョン { $version } が利用できます
about-version-ready = バージョン { $version } の準備ができました
about-window-title = rox - バージョン情報

## Welcome window
welcome-add-folder = フォルダーを追加
welcome-and = と
welcome-back = 戻る
welcome-card-menubar-title = メニューバー
welcome-card-music-title = 音楽
welcome-card-panels-title = パネル
welcome-card-playback-title = 再生
welcome-card-rearranging-title = 並べ替え
welcome-card-settings-title = 設定
welcome-close = 閉じる
welcome-design-mode-note = 並べ替えにはデザインモードが要ります。そのメニューの一番上にあり、既定でオンです。オフにするとレイアウトが固定され、仕上げた配置がずれません。
welcome-done = 完了
welcome-drop-note = パネルの縁に落とすとそこで分割、真ん中に落とすとタブグループを共有、ウィンドウの外に落とすと独立したウィンドウになります。
welcome-key-left-click = 左クリック
welcome-key-middle-mouse = 中クリック
welcome-layout-note = 配置はレイアウトとして保存できます。ワークスペースはレイアウトとパレットをまとめて、渡せる一つの見た目にします。
welcome-menubar-after = を二度押すと出したままになります。
welcome-menubar-before = メニューバーを隠しているときは
welcome-menubar-mid = を押し続けるとドックの上に浮かび、
welcome-music-note = rox がライブラリに読み込み、ファイルはその場に残ります。フォルダーの追加は設定のライブラリから。
welcome-next = 次へ
welcome-or = または
welcome-panels-note = どの面もパネルです。メニューバーのパネルメニューから他のパネルを開けます。
welcome-playback-after = でシーク。
welcome-playback-before = で再生と一時停止、
welcome-quickplay-after = で再生。
welcome-quickplay-before = でクイック再生。曲名を入力して
welcome-rearrange-after = を押しながらパネルのどこかをドラッグすると動かせます。
welcome-rearrange-before = タブをドラッグするか、
welcome-settings-hint-after = で設定が開きます。パレット、透過、動作。
welcome-shelf-caption = 選ぶとメインウィンドウの見た目が入れ替わり、ツアーが終わります。このウィンドウは「アプリケーション > ようこそ」からいつでも開けます。
welcome-stage-lead-quick-start = ワークスペースを選ぶと、メインウィンドウがそれに切り替わります。レイアウト、パレット、見た目のすべて。
welcome-stage-lead-welcome = Foobar が 20XX 年に作られていたら。
welcome-stage-title-quick-start = クイックスタート
welcome-stage-title-welcome = rox へようこそ
welcome-step-hint-after = 、または下のボタンで。
welcome-step-hint-before = 進めるには
welcome-tile-by = 作者 { $author }
welcome-tour-intro = 音楽の入り口と見た目の置き場所をざっと見て回ります。最後は同梱ワークスペースの棚で、どれもクリック一つです。
welcome-window-title = rox - ようこそ

## Console window
console-clear = クリア
console-copy = コピー
console-empty-filtered = このレベルには何もありません
console-empty-none = まだ何も記録されていません
console-filter-error = エラー
console-filter-info = 情報
console-filter-warn = 警告
console-follow = 追従
console-line-count = { $count ->
   *[other] { $count } 行
}
console-open-button = コンソールを開く
console-reveal = 場所を開く
console-window-title = rox - コンソール

## Signals window
signals-about-toggle = シグナルについて
signals-blurb-marked = メニューでこの印が付いたパネルは、たいていのパラメーターをシグナルに結び付けられます。パネルの設定でパラメーターを右クリックしてシグナルを選ぶか、その場で追加してください。
signals-blurb-shared = ここでの調整は共有されます。変更は、そのシグナルにつながるすべてのパネル・ウィンドウのパラメーターに届きます。
signals-blurb-total = トータルは 4 つ目の種類です。別のシグナルを時間で積み上げて 1 で折り返すので、音が大きいあいだ登り、静かなあいだは止まります。時計ではなく曲に合わせて進む位相がシェーダーに要るときに使ってください。
signals-blurb-what = シグナルは、再生中の音を 0 から 1 のひとつの数字に変えます。ある周波数帯のエネルギー、ミックス全体のレベル、帯域内の打点ごとのパルスのいずれかです。応答で追従の速さを決め、しきい値で指定したレベル以下を黙らせます。
signals-no-library = ライブラリのウィンドウが開いていないので、音は出ません。編集は保存されます。
signals-window-title = rox - シグナル

## Equaliser
eq-analyzer-bars = バー
eq-analyzer-off = アナライザーなし
eq-analyzer-wave = 波形
eq-band-badge = バンドのバッジ
    .description = フラットから外れているバンドの数を、アイコンの上のバッジに出す
eq-band-label = バンド { $number }
eq-click-nothing = 何もしない
eq-click-open = 開く
eq-click-section = クリック
    .description = クリックしたときの動き。イコライザーのウィンドウを開くか、カーブ全体をそのままオンオフするか
eq-click-toggle = 切り替え
eq-flatten = フラットにする
eq-freq-label = 周波数
eq-gain-label = ゲイン
eq-heading = イコライザー
eq-help-text = バンドをドラッグすると動き、上でスクロールすると幅が変わります。処理はサウンドカードに送るバッファより手前にあるので、動かしてからスピーカーに届くまで最大 0.5 秒ほどかかります。
eq-hint-off = クリックでオフ
eq-hint-on = クリックでオン
eq-hint-open = クリックでイコライザーを開く
eq-open = イコライザーを開く
eq-readout-curve = カーブ
eq-readout-icon = アイコン
eq-readout-section = 表示
    .description = アイコン、応答カーブのスパークライン、またはその両方。カーブが読めるには幅が 50 ピクセルほど要る
eq-reset-bands = バンドをリセット
eq-shape-active = { $count ->
   *[other] { $count } バンドがフラットから外れ、ピーク { $peak } dB
}
eq-shape-flat = フラット、全バンド 0 dB
eq-status-off = イコライザー オフ
eq-status-on = イコライザー オン
eq-title = EQ ウィジェット
eq-widget-section = ウィジェット
eq-width-label = 幅
eq-window-title = rox - イコライザー

## Keymap
keymap-close-window = ウィンドウを閉じる
    .description = 手前のウィンドウを閉じる。切り離したパネルも含め、どこでも効く
keymap-decrease-font-size = 文字を小さく
    .description = アプリ全体の文字サイズを一段下げる
keymap-focus-search = 検索にフォーカス
    .description = ライブラリの検索ボックスにカーソルを置く
keymap-group-editing = 編集
keymap-group-playback = 再生
keymap-group-view = 表示
keymap-group-windows = ウィンドウ
keymap-increase-font-size = 文字を大きく
    .description = アプリ全体の文字サイズを一段上げる
keymap-key-backspace = Backspace
keymap-key-delete = Delete
keymap-key-down = ↓
keymap-key-end = End
keymap-key-esc = Esc
keymap-key-home = Home
keymap-key-insert = Insert
keymap-key-left = ←
keymap-key-page-down = Page Down
keymap-key-page-up = Page Up
keymap-key-right = →
keymap-key-space = Space
keymap-key-tab = Tab
keymap-key-up = ↑
keymap-mod-alt = Alt
keymap-mod-cmd = Cmd
keymap-mod-ctrl = Ctrl
keymap-mod-fn = Fn
keymap-mod-option = Option
keymap-mod-shift = Shift
keymap-mod-super = Super
keymap-mod-win = Win
keymap-open-quick-play = クイック再生
    .description = 検索してすぐ再生するプロンプトをウィンドウの上に出す
keymap-open-settings = 設定を開く
    .description = このウィンドウを開く
keymap-open-stats = 統計を開く
    .description = 再生統計のウィンドウを開く
keymap-quit = 終了
    .description = rox を終了する。どのウィンドウからでも効くべきなので、どこでも割り当てられている
keymap-reset-font-size = 文字サイズをリセット
    .description = 文字サイズを既定に戻す
keymap-seek-backward = 巻き戻し
    .description = 再生中の曲を少し戻す
keymap-seek-forward = 早送り
    .description = 再生中の曲を少し進める
keymap-stamp-line = 歌詞に時刻を打つ
    .description = 編集中の歌詞の行に、再生位置を書き込む
keymap-toggle-playback = 再生 / 一時停止
    .description = 今の曲を再生するか、その場で一時停止する
keymap-toggle-post-shader = オーバーレイシェーダーを切り替え
    .description = 画面シェーダーをオンオフする。シェーダーがオフにするためのコントロールごと覆い隠すことがあるので、どこでも効くように割り当ててある
keymap-toggle-zoom = パネルグループを最大化
    .description = 最後にクリックしたパネルグループでドックを埋める、または元に戻す

## Panel catalog
panel-catalog-album-carousel = アルバムカルーセル
panel-catalog-artist-grid = アーティストグリッド
panel-catalog-biography = バイオグラフィ
panel-catalog-cover-art = カバーアート
panel-catalog-drawer = ドロワー
panel-catalog-eq-widget = EQ ウィジェット
panel-catalog-filter = フィルター
panel-catalog-folder-tree = フォルダーツリー
panel-catalog-genre-grid = ジャンルグリッド
panel-catalog-group-application = アプリケーション
panel-catalog-group-arrangement = 配置
panel-catalog-group-catalogue = カタログ
panel-catalog-group-controls = 操作
panel-catalog-group-details = 詳細
panel-catalog-group-experimental = 実験的
panel-catalog-group-visualizers = ビジュアライザー
panel-catalog-history = 履歴
panel-catalog-menu = メニュー
panel-catalog-metadata = メタデータ
panel-catalog-mini-toggle = ミニ切り替え
panel-catalog-oscilloscope = オシロスコープ
panel-catalog-overlay = オーバーレイ
panel-catalog-particles = パーティクル
panel-catalog-playlists = プレイリスト
panel-catalog-queue = キュー
panel-catalog-queue-widget = キューウィジェット
panel-catalog-seek = シーク
panel-catalog-slide = スライド
panel-catalog-spectrogram = スペクトログラム
panel-catalog-spectrum = スペクトラム
panel-catalog-stats-widget = 統計ウィジェット
panel-catalog-status = ステータス
panel-catalog-theme-toggle = テーマ切り替え
panel-catalog-track-info = 曲情報
panel-catalog-vu-meter = VU メーター
panel-catalog-waveform = 波形
panel-catalog-window-controls = ウィンドウ操作

## Updater
updater-already-latest = 既に最新のバージョンです
updater-checksum-mismatch = ダウンロードのチェックサムは { $digest } で、リリースが示す { $expected } と違います
updater-checksum-missing-entry = { $sums } に { $name } の項目がありません。検証できないダウンロードは拒否します
updater-no-asset = このリリースには { $name } がありません
updater-no-checksums = このリリースには { $sums } がありません。検証できないダウンロードは拒否します
updater-no-release-build = このプラットフォーム向けのリリースビルドがありません
updater-overran = ダウンロードがリリースの示すサイズを超えました
updater-short = ダウンロードが { $bytes } バイト中 { $done } バイトで止まりました
updater-size-mismatch = サーバーの提示は { $claimed } バイト、リリースの記載は { $bytes } バイトです

## Last.fm
lastfm-import-matching = ライブラリと照合中
lastfm-import-read = お気に入りの曲を { $count } 件読み込みました
lastfm-import-stopped = お気に入りの曲 { $count } 件で止まりました
lastfm-import-matched = 、{ $count } 件が一致
lastfm-import-added = 、{ $count } 件をお気に入りに追加

## Tag tools
tags-editor-clear-all = すべて消去
tags-editor-form-view = フォーム
tags-editor-format-unsupported-all = この形式のタグは、まだ読み書きできません。
tags-editor-format-unsupported-some = これらのファイルの一部は、まだタグを読み書きできない形式です。
tags-editor-guess-button = 推測
tags-editor-guess-folded = { $status }、他 { $count } 件は非表示
tags-editor-guess-help = { $placeholders }。/ は一つ上のフォルダーに対応し、%skip% は読み捨てる
tags-editor-guess-match-count = { $total } 件中 { $hits } 件が一致
tags-editor-guess-no-match = 一致なし
tags-editor-guess-pattern-label = パターン
tags-editor-loading = タグを読み込み中...
tags-editor-look-up = 検索
tags-editor-multiple-values = 複数の値
tags-editor-clear-on-save = 保存すると消去されます
tags-editor-other-tags = その他のタグ ({ $count })
tags-editor-remove = 削除
tags-editor-reveal = 場所を開く
tags-editor-save-errors = { $count } 件のファイルが失敗しました。{ $error }
tags-editor-saving-progress = 保存中 { $done }/{ $total }...
tags-editor-table-view = テーブル
tags-editor-tags-section = タグ
tags-editor-unknown-partial = { $total } 件中 { $count } 件
tags-editor-unread-count = { $total } 件中 { $failed } 件のファイルのタグを読めませんでした
tags-editor-will-clear = 消去されます
tags-editor-will-remove = 削除されます
tags-editor-window-title = rox - タグエディター
tags-guess-empty-segment = パターンの結果でフォルダー名かファイル名が空になります
tags-guess-no-placeholders = プレースホルダーがありません
tags-guess-skip-renders-nothing = %skip% に読み捨てるものがありません
tags-guess-unclosed = % が閉じていません
tags-guess-unknown-placeholder = 不明なプレースホルダー %{ $name }%
tags-matcher-blocked-arm = 適用する項目を有効にしてください
tags-matcher-blocked-no-match = 適用できる候補がありません
tags-matcher-blocked-pick = 候補を選んでください
tags-matcher-blocked-writing = タグを書き込み中...
tags-matcher-match-count = { $count ->
   *[other] { $count } 件の候補
}
tags-matcher-no-matches = 候補が見つかりません
tags-matcher-pick-match = 候補を選ぶ
tags-matcher-search-failed = 検索に失敗しました: { $error }
tags-matcher-searching = 検索中...
tags-matcher-tagging = { $track } にタグを書き込み中
tags-matcher-window-title = rox - メタデータを探す
tags-rename-blocked-cue = CUE のトラックで、専用のファイルがありません
tags-rename-blocked-duplicate = 2 曲が同じ名前になります
tags-rename-blocked-occupied = そこには既にファイルがあります
tags-rename-blocked-outside-roots = どのライブラリのルートにも入っていません
tags-rename-blocked-unresolved = まだカタログにありません
tags-rename-move-error = { $name }: { $error }
tags-rename-move-errors = { $count } 件のファイルが失敗しました。{ $error }
tags-rename-moving = 移動中 { $done }/{ $total }...
tags-rename-nothing-to-move = 移動するものがありません
tags-rename-pattern-help = { $placeholders }。/ はフォルダーを作り、拡張子はファイルのものが付く
tags-rename-pattern-section = パターン
tags-rename-preview-section = プレビュー
tags-rename-unchanged = 変更なし
tags-rename-will-move = { $total } 件中 { $count } 件が移動します
tags-rename-window-title = rox - ファイル名の変更
tags-repair-affected-files = 対象のファイル
tags-repair-section = 修復
tags-repair-check-to-repair = 修復するファイルにチェックを入れてください
tags-repair-count = { $count ->
   *[other] { $count } 件のファイル
}
tags-repair-count-so-far = ここまで { $count } 件
tags-repair-label-scope = 範囲
tags-repair-no-affected = 対象のファイルは見つかりませんでした。
tags-repair-no-folder = スキャンするフォルダーがありません。ライブラリに追加するか、選んでください。
tags-repair-pick-folder = フォルダーを選ぶ...
tags-repair-progress = 修復中 { $done }/{ $total }...
tags-repair-repair-button = { $count ->
    [0] 修復
   *[other] 修復 ({ $count })
}
tags-repair-result = { $count ->
   *[other] { $count } 件のファイルを修復しました
}
tags-repair-result-failed = { $count } 件を修復、{ $failed } 件が失敗
tags-repair-scan-first = 先にスキャン
tags-repair-scan-hint = 書き直せば直るタグの破損があるファイルを、スキャンして探します。
tags-repair-select-all = すべて選択
tags-repair-select-none = 選択を解除
tags-repair-whole-library = ライブラリ全体
tags-repair-window-title = rox - タグの修復

## Convert
convert-arg-names-file = "{ $token }" はファイルを指しています。出力先はフォルダーとパターンから決まります
convert-section-output = 出力
convert-section-preview = プレビュー
convert-arg-not-flag-or-value = "{ $token }" はフラグでも、その値でもありません
convert-check-wrote-nothing = ffmpeg は正常終了しましたが、何も書き出しませんでした
convert-custom-ext-empty = コンテナを決めるのは拡張子なので、指定が要ります
convert-custom-ext-invalid = "{ $ext }" はコンテナ名ではありません。英数字のみ、ドットなし
convert-dialog-browse = 参照...
convert-dialog-check-passed = ffmpeg がこの設定で無音を一瞬エンコードできたので、動きます
convert-dialog-check-waiting = 入力が止まると ffmpeg で確認します
convert-dialog-checking = ffmpeg で確認中...
convert-dialog-choose-folder = 書き出し先のフォルダーを選ぶ
convert-dialog-convert-button = 変換
convert-dialog-custom-label = カスタム
convert-dialog-custom-menu-item = カスタム...
convert-dialog-custom-note = 引数はスペースで区切られるので引用符は使えません。カスタム形式では埋め込みアートは引き継がれません
convert-dialog-format-not-ready = 入力した形式は、まだ ffmpeg の確認を通っていません
convert-dialog-label-extension = 拡張子
convert-dialog-label-format = 形式
convert-dialog-label-into = 出力先
convert-dialog-label-named = ファイル名
convert-dialog-mirror = ライブラリのフォルダー構成をそのまま作る
convert-dialog-nothing-to-convert = 変換するものがありません。すべての行がスキップされます
convert-dialog-pattern-help = { $placeholders }。/ はフォルダーを作り、拡張子は形式が決める
convert-dialog-pick-folder = 書き出し先のフォルダーを選んでください
convert-dialog-span-note = { $count } 件は CUE イメージから切り出し、ライブラリのタグを付けます
convert-dialog-will-convert = { $total } 件中 { $count } 件が変換されます
convert-dialog-window-title = rox - 変換
convert-ffmpeg-silent-failure = ffmpeg が理由を告げずに失敗しました
convert-flag-attach = -attach は別のファイルを読むもので、ここでは使えません
convert-flag-f = コンテナは拡張子が決めるので、-f は指定できません
convert-flag-i = 入力は選んだ曲なので、-i は指定できません
convert-flag-n = -n は毎回付いています
convert-flag-y = 何も上書きしないので -y は使えません。既にある出力先はスキップされます
convert-preset-flac = FLAC
convert-preset-mp3-320 = MP3 320 kbps
convert-preset-mp3-v0 = MP3 V0
convert-preset-opus-192 = Opus 192 kbps
convert-preset-wav = WAV
convert-skip-duplicate = 2 曲が同じ名前になります
convert-skip-exists = 既にあります
convert-summary-failed = 、{ $count } 件が失敗
convert-summary-files = { $count ->
   *[other] { $count } 件のファイル
}
convert-summary-line = { $files } を { $dest } へ
convert-summary-skipped = 、{ $count } 件をスキップ
convert-summary-stopped = { $files } を { $dest } へ書き出したところで停止
convert-version-answered = { $binary } は動きましたが、バージョンを返しませんでした

## Duplicates
duplicates-auto-select = 自動選択
duplicates-check-to-trash = ゴミ箱に入れるコピーにチェックを入れてください
duplicates-copy-count = { $count ->
   *[other] { $count } 個のコピー
}
duplicates-different-albums = 別のアルバム
duplicates-filter-placeholder = タイトル、アーティスト、フォルダーで絞る
duplicates-groups-summary = { $groups ->
   *[other] { $groups } グループ、余分なコピー { $extras } 件
}
duplicates-library-loading = ライブラリをまだ読み込み中です。少ししてからもう一度お試しください。
duplicates-no-duplicates = 重複は見つかりませんでした。
duplicates-no-filter-matches = 条件に合うグループがありません。
duplicates-policy-newest = 新しいものを残す
duplicates-policy-oldest = 古いものを残す
duplicates-policy-quality = 音質の良いものを残す
duplicates-scan-hint = 複数回現れる曲をライブラリからスキャンして探します。
duplicates-select-none = 選択を解除
duplicates-selected-count = { $count } 件を選択中
duplicates-trash-button = { $count ->
    [0] ゴミ箱へ
   *[other] ゴミ箱へ ({ $count })
}
duplicates-trash-error = { $name }: { $error }
duplicates-trash-result = { $count ->
   *[other] { $count } 件のファイルをゴミ箱へ移動しました
}
duplicates-trash-result-failed = { $count } 件をゴミ箱へ移動、{ $failed } 件が失敗
duplicates-trashing = ゴミ箱へ移動中 { $done }/{ $total }...
duplicates-window-title = rox - 重複

## Smart playlists
smart-playlist-descending = 降順
smart-playlist-edit-title = スマートプレイリストを編集
smart-playlist-limit-label = 上限
smart-playlist-limit-placeholder = 上限なし
smart-playlist-match-count = { $count ->
   *[other] { $count } 曲が一致
}
smart-playlist-matched-tracks = 一致した曲
smart-playlist-new-title = 新しいスマートプレイリスト
smart-playlist-no-matches = 一致する曲がありません
smart-playlist-query-label = クエリ
smart-playlist-sort-default = 既定の順
smart-playlist-sort-added = 追加日
smart-playlist-sort-label = 並べ替え
smart-playlist-unknown-field = "{ $field }:" は項目名ではないので、この語は普通のテキストとして扱われます
smart-playlist-window-title = rox - { $verb }

## Playlist creation
playlist-create-not-savable = 保存するにはプレイリストに名前を付けてください
playlist-create-placeholder = プレイリスト名
playlist-create-rename-title = プレイリストの名前を変更
playlist-create-title = 新しいプレイリスト
playlist-create-window-title = rox - { $verb }

## Cover tools
cover-art-back = 裏
cover-art-disc = 盤面
cover-art-front = 表
cover-artwork = アートワーク
    .description = どの画像を出すか。ファイルにその枠が無ければ表ジャケットを出す
cover-disc-style = ディスクの見せ方
    .description = アートワークを CD、またはレコードのレーベル面に見立てる
cover-disc-off = オフ
cover-disc-cd = CD
cover-disc-vinyl = レコード
cover-editor-choose-image = 画像を選ぶ
cover-editor-multiple = 複数
cover-editor-none = なし
cover-editor-not-an-image = そのファイルは rox が埋め込める画像ではありません
cover-editor-not-decoded = その画像はデコードできませんでした
cover-editor-reading = 現在のアートを読み込み中...
cover-editor-remove = 削除
cover-editor-replace = 差し替え
cover-editor-revert = 元に戻す
cover-editor-save-errors = { $count } 件のファイルが失敗しました。{ $error }
cover-editor-saving-progress = 保存中 { $done }/{ $total }...
cover-editor-search-online = オンラインで検索
cover-editor-section = カバーアート
cover-editor-slot-back = 裏ジャケット
cover-editor-slot-front = 表ジャケット
cover-editor-slot-media = 盤面
cover-editor-will-remove = 削除されます
cover-editor-window-title = rox - カバーアート
cover-matcher-blocked-fetching = 元の画像を取得中...
cover-matcher-blocked-no-cover = 設定できるカバーがありません
cover-matcher-blocked-pick = 設定するカバーを選んでください
cover-matcher-cover-count = { $count ->
   *[other] { $count } 件のカバー
}
cover-matcher-editor-closed = カバーエディターは閉じられました
cover-matcher-no-covers = カバーが見つかりません
cover-matcher-search-failed = 検索に失敗しました: { $error }
cover-matcher-set-cover = カバーに設定
cover-matcher-setting = 設定中...
cover-matcher-tile-info = { $provider }  { $width }px
cover-matcher-unsupported-format = 対応していない画像形式です
cover-matcher-window-title = rox - カバーアートを探す
cover-spin = 回転
    .description = 再生中にディスクを回す。盤面の枠、またはディスクの見せ方に効く
cover-spin-disc = ディスクを回す
cover-spin-ramp = 回転の立ち上がり
    .description = ディスクが全速に達するまで、そして惰性で止まるまでの時間
cover-spin-speed = 回転速度
    .description = 全速時の毎分回転数
cover-stretch = 引き伸ばし
    .description = アートワークの縦横比を無視してパネルを埋める
cover-stretch-to-fill = 引き伸ばして埋める
cover-title = カバーアート

## Lyrics
lyrics-always-centered = 常に中央に
    .description = 前後に余白を足し、最初と最後の行も中央に来られるようにする
lyrics-auto-search = 自動検索
    .description = 歌詞の無い曲でオンラインを検索し、確度の高い候補があれば選択画面を出さずに保存する
lyrics-bold = 太字
lyrics-build-word-by-word = 一語ずつ表示
    .description = 歌われるのに合わせて単語を出すカラオケ風の表示。まだ歌われていない行は隠れて待つ
lyrics-edge-bottom = 下
lyrics-edge-top = 上
lyrics-edit-hint-after-stamp = で時刻を打つ
lyrics-edit-hint-or = または
lyrics-edit-loading = 歌詞を読み込み中...
lyrics-edit-lyrics = 歌詞を編集
lyrics-edit-saving = 保存中...
lyrics-edit-section = 歌詞
lyrics-edit-stamp = 時刻を打つ
lyrics-edit-stamp-time = { $time } を打つ
lyrics-edit-window-title = rox - 歌詞の編集
lyrics-fade-lines-in = 行をフェードイン
    .description = 行が現在行になるとき、暗いところからフェードで上げる
lyrics-falloff-edge = 減衰する側
    .description = 現在行のどちら側を減衰で暗くするか
lyrics-find-online = オンラインで歌詞を探す...
lyrics-follow-playback = 再生に追従
    .description = 同期歌詞の再生に合わせて、現在行を中央へなめらかに送る
lyrics-font = フォント
    .description = 歌詞の書体。既定はアプリのフォントに従う
lyrics-gap-threshold = 間奏のしきい値
    .description = イントロや間奏がどれくらい続いたら休符を出すか
lyrics-lead-in-rest = イントロの休符
    .description = 長いイントロの前に空の休符を置き、最初の行が来たときにフェードインさせる
lyrics-line-falloff = 行の減衰
    .description = 現在行から 1 行離れるごとに、どれくらい暗くするか
lyrics-line-spacing = 行間
    .description = 同期歌詞の行どうしの間隔。文字サイズに対する倍率
lyrics-look-again = もう一度探す
lyrics-mark-dots = 点
lyrics-mark-note = 音符
lyrics-marked-notice = 歌詞なしに設定済み
lyrics-matcher-blocked-no-match = 適用できる候補がありません
lyrics-matcher-blocked-pick = 適用する候補を選んでください
lyrics-matcher-blocked-saving = 歌詞を保存中...
lyrics-matcher-match-count = { $count ->
   *[other] { $count } 件の候補
}
lyrics-matcher-no-query = この曲には照合できるアーティストとタイトルがありません
lyrics-matcher-pick-preview = プレビューする候補を選んでください
lyrics-matcher-search-failed = 検索に失敗しました: { $error }
lyrics-matcher-synced-tag = { $provider }  同期あり
lyrics-matcher-window-title = rox - 歌詞を探す
lyrics-no-lyrics-notice = 歌詞なし
lyrics-no-lyrics-track = この曲には歌詞がありません
lyrics-rest-in-gaps = 間奏で休符
    .description = 長い間奏では最後の行を保持せず、空の休符に移る
lyrics-rest-marker = 休符の印
    .description = 同期歌詞で歌のない行に何を出すか。間奏と空行に使われる
lyrics-search-button = オンライン検索ボタン
    .description = 歌詞が無いときの画面に検索ボタンを出す。右クリックメニューからも探せる
lyrics-search-online = オンラインで検索
lyrics-show-song-name = 曲名を表示
    .description = 歌詞が無いときの画面で、「歌詞なし」の上に曲名を出す
lyrics-text-size = 文字サイズ
    .description = 歌詞の文字。同期表示の行の高さもこれに従う
lyrics-title = 歌詞
lyrics-title-unsynced = 非同期時のタイトル
    .description = 非同期の歌詞の上に曲名を固定し、パネルが低くても見えるようにする
lyrics-wipe-lyrics = 歌詞を消す

## Analysis passes
pass-acoustic-body = { $model } が各曲の響きを割り出し、再生中の曲に似た音楽をライブラリから探せるようにします。処理はすべてこのマシンで走り、既に記述済みのものは飛ばします。{ $lands }
pass-acoustic-lands-database = 結果はライブラリのデータベースに入り、ファイルには触れません。
pass-acoustic-lands-tags = 結果はライブラリのデータベースに入り、MP3 と FLAC については各ファイルのタグにも入るので、データベースを作り直しても残ります。他の形式はデータベースの写しだけになります。
pass-acoustic-title = { $count ->
   *[other] { $count } 曲を解析しますか?
}
pass-analyze = 解析
pass-estimate-at = { $workers_phrase } で { $estimate }。
pass-estimate-button = 見積もる
pass-estimating = 見積もり中...
pass-measure = 測定
pass-no-estimate = このマシンではまだ何も走っていないので、見積もりがありません。「見積もる」が数曲を計って、そこから残りを割り出します。
pass-replaygain-body = 各ファイルをデコードして計測し、マスタリングされた音量で鳴らせるようにします。全曲にゲインが無いアルバムは、盤ごとまとめて測ります。{ $lands }
pass-replaygain-lands-database = 数値はライブラリのデータベースに入り、ファイルには触れません。
pass-replaygain-lands-tags = 数値は各ファイルのタグに書き戻され、他のプレイヤーもそこから読みます。
pass-replaygain-title = { $count ->
   *[other] { $count } 曲を測定しますか?
}
pass-tempo-body = 各ファイルから 30 秒の区間を 2 つデコードしてビートを数え、曲の速さをライブラリに出せるようにします。クリックに合わせて録音された音楽で最もよく働き、測れないものは飛ばします。数値はライブラリのデータベースに入り、ファイルには触れません。
pass-tempo-title = { $count ->
   *[other] { $count } 曲のテンポを調べますか?
}
pass-timing = 数曲を計測中...
pass-timing-failed = このライブラリを計測できませんでした: { $error }
pass-workers = 並列数

## Quick play
quick-play-comfortable-rows = ゆったりした行
    .description = 各結果の高さを広げる
quick-play-cover = カバー
    .description = 各結果の左にカバーのサムネイルを出す
quick-play-duration = 長さ
    .description = 各結果の右に長さを出す
quick-play-narrow-by = 絞り込み
quick-play-search-placeholder = ライブラリを検索
quick-play-subtitle = サブタイトル
    .description = 各結果の下にアーティストとアルバムを出す
quick-play-tag-album = アルバム
quick-play-tag-artist = アーティスト

## Drawer panel
drawer-add-tooltip = ドロワーパネルを追加
drawer-answers = 反応する範囲
    .description = どの選択でドロワーが開くか。自分のメインパネルだけか、外のどのパネルでも
drawer-dim = 減光
    .description = ドロワーが開いているとき、後ろのメインパネルをどれくらい暗くするか
drawer-edge = 辺
    .description = ドロワーが収まり、そこから滑り出す辺
drawer-edge-bottom = 下
drawer-edge-top = 上
drawer-handle = ハンドル
    .description = パネルの縁のつまみを表示する。隠すと選択があるまでドロワーは見えず、そのあとは選択が続くあいだつまみが残るので、閉じたドロワーをまた引き出せる
drawer-open-on = 開く条件
    .description = ハンドルの上に留まれば常に開く。選択を足すと、メインパネルでの選択でも開く
drawer-pin-open = 開いたまま固定
drawer-reveal = 出す量
    .description = 開いたドロワーがパネルをどれくらい覆うか
drawer-scope-elsewhere = 外のパネル
drawer-scope-main = メインパネル
drawer-title = ドロワー
drawer-trigger-hover = ホバー
drawer-trigger-selection = 選択

## Mini player
mini-tip-back = 通常のレイアウトに戻る
mini-tip-none = ミニレイアウトが割り当てられていません
mini-tip-shrink = ミニプレイヤーに縮める
mini-title = ミニ切り替え

## System tray
tray-open = 開く
tray-pause = 一時停止
tray-play = 再生
tray-quit = 終了

## Window controls
window-controls-mini-toggle = ミニ切り替え
    .description = ミニレイアウトの切り替えを先頭に置く。ミニレイアウトが割り当てられると出る
window-controls-minimize = 最小化
window-controls-style = スタイル
    .description = フラットなアイコン、または macOS のトラフィックライト
window-controls-style-icons = アイコン
window-controls-title = ウィンドウ操作
window-controls-traffic-lights = トラフィックライト

## Section names the audio panels share. Each one heads the same kind of
## rows in the spectrum, VU, oscilloscope, spectrogram and particles
## settings, so they're defined once rather than per panel.
viz-section-analysis = 解析
viz-section-color = 色
viz-section-peaks = ピーク
viz-section-playback = 再生
viz-section-scale = 目盛り
viz-section-signal = 信号

## Particles panel
particles-add-emitter = エミッターを追加
particles-aim = 向き
particles-aim-fixed = 固定
particles-aim-outward = 外向き
particles-burst = バースト
particles-color = 色
particles-cone = 広がり
particles-direction = 方向
    .description = どちらへ引くか。0 が上、180 が下
particles-drag = 抵抗
    .description = 空気が毎秒どれくらい速度を食うか。0 は真空
particles-drift = 流れ
    .description = 場そのものが動く速さ。渦がその場に留まらないように
particles-edit-emitters = エミッターを編集
particles-emitter-label = エミッター { $index }
particles-emitter-target = エミッター { $index } { $target }
particles-emitters-empty = エミッターがまだありません。追加すると場が動き出します。
particles-glow = グロー
    .description = 各パーティクルの後ろに柔らかい光を敷く
particles-gravity = 重力
particles-gravity-strength = 強さ
    .description = 飛んでいるものすべてにかかる一定の引き
particles-height = 高さ
particles-hold-on-pause = 一時停止で止める
    .description = 一時停止中は場を凍結し、流れて消えないようにする
particles-length = 長さ
particles-lifetime = 寿命
particles-position-x = 位置 X
particles-position-y = 位置 Y
particles-radius = 半径
particles-rate = レート
particles-rotation = 回転
particles-round-particles = 丸いパーティクル
    .description = 四角ではなく点で描く
particles-scale = スケール
    .description = 渦一つの大きさ。小さいと細かく波立ち、大きいとゆったり転がる
particles-section-emitters = エミッター
particles-section-medium = 媒質
particles-section-particles = パーティクル
particles-shape = 形
particles-shape-box = 四角
particles-shape-line = 線
particles-shape-point = 点
particles-shape-ring = 輪
particles-size = サイズ
particles-speed = 速さ
particles-trigger = トリガー
particles-trigger-continuous = 連続
particles-turbulence = 乱流
particles-turbulence-drift = 乱流の流れ
particles-turbulence-scale = 乱流のスケール
particles-turbulence-strength = 強さ
    .description = 場がパーティクルをどれくらい強く押しやるか。0 でオフ
particles-width = 幅

## Spectrum panel
spectrum-axis-labels = 軸のラベル
    .description = パネル上に範囲の目盛りを出す。オクターブ (C1, C2, ...) か周波数 (100, 1k, 10k)
spectrum-bar-gap = バーの間隔
    .description = バーどうしの隙間。広げるほど本数は減る
spectrum-bar-width = バーの太さ
    .description = 各バーの太さ。細いほど多くの帯域が入る
spectrum-block-gap = ブロックの間隔
    .description = 積み上げたセルどうしの継ぎ目
spectrum-block-height = ブロックの高さ
    .description = 積み上げた各セルの高さ
spectrum-cap-gravity = ピークの落下
    .description = 帯域が下がったあと、ピークの印がどれくらい速く落ちるか
spectrum-fft-size = FFT サイズ
    .description = 解析窓。短いと反応が速く、長いと分解能が上がる
spectrum-gradient-base-color = ベースの色
    .description = カスタムのグラデーションの静かな側
spectrum-gradient-cover = カバー
spectrum-gradient-mode = グラデーション
    .description = 音量で帯域に色を付ける。テーマのグラデーション、曲テーマ時はカバーアートの色、またはカスタムの 2 色
spectrum-gradient-theme = テーマ
spectrum-gradient-tip-color = 先端の色
    .description = カスタムのグラデーションの大きい側
spectrum-high-bound-description = バーが解析する最も高い周波数
spectrum-high-fft-size = 上側の FFT サイズ
    .description = 分割点より上の帯域の解析窓
spectrum-hold-on-pause = 一時停止で止める
    .description = 一時停止中はバーを凍結し、無音まで落とさない
spectrum-labels-frequency = 周波数
spectrum-labels-pitch = 音名
spectrum-low-bound-description = バーが解析する最も低い周波数
spectrum-orientation = 向き
    .description = 帯域が伸びる元になる辺
spectrum-outline-bars = バーを輪郭で描く
    .description = 各バーを塗りつぶさず、中抜きの輪郭で描く
spectrum-outline-width = 輪郭の太さ
    .description = 中抜きバーの線の太さ
spectrum-peak-caps = ピークの印
    .description = 各帯域の直近のピークに印を残す
spectrum-section-bands = バンド
spectrum-split-at = 分割点
    .description = 二つのゾーンが接する位置。最寄りのバーに吸着する
spectrum-split-zones = ゾーンを分割
    .description = ある周波数の上と下を、別々の窓サイズで解析する
spectrum-style = スタイル
    .description = 定番のバー、LED 風のブロック、または一本の線
spectrum-style-bars = バー
spectrum-style-blocks = ブロック
spectrum-style-line = ライン
spectrum-symmetry = 対称
    .description = スペクトラムを中心で折り返す。順方向は低域を両端に、逆方向は中央で合わせる
spectrum-symmetry-forward = 順方向
spectrum-symmetry-reverse = 逆方向

## Waveform panel
waveform-bar-gap = バーの間隔
    .description = バーどうしの隙間。0 でひと続きの形になる
waveform-bar-width = バーの太さ
    .description = 各バーの太さ
waveform-outline = 輪郭
    .description = バーを塗らずに輪郭でなぞる。隙間 0 なら一つの形として見える
waveform-scrobble-marker = スクロブルの印
    .description = 曲が Last.fm にスクロブルされたとみなされる位置の細い線
waveform-split-channels = チャンネルを分ける
    .description = 1 チャンネルにつき 1 段、左が上で右が下。モノラルの曲は 1 段のまま
waveform-unavailable = この曲の波形は利用できません

## VU panel
vu-ballistics = 弾道特性
    .description = VU は音量をゆっくり積分する。ピークは素早く上がり、ゆるやかに戻る
vu-ballistics-peak = ピーク
vu-cap-gravity = ピークの落下
    .description = メーターが下がったあと、ピークの印がどれくらい速く落ちるか
vu-channels = チャンネル
    .description = ステレオを 2 つに分けるか、1 つのメーターにまとめるか
vu-channels-mono = モノラル
vu-channels-stereo = ステレオ
vu-db-scale = dB の目盛り
    .description = メーターの後ろに、dB の位置でラベル付きの目盛り線を描く
vu-gradient-mode = グラデーション
    .description = レベルでメーターに色を付ける。テーマのグラデーション、曲テーマ時はカバーアートの色、またはカスタムの 2 色
vu-hold-on-pause = 一時停止で止める
    .description = 一時停止中はメーターを凍結し、無音まで落とさない
vu-orientation = 向き
    .description = メーターが伸びる元になる辺
vu-peak-caps = ピークの印
    .description = 各メーターの直近のピークに印を残す
vu-section-meter = メーター
vu-segment-gap = セグメントの間隔
    .description = 積み上げたセルどうしの継ぎ目
vu-segment-height = セグメントの高さ
    .description = 積み上げた各セルの高さ
vu-style = スタイル
    .description = ひと続きの柱、または LED 風のセグメント
vu-style-continuous = 連続
vu-style-segments = セグメント

## Spectrogram panel
spectrogram-ceiling = 天井
    .description = カラーマップの明るい端に対応するレベル。これより大きい音はすべてここに張り付く
spectrogram-colormap = カラーマップ
    .description = 音量を色にどう対応させるか
spectrogram-colormap-cover = カバー
spectrogram-colormap-grayscale = グレースケール
spectrogram-colormap-ice = アイス
spectrogram-colormap-magma = Magma
spectrogram-colormap-theme = テーマ
spectrogram-colormap-viridis = Viridis
spectrogram-direction = 方向
    .description = 新しい列が入ってくる辺。これによって周波数軸がパネルを上に伸びるか、横に伸びるかも決まる
spectrogram-fft-size = FFT サイズ
    .description = 解析に使う窓のサイズ。列がトランジェントに素早く反応するか、低い 2 つの音をきれいに分けられるかのトレードオフ
spectrogram-floor = フロア
    .description = カラーマップの暗い端に対応するレベル。これより小さい音はすべて背景として表示される
spectrogram-grid = 目盛り
    .description = 画面の上に重ねる周波数の区切り線
spectrogram-high-bound = 上限
    .description = 周波数軸の上端。ナイキスト周波数より下に抑えて、ほぼ無音な上の方のオクターブを省く
spectrogram-history = 履歴
    .description = 一番古い列が流れて消える前に、パネルがいくつの列を保持しておくか
spectrogram-hold-on-pause = 一時停止で止める
    .description = 一時停止中は静止した画面を保持し、無音を流し込まないようにする
spectrogram-labels = ラベル
    .description = パネルに余裕がある場所に沿って表示する周波数の数字
spectrogram-log-scale = 対数スケール
    .description = 実験機器のような均等な Hz 間隔ではなく、すべてのオクターブに同じ幅を与える、音楽的な読み方
spectrogram-low-bound = 下限
    .description = 周波数軸の下端
spectrogram-section-picture = 表示
spectrogram-speed = 速さ
    .description = 画面がどれくらいの速さでスクロールするか。1 秒あたりの列数

## Oscilloscope panel

oscilloscope-channels = チャンネル
    .description = 1 本のトレースにまとめるか、重ねて表示するか、それぞれに枠を立てて積み重ねるか
oscilloscope-channels-mono = モノラル
oscilloscope-channels-overlay = オーバーレイ
oscilloscope-channels-split = 分割
oscilloscope-fill = 塗りつぶし
    .description = トレースと中心線の間のやわらかい塗りつぶし
oscilloscope-gain = ゲイン
    .description = 縦方向のスケール。静かな曲を読み取れるトレースまで持ち上げる
oscilloscope-gradient-mode = グラデーション
    .description = 振れ幅でトレースに色を付ける。テーマのグラデーション、曲テーマ時はカバーアートの色、またはカスタムの 2 色
oscilloscope-grid = 目盛り
    .description = トレースの後ろに目盛りを描く
oscilloscope-hold-on-pause = 一時停止で止める
    .description = 一時停止中は静止したフレームを保持し、トレースが平らに落ちないようにする
oscilloscope-line-width = 線の太さ
    .description = トレースをどれくらい太く描くか
oscilloscope-persistence = 残光
    .description = 以前のフレームがトレースの後ろにどれくらい残るか、蛍光体の残光効果
oscilloscope-section-trace = 波形
oscilloscope-trigger = トリガー
    .description = 信号がトリガーレベルを超えた点から各フレームを始め、周期的な素材が静止して見えるようにする
oscilloscope-trigger-falling = 立ち下がり
oscilloscope-trigger-level = トリガーレベル
    .description = 交差を探すレベル
oscilloscope-trigger-off = オフ
oscilloscope-trigger-rising = 立ち上がり
oscilloscope-window = ウィンドウ
    .description = パネル全体でトレースが表す時間の長さ

## Shader panel
shader-panel-compile-error = このシェーダーはコンパイルできませんでした:
shader-panel-compile-title = このシェーダーはコンパイルできませんでした
shader-panel-enable = 有効にする
shader-panel-inspect = 中身を見る
shader-panel-note-empty-body = サンプルを選ぶか、fs_user(uv) を定義した .wgsl ファイルをパネルに指定してください。
shader-panel-note-empty-title = シェーダーが読み込まれていません。
shader-panel-note-missing-body = このパネルはワークスペースに無いシェーダーを参照しているので、動かすものがありません。
shader-panel-note-missing-title = { $name } はこのワークスペースのシェーダーにありません。
shader-panel-note-off-body = ソースとバインドはそのまま残って止まっています。
shader-panel-note-off-title = このシェーダーはオフです。
shader-panel-note-pending-body = このマシンからではなくレイアウトかワークスペースと一緒に届いたので、確認するまで止まっています。
shader-panel-note-pending-title = このシェーダーはまだ確認されていません。
shader-pending-origin-file = 出どころは { $path } とされています
shader-pending-origin-inline = 裏にファイルはなく、ソースはレイアウトと一緒に届きました
shader-pending-more-lines = ... 他 { $count } 行
shader-eject-name-taken = { $name } はこのワークスペースのシェーダーに既に { $count } 件の連番コピーがあります
shader-eject-not-in-pool = { $name } はこのワークスペースのシェーダーにありません
shader-eject-failed = 書き出し: { $error }
shader-panel-pick = シェーダーを選ぶ
shader-panel-run-shader = シェーダーを動かす
    .description = オフにするとソース、参照先、バインドはそのままで、何も描かれない
shader-panel-section-routes = ルート

## Genre grid panel
genre-grid-clear-picked = 選んだジャンルをクリア
genre-grid-desaturate = 再生中は彩度を落とす
    .description = 再生中のジャンル以外のタイルをグレースケールにする。カーソルを乗せたタイルは色が戻る
genre-grid-dim-while-playing = 再生中は暗くする
    .description = 再生中のジャンル以外のタイルを暗くする。カーソルを乗せたタイルは戻る
genre-grid-follow-description = 曲が変わるたびに再生中のジャンルまでスクロールする
genre-grid-merge-many = { $count } 件のジャンルを "{ $target }" にまとめる
genre-grid-merge-one = "{ $source }" を "{ $target }" にまとめる
genre-grid-pick-filters = 選択でライブラリを絞る
    .description = ジャンルをクリックすると、共有の検索に従うすべてのパネルがそのジャンルに絞られる。オフならクリックはただの選択になる
genre-grid-play-genres = { $count } 件のジャンルを再生
genre-grid-resume-description = 眺めるのをやめたら再生中のジャンルへ戻る
genre-grid-show-names = 名前を表示
    .description = カーソルを乗せたときだけでなく、各タイルの下にジャンルを出す
genre-grid-smooth-description = 飛ばずにジャンルまでなめらかに移動する
genre-grid-tally = { $albums ->
   *[other] { $albums } アルバム、{ $tracks } 曲
}
genre-grid-tile-face = タイルの表示
    .description = タイルに何を出すか。そのジャンルのアルバムカバー、ジャンル自身の色をかけたカバー、または名前を載せた単色のカード
genre-grid-unmerge = { $count ->
   *[other] { $count } 件のまとめを解除
}

## Artist grid panel
artist-grid-clear-picked = 選んだアーティストをクリア
artist-grid-desaturate = 再生中は彩度を落とす
    .description = 再生中のアーティスト以外のタイルをグレースケールにする。カーソルを乗せたタイルは色が戻る
artist-grid-dim-while-playing = 再生中は暗くする
    .description = 再生中のアーティスト以外のタイルを暗くする。カーソルを乗せたタイルは戻る
artist-grid-follow-description = 曲が変わるたびに再生中のアーティストまでスクロールする
artist-grid-group-mode = タイルの単位
    .description = アルバムアーティストなら、盤の客演はその盤を出した名義にまとまる。トラックアーティストなら、客演ごとに別のタイルに分かれる
artist-grid-pick-filters = 選択でライブラリを絞る
    .description = アーティストをクリックすると、共有の検索に従うすべてのパネルがそのアーティストに絞られる。オフならクリックはただの選択になる
artist-grid-play-artists = { $count } 組のアーティストを再生
artist-grid-portraits = アーティストの写真
    .description = 各アーティスト自身の写真を出す。名前ごとに一度だけ取得してディスクに置く。オフなら最初のアルバムのカバーを出す
artist-grid-resume-description = 眺めるのをやめたら再生中のアーティストへ戻る
artist-grid-section-grouping = グループ化
artist-grid-show-names = 名前を表示
    .description = カーソルを乗せたときだけでなく、各タイルの下にアーティストを出す
artist-grid-smooth-description = 飛ばずにアーティストまでなめらかに移動する
artist-grid-tally = { $albums ->
   *[other] { $albums } アルバム、{ $tracks } 曲
}
artist-grid-track-artist = トラックアーティスト

## Wall panels
wall-dim-always = 常に
    .description = 何も再生していなくてもタイルを引っ込めたままにする。カーソルを乗せたタイルだけがはっきり見える
wall-dim-amount = 減光量
    .description = 他のタイルをどこまで暗くするか。100% で見えなくなる
wall-gap = 間隔
    .description = タイルどうしの隙間
wall-name-alignment = 名前の配置
    .description = キャプションをタイルの下でそろえる
wall-rounding = 角の丸み
    .description = 各タイルの角を丸める。100% で円になる
wall-section-picking = 選択
wall-show-counts = 件数を表示
    .description = 各名前の下のアルバム数と曲数
wall-tile-size = タイルサイズ
    .description = タイルの長辺。列はパネルの幅を均等に割る

## Metadata panel
metadata-cover-background = カバーを背景に
    .description = 項目の後ろに曲のカバーアートを敷く
metadata-display = 表示
    .description = タイトルを頭に置いたシート、または上から並ぶ項目名と値の表
metadata-display-sheet = シート
metadata-display-table = テーブル
metadata-edit-save = 保存
metadata-field-bit-depth = ビット深度
metadata-field-bitrate = ビットレート
metadata-field-codec = コーデック
metadata-field-comment = コメント
metadata-field-disc = ディスク
metadata-field-file = ファイル
metadata-field-sample-rate = サンプルレート
metadata-field-track = トラック
metadata-fields = 項目
    .description = シートにどの項目を並べるか。曲が持っていない項目は出ない
metadata-find-online = オンラインでメタデータを探す...
metadata-no-library = ライブラリなし
metadata-row-borders-description = 表の各行の下の細い線
metadata-source = ソース
    .description = 再生中か選択中のものを追うか、ライブラリ全体を読むか
metadata-stripes-description = 表の一行おきに色を付ける

## History panel
history-column-last-played = 最終再生
history-descending = 降順
    .description = 並べ替えを逆にする
history-empty-never = すべての曲が再生済みです
history-empty-recent = 再生の記録がまだありません
history-headings = 最近のリストをアルバムの固まりで区切る。拡張ではカバーと統計も付く
history-sort-browse = 表示順
history-sort-date-added = 追加日
history-sort-menu = 並べ替え
    .description = 未再生の曲をどう並べるか
history-title = 履歴
history-view-most = よく聴く曲
history-view-never = 未再生
history-view-recent = 最近再生した曲
history-view-recent-short = 最近
history-view-row = 表示
    .description = 再生記録のどの切り口をパネルに出すか

## Folder tree panel
folder-tree-clear-scope = フォルダーの絞り込みをクリア
folder-tree-collapse-all = すべて折りたたむ
folder-tree-collapse-branch = 配下を折りたたむ
folder-tree-cover-art = カバーアート
    .description = 行のアイコンの代わりにアルバムアートを出す。フォルダーか曲に
folder-tree-cover-folders = フォルダー
folder-tree-cover-songs = 曲
folder-tree-empty = ライブラリにフォルダーがまだありません
folder-tree-expand-branch = 配下を展開
folder-tree-follow-description = 曲が変わるたびに再生中の曲を開いてそこまでスクロールする
folder-tree-nonmatch-folders = 一致しないフォルダー
    .description = 一致の無いフォルダーを隠すか、暗く残すか
folder-tree-nonmatch-songs = 一致しない曲
    .description = 一致したフォルダーの中で、外れた曲を暗くするか隠すか
folder-tree-play-folder = フォルダーを再生
folder-tree-play-songs = { $count ->
   *[other] { $count } 曲を再生
}
folder-tree-resume-description = 眺めるのをやめたら再生中の曲へ戻る
folder-tree-scope-to-folder = このフォルダーに絞る
folder-tree-smooth-description = 飛ばずに曲までなめらかに移動する
folder-tree-title = ツリー

## Art panel
art-always = 何も再生していなくてもカバーを引っ込めたままにする。カーソルを乗せたカバーだけがはっきり見える
art-convert = 変換...
art-covers-section = カバー
matcher-section-matches = 候補
art-desaturate = 再生中のアルバム以外のカバーをグレースケールにする。カーソルを乗せたカバーは色が戻る
art-dim-while-playing = 再生中のアルバム以外のカバーを暗くする。カーソルを乗せたカバーは戻る
art-disc-style = ディスクの見せ方
    .description = すべてのカバーを CD、またはレコードのレーベル面に見立てる
art-edit-tags = タグを編集...
art-fill-panel = パネルを埋める
    .description = 中央のカバーの大きさをパネルの高さだけで決める (縦向きのときは幅)。両側のカバーは縮まず縁からはみ出す
art-follow-description = 曲が変わるたびに再生中のアルバムを中央に置く
art-glow = グロー
    .description = 中央のカバーの後ろにアクセント色をにじませる。曲テーマがオンなら再生中のアルバムの色になる
art-label-position = ラベル位置
    .description = アルバムの表記の位置。上、カバーの下、下端、非表示
art-letter-rail = 文字インデックス
    .description = 棚の端にアーティストの頭文字を並べる。クリックでその文字の最初のアルバムへ移動
art-layout-section = レイアウト
art-perspective = パース
    .description = 両側のカバーを平たく潰さず、実際の 3D で回す
art-reflections = 反射
    .description = 棚の下の床に各カバーを映す
art-resume-description = 眺めるのをやめたら再生中のアルバムをまた中央に置く
art-shadows = 影
    .description = 各カバーの下の柔らかい影
art-smooth-description = 飛ばずにアルバムまでなめらかに移動する
art-title = アルバムカルーセル
art-vertical-layout = 縦レイアウト
    .description = 棚を横一列ではなく、上下にスクロールする縦一列にする

## Playlists panel
playlists-columns = タイトルの横にどのトラックの列を出すか
playlists-delete = プレイリストを削除
playlists-edit-query = クエリを編集...
playlists-empty = プレイリストがまだありません。曲を追加するか「新しいプレイリスト」を使ってください
playlists-headings = 各プレイリストの曲をアルバムの固まりで区切る。拡張ではカバーと統計も付く
playlists-import-tooltip = プレイリストをインポート
playlists-imported-fallback = インポート
playlists-new = 新しいプレイリスト...
playlists-new-smart = 新しいスマートプレイリスト...
playlists-refuse-drag-out = スマートプレイリストの曲は外へドラッグできません
playlists-refuse-edit-query = スマートプレイリストの中身を変えるにはクエリを編集してください
playlists-refuse-smart-source = スマートプレイリストの曲はクエリから決まります
playlists-remove = { $count ->
   *[other] { $count } 曲をプレイリストから削除
}
playlists-rename = 名前を変更...
playlists-title = プレイリスト

## Queue panel
queue-clear = キューをクリア
queue-empty = キューは空です
queue-headings = キューをアルバムの固まりで区切る。拡張ではカバーと統計も付く
queue-play-now = すぐ再生
queue-remove = { $count ->
   *[other] { $count } 曲をキューから削除
}
queue-title = キュー
queue-widget-always-modal = 常にモーダルで開く
    .description = 既に開いているキューパネルへ移動せず、毎回モーダルでキューを開く
queue-widget-clear-queue = キューをクリア
queue-widget-more = 他 { $count } 件
queue-widget-open-on-click = クリックでキューを開く
    .description = ウィジェットをクリックすると、開いているキューパネルへ移動する。無ければウィンドウでキューを開く
queue-widget-section-click = クリック
queue-widget-title = キューウィジェット
queue-widget-up-next = 次の曲

## Biography panel
biography-background = 背景
    .description = 文章の後ろのアーティストのファンアート。暗くして下に向かってフェードする
biography-fill-width = 幅いっぱいに
    .description = 高いヘッダーを、幅を制限して中央に置かず、全幅に広げる
biography-from-lastfm = Last.fm より
biography-header-image = ヘッダー画像
    .description = 上部の横長のアーティストバナー。バナーが無ければ写真
biography-keep-aspect = 縦横比を保つ
    .description = 帯に合わせて切り取らず、ヘッダーを本来の比率で出す
biography-listeners-count = { $count } リスナー
biography-looking-up = { $name } を検索中
biography-no-artist-tag = アーティストのタグがありません
biography-no-text = 略歴がありません
biography-not-found = { $name } は見つかりませんでした
biography-plays-count = { $count } 回再生
biography-refresh = 更新
biography-similar-artists = 似たアーティスト
    .description = 聴かれ方の近い関連アーティストを下部に出す
biography-similar-heading = 似たアーティスト
biography-stats = 統計
    .description = Last.fm のリスナー数と再生数を名前の下に出す
biography-tags = タグ
    .description = ジャンルのタグをチップの並びで出す
biography-title = バイオグラフィ

## Status panel
status-count-albums = { $count ->
   *[other] { $count } アルバム
}
status-count-artists = { $count ->
   *[other] { $count } アーティスト
}
status-count-plays = { $count ->
   *[other] { $count } 回再生
}
status-count-selected = { $count } 件を選択中
status-count-tracks = { $count ->
   *[other] { $count } 曲
}
status-readouts = 表示項目
    .description = バーに沿ってドラッグして並べ替え。行をまたいでドラッグするか、チップの x と + で表示と非表示を切り替える
status-scope-selection = 選択
status-title = ステータス

## Output panel
output-detail-badge = バッジ
output-detail-compact = コンパクト
output-detail-expanded = 拡張
output-detail-label = 詳しさ
    .description = バッジはチップだけにして残りをホバーに回す。コンパクトは見出しに一行を与え、縁沿いのストリップ向け。拡張は理由も横に添え、パネルが狭ければ下に回す
output-device-name = デバイス名
    .description = 見出しに動作中のデバイス名を出す。オフなら、行はモード・レート・フォーマットだけになる
output-file-rate = ファイルのレート
    .description = 何も変換していないときに、再生中のファイル自身のレートを確かめる。警告の対象は変換なので、変換中はどちらにせよ表示される
output-mode-exclusive = 排他
output-mode-shared = 共有
output-no-output = 出力なし
output-nothing-playing = 再生していません
output-pick-another-device = 別のデバイスを選ぶか、排他をオフにしてください
output-headline-numbers = { $rate } Hz、{ $channels } ch、{ $format }
output-headline = { $mode }、{ output-headline-numbers }
output-headline-device = { $device } で { $mode }、{ output-headline-numbers }
output-fell-back-to-shared = 排他が共有にフォールバックしました: { $why }
output-replaygain-levelling = ReplayGain がこのファイルの音量を { $db } dB 調整しています
output-replaygain-short = ReplayGain { $db } dB
output-rate-resampled = 再生中のファイルは { $rate } Hz で、デバイスに合わせてリサンプリングされています
output-rate-resampled-short = { $rate } Hz ファイル、リサンプリング済み
output-rate-native = 再生中のファイルは { $rate } Hz なので、リサンプリングはされていません
output-rate-native-short = { $rate } Hz ファイル、リサンプリングなし
output-start-track-hint = 曲を再生すると、デバイスが受け入れたフォーマットが出ます
output-title = 出力

## Track columns
columns-bits = ビット
columns-bpm = BPM
columns-codec = コーデック
columns-cover = カバー
columns-fav = お気に入り
columns-gain = ゲイン
columns-kbps = kbps
columns-khz = kHz
columns-name = 名前
columns-number = 番号
columns-scanned = スキャン日
columns-similar = 類似度

## Filter panel
filter-add-column = 列を追加
filter-add-column-tooltip = 列を追加
filter-all = すべて
filter-clear-filters = フィルターをクリア
filter-clear-selection = 選択をクリア
filter-empty = 絞り込む項目を選んでください
filter-remove-column = 列を削除

## Search panel
search-chips-below = 下
search-chips-inline = 同じ行
search-filter-chips = フィルターチップ
search-placeholder = ライブラリを検索

## Playback panel
playback-buttons = ボタン
    .description = バーに沿ってドラッグして並べ替え。行をまたいでドラッグするか、チップの x と + で表示と非表示を切り替える
playback-continue-down-list = 再生を続ける、リストの続きから
playback-continue-off = 再生を続けない
playback-continue-weighted = 再生を続ける、未再生を先に
playback-crossfade-inside-albums = アルバム内でも
playback-crossfade-off = クロスフェード オフ
playback-crossfade-tip = クロスフェード { $length }
playback-highlight-circle = 円
playback-highlight-square = 角丸
playback-hold-draw = { $tip }。長押しで引き方を選択
playback-hold-length = { $tip }。長押しで長さを選択
playback-hold-order = { $tip }。長押しで順序を選択
playback-loop-off = リピート オフ
playback-loop-queue = キューをリピート
playback-loop-track = この曲をリピート
playback-menu-continue = 継続再生ボタン
playback-menu-crossfade = クロスフェードボタン
playback-menu-favourite = お気に入りボタン
playback-menu-random = ランダムボタン
playback-menu-rating = レーティングの星
playback-menu-stop = 停止ボタン
playback-menu-stop-after = この曲で停止ボタン
playback-menu-volume = 音量ボタン
playback-pause = 一時停止
playback-play-highlight = 再生ボタンの強調
    .description = 再生ボタンのアクセントの塗り。円、角丸、またはなし
playback-random-tip-random = ランダムに 1 曲再生
playback-random-tip-similar = この曲に似た曲を再生
playback-seek-back-tip = 10 秒戻す
playback-seek-forward-tip = 10 秒進める
playback-shuffle-off = シャッフル オフ
playback-shuffle-on = シャッフル オン、{ $order } 順
playback-stop-after-armed = この曲で停止、設定済み
playback-stop-after-tip = この曲で停止
playback-stop-tip = 停止して曲を解放
playback-volume-tip-muted = ミュート解除、{ $percent }%。右クリックでスライダー
playback-volume-tip-unmuted = ミュート、{ $percent }%。右クリックでスライダー

## Track info panel
track-info-color-output-chip = 出力チップに色を付ける
    .description = 出力が共有に落ちたりリサンプリングしたりしたとき、チップを警告色にする。オフなら常に同じ落ち着いた色で、状態はホバーの説明が伝える
track-info-cycle-every = 切り替え間隔
    .description = 各行がフェードするまで留まる時間
track-info-cycle-rows = 行を切り替える
    .description = 並べた行を 1 行ずつフェードで入れ替えて 1 行に収める。1 行だけならそのまま表示される
track-info-delay = 待ち時間
    .description = 端に着いた行が、また動き出すまで留まる時間
track-info-marquee = 流れる文字
    .description = パネルに収まらない行の扱い。行き来させるか、途切れず回すか
track-info-menu-overflow = はみ出し
track-info-next = 次: { $line }
track-info-opening = 開いています...
track-info-output-fallback = 排他出力がデバイスに拒否されたため、共有ミキサー経由で再生しています。デバイスの応答: { $reason }
track-info-output-resample-exclusive = このファイルは { $source } kHz ですが、カードは { $device } kHz を受けたので、出力の途中で全サンプルが変換されています。デバイスがファイル自身のレートで動きませんでした。
track-info-output-resample-mixer = このファイルは { $source } kHz ですが、ミキサーは { $device } kHz で動いているので、出力の途中で全サンプルが変換されています。排他モードならファイル自身のレートをカードに渡せます。
track-info-overflow-loop = 回す
track-info-overflow-scroll = 流す
track-info-overflow-truncate = 切る
track-info-queued-count = キューに { $count } 曲
track-info-row-size = { $number } 行目のサイズ
track-info-speed = 速さ
    .description = 文字が流れる速さ
track-info-text-size = 文字サイズ

## Seek panel
seek-ending = 終わり方
    .description = 残り時間をカウントダウンするか、全体の長さを出すか
seek-ending-remaining = 残り
seek-ending-total = 全体
seek-playhead = 再生位置
    .description = バーの高さいっぱいに伸ばすか、線に寄せるか
seek-playhead-full = 全高
seek-playhead-line = 線
seek-playhead-max-height = 再生位置の最大高さ
    .description = 全高の再生位置に上限を設け、線を中心にそろえる。0 でパネルを埋める
seek-playhead-width = 再生位置の幅
    .description = 動く位置マーカーの幅
seek-rounding = 角の丸み
    .description = 線の角の半径。太さの半分まででピル型になる
seek-scrobble-marker = スクロブルの印
    .description = 曲が Last.fm にスクロブルされたとみなされる位置の細い線
seek-show-timings = 時間を表示
seek-thickness = 太さ
    .description = トラックの線の高さ

## Volume panel
volume-pieces = 構成要素
    .description = バーに沿ってドラッグして並べ替え。行をまたいでドラッグするか、チップの x と + で表示と非表示を切り替える。パーセントを隠すと、スピーカーのツールチップがそれを伝える
volume-readout = 表示
    .description = 音量をパーセントで出すか、かかっているデシベルのゲインで出すか
volume-readout-decibels = デシベル
volume-readout-percent = パーセント
volume-stretch = 引き伸ばし
    .description = スライダーの幅を制限せず、パネルいっぱいに広げる
volume-tip-mute = ミュート
volume-tip-mute-level = ミュート、{ $level }
volume-tip-unmute = ミュート解除
volume-tip-unmute-level = ミュート解除、{ $level }

## Shared panel content
content-filter = フィルター
content-no-track = 曲なし
content-total-genres = ジャンル
content-total-time = 合計時間

## Shared panel chrome
panel-columns-description = どのトラックの列を出すか
panel-headings = 見出し
panel-jump-to-playing = 再生中へ移動
panel-menu-display = 表示
panel-title-artists = アーティスト
panel-title-genres = ジャンル
panel-title-oscilloscope = オシロスコープ
panel-title-particles = パーティクル
panel-title-playback = 再生
panel-title-seek = シーク
panel-title-shader = シェーダー
panel-title-spectrogram = スペクトログラム
panel-title-spectrum = スペクトラム
panel-title-theme-toggle = テーマ切り替え
panel-title-track-info = 曲情報
panel-title-volume = 音量
panel-title-vu = VU メーター
panel-title-waveform = 波形

## Everything else
choice-both = 両方
choice-dim = 暗く
choice-hide = 非表示
composite-add-panel = パネルを追加
composite-host-settings = { $host } の設定
composite-move-left = 左へ移動
composite-move-right = 右へ移動
composite-remove = 削除
composite-replace = 差し替え
group-panel-add-slot = スロットを追加
group-panel-move-down = 下へ移動
group-panel-move-up = 上へ移動
group-panel-remove-slot = スロットを削除
group-panel-split-side-by-side = 左右に分割
group-panel-split-stacked = 上下に分割
group-panel-swap-panels = パネルを入れ替え
group-panel-title = グループ
overlay-dim = 減光
    .description = オーバーレイが出ているとき、下のメインパネルをどれくらい暗くするか
overlay-title = オーバーレイ
overlay-toggle = オーバーレイを切り替え
shader-confirm-hint-after = でどこからでもシェーダーを切り替えられます。
shader-confirm-hint-before = シェーダーはウィンドウを使いにくくすることがあります。元に戻すか、このウィンドウを閉じれば元の状態に戻ります。
shader-confirm-keep = そのまま使う
shader-confirm-question = この画面シェーダーをそのまま使いますか?
shader-confirm-revert = 元に戻す
shader-confirm-window-title = rox - オーバーレイシェーダー
slide-add = スライドを追加
slide-next = 次のスライド
slide-previous = 前のスライド
slide-title = スライド
theme-toggle-to-dark = ダークテーマに切り替え
theme-toggle-to-light = ライトテーマに切り替え
transport-favourite-add = お気に入りに追加
transport-favourite-nothing = お気に入りにするものがありません
transport-favourite-remove = お気に入りから削除
transport-pieces = 構成要素
    .description = 行に沿ってドラッグして並べ替え、行をまたいでドラッグして移動。チップの x と + で表示と非表示を切り替える

## Stragglers picked up in the final sweep
duplicates-scanning = スキャン中...
about-copyright = Copyright © 2026
signal-name-placeholder = シグナル名
signals-empty = シグナルがまだありません。追加するか、結び付けられるつまみを右クリックしてください。
signal-add = シグナルを追加
panel-approve = 承認
panel-turn-off = オフにする
shader-from-file = ファイルから...
arrange-add-row = 行を追加
smart-playlist-name-placeholder = プレイリスト名
smart-playlist-name-to-save = 保存するにはプレイリストに名前を付けてください
panel-new-playlist = 新しいプレイリスト...
panel-edit-tags = タグを編集...
panel-edit-cover = カバーアートを編集...
panel-rename-files = ファイル名を変更...
panel-convert = 変換...
panel-catalog-drag-anchor = ドラッグ領域
panel-catalog-spacer = スペーサー

## Duration and worker phrasing
pace-under-a-minute = 1 分足らず
pace-minutes = { $count ->
   *[other] 約 { $count } 分
}
pace-hours = { $count ->
   *[other] 約 { $count } 時間
}
pace-half-hours = 約 { $value } 時間
pace-days = { $count ->
   *[other] 約 { $count } 日
}
pace-workers = { $count ->
   *[other] 並列 { $count }
}
tasks-rest-takes = 、残りは { $estimate }
tasks-measuring-takes = 、測定には { $estimate }
tasks-working-out-takes = 、割り出しには { $estimate }
tasks-time-left = 、残り { $left }
tasks-failed-suffix = ({ $count } 件失敗)
tasks-file-suffix = - { $file }
tasks-no-beat-suffix = ({ $count } 件拍不明)
tasks-estimate-at-workers = ({ tasks-estimate-at })

## Panel vanity names
panel-title-art-view = アートビュー
panel-title-artist-grid = アーティストグリッド
panel-title-genre-grid = ジャンルグリッド
panel-title-biography = バイオグラフィ
panel-title-cover-art = カバーアート
panel-title-drag-anchor = ドラッグ領域
panel-title-drawer = ドロワー
panel-title-eq-widget = EQ ウィジェット
panel-title-filter = フィルター
panel-title-folder-tree = フォルダーツリー
panel-title-group = グループ
panel-title-history = 履歴
panel-title-lyrics = 歌詞
panel-title-menu = メニュー
panel-title-metadata = メタデータ
panel-title-mini-toggle = ミニ切り替え
panel-title-output = 出力
panel-title-overlay = オーバーレイ
panel-title-playlists = プレイリスト
panel-title-queue = キュー
panel-title-queue-widget = キューウィジェット
panel-title-search = 検索
panel-title-slide = スライド
panel-title-spacer = スペーサー
panel-title-stats-widget = 統計ウィジェット
panel-title-vu-meter = VU メーター
panel-title-window-controls = ウィンドウ操作

## Relative time and the output headline
ago-just-now = たった今
ago-minutes = { $count } 分前
ago-hours = { $count } 時間前
ago-days = { $count } 日前
ago-weeks = { $count } 週間前
ago-years = { $count } 年前

span-seconds = { $count ->
   *[other] { $count } 秒
}
span-minutes = { $count ->
   *[other] { $count } 分
}
span-hours = { $count ->
   *[other] { $count } 時間
}
span-days = { $count ->
   *[other] { $count } 日
}
span-weeks = { $count ->
   *[other] { $count } 週間
}
span-years = { $count ->
   *[other] { $count } 年
}
span-pair = { $first }{ $second }
unit-percent = { $value }%

settings-audio-output-headline = { $device } で { $mode }{ $note }、{ $rate } Hz、{ $channels } ch、{ $format }
settings-audio-output-experimental =  (実験的)

## ML model catalog
settings-mlmodels-description = { $summary }。1 曲あたり { $dim } 個の値。{ $licence }
settings-mlmodels-on-disk = 、ディスク上 { $size }
settings-mlmodels-to-download = 、ダウンロード { $size }
model-summary-dsp-timbre-1 = 組み込み、ダウンロード不要。各曲の対数帯域エネルギー、スペクトル形状、オンセット密度をまとめたもの。学習済みネットワークに比べれば粗いが、何も要らずどこでも動く
model-summary-panns-cnn10 = 音が何であるかを認識するよう AudioSet で学習させた畳み込みネットワーク。512 個の値による曲の記述は組み込みの概略よりはるかに豊かだが、24 MB のダウンロードと遅い解析パスが要る

## Shipped workspaces
workspace-shipped-default = (既定)
workspace-shipped-default-blurb = rox の素の見た目。デスクトップの上に半透明の面、ウィンドウ枠なし、曲テーマはオフ。ここにある他の見た目は、すべてここから離れていったもの。
workspace-shipped-catrox-blurb = すべての始まりだった foobar2000 のスキンを作り直したもの。カバーを円形の CD として描き、メタデータの項目を左に並べ、アルバムごとにまとめた曲にレーティングの点を添える。
workspace-shipped-critters-blurb = アプリ全体を 1 ビットの印刷物にしたもの。すべての面に規則的なディザをかけ、階調はサブベースで潰れ、ノイズの壁が曲に合わせてうねる。Critters for Sale より。
workspace-shipped-diffuse-blurb = 再生中のアルバムだけ。カバーと再生カードをひとつのグループにしてウィンドウを埋め、背景の上に透明な面を継ぎ目なく置く。ライブラリ、キュー、歌詞は右端のドロワーで待ち、ハンドルにカーソルを乗せると音楽の上に滑り出す。モノクロなので、色はカバーが担う。
workspace-shipped-foobar-blurb = このプロジェクト全体が相手取っているレイアウト。不透明なパネル、アーティストとアルバムのフィルター列、詰まった曲のテーブル、そしてメニューバーは昔からの場所に。
workspace-shipped-llama-winamp-blurb = 実際にそうだった Winamp ではなく、記憶の中の Winamp。Tahoma、ダーク、枠なし、上に点で描いたスペクトラム、ミニレイアウトにはシェードモード。
workspace-shipped-metro-blurb = Segoe UI のフラットなパネルとゆったりした行。曲テーマがオンで、パレット全体が再生中のカバーに従う。
workspace-shipped-phosphor-blurb = すべて等幅。Consolas、黒地に緑、クイック再生にカバーなし。たまたま音楽を鳴らす端末。
