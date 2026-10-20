//! タイル境界をまたぐ地物の切り取り・結合と座標変換。
//!
//! 座標は「ズーム z・extent の全世界ピクセル座標」（タイル (x, y) の原点が
//! (x * extent, y * extent)、y 下向き）で扱う。隣接タイルは同じ格子に量子化されて
//! いるので、同じ線分をタイル境界で切ると両側で同一の交点が得られる。

use geo::{BooleanOps, Coord, LineString, MultiPolygon, Polygon, Rect};
use std::collections::HashMap;
use std::f64::consts::PI;

pub type P = [f64; 2];

/// 全世界ピクセル座標系の定義（ズームと extent）。
#[derive(Debug, Clone, Copy)]
pub struct Grid {
    pub z: u8,
    pub extent: u32,
}

impl Grid {
    /// タイル内座標を全世界ピクセル座標に変換する。
    pub fn to_global(self, tx: u32, ty: u32, p: [i32; 2]) -> P {
        let e = f64::from(self.extent);
        [f64::from(tx) * e + f64::from(p[0]), f64::from(ty) * e + f64::from(p[1])]
    }

    /// タイルの範囲（全世界ピクセル座標）。
    pub fn tile_bounds(&self, tx: u32, ty: u32) -> Bounds {
        let e = f64::from(self.extent);
        let (x0, y0) = (f64::from(tx) * e, f64::from(ty) * e);
        Bounds { min: [x0, y0], max: [x0 + e, y0 + e] }
    }

    /// 点がいずれかのタイル境界線上にあるか。
    pub fn on_tile_edge(&self, p: P) -> bool {
        let e = f64::from(self.extent);
        (p[0] / e).fract() == 0.0 || (p[1] / e).fract() == 0.0
    }

    /// 全世界ピクセル座標を経緯度 (lon, lat) に変換する。
    pub fn to_lonlat(self, p: P) -> (f64, f64) {
        let size = f64::from(self.extent) * f64::from(1u32 << self.z);
        let lon = p[0] / size * 360.0 - 180.0;
        let lat = (PI * (1.0 - 2.0 * p[1] / size)).sinh().atan().to_degrees();
        (lon, lat)
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Bounds {
    pub min: P,
    pub max: P,
}

impl Bounds {
    /// 半開区間 [min, max) に含まれるか。点地物の重複排除に使う。
    pub fn contains_half_open(&self, p: P) -> bool {
        p[0] >= self.min[0] && p[0] < self.max[0] && p[1] >= self.min[1] && p[1] < self.max[1]
    }
}

/// 折れ線を矩形で切り取る（線分ごとの Liang–Barsky）。範囲内の部分ごとに分割して返す。
pub fn clip_line(line: &[P], b: &Bounds) -> Vec<Vec<P>> {
    let mut out: Vec<Vec<P>> = Vec::new();
    let mut current: Vec<P> = Vec::new();
    for seg in line.windows(2) {
        let Some((p, q, clipped_end)) = clip_segment(seg[0], seg[1], b) else {
            if current.len() >= 2 {
                out.push(std::mem::take(&mut current));
            }
            current.clear();
            continue;
        };
        if current.last() != Some(&p) {
            if current.len() >= 2 {
                out.push(std::mem::take(&mut current));
            }
            current.clear();
            current.push(p);
        }
        if q != p {
            current.push(q);
        }
        if clipped_end {
            if current.len() >= 2 {
                out.push(std::mem::take(&mut current));
            }
            current.clear();
        }
    }
    if current.len() >= 2 {
        out.push(current);
    }
    out
}

/// 線分を矩形で切り取る。戻り値の第 3 要素は終点側が切られたかどうか。
fn clip_segment(a: P, b: P, r: &Bounds) -> Option<(P, P, bool)> {
    let d = [b[0] - a[0], b[1] - a[1]];
    let (mut t0, mut t1) = (0.0f64, 1.0f64);
    // (境界値, 軸, 境界が最小側か)
    let edges = [
        (r.min[0], 0, true),
        (r.max[0], 0, false),
        (r.min[1], 1, true),
        (r.max[1], 1, false),
    ];
    for &(edge, axis, is_min) in &edges {
        let (pa, da) = (a[axis], d[axis]);
        if da == 0.0 {
            let outside = if is_min { pa < edge } else { pa > edge };
            if outside {
                return None;
            }
            continue;
        }
        let t = (edge - pa) / da;
        let entering = if is_min { da > 0.0 } else { da < 0.0 };
        if entering {
            t0 = t0.max(t);
        } else {
            t1 = t1.min(t);
        }
    }
    if t0 > t1 {
        return None;
    }
    let at = |t: f64| -> P {
        if t == 0.0 {
            a
        } else if t == 1.0 {
            b
        } else {
            let mut p = [a[0] + d[0] * t, a[1] + d[1] * t];
            // 交点は境界上に正確に載せる（隣接タイルとの端点一致のため）
            for &(edge, axis, _) in &edges {
                if (p[axis] - edge).abs() < 1e-6 {
                    p[axis] = edge;
                }
            }
            p
        }
    };
    Some((at(t0), at(t1), t1 < 1.0))
}

/// ポリゴン（外周 + 穴）を矩形で切り取る。
pub fn clip_polygon(rings: &[Vec<P>], b: &Bounds) -> MultiPolygon<f64> {
    let to_ls = |r: &Vec<P>| -> LineString<f64> {
        LineString::from(r.iter().map(|p| Coord { x: p[0], y: p[1] }).collect::<Vec<_>>())
    };
    let poly = Polygon::new(to_ls(&rings[0]), rings[1..].iter().map(to_ls).collect());
    let rect = Rect::new(Coord { x: b.min[0], y: b.min[1] }, Coord { x: b.max[0], y: b.max[1] });
    poly.intersection(&rect.to_polygon())
}

/// geo のポリゴンを [外周, 穴, ...] のリング列にする（各リングは始点を末尾に繰り返さない）。
/// 外周が 3 点未満に潰れたものは `None`、潰れた穴は捨てる。
pub fn polygon_rings(poly: &Polygon<f64>) -> Option<Vec<Vec<P>>> {
    let ring = |ls: &LineString<f64>| -> Vec<P> {
        let mut v: Vec<P> = ls.coords().map(|c| [c.x, c.y]).collect();
        if v.len() > 1 && v.first() == v.last() {
            v.pop();
        }
        v
    };
    let exterior = ring(poly.exterior());
    if exterior.len() < 3 {
        return None;
    }
    let mut rings = vec![exterior];
    rings.extend(poly.interiors().iter().map(ring).filter(|r| r.len() >= 3));
    Some(rings)
}

/// タイル境界上の端点が近接する線を結合する。
///
/// 地理院のタイルは隣接タイル間で境界付近の形状がわずかに異なることがあり、
/// 切り取った端点が完全には一致しない（z16 で 1px ≒ 12cm 程度のずれ）。そこで `tol`
/// 以内の端点どうしを組にする。完全に一致する端点が 3 つ以上集まる点は本物の分岐とみなして
/// 結合しない。
pub fn merge_lines(lines: Vec<Vec<P>>, joinable: impl Fn(P) -> bool, tol: f64) -> Vec<Vec<P>> {
    #[derive(Clone, Copy, PartialEq)]
    enum End {
        Start,
        End,
    }
    let slot = |e: End| if e == End::Start { 0 } else { 1 };
    let cell = |p: P| ((p[0] / tol).floor() as i64, (p[1] / tol).floor() as i64);

    let mut ends: Vec<(usize, End, P)> = Vec::new();
    let mut grid: HashMap<(i64, i64), Vec<usize>> = HashMap::new();
    for (i, l) in lines.iter().enumerate() {
        for (end, p) in [(End::Start, l[0]), (End::End, *l.last().unwrap())] {
            if joinable(p) {
                grid.entry(cell(p)).or_default().push(ends.len());
                ends.push((i, end, p));
            }
        }
    }
    // 各端点について、未結合の別の線の端点で `r` 以内にあるものを (距離, 添字) で集める
    let candidates = |k: usize, r: f64, free: &[bool]| -> Vec<(f64, usize)> {
        let (i, _, p) = ends[k];
        let (cx, cy) = cell(p);
        let mut out = Vec::new();
        for dx in -1..=1 {
            for dy in -1..=1 {
                for &m in grid.get(&(cx + dx, cy + dy)).into_iter().flatten() {
                    let (j, _, q) = ends[m];
                    let d = (p[0] - q[0]).hypot(p[1] - q[1]);
                    if j != i && free[m] && d <= r {
                        out.push((d, m));
                    }
                }
            }
        }
        out
    };
    let nearest = |k: usize, r: f64, free: &[bool]| -> Option<usize> {
        candidates(k, r, free)
            .into_iter()
            .min_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)))
            .map(|(_, m)| m)
    };

    // partner[i] = [Start 側の相手, End 側の相手]
    let mut partner: Vec<[Option<(usize, End)>; 2]> = vec![[None, None]; lines.len()];
    let mut free = vec![true; ends.len()];
    let mut link = |k: usize, m: usize, free: &mut [bool]| {
        let ((i, ei, _), (j, ej, _)) = (ends[k], ends[m]);
        partner[i][slot(ei)] = Some((j, ej));
        partner[j][slot(ej)] = Some((i, ei));
        free[k] = false;
        free[m] = false;
    };

    // 1 段階目: 一致する端点がちょうど 2 つの点を結合する。3 つ以上なら分岐点として以後も扱わない
    const EXACT: f64 = 1e-3;
    let (mut pairs, mut junctions) = (Vec::new(), Vec::new());
    for k in 0..ends.len() {
        match candidates(k, EXACT, &free)[..] {
            [(_, m)] if m > k && candidates(m, EXACT, &free).len() == 1 => pairs.push((k, m)),
            [_, _, ..] => junctions.push(k),
            _ => {}
        }
    }
    for (k, m) in pairs {
        link(k, m, &mut free);
    }
    for k in junctions {
        free[k] = false;
    }

    // 2 段階目: 残りの端点を tol 以内で互いに最も近いものどうし組にする。
    // 線がタイル境界を何度も行き来する所では候補が複数になるため、一意性ではなく相互最近傍で決める
    loop {
        let pairs: Vec<(usize, usize)> = (0..ends.len())
            .filter(|&k| free[k])
            .filter_map(|k| {
                let m = nearest(k, tol, &free)?;
                (m > k && nearest(m, tol, &free) == Some(k)).then_some((k, m))
            })
            .collect();
        if pairs.is_empty() {
            break;
        }
        for (k, m) in pairs {
            link(k, m, &mut free);
        }
    }

    let mut used = vec![false; lines.len()];
    let mut out = Vec::new();
    for start in 0..lines.len() {
        if used[start] {
            continue;
        }
        // Start 側をたどって鎖の端（または環の一周）まで戻る
        let (mut cur, mut entry) = (start, End::End);
        loop {
            let back = if entry == End::End { End::Start } else { End::End };
            match partner[cur][slot(back)] {
                // 順方向では j を ej 側から抜けて cur に入る
                Some((j, ej)) if j != start => (cur, entry) = (j, ej),
                _ => break,
            }
        }
        // 鎖の端から順に連結する。`entry` は進行方向の出口側の端
        let mut coords: Vec<P> = Vec::new();
        loop {
            used[cur] = true;
            let mut seg = lines[cur].clone();
            if entry == End::Start {
                seg.reverse();
            }
            if coords.is_empty() {
                coords = seg;
            } else {
                coords.extend_from_slice(&seg[1..]);
            }
            match partner[cur][slot(entry)] {
                Some((j, ej)) if !used[j] => {
                    cur = j;
                    entry = if ej == End::Start { End::End } else { End::Start };
                }
                _ => break,
            }
        }
        out.push(coords);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const B: Bounds = Bounds { min: [0.0, 0.0], max: [10.0, 10.0] };

    #[test]
    fn 範囲外にはみ出した線を境界で切る() {
        let line = vec![[-5.0, 5.0], [5.0, 5.0], [15.0, 5.0]];
        assert_eq!(clip_line(&line, &B), vec![vec![[0.0, 5.0], [5.0, 5.0], [10.0, 5.0]]]);
    }

    #[test]
    fn 範囲を出入りする線は分割する() {
        let line = vec![[2.0, 2.0], [2.0, 12.0], [8.0, 12.0], [8.0, 2.0]];
        assert_eq!(
            clip_line(&line, &B),
            vec![vec![[2.0, 2.0], [2.0, 10.0]], vec![[8.0, 10.0], [8.0, 2.0]]]
        );
    }

    #[test]
    fn 完全に範囲外の線は消える() {
        assert!(clip_line(&[[11.0, 0.0], [20.0, 5.0]], &B).is_empty());
    }

    #[test]
    fn 隣接タイルで切った線は境界で再結合できる() {
        let g = Grid { z: 1, extent: 10 };
        let line = vec![[3.0, 4.0], [17.0, 6.0]]; // タイル 0 と 1 をまたぐ斜めの線
        let mut pieces = clip_line(&line, &g.tile_bounds(0, 0));
        pieces.extend(clip_line(&line, &g.tile_bounds(1, 0)));
        assert_eq!(pieces.len(), 2);
        let merged = merge_lines(pieces, |p| g.on_tile_edge(p), 1.0);
        assert_eq!(merged.len(), 1);
        assert_eq!(merged[0].first(), Some(&[3.0, 4.0]));
        assert_eq!(merged[0].last(), Some(&[17.0, 6.0]));
    }

    #[test]
    fn 向きが逆の線や順不同の断片も結合する() {
        let g = Grid { z: 2, extent: 10 };
        let pieces = vec![
            vec![[20.0, 5.0], [25.0, 5.0]],  // 3 番目
            vec![[10.0, 5.0], [5.0, 5.0]],   // 1 番目（逆向き）
            vec![[10.0, 5.0], [20.0, 5.0]],  // 2 番目
        ];
        let merged = merge_lines(pieces, |p| g.on_tile_edge(p), 1.0);
        assert_eq!(merged.len(), 1);
        let ends = (merged[0][0], *merged[0].last().unwrap());
        assert!(ends == ([5.0, 5.0], [25.0, 5.0]) || ends == ([25.0, 5.0], [5.0, 5.0]));
        assert_eq!(merged[0].len(), 4);
    }

    #[test]
    fn 境界上の環も一周で止まる() {
        let g = Grid { z: 2, extent: 10 };
        let pieces = vec![
            vec![[10.0, 5.0], [5.0, 0.0], [0.0, 5.0]],
            vec![[0.0, 5.0], [5.0, 10.0], [10.0, 5.0]],
        ];
        // 端点 (0,5) と (10,5) はどちらもタイル境界上
        let merged = merge_lines(pieces, |p| g.on_tile_edge(p), 1.0);
        assert_eq!(merged.len(), 1);
        assert_eq!(merged[0].len(), 5);
    }

    #[test]
    fn わずかにずれた端点も許容距離内なら結合する() {
        let g = Grid { z: 2, extent: 10 };
        let pieces = vec![vec![[5.0, 5.0], [10.0, 5.0]], vec![[10.0, 5.3], [15.0, 5.0]]];
        assert_eq!(merge_lines(pieces.clone(), |p| g.on_tile_edge(p), 0.5).len(), 1);
        assert_eq!(merge_lines(pieces, |p| g.on_tile_edge(p), 0.2).len(), 2);
    }

    #[test]
    fn タイルの角の近くでは一致する端点を優先して結合する() {
        let g = Grid { z: 2, extent: 10 };
        // (10, 10) の角のすぐ横を通り、3 タイルに分かれる線。
        // 3 つの断片の端点がすべて 1.0 以内に集まる
        let line = vec![[5.0, 9.45], [15.0, 10.45]];
        let mut pieces = Vec::new();
        for (tx, ty) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
            pieces.extend(clip_line(&line, &g.tile_bounds(tx, ty)));
        }
        assert_eq!(pieces.len(), 3);
        assert_eq!(merge_lines(pieces, |p| g.on_tile_edge(p), 1.0).len(), 1);
    }

    #[test]
    fn 候補が複数あるときは互いに最も近い端点どうしを結合する() {
        let g = Grid { z: 2, extent: 10 };
        // 境界 x=10 に 2 組の断片が 0.3 だけ離れて並ぶ。tol=1 だと各端点に候補が 2 つある
        let pieces = vec![
            vec![[5.0, 5.0], [10.0, 5.0]],
            vec![[10.0, 5.05], [15.0, 5.0]],
            vec![[5.0, 5.3], [10.0, 5.3]],
            vec![[10.0, 5.35], [15.0, 5.3]],
        ];
        let merged = merge_lines(pieces, |p| g.on_tile_edge(p), 1.0);
        assert_eq!(merged.len(), 2);
        for l in merged {
            assert_eq!(l.len(), 3);
            assert!((l[0][1] - l[2][1]).abs() < 1e-9, "同じ高さの断片どうしが結合されること");
        }
    }

    #[test]
    fn 三本以上が集まる点では結合しない() {
        let g = Grid { z: 2, extent: 10 };
        let pieces = vec![
            vec![[5.0, 10.0], [5.0, 5.0]],
            vec![[5.0, 10.0], [5.0, 15.0]],
            vec![[5.0, 10.0], [8.0, 15.0]],
        ];
        assert_eq!(merge_lines(pieces, |p| g.on_tile_edge(p), 1.0).len(), 3);
    }

    #[test]
    fn ポリゴンを矩形で切り取る() {
        let rings = vec![vec![[-5.0, -5.0], [5.0, -5.0], [5.0, 5.0], [-5.0, 5.0]]];
        let mp = clip_polygon(&rings, &B);
        assert_eq!(mp.0.len(), 1);
        use geo::Area;
        assert!((mp.unsigned_area() - 25.0).abs() < 1e-9);
    }

    #[test]
    fn 経緯度変換() {
        let g = Grid { z: 16, extent: 4096 };
        // タイル 16/58089/25777 の左上隅
        let (lon, lat) = g.to_lonlat(g.to_global(58089, 25777, [0, 0]));
        assert!((lon - 139.0924).abs() < 1e-3, "{lon}");
        assert!((lat - 35.8124).abs() < 1e-3, "{lat}");
    }
}
