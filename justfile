# 地理院地図Vector → Garmin 地図（.img）の作成タスク
#
# 範囲・出力先・陰影起伏の有無などの設定は gsi2garmin.toml に書く。
# 別の設定ファイルを使う例:
#   just config=tanzawa.toml all

# 設定ファイル
config := "gsi2garmin.toml"

bin := "target/release/gsi2garmin"

# タスク一覧を表示する
default:
    @just --list

# リリースビルドする
build:
    cargo build --release

# タイルを取得する（取得済みのタイルは飛ばすので、中断後の再実行で続きから取得）
fetch: build
    {{bin}} --config {{config}} fetch

# 取得済みのタイルから .img を作る
img: build
    {{bin}} --config {{config}} build

# 取得から .img 作成までまとめて行う
all: fetch img

# 処理中の一時ファイル（data/work）を削除する
clean:
    rm -rf data/work
