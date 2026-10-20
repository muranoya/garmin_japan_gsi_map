//! mkgmap に渡す OSM XML の書き出し。

use crate::geom::{Grid, P};
use anyhow::Result;
use std::io::Write;

pub type Tags = Vec<(String, String)>;

pub enum Feature {
    Point(P),
    Line(Vec<P>),
    /// [外周, 穴, ...]。各リングは始点を末尾に繰り返さない。
    Polygon(Vec<Vec<P>>),
}

/// タグ付きの地物。`tags` はタグ集合の表（`TagTable`）への添字。
pub struct Item {
    pub tags: usize,
    pub feature: Feature,
}

/// 地物を OSM XML として書き出す。
///
/// ノード → ウェイ → リレーションの順に出力する必要があるため、地物列を 3 回走査する。
/// ID は走査順に決まるので、各パスで同じ規則で採番すれば参照が一致する。
pub fn write<W: Write>(w: &mut W, grid: &Grid, tag_table: &[Tags], items: &[Item]) -> Result<()> {
    writeln!(w, "<?xml version='1.0' encoding='UTF-8'?>")?;
    writeln!(w, "<osm version='0.6' generator='gsi2garmin'>")?;

    // ノード
    let mut node_id = 0u64;
    let mut node = |w: &mut W, p: P, tags: Option<&Tags>| -> Result<()> {
        node_id += 1;
        let (lon, lat) = grid.to_lonlat(p);
        match tags {
            None => writeln!(w, "<node id='{node_id}' lat='{lat:.7}' lon='{lon:.7}'/>")?,
            Some(tags) => {
                writeln!(w, "<node id='{node_id}' lat='{lat:.7}' lon='{lon:.7}'>")?;
                write_tags(w, tags)?;
                writeln!(w, "</node>")?;
            }
        }
        Ok(())
    };
    for item in items {
        match &item.feature {
            Feature::Point(p) => node(w, *p, Some(&tag_table[item.tags]))?,
            Feature::Line(coords) => coords.iter().try_for_each(|p| node(w, *p, None))?,
            Feature::Polygon(rings) => rings.iter().flatten().try_for_each(|p| node(w, *p, None))?,
        }
    }

    // ウェイ
    let (mut node_id, mut way_id) = (0u64, 0u64);
    for item in items {
        match &item.feature {
            Feature::Point(_) => node_id += 1,
            Feature::Line(coords) => {
                way_id += 1;
                writeln!(w, "<way id='{way_id}'>")?;
                for _ in coords {
                    node_id += 1;
                    writeln!(w, "<nd ref='{node_id}'/>")?;
                }
                write_tags(w, &tag_table[item.tags])?;
                writeln!(w, "</way>")?;
            }
            Feature::Polygon(rings) => {
                // 穴がなければ閉じたウェイ 1 本、穴があれば無タグのリングをリレーションで束ねる
                let tags = (rings.len() == 1).then(|| &tag_table[item.tags]);
                for ring in rings {
                    way_id += 1;
                    writeln!(w, "<way id='{way_id}'>")?;
                    let first = node_id + 1;
                    for _ in ring {
                        node_id += 1;
                        writeln!(w, "<nd ref='{node_id}'/>")?;
                    }
                    writeln!(w, "<nd ref='{first}'/>")?;
                    if let Some(tags) = tags {
                        write_tags(w, tags)?;
                    }
                    writeln!(w, "</way>")?;
                }
            }
        }
    }

    // リレーション（穴のあるポリゴン）
    let (mut way_id, mut rel_id) = (0u64, 0u64);
    for item in items {
        match &item.feature {
            Feature::Point(_) => {}
            Feature::Line(_) => way_id += 1,
            Feature::Polygon(rings) if rings.len() == 1 => way_id += 1,
            Feature::Polygon(rings) => {
                rel_id += 1;
                writeln!(w, "<relation id='{rel_id}'>")?;
                for i in 0..rings.len() {
                    way_id += 1;
                    let role = if i == 0 { "outer" } else { "inner" };
                    writeln!(w, "<member type='way' ref='{way_id}' role='{role}'/>")?;
                }
                writeln!(w, "<tag k='type' v='multipolygon'/>")?;
                write_tags(w, &tag_table[item.tags])?;
                writeln!(w, "</relation>")?;
            }
        }
    }
    writeln!(w, "</osm>")?;
    Ok(())
}

fn write_tags<W: Write>(w: &mut W, tags: &Tags) -> Result<()> {
    for (k, v) in tags {
        writeln!(w, "<tag k='{}' v='{}'/>", escape(k), escape(v))?;
    }
    Ok(())
}

fn escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '\'' => out.push_str("&apos;"),
            '"' => out.push_str("&quot;"),
            c if c < ' ' => out.push(' '),
            c => out.push(c),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tags(pairs: &[(&str, &str)]) -> Tags {
        pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
    }

    #[test]
    fn ノードウェイリレーションの参照が一致する() -> Result<()> {
        let grid = Grid { z: 0, extent: 4096 };
        let table = vec![tags(&[("gsi_layer", "label"), ("knj", "雲取山 & <x>")]), tags(&[("a", "b")])];
        let sq = |o: f64| vec![[o, o], [o + 1.0, o], [o + 1.0, o + 1.0]];
        let items = vec![
            Item { tags: 0, feature: Feature::Point([2048.0, 2048.0]) },
            Item { tags: 1, feature: Feature::Line(vec![[0.0, 0.0], [1.0, 1.0]]) },
            Item { tags: 1, feature: Feature::Polygon(vec![sq(10.0)]) },
            Item { tags: 1, feature: Feature::Polygon(vec![sq(20.0), sq(20.5)]) },
        ];
        let mut out = Vec::new();
        write(&mut out, &grid, &table, &items)?;
        let xml = String::from_utf8(out)?;

        assert!(xml.contains("<tag k='knj' v='雲取山 &amp; &lt;x&gt;'/>"));
        assert!(xml.contains("<node id='1' lat='0.0000000' lon='0.0000000'>"));
        // 線は node 2,3、穴なしポリゴンは node 4-6 を閉じる
        assert!(xml.contains("<way id='1'>\n<nd ref='2'/>\n<nd ref='3'/>\n"));
        assert!(xml.contains("<way id='2'>\n<nd ref='4'/>\n<nd ref='5'/>\n<nd ref='6'/>\n<nd ref='4'/>\n<tag k='a'"));
        // 穴ありポリゴンのリングは無タグで、リレーションが way 3, 4 を参照する
        assert!(xml.contains("<way id='3'>\n<nd ref='7'/>\n<nd ref='8'/>\n<nd ref='9'/>\n<nd ref='7'/>\n</way>"));
        assert!(xml.contains("<member type='way' ref='3' role='outer'/>\n<member type='way' ref='4' role='inner'/>"));
        assert_eq!(xml.matches("<node ").count(), 12);
        Ok(())
    }
}
