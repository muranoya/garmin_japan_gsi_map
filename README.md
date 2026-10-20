# garmin_japan_gsi_map

国土地理院の **地理院地図Vector（ベクタタイル）** から、GARMIN の GPS 端末（GPSMAP 67 で確認）で表示できる
**ベクタ形式の地図（`.img`）** を作る CLI ツールです。

ラスタ画像を貼り付けた地図とは異なり、等高線・道路・徒歩道・河川・建物・注記（山名など）・地図記号を
ベクタデータとして持つため、拡大しても鮮明で、ズームに応じて表示内容が切り替わります。
登山時に地理院地図風の地図を表示し、GPX のトラックやルートを重ねて使うことを想定しています。

- 地理院地図風の配色・線種・アイコン（TYP ファイル）
- ズームアウトに応じた等高線の間引き（補助曲線・10m → 20m → 50m → 200m）
- 標高タイル（DEM）による陰影起伏（任意。既定では入れない）
- ルート案内（経路探索）には対応していません

## 仕組み

```
地理院ベクタタイル ──fetch──▶ MBTiles ──build──▶ OSM XML ──mkgmap──▶ .img
地理院標高タイル   ──fetch──▶ MBTiles ──build──▶ HGT ────┘
```

タイル境界で分断された地物の再結合などの幾何処理を Rust で行い、`.img` の生成は
[mkgmap](https://www.mkgmap.org.uk/) に任せています（初回実行時に自動でダウンロードします）。

## 必要なもの

- Rust（cargo）
- Java 17 以上（mkgmap の実行に使用）
- [just](https://github.com/casey/just)

## 使い方

### 地図を作る

```sh
just all
```

範囲・出力先・陰影起伏の有無などの設定は、すべて設定ファイル [`gsi2garmin.toml`](gsi2garmin.toml) に書きます。
指定できる項目と既定値はこのファイルのコメントにまとめてあります。
同梱の設定では奥多摩周辺のタイルを取得し、`data/output/okutama.img` を作ります。

別の範囲の地図を作るときは、設定ファイルをコピーして `bbox` とファイルの保存先を書き換え、
`config` で指定します。

```sh
cp gsi2garmin.toml tanzawa.toml   # bbox = [139.05, 35.40, 139.30, 35.55] などに書き換える
just config=tanzawa.toml all
```

陰影起伏（山肌の影）は `[hillshade]` の `enabled = true` で入ります（`fetch` で標高タイルも取得します）。
端末での描画が重くなり、縮小時に山地が灰色っぽく見えるため、既定では入れていません。

個別のタスクも実行できます（いずれもリリースビルドを使います）。

| コマンド | 内容 |
|---|---|
| `just build` | リリースビルド |
| `just fetch` | ベクタタイル（陰影起伏を入れる設定なら標高タイルも）を取得（中断しても再実行で続きから取得） |
| `just img` | 取得済みのタイルから `.img` を作成 |
| `just clean` | 処理中の一時ファイル（`data/work/`）を削除 |

ファイルは `data/` の下に種類ごとに保存されます。

| フォルダ | 内容 |
|---|---|
| `data/tiles/` | 取得したタイル（MBTiles）。再取得せずに使い回します |
| `data/work/<エリア名>/` | 処理中の一時ファイル（OSM・HGT など）。`.img` を作るたびに作り直します。範囲が広いと数 GB〜十数 GB になります |
| `data/output/` | 最終成果物の `.img` |

### 端末に入れる

できた `.img` を、端末の本体メモリまたは microSD カードの `Garmin` フォルダにコピーし、
端末の地図設定で「地理院地図Vector」を有効にします。

日本語が表示されない場合は、設定ファイルの `[map]` で `code_page = 932`（Shift_JIS）にして作り直してください。

### 見た目を調整する

| ファイル | 内容 |
|---|---|
| `style/gsi/` | mkgmap スタイル。どの地物を Garmin のどの種別にし、どのズームから表示するか |
| `tools/gen_typ.py` | TYP（色・線種・アイコン）の生成スクリプト。`python3 tools/gen_typ.py style/gsi.typ.txt` で再生成 |

設定ファイルの `[build]` で `style = "style/gsi"`、`typ = "style/gsi.typ.txt"` とすると、ビルドし直さずに編集中のスタイルと TYP を試せます。

### コマンドを直接使う

```sh
gsi2garmin fetch                        # カレントフォルダの gsi2garmin.toml を使う
gsi2garmin --config tanzawa.toml build  # 設定ファイルを指定する
```

## データの出典

地図データは国土地理院の「地理院地図Vector」および「標高タイル」を使用しています。
利用にあたっては [国土地理院コンテンツ利用規約](https://www.gsi.go.jp/kikakuchousei/kikakuchousei40182.html) に従ってください。
