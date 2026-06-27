import sys,struct
data=open(sys.argv[1],'rb').read()
SEG=bytearray(0x10000); seg_len=0; maxoff=0
fixups=[]  # (loc_abs, mode_seg_rel, target_disp)
frame_thread={}; target_thread={}

def recs(d):
    i=0
    while i<len(d):
        rt=d[i]; ln=d[i+1]|(d[i+2]<<8); body=d[i+3:i+3+ln-1]; i+=3+ln
        yield rt,body

def idx(b,p):  # OMF index (1 or 2 bytes)
    if b[p]&0x80: return ((b[p]&0x7f)<<8)|b[p+1],p+2
    return b[p],p+1

last_ledata=(0,0)
for rt,b in recs(data):
    if rt==0x98: # SEGDEF
        acbp=b[0]; p=1
        if (acbp>>2)&7==0:  # absolute -> frame+offset (skip)
            p+=3
        seg_len=b[p]|(b[p+1]<<8); 
        if acbp&2: seg_len=0x10000
    elif rt==0xA0: # LEDATA
        si,p=idx(b,0); off=b[p]|(b[p+1]<<8); p+=2; dat=b[p:]
        SEG[off:off+len(dat)]=dat; maxoff=max(maxoff,off+len(dat))
        last_ledata=(off,len(dat))
    elif rt==0xA2: # LIDATA
        si,p=idx(b,0); off=b[p]|(b[p+1]<<8); p+=2
        def expand(b,p):
            rep=b[p]|(b[p+1]<<8); blk=b[p+2]|(b[p+3]<<8); p+=4
            out=b''
            if blk==0:
                ln=b[p]; p+=1; chunk=bytes(b[p:p+ln]); p+=ln
                out=chunk*rep
            else:
                seq=b''
                for _ in range(blk):
                    s,p=expand(b,p); seq+=s
                out=seq*rep
            return out,p
        out,_=expand(b,p)
        SEG[off:off+len(out)]=out; maxoff=max(maxoff,off+len(out))
        last_ledata=(off,len(out))
    elif rt==0x9C: # FIXUPP
        p=0
        while p<len(b):
            if b[p]&0x80==0:  # THREAD
                t=b[p]; p+=1; method=(t>>2)&7; tn=t&3
                if t&0x40: # frame thread
                    if method<3: _,p=idx(b,p) if method in(0,1,2) else (0,p)
                else:
                    _,p=idx(b,p) if method in(0,1,2) else (0,p)
            else: # FIXUP
                loc=((b[p]&0x03)<<8)|b[p+1]; locType=(b[p]>>2)&0xf; M=(b[p]&0x40)!=0; p+=2
                fd=b[p]; p+=1
                F=(fd&0x80)!=0; frame=(fd>>4)&7; T=(fd&0x08)!=0; P=(fd&0x04)!=0; targt=fd&3
                if not F and frame in(0,1,2): _,p=idx(b,p)
                if not T and (targt&3) in(0,1,2): tdat,p=idx(b,p)
                tdisp=0
                if not P: tdisp=b[p]|(b[p+1]<<8); p+=2
                loc_abs=last_ledata[0]+loc
                fixups.append((loc_abs,M,locType,tdisp))

# apply fixups: 16bit offset, segment-relative -> write target displacement
applied=0;skipped=[]
for loc,M,lt,tdisp in fixups:
    if lt in (1,5):  # 16-bit offset
        # OMF: 最終値 = 既存データ(addend) + ターゲット変位。addend を足し忘れると
        # `offset rdat-1` 等の負の addend が消えて off-by-one になる(リズム個別音量バグの原因)。
        cur=SEG[loc]|(SEG[loc+1]<<8)
        val=(cur+tdisp)&0xffff
        SEG[loc]=val&0xff; SEG[loc+1]=(val>>8)&0xff; applied+=1
    else:
        skipped.append((loc,lt,M))
com=bytes(SEG[0x100:maxoff])
open(sys.argv[2],'wb').write(com)
print(f"seg_len={seg_len} maxoff=0x{maxoff:x} COM size={len(com)} fixups={len(fixups)} applied={applied} skipped={skipped[:8]}")
print("COM head:", com[:8].hex())
# 'PMD' 署名(int60_head: jmp short; db 'PMD'): entryは jmp comstart(E9)。+? 署名探し
print("contains 'PMD':", b'PMD' in com, " 'PMDYMF':", b'PMDYMF' in com)
