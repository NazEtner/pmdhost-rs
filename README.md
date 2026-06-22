# pmdhost-rs

PMD の演奏データを自作 YMF288(OPN3-L)ボードで鳴らすためのホスト。

8086 エミュレータ（[libx86emu](https://github.com/wfeldt/libx86emu) を同梱）上で x86 コードを実行し、
OPNA ポート（188h/18Ah/18Ch/18Eh）への `out` を捕獲して `PacketSend` に変換し、
名前付きパイプ `\\.\pipe\OPN3LD`（`YML288BoardDriver` の `driver.exe` がサーバ）へ送る。

- **M2（現状）**: 手書きの小プログラム（既知の良 FM 1ch 発音列）を実行し、エミュ→ボードの経路を実証する骨格。
- M3 以降: 本家 PMD（`PMD.ASM`）をエミュに載せ、`.M` を直接再生する。

## ビルド（WSL2 から Windows 向けにクロスビルド）

実行は Windows 上だが、ビルドは WSL2(Linux)で行う。MinGW を Windows 側に入れずに済む。

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

## 実行

1. Windows で `YML288BoardDriver`（`driver.exe`）を起動し、COM ポートを選択。
2. `pmdhost-rs.exe` を実行 → テスト音が鳴れば配線 OK。

## ライセンス

`vendor/libx86emu/` は libx86emu のソース同梱。ライセンスは同ディレクトリの `LICENSE` を参照
（SciTech/SUSE 由来の permissive ライセンス）。
