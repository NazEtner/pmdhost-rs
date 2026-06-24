//! pmdhost-rs — 本物の PMD を 8086 エミュ(libx86emu)上で動かし、出てくる OPNA レジスタ
//! 書き込みを捕獲して PacketSend にし、driver.exe 経由で実機 YMF288 を鳴らす。
//!
//! board-paced: PMD のタイマ割り込み(opnint→FM_Timer_main)を 1 tick ずつ駆動し、各 tick の
//! レジスタ書き込みをボードのバッファへ積む。テンポはボードの実タイマが律速する。
//! 演奏は stdin プロンプト / 制御TCP(既定 127.0.0.1)から操作できる(play/stop/fade/status)。

mod board;
mod emu;
mod opna;
mod packet;
mod pipe;

use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::sync::mpsc;
use std::time::Duration;

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

// コンソールの ANSI(VT)処理を有効化(エミュ出力の色付け用)。
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

// ---- 制御コマンド -----------------------------------------------------------

enum CmdKind {
    Play(String), // 曲ファイルパス
    Stop,
    Fade(u8), // 速度(1=最遅)
    Status,
    Quit,
}

/// 入力源(stdin/TCP)でパースされ、mpsc で main へ送られる。emu に触るのは main だけ。
struct Command {
    kind: CmdKind,
    remote: bool,                  // TCP 由来か(play のパス制限に使う)
    reply: mpsc::Sender<String>,   // 結果文字列の返送先(送り主が表示)
}

/// 行テキスト → コマンド。`play <file>` / `stop` / `fade <n>` / `status` / `quit`。
fn parse_command(line: &str) -> Option<CmdKind> {
    let line = line.trim();
    if line.is_empty() {
        return None;
    }
    let mut it = line.splitn(2, char::is_whitespace);
    let cmd = it.next()?.to_ascii_lowercase();
    let arg = it.next().unwrap_or("").trim();
    match cmd.as_str() {
        "play" | "p" => Some(CmdKind::Play(arg.to_string())),
        "stop" | "s" => Some(CmdKind::Stop),
        "fade" | "f" => arg.parse::<u8>().ok().map(CmdKind::Fade),
        "status" | "st" => Some(CmdKind::Status),
        "quit" | "exit" | "q" => Some(CmdKind::Quit),
        _ => None,
    }
}

/// play 対象パスを解決。ローカル(プロンプト)は無制限。リモート(TCP)は PMDHOST_MUSICDIR 配下のみ
/// 許可(未設定なら拒否)。パストラバーサルは canonicalize + prefix チェックで防ぐ。
fn resolve_play_path(path: &str, remote: bool, music_dir: &Option<PathBuf>) -> Result<PathBuf, String> {
    if !remote {
        return Ok(PathBuf::from(path));
    }
    let dir = match music_dir {
        Some(d) => d,
        None => return Err("remote play 不可(PMDHOST_MUSICDIR 未設定)".into()),
    };
    let joined = dir.join(path);
    match (joined.canonicalize(), dir.canonicalize()) {
        (Ok(jp), Ok(dp)) if jp.starts_with(&dp) => Ok(jp),
        _ => Err("許可外のパス".into()),
    }
}

/// 曲データをロードして演奏開始(曲切替にも使う)。ボード/driver/opna をリセットしてから流す。
fn load_and_play(emu: &mut Emu, host: &mut Host, timer_vec: u8, song: &[u8]) -> bool {
    let _ = host.flush_reset(); // ボード/driverキューの残留を破棄(連続再生の desync 防止)
    host.reset_intsel_to_b(); // init 書き込みを音楽=バッファB へ(切替時に A に積まれるのを防ぐ)
    host.reset_timers(); // opna モデル・SSGシャドウ・arm 状態を初期化(MUSIC_START の 0x27 をマスクし直す)
    emu.call60(0x01, 0, 0); // MUSIC_STOP(旧曲の全chキーオフ=新曲の頭に被らせない)
    let _ = host.flush_drain();
    let (seg, off) = emu.call60(0x06, 0, 0); // GET_MUSDAT_ADR
    emu.load_mem(seg, off, song);
    emu.call60(0x00, 0, 0); // MUSIC_START(ここで NA/NB が確定)
    let _ = host.flush_drain();
    host.arm_board();
    // 最初のイベントを ForceTimeout で即適用 → ボードの TimerA/B 始動
    if host.next_event().is_some() {
        emu.call_vec(timer_vec, 0, 0, 0);
        let _ = host.flush_drain();
    }
    true
}

/// コマンドを実行(emu/host を操作)。結果文字列を返す(送り主が表示)。
fn exec_command(
    emu: &mut Emu,
    host: &mut Host,
    timer_vec: u8,
    playing: &mut bool,
    music_dir: &Option<PathBuf>,
    cmd: &Command,
) -> String {
    match &cmd.kind {
        CmdKind::Play(path) => match resolve_play_path(path, cmd.remote, music_dir) {
            Ok(p) => match std::fs::read(&p) {
                Ok(song) => {
                    if load_and_play(emu, host, timer_vec, &song) {
                        *playing = true;
                        format!("playing: {} ({} bytes)", p.display(), song.len())
                    } else {
                        "play 失敗".into()
                    }
                }
                Err(e) => format!("読めません: {e}"),
            },
            Err(msg) => msg,
        },
        CmdKind::Stop => {
            // 1. 先読み済みの音楽を破棄(ボード/driverバッファをクリア=即無音化)
            let _ = host.flush_reset();
            // 2. MUSIC_STOP で全chキーオフ(silence)を生成
            emu.call60(0x01, 0, 0);
            // 3. そのキーオフをボードへ即適用(これが無いと最後の音が鳴りっぱなしになる)
            let _ = host.flush_drain();
            *playing = false;
            "stopped".into()
        }
        CmdKind::Fade(n) => {
            emu.call60(0x02, *n, 0); // FADEOUT(速度 n)
            format!("fade {n} (バッファ分だけ遅れて効きます)")
        }
        CmdKind::Status => {
            let (s1, s2) = emu.get_status();
            let syousetu = emu.call60_ax(0x05, 0, 0); // GET_SYOUSETU
            format!("playing={} ST1={s1:02X} loop={s2} measure={syousetu}", *playing)
        }
        CmdKind::Quit => "bye".into(),
    }
}

/// stdin プロンプト: 1行読んでコマンド化し、結果を表示してから次のプロンプトを出す。
fn spawn_stdin(tx: mpsc::Sender<Command>) {
    std::thread::spawn(move || {
        let stdin = std::io::stdin();
        loop {
            print!("pmd> ");
            let _ = std::io::stdout().flush();
            let mut line = String::new();
            if stdin.lock().read_line(&mut line).unwrap_or(0) == 0 {
                break; // EOF
            }
            match parse_command(&line) {
                Some(kind) => {
                    let quit = matches!(kind, CmdKind::Quit);
                    let (rtx, rrx) = mpsc::channel();
                    if tx.send(Command { kind, remote: false, reply: rtx }).is_err() {
                        break;
                    }
                    if let Ok(resp) = rrx.recv_timeout(Duration::from_secs(3)) {
                        println!("{resp}");
                    }
                    if quit {
                        break;
                    }
                }
                None if !line.trim().is_empty() => {
                    println!("? play <file> / stop / fade <n> / status / quit");
                }
                None => {}
            }
        }
    });
}

/// TCP の1接続を処理。行コマンドを受けて結果を返す(リモートなので quit はホストを落とさず切断)。
fn handle_tcp_client(stream: TcpStream, tx: mpsc::Sender<Command>) {
    let peer = stream.peer_addr().map(|a| a.to_string()).unwrap_or_default();
    println!("制御TCP接続: {peer}");
    let mut writer = match stream.try_clone() {
        Ok(w) => w,
        Err(_) => return,
    };
    let _ = writeln!(writer, "pmdhost control: play <file>|stop|fade <n>|status|bye");
    let mut reader = BufReader::new(stream);
    let mut line = String::new();
    loop {
        line.clear();
        if reader.read_line(&mut line).unwrap_or(0) == 0 {
            break; // 切断
        }
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        match parse_command(&line) {
            Some(CmdKind::Quit) => {
                let _ = writeln!(writer, "bye");
                break; // 接続を閉じるだけ(ホストは終了させない)
            }
            Some(kind) => {
                let (rtx, rrx) = mpsc::channel();
                if tx.send(Command { kind, remote: true, reply: rtx }).is_err() {
                    break;
                }
                let resp = rrx
                    .recv_timeout(Duration::from_secs(3))
                    .unwrap_or_else(|_| "(no response)".into());
                if writeln!(writer, "{resp}").is_err() {
                    break;
                }
            }
            None => {
                let _ = writeln!(writer, "? unknown");
            }
        }
    }
    println!("制御TCP切断: {peer}");
}

/// 制御TCP待受(既定 127.0.0.1:5288。PMDHOST_BIND で上書き=公開は opt-in)。
fn spawn_tcp(tx: mpsc::Sender<Command>) {
    let addr = std::env::var("PMDHOST_BIND").unwrap_or_else(|_| "127.0.0.1:5288".into());
    let listener = match TcpListener::bind(&addr) {
        Ok(l) => l,
        Err(e) => {
            eprintln!("制御TCP bind 失敗 ({addr}): {e}");
            return;
        }
    };
    println!("制御TCP待受: {addr}");
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let tx = tx.clone();
            std::thread::spawn(move || handle_tcp_client(stream, tx));
        }
    });
}

fn main() {
    unsafe { timeBeginPeriod(1) };
    enable_vt();

    let song_path = std::env::args().nth(1);
    let dry = std::env::var("PMDHOST_DRY").is_ok();
    let queue_high: u32 = std::env::var("PMDHOST_QUEUE")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(450);

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
    // 前回再生の残留(driver送信キュー + ボードのバッファ/orderキュー)をリセットしてから開始。
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
    let _ = host.flush_drain();

    // --- ドライラン(統計採取。実機なし) ---
    if dry {
        let song = match song_path.as_deref().map(std::fs::read) {
            Some(Ok(d)) => d,
            _ => {
                eprintln!("[dry-run] 曲ファイルを指定してください");
                std::process::exit(1);
            }
        };
        load_and_play(&mut emu, &mut host, timer_vec, &song);
        let dry_ticks: u64 = std::env::var("PMDHOST_DRYTICKS").ok().and_then(|s| s.parse().ok()).unwrap_or(30000);
        let status_poll = std::env::var("PMDHOST_STATUS").is_ok();
        let mut last_st2: u8 = 0;
        let mut t: u64 = 0;
        loop {
            if host.next_event().is_none() {
                std::thread::sleep(Duration::from_millis(1));
                continue;
            }
            emu.call_vec(timer_vec, 0, 0, 0);
            if status_poll && t % 100 == 0 {
                let (s1, s2) = emu.get_status();
                if s2 != last_st2 || t % 20000 == 0 {
                    let syousetu = emu.call60_ax(0x05, 0, 0);
                    println!("[status] t={t} ST1={s1:02X} ST2(ループ数)={s2:02X} 小節={syousetu}");
                    last_st2 = s2;
                }
            }
            let _ = host.end_batch();
            t += 1;
            if t >= dry_ticks {
                println!("[dry-run] {t} イベント完了。バッチサイズ統計:");
                host.print_stats();
                return;
            }
        }
    }

    // --- 制御(プロンプト + TCP) ---
    let (tx, rx) = mpsc::channel::<Command>();
    spawn_stdin(tx.clone());
    spawn_tcp(tx.clone());
    let music_dir = std::env::var("PMDHOST_MUSICDIR").ok().map(PathBuf::from);

    let mut playing = false;
    if let Some(p) = song_path.as_deref() {
        match std::fs::read(p) {
            Ok(song) => {
                load_and_play(&mut emu, &mut host, timer_vec, &song);
                playing = true;
                println!("演奏開始: {p}");
            }
            Err(e) => eprintln!("曲データを読めません ({p}): {e}"),
        }
    }
    println!("操作: play <file> / stop / fade <n> / status / quit(プロンプト or TCP)。");

    let mut t: u64 = 0;
    loop {
        // コマンド処理(emu に触るのはここだけ)。背圧待ち中も応答できるよう毎周ポーリング。
        while let Ok(cmd) = rx.try_recv() {
            let quit = matches!(cmd.kind, CmdKind::Quit);
            let resp = exec_command(&mut emu, &mut host, timer_vec, &mut playing, &music_dir, &cmd);
            let _ = cmd.reply.send(resp);
            if quit {
                println!("終了します。");
                return;
            }
        }

        if !playing {
            std::thread::sleep(Duration::from_millis(10));
            continue;
        }

        // 先回りしすぎないよう driver キュー長で背圧。深ければ駆動せず先頭(コマンド処理)へ戻る。
        if t % 8 == 0 {
            if let Ok(q) = host.queue_size() {
                if q > queue_high {
                    std::thread::sleep(Duration::from_millis(1));
                    continue;
                }
            }
        }

        if host.next_event().is_some() {
            emu.call_vec(timer_vec, 0, 0, 0);
            if let Err(e) = host.end_batch() {
                eprintln!("パイプ送信エラー: {e}");
                playing = false;
            }
            t += 1;
        } else {
            std::thread::sleep(Duration::from_millis(1));
        }
    }
}
