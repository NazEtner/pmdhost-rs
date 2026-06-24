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

use board::Host;
use emu::Emu;
use pipe::Pipe;

static PMD_BIN: &[u8] = include_bytes!("../assets/pmdymf.bin");

// 先読み量(driver.exe の送信キュー上限)。大きいほどボード前に曲を溜め込み、重い区間で
// バッファ枯れ(カクつき)しにくくなる一方、操作(stop/fade等)の効きがその分遅れる。
// PMDHOST_QUEUE で調整可(既定 450 ≒ 0.3秒の先読み。実機で 450 でも安定再生を確認、操作レスポンス優先)。

// Windows のタイマ分解能を 1ms に上げる(既定 ~15.6ms だと数 ms のテンポ間隔が出せない)。
#[link(name = "winmm")]
unsafe extern "system" {
    fn timeBeginPeriod(u_period: u32) -> u32;
}

// コンソールの ANSI(VT)処理を有効化(エミュ出力の色付け用。Windows Terminal は元から対応、
// 旧 conhost でも有効化しておく)。
#[link(name = "kernel32")]
unsafe extern "system" {
    fn GetStdHandle(n_std_handle: u32) -> *mut std::ffi::c_void;
    fn GetConsoleMode(h: *mut std::ffi::c_void, mode: *mut u32) -> i32;
    fn SetConsoleMode(h: *mut std::ffi::c_void, mode: u32) -> i32;
}

fn enable_vt() {
    const STD_OUTPUT_HANDLE: u32 = -11i32 as u32;
    const ENABLE_VIRTUAL_TERMINAL_PROCESSING: u32 = 0x0004;
    unsafe {
        let h = GetStdHandle(STD_OUTPUT_HANDLE);
        let mut mode = 0u32;
        if GetConsoleMode(h, &mut mode) != 0 {
            SetConsoleMode(h, mode | ENABLE_VIRTUAL_TERMINAL_PROCESSING);
        }
    }
}

fn main() {
    unsafe { timeBeginPeriod(1) };
    enable_vt();

    let song_path = match std::env::args().nth(1) {
        Some(p) => p,
        None => {
            eprintln!("usage: pmdhost-rs <song.M>");
            std::process::exit(2);
        }
    };
    let dry = std::env::var("PMDHOST_DRY").is_ok();
    let queue_high: u32 = std::env::var("PMDHOST_QUEUE")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(450);

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
    // 前回再生の残留(driver送信キュー + ボードのバッファ/orderキュー/pendingTimers)をリセットしてから
    // 開始する。driver 再起動なしの連続再生で残留が desync し途中フリーズするのを防ぐ。install の書込より前。
    if let Err(e) = host.flush_reset() {
        eprintln!("パイプ送信エラー(flush): {e}");
        std::process::exit(1);
    }
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
    emu.call60(0x00, 0, 0); // MUSIC_START(ここで Timer A/B のレートが確定。0x27 はまだ転送マスク)
    let _ = host.flush_drain();

    // --- board-paced ---
    // 各イベント(A/B)のバッチをボードのバッファ A/B へ積むだけ。ボードの実タイマ /IRQ が drain。
    // ホストは driver キュー長で背圧をかけ、毎イベントの往復はしない(処理落ち対策)。
    host.arm_board(); // 以降 0x27 を転送(ボードのタイマ始動準備)
    // 最初のイベントは ForceTimeout で即適用 → ボードの TimerA/B が起動。
    if host.next_event().is_some() {
        emu.call_vec(timer_vec, 0, 0, 0);
        let _ = host.flush_drain();
    }
    println!("演奏開始(board-paced: ボードのタイマがテンポを律速)。Ctrl-C で停止。");

    // ドライランは多めに回してバッチサイズ分布を採取(実機運用は無限)。
    let dry_ticks: u64 = std::env::var("PMDHOST_DRYTICKS").ok().and_then(|s| s.parse().ok()).unwrap_or(30000);
    let max_ticks: Option<u64> = if dry { Some(dry_ticks) } else { None };
    let mut t: u64 = 0;
    let status_poll = std::env::var("PMDHOST_STATUS").is_ok();
    let mut last_st2: u8 = 0;
    loop {
        if host.next_event().is_none() {
            std::thread::sleep(std::time::Duration::from_millis(1));
            continue;
        }
        // 先回りしすぎないよう driver キュー長で背圧(8 イベントごとに確認)。
        if !dry && t % 8 == 0 {
            match host.queue_size() {
                Ok(mut q) => {
                    while q > queue_high {
                        std::thread::sleep(std::time::Duration::from_millis(1));
                        q = host.queue_size().unwrap_or(0);
                    }
                }
                Err(e) => { eprintln!("キュー問い合わせエラー: {e}"); break; }
            }
        }
        emu.call_vec(timer_vec, 0, 0, 0); // opnint → FM_Timer_main(該当タイマ)
        // 任意: 演奏状態の監視(ST2 はループ回数, 小節 は GET_SYOUSETU)。
        if dry && status_poll && t % 100 == 0 {
            let (s1, s2) = emu.get_status();
            if s2 != last_st2 || t % 20000 == 0 {
                let syousetu = emu.call60_ax(0x05, 0, 0); // GET_SYOUSETU(小節カウンタ)
                println!("[status] t={t} ST1={s1:02X} ST2(ループ数)={s2:02X} 小節={syousetu}");
                last_st2 = s2;
            }
        }
        if let Err(e) = host.end_batch() {
            eprintln!("パイプ送信エラー: {e}");
            break;
        }
        t += 1;
        if let Some(m) = max_ticks {
            if t >= m {
                println!("[dry-run] {t} イベント完了。バッチサイズ統計:");
                host.print_stats();
                break;
            }
        }
    }
}
