//! 出力先。実機運用は名前付きパイプ(\\.\pipe\OPN3LD、driver.exe がサーバ)。
//! ドライラン(PMDHOST_DRY)はバッチサイズ(IntEnd 間の書き込み数)を計測して報告する。

use std::fs::{File, OpenOptions};
use std::io::{Read, Write};

use crate::packet::PacketSend;

pub const PIPE_NAME: &str = r"\\.\pipe\OPN3LD";

#[derive(Default)]
pub struct BatchStats {
    cur: u32,         // 現バッチの書き込み数
    cur_is_a: bool,   // 現バッチが バッファ A 向きか(最初の書き込みの IntSelect)
    started: bool,
    pub a: Vec<u32>,  // バッファ A(SSGドラム/TimerA)のバッチサイズ列
    pub b: Vec<u32>,  // バッファ B(音楽/TimerB)のバッチサイズ列
    recent: std::collections::VecDeque<(u8, u8, u8)>, // 直近の書き込み(bank,reg,data)
}

pub enum Pipe {
    Real(File),
    Stats(BatchStats),
}

impl Pipe {
    pub fn connect() -> std::io::Result<Self> {
        let file = OpenOptions::new().read(true).write(true).open(PIPE_NAME)?;
        Ok(Pipe::Real(file))
    }

    pub fn dump() -> Self {
        Pipe::Stats(BatchStats::default())
    }

    pub fn send(&mut self, packet: &PacketSend) -> std::io::Result<()> {
        match self {
            Pipe::Real(file) => {
                file.write_all(&packet.as_bytes())?;
                file.flush()
            }
            Pipe::Stats(s) => {
                let ty = packet.ty;
                if ty & PacketSend::INT_END != 0 {
                    // バッチ終端: 現バッチの書き込み数を記録
                    if ty & PacketSend::INT_SELECT_A != 0 {
                        s.a.push(s.cur);
                    } else {
                        s.b.push(s.cur);
                    }
                    s.cur = 0;
                    s.started = false;
                } else if ty & (PacketSend::FORCE_TIMEOUT | PacketSend::FLUSH | PacketSend::SIZE_REQUEST) != 0 {
                    // 制御パケットは無視
                } else {
                    // レジスタ書き込み
                    if !s.started {
                        s.cur_is_a = ty & PacketSend::INT_SELECT_A != 0;
                        s.started = true;
                    }
                    s.cur += 1;
                    let bank = ty & PacketSend::BANK_SELECT;
                    s.recent.push_back((bank, packet.reg_address, packet.data));
                    if s.recent.len() > 80 {
                        s.recent.pop_front();
                    }
                }
                Ok(())
            }
        }
    }

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
            Pipe::Stats(_) => Ok(0),
        }
    }

    /// 計測結果(バッチサイズ統計)を表示。
    pub fn print_stats(&self) {
        if let Pipe::Stats(s) = self {
            report("バッファB(音楽/TimerB)", &s.b);
            report("バッファA(SSGドラム/TimerA)", &s.a);
            println!("--- 停止直前の書き込み(最後の{}個) ---", s.recent.len());
            for (i, (bank, reg, data)) in s.recent.iter().enumerate() {
                print!("{}{:02X}={:02X} ", if *bank != 0 { "b" } else { "a" }, reg, data);
                if i % 10 == 9 {
                    println!();
                }
            }
            println!();
        }
    }
}

fn report(name: &str, sizes: &[u32]) {
    if sizes.is_empty() {
        println!("{name}: バッチ無し");
        return;
    }
    let mut v: Vec<u32> = sizes.iter().copied().filter(|&x| x > 0).collect();
    if v.is_empty() {
        println!("{name}: 空バッチのみ {} 個", sizes.len());
        return;
    }
    v.sort_unstable();
    let n = v.len();
    let sum: u64 = v.iter().map(|&x| x as u64).sum();
    let max = v[n - 1];
    let p50 = v[n / 2];
    let p90 = v[n * 9 / 10];
    let p99 = v[(n * 99 / 100).min(n - 1)];
    let over256 = v.iter().filter(|&&x| x > 256).count();
    println!(
        "{name}: 件数={n} 平均={:.1} 中央={p50} p90={p90} p99={p99} 最大={max}  (256超え={over256}件)",
        sum as f64 / n as f64
    );
}
