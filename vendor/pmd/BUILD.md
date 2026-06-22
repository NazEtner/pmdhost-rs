# PMD (YMF288 向け) ビルド手順

原典 PMD ソース(d2lmirrors/pmd、作者により自由利用可)を YMF288(OPN3-L)向けに
アセンブルし、エミュレータに読み込ませるフラットな COM イメージ `assets/pmdymf.bin` を作る。

## このディレクトリの変更点(原典との差分)

1. **`PMDYMF.ASM`**(新規ラッパ): `board2=1, adpcm=0`(FM6+SSG3+リズム6、ADPCM-B 無し = YMF288)。
2. **`PMD.ASM`**: `include efcdrv.asm` の直前に `pcmmain` / `pcm_effect` の no-op スタブを追加
   (board2 単独だと PCM ドライバ未 include でこの 2 シンボルが未定義になるため。リズムは別系統
   `rhythmmain` なので無事)。それ以外は原典のまま。

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
