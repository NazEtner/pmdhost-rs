// libx86emu の薄いラッパ + PMD 駆動の機構的オーケストレーション。
// 方針(政策)は Rust 側コールバック(I/O・DOS・検出)に置き、ここはエミュ操作の機構だけを持つ。
// 検証元: tools/pmdrun.c(C 観測ハーネス)。
#include <stdint.h>
#include <stddef.h>
#include "x86emu.h"

// メモリ配置(COM: PSP at LOADSEG:0, program at LOADSEG:0100)
#define LOADSEG 0x1000u
#define STUBSEG 0x0E00u  // int 60h; hlt の呼び出しスタブ
#define ENVSEG  0x0F00u  // 空の環境ブロック

// Rust 側で #[unsafe(no_mangle)] extern "C" 実装(src/board.rs)
extern void     rust_io_out(void *user, uint32_t port, uint32_t val, uint32_t size);
extern uint32_t rust_io_in (void *user, uint32_t port, uint32_t size);

// レジスタ受け渡し用(Rust の #[repr(C)] EmuRegs と一致)
typedef struct {
    uint16_t ax, bx, cx, dx, si, di, bp, sp, ds, es, cs, ip, flags;
} emu_regs_t;
// 戻り値: 0=未処理(既定IVT経由) / 1=処理済み / 2=処理済み+停止
extern int rust_intr(void *user, uint8_t num, emu_regs_t *r);

static x86emu_memio_handler_t g_default_memio;

static unsigned shim_memio(x86emu_t *emu, u32 addr, u32 *val, unsigned type) {
    unsigned op = type & 0xff00;
    unsigned size = type & 0xff;
    if (op == X86EMU_MEMIO_O) { rust_io_out(emu->private, addr, *val, size); return 0; }
    if (op == X86EMU_MEMIO_I) { *val = rust_io_in(emu->private, addr, size); return 0; }
    return g_default_memio(emu, addr, val, type);
}

static int shim_intr(x86emu_t *emu, u8 num, unsigned type) {
    (void)type;
    emu_regs_t r;
    r.ax = emu->x86.R_AX; r.bx = emu->x86.R_BX; r.cx = emu->x86.R_CX; r.dx = emu->x86.R_DX;
    r.si = emu->x86.R_SI; r.di = emu->x86.R_DI; r.bp = emu->x86.R_BP; r.sp = emu->x86.R_SP;
    r.ds = emu->x86.R_DS; r.es = emu->x86.R_ES; r.cs = emu->x86.R_CS; r.ip = emu->x86.R_IP;
    r.flags = emu->x86.R_FLG;
    int ret = rust_intr(emu->private, num, &r);
    if (ret != 0) {
        emu->x86.R_AX = r.ax; emu->x86.R_BX = r.bx; emu->x86.R_CX = r.cx; emu->x86.R_DX = r.dx;
        // セグメントは base 再計算のため set_seg_register 経由で書く
        x86emu_set_seg_register(emu, emu->x86.R_DS_SEL, r.ds);
        x86emu_set_seg_register(emu, emu->x86.R_ES_SEL, r.es);
        emu->x86.R_FLG = r.flags;
    }
    if (ret == 2) { x86emu_stop(emu); return 1; }
    return ret;
}

x86emu_t *emu_create(void *user) {
    x86emu_t *emu = x86emu_new(
        X86EMU_PERM_RWX | X86EMU_PERM_VALID,
        X86EMU_PERM_RW | X86EMU_PERM_VALID);
    if (!emu) return NULL;
    emu->private = user;
    g_default_memio = x86emu_set_memio_handler(emu, shim_memio);
    x86emu_set_intr_handler(emu, shim_intr);
    return emu;
}

static void wb(x86emu_t *emu, unsigned lin, unsigned v) { x86emu_write_byte(emu, lin, v); }

// COM イメージ + PSP/MCB/環境/コマンドライン'#'/呼び出しスタブを配置する。
void emu_setup(x86emu_t *emu, const uint8_t *img, uint32_t n) {
    for (uint32_t i = 0; i < n; i++) wb(emu, (LOADSEG << 4) + 0x100 + i, img[i]);
    // MCB(PSP-1段): 所有メモリ段数を大きく見せる
    unsigned mcb = LOADSEG - 1;
    wb(emu, mcb << 4, 0x4D); wb(emu, (mcb << 4) + 1, LOADSEG & 0xff); wb(emu, (mcb << 4) + 2, LOADSEG >> 8);
    wb(emu, (mcb << 4) + 3, 0x00); wb(emu, (mcb << 4) + 4, 0x90);
    // PSP
    wb(emu, LOADSEG << 4, 0xCD); wb(emu, (LOADSEG << 4) + 1, 0x20);
    wb(emu, (LOADSEG << 4) + 0x2C, ENVSEG & 0xff); wb(emu, (LOADSEG << 4) + 0x2D, ENVSEG >> 8);
    wb(emu, ENVSEG << 4, 0); wb(emu, (ENVSEG << 4) + 1, 0); // 空環境
    // コマンドライン "#"(ウイルスチェック skip)
    wb(emu, (LOADSEG << 4) + 0x80, 1); wb(emu, (LOADSEG << 4) + 0x81, 0x23); wb(emu, (LOADSEG << 4) + 0x82, 0x0D);
    // 呼び出しスタブ: int 60h; hlt
    wb(emu, STUBSEG << 4, 0xCD); wb(emu, (STUBSEG << 4) + 1, 0x60); wb(emu, (STUBSEG << 4) + 2, 0xF4);
}

// install 実行(CS:IP=LOADSEG:0100 から)。TSR/exit で rust_intr が停止させる。
void emu_run_install(x86emu_t *emu) {
    x86emu_set_seg_register(emu, emu->x86.R_CS_SEL, LOADSEG);
    x86emu_set_seg_register(emu, emu->x86.R_SS_SEL, LOADSEG);
    x86emu_set_seg_register(emu, emu->x86.R_DS_SEL, LOADSEG);
    x86emu_set_seg_register(emu, emu->x86.R_ES_SEL, LOADSEG);
    emu->x86.R_IP = 0x100; emu->x86.R_SP = 0xFFFE;
    emu->max_instr = 30000000;
    x86emu_run(emu, X86EMU_RUN_MAX_INSTR);
}

// INT 60h を1回呼ぶ。結果の DS:DX を out_ds/out_dx へ返す(AH=06h 等で使用)。
void emu_call60(x86emu_t *emu, uint8_t ah, uint8_t al, uint16_t dx, uint16_t *out_ds, uint16_t *out_dx) {
    emu->x86.mode &= ~0x80u; // HALTED 解除
    x86emu_set_seg_register(emu, emu->x86.R_CS_SEL, STUBSEG);
    x86emu_set_seg_register(emu, emu->x86.R_SS_SEL, LOADSEG);
    x86emu_set_seg_register(emu, emu->x86.R_DS_SEL, LOADSEG);
    x86emu_set_seg_register(emu, emu->x86.R_ES_SEL, LOADSEG);
    emu->x86.R_IP = 0; emu->x86.R_SP = 0xFFF0;
    emu->x86.R_AH = ah; emu->x86.R_AL = al; emu->x86.R_DX = dx;
    emu->max_instr = 5000000;
    x86emu_run(emu, X86EMU_RUN_MAX_INSTR);
    if (out_ds) *out_ds = emu->x86.R_DS;
    if (out_dx) *out_dx = emu->x86.R_DX;
}

// seg:off へ任意バイト列を書く(曲データのロード等)。
void emu_load_mem(x86emu_t *emu, uint16_t seg, uint16_t off, const uint8_t *data, uint32_t n) {
    for (uint32_t i = 0; i < n; i++) wb(emu, (seg << 4) + ((off + i) & 0xffff), data[i]);
}

// install 後の IVT を走査し、PMD セグメント(LOADSEG)を指すベクタのうち INT60h(0x60)以外
// = OPNA タイマ割り込み(opnint)のベクタ番号を返す。見つからなければ -1。
int emu_find_timer_vec(x86emu_t *emu) {
    for (int v = 0; v < 256; v++) {
        unsigned seg = x86emu_read_byte(emu, v * 4 + 2) | (x86emu_read_byte(emu, v * 4 + 3) << 8);
        if (seg == LOADSEG && v != 0x60) return v;
    }
    return -1;
}

// 任意の割り込みベクタを1回呼ぶ(int vec; hlt)。IF=1 で割り込みを通す。
// タイマ ISR(opnint→FM_Timer_main)駆動に使う。
void emu_call_vec(x86emu_t *emu, uint8_t vec, uint8_t ah, uint8_t al, uint16_t dx,
                  uint16_t *out_ds, uint16_t *out_dx) {
    emu->x86.mode &= ~0x80u;
    wb(emu, STUBSEG << 4, 0xCD); wb(emu, (STUBSEG << 4) + 1, vec); wb(emu, (STUBSEG << 4) + 2, 0xF4);
    x86emu_set_seg_register(emu, emu->x86.R_CS_SEL, STUBSEG);
    x86emu_set_seg_register(emu, emu->x86.R_SS_SEL, LOADSEG);
    x86emu_set_seg_register(emu, emu->x86.R_DS_SEL, LOADSEG);
    x86emu_set_seg_register(emu, emu->x86.R_ES_SEL, LOADSEG);
    emu->x86.R_IP = 0; emu->x86.R_SP = 0xFFF0;
    emu->x86.R_AH = ah; emu->x86.R_AL = al; emu->x86.R_DX = dx;
    emu->x86.R_FLG |= 0x200; // IF=1
    emu->max_instr = 5000000;
    x86emu_run(emu, X86EMU_RUN_MAX_INSTR);
    if (out_ds) *out_ds = emu->x86.R_DS;
    if (out_dx) *out_dx = emu->x86.R_DX;
}

void emu_done(x86emu_t *emu) { x86emu_done(emu); }
