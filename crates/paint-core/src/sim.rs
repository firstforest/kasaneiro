//! シミュレーション制御の型: SimParams(H2)と splat 入力。
//!
//! テクスチャ構成(いずれも rgba32float、ping-pong 2枚組):
//! - 水: r=水量 / g=速度x / b=速度y / a=濡れマスク(wet-area mask)
//! - 浮遊顔料(M1b): rgba の各チャンネル = 顔料1種(M1c から4顔料。pigment.rs の PIGMENTS と対応)
//! - 沈着顔料(M1b): 同上。紙に定着した分で、移流しない
//!
//! 濡れマスクは筆が通ったセルで 1。水と顔料が動くのはマスク内だけで、
//! 乾いた紙との境界は壁として扱う(にじみがストローク領域の外へ広がらない)。
//! これに加えて紙ハイトテクスチャ(M1d、r32float 1枚。ping-pong しない静的な紙の凹凸。
//! 生成は paper.rs)があり、速度勾配・にじみ拡張・吸着の3箇所を変調する。
//! 1 シミュレーションステップのパス順序は gpu/mod.rs の prepare() 参照:
//!   splat(水+初速+顔料の注入)→ 速度更新 → 発散緩和 × relax_iters
//!   → FlowOutward(エッジダークニング、edge_eta > 0 のとき)→ 移流(水+浮遊顔料)
//!   → 顔料拡散 × diffuse_iters → 吸着/脱着+蒸発(transfer)

use bytemuck::{Pod, Zeroable};
use serde::{Deserialize, Serialize};

/// 選択できるキャンバス(=シミュレーション)1辺(M8)。正方形のみ。
/// いずれも 64 の倍数(readback の行バイト数が 256B 整列になる = パディング計算が不要)かつ
/// タイルサイズ 16 の倍数(アクティブタイル M6 の格子に割り切れる)。
/// 実行時の値は GpuCanvas が持ち、ここは選択肢と既定値だけを定義する(R9 の値化)
pub const CANVAS_SIZES: [u32; 3] = [512, 1024, 2048];

/// 起動時のキャンバス1辺。試行錯誤は軽い 512² で回す(plan.md §3 M8 = 反復速度の原則)
pub const DEFAULT_CANVAS_SIZE: u32 = 512;

/// 1フレームで GPU に送る splat の上限(storage buffer の固定長)
pub const MAX_SPLATS: usize = 1024;

/// 全シミュレーションパラメータの唯一の置き場(H2)。
/// フィールドを足したら: ①ここに1行 ②app.rs のスライダー1行 ③common.wgsl の struct に1行。
/// メモリレイアウトは WGSL の uniform 規則(16 バイト整列)に合わせること。
/// 全体サイズは 16 の倍数を保つ(末尾の _pad を置き換えてからフィールドを増やす)。
/// serde(default): プリセット(H3)にないフィールドは Default 値で埋める
/// (パラメータを増やしても古い JSON が読めるように)。
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Pod, Zeroable, Serialize, Deserialize)]
#[serde(default)]
pub struct SimParams {
    /// ブラシ半径(テクセル単位)
    pub brush_radius: f32,
    /// 1 splat あたりに置く水量
    pub brush_water: f32,
    /// splat 時にストローク方向へ与える初速
    pub brush_velocity: f32,
    /// シミュレーションの時間刻み
    pub dt: f32,
    /// 移流強度: 水面勾配 → 加速度の係数
    pub accel: f32,
    /// 速度減衰(粘性の代用。1ステップあたりの減衰率)
    pub damping: f32,
    /// 発散緩和の係数 ξ(Curtis 1997 の既定は 0.1)
    pub xi: f32,
    /// 発散緩和の反復回数(1ステップあたり)
    pub relax_iters: u32,
    /// 速度上限(セル/ステップ)。CFL 的制約として 1.0 以下を推奨
    pub vel_max: f32,
    /// デバッグ表示モード(H4): 0=通常 / 1=水量ヒートマップ / 2=速度場 / 3=湿りオーバーレイ
    pub display_mode: u32,
    /// デバッグ表示の輝度スケール
    pub display_gain: f32,
    /// にじみ拡張率: 濡れた隣の水量に比例して乾いたセルのマスクが育つ速さ(0=固定マスク)
    pub wet_expand: f32,
    /// 1 splat あたりに置く顔料量(0 = 水だけのブラシ)
    pub brush_pigment: f32,
    /// 吸着率: 浮遊顔料が紙へ沈着する速さ(水が少ないほど強く効く)
    pub deposit_rate: f32,
    /// 脱着率: 沈着顔料が水に浮き上がる速さ(水が多いほど強く効く)
    pub lift_rate: f32,
    /// 蒸発率: 濡れ領域の水が 1 ステップに減る量
    pub evap_rate: f32,
    /// 顔料拡散率: 浮遊顔料が水の中をにじんで広がる速さ(1反復あたり。安定のため WGSL 側で 0.2 に制限)
    pub pigment_diffuse: f32,
    /// 顔料拡散の反復回数(1ステップあたり)。実効的なにじみ速度 = 拡散率 × 反復回数。
    /// 水筆で描いた水路に顔料溜まりを接続したとき、色が水路へ広がる速さはここで稼ぐ
    pub diffuse_iters: u32,
    /// ブラシで注入する顔料スロット(0..3 = pigment.rs の PIGMENTS と対応)
    pub brush_channel: u32,
    /// 発色の濃さ: 顔料濃度 → 被覆率(1-exp(-density·濃度))の係数(display.wgsl)
    pub pigment_density: f32,
    /// 紙ハイトの振幅: 水面勾配に足す紙の凹凸の強さ(水が紙の谷へ流れる → ストリーク)
    pub paper_amp: f32,
    /// 粒状化: 紙の凹部で吸着が強まる度合い(0=紙目の影響なし)
    pub paper_gran: f32,
    /// にじみ拡張の紙目変調: 濡れ前線が紙の谷を選んで進む度合い(0=一様に広がる)
    pub paper_wet: f32,
    /// FlowOutward の η: 濡れ領域の縁ほど水を除去する強さ(0=エッジダークニングなし)。
    /// 既定 0 で先送り(status.md 参照): 今の弱い定式化では顔料が縁でなく中心へ寄るため、
    /// ちゃんとした乾燥が入る M2 で再検討する。パス・スライダーは残してある(オフならゼロコスト)
    pub edge_eta: f32,
    /// FlowOutward の濡れマスクぼかし半径(テクセル。縁と判定する帯の幅。WGSL 側で 1..8 に制限)
    pub edge_radius: u32,
    /// 筆圧→半径の効き(M1.5)。0=筆圧無効 / 1=筆圧に完全比例。
    /// 実効値 = 基準値 × mix(1, 筆圧^γ, 効き)。マウス(筆圧 1.0)では常に基準値のまま
    pub pressure_radius: f32,
    /// 筆圧→水量の効き(同上)
    pub pressure_water: f32,
    /// 筆圧→顔料量の効き(同上)
    pub pressure_pigment: f32,
    /// 筆圧の応答カーブ γ(筆圧^γ)。1=線形 / >1 で軽いタッチがより細く弱くなる
    pub pressure_gamma: f32,
    /// 乾燥シフト(M2): 焼き込み時に顔料濃度へ掛ける係数。デジタルでは乾いても濃度を保つ(=1.0)。
    /// アナログ調に乾くと薄くする場合は <1 にする
    pub dry_shift: f32,
    /// 焼き込み時の粒状感ゲート(M2): 紙の凹部で濃く/凸部で薄く定着する度合い(0=無効)。
    /// transfer.wgsl の paper_gran(描画中の吸着変調)とは独立に、乾く瞬間の紙目を強調する
    pub dry_gran: f32,
    /// 焼き込み時のエッジダークニング(M2): 顔料の縁バンドで濃度を増す強さ(0=無効)。
    /// Curtis のエッジダークニングは乾燥時に起こる現象なので定着パスで掛ける。
    /// 縁バンドは顔料被覆率のぼかし残差(濡れマスク基準だと縁の濃度がゼロで効かない)
    /// (M1d の FlowOutward = シミュレーション中の縁への移流とは別方式。縁バンド幅は edge_radius を共用)
    pub dry_edge: f32,
    /// Wet the Layer(M2): 再湿潤で全面に足す水量
    pub rewet_water: f32,
    /// ブラシツール(M3/M4): 0=描画 / 1=リフト(削り) / 2=消去 / 3=水筆 / 4=ならし。splat.wgsl が分岐する。
    /// リフト = 沈着顔料を浮遊層へ戻して縁へ流す(ステイニング顔料は ω で残る)。
    /// 消去 = 湿レイヤーの水・顔料・濡れマスクを機械的にゼロへ(紙の白まで戻す完全消去)。
    /// 水筆 = 水を置きつつブラシ下の顔料を近傍平均へ均す(質量保存の箱ぼかし。半径2固定の局所ならし。water_lift)。
    /// ならし = 水筆と同じ均しだがブラシスケールの広い近傍で行う(濃い山に置くと周囲へ均一に伸びる。smear_rate)
    pub tool: u32,
    /// リフトの強さ(M3): 1 ストロークで沈着顔料を浮遊層へ戻す割合の基準値。
    /// 実効値は顔料の ω(剥がれにくさ)と紙ハイト(凸部ほど剥がれる)で変調される
    pub lift_strength: f32,
    /// レイヤー合成方式(M3): 0=multiply(M2 の乗算)/ 1=KM(Kubelka-Munk の R/T 合成)。
    /// KM は各乾燥/湿レイヤーを白地・黒地に置いた発色から R,T を導き下から光学合成する
    /// (km.rs / display.wgsl 参照)。H5 のストローク再生で multiply と A/B 比較できる。
    pub compose_mode: u32,
    /// 水筆(M4)の均し強度: tool=3 のとき、ブラシ下の顔料(浮遊+沈着)を近傍平均へ寄せる
    /// 1フレームあたりの緩和率。大きいほど速く均一になる。質量保存の箱ぼかしなので濃くはならない
    pub water_lift: f32,
    /// ならし(M4)の均し強度: tool=4 のとき、総顔料(浮遊+沈着)をブラシスケールの近傍平均へ
    /// 寄せる1フレームあたりの緩和率。濃いところに置くとその山が周囲へ拡散して均一に伸びる。
    /// 水筆(water_lift、半径2固定の局所ならし)と違い、近傍半径をブラシ半径に応じて広げる
    pub smear_rate: f32,
    /// ラスタ線画(M4.5a)の種別: 0=鉛筆(グレー粒状線・紙ハイトで濃度変調・筆圧→濃さ)/
    /// 1=ペン(濃色スムーズ線・筆圧→太さ)/ 2=ハイライト(M4.5c 予約)。linesplat.wgsl の視覚分岐。
    /// 対象テクスチャ自体は bind group で選ぶ(流体は通らない)
    pub line_mode: u32,
    /// ラスタ消しゴム(M4.5a): 0=描画(蓄積)/ 1=減算。linesplat.wgsl が分岐する
    pub line_eraser: u32,
    /// 鉛筆の半径(M4.5a、テクセル)。水ブラシ(brush_radius)とは独立。筆圧で締められる
    pub pencil_radius: f32,
    /// 鉛筆の線の濃さ(M4.5a): 1パスで置くインク濃度の基準(筆圧で変調)
    pub pencil_strength: f32,
    /// 鉛筆の粒状感(M4.5a): 紙ハイトでインク濃度を変調する度合い(0=一様 / 1=山に強く乗る)
    pub pencil_gran: f32,
    /// ペンの半径(M4.5a、テクセル)。水ブラシ(brush_radius)とは独立。筆圧→太さ
    pub pen_radius: f32,
    /// ペンの線の濃さ(M4.5a): 1パスで置くインク濃度の基準(満量で掛かる)
    pub pen_strength: f32,
    /// 下書き(鉛筆)レイヤーの表示(M4.5a): 0=非表示 / 1=表示。display.wgsl が合成時に参照
    pub show_pencil: u32,
    /// 清書(ペン)レイヤーの表示(M4.5a): 0=非表示 / 1=表示
    pub show_pen: u32,
    /// 清書ペン線の透水率(M4.5b): 水の境界としての効きの強さ。
    /// 透水率 perm = 1 − line_block×ペン濃度 を ①拡散(diffuse)の隣接流束 ②にじみ拡張
    /// (velocity の wet_expand 蓄積)③速度場 に掛ける。ブラシの直接スプラットには掛けない
    /// (線を跨ぐ筆使いなら明示的に越えられる)。0 で従来どおり(境界なし)
    pub line_block: f32,
    /// ハイライトの半径(M4.5c、テクセル)。不透明白ブラシ。筆圧で締められる
    pub highlight_radius: f32,
    /// ハイライトの不透明度基準(M4.5c): 1パスで置く白の不透明度(筆圧で変調)
    pub highlight_strength: f32,
    /// ハイライトレイヤーの表示(M4.5c): 0=非表示 / 1=表示。合成の最後に白を重ねる
    pub show_highlight: u32,
    /// アクティブタイル最適化(M6): 0=無効(全面計算)/ 1=有効(濡れ+ブラシ近傍のタイルだけ計算)。
    /// tilescan/tiledilate が濡れ面積+ブラシからタイル有効フラグを作り、各シミュパスは非アクティブな
    /// タイルを素通しして計算を省く。0 で従来どおり全面計算に戻せる(A/B・不具合時の退避)。
    /// SimParams 末尾で 16B 境界を担う(52フィールド=208B)
    pub active_tiles: u32,
}

impl SimParams {
    /// 任意の基準半径に筆圧を反映した実効半径(splat.wgsl / linesplat.wgsl と同じ式)。
    /// 水ブラシは brush_radius、ラスタ線画は pencil_radius / pen_radius を base に渡す
    pub fn radius_at_base(&self, base: f32, pressure: f32) -> f32 {
        let p = pressure
            .clamp(0.0, 1.0)
            .powf(self.pressure_gamma.max(0.01));
        let factor = 1.0 + (p - 1.0) * self.pressure_radius.clamp(0.0, 1.0);
        (base * factor).max(0.5)
    }

    /// 水ブラシの実効半径。CPU 側ではストローク補間のサンプル間隔の算出に使う(brush.rs / replay.rs)
    pub fn radius_at(&self, pressure: f32) -> f32 {
        self.radius_at_base(self.brush_radius, pressure)
    }
}

impl Default for SimParams {
    fn default() -> Self {
        Self {
            brush_radius: 16.0,
            brush_water: 0.5,
            brush_velocity: 0.5,
            dt: 0.5,
            accel: 1.0,
            damping: 0.05,
            xi: 0.1,
            relax_iters: 16,
            vel_max: 1.0,
            display_mode: 0,
            display_gain: 1.0,
            wet_expand: 0.0,
            brush_pigment: 0.11,
            deposit_rate: 0.05,
            lift_rate: 0.02,
            evap_rate: 0.005,
            pigment_diffuse: 0.15,
            diffuse_iters: 4,
            brush_channel: 0,
            pigment_density: 1.0,
            paper_amp: 0.3,
            paper_gran: 0.4,
            paper_wet: 0.5,
            edge_eta: 0.0,
            edge_radius: 4,
            pressure_radius: 0.6,
            pressure_water: 0.3,
            pressure_pigment: 0.7,
            pressure_gamma: 1.0,
            dry_shift: 1.0,
            dry_gran: 0.0,
            dry_edge: 0.4,
            rewet_water: 0.5,
            tool: 0,
            lift_strength: 0.3,
            compose_mode: 1, // 既定は KM(M3 の完成形)。0 で M2 の multiply に戻せる
            water_lift: 0.4, // 水筆の均し(近傍平均への緩和率)。なでるほど均一へ収束する
            smear_rate: 0.35, // ならし: 総顔料をブラシスケールで均す緩和率。濃い山を周囲へ伸ばす
            line_mode: 0,     // 既定は鉛筆
            line_eraser: 0,
            pencil_radius: 8.0,   // 鉛筆は柔らかめの中細
            pencil_strength: 0.7,
            pencil_gran: 0.5,
            pen_radius: 4.0,      // ペンは細く硬い線
            pen_strength: 0.9,    // ペンは濃い
            show_pencil: 1,
            show_pen: 1,
            line_block: 0.0,          // M4.5b: 既定は境界なし(明示的に上げると水が越えにくくなる)
            highlight_radius: 6.0,    // M4.5c: ハイライトは中太
            highlight_strength: 0.85, // M4.5c: 白の不透明度
            show_highlight: 1,
            active_tiles: 1, // M6: 既定で有効(濡れ面積に比例。0 で全面計算へ戻せる)
        }
    }
}

/// ストローク上の1点。WGSL 側の Splat と同レイアウト(32 バイト)。
#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub struct Splat {
    /// テクセル座標
    pub pos: [f32; 2],
    /// ストローク方向の単位ベクトル(初速の向き。ストローク始点は 0)
    pub vel: [f32; 2],
    /// 筆圧(マウスは 1.0。M1.5 で octotablet の値が入る)
    pub pressure: f32,
    pub _pad: [f32; 3],
}

impl Splat {
    pub fn new(pos: [f32; 2], vel: [f32; 2], pressure: f32) -> Self {
        Self {
            pos,
            vel,
            pressure,
            _pad: [0.0; 3],
        }
    }
}

/// splat storage buffer の先頭 16 バイト
#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub struct SplatHeader {
    pub count: u32,
    pub _pad: [u32; 3],
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 筆圧→半径マッピング(M1.5): 筆圧 1.0(マウス)は常に基準半径、
    /// 効き 1.0 + γ=1 で完全比例、効き 0 で筆圧を無視
    #[test]
    fn radius_at_pressure() {
        let mut params = SimParams {
            brush_radius: 20.0,
            pressure_radius: 1.0,
            pressure_gamma: 1.0,
            ..SimParams::default()
        };
        assert_eq!(params.radius_at(1.0), 20.0);
        assert_eq!(params.radius_at(0.5), 10.0);
        params.pressure_radius = 0.0;
        assert_eq!(params.radius_at(0.0), 20.0);
    }

    /// SimParams は WGSL uniform 規則(16 バイト整列)に合わせてサイズが 16 の倍数
    #[test]
    fn params_size_aligned() {
        assert_eq!(std::mem::size_of::<SimParams>() % 16, 0);
    }
}
