//! I/O トラップ・ボード検出応答・DOS スタブ・パイプ送信(board-paced 検証版)。
//! PMD のタイマ割り込み(Timer A=SSGドラム / Timer B=音楽)を 1 tick=1 バッチで、対応する
//! ボードのバッファ(A/B)へ IntEnd 付きで積むだけ。**ボードの実 YMF288 タイマ /IRQ が drain**
//! することでテンポを律速する(ホストは毎イベント往復しない=処理落ち対策)。
//! ホストは driver.exe のキュー長(SizeRequest)で背圧をかけ、先回りしすぎないようにする。
//! 順序は opna.rs の仮想モデル(同時は B 優先)。検証元 tools/pmdrun.c。

use std::ffi::c_void;

use crate::emu::EmuRegs;
use crate::opna::{Opna, Timer};
use crate::packet::PacketSend;
use crate::pipe::Pipe;

const LOADSEG: u16 = 0x1000;

pub struct Host {
    pipe: Pipe,
    latch0: u8,
    latch1: u8,
    ssg: [u8; 16],
    opna: Opna,
    cur_intsel: u8,    // 現イベントの IntSelect(0=バッファB / 0x40=バッファA)
    timer_armed: bool, // true: 0x27 をそのまま転送(ボードのタイマを動かす)
    timer_vec: u8,
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
            opna: Opna::new(),
            cur_intsel: 0,
            timer_armed: false,
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
                    self.ssg[r as usize] = val;
                }
                self.opna.write_reg(r, val);
                if r == 0x27 {
                    // armed 後はボードのタイマを動かすため PMD の値をそのまま転送。
                    // armed 前(install/MUSIC_START)は全タイマ無効で転送(早期始動の抑止)。
                    let v = if self.timer_armed { val } else { val & !0x0F };
                    self.emit(0, 0x27, v);
                } else {
                    self.emit(0, r, val);
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
        let bank_bit = if bank != 0 { PacketSend::BANK_SELECT } else { 0 };
        let ty = bank_bit | self.cur_intsel; // 現イベントのバッファ(A/B)へ
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
                    0x01
                } else if self.latch0 == 0x0E || self.latch0 == 0x0F {
                    0xFF
                } else if self.latch0 < 0x10 {
                    self.ssg[self.latch0 as usize]
                } else {
                    0x00
                }
            }
            0x18C | 0x18E => 0x00,
            0x188 => self.opna.read_status(),
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

    pub fn set_timer_vec(&mut self, vec: u8) {
        self.timer_vec = vec;
    }

    pub fn arm_board(&mut self) {
        self.timer_armed = true;
    }

    /// ドライランのバッチサイズ統計を表示。
    pub fn print_stats(&self) {
        self.pipe.print_stats();
    }

    /// 次のイベント(A/B)。opna のフラグを立て、書き込み先バッファ(IntSelect)も設定する。
    pub fn next_event(&mut self) -> Option<Timer> {
        let t = self.opna.next_event()?;
        self.cur_intsel = match t {
            Timer::A => PacketSend::INT_SELECT_A, // SSGドラム/効果音 → バッファ A
            Timer::B => 0,                        // 音楽 → バッファ B
        };
        Some(t)
    }

    /// 再生開始前のリセット(FLUSH)。driver の送信キューを破棄し、ボードへ転送 →
    /// ファームのバッファ/orderキュー/pendingTimers をクリアする。driver 再起動なしの連続再生で、
    /// 前回再生の残留(driverキュー先読み分 + ボードの中途半端なバッファ)が次回に desync して
    /// 途中フリーズするのを防ぐ。
    pub fn flush_reset(&mut self) -> std::io::Result<()> {
        self.pipe.send(&PacketSend::new(PacketSend::FLUSH, 0, 0))?;
        self.take_error()
    }

    /// バッチ終端 + 強制 drain(install / MUSIC_START / 最初の tick = ボードのタイマ始動)。
    pub fn flush_drain(&mut self) -> std::io::Result<()> {
        self.pipe.send(&PacketSend::new(PacketSend::INT_END | self.cur_intsel, 0, 0))?;
        self.pipe.send(&PacketSend::new(PacketSend::FORCE_TIMEOUT, 0, 0))?;
        self.take_error()
    }

    /// バッチ終端のみ(現バッファへ)。以降の tick はボードの /IRQ が drain。
    pub fn end_batch(&mut self) -> std::io::Result<()> {
        self.pipe.send(&PacketSend::new(PacketSend::INT_END | self.cur_intsel, 0, 0))?;
        self.take_error()
    }

    /// driver.exe の送信キュー長(背圧用。先回りしすぎを防ぐ)。
    pub fn queue_size(&mut self) -> std::io::Result<u32> {
        self.pipe.query_size()
    }

    fn take_error(&mut self) -> std::io::Result<()> {
        if let Some(e) = self.error.take() {
            return Err(e);
        }
        Ok(())
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
