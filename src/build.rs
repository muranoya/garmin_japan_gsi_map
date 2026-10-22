//! ベクタタイル → OSM XML（区画ごと）→ mkgmap → .img の変換。

use crate::dem::{self, Dem};
use crate::geom::{self, Bounds, Grid, P};
use crate::mkgmap::{self, MapInfo};
use crate::mvt::{self, Geometry};
use crate::osm::{self, Feature, Item, Tags};
use crate::source::TileSource;
use crate::split;
use crate::tile::{self, TileCoord};
use anyhow::{Context, Result, bail, ensure};
use flate2::{Compression, write::GzEncoder};
use indicatif::{ProgressBar, ProgressStyle};
use rayon::prelude::*;
use std::collections::{BTreeMap, HashMap};
use std::fs::File;
use std::io::BufWriter;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Instant;

/// タイル境界で線を結合するときの端点の許容距離（ピクセル）。z16 で 8px ≒ 1m。
const MERGE_TOLERANCE_PX: f64 = 8.0;

/// 縮小表示用のデータを取るズーム。地理院は低ズームのタイルに小縮尺の地図向けのデータ（主な山名・
/// 100m 間隔の等高線・主な道路や河川など）を収録しているので、基準ズームより低いこれらのズームの地物を
/// `gsi_zoom` タグ付きで加え、どの段にどれを使うかはスタイルで決める。
pub const OVERVIEW_ZOOMS: [u8; 4] = [8, 10, 12, 13];

/// 領域（まとめて処理するタイルの集まり）のデータ量の上限を、区画のノード数の上限から決めるための係数。
/// 圧縮されたままのタイル 1 バイトがおよそ 0.17 ノードになる（奥多摩〜東京で計測）ので、
/// 領域 1 つが区画 1 つ分程度になるようにする。
const BYTES_PER_NODE: u64 = 6;

pub struct BuildOptions {
    pub input: PathBuf,
    pub output: PathBuf,
    pub zoom: Option<u8>,
    pub style: Option<PathBuf>,
    pub mkgmap: Option<PathBuf>,
    pub work_dir: Option<PathBuf>,
    pub typ: Option<PathBuf>,
    pub dem: Option<PathBuf>,
    /// 1 区画（mkgmap のタイル）あたりのノード数の上限
    pub max_nodes: usize,
    pub map: MapInfo,
}

pub async fn run(opts: BuildOptions) -> Result<()> {
    let jar = mkgmap::ensure_jar(opts.mkgmap.clone()).await?;

    // 作業フォルダ。指定がなければ一時フォルダを使い、終了時に消す
    let tmp;
    let work = match &opts.work_dir {
        Some(d) => {
            // 前回の中間ファイルが残っていると容量を食うので作り直す
            for sub in ["osm", "hgt", "out"] {
                let p = d.join(sub);
                if p.exists() {
                    std::fs::remove_dir_all(&p)
                        .with_context(|| format!("作業フォルダを削除できません: {}", p.display()))?;
                }
            }
            std::fs::create_dir_all(d)?;
            d.clone()
        }
        None => {
            tmp = tempfile::tempdir()?;
            tmp.path().to_path_buf()
        }
    };

    let started = Instant::now();
    let src = TileSource::open(&opts.input)?;
    let zooms = src.zooms()?;
    let z = match opts.zoom {
        Some(z) if zooms.contains(&z) => z,
        Some(z) => bail!("入力にズーム {z} のタイルがありません（収録: {zooms:?}）"),
        None => *zooms.last().context("入力にタイルがありません")?,
    };
    let tiles = src.tile_sizes(z)?;
    let grid = Grid { z, extent: first_extent(&src, &tiles)?.context("入力に地物がありません")? };
    let area = tiles_bounds(grid, tiles.iter().map(|(t, _)| t));
    let (west, north) = grid.to_lonlat(area.min);
    let (east, south) = grid.to_lonlat(area.max);
    let overview_zooms: Vec<u8> = OVERVIEW_ZOOMS.into_iter().filter(|lz| *lz < z && zooms.contains(lz)).collect();
    if !overview_zooms.is_empty() {
        eprintln!("縮小表示用に z{overview_zooms:?} のタイルも使います");
    }
    drop(src);

    // 全地物をメモリに載せずに済むよう、タイルを領域に分けて領域ごとに処理する
    let tile_count = tiles.len();
    let regions = split::plan_regions(tiles, opts.max_nodes as u64 * BYTES_PER_NODE);
    eprintln!("z{z}: {tile_count} タイルを {} 領域に分けて処理します", regions.len());
    let osm_dir = work.join("osm");
    std::fs::create_dir_all(&osm_dir)?;
    let next_id = AtomicUsize::new(0);
    let pb = ProgressBar::new(regions.len() as u64);
    pb.set_style(ProgressStyle::with_template("{bar:40} {pos}/{len} 領域 (残り {eta})")?);
    let outs: Vec<RegionOut> = regions
        .par_iter()
        .map(|tiles| {
            let out = process_region(&opts, grid, tiles, &overview_zooms, &osm_dir, &next_id);
            pb.inc(1);
            out
        })
        .collect::<Result<_>>()?;
    pb.finish_and_clear();
    let mut osm_paths: Vec<PathBuf> = outs.iter().flat_map(|o| o.paths.iter().cloned()).collect();
    osm_paths.sort();
    let sum = |f: fn(&RegionOut) -> usize| outs.iter().map(f).sum::<usize>();
    eprintln!(
        "線 {} 断片 → {} 本に結合 / 地物 {} 件",
        sum(|o| o.pieces),
        sum(|o| o.lines),
        sum(|o| o.features)
    );
    let size: u64 = osm_paths.iter().map(|p| std::fs::metadata(p).map_or(0, |m| m.len())).sum();
    eprintln!(
        "タイルの読み込み・結合・OSM XML 書き出し: {} 区画 {:.1?}（gzip 圧縮後 {:.1} MB）",
        osm_paths.len(),
        started.elapsed(),
        size as f64 / 1e6
    );

    let style_dir = match &opts.style {
        Some(d) => d.clone(),
        None => {
            let d = work.join("style");
            mkgmap::write_bundled_style(&d)?;
            d
        }
    };
    let typ = match &opts.typ {
        Some(p) => p.clone(),
        None => {
            let p = work.join("gsi.typ.txt");
            mkgmap::write_bundled_typ(&p)?;
            p
        }
    };

    let mut extra = Vec::new();
    if let Some(dem_path) = &opts.dem {
        let started = Instant::now();
        let dem = Dem::load(&TileSource::open(dem_path)?)?;
        let hgt_dir = work.join("hgt");
        let n = dem::write_hgt(&dem, [west, south, east, north], &hgt_dir)?;
        eprintln!("DEM → HGT {n} ファイル: {:.1?}", started.elapsed());
        let dists = mkgmap::dem_dists(&style_dir)?;
        extra.push(format!("--dem={}", hgt_dir.display()));
        extra.push(format!(
            "--dem-dists={}",
            dists.iter().map(u32::to_string).collect::<Vec<_>>().join(",")
        ));
    }

    let started = Instant::now();
    let mut inputs = osm_paths;
    inputs.push(typ);
    let img = mkgmap::run(&jar, &style_dir, &inputs, &work.join("out"), &opts.map, &extra)?;
    eprintln!("mkgmap: {:.1?}", started.elapsed());

    if let Some(dir) = opts.output.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::copy(&img, &opts.output)
        .with_context(|| format!("出力先に書き込めません: {}", opts.output.display()))?;
    eprintln!("出力しました: {}", opts.output.display());
    Ok(())
}

/// 1 タイル分の処理結果（座標は全世界ピクセル座標）。
#[derive(Default)]
struct TileOut {
    points: Vec<(Tags, P)>,
    lines: Vec<(Tags, Vec<P>)>,
    polygons: Vec<(Tags, Vec<Vec<P>>)>,
}

/// 1 領域分の処理結果。
struct RegionOut {
    /// 書き出した OSM XML
    paths: Vec<PathBuf>,
    /// 結合前の線の断片の数
    pieces: usize,
    /// 結合後の線の数
    lines: usize,
    /// 地物の数
    features: usize,
}

/// 最初に見つかった地物のあるタイルの extent。
fn first_extent(src: &TileSource, tiles: &[(TileCoord, u64)]) -> Result<Option<u32>> {
    for (t, _) in tiles {
        if let Some(data) = src.get(*t)? {
            if let Some(layer) = mvt::decode(&data).with_context(|| format!("タイル {t}"))?.first() {
                return Ok(Some(layer.extent));
            }
        }
    }
    Ok(None)
}

/// タイルをすべて包含する範囲（全世界ピクセル座標）。
fn tiles_bounds<'a>(grid: Grid, tiles: impl Iterator<Item = &'a TileCoord>) -> Bounds {
    let (mut tmin, mut tmax) = ((u32::MAX, u32::MAX), (0u32, 0u32));
    for t in tiles {
        tmin = (tmin.0.min(t.x), tmin.1.min(t.y));
        tmax = (tmax.0.max(t.x), tmax.1.max(t.y));
    }
    Bounds { min: grid.tile_bounds(tmin.0, tmin.1).min, max: grid.tile_bounds(tmax.0, tmax.1).max }
}

/// 1 つの領域のタイルを読み込んで切り取り・結合し、区画に分けて OSM XML（gzip 圧縮）を書き出す。
///
/// 領域どうしを並列に処理するので、領域内は 1 スレッドで処理する（同時にメモリに載る領域の数を
/// スレッド数までに抑えるため）。線は領域の境界で切れるが、区画の境界でも切るので結果は変わらない。
fn process_region(
    opts: &BuildOptions,
    grid: Grid,
    tiles: &[TileCoord],
    overview_zooms: &[u8],
    osm_dir: &Path,
    next_id: &AtomicUsize,
) -> Result<RegionOut> {
    let src = TileSource::open(&opts.input)?;
    let mut tag_index: HashMap<Tags, usize> = HashMap::new();
    let mut tag_table: Vec<Tags> = Vec::new();
    let mut intern = |tags: Tags| -> usize {
        *tag_index.entry(tags).or_insert_with_key(|t| {
            tag_table.push(t.clone());
            tag_table.len() - 1
        })
    };
    let mut items: Vec<Item> = Vec::new();
    let mut line_pieces: HashMap<usize, Vec<Vec<P>>> = HashMap::new();
    for &t in tiles {
        let Some(data) = src.get(t)? else { continue };
        let Some((e, out)) = process_tile(t, &data).with_context(|| format!("タイル {t}"))? else {
            continue;
        };
        if e != grid.extent {
            bail!("extent が混在しています（{} と {e}）", grid.extent);
        }
        for (tags, p) in out.points {
            items.push(Item { tags: intern(tags), feature: Feature::Point(p) });
        }
        for (tags, line) in out.lines {
            line_pieces.entry(intern(tags)).or_default().push(line);
        }
        for (tags, rings) in out.polygons {
            items.push(Item { tags: intern(tags), feature: Feature::Polygon(rings) });
        }
    }

    let pieces: usize = line_pieces.values().map(Vec::len).sum();
    let mut classes: Vec<(usize, Vec<Vec<P>>)> = line_pieces.into_iter().collect();
    classes.sort_by_key(|(t, _)| *t);
    let before = items.len();
    for (t, lines) in classes {
        let merged = geom::merge_lines(lines, |p| grid.on_tile_edge(p), MERGE_TOLERANCE_PX);
        items.extend(merged.into_iter().map(|l| Item { tags: t, feature: Feature::Line(l) }));
    }
    let lines = items.len() - before;

    let area = tiles_bounds(grid, tiles.iter());
    for &lz in overview_zooms {
        let low: Vec<Item> = overview_features(&src, grid, lz, tiles)?
            .into_iter()
            .map(|(tags, feature)| Item { tags: intern(tags), feature })
            .collect();
        items.extend(split::clip_items(&low, &area));
    }

    let mut paths = Vec::new();
    for part in split::plan(&items, area, opts.max_nodes) {
        // mkgmap は 8 桁の数字のファイル名をそのタイルの mapname として使う
        let id = next_id.fetch_add(1, Ordering::Relaxed) + 1;
        ensure!(id <= 9999, "区画が 9999 を超えました。build.max_nodes を大きくしてください");
        let path = osm_dir.join(format!("{}{id:04}.osm.gz", opts.map.family_id));
        let part_items = split::clip_items(&items, &part);
        let mut w = BufWriter::new(GzEncoder::new(File::create(&path)?, Compression::fast()));
        osm::write(&mut w, &grid, Some(&part), &tag_table, &part_items)?;
        w.into_inner().map_err(|e| e.into_error())?.finish()?;
        paths.push(path);
    }
    Ok(RegionOut { paths, pieces, lines, features: items.len() })
}

/// 縮小表示用のズーム `lz` のタイルのうち `tiles` にかかるものを読み、`grid` の座標に換算して返す。
/// 線は `lz` のタイル境界で結合しておく（領域の範囲での切り取りは呼び出し側で行う）。
/// 基準ズームの地物と区別できるように `gsi_zoom` タグを付ける。
fn overview_features(src: &TileSource, grid: Grid, lz: u8, tiles: &[TileCoord]) -> Result<Vec<(Tags, Feature)>> {
    let mut low_grid: Option<Grid> = None;
    let mut out = TileOut::default();
    let mut line_pieces: BTreeMap<Tags, Vec<Vec<P>>> = BTreeMap::new();
    for t in tile::covering(tiles, lz) {
        let Some(data) = src.get(t)? else { continue };
        let Some((extent, o)) = process_tile(t, &data).with_context(|| format!("タイル {t}"))? else {
            continue;
        };
        match low_grid {
            None => low_grid = Some(Grid { z: lz, extent }),
            Some(g) if g.extent != extent => bail!("z{lz} で extent が混在しています（{} と {extent}）", g.extent),
            _ => {}
        }
        out.points.extend(o.points);
        out.polygons.extend(o.polygons);
        for (tags, line) in o.lines {
            line_pieces.entry(tags).or_default().push(line);
        }
    }
    let Some(low_grid) = low_grid else { return Ok(Vec::new()) };

    let scale = f64::from(1u32 << (grid.z - lz)) * f64::from(grid.extent) / f64::from(low_grid.extent);
    let to_base = |p: &P| [p[0] * scale, p[1] * scale];
    let zoom_tag = ("gsi_zoom".to_string(), lz.to_string());
    let tagged = |mut tags: Tags| {
        tags.push(zoom_tag.clone());
        tags
    };
    let mut features = Vec::new();
    for (tags, p) in out.points {
        features.push((tagged(tags), Feature::Point(to_base(&p))));
    }
    for (tags, lines) in line_pieces {
        let tags = tagged(tags);
        for line in geom::merge_lines(lines, |p| low_grid.on_tile_edge(p), MERGE_TOLERANCE_PX) {
            features.push((tags.clone(), Feature::Line(line.iter().map(to_base).collect())));
        }
    }
    for (tags, rings) in out.polygons {
        let rings = rings.iter().map(|r| r.iter().map(to_base).collect()).collect();
        features.push((tagged(tags), Feature::Polygon(rings)));
    }
    Ok(features)
}

/// タイルをデコードし、タイル範囲で切り取る。地物のないタイルは `None`。
fn process_tile(t: TileCoord, data: &[u8]) -> Result<Option<(u32, TileOut)>> {
    let layers = mvt::decode(data)?;
    let Some(extent) = layers.first().map(|l| l.extent) else {
        return Ok(None);
    };
    let grid = Grid { z: t.z, extent };
    let bounds = grid.tile_bounds(t.x, t.y);
    let mut out = TileOut::default();
    for layer in layers {
        if layer.extent != extent {
            bail!("タイル内で extent が混在しています");
        }
        for f in layer.features {
            let mut tags = Vec::with_capacity(f.props.len() + 1);
            tags.push(("gsi_layer".to_string(), layer.name.clone()));
            tags.extend(f.props);
            let global = |pts: &[[i32; 2]]| -> Vec<P> {
                pts.iter().map(|&p| grid.to_global(t.x, t.y, p)).collect()
            };
            match f.geometry {
                Geometry::Points(pts) => {
                    for p in global(&pts) {
                        if bounds.contains_half_open(p) {
                            out.points.push((tags.clone(), p));
                        }
                    }
                }
                Geometry::Lines(lines) => {
                    for line in lines {
                        for piece in geom::clip_line(&global(&line), &bounds) {
                            out.lines.push((tags.clone(), piece));
                        }
                    }
                }
                Geometry::Polygons(polys) => {
                    for rings in polys {
                        let rings: Vec<Vec<P>> = rings.iter().map(|r| global(r)).collect();
                        for poly in geom::clip_polygon(&rings, &bounds) {
                            if let Some(rs) = geom::polygon_rings(&poly) {
                                out.polygons.push((tags.clone(), rs));
                            }
                        }
                    }
                }
            }
        }
    }
    Ok(Some((extent, out)))
}
