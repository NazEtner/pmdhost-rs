//! libx86emu(csrc/shim.c 経由)への薄い安全ラッパ。

use std::ffi::c_void;

// shim.c が公開する不透明ハンドル
#[repr(C)]
pub struct EmuRaw {
    _private: [u8; 0],
}

unsafe extern "C" {
    fn emu_create(user: *mut c_void) -> *mut EmuRaw;
    fn emu_load(emu: *mut EmuRaw, addr: u32, data: *const u8, len: u32);
    fn emu_set_start(emu: *mut EmuRaw, cs: u16, ip: u16, ss: u16, sp: u16);
    fn emu_run(emu: *mut EmuRaw, max_instr: u32);
    fn emu_done(emu: *mut EmuRaw);
}

pub struct Emu {
    raw: *mut EmuRaw,
}

impl Emu {
    /// `user` は I/O コールバックへ渡されるユーザポインタ(Host へのポインタ)。
    /// 呼び出し側は Emu の生存期間中、その指す先を生かし続けること。
    pub fn new(user: *mut c_void) -> Self {
        let raw = unsafe { emu_create(user) };
        assert!(!raw.is_null(), "x86emu_new に失敗しました");
        Self { raw }
    }

    pub fn load(&mut self, addr: u32, data: &[u8]) {
        unsafe { emu_load(self.raw, addr, data.as_ptr(), data.len() as u32) }
    }

    pub fn set_start(&mut self, cs: u16, ip: u16, ss: u16, sp: u16) {
        unsafe { emu_set_start(self.raw, cs, ip, ss, sp) }
    }

    pub fn run(&mut self, max_instr: u32) {
        unsafe { emu_run(self.raw, max_instr) }
    }
}

impl Drop for Emu {
    fn drop(&mut self) {
        unsafe { emu_done(self.raw) }
    }
}
