# pmdhost-rs

PMD の演奏データ（`.M`）を自作 YMF288(OPN3-L)ボードで鳴らすためのホスト。

本物の PMD（KAJA 氏の `PMD.ASM`）を 8086 エミュレータ（[libx86emu](https://github.com/wfeldt/libx86emu) を同梱）上で実行し、
OPNA ポート（188h/18Ah/18Ch/18Eh）への `out` を捕獲して `PacketSend` に変換、
名前付きパイプ `\\.\pipe\OPN3LD`（`YML288BoardDriver` の `driver.exe` がサーバ）へ送ってボードを鳴らす。

PMD を YMF288 向けにアセンブルしたフラット COM イメージ `assets/pmdymf.bin` をバイナリに埋め込み、
起動時にエミュへロード → install → 指定された `.M` を再生する。

## 再生モデル（board-paced）

PMD のタイマ割り込み（Timer A=SSG ドラム / Timer B=音楽）を 1 tick = 1 バッチとして、
対応するボードのバッファ（A/B）へ `IntEnd` 付きで積む。**テンポを律速するのはボードの実 YMF288
タイマ /IRQ** で、ホストは毎イベントごとに往復しない（処理落ち対策）。ホストは `driver.exe` の
送信キュー長（`SizeRequest`）で背圧をかけ、先回りしすぎないようにする。詳細は [src/board.rs](src/board.rs)・
[src/main.rs](src/main.rs) を参照。

## ビルド（WSL2 から Windows 向けにクロスビルド）

実行は Windows 上だが、ビルドは WSL2(Linux)で行う。MinGW を Windows 側に入れずに済む。
libx86emu(C) と `csrc/shim.c` を `cc` で同梱コンパイルして静的リンクする（[build.rs](build.rs)）。

```sh
# WSL2 側の準備（初回のみ）
sudo apt install mingw-w64
rustup target add x86_64-pc-windows-gnu

# ビルド（.cargo/config.toml でターゲットは windows-gnu 固定）
cargo build --release
# → target/x86_64-pc-windows-gnu/release/pmdhost-rs.exe
```

生成された `.exe` は Windows ネイティブのプログラム。Windows 上で `driver.exe` を起動してから実行する。
（WSL2 は実行時には不要。`/mnt/c/...` 経由で Windows 側へコピーするか、`target` を Windows から参照する。）

埋め込む `assets/pmdymf.bin` の作り方（PMD ソースのアセンブル手順）は [vendor/pmd/BUILD.md](vendor/pmd/BUILD.md) を参照。

## 実行

1. Windows で `YML288BoardDriver`（`driver.exe`）を起動し、COM ポートを選択。
2. 再生したい曲を引数に渡して `pmdhost-rs.exe` を実行する。

```
pmdhost-rs.exe <song.M>
```

`Ctrl-C` で停止。

### 環境変数

| 変数 | 既定 | 説明 |
| --- | --- | --- |
| `PMDHOST_DRY` | （未設定） | ドライラン。実機（パイプ）に繋がず、レジスタ書き込みをダンプしバッチサイズ統計を出す。 |
| `PMDHOST_QUEUE` | `450` | 先読みキュー長（driver.exe の送信キュー上限）。大きいほど重い区間でバッファ枯れ（カクつき）しにくいが、stop/fade 等の操作レスポンスが遅れる。約 450 ≒ 0.3 秒の先読み。 |
| `PMDHOST_DRYTICKS` | `30000` | ドライラン時に回す tick 数。 |
| `PMDHOST_STATUS` | （未設定） | ドライラン中に演奏状態（ループ回数・小節カウンタ）を定期表示。 |

## ライセンス

- `vendor/libx86emu/` は libx86emu のソース同梱。ライセンスは同ディレクトリの `LICENSE` を参照
  （SciTech/SUSE 由来の permissive ライセンス）。
- `vendor/pmd/` は PMD（KAJA 氏作、作者により自由利用可）のソース同梱。YMF288 向けの変更点は
  [vendor/pmd/BUILD.md](vendor/pmd/BUILD.md) を参照。
