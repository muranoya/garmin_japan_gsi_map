# 地理院地図Vector → Garmin 地図（.img）の作成タスク
#
# 対象エリアは変数で切り替える。例:
#   just area=tanzawa bbox=139.05,35.40,139.30,35.55 fetch img

# エリア名（出力ファイル名に使う）
area := "okutama"
# 範囲 minlon,minlat,maxlon,maxlat
bbox := "138.92,35.45,140.15,35.92"
# 陰影起伏を入れるか（true で標高タイルも取得して地図に入れる）。
# 端末での描画が重くなり、縮小時に山地が灰色っぽく見えるので既定では入れない
hillshade := "false"
# データの置き場所
#   data/tiles/   取得したタイル（MBTiles）。再取得せずに使い回す
#   data/work/    処理中の一時ファイル。just img の開始時に作り直す（just clean で削除）
#   data/output/  最終成果物（.img）
data_dir := "data"
tiles_dir := data_dir / "tiles"
work_dir := data_dir / "work" / area
output_dir := data_dir / "output"

bin := "target/release/gsi2garmin"
tiles := tiles_dir / area + ".mbtiles"
dem := tiles_dir / area + "-dem.mbtiles"
img_file := output_dir / area + ".img"

# タスク一覧を表示する
default:
    @just --list

# リリースビルドする
build:
    cargo build --release

# タイルを取得する（取得済みのタイルは飛ばすので、中断後の再実行で続きから取得）
fetch: fetch-vector
    {{ if hillshade == "true" { just_executable() + " area=" + area + " bbox=" + bbox + " fetch-dem" } else { "" } }}

# ベクタタイル（地理院地図Vector）を取得する
fetch-vector: build
    mkdir -p {{tiles_dir}}
    {{bin}} fetch --bbox {{bbox}} -o {{tiles}}

# 標高タイル（陰影起伏用）を取得する
fetch-dem: build
    mkdir -p {{tiles_dir}}
    {{bin}} fetch --source dem --bbox {{bbox}} -o {{dem}}

# 取得済みのタイルから .img を作る（追加の build オプションを渡せる。例: just img --code-page 932）
img *args: build
    rm -rf {{work_dir}}
    mkdir -p {{work_dir}} {{output_dir}}
    {{bin}} build {{tiles}} {{ if hillshade == "true" { "--dem " + dem } else { "" } }} -o {{img_file}} --work-dir {{work_dir}} {{args}}

# 取得から .img 作成までまとめて行う
all: fetch img

# 処理中の一時ファイル（data/work）を削除する
clean:
    rm -rf {{data_dir}}/work
