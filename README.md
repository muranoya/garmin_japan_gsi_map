# garmin_japan_gsi_map

国土地理院の **地理院地図Vector（ベクタタイル）** から、GARMIN の GPS 端末（GPSMAP 67 で確認）で表示できる
**ベクタ形式の地図（`.img`）** を作る CLI ツールです。

ラスタ画像を貼り付けた地図とは異なり、等高線・道路・徒歩道・河川・建物・注記（山名など）・地図記号を
ベクタデータとして持つため、拡大しても鮮明で、ズームに応じて表示内容が切り替わります。
登山時に地理院地図風の地図を表示し、GPX のトラックやルートを重ねて使うことを想定しています。

- 地理院地図風の配色・線種・アイコン（TYP ファイル）
- ズームアウトに応じた等高線の間引き（補助曲線 → 10m → 50m → 100m）
- 標高タイル（DEM）による陰影起伏
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

既定では奥多摩の範囲のタイルを `data/` に取得し、`data/okutama.img` を作ります。
別の範囲を作るときは、エリア名と範囲（`最小経度,最小緯度,最大経度,最大緯度`）を指定します。

```sh
just area=tanzawa bbox=139.05,35.40,139.30,35.55 all
```

個別のタスクも実行できます（いずれもリリースビルドを使います）。

| コマンド | 内容 |
|---|---|
| `just build` | リリースビルド |
| `just fetch` | ベクタタイルと標高タイルを取得（中断しても再実行で続きから取得） |
| `just img` | 取得済みのタイルから `.img` を作成 |
| `just img <オプション>` | `build` に追加のオプションを渡す（例: `just img --code-page 932`） |

### 端末に入れる

できた `.img` を、端末の本体メモリまたは microSD カードの `Garmin` フォルダにコピーし、
端末の地図設定で「地理院地図Vector」を有効にします。

日本語が表示されない場合は、`just img --code-page 932`（Shift_JIS）で作り直してください。

### 見た目を調整する

| ファイル | 内容 |
|---|---|
| `style/gsi/` | mkgmap スタイル。どの地物を Garmin のどの種別にし、どのズームから表示するか |
| `tools/gen_typ.py` | TYP（色・線種・アイコン）の生成スクリプト。`python3 tools/gen_typ.py style/gsi.typ.txt` で再生成 |

`just img --style style/gsi --typ style/gsi.typ.txt` とすると、ビルドし直さずに編集中のスタイルと TYP を試せます。

### コマンドを直接使う

```sh
gsi2garmin fetch --bbox 138.95,35.70,139.25,35.90 -o data/okutama.mbtiles
gsi2garmin fetch --source dem --bbox 138.95,35.70,139.25,35.90 -o data/okutama-dem.mbtiles
gsi2garmin build data/okutama.mbtiles --dem data/okutama-dem.mbtiles -o data/okutama.img
```

すべてのオプションは `gsi2garmin fetch --help` / `gsi2garmin build --help` で確認できます。

## データの出典

地図データは国土地理院の「地理院地図Vector」および「標高タイル」を使用しています。
利用にあたっては [国土地理院コンテンツ利用規約](https://www.gsi.go.jp/kikakuchousei/kikakuchousei40182.html) に従ってください。
