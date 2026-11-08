//! Context-independent Japanese/English settings UI catalog.
use serde::{Deserialize, Serialize};
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Language {
    #[default]
    Japanese,
    English,
}
impl Language {
    pub fn system() -> Self {
        // PRIMARYLANGID: all Japanese UI variants use 0x11.
        if unsafe { windows::Win32::Globalization::GetUserDefaultUILanguage() } & 0x3ff == 0x11 {
            Self::Japanese
        } else {
            Self::English
        }
    }
    pub fn text(self, source: &str) -> &str {
        if self == Self::Japanese {
            return source;
        }
        match source {
            "DirectInputとXInputの入力を設定します。" => {
                "Configure DirectInput and XInput controls."
            }
            "入力方式ごとにデバイスとスティックを設定します。" => {
                "Configure devices and sticks for the selected input API."
            }
            "DirectInputを有効にする" => "Enable DirectInput",
            "デッドゾーン" => "Dead zone",
            "軸と反転" => "Axes & inversion",
            "入力先をクリックし、実コントローラーのボタンを押して割り当てます。" => {
                "Click a Nintendo button, then press a button on your controller."
            }
            "Escで取消、Deleteで解除。十字キーとXInputのトリガーにも対応します。" => {
                "Esc cancels; Delete unassigns. Supports D-pad and XInput triggers."
            }
            "この方式の設定を既定に戻す" => "Reset this input API",
            "コントローラーの割り当て" => "Assign a controller button",
            "入力先" => "Nintendo button",
            "すべてのボタンを離してから、割り当てるボタンを1つ押してください。" => {
                "Release all buttons, then press one controller button."
            }
            "右スティックまたは実験的ジャイロへマウス移動を割り当てます。" => {
                "Map mouse movement to the right stick or experimental gyro."
            }
            "割り当て先" => "Output",
            "ジャイロ（実験的）" => "Gyro (experimental)",
            "ゲーム側のジャイロ対応が必要です。実ゲームでの動作は未検証です。" => {
                "Requires gyro support in the game. Live gameplay has not been verified."
            }
            "横方向：1回転のマウスカウント" => "Yaw: mouse counts per turn",
            "縦方向：1回転のマウスカウント" => "Pitch: mouse counts per turn",
            "カウント値が小さいほど高感度になります。" => {
                "Lower counts mean higher sensitivity."
            }
            "上下の可動範囲" => "Pitch range",
            "横方向を反転" => "Invert yaw",
            "縦方向を反転" => "Invert pitch",
            "姿勢をリセット" => "Reset attitude",
            "取り込み中" => "Captured",
            "解除中" => "Released",

            "SYSTEM / splay から抽出…" => "Extract from SYSTEM / splay...",
            "パスフレーズの抽出" => "Extract GameShare passphrase",
            "所有する本体の読み取り専用SYSTEM、splay Program NCA、またはExeFSのmain NSOを選択します。" => {
                "Select your own read-only mounted SYSTEM, splay Program NCA, or ExeFS main NSO."
            }
            "NCAの復号には、接続画面で選んだprod.keysのheader_keyとkey_area_keyが必要です。" => {
                "NCA decryption uses header_key and key_area_key from prod.keys selected on the Connection page."
            }
            "SYSTEMフォルダー…" => "SYSTEM folder...",
            "NCA / NSOファイル…" => "NCA / NSO file...",
            "対応ビルドだけを検証して抽出します。未知の版では出力しません。" => {
                "Only verified builds are supported. Unknown builds produce no output."
            }
            "抽出値は非表示で、設定フォルダー内のprivateへ16バイトだけ保存します。元のファイルと既存の出力は上書きしません。" => {
                "Saves only 16 bytes under private/ beside your settings, without showing them. Source files and existing outputs are never overwritten."
            }
            "抽出中…" => "Extracting...",
            "抽出して設定に反映" => "Extract & use",
            "抽出中です。SYSTEMには書き込みません。" => {
                "Extracting. SYSTEM is opened read-only."
            }
            "抽出しました。設定を保存してください。" => {
                "Passphrase extracted. Save your settings to keep the selected path."
            }
            "十字 左" => "D-pad Left",
            "十字 上" => "D-pad Up",
            "十字 右" => "D-pad Right",
            "十字 下" => "D-pad Down",
            "接続" => "Connection",
            "Switch 2 の表示名と接続方法を設定します。" => {
                "Choose your Switch 2 display name and connection method."
            }
            "映像と音声" => "Video & audio",
            "画質・音声と、設定画面の表示を調整します。" => {
                "Adjust streaming and settings window preferences."
            }
            "キーボード" => "Keyboard",
            "ボタンをクリックして、使いたいキーを押してください。" => {
                "Click a binding, then press the key you want to use."
            }
            "コントローラー" => "Controllers",
            "DS4・プロコン・XInput の割り当てとスティックを調整します。" => {
                "Configure DS4, Switch Pro and XInput buttons and sticks."
            }
            "マウス" => "Mouse",
            "マウスからのスティック・ジャイロ操作とボタンを設定します。" => {
                "Configure mouse-to-stick / gyro controls and button bindings."
            }
            "コントローラーの割り当て前に切断してください。" => {
                "Disconnect before configuring controller buttons."
            }
            "入力ウィンドウを利用できません。" => "Input window is unavailable.",
            "ジャイロのリセットキーが他の割り当てと重複しています。" => {
                "The gyro reset key conflicts with another binding."
            }
            "入力テスト" => "Input test",
            "Switch 2 に接続せず、現在の設定で入力を確認できます。" => {
                "Test your current controls without connecting to Switch 2."
            }
            "未割当" => "Unassigned",
            "Esc と F9 は予約されています。" => "Esc and F9 are reserved.",
            "取り込みキーを未割当にすることはできません。" => {
                "The mouse capture key cannot be unassigned."
            }
            "設定を確認して、Switch 2 の募集に接続できます。" => {
                "Review your settings, then connect to a Switch 2 host."
            }
            "設定を保存しました。次の接続から適用されます。" => {
                "Settings saved. Changes apply to the next connection."
            }
            "ゲーム中はゲーム画面で入力を確認してください。" => {
                "Use the game window to test controls while playing."
            }
            "入力ウィンドウがありません。" => "The input window is unavailable.",
            "既に接続中です。" => "Already connected.",
            "Switch 2 で募集を開始し、設定した表示名を承認してください。" => {
                "Start GameShare on Switch 2 and approve your display name."
            }
            "通信と音声を終了しています…" => "Stopping communication and audio...",
            "セッションログはまだありません。" => {
                "No session log is available yet."
            }
            "キーを変更しました。保存すると次の接続から適用されます。" => {
                "Key binding changed. Save to apply on the next connection."
            }
            "キーの割り当てをキャンセルしました。" => "Key assignment cancelled.",
            "切断しました。設定を変更して再接続できます。" => {
                "Disconnected. You can change settings and reconnect."
            }
            "接続が終了しました。\n{}" => "Connection ended.\n{}",
            "未接続" => "Disconnected",
            "切断中" => "Disconnecting",
            "プレイ中" => "Playing",
            "接続中" => "Connecting",
            "ライト" => "Light",
            "ダーク" => "Dark",
            "切断中…" => "Disconnecting...",
            "切断" => "Disconnect",
            "保存して接続" => "Save & connect",
            "設定を保存" => "Save settings",
            "● 未保存の変更があります" => "Unsaved changes",
            "設定は保存されています" => "Settings saved",
            "入力・接続を確認してください" => "Check your settings and connection",
            "閉じる" => "Close",
            "キーを割り当て" => "Assign a key",
            "使いたいキーを押してください。" => "Press the key you want to use.",
            "Esc: キャンセル   /   Delete: 未割当" => "Esc: Cancel   /   Delete: Unassign",
            "キャンセル" => "Cancel",
            "未保存の変更" => "Unsaved changes",
            "変更した設定を保存してから閉じますか？" => {
                "Save your changes before closing?"
            }
            "保存して閉じる" => "Save & close",
            "保存せず閉じる" => "Discard & close",
            "戻る" => "Back",
            "ファイルを選択してください" => "Choose a file",
            "参照…" => "Browse...",
            "参加プロフィール" => "Player profile",
            "Switch 2 の参加者一覧に表示されます。" => {
                "Shown in the Switch 2 participant list."
            }
            "表示名" => "Display name",
            "日本語は目安10文字まで。変更は次の接続から反映されます。" => {
                "Up to 32 UTF-8 bytes (about 10 Japanese characters). Applies next time."
            }
            "接続方法" => "Connection method",
            "ESP32 (Native USB) を自動検出" => "Auto-detect ESP32 (Native USB)",
            "再検出" => "Refresh",
            "接続先を直接指定" => "Enter a connection port manually",
            "認証ファイル" => "Authentication files",
            "ファイルを参照します。鍵の内容を設定へ複製することはありません。" => {
                "Only file paths are saved. Key contents are never copied into settings."
            }
            "パスフレーズファイル" => "Passphrase file",
            "USB ブリッジ" => "USB bridge",
            "ESP32 + FX2LP を使用する場合の接続準備です。" => {
                "Connection preparation for ESP32 + FX2LP."
            }
            "接続時に FX2 の RAM ファームを自動ロードする" => {
                "Load FX2 RAM firmware automatically when connecting"
            }
            "FX2 RAM ファーム" => "FX2 RAM firmware",
            "RAM への一時ロードです。EEPROM は変更しません。" => {
                "Temporary RAM loading only. EEPROM is not modified."
            }
            "セッションログを開く" => "Open session log",
            "接続に問題がある場合の確認に使用できます。" => {
                "Use the log to investigate connection problems."
            }
            "映像" => "Video",
            "ホストが送信する映像の希望設定です。" => {
                "Preferred stream settings. The host makes the final choice."
            }
            "要求解像度" => "Requested resolution",
            "受信容量の上限" => "Receive capacity limit",
            "GPU ノイズ低減を有効にする" => "Enable GPU noise reduction",
            "解像度はホストが最終決定します。ノイズ低減はプレイ中に F9 でも切り替えられます。" => {
                "The host decides the resolution. Press F9 during play to toggle noise reduction."
            }
            "音声と同期" => "Audio & synchronization",
            "低遅延でのプレイを優先する場合は、映像同期をオフにします。" => {
                "Leave video synchronization off to prioritize low latency."
            }
            "ゲーム音を再生する" => "Play game audio",
            "映像を音声の再生時刻に合わせる" => {
                "Synchronize video to audio playback"
            }
            "映像同期を有効にすると、操作から表示までの待ち時間が増えます。" => {
                "Video synchronization adds input-to-display latency."
            }
            "設定画面の表示" => "Settings window",
            "再生画面の解像度には影響しません。" => {
                "Does not change the game window resolution."
            }
            "テーマ" => "Theme",
            "表示倍率" => "Interface scale",
            "キーを押してください…" => "Press a key...",
            "既定のキー配置に戻す" => "Reset keyboard bindings",
            "WASD プリセット" => "WASD preset",
            "ゲームのボタン" => "Game buttons",
            "クリック → キー押下で変更。Delete で未割当、Esc でキャンセルします。" => {
                "Click, then press a key. Delete unassigns; Esc cancels."
            }
            "左スティック" => "Left stick",
            "十字キーとは別に割り当てられます。" => {
                "Can be assigned independently of the D-pad."
            }
            "左 ←" => "Left",
            "上 ↑" => "Up",
            "右 →" => "Right",
            "下 ↓" => "Down",
            "Esc はプレイ終了、F9 はノイズ低減用です。マウス取り込みキーとの重複は保存時に確認します。" => {
                "Esc exits play; F9 toggles noise reduction. Capture-key conflicts are checked on save."
            }
            "デバイス" => "Device",
            "DS4 の USB / Bluetooth、Switch プロコン、XInput に対応します。" => {
                "Supports DS4 USB / Bluetooth, Switch Pro and XInput."
            }
            "保存済みデバイス（未接続）" => "Saved device (disconnected)",
            "自動選択" => "Automatic",
            "XInput を有効にする" => "Enable XInput",
            "コントローラー {}" => "Controller {}",
            "スティック" => "Sticks",
            "中心付近の誤反応はデッドゾーンで調整できます。" => {
                "Adjust dead zones to prevent drift around the center."
            }
            "XInput 左 DZ" => "XInput left DZ",
            "XInput 右 DZ" => "XInput right DZ",
            "DirectInput の軸と反転" => "DirectInput axes & inversion",
            "左スティック X" => "Left stick X",
            "左スティック Y" => "Left stick Y",
            "右スティック X" => "Right stick X",
            "右スティック Y" => "Right stick Y",
            "反転" => "Invert",
            "ボタンの割り当て" => "Button mapping",
            "自動配置から、ボタンごとのカスタマイズへ切り替えられます。" => {
                "Switch from automatic mapping to individual button assignments."
            }
            "ボタン割り当てをカスタマイズする" => "Customize button mapping",
            "タッチパッド" => "Touchpad",
            "ボタン 15" => "Button 15",
            "ボタン 16" => "Button 16",
            "ボタン {}" => "Button {}",
            "デバイスに合った標準の配置を使用します。" => {
                "Using the default mapping for this device."
            }
            "コントローラー設定を既定に戻す" => "Reset controller settings",
            "マウスで視点を操作" => "Mouse camera control",
            "移動量を右スティックの入力へ変換します。" => {
                "Convert mouse movement to right-stick input."
            }
            "マウス操作を有効にする" => "Enable mouse controls",
            "取り込み / 解除" => "Capture / release",
            "感度" => "Sensitivity",
            "平滑化" => "Smoothing",
            "感度は高いほど少ない移動で大きく動きます。平滑化は低いほど応答が速くなります。" => {
                "Higher sensitivity needs less movement. Lower smoothing responds faster."
            }
            "マウスのボタン" => "Mouse buttons",
            "取り込み中だけゲームに送信します。" => {
                "Sent to the game only while the mouse is captured."
            }
            "左ボタン" => "Left button",
            "右ボタン" => "Right button",
            "中央ボタン" => "Middle button",
            "戻るボタン" => "Back button",
            "進むボタン" => "Forward button",
            "フォーカスを外すと取り込みを解除します。設定変更後は入力テストを再開してください。" => {
                "Capture is released when focus is lost. Restart the input test after changing settings."
            }
            "入力の確認" => "Test your controls",
            "テスト中の操作はゲームへ送信されません。設定を変更したらテストを再開してください。" => {
                "Test input is not sent to the game. Restart after changing settings."
            }
            "テストを停止" => "Stop test",
            "入力テストを開始" => "Start input test",
            "入力変化  {} 回" => "Input changes: {}",
            "プレイ中はゲーム画面をクリックして操作を確認してください。" => {
                "Click the game window to test controls during play."
            }
            "中心からの入力をリアルタイムで表示します。" => {
                "Live stick positions relative to the center."
            }
            "右スティック" => "Right stick",
            "ボタン" => "Buttons",
            "短い押下も見つけやすいよう、ハイライトを少し保持します。" => {
                "Highlights remain briefly so short presses are easier to see."
            }
            "拡張 {i}" => "Extra {i}",
            "拡張ボタン" => "Extra buttons",
            "設定ファイル: {}" => "Settings file: {}",
            "NS2GameShare — 設定" => "NS2GameShare - Settings",
            "設定画面を起動できません: {error}" => "Unable to open settings: {error}",
            "言語 / Language" => "Language",
            "入力変化" => "Input changes",
            "拡張" => "Extra",
            "すべてのファイル\0*.*\0" => "All files\0*.*\0",
            "設定ファイルを読み込めません。元のファイルは上書きしていません。" => {
                "Unable to read settings. The original file has not been overwritten."
            }
            "画面の拡大率は80～150%です。" => {
                "Interface scale must be between 80 and 150%."
            }
            "接続先を指定してください。" => "Specify a connection port.",
            "受信容量は600～2000 kbpsです。" => {
                "Receive capacity must be between 600 and 2000 kbps."
            }
            "映像サイズが範囲外です。" => {
                "Video dimensions are outside the supported range."
            }
            "prod.keys のファイルを選択してください。" => {
                "Select your prod.keys file."
            }
            "パスフレーズファイルを選択してください。" => {
                "Select your passphrase file."
            }
            "FX2自動ロードにはFX2接続先とRAMファームを指定してください。" => {
                "FX2 auto-loading requires the FX2 connection and a RAM firmware file."
            }
            "マウス取り込みキーとゲーム操作キーが重複しています。" => {
                "The mouse capture key conflicts with a game control key."
            }
            "キーには Esc と F9 を使用できません。" => {
                "Esc and F9 cannot be assigned to game controls."
            }
            "表示名は空白のみを避け、制御文字を含まない UTF-8 32 バイト以内にしてください（日本語は目安10文字）。" => {
                "Use a nonblank display name of up to 32 UTF-8 bytes, without control characters."
            }
            "接続が終了しました。" => "Connection ended.",
            _ => source,
        }
    }
    pub fn message(self, message: &str) -> String {
        message
            .lines()
            .map(|line| {
                if let Some((prefix, detail)) = line.split_once(": ") {
                    format!("{}: {}", self.text(prefix), detail)
                } else {
                    self.text(line).into()
                }
            })
            .collect::<Vec<_>>()
            .join("\n")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn language_round_trip_and_user_data_preservation() {
        assert_eq!(
            serde_json::to_string(&Language::English).unwrap(),
            "\"english\""
        );
        assert_eq!(Language::English.text("接続"), "Connection");
        assert_eq!(Language::Japanese.text("接続"), "接続");
        assert_eq!(Language::English.text("My PC 01"), "My PC 01");
        assert_eq!(
            Language::English.message("接続が終了しました。\nC:/秘密/My PC"),
            "Connection ended.\nC:/秘密/My PC"
        );
    }
}
