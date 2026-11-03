//! 混色皿の中身の保存。皿は作品とは別にアプリ側に1枚だけあり、終了時(と eframe の自動保存)に書いて
//! 次の起動で戻す。濡れている所も濡れたまま戻すので、湿レイヤーの [水, 浮遊, 沈着] をそのまま持つ。
//! 顔料はスロットごとの濃度なので、パレットは保存しない(戻した皿は今のパレットで発色する)。
//!
//! 置き場所は eframe の保存先(Windows では `%APPDATA%\kasaneiro\data\mixpad.bin`)。作品
//! (`works/`)と違って名前を付けて管理するものではなく、パレット・筆の中身と同じ「前回の続き」の状態なので。
//!
//! 形式: `MAGIC "KSM1"` + 1辺(u32 LE)+ f32 LE の [水, 浮遊, 沈着](各 1辺²×4)。

use std::path::PathBuf;

const MAGIC: &[u8; 4] = b"KSM1";
const FILE_NAME: &str = "mixpad.bin";

/// 皿の湿レイヤー(GpuCanvas の [水, 浮遊, 沈着]。各 size²·4 f32、行連続)
#[derive(Debug, PartialEq)]
pub struct MixpadState {
    pub size: u32,
    pub wet: [Vec<f32>; 3],
}

fn path() -> Option<PathBuf> {
    eframe::storage_dir(crate::APP_ID).map(|dir| dir.join(FILE_NAME))
}

pub fn save(state: &MixpadState) -> Result<PathBuf, String> {
    let path = path().ok_or("保存先のフォルダが分かりません")?;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("{} を作れません: {e}", dir.display()))?;
    }
    crate::assets::write_atomic(&path, &encode(state))?;
    Ok(path)
}

/// 保存した皿を読む。まだ保存していなければ None
pub fn load() -> Result<Option<MixpadState>, String> {
    let Some(path) = path() else {
        return Ok(None);
    };
    let bytes = match std::fs::read(&path) {
        Ok(b) => b,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(format!("{} を読めません: {e}", path.display())),
    };
    decode(&bytes)
        .map(Some)
        .map_err(|e| format!("{} の形式が不正です: {e}", path.display()))
}

fn encode(state: &MixpadState) -> Vec<u8> {
    let mut out = Vec::with_capacity(8 + state.wet.iter().map(|v| v.len() * 4).sum::<usize>());
    out.extend_from_slice(MAGIC);
    out.extend_from_slice(&state.size.to_le_bytes());
    for v in &state.wet {
        for f in v {
            out.extend_from_slice(&f.to_le_bytes());
        }
    }
    out
}

fn decode(bytes: &[u8]) -> Result<MixpadState, String> {
    if bytes.len() < 8 || &bytes[..4] != MAGIC {
        return Err("混色皿のファイルではありません".to_owned());
    }
    let size = u32::from_le_bytes(bytes[4..8].try_into().expect("4 バイト"));
    let per = (size as usize)
        .checked_mul(size as usize)
        .and_then(|n| n.checked_mul(4))
        .ok_or("大きさが不正です")?;
    let body = &bytes[8..];
    if body.len() != per * 3 * 4 {
        return Err(format!(
            "大きさ {size} に対してデータの長さが合いません({} バイト)",
            body.len()
        ));
    }
    let floats: Vec<f32> = body
        .chunks_exact(4)
        .map(|c| f32::from_le_bytes(c.try_into().expect("4 バイト")))
        .collect();
    let wet = [0, 1, 2].map(|k| floats[k * per..(k + 1) * per].to_vec());
    Ok(MixpadState { size, wet })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> MixpadState {
        let n = 2 * 2 * 4;
        MixpadState {
            size: 2,
            wet: [0, 1, 2].map(|k| (0..n).map(|i| (k * 100 + i) as f32 * 0.5).collect()),
        }
    }

    #[test]
    fn roundtrip() {
        let state = sample();
        assert_eq!(decode(&encode(&state)).unwrap(), state);
    }

    #[test]
    fn rejects_wrong_length() {
        let mut bytes = encode(&sample());
        bytes.push(0);
        assert!(decode(&bytes).is_err());
        bytes.truncate(bytes.len() - 5);
        assert!(decode(&bytes).is_err());
    }

    #[test]
    fn rejects_garbage() {
        assert!(decode(b"KSN1\0\0\0\0").is_err());
        assert!(decode(b"KS").is_err());
    }
}
