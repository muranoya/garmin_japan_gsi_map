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
- 地物コード（`ftCode`、注記の `annoCtg`）の意味は、地理院のスタイル定義
  `https://gsi-cyberjapan.github.io/gsivectortile-mapbox-gl-js/std.json` の各レイヤの `metadata.path` から分かる
- 同じ地物に複数の注記が付くことがある（例: 滝に `annoCtg` 323 と 822 の両方）

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
- 処理の流れの主なモジュール: `fetch.rs`（取得）→ `source.rs`（読込）→ `mvt.rs`（デコード）→ `geom.rs` → `osm.rs`（OSM XML）→ `mkgmap.rs`
- CLI:
  - `gsi2garmin fetch --bbox <minlon,minlat,maxlon,maxlat> -o area.mbtiles`
  - `gsi2garmin build <tiles_dir|area.mbtiles> -o area.img [--work-dir <dir>] [--style <dir>] [--code-page 932]`

## 技術スタック

- 言語: Rust（Cargo）。並列処理は rayon を想定
- 外部ツール: mkgmap（Java 21 で実行）
- 調査用の使い捨てスクリプトは Python（`uv run --with ...`）でもよいが、本体は Rust で書く

## 開発ルール

- `Cargo.toml` の依存はメジャーバージョンのみ指定（例: `serde = "1"`）。`Cargo.lock` はコミットする
- ダウンロードしたタイル・中間ファイル・生成した `.img` はリポジトリにコミットしない
- 地理院サーバへの取得は低い並列度・適度な間隔で行い、取得済みタイルはキャッシュして再取得しない
- 動作確認の基準エリアは **奥多摩**（おおよそ 138.95,35.70 – 139.25,35.90）
- 実装・説明・コメントは日本語で書く
