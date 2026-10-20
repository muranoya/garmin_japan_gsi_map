//! 地理院ベクタタイルのダウンロード。

use crate::mbtiles::{FetchRecord, MbTiles};
use crate::tile::{BBox, TileCoord};
use anyhow::{Result, bail};
use futures::{StreamExt, stream};
use indicatif::{ProgressBar, ProgressStyle};
use reqwest::{Client, StatusCode, header};
use std::path::PathBuf;
use std::time::Duration;

/// 取得するデータの種類。URL・既定ズーム・MBTiles のメタデータを決める。
#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum Dataset {
    /// 地理院地図Vector（ベクタタイル, 最大 z16）
    Vector,
    /// 標高タイル DEM10B（全国, PNG, 最大 z14）。陰影起伏用
    Dem,
    /// 標高タイル DEM5A（航空レーザ測量の範囲のみ, PNG, 最大 z15）
    Dem5a,
}

impl Dataset {
    pub fn url(self) -> &'static str {
        match self {
            Self::Vector => "https://cyberjapandata.gsi.go.jp/xyz/experimental_bvmap/{z}/{x}/{y}.pbf",
            Self::Dem => "https://cyberjapandata.gsi.go.jp/xyz/dem_png/{z}/{x}/{y}.png",
            Self::Dem5a => "https://cyberjapandata.gsi.go.jp/xyz/dem5a_png/{z}/{x}/{y}.png",
        }
    }

    pub fn max_zoom(self) -> u8 {
        match self {
            Self::Vector => 16,
            Self::Dem => 14,
            Self::Dem5a => 15,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Vector => "地理院地図Vector",
            Self::Dem => "地理院 標高タイル DEM10B",
            Self::Dem5a => "地理院 標高タイル DEM5A",
        }
    }

    pub fn format(self) -> &'static str {
        match self {
            Self::Vector => "pbf",
            Self::Dem | Self::Dem5a => "png",
        }
    }
}

/// 何タイルごとにコミットするか。中断時に失うのは最大でこの件数分。
const COMMIT_EVERY: usize = 200;
const MAX_RETRIES: u32 = 5;

pub struct FetchOptions {
    pub bbox: BBox,
    pub zooms: Vec<u8>,
    pub output: PathBuf,
    pub url_template: String,
    pub concurrency: usize,
    pub dataset: Dataset,
}

enum Outcome {
    Saved(FetchRecord),
    Failed(TileCoord, anyhow::Error),
}

pub async fn run(opts: FetchOptions) -> Result<()> {
    let db = MbTiles::open(&opts.output)?;

    let mut targets = Vec::new();
    for &z in &opts.zooms {
        let done = db.fetched_tiles(z)?;
        let total = opts.bbox.tile_count(z);
        let before = targets.len();
        targets.extend(opts.bbox.tiles(z).filter(|t| !done.contains(t)));
        eprintln!(
            "z{z}: 範囲内 {total} タイル / 取得済み {} / 今回取得 {}",
            total - (targets.len() - before) as u64,
            targets.len() - before
        );
    }
    if targets.is_empty() {
        db.update_metadata(&opts.bbox, opts.dataset.name(), opts.dataset.format(), &opts.url_template)?;
        eprintln!("すべて取得済みです");
        return Ok(());
    }

    let client = Client::builder()
        .user_agent(concat!("gsi2garmin/", env!("CARGO_PKG_VERSION"), " (personal use)"))
        .timeout(Duration::from_secs(60))
        .build()?;

    let pb = ProgressBar::new(targets.len() as u64);
    pb.set_style(ProgressStyle::with_template(
        "{bar:40} {pos}/{len} ({per_sec}, 残り {eta}) {msg}",
    )?);

    let mut results = stream::iter(targets)
        .map(|tile| {
            let client = &client;
            let url = expand_url(&opts.url_template, tile);
            async move {
                match fetch_with_retry(client, &url, tile).await {
                    Ok(rec) => Outcome::Saved(rec),
                    Err(e) => Outcome::Failed(tile, e),
                }
            }
        })
        .buffer_unordered(opts.concurrency);

    let ctrl_c = tokio::signal::ctrl_c();
    tokio::pin!(ctrl_c);

    let (mut saved, mut missing, mut failed, mut pending) = (0usize, 0usize, 0usize, 0usize);
    let mut interrupted = false;
    db.begin()?;
    loop {
        let outcome = tokio::select! {
            o = results.next() => match o {
                Some(o) => o,
                None => break,
            },
            _ = &mut ctrl_c => {
                interrupted = true;
                break;
            }
        };
        match outcome {
            Outcome::Saved(rec) => {
                if rec.data.is_some() {
                    saved += 1;
                } else {
                    missing += 1;
                }
                db.put(&rec)?;
                pending += 1;
                if pending >= COMMIT_EVERY {
                    db.commit()?;
                    db.begin()?;
                    pending = 0;
                }
            }
            Outcome::Failed(tile, e) => {
                failed += 1;
                pb.println(format!("取得失敗 {tile}: {e:#}"));
            }
        }
        pb.inc(1);
    }
    db.commit()?;
    db.update_metadata(&opts.bbox, opts.dataset.name(), opts.dataset.format(), &opts.url_template)?;
    pb.finish_and_clear();

    eprintln!("保存 {saved} / データなし(404) {missing} / 失敗 {failed}");
    if interrupted {
        eprintln!("中断しました。同じコマンドを再実行すると続きから取得します");
    } else if failed > 0 {
        eprintln!("失敗したタイルは再実行で再取得します");
    }
    Ok(())
}

fn expand_url(template: &str, t: TileCoord) -> String {
    template
        .replace("{z}", &t.z.to_string())
        .replace("{x}", &t.x.to_string())
        .replace("{y}", &t.y.to_string())
}

/// 通信エラー・429・5xx は指数バックオフで再試行する。404 はデータなしとして成功扱い。
async fn fetch_with_retry(client: &Client, url: &str, tile: TileCoord) -> Result<FetchRecord> {
    let mut attempt = 0;
    loop {
        attempt += 1;
        let err = match fetch_once(client, url, tile).await {
            Ok(Some(rec)) => return Ok(rec),
            Ok(None) => anyhow::anyhow!("サーバが一時的に応答できません"),
            Err(e) => e,
        };
        if attempt >= MAX_RETRIES {
            return Err(err);
        }
        tokio::time::sleep(Duration::from_secs(1 << attempt)).await;
    }
}

/// 再試行すべき応答なら `Ok(None)` を返す。
async fn fetch_once(client: &Client, url: &str, tile: TileCoord) -> Result<Option<FetchRecord>> {
    let resp = client.get(url).send().await?;
    let status = resp.status();
    if status == StatusCode::TOO_MANY_REQUESTS || status.is_server_error() {
        return Ok(None);
    }
    let header_str = |name| {
        resp.headers()
            .get(name)
            .and_then(|v: &header::HeaderValue| v.to_str().ok())
            .map(str::to_owned)
    };
    let etag = header_str(header::ETAG);
    let last_modified = header_str(header::LAST_MODIFIED);
    let data = match status {
        StatusCode::OK => Some(resp.bytes().await?.to_vec()),
        StatusCode::NOT_FOUND => None,
        s => bail!("想定外の HTTP ステータス {s}: {url}"),
    };
    Ok(Some(FetchRecord { tile, status: status.as_u16(), data, etag, last_modified }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn urlテンプレートの展開() {
        let t = TileCoord { z: 16, x: 58089, y: 25777 };
        assert_eq!(
            expand_url(Dataset::Vector.url(), t),
            "https://cyberjapandata.gsi.go.jp/xyz/experimental_bvmap/16/58089/25777.pbf"
        );
    }
}
