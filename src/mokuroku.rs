//! 地理院のタイル一覧（`mokuroku.csv.gz`）。
//!
//! タイルの URL と同じ場所に置かれていて、サーバにあるタイルのパス・更新時刻・サイズ・MD5 が並ぶ。
//! 例: `16/58089/25777.pbf,1700456738,12345,8d98204e2b3cff9498a418261726ef48`

use crate::tile::TileCoord;
use anyhow::{Context, Result, bail};
use flate2::read::GzDecoder;
use reqwest::Client;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::time::Duration;

/// タイルの URL テンプレートに対応する一覧の URL と、保存するファイル名を返す。
/// `https://.../xyz/experimental_bvmap/{z}/{x}/{y}.pbf`
/// → (`https://.../xyz/experimental_bvmap/mokuroku.csv.gz`, `experimental_bvmap-mokuroku.csv.gz`)
fn locate(url_template: &str) -> Result<(String, String)> {
    let Some((base, _)) = url_template.split_once("{z}/{x}/{y}.") else {
        bail!("URL が {{z}}/{{x}}/{{y}}.<拡張子> で終わっていないのでタイル一覧の場所が分かりません: {url_template}");
    };
    let name = base.trim_end_matches('/').rsplit('/').next().unwrap_or_default();
    if name.is_empty() || name.contains(':') {
        bail!("URL からデータの名前が分かりません: {url_template}");
    }
    Ok((format!("{base}mokuroku.csv.gz"), format!("{name}-mokuroku.csv.gz")))
}

/// 一覧に載っているタイルを返す。一覧は `dir` に保存し、保存済みならそれを使う。
pub async fn load(client: &Client, url_template: &str, dir: &Path) -> Result<Vec<TileCoord>> {
    let (url, name) = locate(url_template)?;
    let path = dir.join(name);
    if path.exists() {
        eprintln!("保存済みのタイル一覧を使います（最新にするには削除してください）: {}", path.display());
    } else {
        eprintln!("タイル一覧を取得します: {url}");
        download(client, &url, &path).await?;
    }
    let file = std::fs::File::open(&path).with_context(|| format!("{} を開けません", path.display()))?;
    parse(BufReader::new(GzDecoder::new(file))).with_context(|| format!("タイル一覧 {}", path.display()))
}

/// 中断しても壊れたファイルが残らないよう、別名で書いてから置き換える。
async fn download(client: &Client, url: &str, path: &Path) -> Result<()> {
    // 一覧は数十 MB あるので、タイル用の時間制限より長く待つ
    let req = client.get(url).timeout(Duration::from_secs(600));
    let data = req.send().await?.error_for_status()?.bytes().await?;
    std::fs::create_dir_all(path.parent().unwrap_or(Path::new("")))?;
    let mut part = PathBuf::from(path);
    part.as_mut_os_string().push(".part");
    std::fs::write(&part, &data)?;
    std::fs::rename(&part, path)?;
    Ok(())
}

fn parse(reader: impl BufRead) -> Result<Vec<TileCoord>> {
    let mut tiles = Vec::new();
    for (i, line) in reader.lines().enumerate() {
        let line = line?;
        if line.is_empty() {
            continue;
        }
        let tile = parse_path(line.split(',').next().unwrap_or_default())
            .with_context(|| format!("{} 行目を読めません: {line}", i + 1))?;
        tiles.push(tile);
    }
    Ok(tiles)
}

/// `16/58089/25777.pbf` を読む。
fn parse_path(path: &str) -> Result<TileCoord> {
    let stem = path.rsplit_once('.').map_or(path, |(s, _)| s);
    let [z, x, y] = stem.split('/').collect::<Vec<_>>()[..] else {
        bail!("z/x/y の形式ではありません");
    };
    Ok(TileCoord { z: z.parse()?, x: x.parse()?, y: y.parse()? })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 一覧の場所はタイルのurlから決める() -> Result<()> {
        let (url, name) = locate("https://cyberjapandata.gsi.go.jp/xyz/experimental_bvmap/{z}/{x}/{y}.pbf")?;
        assert_eq!(url, "https://cyberjapandata.gsi.go.jp/xyz/experimental_bvmap/mokuroku.csv.gz");
        assert_eq!(name, "experimental_bvmap-mokuroku.csv.gz");
        assert!(locate("https://example.com/tiles?z={z}&x={x}&y={y}").is_err());
        assert!(locate("https://{z}/{x}/{y}.pbf").is_err());
        Ok(())
    }

    #[test]
    fn 一覧を読む() -> Result<()> {
        let text = "4/14/5.pbf,1779431075,8787,8d98\n16/58089/25777.pbf,1700456738,56,ff61\n\n";
        assert_eq!(
            parse(text.as_bytes())?,
            [TileCoord { z: 4, x: 14, y: 5 }, TileCoord { z: 16, x: 58089, y: 25777 }]
        );
        assert!(parse("16/58089.pbf,1,2,3\n".as_bytes()).is_err());
        Ok(())
    }
}
