//! OPNA / YMF288 のタイマ + ステータスレジスタの最小エミュレーション。
//! データシート(YMF288 / YM2608 Application Manual)準拠。
//!
//! 役割: ホスト側で「次にどちらのタイマ割り込みが来るか」を仮想時間で決め、
//! PMD の ISR に渡すステータスを生成する。実時間のペースはボードの水晶が律速するので、
//! ここでは割り込みの**順序と比率**だけを忠実に再現する(周期係数の絶対値は重要でない)。

// 0x27 制御ビット
const LOAD_A: u8 = 0x01;
const LOAD_B: u8 = 0x02;
const ENABLE_A: u8 = 0x04;
const ENABLE_B: u8 = 0x08;
const RESET_A: u8 = 0x10;
const RESET_B: u8 = 0x20;

// ステータスビット
const ST_TIMER_A: u8 = 0x01; // TIA
const ST_TIMER_B: u8 = 0x02; // TIB
#[allow(dead_code)]
const ST_BUSY: u8 = 0x80; // エミュでは常に 0(即完了扱い)

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Timer {
    A,
    B,
}

pub struct Opna {
    na: u16, // Timer A 設定値(10bit) reg 0x24(上位8) / 0x25(下位2)
    nb: u8,  // Timer B 設定値(8bit) reg 0x26
    ctrl: u8, // 0x27
    flag_a: bool,
    flag_b: bool,
    // 仮想時間(fM サイクル相当)。順序判定のみに使う。
    now: u64,
    next_a: u64, // 次に Timer A が溢れる仮想時刻
    next_b: u64,
}

impl Opna {
    pub fn new() -> Self {
        Self {
            na: 0,
            nb: 0,
            ctrl: 0,
            flag_a: false,
            flag_b: false,
            now: 0,
            next_a: 0,
            next_b: 0,
        }
    }

    /// 0x24-0x27 への書き込みを反映する。それ以外のアドレスは無視。
    pub fn write_reg(&mut self, addr: u8, data: u8) {
        match addr {
            0x24 => self.na = (self.na & 0x0003) | ((data as u16) << 2),
            0x25 => self.na = (self.na & 0x03FC) | (data as u16 & 0x0003),
            0x26 => self.nb = data,
            0x27 => self.write_ctrl(data),
            _ => {}
        }
    }

    fn write_ctrl(&mut self, data: u8) {
        // Reset ビットでフラグをクリア(立てっぱなしの解除。データシートの Reset A/B)
        if data & RESET_A != 0 {
            self.flag_a = false;
        }
        if data & RESET_B != 0 {
            self.flag_b = false;
        }
        // Load の立ち上がりでタイマ再ロード(カウント開始)
        let prev = self.ctrl;
        self.ctrl = data;
        if data & LOAD_A != 0 && prev & LOAD_A == 0 {
            self.next_a = self.now + self.period_a();
        }
        if data & LOAD_B != 0 && prev & LOAD_B == 0 {
            self.next_b = self.now + self.period_b();
        }
    }

    // 周期(fM サイクル)。T_A = 72*(1024-NA)/fM, T_B = 1152*(256-NB)/fM(比 16:1)。
    fn period_a(&self) -> u64 {
        72 * (1024 - self.na as u64)
    }
    fn period_b(&self) -> u64 {
        1152 * (256 - self.nb as u64)
    }

    /// 現在の仮想時刻(fM サイクル)。イベントの実時間換算に使う。
    pub fn now(&self) -> u64 {
        self.now
    }

    /// ステータスレジスタ(A0=0 のリード)。bit0=TIA, bit1=TIB, bit7=Busy(常に0)。
    pub fn read_status(&self) -> u8 {
        let mut s = 0;
        if self.flag_a {
            s |= ST_TIMER_A;
        }
        if self.flag_b {
            s |= ST_TIMER_B;
        }
        s
    }

    /// 次のタイマ割り込みを 1 つ進め、対応するフラグを立てて返す。
    /// 1 イベント = 1 タイマ(各イベントがボードの 1 バッファに対応する)。
    /// 同時刻なら B を先に発火する(PMD `FM_Timer_main` の「両方時は B→A」順序に一致)。
    /// 起動している(Load かつ Enable)タイマが無ければ None。
    #[allow(dead_code)] // tick ループ(M3b)で使用する
    pub fn next_event(&mut self) -> Option<Timer> {
        let a_on = self.ctrl & LOAD_A != 0 && self.ctrl & ENABLE_A != 0;
        let b_on = self.ctrl & LOAD_B != 0 && self.ctrl & ENABLE_B != 0;
        match (a_on, b_on) {
            (false, false) => None,
            (true, false) => Some(self.fire_a()),
            (false, true) => Some(self.fire_b()),
            // 両方起動中: 早い方。同時刻は B を先(PMD 順序)
            (true, true) => {
                if self.next_b <= self.next_a {
                    Some(self.fire_b())
                } else {
                    Some(self.fire_a())
                }
            }
        }
    }

    fn fire_a(&mut self) -> Timer {
        self.now = self.next_a;
        self.flag_a = true;
        self.next_a += self.period_a();
        Timer::A
    }

    fn fire_b(&mut self) -> Timer {
        self.now = self.next_b;
        self.flag_b = true;
        self.next_b += self.period_b();
        Timer::B
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Load/Enable 両方をセットする 0x27 値
    const RUN_A: u8 = LOAD_A | ENABLE_A;
    const RUN_B: u8 = LOAD_B | ENABLE_B;

    #[test]
    fn na_compose_from_two_regs() {
        let mut o = Opna::new();
        o.write_reg(0x24, 0xAB); // 上位8bit
        o.write_reg(0x25, 0x02); // 下位2bit
        assert_eq!(o.na, (0xAB << 2) | 0x02);
    }

    #[test]
    fn tie_fires_b_before_a() {
        let mut o = Opna::new();
        // 同じ周期になるよう NA/NB を選ぶ: period_a=72*(1024-NA), period_b=1152*(256-NB)
        // NA=0 -> 72*1024=73728, NB=192 -> 1152*64=73728 で同時刻
        o.write_reg(0x24, 0);
        o.write_reg(0x25, 0);
        o.write_reg(0x26, 192);
        o.write_reg(0x27, RUN_A | RUN_B);
        // 同時刻なので B が先
        assert_eq!(o.next_event(), Some(Timer::B));
        // 次は A(B はクリアされていないが時刻が進む)
        assert_eq!(o.next_event(), Some(Timer::A));
    }

    #[test]
    fn faster_timer_fires_more_often() {
        let mut o = Opna::new();
        // Timer A を Timer B より十分速く(NA小, NB小)
        o.write_reg(0x24, 0); // NA=0 -> period 73728
        o.write_reg(0x25, 0);
        o.write_reg(0x26, 0); // NB=0 -> period 1152*256=294912 (約4倍遅い)
        o.write_reg(0x27, RUN_A | RUN_B);
        let mut a = 0;
        let mut b = 0;
        for _ in 0..20 {
            match o.next_event() {
                Some(Timer::A) => a += 1,
                Some(Timer::B) => b += 1,
                None => break,
            }
        }
        assert!(a > b, "速い Timer A の方が多く発火するはず (a={a}, b={b})");
    }

    #[test]
    fn reset_bit_clears_flag() {
        let mut o = Opna::new();
        o.write_reg(0x26, 0);
        o.write_reg(0x27, RUN_B);
        o.next_event();
        assert_eq!(o.read_status() & ST_TIMER_B, ST_TIMER_B);
        o.write_reg(0x27, RUN_B | RESET_B); // Reset B
        assert_eq!(o.read_status() & ST_TIMER_B, 0);
    }

    #[test]
    fn no_event_when_disabled() {
        let mut o = Opna::new();
        o.write_reg(0x26, 0);
        // Load のみ(Enable 無し)では割り込み無し
        o.write_reg(0x27, LOAD_B);
        assert_eq!(o.next_event(), None);
    }
}
