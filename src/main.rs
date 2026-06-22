//! pmdhost-rs — PMD 演奏を YMF288 実機で鳴らすためのホスト。
//! M2(骨格): 8086エミュ(libx86emu)上で手書きの小プログラムを実行し、
//! その out 命令を捕獲 → PacketSend → driver.exe → ボードで発音、という経路を実証する。

mod board;
mod emu;
mod opna;
mod packet;
mod pipe;

use board::Host;
use emu::Emu;
use pipe::Pipe;

// ユーザが実機で鳴らせた既知の良データ(FM 1ch 発音、すべて表=bank0)。
// (reg, data) の並び。これを 8086 機械語の out 列に変換して流す。
const TEST_WRITES: &[(u8, u8)] = &[
    (0x29, 0x80),
    (0xB4, 0xC0),
    (0xB0, 0xC7),
    (0x40, 0x00), (0x50, 0x1F), (0x60, 0x00), (0x70, 0x00), (0x80, 0x0F),
    (0x44, 0x00), (0x54, 0x1F), (0x64, 0x00), (0x74, 0x00), (0x84, 0x0F),
    (0x48, 0x00), (0x58, 0x1F), (0x68, 0x00), (0x78, 0x00), (0x88, 0x0F),
    (0x4C, 0x00), (0x5C, 0x1F), (0x6C, 0x00), (0x7C, 0x00), (0x8C, 0x0F),
    (0xA4, 0x22), (0xA0, 0x69),
    (0x28, 0xF0),
];

const LOAD_SEG: u16 = 0x1000;
const LOAD_OFS: u16 = 0x0000;
const STACK_SEG: u16 = 0x2000;
const STACK_PTR: u16 = 0xFFFE;

// (reg,data) 列を「OPNA 表ポートへ out する 8086 機械語」に変換する。
//   mov dx, 0x0188 ; mov al, reg  ; out dx, al   (アドレスラッチ)
//   mov dx, 0x018A ; mov al, data ; out dx, al   (データ)
// 末尾に hlt。
fn build_program(writes: &[(u8, u8)]) -> Vec<u8> {
    let mut code = Vec::new();
    for &(reg, data) in writes {
        code.extend_from_slice(&[0xBA, 0x88, 0x01, 0xB0, reg, 0xEE]);
        code.extend_from_slice(&[0xBA, 0x8A, 0x01, 0xB0, data, 0xEE]);
    }
    code.push(0xF4); // hlt
    code
}

fn main() {
    println!(
        "pmdhost-rs (M2 skeleton)  build {} [{}]",
        env!("BUILD_TIME"),
        env!("BUILD_TARGET")
    );

    let pipe = match Pipe::connect() {
        Ok(p) => p,
        Err(e) => {
            eprintln!(
                "パイプ接続失敗 ({}): 先に driver.exe を起動してください\n{e}",
                pipe::PIPE_NAME
            );
            std::process::exit(1);
        }
    };

    // Host は I/O コールバックからポインタ経由で参照するので、emu 実行中は生かし続ける。
    let mut host = Box::new(Host::new(pipe));
    let user = (&mut *host as *mut Host).cast::<std::ffi::c_void>();

    let mut emu = Emu::new(user);

    let program = build_program(TEST_WRITES);
    println!("テストプログラム {} バイト({} レジスタ書き込み)を実行", program.len(), TEST_WRITES.len());

    let load_addr = ((LOAD_SEG as u32) << 4) | LOAD_OFS as u32;
    emu.load(load_addr, &program);
    emu.set_start(LOAD_SEG, LOAD_OFS, STACK_SEG, STACK_PTR);
    emu.run(1_000_000);

    match host.finish() {
        Ok(()) => println!("完了。ボードが鳴れば M2 の配線 OK。"),
        Err(e) => eprintln!("パイプ送信エラー: {e}"),
    }
}
