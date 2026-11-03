//! 作品保存(M7): 描きかけの状態を1ファイルへ保存/読込する。
//!
//! プリセット(H3)・パレット(M5d)・ストローク(H5)が「軽い設定 JSON」なのに対し、作品は
//! 全シミュレーションテクスチャ(湿レイヤー)+乾燥レイヤー+線画+レイヤーごとパレット(M5c)を
//! 含む数十 MB の生 f32 データを持つ。JSON base64 だと肥大化・低速なので **独自バイナリ1ファイル**
//! (`works/*.kasane`)にする: 先頭に軽いメタ情報(SimParams・パレット・レイヤー構成)を JSON で置き、
//! 続けて生 f32 ブロブを固定順で並べる。作品ファイルは使い捨てでなく蓄積されるが git 管理外
//! (スナップショット同様、比較用でなくユーザーの制作物なので）。
//!
//! GPU ⇄ f32 配列の変換は [`crate::gpu::WorkTextures`](GPU 依存)、ここはファイル形式だけを扱う。
//! ファイル入出力を1モジュールに閉じ込めることで、将来 Web 版で保存先を差し替える余地を残す
//! (docs/design.md §3.8 の「保存の境界を1モジュールに閉じる」)。

use crate::gpu::{BASE_SLICES, MAX_LAYERS, WorkTextures};
use paint_core::sim::CANVAS_SIZES;
use paint_core::sim::SimParams;
use pigment::Palette;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// ファイル先頭の識別子(かさねいろ形式バージョン 1)。読込時に検査する。
/// リリース前は互換を持たない方針なので、形式を変えたらここを上げて古いファイルは拒否する
const MAGIC: &[u8; 4] = b"KSN1";

/// 作品ファイルの拡張子(JSON プリセット等と区別する)
pub(crate) const EXT: &str = "kasane";

/// レイヤー構成1枚分(GpuCanvas::DriedLayer と対応。GPU 非依存に持つ)
#[derive(Clone, Copy, Serialize, Deserialize, PartialEq, Debug)]
pub struct StoredLayer {
    pub slot: u32,
    pub visible: bool,
    /// 濃度倍率。省略時 1(倍率を持たない作品は乾かしたときのままで読む)
    #[serde(default = "unit_density")]
    pub density: f32,
}

fn unit_density() -> f32 {
    1.0
}

/// ファイル先頭のメタ情報(JSON)。生 f32 ブロブの並びを解釈するための寸法も持つ
#[derive(Serialize, Deserialize, PartialEq, Debug)]
struct WorkMeta {
    /// 保存時のキャンバス1辺。読込時は app がこのサイズでキャンバスを作り直して復元する(M8)
    canvas_size: u32,
    /// 乾燥レイヤーのスライス数(生ブロブの個数を決める)
    layer_count: u32,
    params: SimParams,
    palette: Palette,
    /// 重ね順・可視性(先頭が最下層)
    layers: Vec<StoredLayer>,
    /// M5h: 乾燥レイヤーの記録時パレット(index = slot、名前・ρ/ω/γ 込みの正典)。
    /// decode が layer_count との個数一致を検査する(以降の不変条件 len == layers.len() の入口)
    layer_palettes: Vec<Palette>,
    /// 下の乾燥レイヤーを焼き固めた下地があるか。true のときだけ latent の後ろに下地のブロブが続く
    /// (下地のない作品は従来どおりの並びで読み書きできる)
    #[serde(default)]
    has_base: bool,
}

/// 1作品分の全状態(メタ + GPU テクスチャの生データ)。
/// app が GpuCanvas から集めて save に渡す / load が返して app が復元する
pub struct WorkFile {
    /// キャンバス1辺(M8)。保存時のサイズがブロブの寸法を決め、読込時は app が
    /// このサイズでキャンバスを作り直してから復元する
    pub canvas_size: u32,
    pub params: SimParams,
    pub palette: Palette,
    pub layers: Vec<StoredLayer>,
    /// M5h: 乾燥レイヤーの記録時パレット(index = slot)。層数と常に一致(decode が検査)
    pub layer_palettes: Vec<Palette>,
    pub textures: WorkTextures,
}

pub fn works_dir() -> PathBuf {
    // アセット(git 管理)ではなくユーザーの制作物なので基準ディレクトリ直下 works/ に置く
    // (snapshots/ と同じ扱い。.gitignore 済み。配布ビルドでは exe 隣 — assets.rs)
    crate::assets::base_dir().join("works")
}

/// 保存済み作品名の一覧(拡張子なし・ソート済み)
pub fn list() -> Vec<String> {
    crate::assets::list_names(&works_dir(), EXT)
}

/// 同名の作品が既にあるか(上書き確認用。名前は保存時と同じく正規化して判定する)
pub fn exists(name: &str) -> bool {
    crate::assets::named_exists(&works_dir(), name, EXT)
}

/// 作品を保存する。名前は `sanitize_name` で正規化し、`{name}.kasane.tmp` に書いてから
/// rename で置き換える(書き込み途中で落ちても既存の作品は壊れない)
pub fn save(name: &str, work: &WorkFile) -> Result<PathBuf, String> {
    let bytes = encode(work)?;
    crate::assets::save_named(&works_dir(), name, EXT, &bytes)
}

pub fn load(name: &str) -> Result<WorkFile, String> {
    let bytes = crate::assets::read_named(&works_dir(), name, EXT)?;
    decode(&bytes).map_err(|e| format!("{name}.{EXT} の形式が不正です: {e}"))
}

/// WorkFile → バイト列。MAGIC + メタ長 + メタ JSON + 生 f32 ブロブ(固定順)
pub(crate) fn encode(work: &WorkFile) -> Result<Vec<u8>, String> {
    let meta = WorkMeta {
        canvas_size: work.canvas_size,
        layer_count: work.textures.dried.len() as u32,
        params: work.params,
        palette: work.palette.clone(),
        layers: work.layers.clone(),
        layer_palettes: work.layer_palettes.clone(),
        has_base: work.textures.base.is_some(),
    };
    let meta_json = serde_json::to_vec(&meta).map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    out.extend_from_slice(MAGIC);
    out.extend_from_slice(&(meta_json.len() as u32).to_le_bytes());
    out.extend_from_slice(&meta_json);
    // 生ブロブ: 湿レイヤー3 → 乾燥レイヤー → 線画3 → latent(以上 f32)→ 下地(f16、あるときだけ)。
    // persist.rs の export と同順
    for v in &work.textures.wet {
        out.extend_from_slice(bytemuck::cast_slice(v));
    }
    for v in &work.textures.dried {
        out.extend_from_slice(bytemuck::cast_slice(v));
    }
    for v in &work.textures.lines {
        out.extend_from_slice(bytemuck::cast_slice(v));
    }
    out.extend_from_slice(bytemuck::cast_slice(&work.textures.latents));
    if let Some(base) = &work.textures.base {
        out.extend_from_slice(bytemuck::cast_slice(base));
    }
    Ok(out)
}

/// バイト列 → WorkFile。ブロブの並びはメタの canvas_size / layer_count から決まる
fn decode(bytes: &[u8]) -> Result<WorkFile, String> {
    let mut cur = 0usize;
    if bytes.len() < 8 || &bytes[0..4] != MAGIC {
        return Err("識別子が一致しません(作品ファイルではありません)".to_owned());
    }
    cur += 4;
    let meta_len = u32::from_le_bytes(bytes[cur..cur + 4].try_into().unwrap()) as usize;
    cur += 4;
    let meta_end = cur + meta_len;
    if meta_end > bytes.len() {
        return Err("メタ情報が途中で切れています".to_owned());
    }
    let meta: WorkMeta =
        serde_json::from_slice(&bytes[cur..meta_end]).map_err(|e| e.to_string())?;
    cur = meta_end;

    // 選択肢にないサイズは不正データ扱い(壊れたメタで巨大確保に走らないための検査。M8)
    if !CANVAS_SIZES.contains(&meta.canvas_size) {
        return Err(format!(
            "未対応のキャンバスサイズです(保存={} / 選択肢={CANVAS_SIZES:?})",
            meta.canvas_size
        ));
    }
    // 乾燥レイヤーは GPU 側のスライス数(MAX_LAYERS)まで。レイヤー構成は層数と 1:1 で、
    // 各 slot はスライス番号として層数未満
    if meta.layer_count as usize > MAX_LAYERS {
        return Err(format!(
            "乾燥レイヤーが多すぎます({} > {MAX_LAYERS})",
            meta.layer_count
        ));
    }
    if meta.layers.len() != meta.layer_count as usize {
        return Err(format!(
            "レイヤー構成の個数が層数と一致しません({} != {})",
            meta.layers.len(),
            meta.layer_count
        ));
    }
    if let Some(bad) = meta.layers.iter().find(|l| l.slot >= meta.layer_count) {
        return Err(format!(
            "レイヤーのスロット番号が層数を超えています(slot={} / 層数={})",
            bad.slot, meta.layer_count
        ));
    }
    if let Some(bad) = meta
        .layers
        .iter()
        .find(|l| !(l.density.is_finite() && l.density >= 0.0))
    {
        return Err(format!("レイヤーの濃度倍率が不正です({})", bad.density));
    }
    // M5h: 記録時パレットは層数と 1:1(GpuCanvas::layer_palettes の不変条件の入口検査)
    if meta.layer_palettes.len() != meta.layer_count as usize {
        return Err(format!(
            "レイヤーごとパレットの個数が層数と一致しません({} != {})",
            meta.layer_palettes.len(),
            meta.layer_count
        ));
    }

    let texels = (meta.canvas_size * meta.canvas_size) as usize;
    let rgba = texels * 4;
    let latent_len = crate::gpu::LATENT_TOTAL * 4;

    let wet = [
        take_f32(bytes, &mut cur, rgba)?,
        take_f32(bytes, &mut cur, rgba)?,
        take_f32(bytes, &mut cur, rgba)?,
    ];
    let mut dried = Vec::with_capacity(meta.layer_count as usize);
    for _ in 0..meta.layer_count {
        dried.push(take_f32(bytes, &mut cur, rgba)?);
    }
    let lines = [
        take_f32(bytes, &mut cur, texels)?,
        take_f32(bytes, &mut cur, texels)?,
        take_f32(bytes, &mut cur, texels)?,
    ];
    let latents = take_f32(bytes, &mut cur, latent_len)?;
    let base = if meta.has_base {
        Some(take_u16(bytes, &mut cur, BASE_SLICES * rgba)?)
    } else {
        None
    };
    if cur != bytes.len() {
        return Err(format!(
            "テクスチャデータの後ろに余分なデータがあります({} バイト)",
            bytes.len() - cur
        ));
    }

    Ok(WorkFile {
        canvas_size: meta.canvas_size,
        params: meta.params,
        palette: meta.palette,
        layers: meta.layers,
        layer_palettes: meta.layer_palettes,
        textures: WorkTextures {
            wet,
            dried,
            lines,
            latents,
            base,
        },
    })
}

/// バイト列から `count` 個の u16(下地の f16 の生ビット)を読み進める
fn take_u16(bytes: &[u8], cur: &mut usize, count: usize) -> Result<Vec<u16>, String> {
    let end = *cur + count * 2;
    if end > bytes.len() {
        return Err("下地のデータが途中で切れています".to_owned());
    }
    let v: Vec<u16> = bytes[*cur..end]
        .chunks_exact(2)
        .map(|c| u16::from_le_bytes([c[0], c[1]]))
        .collect();
    *cur = end;
    Ok(v)
}

/// バイト列から `count` 個の f32 を読み進める。ブロブ開始オフセットは 4 の倍数とは限らない
/// (メタ JSON 長が可変)ため、cast_slice(要整列)でなく from_le_bytes で1個ずつ組む
fn take_f32(bytes: &[u8], cur: &mut usize, count: usize) -> Result<Vec<f32>, String> {
    let end = *cur + count * 4;
    if end > bytes.len() {
        return Err("テクスチャデータが途中で切れています".to_owned());
    }
    let v: Vec<f32> = bytes[*cur..end]
        .chunks_exact(4)
        .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
        .collect();
    *cur = end;
    Ok(v)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 実寸のダミーデータで encode → decode の往復を検査(形式・ブロブ順の回帰チェック)。
    /// 値はインデックス由来の決定論値にして、並びの取り違えも検出できるようにする。
    /// M8: サイズ可変化に伴い、既定以外のキャンバスサイズでも往復できることを検査する
    #[test]
    fn roundtrip() {
        for canvas_size in [512u32, 1024] {
            let texels = (canvas_size * canvas_size) as usize;
            let ramp = |base: f32, n: usize| -> Vec<f32> {
                (0..n).map(|i| base + i as f32 * 1e-4).collect()
            };
            let textures = WorkTextures {
                wet: [
                    ramp(1.0, texels * 4),
                    ramp(2.0, texels * 4),
                    ramp(3.0, texels * 4),
                ],
                dried: vec![ramp(4.0, texels * 4), ramp(5.0, texels * 4)],
                lines: [ramp(6.0, texels), ramp(7.0, texels), ramp(8.0, texels)],
                latents: ramp(9.0, crate::gpu::LATENT_TOTAL * 4),
                base: Some((0..BASE_SLICES * texels * 4).map(|i| i as u16).collect()),
            };
            // M5h: レイヤーごとパレット(index = slot)。1枚目だけ色を変えて取り違えも検出する
            let mut recorded = Palette::default_palette();
            recorded.pigments[0].rgb = [1, 2, 3];
            let work = WorkFile {
                canvas_size,
                params: SimParams {
                    brush_radius: 12.5,
                    ..Default::default()
                },
                palette: Palette::default_palette(),
                layers: vec![
                    StoredLayer {
                        slot: 0,
                        visible: true,
                        density: 1.0,
                    },
                    StoredLayer {
                        slot: 1,
                        visible: false,
                        density: 0.4,
                    },
                ],
                layer_palettes: vec![recorded, Palette::default_palette()],
                textures: textures.clone(),
            };

            let bytes = encode(&work).unwrap();
            let back = decode(&bytes).unwrap();
            assert_eq!(back.canvas_size, canvas_size);
            assert_eq!(back.params, work.params);
            assert_eq!(back.palette, work.palette);
            assert_eq!(back.layers, work.layers);
            assert_eq!(back.layer_palettes, work.layer_palettes);
            // テクスチャは巨大なので assert_eq!(Debug ダンプ)を避け、等価判定だけ行う
            assert!(back.textures == textures, "テクスチャの往復が一致しません");
        }
    }

    /// レイヤーごとパレット(M5h)の個数が層数と合わないメタは拒否されること
    /// (GpuCanvas::layer_palettes の不変条件 len == layers.len() の入口検査)
    #[test]
    fn rejects_layer_palette_count_mismatch() {
        let meta = serde_json::to_vec(&WorkMeta {
            canvas_size: 512,
            layer_count: 2,
            params: SimParams::default(),
            palette: Palette::default_palette(),
            layers: vec![
                StoredLayer {
                    slot: 0,
                    visible: true,
                    density: 1.0,
                },
                StoredLayer {
                    slot: 1,
                    visible: true,
                    density: 1.0,
                },
            ],
            layer_palettes: vec![Palette::default_palette()], // 2層なのに1個
            has_base: false,
        })
        .unwrap();
        let mut bytes = Vec::new();
        bytes.extend_from_slice(MAGIC);
        bytes.extend_from_slice(&(meta.len() as u32).to_le_bytes());
        bytes.extend_from_slice(&meta);
        match decode(&bytes) {
            Err(e) => assert!(
                e.contains("パレットの個数"),
                "個数検査で拒否されるはず: {e}"
            ),
            Ok(_) => panic!("個数不一致が受理されました"),
        }
    }

    /// メタ JSON だけのバイト列(ブロブなし)。メタ段階の検査で拒否されることを見るテスト用
    fn meta_only(meta: &WorkMeta) -> Vec<u8> {
        let json = serde_json::to_vec(meta).unwrap();
        let mut bytes = Vec::new();
        bytes.extend_from_slice(MAGIC);
        bytes.extend_from_slice(&(json.len() as u32).to_le_bytes());
        bytes.extend_from_slice(&json);
        bytes
    }

    fn expect_err(bytes: &[u8], needle: &str) {
        match decode(bytes) {
            Err(e) => assert!(
                e.contains(needle),
                "「{needle}」の検査で拒否されるはず: {e}"
            ),
            Ok(_) => panic!("「{needle}」の不正が受理されました"),
        }
    }

    /// レイヤー構成の不正(層数が MAX_LAYERS 超・構成と層数の不一致・層数以上の slot)は拒否されること
    #[test]
    fn rejects_bad_layer_layout() {
        let layered = |layer_count: u32, slots: &[u32]| WorkMeta {
            canvas_size: 512,
            layer_count,
            params: SimParams::default(),
            palette: Palette::default_palette(),
            layers: slots
                .iter()
                .map(|&slot| StoredLayer {
                    slot,
                    visible: true,
                    density: 1.0,
                })
                .collect(),
            layer_palettes: vec![Palette::default_palette(); layer_count as usize],
            has_base: false,
        };
        let too_many: Vec<u32> = (0..MAX_LAYERS as u32 + 1).collect();
        expect_err(
            &meta_only(&layered(MAX_LAYERS as u32 + 1, &too_many)),
            "多すぎます",
        );
        expect_err(&meta_only(&layered(2, &[0])), "レイヤー構成の個数");
        expect_err(&meta_only(&layered(2, &[0, 2])), "スロット番号");
    }

    /// 下地の欄がないメタ(下地を持たない作品)は下地なしとして読めること
    #[test]
    fn reads_meta_without_base_field() {
        let texels = 512 * 512;
        let work = WorkFile {
            canvas_size: 512,
            params: SimParams::default(),
            palette: Palette::default_palette(),
            layers: Vec::new(),
            layer_palettes: Vec::new(),
            textures: WorkTextures {
                wet: [
                    vec![0.0; texels * 4],
                    vec![0.0; texels * 4],
                    vec![0.0; texels * 4],
                ],
                dried: Vec::new(),
                lines: [vec![0.0; texels], vec![0.0; texels], vec![0.0; texels]],
                latents: vec![0.0; crate::gpu::LATENT_TOTAL * 4],
                base: None,
            },
        };
        let bytes = encode(&work).unwrap();
        let meta_len = u32::from_le_bytes(bytes[4..8].try_into().unwrap()) as usize;
        let mut meta: serde_json::Value = serde_json::from_slice(&bytes[8..8 + meta_len]).unwrap();
        meta.as_object_mut()
            .unwrap()
            .remove("has_base")
            .expect("has_base 欄がある");
        let meta = serde_json::to_vec(&meta).unwrap();
        let mut old = Vec::new();
        old.extend_from_slice(MAGIC);
        old.extend_from_slice(&(meta.len() as u32).to_le_bytes());
        old.extend_from_slice(&meta);
        old.extend_from_slice(&bytes[8 + meta_len..]);
        let back = decode(&old).unwrap();
        assert!(back.textures.base.is_none());
    }

    /// 正しい作品の後ろに余分なバイトが付いていたら拒否されること
    #[test]
    fn rejects_trailing_bytes() {
        let texels = 512 * 512;
        let work = WorkFile {
            canvas_size: 512,
            params: SimParams::default(),
            palette: Palette::default_palette(),
            layers: Vec::new(),
            layer_palettes: Vec::new(),
            textures: WorkTextures {
                wet: [
                    vec![0.0; texels * 4],
                    vec![0.0; texels * 4],
                    vec![0.0; texels * 4],
                ],
                dried: Vec::new(),
                lines: [vec![0.0; texels], vec![0.0; texels], vec![0.0; texels]],
                latents: vec![0.0; crate::gpu::LATENT_TOTAL * 4],
                base: None,
            },
        };
        let mut bytes = encode(&work).unwrap();
        assert!(decode(&bytes).is_ok());
        bytes.push(0);
        expect_err(&bytes, "余分なデータ");
    }

    /// 壊れたデータ(識別子違い・サイズ不足・未対応キャンバスサイズ)は Err になること
    #[test]
    fn rejects_garbage() {
        assert!(decode(b"not a work file").is_err());
        assert!(decode(&[]).is_err());
        // 未対応サイズのメタは巨大確保に進まず拒否される(M8)
        let meta = serde_json::to_vec(&WorkMeta {
            canvas_size: 123_456,
            layer_count: 0,
            params: SimParams::default(),
            palette: Palette::default_palette(),
            layers: Vec::new(),
            layer_palettes: Vec::new(),
            has_base: false,
        })
        .unwrap();
        let mut bytes = Vec::new();
        bytes.extend_from_slice(MAGIC);
        bytes.extend_from_slice(&(meta.len() as u32).to_le_bytes());
        bytes.extend_from_slice(&meta);
        // WorkFile は巨大バッファ持ちで Debug 未実装のため unwrap_err でなく match で検査
        match decode(&bytes) {
            Err(e) => assert!(
                e.contains("キャンバスサイズ"),
                "サイズ検査で拒否されるはず: {e}"
            ),
            Ok(_) => panic!("未対応サイズが受理されました"),
        }
    }
}
