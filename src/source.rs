//! 変換元タイルの読み込み（MBTiles またはタイルフォルダ）。

use crate::tile::TileCoord;
use anyhow::{Context, Result, bail};
use flate2::read::GzDecoder;
use rusqlite::{Connection, OpenFlags};
use std::io::Read;
use std::path::{Path, PathBuf};

pub enum TileSource {
    MbTiles(Connection),
    /// `{z}/{x}/{y}.pbf` 形式のフォルダ
    Dir(PathBuf),
}

impl TileSource {
    pub fn open(path: &Path) -> Result<Self> {
        if path.is_dir() {
            return Ok(Self::Dir(path.to_path_buf()));
        }
        let conn = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)
            .with_context(|| format!("入力を開けません: {}", path.display()))?;
        Ok(Self::MbTiles(conn))
    }

    /// 収録されているズームレベル（昇順）。
    pub fn zooms(&self) -> Result<Vec<u8>> {
        let mut zooms: Vec<u8> = match self {
            Self::MbTiles(conn) => conn
                .prepare("SELECT DISTINCT zoom_level FROM tiles")?
                .query_map([], |r| r.get(0))?
                .collect::<Result<_, _>>()?,
            Self::Dir(root) => std::fs::read_dir(root)?
                .filter_map(|e| e.ok()?.file_name().to_str()?.parse().ok())
                .collect(),
        };
        zooms.sort_unstable();
        Ok(zooms)
    }

    /// 指定ズームのタイルを `batch` 件ずつまとめて `f` に渡す。データは gzip を展開済み。
    pub fn for_each_batch(
        &self,
        z: u8,
        batch: usize,
        mut f: impl FnMut(Vec<(TileCoord, Vec<u8>)>) -> Result<()>,
    ) -> Result<()> {
        let mut buf = Vec::with_capacity(batch);
        let mut push = |t: TileCoord, data: Vec<u8>| -> Result<()> {
            buf.push((t, decompress(data)?));
            if buf.len() >= batch {
                f(std::mem::take(&mut buf))?;
            }
            Ok(())
        };
        match self {
            Self::MbTiles(conn) => {
                let mut stmt = conn.prepare(
                    "SELECT tile_column, tile_row, tile_data FROM tiles WHERE zoom_level = ?1",
                )?;
                let mut rows = stmt.query([z])?;
                while let Some(r) = rows.next()? {
                    push(TileCoord::from_tms(z, r.get(0)?, r.get(1)?), r.get(2)?)?;
                }
            }
            Self::Dir(root) => {
                for (t, path) in list_dir_tiles(&root.join(z.to_string()), z)? {
                    push(t, std::fs::read(&path)?)?;
                }
            }
        }
        if !buf.is_empty() {
            f(buf)?;
        }
        Ok(())
    }
}

fn list_dir_tiles(zdir: &Path, z: u8) -> Result<Vec<(TileCoord, PathBuf)>> {
    if !zdir.is_dir() {
        bail!("ズーム {z} のフォルダがありません: {}", zdir.display());
    }
    let mut out = Vec::new();
    for xe in std::fs::read_dir(zdir)? {
        let xe = xe?;
        let Some(x) = xe.file_name().to_str().and_then(|s| s.parse::<u32>().ok()) else {
            continue;
        };
        for ye in std::fs::read_dir(xe.path())? {
            let path = ye?.path();
            let is_tile = matches!(path.extension().and_then(|e| e.to_str()), Some("pbf" | "mvt"));
            let y = path.file_stem().and_then(|s| s.to_str()).and_then(|s| s.parse::<u32>().ok());
            if let (true, Some(y)) = (is_tile, y) {
                out.push((TileCoord { z, x, y }, path));
            }
        }
    }
    out.sort_by_key(|(t, _)| (t.y, t.x));
    Ok(out)
}

/// gzip 圧縮されていれば展開する（MBTiles の pbf は gzip 圧縮が慣例のため両対応にする）。
fn decompress(data: Vec<u8>) -> Result<Vec<u8>> {
    if data.starts_with(&[0x1f, 0x8b]) {
        let mut out = Vec::new();
        GzDecoder::new(&data[..]).read_to_end(&mut out)?;
        Ok(out)
    } else {
        Ok(data)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use flate2::{Compression, write::GzEncoder};
    use std::io::Write;

    #[test]
    fn gzip圧縮の有無を自動判定する() -> Result<()> {
        let raw = b"\x1a\x02abc".to_vec();
        let mut enc = GzEncoder::new(Vec::new(), Compression::default());
        enc.write_all(&raw)?;
        assert_eq!(decompress(enc.finish()?)?, raw);
        assert_eq!(decompress(raw.clone())?, raw);
        Ok(())
    }

    #[test]
    fn フォルダ形式のタイルを読む() -> Result<()> {
        let dir = tempfile::tempdir()?;
        for (x, y) in [(10, 20), (11, 20)] {
            let d = dir.path().join(format!("16/{x}"));
            std::fs::create_dir_all(&d)?;
            std::fs::write(d.join(format!("{y}.pbf")), [x as u8])?;
        }
        std::fs::write(dir.path().join("16/10/readme.txt"), "x")?;
        let src = TileSource::open(dir.path())?;
        assert_eq!(src.zooms()?, vec![16]);
        let mut got = Vec::new();
        src.for_each_batch(16, 1, |b| {
            got.extend(b);
            Ok(())
        })?;
        assert_eq!(
            got,
            vec![
                (TileCoord { z: 16, x: 10, y: 20 }, vec![10]),
                (TileCoord { z: 16, x: 11, y: 20 }, vec![11]),
            ]
        );
        Ok(())
    }
}
