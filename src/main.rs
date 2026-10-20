mod build;
mod dem;
mod fetch;
mod geom;
mod mbtiles;
mod mkgmap;
mod mvt;
mod osm;
mod source;
mod tile;

use anyhow::{Result, ensure};
use clap::{Parser, Subcommand};
use std::path::PathBuf;
use tile::BBox;

/// 地理院地図Vector から Garmin 用ベクタ地図を作るツール
#[derive(Parser)]
#[command(version)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// 地理院ベクタタイルを取得して MBTiles に保存する（中断しても再実行で続きから取得）
    Fetch {
        /// 取得範囲 minlon,minlat,maxlon,maxlat（例: 奥多摩 138.95,35.70,139.25,35.90）
        #[arg(long, allow_hyphen_values = true)]
        bbox: BBox,
        /// 取得するデータの種類
        #[arg(long, value_enum, default_value_t = fetch::Dataset::Vector)]
        source: fetch::Dataset,
        /// 取得するズームレベル（カンマ区切りで複数指定可。省略時はデータの最大ズーム）
        #[arg(long, value_delimiter = ',')]
        zoom: Vec<u8>,
        /// 保存先の MBTiles ファイル（既存なら追記）
        #[arg(short, long)]
        output: PathBuf,
        /// 同時リクエスト数（地理院サーバへの負荷を考えて小さめに）
        #[arg(long, default_value_t = 4, value_parser = clap::value_parser!(u16).range(1..=16))]
        concurrency: u16,
        /// タイル URL テンプレート（省略時は --source に応じた地理院の URL）
        #[arg(long)]
        url: Option<String>,
    },
    /// タイル（MBTiles またはフォルダ）から Garmin 用の .img を作る
    Build {
        /// 入力: MBTiles ファイル、または {z}/{x}/{y}.pbf 形式のフォルダ
        input: PathBuf,
        /// 出力する .img ファイル（端末の /Garmin フォルダに置く）
        #[arg(short, long)]
        output: PathBuf,
        /// 使うズームレベル（省略時は入力の最大ズーム）
        #[arg(long)]
        zoom: Option<u8>,
        /// mkgmap のスタイルフォルダ（省略時は同梱の style/gsi）
        #[arg(long)]
        style: Option<PathBuf>,
        /// mkgmap.jar のパス（省略時は自動でダウンロードしてキャッシュする）
        #[arg(long)]
        mkgmap: Option<PathBuf>,
        /// 中間ファイルを残すフォルダ（省略時は一時フォルダを使って削除する）
        #[arg(long)]
        work_dir: Option<PathBuf>,
        /// TYP ファイル（テキスト形式。省略時は同梱の style/gsi.typ.txt）
        #[arg(long)]
        typ: Option<PathBuf>,
        /// 陰影起伏用の標高タイル（`fetch --source dem` で取得した MBTiles またはフォルダ）
        #[arg(long)]
        dem: Option<PathBuf>,
        /// 端末の地図一覧に表示する名前
        #[arg(long, default_value = "地理院地図Vector")]
        name: String,
        /// 地図の説明（省略時は入力ファイル名）
        #[arg(long)]
        description: Option<String>,
        /// 文字コード。65001 = UTF-8、日本語が表示できない場合は 932（Shift_JIS）を試す
        #[arg(long, default_value_t = 65001)]
        code_page: u16,
        /// Garmin のファミリー ID。他の地図と重ならない値にする
        #[arg(long, default_value_t = 7251)]
        family_id: u16,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    match Cli::parse().command {
        Command::Fetch { bbox, source, zoom, output, concurrency, url } => {
            let max = source.max_zoom();
            let zooms = if zoom.is_empty() { vec![max] } else { zoom };
            ensure!(zooms.iter().all(|&z| z <= max), "ズームレベルは 0〜{max} で指定してください");
            fetch::run(fetch::FetchOptions {
                bbox,
                zooms,
                output,
                url_template: url.unwrap_or_else(|| source.url().to_string()),
                concurrency: concurrency.into(),
                dataset: source,
            })
            .await
        }
        Command::Build {
            input,
            output,
            zoom,
            style,
            mkgmap,
            work_dir,
            typ,
            dem,
            name,
            description,
            code_page,
            family_id,
        } => {
            let description = description.unwrap_or_else(|| {
                input.file_stem().map_or(name.clone(), |s| s.to_string_lossy().into_owned())
            });
            build::run(build::BuildOptions {
                input,
                output,
                zoom,
                style,
                mkgmap,
                work_dir,
                typ,
                dem,
                map: mkgmap::MapInfo { family_id, name, description, code_page },
            })
            .await
        }
    }
}
