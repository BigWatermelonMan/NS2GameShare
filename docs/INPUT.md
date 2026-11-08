# Controller and mouse configuration

[日本語](#日本語)

## Controller buttons

1. Disconnect the session and connect the controller to Windows.
2. Open **Controllers**, then select **DirectInput** or **XInput**. DS4 and Switch Pro use DirectInput settings.
3. Select the device or XInput slot, or leave it on **Automatic**.
4. In **Button mapping**, click the Nintendo button you want to configure.
5. Release all buttons, then press one controller button or D-pad direction.
6. Save settings and reconnect to apply the changes.

Esc cancels assignment; Delete unassigns. XInput triggers can also be assigned.
Assigning a physical button to a new Nintendo button replaces its previous assignment.
DirectInput stick axes and inversion are configured separately under **Axes & inversion**.
Use **Input test** to check the result without connecting to a host.

## Mouse

Enable mouse controls in **Mouse** and choose the output: **Right stick** (default) or **Gyro (experimental)**.
Press F8 to capture/release the mouse. Losing focus also releases it.
Mouse button mappings apply while captured.

Right-stick mode has sensitivity and smoothing controls. Lower smoothing responds faster.

### Experimental gyro

The host game must support motion controls and have them enabled.
**Game compatibility, axes and sensitivity have not been verified in gameplay.**

- Set yaw and pitch sensitivity as mouse **counts per full virtual controller turn** (default 8000).
  Lower values increase sensitivity. The game's settings also affect camera rotation.
- Adjust pitch range (default ±80°) and invert yaw/pitch independently.
- Press F7 to reset the virtual attitude. Use the game's camera reset as needed.
  Capture and reset keys can be changed; conflicting assignments are rejected when saving.
- Stick-mode sensitivity and smoothing do not affect gyro mode.

**Input test** displays the virtual yaw/pitch. Restart the test after changing settings.

## 日本語

### コントローラー

1. セッションを切断し、コントローラーをWindowsに接続します。
2. 「コントローラー」でDirectInput／XInputを選びます。DS4とSwitchプロコンはDirectInputを使用します。
3. デバイスまたはXInputの番号を選びます。自動選択も使用できます。
4. 「ボタンの割り当て」で、設定したいNintendoボタンの割当欄をクリックします。
5. すべてのボタンを離してから、実機のボタンまたは十字キーの1方向を押します。
6. 設定を保存し、再接続すると反映されます。

Escで取消、Deleteで解除できます。XInputのトリガーも割り当てられます。
同じ物理ボタンを別の入力先へ割り当てると、以前の割り当てから移動します。
DirectInputのスティック軸と反転は「軸と反転」で設定します。
「入力テスト」で、Switch 2へ接続せずに動作を確認できます。

### マウス

「マウス」で操作を有効にし、割り当て先を「右スティック」（既定）または「ジャイロ（実験的）」から選びます。
F8で取り込み／解除します。フォーカスを外した場合も解除されます。マウスボタンの割り当ては取り込み中に有効です。
右スティックでは感度と平滑化を調整できます。平滑化を低くすると反応が速くなります。

ジャイロにはゲーム側の対応と有効化が必要です。**ゲームとの互換性・軸方向・感度はゲーム内で未検証です。**

- 横・縦の感度は、仮想コントローラーが1回転するマウスカウントで指定します（既定8000）。小さい値ほど高感度です。
  ゲーム側の設定もカメラの回転量に影響します。
- 上下の可動範囲（既定±80°）と、横・縦の反転を調整できます。
- F7で仮想姿勢をリセットします。必要に応じてゲーム側のカメラリセットも使用してください。
  取り込みキーとリセットキーは変更できます。重複した割り当ては保存時に検出します。
- 右スティックの感度・平滑化はジャイロには影響しません。

「入力テスト」で仮想姿勢を表示します。設定を変えたらテストを再開してください。
