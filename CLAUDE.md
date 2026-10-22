# Japan GSI to Garmin Map

## プロジェクト概要

国土地理院の **地理院地図Vector（ベクタタイル）** を元に、GARMIN GPSMAP 67 で表示できる**ベクタ形式の Garmin 地図（`.img`）** を生成する CLI ツール。

- 用途: 登山時の地図表示。事前に取り込んだ GPX のトラック／ルートを重ねて使う
- 目標とする見た目は「地理院地図風」。完全再現は求めない

### スコープ外（やらないこと）

- ルーティング（経路探索）対応。道路は表示専用でよい
- ラスタ画像（地理院タイル PNG）の変換
- 成果物の配布・公開

## データソース

### 地理院地図Vector（主データ）

- URL: `https://cyberjapandata.gsi.go.jp/xyz/experimental_bvmap/{z}/{x}/{y}.pbf`
  - Mapbox Vector Tile、extent 4096、**最大ズーム 16**（17 以上は 404）
  - 属性は数値コード（`ftCode` など）。変換表はこのコード基準で作る
- 別系統 `optimal_bvmap-v1` は属性が日本語文字列（`vt_code`, `vt_rdctg='都道府県道'` 等）。原則使わない
- 主なレイヤ（experimental_bvmap）:

| レイヤ | 型 | 主な属性 |
|---|---|---|
| `contour` | Line | `alti`（標高）, `altiFlag` |
| `road` | Line | `rdCtg`（道路種別）, `rnkWidth`（幅員区分）, `lvOrder`（階層）, `motorway`, `tollSect` |
| `railway` | Line | `snglDbl`, `railState`, `lvOrder` |
| `river` / `waterarea` | Line / Polygon | `ftCode` |
| `building` | Polygon/Line | `ftCode`, `lvOrder` |
| `label` | Point | `knj`（注記文字）, `kana`, `annoCtg`, `arrng`, `arrngAgl` |
| `symbol` / `elevation` | Point | `ftCode`, `alti` |
| `landforml` / `landformp` | Line / Point | 崖・岩などの地形表現 |
| `structurel` / `boundary` | Line | 送電線・構造物 / 行政界 |

- 各タイルは **80px のバッファ**付きで、隣接タイルと地物が重複している
- 隣接タイル間で境界付近の形状がわずかに異なる（タイルごとに簡略化されているとみられる）。境界で切った端点が数 px〜数十 px ずれることがある
- z14〜z16 のいずれも 25000 レベルのデータを含む。既定では最も詳細な z16 を使う
- 低ズームのタイルには地理院が小縮尺用に選んだ地物が入る（z10 の `annoCtg=315` は主な山名）。
  ズームアウト時の山名のため z10 も取得し、その点を `gsi_zoom=10` タグ付きで加えている（`build::OVERVIEW_ZOOM`）
- 地物コード（`ftCode`、注記の `annoCtg`）の意味は、地理院のスタイル定義
  `https://gsi-cyberjapan.github.io/gsivectortile-mapbox-gl-js/std.json` の各レイヤの `metadata.path` から分かる
- 同じ地物に複数の注記が付くことがある（例: 滝に `annoCtg` 323 と 822 の両方）
- タイル一覧 `https://cyberjapandata.gsi.go.jp/xyz/experimental_bvmap/mokuroku.csv.gz`（約 77MB）に、実在するタイルの
  `パス,更新時刻,サイズ,MD5` が並ぶ。z16 は約 216 万タイル・17GB（bbox で日本全体を囲むと約 2,700 万タイルで大半が海）。
  `fetch.area = "all"` はこの一覧のタイルを取得する（`src/mokuroku.rs`）。標高タイル（dem_png）には一覧がないので、
  ベクタタイルの一覧を覆うタイルを取る

### DEM（補助）

- 陰影起伏（mkgmap `--dem`）用途のみ。等高線はベクタタイルのものを使い、DEM から再生成しない

## アーキテクチャ

```
入力: タイルフォルダ ({z}/{x}/{y}.pbf) または MBTiles
  → MVT デコード
  → タイル境界で分断された地物の再結合
  → ftCode 等 → OSM 風タグへの変換
  → .osm.pbf（または OSM XML）出力
  → mkgmap（+ style + TYP）で gmapsupp.img を生成
```

- `.img` の書き出しは自前実装せず **mkgmap に任せる**（Java。jar は初回に `~/.cache/gsi2garmin/` へ自動取得）
- **Rust 側は幾何処理だけを行い、属性の意味は解釈しない。** レイヤ名を `gsi_layer` タグに入れ、
  地理院の属性（`ftCode`, `alti`, `knj` など）をそのまま OSM タグとして出力する。
  Garmin の種別への対応付けはすべて mkgmap スタイル（`style/gsi/`）で行う
- 見た目は mkgmap のスタイルファイルと TYP ファイルで調整する。`build --style <dir>` でコード変更なしに試行錯誤できる
- 幾何処理（`src/geom.rs`）:
  - 線: 各タイルの範囲ちょうどで切り直し、タイル境界上の端点どうしを結合する。
    1 段階目は一致する端点（3 つ以上集まる点は分岐とみなして結合しない）、2 段階目は許容距離内の相互最近傍
  - 面: タイル範囲で切り取るだけ（結合はしない）
  - 点: タイル範囲内（半開区間）のものだけを採用し、バッファによる重複を除く
- 分割（`src/split.rs`）: Garmin は 1 タイルの RGN が 16MB までなので、ノード数が `--max-nodes`（既定 200 万）以下に
  なるまで四分木で範囲を分け、区画ごとに `<bounds>` 付きの OSM を書き出す。mkgmap は区画を並列に処理する。
  目安: 奥多摩〜東京（約 2.4 万タイル）で 160 区画、最大の区画でも RGN は上限の約 20%
- 全国規模でも全地物をメモリに載せないよう、先にタイルを四分木で「領域」に分け（`split::plan_regions`。
  データ量は圧縮されたままのタイルのバイト数で見積もる）、領域ごとに読込・結合・区画への分割・書き出しを行う。
  領域どうしを並列に処理し、領域内は 1 スレッドで処理する。線は領域の境界でも切れる（区画の境界と同じ扱い）。
  目安: 関東周辺 8.3 万タイル（全国の約 1/10）で Rust 側最大 1.1GB
- 処理の流れの主なモジュール: `fetch.rs`（取得）→ `source.rs`（読込）→ `mvt.rs`（デコード）→ `geom.rs` → `osm.rs`（OSM XML）→ `mkgmap.rs`。DEM は `dem.rs`
- CLI（普段は justfile 経由で使う: `just fetch` / `just img` / `just all`、設定ファイルは `just config=<file> ...` で指定）:
  - `gsi2garmin [--config gsi2garmin.toml] fetch`（ベクタタイル。陰影起伏が有効なら標高タイルも）
  - `gsi2garmin [--config gsi2garmin.toml] build`
- 設定（範囲・ファイルの保存先・陰影起伏の有無・地図名・文字コードなど）はすべて TOML の設定ファイルに書き、
  CLI 引数では渡さない。項目は `src/config.rs`、説明付きの例はリポジトリ直下の `gsi2garmin.toml`。
  項目を増やしたら `gsi2garmin.toml` にもコメント付きで書き足す
- 陰影起伏は既定で無効（端末の描画が重くなり、縮小時に山地が灰色っぽく見えるため）
- 見た目の定義:
  - `style/gsi/`: mkgmap スタイル（地物 → Garmin 種別・表示解像度）。`options` の `levels` は DEM の点間隔にも使う
  - `style/gsi.typ.txt`: TYP（色・線種・アイコン）。**`tools/gen_typ.py` から生成する**ので、変更はスクリプト側で行い
    `python3 tools/gen_typ.py style/gsi.typ.txt` で再生成する
- DEM: 地理院の標高 PNG タイルを 1 秒角の HGT（`src/dem.rs`）に変換して mkgmap の `--dem` に渡す

## 技術スタック

- 言語: Rust（Cargo）。並列処理は rayon を想定
- 外部ツール: mkgmap（Java 21 で実行）
- 調査用の使い捨てスクリプトは Python（`uv run --with ...`）でもよいが、本体は Rust で書く

## 開発ルール

- `Cargo.toml` の依存はメジャーバージョンのみ指定（例: `serde = "1"`）。`Cargo.lock` はコミットする
- ダウンロードしたタイル・中間ファイル・生成した `.img` はリポジトリにコミットしない。置き場所は `data/tiles/`（MBTiles）、`data/work/<area>/`（一時ファイル）、`data/output/`（.img）
- 地理院サーバへの取得は低い並列度・適度な間隔で行い、取得済みタイルはキャッシュして再取得しない
- 動作確認の基準エリアは **奥多摩**（おおよそ 138.95,35.70 – 139.25,35.90）
- 実装・説明・コメントは日本語で書く
