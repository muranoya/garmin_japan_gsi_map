//! XYZ タイル座標の計算（Web メルカトル）。

use anyhow::{Result, bail};
use std::f64::consts::PI;
use std::fmt;
use std::str::FromStr;

/// XYZ 方式のタイル座標（y は北から南へ増える）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TileCoord {
    pub z: u8,
    pub x: u32,
    pub y: u32,
}

impl TileCoord {
    /// MBTiles が採用する TMS 方式の行番号（y は南から北へ増える）。
    pub fn tms_row(&self) -> u32 {
        (1u32 << self.z) - 1 - self.y
    }

    pub fn from_tms(z: u8, x: u32, tms_row: u32) -> Self {
        Self { z, x, y: (1u32 << z) - 1 - tms_row }
    }
}

impl fmt::Display for TileCoord {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}/{}/{}", self.z, self.x, self.y)
    }
}

/// 経緯度の矩形範囲（WGS84）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BBox {
    pub min_lon: f64,
    pub min_lat: f64,
    pub max_lon: f64,
    pub max_lat: f64,
}

/// Web メルカトルで表現できる緯度の上限。
const MAX_LAT: f64 = 85.051_128_779_806_59;

impl BBox {
    /// 値の範囲を確かめて作る。
    pub fn new(min_lon: f64, min_lat: f64, max_lon: f64, max_lat: f64) -> Result<Self> {
        let s = format!("{min_lon},{min_lat},{max_lon},{max_lat}");
        if !(-180.0..=180.0).contains(&min_lon) || !(-180.0..=180.0).contains(&max_lon) {
            bail!("経度が範囲外です: {s}");
        }
        if !(-MAX_LAT..=MAX_LAT).contains(&min_lat) || !(-MAX_LAT..=MAX_LAT).contains(&max_lat) {
            bail!("緯度が範囲外です: {s}");
        }
        if min_lon >= max_lon || min_lat >= max_lat {
            bail!("bbox の最小値が最大値以上になっています: {s}");
        }
        Ok(BBox { min_lon, min_lat, max_lon, max_lat })
    }

    /// 2 つの範囲を包含する範囲を返す。
    pub fn union(&self, other: &BBox) -> BBox {
        BBox {
            min_lon: self.min_lon.min(other.min_lon),
            min_lat: self.min_lat.min(other.min_lat),
            max_lon: self.max_lon.max(other.max_lon),
            max_lat: self.max_lat.max(other.max_lat),
        }
    }

    /// 指定ズームでこの範囲に掛かるタイルを列挙する。
    pub fn tiles(&self, z: u8) -> impl Iterator<Item = TileCoord> + use<> {
        let (x0, y0) = lonlat_to_tile(self.min_lon, self.max_lat, z);
        let (x1, y1) = lonlat_to_tile(self.max_lon, self.min_lat, z);
        (y0..=y1).flat_map(move |y| (x0..=x1).map(move |x| TileCoord { z, x, y }))
    }

    pub fn tile_count(&self, z: u8) -> u64 {
        let (x0, y0) = lonlat_to_tile(self.min_lon, self.max_lat, z);
        let (x1, y1) = lonlat_to_tile(self.max_lon, self.min_lat, z);
        u64::from(x1 - x0 + 1) * u64::from(y1 - y0 + 1)
    }
}

impl FromStr for BBox {
    type Err = anyhow::Error;

    /// `minlon,minlat,maxlon,maxlat` 形式を読む。
    fn from_str(s: &str) -> Result<Self> {
        let v: Vec<f64> = s
            .split(',')
            .map(|p| p.trim().parse::<f64>())
            .collect::<Result<_, _>>()?;
        let [min_lon, min_lat, max_lon, max_lat] = v[..] else {
            bail!("bbox は minlon,minlat,maxlon,maxlat の 4 値で指定してください: {s}");
        };
        BBox::new(min_lon, min_lat, max_lon, max_lat)
    }
}

impl fmt::Display for BBox {
    /// MBTiles の `bounds` メタデータと同じ書式。
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{},{},{},{}", self.min_lon, self.min_lat, self.max_lon, self.max_lat)
    }
}

/// 経緯度を含むタイルの (x, y) を返す。範囲の端はタイル番号の上限に丸める。
pub fn lonlat_to_tile(lon: f64, lat: f64, z: u8) -> (u32, u32) {
    let n = f64::from(1u32 << z);
    let lat = lat.clamp(-MAX_LAT, MAX_LAT).to_radians();
    let x = ((lon + 180.0) / 360.0 * n).floor();
    let y = ((1.0 - lat.tan().asinh() / PI) / 2.0 * n).floor();
    let max = n - 1.0;
    (x.clamp(0.0, max) as u32, y.clamp(0.0, max) as u32)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 奥多摩付近のタイル番号() {
        // 実際に地理院サーバから取得できたタイルと一致すること
        assert_eq!(lonlat_to_tile(139.096, 35.809, 16), (58089, 25777));
        assert_eq!(lonlat_to_tile(139.096, 35.809, 14), (14522, 6444));
    }

    #[test]
    fn 範囲の端はタイル番号の上限に丸める() {
        assert_eq!(lonlat_to_tile(180.0, -90.0, 2), (3, 3));
        assert_eq!(lonlat_to_tile(-180.0, 90.0, 2), (0, 0));
    }

    #[test]
    fn tms変換は往復で一致する() {
        let t = TileCoord { z: 16, x: 58089, y: 25777 };
        assert_eq!(t.tms_row(), 65535 - 25777);
        assert_eq!(TileCoord::from_tms(16, t.x, t.tms_row()), t);
    }

    #[test]
    fn bboxのタイル列挙と件数が一致する() {
        let b: BBox = "138.95,35.70,139.25,35.90".parse().unwrap();
        for z in [12, 14, 16] {
            assert_eq!(b.tiles(z).count() as u64, b.tile_count(z));
        }
    }

    #[test]
    fn 不正なbboxはエラー() {
        assert!("1,2,3".parse::<BBox>().is_err());
        assert!("139.2,35.7,139.0,35.9".parse::<BBox>().is_err());
        assert!("139,35.7,200,35.9".parse::<BBox>().is_err());
    }
}
