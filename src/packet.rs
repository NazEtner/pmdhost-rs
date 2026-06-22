//! ボード(driver.exe)へ送る 4 バイトパケット。
//! YML288BoardDriver の driver.hpp `PacketSend` と完全一致(endSym=0xAA 固定)。

#[repr(C)]
#[derive(Clone, Copy)]
pub struct PacketSend {
    pub ty: u8,          // type ビット(下記定数)
    pub reg_address: u8,
    pub data: u8,
    pub end_sym: u8,     // 0xAA 固定
}

impl PacketSend {
    pub const END_SYM: u8 = 0xAA;

    // type ビット(MSB First)
    pub const INT_END: u8 = 0x80; // IntSelect 側のバッチ終端
    pub const INT_SELECT_A: u8 = 0x40; // 1=Timer A バッファ / 0=Timer B
    pub const FORCE_TIMEOUT: u8 = 0x20; // 強制ドレイン
    pub const FLUSH: u8 = 0x10;
    pub const SIZE_REQUEST: u8 = 0x08;
    pub const BANK_SELECT: u8 = 0x01; // 1=裏(18Ch/18Eh) / 0=表(188h/18Ah)

    pub fn new(ty: u8, reg_address: u8, data: u8) -> Self {
        Self { ty, reg_address, data, end_sym: Self::END_SYM }
    }

    pub fn as_bytes(&self) -> [u8; 4] {
        [self.ty, self.reg_address, self.data, self.end_sym]
    }
}
