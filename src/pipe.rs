//! 出力先。実機運用は名前付きパイプ(\\.\pipe\OPN3LD、driver.exe がサーバ)。
//! ドライラン(PMDHOST_DRY)はレジスタ書き込みを stdout にダンプして検証する。

use std::fs::{File, OpenOptions};
use std::io::{Read, Write};

use crate::packet::PacketSend;

pub const PIPE_NAME: &str = r"\\.\pipe\OPN3LD";

pub enum Pipe {
    Real(File),
    Dump { count: u64 }, // 先頭のみ表示
}

impl Pipe {
    pub fn connect() -> std::io::Result<Self> {
        let file = OpenOptions::new().read(true).write(true).open(PIPE_NAME)?;
        Ok(Pipe::Real(file))
    }

    pub fn dump() -> Self {
        Pipe::Dump { count: 0 }
    }

    pub fn send(&mut self, packet: &PacketSend) -> std::io::Result<()> {
        match self {
            Pipe::Real(file) => {
                file.write_all(&packet.as_bytes())?;
                file.flush()
            }
            Pipe::Dump { count } => {
                // レジスタ書き込み(制御ビットなし)だけを先頭 240 個表示
                let ctrl = packet.ty & !PacketSend::BANK_SELECT;
                if ctrl == 0 && *count < 240 {
                    let bank = packet.ty & PacketSend::BANK_SELECT;
                    print!("{}{:02X}={:02X} ", if bank != 0 { "b" } else { "a" }, packet.reg_address, packet.data);
                    if *count % 8 == 7 {
                        println!();
                    }
                }
                *count += 1;
                Ok(())
            }
        }
    }

    /// driver.exe の送信キュー長を問い合わせる(SizeRequest)。背圧スロットル用。
    pub fn query_size(&mut self) -> std::io::Result<u32> {
        match self {
            Pipe::Real(file) => {
                let req = PacketSend::new(PacketSend::SIZE_REQUEST, 0, 0);
                file.write_all(&req.as_bytes())?;
                file.flush()?;
                let mut buf = [0u8; 4];
                file.read_exact(&mut buf)?;
                Ok(u32::from_le_bytes(buf))
            }
            Pipe::Dump { .. } => Ok(0),
        }
    }
}
