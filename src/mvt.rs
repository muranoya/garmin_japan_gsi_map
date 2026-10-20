//! Mapbox Vector Tile (v2) のデコード。
//!
//! 仕様: <https://github.com/mapbox/vector-tile-spec/tree/master/2.1>
//! protobuf のメッセージ定義は小さいので、build.rs を使わず prost の derive で直接定義する。

use anyhow::{Result, bail};
use prost::Message;

mod pb {
    #[derive(Clone, PartialEq, prost::Message)]
    pub struct Tile {
        #[prost(message, repeated, tag = "3")]
        pub layers: Vec<Layer>,
    }

    #[derive(Clone, PartialEq, prost::Message)]
    pub struct Layer {
        #[prost(string, required, tag = "1")]
        pub name: String,
        #[prost(message, repeated, tag = "2")]
        pub features: Vec<Feature>,
        #[prost(string, repeated, tag = "3")]
        pub keys: Vec<String>,
        #[prost(message, repeated, tag = "4")]
        pub values: Vec<Value>,
        #[prost(uint32, optional, tag = "5")]
        pub extent: Option<u32>,
    }

    #[derive(Clone, PartialEq, prost::Message)]
    pub struct Feature {
        #[prost(uint32, repeated, tag = "2")]
        pub tags: Vec<u32>,
        #[prost(int32, optional, tag = "3")]
        pub r#type: Option<i32>,
        #[prost(uint32, repeated, tag = "4")]
        pub geometry: Vec<u32>,
    }

    #[derive(Clone, PartialEq, prost::Message)]
    pub struct Value {
        #[prost(string, optional, tag = "1")]
        pub string_value: Option<String>,
        #[prost(float, optional, tag = "2")]
        pub float_value: Option<f32>,
        #[prost(double, optional, tag = "3")]
        pub double_value: Option<f64>,
        #[prost(int64, optional, tag = "4")]
        pub int_value: Option<i64>,
        #[prost(uint64, optional, tag = "5")]
        pub uint_value: Option<u64>,
        #[prost(sint64, optional, tag = "6")]
        pub sint_value: Option<i64>,
        #[prost(bool, optional, tag = "7")]
        pub bool_value: Option<bool>,
    }
}

/// タイル内座標（y は下向き）。バッファ領域を含むため 0..extent の範囲外もとり得る。
pub type Point = [i32; 2];

#[derive(Debug, Clone, PartialEq)]
pub enum Geometry {
    Points(Vec<Point>),
    Lines(Vec<Vec<Point>>),
    /// ポリゴンごとに [外周, 穴, 穴, ...]。リングは始点を末尾に繰り返さない。
    Polygons(Vec<Vec<Vec<Point>>>),
}

#[derive(Debug, Clone)]
pub struct Feature {
    /// 属性を文字列化したもの（キー順はタイル内の出現順）。
    pub props: Vec<(String, String)>,
    pub geometry: Geometry,
}

#[derive(Debug, Clone)]
pub struct Layer {
    pub name: String,
    pub extent: u32,
    pub features: Vec<Feature>,
}

pub fn decode(data: &[u8]) -> Result<Vec<Layer>> {
    let tile = pb::Tile::decode(data)?;
    tile.layers.into_iter().map(decode_layer).collect()
}

fn decode_layer(layer: pb::Layer) -> Result<Layer> {
    let values: Vec<String> = layer.values.iter().map(value_to_string).collect();
    let mut features = Vec::with_capacity(layer.features.len());
    for f in &layer.features {
        let mut props = Vec::with_capacity(f.tags.len() / 2);
        for [k, v] in f.tags.as_chunks::<2>().0 {
            let (Some(k), Some(v)) = (layer.keys.get(*k as usize), values.get(*v as usize))
            else {
                bail!("レイヤ {} の属性インデックスが範囲外です", layer.name);
            };
            props.push((k.clone(), v.clone()));
        }
        let geometry = match f.r#type.unwrap_or(0) {
            1 => Geometry::Points(decode_commands(&f.geometry)?.into_iter().flatten().collect()),
            2 => Geometry::Lines(decode_commands(&f.geometry)?),
            3 => Geometry::Polygons(group_rings(decode_commands(&f.geometry)?)),
            _ => continue, // UNKNOWN 型は仕様上無視してよい
        };
        features.push(Feature { props, geometry });
    }
    Ok(Layer { name: layer.name, extent: layer.extent.unwrap_or(4096), features })
}

fn value_to_string(v: &pb::Value) -> String {
    if let Some(s) = &v.string_value {
        s.clone()
    } else if let Some(i) = v.int_value.or(v.sint_value) {
        i.to_string()
    } else if let Some(u) = v.uint_value {
        u.to_string()
    } else if let Some(d) = v.double_value {
        d.to_string()
    } else if let Some(f) = v.float_value {
        f.to_string()
    } else if let Some(b) = v.bool_value {
        b.to_string()
    } else {
        String::new()
    }
}

/// ジオメトリコマンド列をパス（MoveTo ごとに区切った点列）に展開する。
fn decode_commands(cmds: &[u32]) -> Result<Vec<Vec<Point>>> {
    let mut paths: Vec<Vec<Point>> = Vec::new();
    let (mut x, mut y) = (0i32, 0i32);
    let mut i = 0;
    while i < cmds.len() {
        let id = cmds[i] & 0x7;
        let count = (cmds[i] >> 3) as usize;
        i += 1;
        match id {
            1 | 2 => {
                if i + count * 2 > cmds.len() {
                    bail!("ジオメトリのパラメータが不足しています");
                }
                for _ in 0..count {
                    x += zigzag(cmds[i]);
                    y += zigzag(cmds[i + 1]);
                    i += 2;
                    if id == 1 {
                        paths.push(vec![[x, y]]);
                    } else {
                        let Some(path) = paths.last_mut() else {
                            bail!("MoveTo の前に LineTo があります");
                        };
                        path.push([x, y]);
                    }
                }
            }
            7 => {} // ClosePath: リングは暗黙に閉じているものとして扱う
            _ => bail!("未知のジオメトリコマンド {id}"),
        }
    }
    Ok(paths)
}

fn zigzag(v: u32) -> i32 {
    ((v >> 1) as i32) ^ -((v & 1) as i32)
}

/// y 下向き座標での符号付き面積の 2 倍。仕様では正が外周、負が穴。
pub fn signed_area2(ring: &[Point]) -> i64 {
    let n = ring.len();
    (0..n)
        .map(|i| {
            let [x0, y0] = ring[i];
            let [x1, y1] = ring[(i + 1) % n];
            i64::from(x0) * i64::from(y1) - i64::from(x1) * i64::from(y0)
        })
        .sum()
}

/// リング列を「外周 + それに続く穴」のポリゴン単位にまとめる。面積 0 のリングは捨てる。
fn group_rings(rings: Vec<Vec<Point>>) -> Vec<Vec<Vec<Point>>> {
    let mut polygons: Vec<Vec<Vec<Point>>> = Vec::new();
    for ring in rings {
        let area = signed_area2(&ring);
        if ring.len() < 3 || area == 0 {
            continue;
        }
        match polygons.last_mut() {
            Some(poly) if area < 0 => poly.push(ring),
            _ if area < 0 => {} // 外周のない穴は捨てる
            _ => polygons.push(vec![ring]),
        }
    }
    polygons
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cmd(id: u32, count: u32) -> u32 {
        (count << 3) | id
    }

    fn zz(v: i32) -> u32 {
        ((v << 1) ^ (v >> 31)) as u32
    }

    fn encode_tile(layer: pb::Layer) -> Vec<u8> {
        pb::Tile { layers: vec![layer] }.encode_to_vec()
    }

    #[test]
    fn 仕様書の例の線をデコードできる() {
        // vector-tile-spec 4.3.5.2 の例: (2,2)-(2,10)-(10,10) と (1,1)-(3,5)
        let geom = vec![
            cmd(1, 1), zz(2), zz(2),
            cmd(2, 2), zz(0), zz(8), zz(8), zz(0),
            cmd(1, 1), zz(-9), zz(-9),
            cmd(2, 1), zz(2), zz(4),
        ];
        assert_eq!(
            decode_commands(&geom).unwrap(),
            vec![vec![[2, 2], [2, 10], [10, 10]], vec![[1, 1], [3, 5]]]
        );
    }

    #[test]
    fn ポリゴンは外周と穴にまとめる() {
        let outer = vec![[0, 0], [10, 0], [10, 10], [0, 10]]; // y 下向きで時計回り = 正
        let hole = vec![[2, 2], [2, 8], [8, 8], [8, 2]];
        assert!(signed_area2(&outer) > 0);
        assert!(signed_area2(&hole) < 0);
        let grouped = group_rings(vec![outer.clone(), hole.clone(), outer.clone()]);
        assert_eq!(grouped, vec![vec![outer.clone(), hole], vec![outer]]);
    }

    #[test]
    fn 属性を文字列化して取り出す() {
        let layer = pb::Layer {
            name: "contour".into(),
            features: vec![pb::Feature {
                tags: vec![0, 0, 1, 1],
                r#type: Some(2),
                geometry: vec![cmd(1, 1), zz(0), zz(0), cmd(2, 1), zz(5), zz(5)],
            }],
            keys: vec!["ftCode".into(), "orgGILvl".into()],
            values: vec![
                pb::Value { int_value: Some(7351), ..Default::default() },
                pb::Value { string_value: Some("25000".into()), ..Default::default() },
            ],
            extent: None,
        };
        let layers = decode(&encode_tile(layer)).unwrap();
        assert_eq!(layers[0].extent, 4096);
        let f = &layers[0].features[0];
        assert_eq!(
            f.props,
            vec![("ftCode".into(), "7351".into()), ("orgGILvl".into(), "25000".into())]
        );
        assert_eq!(f.geometry, Geometry::Lines(vec![vec![[0, 0], [5, 5]]]));
    }
}
