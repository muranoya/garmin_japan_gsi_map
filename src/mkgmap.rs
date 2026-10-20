//! mkgmap の取得と実行。

use anyhow::{Context, Result, bail};
use std::path::{Path, PathBuf};
use std::process::Command;

pub const VERSION: &str = "r4924";

/// リポジトリ同梱のスタイル（`style/gsi`）。`--style` 未指定時に作業フォルダへ書き出して使う。
const STYLE_FILES: &[(&str, &str)] = &[
    ("version", include_str!("../style/gsi/version")),
    ("info", include_str!("../style/gsi/info")),
    ("options", include_str!("../style/gsi/options")),
    ("lines", include_str!("../style/gsi/lines")),
    ("polygons", include_str!("../style/gsi/polygons")),
    ("points", include_str!("../style/gsi/points")),
];

/// リポジトリ同梱の TYP ファイル（テキスト形式。mkgmap がコンパイルする）。
const TYP: &str = include_str!("../style/gsi.typ.txt");

pub fn write_bundled_typ(path: &Path) -> Result<()> {
    std::fs::write(path, TYP)?;
    Ok(())
}

pub fn write_bundled_style(dir: &Path) -> Result<()> {
    std::fs::create_dir_all(dir)?;
    for (name, body) in STYLE_FILES {
        std::fs::write(dir.join(name), body)?;
    }
    Ok(())
}

fn cache_dir() -> Result<PathBuf> {
    let base = match std::env::var_os("XDG_CACHE_HOME") {
        Some(d) if !d.is_empty() => PathBuf::from(d),
        _ => PathBuf::from(std::env::var_os("HOME").context("HOME が未設定です")?).join(".cache"),
    };
    Ok(base.join("gsi2garmin"))
}

/// mkgmap.jar のパスを返す。未指定ならキャッシュを使い、なければダウンロードして展開する。
pub async fn ensure_jar(explicit: Option<PathBuf>) -> Result<PathBuf> {
    if let Some(jar) = explicit {
        if !jar.is_file() {
            bail!("mkgmap.jar が見つかりません: {}", jar.display());
        }
        return Ok(jar);
    }
    let cache = cache_dir()?;
    let jar = cache.join(format!("mkgmap-{VERSION}")).join("mkgmap.jar");
    if jar.is_file() {
        return Ok(jar);
    }

    let url = format!("https://www.mkgmap.org.uk/download/mkgmap-{VERSION}.zip");
    eprintln!("mkgmap {VERSION} をダウンロードします: {url}");
    let resp = reqwest::get(&url).await?.error_for_status()?;
    let bytes = resp.bytes().await?;
    std::fs::create_dir_all(&cache)?;
    let mut zip = zip::ZipArchive::new(std::io::Cursor::new(bytes))?;
    zip.extract(&cache)?;
    if !jar.is_file() {
        bail!("展開した mkgmap に mkgmap.jar がありません: {}", jar.display());
    }
    Ok(jar)
}

pub struct MapInfo {
    pub family_id: u16,
    pub name: String,
    pub description: String,
    pub code_page: u16,
}

/// mkgmap を実行して gmapsupp.img を作る。生成したファイルのパスを返す。
/// `inputs` には OSM ファイルと TYP のテキストファイルを渡す。
pub fn run(
    jar: &Path,
    style_dir: &Path,
    inputs: &[PathBuf],
    out_dir: &Path,
    info: &MapInfo,
) -> Result<PathBuf> {
    std::fs::create_dir_all(out_dir)?;
    let mut cmd = Command::new("java");
    cmd.arg("-jar")
        .arg(jar)
        .arg(format!("--output-dir={}", out_dir.display()))
        .arg(format!("--style-file={}", style_dir.display()))
        .arg(format!("--code-page={}", info.code_page))
        .arg(format!("--family-id={}", info.family_id))
        .arg(format!("--mapname={}0001", info.family_id))
        .arg("--product-id=1")
        .arg(format!("--family-name={}", info.name))
        .arg(format!("--series-name={}", info.name))
        .arg(format!("--area-name={}", info.description))
        .arg(format!("--description={}", info.description))
        .arg("--gmapsupp")
        .args(inputs);
    eprintln!("mkgmap を実行します");
    let status = cmd
        .status()
        .context("java を起動できません。Java（17 以上）をインストールしてください")?;
    if !status.success() {
        bail!("mkgmap が失敗しました（{status}）");
    }
    let img = out_dir.join("gmapsupp.img");
    if !img.is_file() {
        bail!("mkgmap の出力に gmapsupp.img がありません: {}", out_dir.display());
    }
    Ok(img)
}
