# 通信プロトコル

pmdhost-rs は 3 つの境界をまたいで動く。

`PMD本体 (8086 COM)` ⇄ **境界A** ⇄ `pmdhost-rs (host)` → **境界B** → `driver.exe`
（host へは別途 **境界C** からユーザ操作が入る）

- **境界A**（PMD本体 ↔ host）… 本物の PMD を 8086 エミュレータ（[libx86emu](https://github.com/wfeldt/libx86emu) 同梱）上で実行し、PMD が叩く I/O / 割り込み / DOS 呼び出しをホストが捕獲・代行する。プロセス内の FFI 境界（[csrc/shim.c](../csrc/shim.c) ↔ [src/board.rs](../src/board.rs) / [src/emu.rs](../src/emu.rs)）。双方向。
- **境界B**（host → driver.exe）… 捕獲した OPNA レジスタ書き込みを 4 バイトの `PacketSend` に変換し、名前付きパイプ `\\.\pipe\OPN3LD` 経由で `driver.exe`（`YML288BoardDriver`、別リポジトリ）へ送って実機 YMF288 を鳴らす。基本は一方向（`SIZE_REQUEST` のみ応答あり）。
- **境界C**（ユーザ → host）… 演奏操作のための制御インタフェース（stdin プロンプト / TCP）。本書末尾に概要のみ記す。

---

## 境界A: PMD本体 ↔ pmdhost-rs（エミュI/Oトラップ）

ホストは PMD を「PC-98 + OPNA が在る DOS 環境」だと錯覚させて install させ、その後タイマ割り込みを 1 tick ずつ手動で叩いて演奏を進める。PMD ↔ ホストのやり取りは次の 4 系統。

### A-1. メモリ配置（install のお膳立て）

[csrc/shim.c](../csrc/shim.c) `emu_setup` が COM 実行に必要な最小の DOS 環境を構築する。

| 配置先 | 内容 | 目的 |
| --- | --- | --- |
| `LOADSEG:0100`（`0x1000:0100`）| `assets/pmdymf.bin`（フラット COM イメージ）| PMD 本体。`CS=DS=ES=SS=LOADSEG`, `IP=0100`, `SP=0xFFFE` で実行 |
| `LOADSEG-1`（MCB）| offset3 に所有段数 `0x9000` | メモリチェック通過 |
| `LOADSEG:0000`（PSP）| `CD 20`、offset `2C` に環境セグメント | DOS プロセス偽装 |
| `ENVSEG`（`0x0F00`）| `00 00`（空環境）| `PMDOPT` 検索が空振りして正常終了 |
| `LOADSEG:0080`（cmdline）| 長さ1 + `'#'` | 起動時のウイルス（改竄）チェックを **スキップ**（KAJA 氏が VIRUS.INC に用意したバイパス）|
| `STUBSEG`（`0x0E00`）| `CD <vec> F4`（`int <vec>; hlt`）| ホストから INT を呼ぶための踏み台 |

詳細は [vendor/pmd/BUILD.md](../vendor/pmd/BUILD.md) も参照。

### A-2. I/O ポート（OPNA） — PMD → ホスト

PMD が OPNA を叩く `out`/`in` を [src/board.rs](../src/board.rs) の `out()` / `in_port()` が捕獲する。ポートは PC-98 の OPNA(86 音源)配置。

**書き込み（`out`）** — アドレスラッチ → データの 2 段書き込み：

| ポート | 役割 |
| --- | --- |
| `0x188` | 表バンク アドレスラッチ（reg 番号）|
| `0x18A` | 表バンク データ（FM1-3 / SSG / リズム / Timer）|
| `0x18C` | 裏バンク アドレスラッチ |
| `0x18E` | 裏バンク データ（FM4-6 等）|

データ書き込みごとに `emit(bank, reg, data)` が走り、後述の `PacketSend`（レジスタ書き込み）として境界Bへ流れる。`reg 0x24-0x27`（Timer A/B 設定・制御）は同時に [src/opna.rs](../src/opna.rs) の仮想タイマモデルへも反映され、「次にどちらの割り込みが発火するか」の判定に使われる。

**読み込み（`in`）** — install 時のボード検出とステータス読みに応答：

| ポート | 応答 | 意味 |
| --- | --- | --- |
| `0x188` | OPNA ステータス（bit0=TimerA, bit1=TimerB）| 仮想タイマモデルが生成 |
| `0x18A` | latch=0xFF→`0x01` / latch∈{0x0E,0x0F}→`0xFF` / latch<0x10→SSGシャドウ | OPNA 在り判定 + SSG ジョイポート（不在）|
| `0x08A` | `0xFF` | base `0x088` の OPNA は**不在** |
| `0xA460` | `0xFF` | 音源種別検出への応答 |

この応答で PMD は「base 0x188 に YMF288 が在る」と認識する。

### A-3. DOS サービス（INT 21h） — PMD → ホスト

install を通すための最小 DOS スタブ。[src/board.rs](../src/board.rs) `dos()` が AH で分岐し、戻り値で「処理済み（1）/ 処理済み+エミュ停止（2）」を返す。

| AH | サービス | ホストの応答 |
| --- | --- | --- |
| `0x30` | get DOS version | `AX=0x0005`（ver 5）|
| `0x52` | get SysVars | `ES:BX` にダミー |
| `0x51`/`0x62` | get PSP | `BX=LOADSEG` |
| `0x25` | set int vector | no-op（IVT は emu が保持）|
| `0x35` | get int vector | `BX=0, ES=0` |
| `0x48` | allocate memory | `AX=0x9000`（大きめ）|
| `0x49`/`0x4A` | free / resize | no-op 成功 |
| `0x09`/`0x02` | 文字列 / 1文字出力 | 後述のバナー表示へ |
| `0x31` | **TSR（常駐終了）** | `installed=true` にして**停止** ← install 成功点 |
| `0x4C` | exit | `exited=true` にして停止 |
| その他 | — | CF=1（未対応）|

`INT 2F`（マルチプレクサ）と `INT 60h` 自身は no-op（処理済み）として通す。

**バナー出力**: PMD が install 時に出す起動メッセージ（Shift-JIS）は [csrc/shim.c](../csrc/shim.c) `shim_intr` が INT 21h AH=09h/02h を横取りして取り出し、[src/board.rs](../src/board.rs) `rust_dos_print` が cp932→UTF-8 変換のうえ端末へ（明るい緑で）表示する。

### A-4. PMD API（INT 60h）+ タイマ割り込み — ホスト → PMD

ホストが PMD を能動的に動かす方向。`STUBSEG` の `int <vec>; hlt` を書き換えて任意ベクタを 1 回呼ぶ（[csrc/shim.c](../csrc/shim.c) `emu_call60` / `emu_call_vec`）。

**INT 60h（PMD コマンド、AH で指定）** — [src/main.rs](../src/main.rs) / [src/emu.rs](../src/emu.rs) が使用するもの：

| AH | コマンド | 用途 |
| --- | --- | --- |
| `0x00` | MUSIC_START | 演奏開始（NA/NB 確定）|
| `0x01` | MUSIC_STOP | 全ch キーオフ（停止 / 曲切替時の被り防止）|
| `0x02` | FADEOUT(速度) | フェードアウト |
| `0x05` | GET_SYOUSETU | 小節カウンタ（状態監視）|
| `0x06` | GET_MUSDAT_ADR | 曲データのロード先 `seg:off` を取得 |
| `0x0A` | GET_STATUS | `AH=ST1, AL=ST2`（ST2=ループ回数, 0xFF=曲終了）|

**曲データのロード**: `GET_MUSDAT_ADR` で得た番地へ、`.M` のバイト列を `emu_load_mem` で直接書き込む。

**タイマ割り込み駆動（演奏の心臓部）**: install 後、IVT を走査して PMD セグメントを指す INT60h 以外のベクタ＝OPNA タイマ割り込み（`opnint`）を特定する（`emu_find_timer_vec`）。演奏中はホストの仮想タイマモデル（[src/opna.rs](../src/opna.rs)）が「次に発火するのは Timer A か B か」を決め、`call_vec(timer_vec)` でその ISR（`opnint → FM_Timer_main`）を 1 回実行する。1 回の ISR 実行 = 1 tick = 1 バッチで、その間に PMD が出す OPNA 書き込みが境界Bの 1 バッファ分になる。

> **重要**: ホストの仮想タイマは割り込みの**順序と比率**だけを再現する（同時刻なら B→A、PMD `FM_Timer_main` と同順）。**実時間のテンポはボード側の実 YMF288 タイマが律速する**（board-paced）。ホストは絶対周期を持たない。

---

## 境界B: pmdhost-rs ↔ driver.exe（名前付きパイプ）

実機運用の出力経路。`driver.exe`（`YML288BoardDriver`）が名前付きパイプ **`\\.\pipe\OPN3LD`** のサーバ、pmdhost-rs がクライアント（[src/pipe.rs](../src/pipe.rs)）。`PMDHOST_DRY` 指定時はパイプの代わりに統計ダンプへ差し替わる。

### B-1. パケット形式（4 バイト固定）

[src/packet.rs](../src/packet.rs) `PacketSend`。`driver.hpp` の `PacketSend` と完全一致。

```
byte0: type       (下記ビットの OR)
byte1: reg_address (OPNA レジスタ番号。制御パケットでは 0)
byte2: data        (書き込む値。制御パケットでは 0)
byte3: end_sym     (0xAA 固定。フレーム同期用)
```

### B-2. type ビット（MSB first）

| ビット | 定数 | 意味 |
| --- | --- | --- |
| `0x80` | `INT_END` | バッチ終端。`INT_SELECT` 側のバッファを 1 tick 分として確定 |
| `0x40` | `INT_SELECT_A` | 1 = Timer A バッファ（SSGドラム/効果音）/ 0 = Timer B バッファ（音楽）|
| `0x20` | `FORCE_TIMEOUT` | 強制ドレイン（溜めずに即適用）|
| `0x10` | `FLUSH` | リセット（送信キュー + ボードのバッファ/orderキュー/pendingTimers をクリア）|
| `0x08` | `SIZE_REQUEST` | 送信キュー長の問い合わせ（応答あり、後述）|
| `0x01` | `BANK_SELECT` | 1 = 裏バンク（18Ch/18Eh）/ 0 = 表バンク（188h/18Ah）|

### B-3. パケットの種類と使い分け

| 送るもの | type | 生成箇所 |
| --- | --- | --- |
| レジスタ書き込み | `BANK_SELECT? | INT_SELECT_A?`（現イベントのバッファ）| `Host::emit` |
| バッチ終端 | `INT_END | <現バッファ>` | `Host::end_batch` |
| バッチ終端 + 強制ドレイン | `INT_END | <現バッファ>` の後に `FORCE_TIMEOUT` | `Host::flush_drain`（install / MUSIC_START / 最初の tick）|
| リセット | `FLUSH` | `Host::flush_reset`（再生開始前・曲切替・stop）|
| キュー長問い合わせ | `SIZE_REQUEST` | `Host::queue_size`（背圧用）|

### B-4. board-paced モデルと背圧

- PMD のタイマ割り込みを 1 tick = 1 バッチとして、対応するボードのバッファ（**A** = Timer A / SSGドラム、**B** = Timer B / 音楽）へ `INT_END` 付きで積むだけ。
- **テンポを律速するのはボードの実 YMF288 タイマ /IRQ**。ホストは毎イベントごとに往復せず、先回りして積む（処理落ち対策）。
- 先回りしすぎないよう、ホストは `SIZE_REQUEST` で `driver.exe` の送信キュー長を取り、上限（`PMDHOST_QUEUE`、既定 450 ≒ 0.3 秒）を超えたら駆動を一時停止して背圧をかける。

### B-5. SIZE_REQUEST の応答（唯一の戻り方向）

境界Bは基本的にホスト → driver の一方向だが、`SIZE_REQUEST` だけは応答がある：

```
host  → driver : [0x08, 0x00, 0x00, 0xAA]   (SIZE_REQUEST)
host  ← driver : 4 バイト = u32 (little-endian) 現在の送信キュー長
```

[src/pipe.rs](../src/pipe.rs) `query_size` が送信直後に 4 バイトを `read_exact` して `u32::from_le_bytes` で読む。

### B-6. ドライラン（PMDHOST_DRY）

実機の代わりに [src/pipe.rs](../src/pipe.rs) `BatchStats` がパケットを解釈し、`INT_END` 間の書き込み数（バッチサイズ）をバッファ A/B 別に集計して百分位を表示する。レジスタ書き込みの内容そのものも直近 80 件を保持し、停止直前のダンプに使う。実機・パイプには一切繋がない。

---

## 境界C: 制御インタフェース（stdin / TCP）

演奏操作のための入力。[src/main.rs](../src/main.rs) が stdin プロンプトと制御 TCP の両方を受け、`mpsc` で唯一の演奏ループへ送る（emu に触れるのはそのループのみ）。行ベースのテキストコマンド：

```
play <file> | stop | fade <n> | status | quit
```

| 経路 | 待受 | 備考 |
| --- | --- | --- |
| stdin | `pmd>` プロンプト | `play` のパスは無制限（ローカル操作）|
| TCP | 既定 `127.0.0.1:5288`（`PMDHOST_BIND` で変更）| `quit` は接続切断のみ（ホストは落とさない）|

**セキュリティ上の既定**:
- TCP は **localhost のみ**待受。外部公開は `PMDHOST_BIND` での明示的 opt-in。
- TCP（リモート）からの `play` は `PMDHOST_MUSICDIR` 配下に限定（未設定なら拒否）。`canonicalize` + プレフィックス検査でパストラバーサルを防ぐ。
