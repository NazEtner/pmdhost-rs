// libx86emu の薄いラッパ。
// Rust からは構造体レイアウトに触れず、ここで公開する数本の関数だけ FFI する。
// I/O(in/out)だけ Rust 側コールバックへ転送し、メモリ R/W/X は libx86emu 既定処理へ委譲する。
#include <stdint.h>
#include <stddef.h>
#include "x86emu.h"

// Rust 側で #[unsafe(no_mangle)] extern "C" として実装する(src/board.rs)
extern void     rust_io_out(void *user, uint32_t port, uint32_t val, uint32_t size);
extern uint32_t rust_io_in (void *user, uint32_t port, uint32_t size);

// メモリアクセス用の既定ハンドラ。I/O 以外はこちらへ委譲する。
static x86emu_memio_handler_t g_default_memio;

static unsigned shim_memio(x86emu_t *emu, u32 addr, u32 *val, unsigned type) {
    unsigned op = type & 0xff00;      // R/W/X/I/O の別
    unsigned size = type & 0xff;      // 0=8bit, 1=16bit, 2=32bit
    if (op == X86EMU_MEMIO_O) {       // OUT(I/O 書き込み)
        rust_io_out(emu->private, addr, *val, size);
        return 0;
    }
    if (op == X86EMU_MEMIO_I) {       // IN(I/O 読み出し)
        *val = rust_io_in(emu->private, addr, size);
        return 0;
    }
    return g_default_memio(emu, addr, val, type); // メモリは既定のページメモリへ
}

x86emu_t *emu_create(void *user) {
    // M2 のテスト用にメモリ/IO とも広めに許可する。
    x86emu_t *emu = x86emu_new(
        X86EMU_PERM_RWX | X86EMU_PERM_VALID,
        X86EMU_PERM_RW | X86EMU_PERM_VALID
    );
    if (!emu) return NULL;
    emu->private = user;
    g_default_memio = x86emu_set_memio_handler(emu, shim_memio);
    return emu;
}

void emu_load(x86emu_t *emu, uint32_t addr, const uint8_t *data, uint32_t len) {
    for (uint32_t i = 0; i < len; i++) {
        x86emu_write_byte(emu, addr + i, data[i]);
    }
}

void emu_set_start(x86emu_t *emu, uint16_t cs, uint16_t ip, uint16_t ss, uint16_t sp) {
    x86emu_set_seg_register(emu, emu->x86.R_CS_SEL, cs);
    x86emu_set_seg_register(emu, emu->x86.R_SS_SEL, ss);
    x86emu_set_seg_register(emu, emu->x86.R_DS_SEL, cs);
    x86emu_set_seg_register(emu, emu->x86.R_ES_SEL, cs);
    emu->x86.R_IP = ip;
    emu->x86.R_SP = sp;
}

void emu_run(x86emu_t *emu, uint32_t max_instr) {
    emu->max_instr = max_instr;
    x86emu_run(emu, X86EMU_RUN_MAX_INSTR);
}

void emu_done(x86emu_t *emu) {
    x86emu_done(emu);
}
