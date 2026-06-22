#include <stdio.h>
#include <stdint.h>
#include <stdlib.h>
#include <string.h>
#include "x86emu.h"
#define LOADSEG 0x1000
#define STUBSEG 0x0E00
static x86emu_memio_handler_t def_memio;
static uint8_t g_latch188=0, g_status=0; static int g_tsr=0; static long g_w=0; static int g_dump=0;
static int g_timervec=-1;

static unsigned memio(x86emu_t *emu, u32 addr, u32 *val, unsigned type){
  unsigned op=type&0xff00;
  if(op==X86EMU_MEMIO_O){ uint8_t v=*val;
    if(addr==0x188)g_latch188=v;
    if(addr==0x18A && g_latch188==0x27) g_status=0; // Timer Reset(0x27書込)でフラグclear
    if(g_dump && (addr==0x188||addr==0x18A||addr==0x18C||addr==0x18E)){ g_w++;
      printf("%03X<%02X ",addr,v); if(g_w%8==0)printf("\n"); }
    return 0; }
  if(op==X86EMU_MEMIO_I){ uint8_t r=0xFF;
    switch(addr){ case 0x18A: r=(g_latch188==0xFF)?0x01:0x00; break;
      case 0x18C: case 0x18E: r=0x00; break; case 0x188: r=g_status; break;
      case 0x08A: r=0xFF; break; case 0xA460: r=0xFF; break; default: r=0xFF; }
    *val=r; return 0; }
  return def_memio(emu,addr,val,type);
}
static int intr(x86emu_t *emu, u8 num, unsigned type){
  if(num==g_timervec) return 0;   // opnint へ(IVT経由)
  if(num==0x60) return 0;
  if(num==0x21){ unsigned ah=emu->x86.R_AH;
    switch(ah){ case 0x30: emu->x86.R_AX=5; break; case 0x52: emu->x86.R_ES=0x50; emu->x86.R_BX=0; break;
      case 0x51: case 0x62: emu->x86.R_BX=LOADSEG; break; case 0x25: break; case 0x35: emu->x86.R_BX=0;emu->x86.R_ES=0; break;
      case 0x49: break; case 0x48: emu->x86.R_AX=0x9000; break; case 0x4A: break; case 0x09: case 0x02: break;
      case 0x31: g_tsr=1; x86emu_stop(emu); break; case 0x4C: g_tsr=2; x86emu_stop(emu); break;
      default: emu->x86.R_FLG|=1; return 1; }
    emu->x86.R_FLG&=~1; return 1; }
  return 1;
}
static void wb(x86emu_t*e,unsigned l,unsigned v){x86emu_write_byte(e,l,v);}
static void call_vec(x86emu_t*emu,unsigned vec,unsigned ah,unsigned al,unsigned dx){
  emu->x86.mode&=~0x80u;
  wb(emu,(STUBSEG<<4),0xCD); wb(emu,(STUBSEG<<4)+1,vec); wb(emu,(STUBSEG<<4)+2,0xF4);
  x86emu_set_seg_register(emu,emu->x86.R_CS_SEL,STUBSEG);
  x86emu_set_seg_register(emu,emu->x86.R_SS_SEL,LOADSEG);
  x86emu_set_seg_register(emu,emu->x86.R_DS_SEL,LOADSEG);
  x86emu_set_seg_register(emu,emu->x86.R_ES_SEL,LOADSEG);
  emu->x86.R_IP=0; emu->x86.R_SP=0xFFF0; emu->x86.R_AH=ah; emu->x86.R_AL=al; emu->x86.R_DX=dx;
  emu->x86.R_FLG|=0x200; // IF=1
  emu->max_instr=5000000; x86emu_run(emu,X86EMU_RUN_MAX_INSTR);
}
int main(int argc,char**argv){
  const char*path=argv[1]; const char*song=argc>2?argv[2]:0;
  FILE*f=fopen(path,"rb"); static uint8_t img[0x10000]; size_t n=fread(img,1,sizeof img,f); fclose(f);
  x86emu_t*emu=x86emu_new(X86EMU_PERM_RWX|X86EMU_PERM_VALID,X86EMU_PERM_RW|X86EMU_PERM_VALID);
  def_memio=x86emu_set_memio_handler(emu,memio); x86emu_set_intr_handler(emu,intr);
  for(size_t i=0;i<n;i++) wb(emu,(LOADSEG<<4)+0x100+i,img[i]);
  unsigned mcb=LOADSEG-1; wb(emu,mcb<<4,0x4D); wb(emu,(mcb<<4)+1,0); wb(emu,(mcb<<4)+2,0x10); wb(emu,(mcb<<4)+3,0); wb(emu,(mcb<<4)+4,0x90);
  wb(emu,LOADSEG<<4,0xCD); wb(emu,(LOADSEG<<4)+1,0x20);
  wb(emu,(LOADSEG<<4)+0x2C,0); wb(emu,(LOADSEG<<4)+0x2D,0x0F); wb(emu,0x0F000,0); wb(emu,0x0F001,0);
  wb(emu,(LOADSEG<<4)+0x80,1); wb(emu,(LOADSEG<<4)+0x81,0x23); wb(emu,(LOADSEG<<4)+0x82,0x0D);
  x86emu_set_seg_register(emu,emu->x86.R_CS_SEL,LOADSEG); x86emu_set_seg_register(emu,emu->x86.R_SS_SEL,LOADSEG);
  x86emu_set_seg_register(emu,emu->x86.R_DS_SEL,LOADSEG); x86emu_set_seg_register(emu,emu->x86.R_ES_SEL,LOADSEG);
  emu->x86.R_IP=0x100; emu->x86.R_SP=0xFFFE; emu->max_instr=30000000; x86emu_run(emu,X86EMU_RUN_MAX_INSTR);
  printf("[install tsr=%d]\n",g_tsr); if(g_tsr!=1) return 1;
  // IVT走査: LOADSEGを指すベクタ
  printf("IVT vectors -> seg %04X:",LOADSEG);
  for(int v=0;v<256;v++){ unsigned seg=x86emu_read_byte(emu,v*4+2)|(x86emu_read_byte(emu,v*4+3)<<8);
    if(seg==LOADSEG){ printf(" %02X",v); if(v!=0x60 && g_timervec<0) g_timervec=v; } }
  printf("\n=> timer vector = %02X\n",g_timervec);
  if(!song) return 0;
  FILE*s=fopen(song,"rb"); static uint8_t md[0x10000]; size_t mn=fread(md,1,sizeof md,s); fclose(s);
  uint16_t mseg,moff; call_vec(emu,0x60,0x06,0,0); mseg=emu->x86.R_DS; moff=emu->x86.R_DX;
  for(size_t i=0;i<mn;i++) wb(emu,(mseg<<4)+((moff+i)&0xffff),md[i]);
  printf("=== AH=00h MUSIC START (timer regs?) ===\n"); g_dump=1; g_w=0; call_vec(emu,0x60,0x00,0,0); g_dump=0;
  printf("\n[MUSIC_START writes=%ld]\n",g_w);
  // tick 駆動: TimerB フラグ立てて opnint ベクタ呼ぶ
  printf("=== tick0 via int %02X (status=TimerB) ===\n",g_timervec);
  g_dump=1; g_w=0; g_status=0x02; call_vec(emu,g_timervec,0,0,0);
  printf("\n[tick0 writes=%ld, status after=%02X]\n",g_w,g_status);
  return 0;
}
