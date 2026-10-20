//! 設定ファイル（TOML）の読み込み。
//!
//! `fetch` / `build` の設定はすべてこのファイルで与える。項目の説明はリポジトリ直下の
//! `gsi2garmin.toml` を参照。相対パスは設定ファイルのあるフォルダを基準に解決する。

use crate::fetch::Dataset;
use crate::tile::BBox;
use anyhow::{Context, Result, bail, ensure};
use serde::Deserialize;
use std::path::{Path, PathBuf};

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    /// 地図にする範囲 [最小経度, 最小緯度, 最大経度, 最大緯度]（fetch.area = "all" のときは不要）
    bbox: Option<[f64; 4]>,
    /// 取得・変換に使うズームレベル（省略時はベクタタイルの最大ズーム）
    pub zoom: Option<u8>,
    pub files: Files,
    #[serde(default)]
    pub fetch: Fetch,
    #[serde(default)]
    pub map: Map,
    #[serde(default)]
    pub hillshade: Hillshade,
    #[serde(default)]
    pub build: Build,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Files {
    /// ベクタタイル（MBTiles。build では {z}/{x}/{y}.pbf 形式のフォルダも可）
    pub tiles: PathBuf,
    /// 標高タイル（陰影起伏を入れるときに使う）
    pub dem: Option<PathBuf>,
    /// 中間ファイルの置き場所（省略時は一時フォルダを使って終了時に削除する）
    pub work_dir: Option<PathBuf>,
    /// 出力する .img
    pub output: PathBuf,
}

#[derive(Debug, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Fetch {
    /// 同時リクエスト数
    pub concurrency: u16,
    /// ベクタタイルの URL テンプレート（省略時は地理院の URL）
    pub url: Option<String>,
    /// 取得するタイルの決め方
    pub area: Area,
}

impl Default for Fetch {
    fn default() -> Self {
        Self { concurrency: 4, url: None, area: Area::Bbox }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Area {
    /// bbox の範囲内のタイル
    Bbox,
    /// 地理院のタイル一覧に載っているすべてのタイル（bbox は無視する）
    All,
}

#[derive(Debug, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Map {
    /// 端末の地図一覧に表示する名前
    pub name: String,
    /// 地図の説明（省略時は出力ファイル名）
    pub description: Option<String>,
    /// 文字コード
    pub code_page: u16,
    /// Garmin のファミリー ID
    pub family_id: u16,
}

impl Default for Map {
    fn default() -> Self {
        Self { name: "地理院地図Vector".into(), description: None, code_page: 65001, family_id: 7251 }
    }
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Hillshade {
    /// 陰影起伏を入れるか
    pub enabled: bool,
    /// 標高タイルの種類
    pub source: DemSource,
    /// 標高タイルの URL テンプレート（省略時は source に応じた地理院の URL）
    pub url: Option<String>,
}

#[derive(Debug, Default, Clone, Copy, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DemSource {
    /// DEM10B（全国）
    #[default]
    Dem10b,
    /// DEM5A（航空レーザ測量の範囲のみ）
    Dem5a,
}

impl DemSource {
    pub fn dataset(self) -> Dataset {
        match self {
            Self::Dem10b => Dataset::Dem,
            Self::Dem5a => Dataset::Dem5a,
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Build {
    /// 1 区画（mkgmap のタイル）あたりのノード数の上限
    pub max_nodes: usize,
    /// mkgmap のスタイルフォルダ（省略時は同梱のもの）
    pub style: Option<PathBuf>,
    /// TYP ファイル（テキスト形式。省略時は同梱のもの）
    pub typ: Option<PathBuf>,
    /// mkgmap.jar（省略時は自動でダウンロードしてキャッシュする）
    pub mkgmap: Option<PathBuf>,
}

impl Default for Build {
    fn default() -> Self {
        Self { max_nodes: 2_000_000, style: None, typ: None, mkgmap: None }
    }
}

impl Config {
    pub fn load(path: &Path) -> Result<Self> {
        let text = std::fs::read_to_string(path)
            .with_context(|| format!("設定ファイルを読めません: {}", path.display()))?;
        let mut config: Config = toml::from_str(&text)
            .with_context(|| format!("設定ファイルの内容が不正です: {}", path.display()))?;
        config.validate()?;
        let base = path.parent().unwrap_or(Path::new(""));
        config.resolve_paths(base);
        Ok(config)
    }

    pub fn bbox(&self) -> Result<BBox> {
        let Some([a, b, c, d]) = self.bbox else {
            bail!("bbox を指定してください（fetch.area = \"all\" のときは不要）");
        };
        BBox::new(a, b, c, d)
    }

    /// 陰影起伏用の標高タイル。陰影起伏を入れない設定なら None。
    pub fn dem(&self) -> Option<&Path> {
        if self.hillshade.enabled { self.files.dem.as_deref() } else { None }
    }

    fn validate(&self) -> Result<()> {
        if self.bbox.is_some() || self.fetch.area == Area::Bbox {
            self.bbox()?;
        }
        let max = Dataset::Vector.max_zoom();
        if let Some(z) = self.zoom {
            ensure!(z <= max, "zoom は 0〜{max} で指定してください（{z}）");
        }
        ensure!(
            (1..=16).contains(&self.fetch.concurrency),
            "fetch.concurrency は 1〜16 で指定してください（{}）",
            self.fetch.concurrency
        );
        if self.hillshade.enabled && self.files.dem.is_none() {
            bail!("hillshade.enabled = true のときは files.dem（標高タイルの保存先）を指定してください");
        }
        Ok(())
    }

    /// 相対パスを設定ファイルのあるフォルダ基準にする。
    fn resolve_paths(&mut self, base: &Path) {
        let join = |p: &mut PathBuf| *p = base.join(&*p);
        join(&mut self.files.tiles);
        join(&mut self.files.output);
        for p in [
            &mut self.files.dem,
            &mut self.files.work_dir,
            &mut self.build.style,
            &mut self.build.typ,
            &mut self.build.mkgmap,
        ]
        .into_iter()
        .flatten()
        {
            join(p);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(s: &str) -> Result<Config> {
        let mut c: Config = toml::from_str(s)?;
        c.validate()?;
        c.resolve_paths(Path::new("conf"));
        Ok(c)
    }

    const MINIMAL: &str = r#"
        bbox = [138.95, 35.70, 139.25, 35.90]
        [files]
        tiles = "t.mbtiles"
        output = "o.img"
    "#;

    #[test]
    fn 同梱の設定ファイルを読める() -> Result<()> {
        let c = Config::load(&Path::new(env!("CARGO_MANIFEST_DIR")).join("gsi2garmin.toml"))?;
        assert!(c.files.output.ends_with("data/output/okutama.img"));
        Ok(())
    }

    #[test]
    fn 省略した項目は既定値になる() -> Result<()> {
        let c = parse(MINIMAL)?;
        assert_eq!(c.fetch.concurrency, 4);
        assert_eq!(c.fetch.area, Area::Bbox);
        assert_eq!(c.map.code_page, 65001);
        assert_eq!(c.build.max_nodes, 2_000_000);
        assert!(!c.hillshade.enabled);
        assert_eq!(c.dem(), None);
        Ok(())
    }

    #[test]
    fn 相対パスは設定ファイルの場所を基準にする() -> Result<()> {
        let c = parse(MINIMAL)?;
        assert_eq!(c.files.tiles, Path::new("conf/t.mbtiles"));
        assert_eq!(c.files.output, Path::new("conf/o.img"));
        Ok(())
    }

    #[test]
    fn 陰影起伏を入れるときだけ標高タイルを使う() -> Result<()> {
        let s = MINIMAL.replace("output", "dem = \"d.mbtiles\"\noutput");
        assert_eq!(parse(&s)?.dem(), None);
        let c = parse(&format!("{s}\n[hillshade]\nenabled = true\nsource = \"dem5a\""))?;
        assert_eq!(c.dem(), Some(Path::new("conf/d.mbtiles")));
        assert_eq!(c.hillshade.source.dataset(), Dataset::Dem5a);
        Ok(())
    }

    #[test]
    fn 不正な設定はエラーにする() {
        // 標高タイルの保存先がない
        assert!(parse(&format!("{MINIMAL}\n[hillshade]\nenabled = true")).is_err());
        // 知らない項目（書き間違い）
        assert!(parse(&format!("{MINIMAL}\n[map]\ncodepage = 932")).is_err());
        // 範囲の最小値と最大値が逆
        assert!(parse(&MINIMAL.replace("138.95, 35.70, 139.25", "139.25, 35.70, 138.95")).is_err());
        // 範囲がない
        assert!(parse(&MINIMAL.replace("bbox = [138.95, 35.70, 139.25, 35.90]", "")).is_err());
    }

    #[test]
    fn 全域を取得するときは範囲を省略できる() -> Result<()> {
        let s = MINIMAL.replace("bbox = [138.95, 35.70, 139.25, 35.90]", "");
        let c = parse(&format!("{s}\n[fetch]\narea = \"all\""))?;
        assert_eq!(c.fetch.area, Area::All);
        Ok(())
    }
}
