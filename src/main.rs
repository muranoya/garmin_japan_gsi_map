mod fetch;
mod mbtiles;
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
        /// 取得するズームレベル（カンマ区切りで複数指定可。最大 16）
        #[arg(long, value_delimiter = ',', default_value = "16")]
        zoom: Vec<u8>,
        /// 保存先の MBTiles ファイル（既存なら追記）
        #[arg(short, long)]
        output: PathBuf,
        /// 同時リクエスト数（地理院サーバへの負荷を考えて小さめに）
        #[arg(long, default_value_t = 4, value_parser = clap::value_parser!(u16).range(1..=16))]
        concurrency: u16,
        /// タイル URL テンプレート
        #[arg(long, default_value = fetch::DEFAULT_URL)]
        url: String,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    match Cli::parse().command {
        Command::Fetch { bbox, zoom, output, concurrency, url } => {
            ensure!(zoom.iter().all(|&z| z <= 16), "ズームレベルは 0〜16 で指定してください");
            fetch::run(fetch::FetchOptions {
                bbox,
                zooms: zoom,
                output,
                url_template: url,
                concurrency: concurrency.into(),
            })
            .await
        }
    }
}
