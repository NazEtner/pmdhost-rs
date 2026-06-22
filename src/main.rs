//! pmdhost-rs — 本物の PMD を 8086 エミュ(libx86emu)上で動かし、出てくる OPNA レジスタ
//! 書き込みを捕獲して PacketSend にし、driver.exe 経由で実機 YMF288 を鳴らす。
//!
//! host-paced + 正しいテンポ: PMD(sync=0)のタイマ割り込み(opnint→FM_Timer_main)を 1 tick ずつ
//! 駆動し、各 tick のレジスタ書き込みをボードへ ForceTimeout で即適用(M1 で実証済みの経路)。
//! テンポは PMD が書く Timer B 値(0x26)から算出して刻む。テンポ変更にも自動追従。
//! 検証元 tools/pmdrun.c。

mod board;
mod emu;
mod opna;
mod packet;
mod pipe;

use std::time::Instant;

use board::Host;
use emu::Emu;
use pipe::Pipe;

static PMD_BIN: &[u8] = include_bytes!("../assets/pmdymf.bin");

// Windows のタイマ分解能を 1ms に上げる(既定 ~15.6ms だと数 ms のテンポ間隔が出せない)。
#[link(name = "winmm")]
unsafe extern "system" {
    fn timeBeginPeriod(u_period: u32) -> u32;
}

fn main() {
    unsafe { timeBeginPeriod(1) };

    let song_path = match std::env::args().nth(1) {
        Some(p) => p,
        None => {
            eprintln!("usage: pmdhost-rs <song.M>");
            std::process::exit(2);
        }
    };
    let dry = std::env::var("PMDHOST_DRY").is_ok();
    // テンポ微調整倍率(1.0=既定。速ければ大きく、遅ければ小さく)。
    let tempo_mul: f64 = std::env::var("PMDHOST_TEMPO")
        .ok()
        .and_then(|s| s.parse().ok())
        .filter(|v: &f64| *v > 0.0)
        .unwrap_or(1.0);

    let song = match std::fs::read(&song_path) {
        Ok(d) => d,
        Err(e) => {
            eprintln!("曲データを読めません ({song_path}): {e}");
            std::process::exit(1);
        }
    };

    let pipe = if dry {
        println!("[dry-run] レジスタ書き込みをダンプ(実機なし)");
        Pipe::dump()
    } else {
        match Pipe::connect() {
            Ok(p) => p,
            Err(e) => {
                eprintln!("パイプ接続失敗 ({}): 先に driver.exe を起動してください\n{e}", pipe::PIPE_NAME);
                std::process::exit(1);
            }
        }
    };

    let mut host = Box::new(Host::new(pipe));
    let user = (&mut *host as *mut Host).cast::<std::ffi::c_void>();
    let mut emu = Emu::new(user);

    // --- install ---
    println!("PMD をエミュにロードして install 中…");
    emu.setup(PMD_BIN);
    emu.run_install();
    if !host.installed {
        eprintln!("install に失敗(exited={})", host.exited);
        std::process::exit(1);
    }
    let timer_vec = match emu.find_timer_vec() {
        Some(v) => v,
        None => {
            eprintln!("タイマ割り込みベクタが見つかりません");
            std::process::exit(1);
        }
    };
    host.set_timer_vec(timer_vec);
    println!("install OK(YMF288 認識)。タイマベクタ=INT {timer_vec:02X}。");
    if let Err(e) = host.flush_drain() {
        eprintln!("パイプ送信エラー: {e}");
        std::process::exit(1);
    }

    // --- 曲ロード & 演奏開始 ---
    let (seg, off) = emu.call60(0x06, 0, 0);
    println!("曲バッファ {seg:04X}:{off:04X} へ {} バイト書込", song.len());
    emu.load_mem(seg, off, &song);
    emu.call60(0x00, 0, 0); // MUSIC_START
    let _ = host.flush_drain();
    println!("演奏開始(テンポは曲の Timer B 値から自動 / PMDHOST_TEMPO={tempo_mul} で微調整)。Ctrl-C で停止。");

    // --- tick ループ(各 tick を ForceTimeout で即適用、テンポ間隔で待つ)---
    let max_ticks: Option<u64> = if dry { Some(40) } else { None };
    let mut next = Instant::now();
    let mut t: u64 = 0;
    loop {
        host.arm_timer_b();
        emu.call_vec(timer_vec, 0, 0, 0); // opnint → FM_Timer_main(1 tick)
        if let Err(e) = host.flush_drain() {
            eprintln!("パイプ送信エラー: {e}");
            break;
        }
        t += 1;
        if let Some(m) = max_ticks {
            if t >= m {
                println!("\n[dry-run] {t} tick 完了");
                break;
            }
            continue;
        }
        // テンポ間隔で待つ(0x26 を捕獲しているので自動追従)。絶対時刻でジッタ蓄積を防ぐ。
        let us = (host.tick_base_micros() as f64 * tempo_mul) as u64;
        next += std::time::Duration::from_micros(us.max(1));
        let now = Instant::now();
        if next > now {
            std::thread::sleep(next - now);
        } else {
            next = now;
        }
    }
}
