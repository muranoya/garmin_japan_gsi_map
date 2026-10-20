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

- タイル境界で地物がクリップされているため、**線は端点で結合、面は union して再構成する**必要がある
- レイヤごとに収録ズームが異なる場合がある（例: 等高線は z14 にも存在）。どのズームから何を取るかは実データで確認して決める

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

- `.img` の書き出しは自前実装せず **mkgmap に任せる**（Java。jar は自動取得する想定）
- 見た目は mkgmap のスタイルファイルと TYP ファイルで調整する。コード変更なしで試行錯誤できる形を保つ
- CLI イメージ（予定）:
  - `gsi2garmin fetch --bbox <minlon,minlat,maxlon,maxlat> -o area.mbtiles`
  - `gsi2garmin build <tiles_dir|area.mbtiles> -o gmapsupp.img`

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
