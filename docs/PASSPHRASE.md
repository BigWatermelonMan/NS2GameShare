# GameShare passphrase extraction / パスフレーズの抽出

[日本語](#日本語)

The GameShare passphrase is separate from your Wi-Fi or Nintendo Account password.
Use files from your own console backup. Keys, passphrases and console software are not included.

## GUI

1. Mount your **Switch SYSTEM** backup **read-only** with your backup tool.
   Select the folder containing `Contents/registered/`. An extracted splay Program NCA or its ExeFS `main` NSO also works.
2. In **Connection → Authentication files**, select your `prod.keys`.
   NCA input requires `header_key` and the corresponding `key_area_key_*` in addition to the GameShare connection keys.
   Mounting SYSTEM alone does not decrypt the NCA contents.
3. Click **Extract from SYSTEM / splay...**, then select the SYSTEM folder or NCA/NSO file.
4. Click **Extract & use**. If extraction succeeds, the passphrase path is filled in automatically.
5. Click **Save settings**.

The output is a new 16-byte binary file under `%LOCALAPPDATA%/NS2GameShare/private/`,
or `private/` beside a custom settings file. Its value is not displayed.
Source files and existing outputs are never overwritten. Keep the output file at its saved path;
do not convert it to hexadecimal text or add a newline.

## Supported sources

- Switch **splay**, Program ID `0100000000001048`.
- Build ID: `1bbb97e6b09abb88ab06057739a1d55d89af5af0000000000000000000000000`.
- Complete NCA2/NCA3 files with key-area AES-CTR ExeFS, or NSO0 with uncompressed/LZ4 segments.

Switch 2 SYSTEM, unknown builds, split NCAs, title-rights encryption, patch sections and ZBIC are unsupported.
If multiple splay NCAs are found, select one directly. Unsupported or damaged inputs produce no passphrase file.

## Command line

From the repository root after building:

```powershell
.\client-windows\target\release\NS2GameShare.exe extract-passphrase `
  --source 'G:\SYSTEM' `
  --keys 'C:\private\prod.keys' `
  --output 'C:\private\gameshare-passphrase.bin'
```

Replace the example paths with your own. If `G:\` is the SYSTEM root, use `--source 'G:\'`.
The source may also be a splay Program NCA or ExeFS `main` file.
Choose a new output filename outside the mounted source.

Do not attach backups, keys or passphrases to issues. For an unsupported build, report the Build ID and error text with private paths removed.
See [SECURITY.md](../SECURITY.md).

## 日本語

GameShareのパスフレーズは、Wi-FiやNintendo Accountのパスワードとは別の値です。
自分の本体から取得したバックアップを使用してください。鍵・パスフレーズ・本体のソフトウェアは付属しません。

1. バックアップ用ツールで、自分の**SwitchのSYSTEM**を**読み取り専用**マウントします。
   `Contents/registered/`が見えるフォルダーを選びます。抽出済みのsplay Program NCAやExeFSの`main` NSOも利用できます。
2. 「接続 → 認証ファイル」で自分の`prod.keys`を選びます。
   NCA入力では、接続用の鍵に加えて`header_key`と該当世代の`key_area_key_*`が必要です。
   SYSTEMのマウントだけではNCA自体の復号は完了しません。
3. 「SYSTEM / splay から抽出…」でSYSTEMフォルダーまたはNCA／NSOファイルを選びます。
4. 「抽出して設定に反映」を押します。成功するとパスフレーズ欄に保存先が入ります。
5. 「設定を保存」を押します。

出力は通常`%LOCALAPPDATA%/NS2GameShare/private/`に保存する16バイトのバイナリです。
値は表示せず、元ファイルと既存出力も上書きしません。保存したファイルは移動・削除せず、16進数の文字列への変換や改行の追加もしないでください。

**対応するのは上記Build IDのSwitch版splayのみです。** Switch 2のSYSTEM、未知のビルド、分割NCAなどには対応していません。
対象NCAが複数ある場合は1つを直接選んでください。非対応または破損した入力からは出力しません。

CLIを使う場合は、上の例の`--source`を抽出元、`--keys`を鍵ファイル、`--output`を新しい非公開ファイル名に置き換えてください。
出力先はマウント元の外にします。
不明なビルドの報告にはBuild IDと個人情報を除いたエラー文だけを記載し、バックアップ・鍵・抽出値を添付しないでください。
