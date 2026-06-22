//! 名前付きパイプ(\\.\pipe\OPN3LD)のクライアント。
//! driver.exe がサーバ。ここはバイトモードで PacketSend を書き込むだけ。

use std::fs::{File, OpenOptions};
use std::io::Write;

use crate::packet::PacketSend;

pub const PIPE_NAME: &str = r"\\.\pipe\OPN3LD";

pub struct Pipe {
    file: File,
}

impl Pipe {
    pub fn connect() -> std::io::Result<Self> {
        let file = OpenOptions::new().read(true).write(true).open(PIPE_NAME)?;
        Ok(Self { file })
    }

    pub fn send(&mut self, packet: &PacketSend) -> std::io::Result<()> {
        self.file.write_all(&packet.as_bytes())?;
        self.file.flush()
    }
}
