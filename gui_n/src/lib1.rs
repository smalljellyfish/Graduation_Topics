use anyhow::Result;
use lazy_static::lazy_static;
use log::{debug, error};
use regex::Regex;
use serde::Deserialize;
use serde_json::Value;
use std::fs::File;
use std::io::Read;
use std::sync::Mutex;
use thiserror::Error;


lazy_static! {
    static ref LAST_ERROR: Mutex<Option<String>> = Mutex::new(None);
}

#[derive(Deserialize)]
pub struct ServiceConfig {
    pub client_id: String,
    pub client_secret: String,
}

#[derive(Deserialize)]
pub struct Config {
    pub spotify: ServiceConfig,
    pub osu: ServiceConfig,
}

#[derive(Error, Debug)]
pub enum ConfigError {
    #[error("無法開啟配置文件: {0}")]
    FileOpenError(String),
    #[error("無法讀取配置文件內容: {0}")]
    FileReadError(String),
    #[error("配置文件格式錯誤: {0}")]
    JsonParseError(String),
    #[error("Spotify 配置錯誤: {0}")]
    SpotifyConfigError(String),
    #[error("Osu 配置錯誤: {0}")]
    OsuConfigError(String),
    #[error("其他錯誤: {0}")]
    Other(String),
}

pub fn read_config(debug_mode: bool) -> Result<Config, ConfigError> {
    if debug_mode {
        debug!("開始讀取配置文件");
    }

    let file_path = "config.json";
    let mut file = File::open(file_path)
        .map_err(|e| ConfigError::FileOpenError(e.to_string()))?;

    if debug_mode {
        debug!("成功開啟配置文件: {}", file_path);
    }

    let mut content = String::new();
    file.read_to_string(&mut content)
        .map_err(|e| ConfigError::FileReadError(e.to_string()))?;

    if debug_mode {
        debug!("成功讀取配置文件內容");
    }

    let config_value: Value = serde_json::from_str(&content)
        .map_err(|e| ConfigError::JsonParseError(e.to_string()))?;

    if debug_mode {
        debug!("成功解析 JSON 格式");
    }

    // 檢查 Spotify 配置
    if let Err(e) = check_spotify_config(&config_value) {
        return Err(ConfigError::SpotifyConfigError(e.join(", ")));
    }

    // 檢查 Osu 配置
    if let Err(e) = check_osu_config(&config_value) {
        return Err(ConfigError::OsuConfigError(e.join(", ")));
    }

    // 解析配置
    let config: Config = serde_json::from_value(config_value)
        .map_err(|e| ConfigError::JsonParseError(e.to_string()))?;

    Ok(config)
}

fn check_spotify_config(config_value: &Value) -> Result<(), Vec<String>> {
    let mut errors = Vec::new();

    let spotify = match config_value.get("spotify") {
        Some(s) => s,
        None => {
            errors.push("缺少 Spotify 配置".to_string());
            return Err(errors);
        }
    };

    let client_id = spotify.get("client_id").and_then(Value::as_str);
    let client_secret = spotify.get("client_secret").and_then(Value::as_str);

    if let Some(id) = client_id {
        if id.len() != 32 {
            errors.push("Spotify client_id 長度不正確，應為 32 個字符".to_string());
        }
        let hex_regex = Regex::new(r"^[0-9a-f]{32}$").unwrap();
        if !hex_regex.is_match(id) {
            errors.push("Spotify client_id 格式錯誤，應為 32 位十六進制字符".to_string());
        }
    } else {
        errors.push("Spotify client_id 缺失或格式錯誤".to_string());
    }

    if let Some(secret) = client_secret {
        if secret.len() != 32 {
            errors.push("Spotify client_secret 長度不正確，應為 32 個字符".to_string());
        }
        let hex_regex = Regex::new(r"^[0-9a-f]{32}$").unwrap();
        if !hex_regex.is_match(secret) {
            errors.push("Spotify client_secret 格式錯誤，應為 32 位十六進制字符".to_string());
        }
    } else {
        errors.push("Spotify client_secret 缺失或格式錯誤".to_string());
    }

    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors)
    }
}
fn check_osu_config(config_value: &Value) -> Result<(), Vec<String>> {
    let mut errors = Vec::new();

    let osu = match config_value.get("osu") {
        Some(o) => o,
        None => {
            errors.push("缺少 Osu 配置".to_string());
            return Err(errors);
        }
    };

    let client_id = osu.get("client_id").and_then(Value::as_str);
    let client_secret = osu.get("client_secret").and_then(Value::as_str);

    if let Some(id) = client_id {
        if !id.chars().all(char::is_numeric) || id.len() < 5 {
            errors.push("Osu client_id 格式錯誤，應為至少 5 位的數字".to_string());
        }
    } else {
        errors.push("Osu client_id 缺失或格式錯誤".to_string());
    }

    if let Some(secret) = client_secret {
        if secret.len() < 40 {
            errors.push("Osu client_secret 長度不足，應至少為 40 個字符".to_string());
        }
    } else {
        errors.push("Osu client_secret 缺失或格式錯誤".to_string());
    }

    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors)
    }
}




