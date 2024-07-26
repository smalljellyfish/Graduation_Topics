use anyhow::{anyhow, Context, Result};
use lazy_static::lazy_static;
use log::{debug, error, info};
use regex::Regex;
use serde::Deserialize;
use serde_json::Value;
use std::fs::File;
use std::io::Read;
use std::sync::Mutex;


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

pub fn read_config(debug_mode: bool) -> Result<Config> {
    if debug_mode {
        debug!("開始讀取配置文件");
    }

    let file_path = "config.json";
    let mut file =
        File::open(file_path).with_context(|| format!("無法開啟配置文件: {}", file_path))?;

    if debug_mode {
        debug!("成功開啟配置文件: {}", file_path);
    }

    let mut content = String::new();
    file.read_to_string(&mut content)
        .with_context(|| "無法讀取配置文件內容")?;

    if debug_mode {
        debug!("成功讀取配置文件內容");
    }

    let config_value: Value =
        serde_json::from_str(&content).with_context(|| "配置文件格式錯誤，請檢查 JSON 格式")?;

    if debug_mode {
        debug!("成功解析 JSON 格式");
    }

    let mut errors = Vec::new();

    // Spotify 配置檢查
    if let Err(e) = check_spotify_config(&config_value) {
        errors.extend(e);
    }

    // Osu 配置檢查
    if let Err(e) = check_osu_config(&config_value) {
        errors.extend(e);
    }

    if !errors.is_empty() {
        let error_msg = format!("配置檢查失敗:\n{}", errors.join("\n"));

        // 檢查錯誤是否有變化
        let mut last_error = LAST_ERROR.lock().unwrap();
        if last_error.as_ref() != Some(&error_msg) {
            error!("{}", error_msg);
            *last_error = Some(error_msg.clone());
        }

        return Err(anyhow!(error_msg));
    } else {
        // 如果沒有錯誤，清除上一次的錯誤記錄
        let mut last_error = LAST_ERROR.lock().unwrap();
        if last_error.is_some() {
            info!("配置檢查通過");
            *last_error = None;
        }
    }

    // 如果檢查通過，解析為 Config 結構
    let config: Config =
        serde_json::from_value(config_value).with_context(|| "無法將配置文件解析為 Config 結構")?;

    if debug_mode {
        debug!("成功將配置解析為 Config 結構");
    }

    if debug_mode {
        debug!("完整的 JSON 配置文件: {}", content);
    }

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




