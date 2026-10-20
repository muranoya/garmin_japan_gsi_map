# 地理院地図Vector → Garmin 地図（.img）の作成タスク
#
# 対象エリアは変数で切り替える。例:
#   just area=tanzawa bbox=139.05,35.40,139.30,35.55 fetch img

# エリア名（出力ファイル名に使う）
area := "okutama"
# 範囲 minlon,minlat,maxlon,maxlat
bbox := "138.95,35.70,139.25,35.90"
# 取得データと生成物の置き場所
data_dir := "data"

bin := "target/release/gsi2garmin"
tiles := data_dir / area + ".mbtiles"
dem := data_dir / area + "-dem.mbtiles"
img_file := data_dir / area + ".img"

# タスク一覧を表示する
default:
    @just --list

# リリースビルドする
build:
    cargo build --release

# ベクタタイルと標高タイルを取得する（取得済みのタイルは飛ばすので、中断後の再実行で続きから取得）
fetch: fetch-vector fetch-dem

# ベクタタイル（地理院地図Vector）を取得する
fetch-vector: build
    mkdir -p {{data_dir}}
    {{bin}} fetch --bbox {{bbox}} -o {{tiles}}

# 標高タイル（陰影起伏用）を取得する
fetch-dem: build
    mkdir -p {{data_dir}}
    {{bin}} fetch --source dem --bbox {{bbox}} -o {{dem}}

# 取得済みのタイルから .img を作る（追加の build オプションを渡せる。例: just img --code-page 932）
img *args: build
    {{bin}} build {{tiles}} --dem {{dem}} -o {{img_file}} {{args}}

# 取得から .img 作成までまとめて行う
all: fetch img
