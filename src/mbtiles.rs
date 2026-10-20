//! MBTiles（SQLite）へのタイル保存。
//!
//! 標準の `metadata` / `tiles` テーブルに加え、取得状況を記録する
//! `tile_fetch` テーブルを持つ。404（データなし）のタイルもここに記録し、
//! 再実行時に取得済み・データなしのタイルを再取得しないようにする。
//!
//! タイルデータは地理院サーバから受け取ったバイト列をそのまま保存する
//! （地理院の pbf は gzip 圧縮されていない）。

use crate::tile::{BBox, TileCoord};
use anyhow::{Context, Result};
use rusqlite::{Connection, OptionalExtension, params};
use std::collections::HashSet;
use std::path::Path;

pub struct MbTiles {
    conn: Connection,
}

/// 1 タイル分の取得結果。
pub struct FetchRecord {
    pub tile: TileCoord,
    pub status: u16,
    /// 404 の場合は `None`。
    pub data: Option<Vec<u8>>,
    pub etag: Option<String>,
    pub last_modified: Option<String>,
}

impl MbTiles {
    /// ファイルを開く。存在しなければスキーマを作成する。
    pub fn open(path: &Path) -> Result<Self> {
        let conn = Connection::open(path)
            .with_context(|| format!("MBTiles を開けません: {}", path.display()))?;
        conn.execute_batch(
            "PRAGMA journal_mode = WAL;
             PRAGMA synchronous = NORMAL;
             CREATE TABLE IF NOT EXISTS metadata (name TEXT PRIMARY KEY, value TEXT);
             CREATE TABLE IF NOT EXISTS tiles (
                 zoom_level INTEGER, tile_column INTEGER, tile_row INTEGER, tile_data BLOB,
                 PRIMARY KEY (zoom_level, tile_column, tile_row)
             );
             CREATE TABLE IF NOT EXISTS tile_fetch (
                 zoom_level INTEGER, tile_column INTEGER, tile_row INTEGER,
                 status INTEGER NOT NULL,
                 etag TEXT, last_modified TEXT,
                 fetched_at INTEGER NOT NULL,
                 PRIMARY KEY (zoom_level, tile_column, tile_row)
             );",
        )?;
        Ok(Self { conn })
    }

    /// 指定ズームで取得済み（200 または 404 を記録済み）のタイル。
    pub fn fetched_tiles(&self, z: u8) -> Result<HashSet<TileCoord>> {
        let mut stmt = self
            .conn
            .prepare("SELECT tile_column, tile_row FROM tile_fetch WHERE zoom_level = ?1")?;
        let rows = stmt.query_map([z], |r| Ok(TileCoord::from_tms(z, r.get(0)?, r.get(1)?)))?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    pub fn begin(&self) -> Result<()> {
        self.conn.execute_batch("BEGIN")?;
        Ok(())
    }

    pub fn commit(&self) -> Result<()> {
        self.conn.execute_batch("COMMIT")?;
        Ok(())
    }

    pub fn put(&self, rec: &FetchRecord) -> Result<()> {
        let t = rec.tile;
        if let Some(data) = &rec.data {
            self.conn.execute(
                "INSERT OR REPLACE INTO tiles VALUES (?1, ?2, ?3, ?4)",
                params![t.z, t.x, t.tms_row(), data],
            )?;
        }
        self.conn.execute(
            "INSERT OR REPLACE INTO tile_fetch VALUES (?1, ?2, ?3, ?4, ?5, ?6, unixepoch())",
            params![t.z, t.x, t.tms_row(), rec.status, rec.etag, rec.last_modified],
        )?;
        Ok(())
    }

    /// 取得範囲とズームに合わせてメタデータを更新する。
    pub fn update_metadata(&self, bbox: &BBox, source_url: &str) -> Result<()> {
        let bounds = match self.metadata("bounds")? {
            Some(s) => s.parse::<BBox>().map_or(*bbox, |old| old.union(bbox)),
            None => *bbox,
        };
        let (minzoom, maxzoom): (Option<u8>, Option<u8>) = self.conn.query_row(
            "SELECT MIN(zoom_level), MAX(zoom_level) FROM tiles",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?;
        let center_lon = (bounds.min_lon + bounds.max_lon) / 2.0;
        let center_lat = (bounds.min_lat + bounds.max_lat) / 2.0;

        let mut entries = vec![
            ("name", "地理院地図Vector".to_string()),
            ("format", "pbf".to_string()),
            ("type", "baselayer".to_string()),
            ("attribution", "国土地理院".to_string()),
            ("source", source_url.to_string()),
            ("bounds", bounds.to_string()),
        ];
        if let (Some(min), Some(max)) = (minzoom, maxzoom) {
            entries.push(("minzoom", min.to_string()));
            entries.push(("maxzoom", max.to_string()));
            entries.push(("center", format!("{center_lon},{center_lat},{max}")));
        }
        for (name, value) in entries {
            self.conn.execute(
                "INSERT OR REPLACE INTO metadata VALUES (?1, ?2)",
                params![name, value],
            )?;
        }
        Ok(())
    }

    fn metadata(&self, name: &str) -> Result<Option<String>> {
        Ok(self
            .conn
            .query_row("SELECT value FROM metadata WHERE name = ?1", [name], |r| r.get(0))
            .optional()?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(tile: TileCoord, status: u16) -> FetchRecord {
        FetchRecord {
            tile,
            status,
            data: (status == 200).then(|| vec![1, 2, 3]),
            etag: None,
            last_modified: None,
        }
    }

    #[test]
    fn 取得記録は200も404も取得済みとして扱う() -> Result<()> {
        let db = MbTiles::open(Path::new(":memory:"))?;
        let a = TileCoord { z: 16, x: 58089, y: 25777 };
        let b = TileCoord { z: 16, x: 58090, y: 25777 };
        db.begin()?;
        db.put(&record(a, 200))?;
        db.put(&record(b, 404))?;
        db.commit()?;

        let fetched = db.fetched_tiles(16)?;
        assert_eq!(fetched, HashSet::from([a, b]));
        assert!(db.fetched_tiles(15)?.is_empty());

        // tiles テーブルには 200 のタイルだけが TMS の行番号で入る
        let rows: Vec<(u32, u32)> = db
            .conn
            .prepare("SELECT tile_column, tile_row FROM tiles")?
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
            .collect::<Result<_, _>>()?;
        assert_eq!(rows, vec![(a.x, a.tms_row())]);
        Ok(())
    }

    #[test]
    fn メタデータのboundsは既存範囲と合成する() -> Result<()> {
        let db = MbTiles::open(Path::new(":memory:"))?;
        db.update_metadata(&"139,35.7,139.1,35.8".parse()?, "u")?;
        db.update_metadata(&"139.05,35.6,139.2,35.75".parse()?, "u")?;
        assert_eq!(db.metadata("bounds")?.as_deref(), Some("139,35.6,139.2,35.8"));
        Ok(())
    }
}
