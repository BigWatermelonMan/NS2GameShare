//! Persistent, user-owned settings; the launcher never stores key material.
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use std::{
    ffi::OsString,
    path::{Path, PathBuf},
};
pub mod i18n;
mod native;
mod ui;
pub use ui::run;

#[derive(Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub language: i18n::Language,
    pub dark_theme: bool,
    pub ui_scale: f32,
    pub display_name: String,
    pub port: String,
    pub keys: PathBuf,
    pub passphrase_file: PathBuf,
    pub fx2_firmware: PathBuf,
    pub fx2_auto_load: bool,
    pub receive_kbps: u32,
    pub video_width: u32,
    pub video_height: u32,
    pub audio: bool,
    pub av_sync: bool,
    pub denoise: bool,
    pub input: crate::input::Config,
}
impl Default for Settings {
    fn default() -> Self {
        Self {
            language: i18n::Language::system(),
            dark_theme: false,
            ui_scale: 1.0,
            display_name: crate::profile::APP_NAME.into(),
            port: "fx2".into(),
            keys: PathBuf::new(),
            passphrase_file: PathBuf::new(),
            fx2_firmware: PathBuf::new(),
            fx2_auto_load: false,
            receive_kbps: 1400,
            video_width: 960,
            video_height: 540,
            audio: true,
            av_sync: false,
            denoise: false,
            input: Default::default(),
        }
    }
}
pub fn default_path() -> PathBuf {
    std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")))
        .join("NS2GameShare")
        .join("settings.json")
}
pub fn input_path(path: &Path) -> PathBuf {
    path.with_extension("input.json")
}
fn atomic_json(path: &Path, value: &impl Serialize) -> Result<()> {
    if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
        std::fs::create_dir_all(parent)?;
    }
    let temp = path.with_extension("json.tmp");
    std::fs::write(&temp, serde_json::to_vec_pretty(value)?)?;
    std::fs::rename(temp, path)?;
    Ok(())
}
impl Settings {
    pub fn load(path: &Path) -> Result<Self> {
        if !path.exists() {
            return Ok(Self::default());
        }
        let settings: Self = serde_json::from_slice(&std::fs::read(path)?)
            .context("設定ファイルを読み込めません。元のファイルは上書きしていません。")?;
        settings.validate()?;
        Ok(settings)
    }
    pub fn validate(&self) -> Result<()> {
        ensure!(
            self.ui_scale.is_finite() && (0.8..=1.5).contains(&self.ui_scale),
            "画面の拡大率は80～150%です。"
        );
        crate::profile::validate_name(&self.display_name)?;
        ensure!(
            !self.port.trim().is_empty() && !self.port.chars().any(char::is_control),
            "接続先を指定してください。"
        );
        ensure!(
            (600..=2000).contains(&self.receive_kbps),
            "受信容量は600～2000 kbpsです。"
        );
        ensure!(
            (320..=1920).contains(&self.video_width) && (180..=1080).contains(&self.video_height),
            "映像サイズが範囲外です。"
        );
        self.input.validate()
    }
    pub fn save(&self, path: &Path) -> Result<()> {
        self.validate()?;
        atomic_json(&input_path(path), &self.input)?;
        atomic_json(path, self)
    }
    pub fn validate_connection(&self) -> Result<()> {
        self.validate()?;
        ensure!(
            self.keys.is_file(),
            "prod.keys のファイルを選択してください。"
        );
        ensure!(
            self.passphrase_file.is_file(),
            "パスフレーズファイルを選択してください。"
        );
        ensure!(
            !self.fx2_auto_load || (self.port == "fx2" && self.fx2_firmware.is_file()),
            "FX2自動ロードにはFX2接続先とRAMファームを指定してください。"
        );
        Ok(())
    }
    pub fn play_args(&self, path: &Path) -> Vec<OsString> {
        // Individual OS arguments preserve Japanese, spaces and literal punctuation.
        let mut args: Vec<OsString> = vec![
            "play".into(),
            "--name".into(),
            self.display_name.clone().into(),
            "--port".into(),
            self.port.clone().into(),
            "--keys".into(),
            self.keys.as_os_str().into(),
            "--passphrase-file".into(),
            self.passphrase_file.as_os_str().into(),
            "--input-config".into(),
            input_path(path).into(),
            "--receive-kbps".into(),
            self.receive_kbps.to_string().into(),
            "--video-width".into(),
            self.video_width.to_string().into(),
            "--video-height".into(),
            self.video_height.to_string().into(),
        ];
        for (on, flag) in [
            (!self.audio, "--mute"),
            (self.av_sync, "--av-sync"),
            (self.denoise, "--denoise"),
        ] {
            if on {
                args.push(flag.into());
            }
        }
        if self.fx2_auto_load {
            args.push("--fx2-firmware".into());
            args.push(self.fx2_firmware.as_os_str().into());
        }
        args
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn appearance_defaults_migrate_without_changing_playback_arguments() {
        let mut settings: Settings =
            serde_json::from_str(r#"{"display_name":"NS2GameShare-PC","port":"fx2"}"#).unwrap();
        assert!(!settings.dark_theme);
        assert_eq!(settings.language, i18n::Language::system());
        assert_eq!(settings.ui_scale, 1.0);
        let args = settings.play_args(Path::new("settings.json"));
        settings.dark_theme = true;
        settings.ui_scale = 1.5;
        settings.language = i18n::Language::English;
        settings.validate().unwrap();
        assert_eq!(settings.play_args(Path::new("settings.json")), args);
        settings.ui_scale = 2.0;
        assert!(settings.validate().is_err());
    }
    #[test]
    fn names_and_paths_are_separate_arguments_and_settings_round_trip() {
        let mut settings = Settings {
            display_name: "テスト PC".into(),
            keys: PathBuf::from("C:/My Keys/prod.keys"),
            ..Default::default()
        };
        settings.input.keyboard.buttons[0] = Some(0x43);
        let encoded = serde_json::to_vec(&settings).unwrap();
        let loaded: Settings = serde_json::from_slice(&encoded).unwrap();
        loaded.validate().unwrap();
        assert_eq!(loaded.input.keyboard.buttons[0], Some(0x43));
        let args = loaded.play_args(Path::new("C:/My Settings/session.json"));
        assert_eq!(args[2], "テスト PC");
        assert_eq!(args[6], "C:/My Keys/prod.keys");
        assert_eq!(args[10], "C:/My Settings/session.input.json");
        assert!(!String::from_utf8(encoded).unwrap().contains("master_key"));
    }
    #[test]
    fn legacy_input_settings_keep_defaults_and_validate_conflicts() {
        let old: crate::input::Config =
            serde_json::from_str(r#"{"mouse_sensitivity":0.03}"#).unwrap();
        old.validate().unwrap();
        assert_eq!(old.keyboard.buttons[0], Some(0x58));
        let mut s = Settings::default();
        s.input.keyboard.buttons[0] = Some(0x77);
        assert!(s.validate().is_err());
        s.input.keyboard.buttons[0] = Some(0x43);
        s.input.xinput_index = Some(4);
        assert!(s.validate().is_err());
    }
}
