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
//! (plan §4 の「保存の trait 抽象を維持」)。

use crate::gpu::WorkTextures;
use paint_core::sim::CANVAS_SIZES;
use pigment::Palette;
use paint_core::sim::SimParams;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// ファイル先頭の識別子(形式バージョン 1)。読込時に検査する。
/// 旧名 my-paint 時代の "MPW1" のまま変えない(変えると既存の作品ファイルが読めなくなる)
const MAGIC: &[u8; 4] = b"MPW1";

/// 作品ファイルの拡張子(JSON プリセット等と区別する)。
/// 旧拡張子 .mpaint からの改名時に既存ファイルもリネーム済み(中身の形式は不変)
const EXT: &str = "kasane";

/// レイヤー構成1枚分(GpuCanvas::DriedLayer と対応。GPU 非依存に持つ)
#[derive(Clone, Copy, Serialize, Deserialize, PartialEq, Debug)]
pub struct StoredLayer {
    pub slot: u32,
    pub visible: bool,
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
    /// `#[serde(default)]` 必須: 旧 .kasane はこのフィールドを持たず、空 Vec で読めることが
    /// 後方互換の要(欠落分は app::load_work が latent 逆変換で正規化する)
    #[serde(default)]
    layer_palettes: Vec<Palette>,
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
    /// M5h: 乾燥レイヤーの記録時パレット(index = slot)。旧ファイルは空 Vec
    pub layer_palettes: Vec<Palette>,
    pub textures: WorkTextures,
}

pub fn works_dir() -> PathBuf {
    // アセット(git 管理)ではなくユーザーの制作物なのでリポジトリ直下 works/ に置く
    // (snapshots/ と同じ扱い。.gitignore 済み)
    Path::new(env!("CARGO_MANIFEST_DIR")).join("works")
}

/// 保存済み作品名の一覧(拡張子なし・ソート済み)
pub fn list() -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(works_dir()) else {
        return Vec::new();
    };
    let mut names: Vec<String> = entries
        .filter_map(|e| {
            let path = e.ok()?.path();
            if path.extension().is_some_and(|ext| ext == EXT) {
                Some(path.file_stem()?.to_string_lossy().into_owned())
            } else {
                None
            }
        })
        .collect();
    names.sort();
    names
}

pub fn save(name: &str, work: &WorkFile) -> Result<PathBuf, String> {
    let dir = works_dir();
    std::fs::create_dir_all(&dir).map_err(|e| format!("{} を作れません: {e}", dir.display()))?;
    let path = dir.join(format!("{name}.{EXT}"));
    let bytes = encode(work)?;
    std::fs::write(&path, bytes).map_err(|e| format!("{} に書けません: {e}", path.display()))?;
    Ok(path)
}

pub fn load(name: &str) -> Result<WorkFile, String> {
    let path = works_dir().join(format!("{name}.{EXT}"));
    let bytes =
        std::fs::read(&path).map_err(|e| format!("{} を読めません: {e}", path.display()))?;
    decode(&bytes).map_err(|e| format!("{name}.{EXT} の形式が不正です: {e}"))
}

/// M5h: 乾燥レイヤーの記録時パレットを層数(= textures.dried.len())に揃えて返す(読込時正規化)。
/// 旧 .kasane(layer_palettes 欠落=空 Vec)は latent ブロブ(M5c が色 latent を保存済み)から
/// 基本色を逆変換して補完する。名前・ρ/ω/γ は latent に無いため復元できず、名前は
/// 「層{n}の色{i}」の自動命名(UI がこの型で「色のみ復元」と判別する)、ρ/ω/γ は保存時
/// パレット(meta.palette)の同番スロット値を借りる(「色だけ差し替えて描き味は維持」)。
/// 呼び出し側(app::load_work)はこの結果を GpuCanvas::layer_palettes へ代入し、
/// 以降の UI は欠損の有無を意識しない(len == 層数 が常に成立)
pub fn normalized_layer_palettes(file: &WorkFile) -> Vec<Palette> {
    (0..file.textures.dried.len())
        .map(|slot| {
            file.layer_palettes.get(slot).cloned().unwrap_or_else(|| {
                // latent ブロブのレイアウト: グローバル光学 → スロット別パレット枠(persist.rs)
                let base = (pigment::GLOBAL_LATENTS + slot * pigment::PIGMENT_LATENTS) * 4;
                let mut block = [[0.0f32; 4]; pigment::PIGMENT_LATENTS];
                for (k, v4) in block.iter_mut().enumerate() {
                    let at = base + k * 4;
                    v4.copy_from_slice(&file.textures.latents[at..at + 4]);
                }
                let rgbs = pigment::latent_block_to_rgbs(&block);
                let mut pal = file.palette.clone();
                for (i, p) in pal.pigments.iter_mut().enumerate() {
                    p.rgb = rgbs[i];
                    p.name = fallback_pigment_name(slot, i);
                }
                pal
            })
        })
        .collect()
}

/// 旧 .kasane フォールバック復元の自動命名(M5h)。「層{n}の色{i}」型=由来が名前から分かる。
/// UI(乾いた層の抽出パネル)はこの型の名前で「※色のみ復元」の注記を出す
pub fn fallback_pigment_name(slot: usize, index: usize) -> String {
    format!("層{}の色{}", slot + 1, index + 1)
}

/// WorkFile → バイト列。MAGIC + メタ長 + メタ JSON + 生 f32 ブロブ(固定順)
fn encode(work: &WorkFile) -> Result<Vec<u8>, String> {
    let meta = WorkMeta {
        canvas_size: work.canvas_size,
        layer_count: work.textures.dried.len() as u32,
        params: work.params,
        palette: work.palette.clone(),
        layers: work.layers.clone(),
        layer_palettes: work.layer_palettes.clone(),
    };
    let meta_json = serde_json::to_vec(&meta).map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    out.extend_from_slice(MAGIC);
    out.extend_from_slice(&(meta_json.len() as u32).to_le_bytes());
    out.extend_from_slice(&meta_json);
    // 生 f32 ブロブ: 湿レイヤー3 → 乾燥レイヤー → 線画3 → latent(persist.rs の export と同順)
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
        },
    })
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
                wet: [ramp(1.0, texels * 4), ramp(2.0, texels * 4), ramp(3.0, texels * 4)],
                dried: vec![ramp(4.0, texels * 4), ramp(5.0, texels * 4)],
                lines: [ramp(6.0, texels), ramp(7.0, texels), ramp(8.0, texels)],
                latents: ramp(9.0, crate::gpu::LATENT_TOTAL * 4),
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
                    StoredLayer { slot: 0, visible: true },
                    StoredLayer { slot: 1, visible: false },
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

    /// 旧 .kasane(メタ JSON に layer_palettes フィールドが無い)が空 Vec で読めること
    /// (M5h の後方互換の要。`#[serde(default)]` の剥落をここで検知する)
    #[test]
    fn old_meta_without_layer_palettes_loads() {
        // 現行メタを JSON にしてから layer_palettes キーを取り除く=旧形式の再現
        let meta = WorkMeta {
            canvas_size: 512,
            layer_count: 0,
            params: SimParams::default(),
            palette: Palette::default_palette(),
            layers: Vec::new(),
            layer_palettes: Vec::new(),
        };
        let mut v: serde_json::Value = serde_json::to_value(&meta).unwrap();
        v.as_object_mut().unwrap().remove("layer_palettes");
        let old: WorkMeta = serde_json::from_value(v).unwrap();
        assert!(old.layer_palettes.is_empty());
        assert_eq!(old.palette, meta.palette);
    }

    /// 読込時正規化(M5h): 記録があればそのまま、旧ファイル(空 Vec)は latent 逆変換で
    /// 色を復元し、名前は自動命名・ρ/ω/γ は保存時パレット由来になること
    #[test]
    fn normalize_fills_missing_layer_palettes() {
        // 保存時パレットと「当時のパレット」を別の色にして、復元元の取り違えを検出する
        let mut recorded = Palette::default_palette();
        recorded.pigments[0].rgb = [200, 40, 10];
        let mut latents = vec![0.0f32; crate::gpu::LATENT_TOTAL * 4];
        let base = pigment::GLOBAL_LATENTS * 4; // slot 0 のパレット枠
        for (k, v4) in recorded.pigment_latents().iter().enumerate() {
            latents[base + k * 4..base + k * 4 + 4].copy_from_slice(v4);
        }
        let meta_palette = Palette::default_palette();
        let file = WorkFile {
            canvas_size: 512,
            params: SimParams::default(),
            palette: meta_palette.clone(),
            layers: vec![StoredLayer { slot: 0, visible: true }],
            layer_palettes: Vec::new(), // 旧ファイル相当
            textures: WorkTextures {
                wet: [Vec::new(), Vec::new(), Vec::new()],
                dried: vec![Vec::new()], // 層1枚(正規化は個数だけ見る)
                lines: [Vec::new(), Vec::new(), Vec::new()],
                latents,
            },
        };

        let normalized = normalized_layer_palettes(&file);
        assert_eq!(normalized.len(), 1);
        let p0 = &normalized[0].pigments[0];
        // 色は latent 逆変換(mixbox 往復で ±1 の量子化誤差を許容)
        for c in 0..3 {
            assert!(
                (p0.rgb[c] as i32 - recorded.pigments[0].rgb[c] as i32).abs() <= 1,
                "latent 逆変換の色がずれています: {:?}",
                p0.rgb
            );
        }
        assert_eq!(p0.name, fallback_pigment_name(0, 0));
        // ρ/ω/γ は保存時パレットの同番スロット値
        assert_eq!(p0.density, meta_palette.pigments[0].density);
        assert_eq!(p0.staining, meta_palette.pigments[0].staining);

        // 記録がある場合はそのまま返る(補完しない)
        let mut with_record = file;
        with_record.layer_palettes = vec![recorded.clone()];
        assert_eq!(normalized_layer_palettes(&with_record), vec![recorded]);
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
        })
        .unwrap();
        let mut bytes = Vec::new();
        bytes.extend_from_slice(MAGIC);
        bytes.extend_from_slice(&(meta.len() as u32).to_le_bytes());
        bytes.extend_from_slice(&meta);
        // WorkFile は巨大バッファ持ちで Debug 未実装のため unwrap_err でなく match で検査
        match decode(&bytes) {
            Err(e) => assert!(e.contains("キャンバスサイズ"), "サイズ検査で拒否されるはず: {e}"),
            Ok(_) => panic!("未対応サイズが受理されました"),
        }
    }
}
