//! I/O トラップ・ボード検出応答・DOS スタブ・パイプ送信(host-paced + 正しいテンポ)。
//! PMD のタイマ ISR(opnint→FM_Timer_main)を 1 tick ずつ駆動し、各 tick のレジスタ書き込みを
//! ボードへ ForceTimeout で即適用(M1 で実証済みの経路)。テンポは PMD が書く Timer B 値(0x26)
//! から算出して刻む(T_B = 144*(256-TB) us)。テンポ変更にも自動追従。
//! ボードの実タイマ /IRQ を使う board-paced は別途(ファームの /IRQ 経路検証 + スループット改善が必要)。
//! 検証元: tools/pmdrun.c。

use std::ffi::c_void;

use crate::emu::EmuRegs;
use crate::packet::PacketSend;
use crate::pipe::Pipe;

const LOADSEG: u16 = 0x1000;

pub struct Host {
    pipe: Pipe,
    latch0: u8,
    latch1: u8,
    ssg: [u8; 16], // SSG レジスタ(0x00-0x0F)のシャドウ。read-modify-write(reg7 ミキサ等)の読み戻し用。
    status: u8,    // OPNA ステータス(0x188 リードで返す)。tick 前に TimerB フラグを立てる。
    tb: u8,        // PMD が書いた Timer B 値(0x26)。テンポ算出に使う。
    timer_vec: u8, // OPNA タイマ ISR のベクタ(install 後に設定)
    pub installed: bool,
    pub exited: bool,
    error: Option<std::io::Error>,
}

impl Host {
    pub fn new(pipe: Pipe) -> Self {
        Self {
            pipe,
            latch0: 0,
            latch1: 0,
            ssg: [0; 16],
            status: 0,
            tb: 0xC8, // フォールバック(~124Hz)
            timer_vec: 0xFF,
            installed: false,
            exited: false,
            error: None,
        }
    }

    fn out(&mut self, port: u32, val: u8) {
        match port {
            0x188 => self.latch0 = val,
            0x18A => {
                let r = self.latch0;
                if r < 0x10 {
                    self.ssg[r as usize] = val; // SSG レジスタをシャドウ(読み戻し用)
                }
                match r {
                    0x26 => {
                        self.tb = val; // Timer B 値を捕獲(テンポ)
                        self.emit(0, r, val);
                    }
                    0x27 => {
                        self.status = 0; // Timer Reset 相当
                        // host-paced ではボードのタイマは使わない → 全タイマ無効で転送(reset/ch3 は保持)。
                        self.emit(0, 0x27, val & !0x0F);
                    }
                    _ => self.emit(0, r, val),
                }
            }
            0x18C => self.latch1 = val,
            0x18E => {
                let r = self.latch1;
                self.emit(1, r, val);
            }
            _ => {}
        }
    }

    fn emit(&mut self, bank: u8, reg: u8, data: u8) {
        let ty = if bank != 0 { PacketSend::BANK_SELECT } else { 0 };
        if let Err(e) = self.pipe.send(&PacketSend::new(ty, reg, data)) {
            if self.error.is_none() {
                self.error = Some(e);
            }
        }
    }

    fn in_port(&self, port: u32) -> u8 {
        match port {
            0x18A => {
                if self.latch0 == 0xFF {
                    0x01 // ボード検出 ID
                } else if self.latch0 == 0x0E || self.latch0 == 0x0F {
                    0xFF // ジョイスティック等の入力ポート: 何も押されていない
                } else if self.latch0 < 0x10 {
                    self.ssg[self.latch0 as usize] // SSG レジスタ読み戻し(reg7 ミキサ RMW 等)
                } else {
                    0x00
                }
            }
            0x18C | 0x18E => 0x00,
            0x188 => self.status,
            0x08A => 0xFF,
            0xA460 => 0xFF,
            _ => 0xFF,
        }
    }

    fn dos(&mut self, r: &mut EmuRegs) -> i32 {
        match r.ah() {
            0x30 => r.set_ax(0x0005),
            0x52 => { r.es = 0x0050; r.bx = 0; }
            0x51 | 0x62 => r.bx = LOADSEG,
            0x25 => {}
            0x35 => { r.bx = 0; r.es = 0; }
            0x49 => {}
            0x48 => r.set_ax(0x9000),
            0x4A => {}
            0x09 | 0x02 => {}
            0x31 => { self.installed = true; return 2; }
            0x4C => { self.exited = true; return 2; }
            _ => { r.flags |= 1; return 1; }
        }
        r.flags &= !1;
        1
    }

    // --- 駆動ヘルパ -----------------------------------------------------------

    /// tick 前: TimerB が来たことにする。
    pub fn arm_timer_b(&mut self) {
        self.status = 0x02;
    }

    /// OPNA タイマ ISR のベクタを登録。
    pub fn set_timer_vec(&mut self, vec: u8) {
        self.timer_vec = vec;
    }

    /// バッチ終端 + 強制 drain(各 tick をボードへ即適用)。
    pub fn flush_drain(&mut self) -> std::io::Result<()> {
        self.pipe.send(&PacketSend::new(PacketSend::INT_END, 0, 0))?;
        self.pipe.send(&PacketSend::new(PacketSend::FORCE_TIMEOUT, 0, 0))?;
        if let Some(e) = self.error.take() {
            return Err(e);
        }
        Ok(())
    }

    /// 現在のテンポでの 1 tick 間隔(マイクロ秒、基準値)。
    /// YM2608 の Timer B は T_B = 1152*(256-TB)/fM(fM=8MHz=8cyc/us → 144*(256-TB) us)だが、
    /// YMF288 は実測でその ~1/2 レート(2 倍速くなる)だったため係数を 288 にしている。
    /// 最終間隔は main 側で PMDHOST_TEMPO 倍率を掛けて微調整可能。
    pub fn tick_base_micros(&self) -> u64 {
        (288u64 * (256 - self.tb as u64)).max(1)
    }
}

// ---- shim.c から呼ばれるコールバック ----------------------------------------

#[unsafe(no_mangle)]
pub extern "C" fn rust_io_out(user: *mut c_void, port: u32, val: u32, _size: u32) {
    if user.is_null() {
        return;
    }
    let host = unsafe { &mut *(user as *mut Host) };
    host.out(port, val as u8);
}

#[unsafe(no_mangle)]
pub extern "C" fn rust_io_in(user: *mut c_void, port: u32, _size: u32) -> u32 {
    if user.is_null() {
        return 0xFF;
    }
    let host = unsafe { &*(user as *const Host) };
    host.in_port(port) as u32
}

#[unsafe(no_mangle)]
pub extern "C" fn rust_intr(user: *mut c_void, num: u8, regs: *mut EmuRegs) -> i32 {
    if user.is_null() {
        return 1;
    }
    let host = unsafe { &mut *(user as *mut Host) };
    match num {
        0x21 => {
            let r = unsafe { &mut *regs };
            host.dos(r)
        }
        0x60 => 0,
        n if n == host.timer_vec => 0,
        _ => 1,
    }
}
