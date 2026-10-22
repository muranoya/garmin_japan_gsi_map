//! 地図を mkgmap のタイル（区画）に分割する。
//!
//! Garmin の地図は 1 タイルあたりの RGN セクションが 16MB までに制限されるため、
//! 広い範囲は複数のタイルに分ける必要がある。区画内のノード数が上限以下になるまで
//! 範囲を四分木で再帰的に分割し、地物は区画の範囲で切り取る。
//!
//! 全国のような広い範囲は全地物をメモリに載せられないので、まずタイル単位で「領域」に分け
//! （`plan_regions`）、領域ごとに読み込み・結合・区画への分割・書き出しを行う。

use crate::geom::{self, Bounds, P};
use crate::osm::{Feature, Item};
use crate::tile::TileCoord;

/// タイルを四分木で領域に分ける。各領域のデータ量（タイルの圧縮されたままのバイト数の合計）が
/// `max_bytes` 以下になるまで分割する（1 タイルで超える場合はそのタイルだけの領域にする）。
/// 領域はズーム 0 から辿った四分木の節点なので互いに重ならない。
pub fn plan_regions(tiles: Vec<(TileCoord, u64)>, max_bytes: u64) -> Vec<Vec<TileCoord>> {
    let mut out = Vec::new();
    if let Some(z) = tiles.first().map(|(t, _)| t.z) {
        plan_regions_rec(tiles, z, 0, max_bytes, &mut out);
    }
    out
}

fn plan_regions_rec(tiles: Vec<(TileCoord, u64)>, z: u8, level: u8, max_bytes: u64, out: &mut Vec<Vec<TileCoord>>) {
    if tiles.is_empty() {
        return;
    }
    if level >= z || tiles.iter().map(|(_, b)| b).sum::<u64>() <= max_bytes {
        out.push(tiles.into_iter().map(|(t, _)| t).collect());
        return;
    }
    let shift = z - level - 1;
    let mut children: [Vec<(TileCoord, u64)>; 4] = Default::default();
    for (t, b) in tiles {
        let i = ((t.x >> shift) & 1) + 2 * ((t.y >> shift) & 1);
        children[i as usize].push((t, b));
    }
    for c in children {
        plan_regions_rec(c, z, level + 1, max_bytes, out);
    }
}

/// 分割の深さの上限。z16 の全世界ピクセル座標で 1 区画が極端に小さくならないようにする。
const MAX_DEPTH: u32 = 16;

fn coords(f: &Feature) -> Box<dyn Iterator<Item = &P> + '_> {
    match f {
        Feature::Point(p) => Box::new(std::iter::once(p)),
        Feature::Line(c) => Box::new(c.iter()),
        Feature::Polygon(rings) => Box::new(rings.iter().flatten()),
    }
}

fn bbox(f: &Feature) -> Bounds {
    let mut b = Bounds { min: [f64::MAX, f64::MAX], max: [f64::MIN, f64::MIN] };
    for p in coords(f) {
        b.min = [b.min[0].min(p[0]), b.min[1].min(p[1])];
        b.max = [b.max[0].max(p[0]), b.max[1].max(p[1])];
    }
    b
}

fn intersects(a: &Bounds, b: &Bounds) -> bool {
    a.min[0] <= b.max[0] && a.max[0] >= b.min[0] && a.min[1] <= b.max[1] && a.max[1] >= b.min[1]
}

fn inside(a: &Bounds, outer: &Bounds) -> bool {
    a.min[0] >= outer.min[0] && a.max[0] <= outer.max[0] && a.min[1] >= outer.min[1] && a.max[1] <= outer.max[1]
}

/// 区画の範囲を決める。各区画内のノード数が `max_nodes` 以下になるように分割する。
/// 地物のない区画は返さない。
pub fn plan(items: &[Item], area: Bounds, max_nodes: usize) -> Vec<Bounds> {
    let boxes: Vec<Bounds> = items.iter().map(|it| bbox(&it.feature)).collect();
    let mut out = Vec::new();
    let all: Vec<usize> = (0..items.len()).collect();
    plan_rec(items, &boxes, all, area, max_nodes, 0, &mut out);
    out
}

fn plan_rec(
    items: &[Item],
    boxes: &[Bounds],
    idx: Vec<usize>,
    area: Bounds,
    max_nodes: usize,
    depth: u32,
    out: &mut Vec<Bounds>,
) {
    if idx.is_empty() {
        return;
    }
    let nodes: usize = idx
        .iter()
        .map(|&i| {
            if inside(&boxes[i], &area) {
                coords(&items[i].feature).count()
            } else {
                coords(&items[i].feature).filter(|p| area.contains_half_open(**p)).count()
            }
        })
        .sum();
    if nodes <= max_nodes || depth >= MAX_DEPTH {
        out.push(area);
        return;
    }
    let mid = [(area.min[0] + area.max[0]) / 2.0, (area.min[1] + area.max[1]) / 2.0];
    let children = [
        Bounds { min: area.min, max: mid },
        Bounds { min: [mid[0], area.min[1]], max: [area.max[0], mid[1]] },
        Bounds { min: [area.min[0], mid[1]], max: [mid[0], area.max[1]] },
        Bounds { min: mid, max: area.max },
    ];
    for child in children {
        let sub: Vec<usize> = idx.iter().copied().filter(|&i| intersects(&boxes[i], &child)).collect();
        plan_rec(items, boxes, sub, child, max_nodes, depth + 1, out);
    }
}

/// 区画に含まれる地物を、区画の範囲で切り取って返す。
pub fn clip_items(items: &[Item], area: &Bounds) -> Vec<Item> {
    let mut out = Vec::new();
    for it in items {
        let b = bbox(&it.feature);
        if !intersects(&b, area) {
            continue;
        }
        match &it.feature {
            // 点は半開区間で判定し、隣の区画と重複させない
            Feature::Point(p) => {
                if area.contains_half_open(*p) {
                    out.push(Item { tags: it.tags, feature: Feature::Point(*p) });
                }
            }
            _ if inside(&b, area) => out.push(it.clone()),
            Feature::Line(c) => out.extend(
                geom::clip_line(c, area).into_iter().map(|l| Item { tags: it.tags, feature: Feature::Line(l) }),
            ),
            Feature::Polygon(rings) => out.extend(
                geom::clip_polygon(rings, area)
                    .into_iter()
                    .filter_map(|p| geom::polygon_rings(&p))
                    .map(|r| Item { tags: it.tags, feature: Feature::Polygon(r) }),
            ),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const AREA: Bounds = Bounds { min: [0.0, 0.0], max: [100.0, 100.0] };

    #[test]
    fn データ量に応じてタイルを領域に分ける() {
        let t = |x, y| TileCoord { z: 4, x, y };
        // 左上の 1/4 に大きなタイルが集まっている
        let tiles = vec![(t(0, 0), 5), (t(1, 0), 5), (t(1, 1), 5), (t(9, 9), 1), (t(15, 15), 1)];
        let regions = plan_regions(tiles, 10);
        assert_eq!(regions.iter().map(Vec::len).sum::<usize>(), 5, "すべてのタイルがどこかの領域に入る");
        // 右下の 2 タイルは合計 2 なので 1 つの領域になる
        assert!(regions.contains(&vec![t(9, 9), t(15, 15)]));
        // 左上は合計 15 で上限を超えるので分ける
        assert!(regions.iter().all(|r| !(r.contains(&t(0, 0)) && r.contains(&t(1, 1)) && r.contains(&t(1, 0)))));
    }

    #[test]
    fn 一タイルで上限を超えるときはそのタイルだけの領域にする() {
        let t = |x, y| TileCoord { z: 2, x, y };
        let regions = plan_regions(vec![(t(0, 0), 100), (t(1, 0), 100)], 10);
        assert_eq!(regions, vec![vec![t(0, 0)], vec![t(1, 0)]]);
    }

    fn line(tags: usize, pts: &[P]) -> Item {
        Item { tags, feature: Feature::Line(pts.to_vec()) }
    }

    #[test]
    fn ノード数が上限以下なら分割しない() {
        let items = vec![line(0, &[[10.0, 10.0], [20.0, 20.0]])];
        assert_eq!(plan(&items, AREA, 10), vec![AREA]);
    }

    #[test]
    fn 密な区画だけを分割し地物のない区画は省く() {
        // 左上 1/4 に 40 点を集中させる
        let items: Vec<Item> = (0..20)
            .map(|i| line(0, &[[1.0 + i as f64, 1.0], [1.0 + i as f64, 2.0]]))
            .collect();
        let parts = plan(&items, AREA, 10);
        assert!(parts.len() > 1);
        assert!(parts.iter().all(|b| b.max[0] <= 50.0 && b.max[1] <= 50.0), "{parts:?}");
    }

    #[test]
    fn 区画をまたぐ線は切り取り点は片方だけに入れる() {
        let items = vec![
            line(0, &[[10.0, 10.0], [90.0, 10.0]]),
            Item { tags: 1, feature: Feature::Point([50.0, 50.0]) },
        ];
        let left = Bounds { min: [0.0, 0.0], max: [50.0, 100.0] };
        let right = Bounds { min: [50.0, 0.0], max: [100.0, 100.0] };
        let l = clip_items(&items, &left);
        let r = clip_items(&items, &right);
        assert!(matches!(&l[0].feature, Feature::Line(c) if c == &vec![[10.0, 10.0], [50.0, 10.0]]));
        assert!(matches!(&r[0].feature, Feature::Line(c) if c == &vec![[50.0, 10.0], [90.0, 10.0]]));
        assert_eq!(l.len(), 1, "境界上の点は右の区画に入る");
        assert_eq!(r.len(), 2);
    }

    #[test]
    fn 区画をまたぐポリゴンを切り取る() {
        let sq = vec![vec![[40.0, 40.0], [60.0, 40.0], [60.0, 60.0], [40.0, 60.0]]];
        let items = vec![Item { tags: 0, feature: Feature::Polygon(sq) }];
        let left = Bounds { min: [0.0, 0.0], max: [50.0, 100.0] };
        let got = clip_items(&items, &left);
        assert_eq!(got.len(), 1);
        let Feature::Polygon(rings) = &got[0].feature else { panic!() };
        assert!(rings[0].iter().all(|p| p[0] <= 50.0));
    }
}
