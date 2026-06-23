//! libx86emu(csrc/shim.c 経由)への薄い安全ラッパ。

use std::ffi::c_void;

// shim.c が公開する不透明ハンドル
#[repr(C)]
pub struct EmuRaw {
    _private: [u8; 0],
}

/// shim.c の emu_regs_t と一致(INT ハンドラのレジスタ受け渡し)。
#[repr(C)]
pub struct EmuRegs {
    pub ax: u16,
    pub bx: u16,
    pub cx: u16,
    pub dx: u16,
    pub si: u16,
    pub di: u16,
    pub bp: u16,
    pub sp: u16,
    pub ds: u16,
    pub es: u16,
    pub cs: u16,
    pub ip: u16,
    pub flags: u16,
}

#[allow(dead_code)] // al/dl は今後の DOS 拡張用
impl EmuRegs {
    pub fn ah(&self) -> u8 {
        (self.ax >> 8) as u8
    }
    pub fn al(&self) -> u8 {
        self.ax as u8
    }
    pub fn dl(&self) -> u8 {
        self.dx as u8
    }
    pub fn set_ax(&mut self, v: u16) {
        self.ax = v;
    }
}

unsafe extern "C" {
    fn emu_create(user: *mut c_void) -> *mut EmuRaw;
    fn emu_setup(emu: *mut EmuRaw, img: *const u8, n: u32);
    fn emu_run_install(emu: *mut EmuRaw);
    fn emu_call60(emu: *mut EmuRaw, ah: u8, al: u8, dx: u16, out_ds: *mut u16, out_dx: *mut u16);
    fn emu_load_mem(emu: *mut EmuRaw, seg: u16, off: u16, data: *const u8, n: u32);
    fn emu_find_timer_vec(emu: *mut EmuRaw) -> i32;
    fn emu_call_vec(emu: *mut EmuRaw, vec: u8, ah: u8, al: u8, dx: u16, out_ds: *mut u16, out_dx: *mut u16);
    fn emu_done(emu: *mut EmuRaw);
    fn emu_get_status(emu: *mut EmuRaw) -> u16;
    fn emu_call60_ax(emu: *mut EmuRaw, ah: u8, al: u8, dx: u16) -> u16;
}

pub struct Emu {
    raw: *mut EmuRaw,
}

impl Emu {
    /// `user` は I/O / INT コールバックへ渡されるユーザポインタ(Host へのポインタ)。
    /// 呼び出し側は Emu の生存期間中、その指す先を生かし続けること。
    pub fn new(user: *mut c_void) -> Self {
        let raw = unsafe { emu_create(user) };
        assert!(!raw.is_null(), "x86emu_new に失敗しました");
        Self { raw }
    }

    /// COM イメージ + PSP/MCB/環境/cmdline'#'/呼び出しスタブを配置。
    pub fn setup(&mut self, image: &[u8]) {
        unsafe { emu_setup(self.raw, image.as_ptr(), image.len() as u32) }
    }

    /// install 実行(TSR/exit で停止)。
    pub fn run_install(&mut self) {
        unsafe { emu_run_install(self.raw) }
    }

    /// INT 60h を1回呼ぶ。戻り値は (DS, DX)。
    pub fn call60(&mut self, ah: u8, al: u8, dx: u16) -> (u16, u16) {
        let mut ds = 0u16;
        let mut dxo = 0u16;
        unsafe { emu_call60(self.raw, ah, al, dx, &mut ds, &mut dxo) }
        (ds, dxo)
    }

    /// seg:off へバイト列を書く(曲データのロード等)。
    pub fn load_mem(&mut self, seg: u16, off: u16, data: &[u8]) {
        unsafe { emu_load_mem(self.raw, seg, off, data.as_ptr(), data.len() as u32) }
    }

    /// OPNA タイマ割り込み(opnint)のベクタ番号。install 後に呼ぶこと。
    pub fn find_timer_vec(&mut self) -> Option<u8> {
        let v = unsafe { emu_find_timer_vec(self.raw) };
        if v < 0 { None } else { Some(v as u8) }
    }

    /// GET_STATUS(AH=0Ah)。戻り (ST1, ST2)。ST2 はループ回数(0xFF で曲終了)。
    pub fn get_status(&mut self) -> (u8, u8) {
        let ax = unsafe { emu_get_status(self.raw) };
        ((ax >> 8) as u8, ax as u8)
    }

    /// 任意 INT60 を呼び AX を返す(GET_SYOUSETU 等の状態監視用)。
    pub fn call60_ax(&mut self, ah: u8, al: u8, dx: u16) -> u16 {
        unsafe { emu_call60_ax(self.raw, ah, al, dx) }
    }

    /// 任意ベクタを1回呼ぶ(タイマ ISR 駆動)。
    pub fn call_vec(&mut self, vec: u8, ah: u8, al: u8, dx: u16) -> (u16, u16) {
        let mut ds = 0u16;
        let mut dxo = 0u16;
        unsafe { emu_call_vec(self.raw, vec, ah, al, dx, &mut ds, &mut dxo) }
        (ds, dxo)
    }
}

impl Drop for Emu {
    fn drop(&mut self) {
        unsafe { emu_done(self.raw) }
    }
}
