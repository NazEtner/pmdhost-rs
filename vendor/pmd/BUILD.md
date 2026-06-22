# PMD (YMF288 向け) ビルド手順

原典 PMD ソース(d2lmirrors/pmd、作者により自由利用可)を YMF288(OPN3-L)向けに
アセンブルし、エミュレータに読み込ませるフラットな COM イメージ `assets/pmdymf.bin` を作る。

## このディレクトリの変更点(原典との差分)

1. **`PMDYMF.ASM`**(新規ラッパ): `board2=1, adpcm=0`(FM6+SSG3+リズム6、ADPCM-B 無し = YMF288)。
2. **`PMD.ASM` パッチ 2 箇所**:
   - `include efcdrv.asm` の直前に `pcmmain` / `pcm_effect` の no-op スタブを追加
     (board2 単独だと PCM ドライバ未 include でこの 2 シンボルが未定義になるため。リズムは
     別系統 `rhythmmain` なので無事)。
   - `wait_check:` の `include wait.inc` を `mov ax,1 / mov cx,1 / mov bx,1 / ret` に置換
     (wait.inc は PC-98 ハードタイマ割り込みで CPU 速度を較正するが、エミュではタイマが発火せず
     無限ループする。wait_clock は OPNA バス待ちのビジーループ長で、実バスタイミングはボードが
     担当するためエミュでは不要 → 固定値で良い)。
   それ以外は原典のまま。

## 実行に必要なエミュ側のお膳立て(install を通すため)

`tools/pmdrun.c`(C 観測ハーネス)で検証済み。install 成功に必要なもの:
- **コマンドライン `#`**(PSP:80 に長さ 1, '#')→ 起動時のウイルスチェック(改竄検出)をスキップ
  (KAJA 氏が VIRUS.INC に用意したビルトインのバイパス。実 DOS の FCB/SDA が無い環境向け)。
- **MCB**(PSP-1 段)offset 3 に大きめの所有段数(例 0x9000)→ メモリチェック通過。
- **空の環境ブロック**(PSP:2C が指す先に 00 00)→ PMDOPT 検索が何もヒットせず正常終了。
- **DOS INT 21h スタブ**: 30h(ver=5),52h(SysVars ダミー),09h/02h(print),49h(free),51h(PSP),
  31h(TSR=install 完了),4Ch(exit)。INT 2F は noop。
- **I/O 検出応答**: base 0x188 で OPNA 在り(0x188 に 0xFF 書込後 0x18A→0x01、0x18C/0x18E→非0xFF)、
  base 0x088 は 0xFF(不在)、0xA460→0xFF。
- ロード: seg:0100 に COM イメージ、CS=DS=ES=SS=seg、IP=0100、SP=0xFFFE。
- 成功すると INT 21h AH=31h(TSR)に到達し、INT 60h ハンドラが常駐する。

## アセンブル

JWasm/UASM 等のフリーアセンブラは PMD の MASM 流儀(前方参照の構造体メンバ等)を扱えない。
**MASM (`ml.exe`, VS 同梱) を使う**。`/omf` で OMF を出力(COFF ではなく)。

```
ml /c /omf /Zm /Fo pmdymf_omf.obj PMDYMF.ASM
```

単一セグメント・tiny・org 100h・外部参照なしの OMF が出る。

## COM 化(OMF → フラットバイナリ)

16bit リンカの代わりに同梱の変換器を使う(単一セグメント前提の最小 OMF→COM)。

```
python ../../tools/omf2com.py pmdymf_omf.obj ../../assets/pmdymf.bin
```

検証: 先頭が `E9 .. ..`(jmp comstart)→ `EB 11`(int60_head)→ `50 4D 44`('PMD' 署名)、
'PMDYMF' 文字列を含む、サイズ約 22KB。

このイメージは org 100h 基準のフラット COM。エミュレータでは seg:0100 にロードし CS:IP=seg:0100 で実行する。
