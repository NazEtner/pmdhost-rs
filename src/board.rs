//! I/O トラップの状態機械 + パイプ送信。
//! エミュの out 命令(188h/18Ah/18Ch/18Eh)を OPNA のアドレス/データ・ラッチとして
//! ペアリングし、1 レジスタ書き込みごとに PacketSend を 1 個吐く。

use std::ffi::c_void;

use crate::opna::Opna;
use crate::packet::PacketSend;
use crate::pipe::Pipe;

pub struct Host {
    pipe: Pipe,
    latch0: u8, // 表(bank0)アドレスラッチ
    latch1: u8, // 裏(bank1)アドレスラッチ
    opna: Opna, // ホスト側のタイマ/ステータスモデル(割り込み順序の決定用)
    error: Option<std::io::Error>,
}

impl Host {
    pub fn new(pipe: Pipe) -> Self {
        Self { pipe, latch0: 0, latch1: 0, opna: Opna::new(), error: None }
    }

    fn out(&mut self, port: u32, val: u8) {
        match port {
            0x188 => self.latch0 = val,
            0x18A => {
                let r = self.latch0;
                self.opna.write_reg(r, val); // 0x24-0x27 はタイマモデルへ反映(他は無視)
                self.emit(0, r, val);
            }
            0x18C => self.latch1 = val,
            0x18E => { let r = self.latch1; self.emit(1, r, val); }
            _ => {}
        }
    }

    // I/O リード。0x188(A0=0)のリードは OPNA ステータス。それ以外は 0xFF。
    // (ボード検出 check_spb 用の応答は M3a で拡張する)
    fn in_port(&self, port: u32) -> u8 {
        match port {
            0x188 => self.opna.read_status(),
            _ => 0xFF,
        }
    }

    fn emit(&mut self, bank: u8, reg: u8, data: u8) {
        // M2 は IntSelect=0(バッファB)固定で積む。
        let ty = if bank != 0 { PacketSend::BANK_SELECT } else { 0 };
        if let Err(e) = self.pipe.send(&PacketSend::new(ty, reg, data)) {
            if self.error.is_none() {
                self.error = Some(e);
            }
        }
    }

    /// バッチ終端 + 強制ドレイン(手動テスト末尾の 80 00 00 / 20 00 00 と同じ)。
    pub fn finish(&mut self) -> std::io::Result<()> {
        self.pipe.send(&PacketSend::new(PacketSend::INT_END, 0, 0))?;
        self.pipe.send(&PacketSend::new(PacketSend::FORCE_TIMEOUT, 0, 0))?;
        if let Some(e) = self.error.take() {
            return Err(e);
        }
        Ok(())
    }
}

// ---- shim.c から呼ばれる I/O コールバック ----------------------------------

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
