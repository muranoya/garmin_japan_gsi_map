//! ベクタタイル → OSM XML（区画ごと）→ mkgmap → .img の変換。

use crate::dem::{self, Dem};
use crate::geom::{self, Bounds, Grid, P};
use crate::mkgmap::{self, MapInfo};
use crate::mvt::{self, Geometry};
use crate::osm::{self, Feature, Item, Tags};
use crate::source::TileSource;
use crate::split;
use crate::tile::TileCoord;
use anyhow::{Context, Result, bail};
use rayon::prelude::*;
use std::collections::HashMap;
use std::io::BufWriter;
use std::path::PathBuf;
use std::time::Instant;

/// タイル境界で線を結合するときの端点の許容距離（ピクセル）。z16 で 8px ≒ 1m。
const MERGE_TOLERANCE_PX: f64 = 8.0;

/// 縮小表示用の注記を取るズーム。地理院は小縮尺の地図向けに主な山名などを選んで低ズームのタイルに
/// 収録しているので、このズームのタイルの点を `gsi_zoom` タグ付きで加え、どれを使うかはスタイルで決める。
pub const OVERVIEW_ZOOM: u8 = 10;

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
    let collected = collect(&opts)?;
    eprintln!("タイルの読み込み・切り取り・結合: {:.1?}", started.elapsed());

    // 地図を mkgmap のタイル（区画）に分けて、区画ごとに OSM XML を書き出す
    let started = Instant::now();
    let parts = split::plan(&collected.items, collected.area, opts.max_nodes);
    let osm_dir = work.join("osm");
    std::fs::create_dir_all(&osm_dir)?;
    // mkgmap は 8 桁の数字のファイル名をそのタイルの mapname として使う
    let osm_paths: Vec<PathBuf> = parts
        .par_iter()
        .enumerate()
        .map(|(i, area)| -> Result<PathBuf> {
            let path = osm_dir.join(format!("{}{:04}.osm", opts.map.family_id, i + 1));
            let items = split::clip_items(&collected.items, area);
            let mut w = BufWriter::new(std::fs::File::create(&path)?);
            osm::write(&mut w, &collected.grid, Some(area), &collected.tag_table, &items)?;
            Ok(path)
        })
        .collect::<Result<_>>()?;
    let size: u64 = osm_paths.iter().map(|p| std::fs::metadata(p).map_or(0, |m| m.len())).sum();
    eprintln!(
        "OSM XML 書き出し: {} 区画 {:.1?}（{:.1} MB）",
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
        let n = dem::write_hgt(&dem, collected.bounds, &hgt_dir)?;
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

struct Collected {
    grid: Grid,
    /// 入力タイル範囲の経緯度 [min_lon, min_lat, max_lon, max_lat]
    bounds: [f64; 4],
    /// 入力タイル範囲（全世界ピクセル座標）
    area: Bounds,
    tag_table: Vec<Tags>,
    items: Vec<Item>,
}

/// 1 タイル分の処理結果（座標は全世界ピクセル座標）。
#[derive(Default)]
struct TileOut {
    points: Vec<(Tags, P)>,
    lines: Vec<(Tags, Vec<P>)>,
    polygons: Vec<(Tags, Vec<Vec<P>>)>,
}

fn collect(opts: &BuildOptions) -> Result<Collected> {
    let src = TileSource::open(&opts.input)?;
    let zooms = src.zooms()?;
    let z = match opts.zoom {
        Some(z) if zooms.contains(&z) => z,
        Some(z) => bail!("入力にズーム {z} のタイルがありません（収録: {zooms:?}）"),
        None => *zooms.last().context("入力にタイルがありません")?,
    };

    let mut extent: Option<u32> = None;
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
    let mut tiles = 0usize;
    let (mut tmin, mut tmax) = ((u32::MAX, u32::MAX), (0u32, 0u32));

    src.for_each_batch(z, 256, |batch| {
        let outs: Vec<(u32, TileOut)> = batch
            .par_iter()
            .map(|(t, data)| process_tile(*t, data).with_context(|| format!("タイル {t}")))
            .collect::<Result<Vec<_>>>()?
            .into_iter()
            .flatten()
            .collect();
        for (e, out) in outs {
            match extent {
                None => extent = Some(e),
                Some(prev) if prev != e => bail!("extent が混在しています（{prev} と {e}）"),
                _ => {}
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
        tiles += batch.len();
        for (t, _) in &batch {
            tmin = (tmin.0.min(t.x), tmin.1.min(t.y));
            tmax = (tmax.0.max(t.x), tmax.1.max(t.y));
        }
        Ok(())
    })?;

    let grid = Grid { z, extent: extent.context("入力に地物がありません")? };
    let pieces: usize = line_pieces.values().map(Vec::len).sum();
    let mut classes: Vec<(usize, Vec<Vec<P>>)> = line_pieces.into_iter().collect();
    classes.sort_by_key(|(t, _)| *t);
    let merged: Vec<(usize, Vec<Vec<P>>)> = classes
        .into_par_iter()
        .map(|(t, lines)| (t, geom::merge_lines(lines, |p| grid.on_tile_edge(p), MERGE_TOLERANCE_PX)))
        .collect();
    let merged_count: usize = merged.iter().map(|(_, l)| l.len()).sum();
    for (t, lines) in merged {
        items.extend(lines.into_iter().map(|l| Item { tags: t, feature: Feature::Line(l) }));
    }

    eprintln!(
        "z{z}: {tiles} タイル / 線 {pieces} 断片 → {merged_count} 本に結合 / 地物 {} 件",
        items.len()
    );
    let area = Bounds { min: grid.tile_bounds(tmin.0, tmin.1).min, max: grid.tile_bounds(tmax.0, tmax.1).max };

    if z > OVERVIEW_ZOOM && zooms.contains(&OVERVIEW_ZOOM) {
        let points = overview_points(&src, grid, &area)?;
        eprintln!("z{OVERVIEW_ZOOM}: 縮小表示用の点 {} 件", points.len());
        for (tags, p) in points {
            items.push(Item { tags: intern(tags), feature: Feature::Point(p) });
        }
    }
    let (west, north) = grid.to_lonlat(area.min);
    let (east, south) = grid.to_lonlat(area.max);
    Ok(Collected { grid, bounds: [west, south, east, north], area, tag_table, items })
}

/// 縮小表示用のズームのタイルから点地物を読み、`grid` の座標に換算して `area` 内のものを返す。
/// 区別できるように `gsi_zoom` タグを付ける。
fn overview_points(src: &TileSource, grid: Grid, area: &Bounds) -> Result<Vec<(Tags, P)>> {
    let mut points = Vec::new();
    src.for_each_batch(OVERVIEW_ZOOM, 256, |batch| {
        for (t, data) in batch {
            let Some((extent, out)) = process_tile(t, &data).with_context(|| format!("タイル {t}"))? else {
                continue;
            };
            let scale = f64::from(1u32 << (grid.z - OVERVIEW_ZOOM)) * f64::from(grid.extent) / f64::from(extent);
            for (mut tags, p) in out.points {
                let p = [p[0] * scale, p[1] * scale];
                if area.contains_half_open(p) {
                    tags.push(("gsi_zoom".to_string(), OVERVIEW_ZOOM.to_string()));
                    points.push((tags, p));
                }
            }
        }
        Ok(())
    })?;
    Ok(points)
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
