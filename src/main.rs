mod build;
mod config;
mod dem;
mod fetch;
mod geom;
mod mbtiles;
mod mkgmap;
mod mvt;
mod osm;
mod source;
mod split;
mod tile;

use anyhow::Result;
use clap::{Parser, Subcommand};
use config::Config;
use std::path::PathBuf;

/// 地理院地図Vector から Garmin 用ベクタ地図を作るツール
///
/// 範囲・出力先・陰影起伏の有無などの設定はすべて設定ファイル（TOML）に書く。
#[derive(Parser)]
#[command(version)]
struct Cli {
    /// 設定ファイル
    #[arg(short, long, global = true, default_value = "gsi2garmin.toml")]
    config: PathBuf,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// タイルを取得して MBTiles に保存する（中断しても再実行で続きから取得）。
    /// 陰影起伏を入れる設定なら標高タイルも取得する
    Fetch,
    /// 取得したタイルから Garmin 用の .img を作る
    Build,
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    let config = Config::load(&cli.config)?;
    match cli.command {
        Command::Fetch => {
            let bbox = config.bbox()?;
            let vector = fetch::Dataset::Vector;
            fetch::run(fetch::FetchOptions {
                bbox,
                zooms: {
                    // 縮小表示用の注記（主な山名など）を取るため、低ズームのタイルも取得する
                    let z = config.zoom.unwrap_or(vector.max_zoom());
                    if z > build::OVERVIEW_ZOOM { vec![build::OVERVIEW_ZOOM, z] } else { vec![z] }
                },
                output: config.files.tiles.clone(),
                url_template: config.fetch.url.clone().unwrap_or_else(|| vector.url().to_string()),
                concurrency: config.fetch.concurrency.into(),
                dataset: vector,
            })
            .await?;
            if let Some(dem) = config.dem() {
                let source = config.hillshade.source.dataset();
                eprintln!("標高タイル（陰影起伏用）を取得します");
                fetch::run(fetch::FetchOptions {
                    bbox,
                    zooms: vec![source.max_zoom()],
                    output: dem.to_path_buf(),
                    url_template: config.hillshade.url.clone().unwrap_or_else(|| source.url().to_string()),
                    concurrency: config.fetch.concurrency.into(),
                    dataset: source,
                })
                .await?;
            }
            Ok(())
        }
        Command::Build => {
            let output = config.files.output.clone();
            let description = config.map.description.clone().unwrap_or_else(|| {
                output.file_stem().map_or(config.map.name.clone(), |s| s.to_string_lossy().into_owned())
            });
            build::run(build::BuildOptions {
                input: config.files.tiles.clone(),
                output,
                zoom: config.zoom,
                style: config.build.style.clone(),
                mkgmap: config.build.mkgmap.clone(),
                work_dir: config.files.work_dir.clone(),
                typ: config.build.typ.clone(),
                dem: config.dem().map(PathBuf::from),
                max_nodes: config.build.max_nodes,
                map: mkgmap::MapInfo {
                    family_id: config.map.family_id,
                    name: config.map.name.clone(),
                    description,
                    code_page: config.map.code_page,
                },
            })
            .await
        }
    }
}
