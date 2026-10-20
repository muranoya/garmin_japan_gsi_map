//! 地理院の標高タイル（PNG）から mkgmap 用の HGT ファイルを作る。
//!
//! 標高タイルは RGB の 24bit 値 x に標高を 0.01m 単位で格納している
//! （x < 2^23: x * 0.01、x > 2^23: (x - 2^24) * 0.01、x = 2^23: データなし）。
//! <https://maps.gsi.go.jp/development/demtile.html>
//!
//! HGT は 1°×1° の範囲を 1 秒角間隔（3601×3601 点、両端を含む）で持つ、
//! 北から南への行順・ビッグエンディアン i16 のファイル。範囲外やデータなしは -32768。

use crate::source::TileSource;
use anyhow::{Context, Result, bail};
use rayon::prelude::*;
use std::collections::HashMap;
use std::f64::consts::PI;
use std::path::Path;

const TILE: usize = 256;
/// 1 秒角の HGT の一辺の点数。
const HGT_SIZE: usize = 3601;
const VOID: i16 = -32768;

/// 標高タイル群（同一ズーム）。値はメートル、データなしは NaN。
pub struct Dem {
    z: u8,
    tiles: HashMap<(u32, u32), Vec<f32>>,
}

impl Dem {
    /// 入力の最大ズームの標高タイルをすべて読み込む。
    pub fn load(src: &TileSource) -> Result<Self> {
        let z = *src.zooms()?.last().context("DEM の入力にタイルがありません")?;
        let mut tiles = HashMap::new();
        src.for_each_batch(z, 256, |batch| {
            let decoded: Vec<_> = batch
                .par_iter()
                .map(|(t, data)| decode_png(data).map(|v| ((t.x, t.y), v)).with_context(|| format!("DEM タイル {t}")))
                .collect::<Result<_>>()?;
            tiles.extend(decoded);
            Ok(())
        })?;
        Ok(Self { z, tiles })
    }

    /// 全世界ピクセル座標（ピクセル中心が整数）の値。
    fn pixel(&self, gx: i64, gy: i64) -> f32 {
        let (tx, ty) = (gx.div_euclid(TILE as i64), gy.div_euclid(TILE as i64));
        let Some(tile) = self.tiles.get(&(tx as u32, ty as u32)) else {
            return f32::NAN;
        };
        let (px, py) = (gx.rem_euclid(TILE as i64) as usize, gy.rem_euclid(TILE as i64) as usize);
        tile[py * TILE + px]
    }

    /// 経緯度の標高を双線形補間で求める。周囲 4 点のいずれかが欠けていれば最近傍の値を使う。
    pub fn sample(&self, lon: f64, lat: f64) -> Option<f32> {
        let size = (TILE as f64) * f64::from(1u32 << self.z);
        let x = (lon + 180.0) / 360.0 * size - 0.5;
        let lat_r = lat.to_radians();
        let y = (1.0 - lat_r.tan().asinh() / PI) / 2.0 * size - 0.5;
        let (x0, y0) = (x.floor(), y.floor());
        let (fx, fy) = ((x - x0) as f32, (y - y0) as f32);
        let (x0, y0) = (x0 as i64, y0 as i64);
        let v = [
            self.pixel(x0, y0),
            self.pixel(x0 + 1, y0),
            self.pixel(x0, y0 + 1),
            self.pixel(x0 + 1, y0 + 1),
        ];
        if v.iter().all(|e| e.is_finite()) {
            let top = v[0] * (1.0 - fx) + v[1] * fx;
            let bottom = v[2] * (1.0 - fx) + v[3] * fx;
            return Some(top * (1.0 - fy) + bottom * fy);
        }
        let nearest = v[usize::from(fx >= 0.5) + 2 * usize::from(fy >= 0.5)];
        nearest.is_finite().then_some(nearest)
    }
}

fn decode_png(data: &[u8]) -> Result<Vec<f32>> {
    let mut decoder = png::Decoder::new(std::io::Cursor::new(data));
    decoder.set_transformations(png::Transformations::EXPAND);
    let mut reader = decoder.read_info()?;
    let mut buf = vec![0; reader.output_buffer_size().context("PNG のサイズが不正です")?];
    let info = reader.next_frame(&mut buf)?;
    if (info.width as usize, info.height as usize) != (TILE, TILE) {
        bail!("標高タイルの大きさが 256x256 ではありません（{}x{}）", info.width, info.height);
    }
    let channels = match info.color_type {
        png::ColorType::Rgb => 3,
        png::ColorType::Rgba => 4,
        c => bail!("標高タイルの色形式に対応していません: {c:?}"),
    };
    Ok(buf[..info.buffer_size()]
        .chunks_exact(channels)
        .map(|px| decode_elevation(px[0], px[1], px[2]))
        .collect())
}

fn decode_elevation(r: u8, g: u8, b: u8) -> f32 {
    let x = (u32::from(r) << 16) | (u32::from(g) << 8) | u32::from(b);
    match x.cmp(&(1 << 23)) {
        std::cmp::Ordering::Less => x as f32 * 0.01,
        std::cmp::Ordering::Equal => f32::NAN,
        std::cmp::Ordering::Greater => (x as f32 - (1u32 << 24) as f32) * 0.01,
    }
}

/// 南西端が (lon, lat) の 1°区画の HGT ファイル名（例: N35E139.hgt）。
fn hgt_name(lon: i32, lat: i32) -> String {
    let ns = if lat >= 0 { 'N' } else { 'S' };
    let ew = if lon >= 0 { 'E' } else { 'W' };
    format!("{ns}{:02}{ew}{:03}.hgt", lat.abs(), lon.abs())
}

/// mkgmap は地図範囲より少し外側の HGT も参照するため、この幅（度）だけ広い範囲のファイルを用意する。
const FILE_MARGIN_DEG: f64 = 0.1;

/// `bounds`（min_lon, min_lat, max_lon, max_lat）を覆う HGT ファイルを `dir` に書き出す。
/// 範囲外の点はデータなしにする。書き出したファイル数を返す。
pub fn write_hgt(dem: &Dem, bounds: [f64; 4], dir: &Path) -> Result<usize> {
    std::fs::create_dir_all(dir)?;
    let [min_lon, min_lat, max_lon, max_lat] = bounds;
    let step = 1.0 / (HGT_SIZE - 1) as f64;
    let cells = |min: f64, max: f64| {
        ((min - FILE_MARGIN_DEG).floor() as i32)..=((max + FILE_MARGIN_DEG).floor() as i32)
    };
    let mut count = 0;
    for lat0 in cells(min_lat, max_lat) {
        for lon0 in cells(min_lon, max_lon) {
            let mut grid = vec![VOID; HGT_SIZE * HGT_SIZE];
            grid.par_chunks_mut(HGT_SIZE).enumerate().for_each(|(row, line)| {
                let lat = f64::from(lat0 + 1) - row as f64 * step;
                if lat < min_lat - step || lat > max_lat + step {
                    return;
                }
                for (col, v) in line.iter_mut().enumerate() {
                    let lon = f64::from(lon0) + col as f64 * step;
                    if lon < min_lon - step || lon > max_lon + step {
                        continue;
                    }
                    if let Some(e) = dem.sample(lon, lat) {
                        *v = e.round().clamp(-32767.0, 32767.0) as i16;
                    }
                }
            });
            let bytes: Vec<u8> = grid.iter().flat_map(|v| v.to_be_bytes()).collect();
            std::fs::write(dir.join(hgt_name(lon0, lat0)), bytes)?;
            count += 1;
        }
    }
    Ok(count)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 標高値のデコード() {
        assert!((decode_elevation(0, 0x98, 0xc5) - 391.09).abs() < 1e-3);
        assert!(decode_elevation(0x80, 0, 0).is_nan());
        // 負の標高: 2^24 - 100 → -1.00m
        assert!((decode_elevation(0xff, 0xff, 0x9c) + 1.0).abs() < 1e-4);
    }

    #[test]
    fn hgtファイル名() {
        assert_eq!(hgt_name(139, 35), "N35E139.hgt");
        assert_eq!(hgt_name(-5, -3), "S03W005.hgt");
    }

    fn flat_dem(z: u8, x: u32, y: u32, f: impl Fn(usize, usize) -> f32) -> Dem {
        let tile = (0..TILE * TILE).map(|i| f(i % TILE, i / TILE)).collect();
        Dem { z, tiles: HashMap::from([((x, y), tile)]) }
    }

    #[test]
    fn ピクセル中心の間を双線形補間する() {
        // z0 の 1 タイル。x 方向に 1px あたり 1m 増える
        let dem = flat_dem(0, 0, 0, |px, _| px as f32);
        // 経度 0 はちょうど 128px の境目 → 127 と 128 の中間
        assert!((dem.sample(0.0, 0.0).unwrap() - 127.5).abs() < 1e-3);
    }

    #[test]
    fn データなしの隣接点があれば最近傍を使う() {
        let dem = flat_dem(0, 0, 0, |px, _| if px >= 128 { f32::NAN } else { 10.0 });
        let lon_at = |px: f64| (px + 0.5) / 256.0 * 360.0 - 180.0;
        assert_eq!(dem.sample(lon_at(127.2), 0.0), Some(10.0));
        assert_eq!(dem.sample(lon_at(127.8), 0.0), None);
        assert_eq!(dem.sample(lon_at(200.0), 0.0), None);
    }

    #[test]
    fn hgtを書き出す() -> Result<()> {
        // z0 の 1 タイルを一定の 100m に
        let dem = flat_dem(0, 0, 0, |_, _| 100.0);
        let dir = tempfile::tempdir()?;
        let n = write_hgt(&dem, [139.2, 35.2, 139.4, 35.4], dir.path())?;
        assert_eq!(n, 1, "周囲の余白が隣の区画に掛からなければ 1 ファイル");
        let bytes = std::fs::read(dir.path().join("N35E139.hgt"))?;
        assert_eq!(bytes.len(), HGT_SIZE * HGT_SIZE * 2);
        let at = |row: usize, col: usize| {
            let i = (row * HGT_SIZE + col) * 2;
            i16::from_be_bytes([bytes[i], bytes[i + 1]])
        };
        // 35.3N, 139.3E は範囲内、35.9N は範囲外
        assert_eq!(at(2160, 1080), 100);
        assert_eq!(at(360, 1080), VOID);
        Ok(())
    }

    #[test]
    fn 区画の端に近い範囲では隣の区画のファイルも書き出す() -> Result<()> {
        let dem = flat_dem(0, 0, 0, |_, _| 100.0);
        let dir = tempfile::tempdir()?;
        assert_eq!(write_hgt(&dem, [139.2, 35.7, 139.4, 35.95], dir.path())?, 2);
        assert!(dir.path().join("N36E139.hgt").is_file());
        Ok(())
    }
}
