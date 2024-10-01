// 標準庫導入
use std::fs::File;
use std::fs;
use std::io::{self, Read};
use std::process::Command;
use std::sync::Mutex;
use std::path::PathBuf;
use std::collections::HashMap;

// 第三方庫導入
use anyhow::Result;
use chrono::Utc;
use chrono::DateTime;
use dirs;
use dirs::home_dir;
use reqwest::Client;
use lazy_static::lazy_static;
use log::{debug, error, LevelFilter};
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use thiserror::Error;

// 靜態變量
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
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct LoginInfo {
    pub platform: String,  // 新增字段，用於識別平台（如 "spotify" 或 "osu"）
    pub access_token: String,
    pub refresh_token: String,
    pub expiry_time: DateTime<Utc>,
    pub avatar_url: Option<String>,
    pub user_name: Option<String>,
}

#[derive(Deserialize)]
struct RefreshTokenResponse {
    access_token: String,
    expires_in: i64,
    refresh_token: Option<String>,
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
    let mut file = File::open(file_path).map_err(|e| ConfigError::FileOpenError(e.to_string()))?;

    if debug_mode {
        debug!("成功開啟配置文件: {}", file_path);
    }

    let mut content = String::new();
    file.read_to_string(&mut content)
        .map_err(|e| ConfigError::FileReadError(e.to_string()))?;

    if debug_mode {
        debug!("成功讀取配置文件內容");
    }

    let config_value: Value =
        serde_json::from_str(&content).map_err(|e| ConfigError::JsonParseError(e.to_string()))?;

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

//設置日誌級別
pub fn set_log_level(debug_mode: bool) {
    let log_level = if debug_mode {
        LevelFilter::Debug
    } else {
        LevelFilter::Info
    };
    log::set_max_level(log_level);
}
// 新增輔助函數來獲取保存路徑
pub fn get_app_data_path() -> PathBuf {
    let mut path = dirs::data_local_dir().unwrap_or_else(|| PathBuf::from("."));
    path.push("SongSearch");
    path
}

pub fn save_login_info(login_info: &HashMap<String, LoginInfo>) -> Result<(), ConfigError> {
    let app_data_path = get_app_data_path();
    fs::create_dir_all(&app_data_path)
        .map_err(|e| ConfigError::Other(format!("無法創建應用數據目錄: {}", e)))?;

    let file_path = app_data_path.join("login_info.json");
    let json = serde_json::to_string(login_info)
        .map_err(|e| ConfigError::Other(format!("無法序列化登入信息: {}", e)))?;
    
    fs::write(&file_path, json)
        .map_err(|e| ConfigError::FileOpenError(format!("無法保存登入信息: {}", e)))
}

pub fn read_login_info() -> Result<HashMap<String, LoginInfo>, ConfigError> {
    let file_path = get_app_data_path().join("login_info.json");
    
    match fs::read_to_string(file_path) {
        Ok(contents) => {
            let login_info: HashMap<String, LoginInfo> = serde_json::from_str(&contents)
                .map_err(|e| ConfigError::JsonParseError(format!("無法解析登入信息: {}", e)))?;
            Ok(login_info)
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(HashMap::new()),
        Err(e) => Err(ConfigError::FileReadError(format!("無法讀取登入信息: {}", e))),
    }
}

pub fn is_token_valid(login_info: &LoginInfo) -> bool {
    Utc::now() < login_info.expiry_time
}

pub async fn check_and_refresh_token(client: &Client, config: &Config, platform: &str) -> Result<LoginInfo, ConfigError> {
    let mut login_infos = read_login_info()?;
    
    match login_infos.get(platform) {
        Some(login_info) => {
            if is_token_valid(login_info) {
                Ok(login_info.clone())
            } else {
                // 令牌已過期,嘗試刷新
                let new_token = refresh_spotify_token(client, &config.spotify, &login_info.refresh_token).await?;
                
                let new_login_info = LoginInfo {
                    platform: platform.to_string(),
                    access_token: new_token.access_token,
                    refresh_token: new_token.refresh_token.unwrap_or_else(|| login_info.refresh_token.clone()),
                    expiry_time: Utc::now() + chrono::Duration::seconds(new_token.expires_in as i64),
                    avatar_url: login_info.avatar_url.clone(),
                    user_name: login_info.user_name.clone(),
                };
                
                login_infos.insert(platform.to_string(), new_login_info.clone());
                save_login_info(&login_infos)?;
                Ok(new_login_info)
            }
        }
        None => Err(ConfigError::Other(format!("沒有保存的{}登入信息", platform))),
    }
}

async fn refresh_spotify_token(
    client: &Client,
    config: &ServiceConfig,
    refresh_token: &str,
) -> Result<RefreshTokenResponse, ConfigError> {
    let token_url = "https://accounts.spotify.com/api/token";
    let params = [
        ("grant_type", "refresh_token"),
        ("refresh_token", refresh_token),
    ];

    let response = client
        .post(token_url)
        .basic_auth(&config.client_id, Some(&config.client_secret))
        .form(&params)
        .send()
        .await
        .map_err(|e| ConfigError::Other(format!("刷新令牌請求失敗: {}", e)))?;

    if response.status().is_success() {
        let token_data: RefreshTokenResponse = response
            .json()
            .await
            .map_err(|e| ConfigError::Other(format!("解析刷新令牌響應失敗: {}", e)))?;
        Ok(token_data)
    } else {
        let error_text = response
            .text()
            .await
            .map_err(|e| ConfigError::Other(format!("讀取錯誤響應失敗: {}", e)))?;
        Err(ConfigError::Other(format!("刷新令牌失敗: {}", error_text)))
    }
}

pub fn load_download_directory() -> Option<PathBuf> {
    // 首先嘗試讀取保存的下載目錄
    let saved_path = get_app_data_path().join("download_directory.txt");
    if let Ok(path_str) = fs::read_to_string(&saved_path) {
        let path = PathBuf::from(path_str);
        if path.exists() {
            return Some(path);
        }
    }

    // 如果沒有保存的目錄或目錄不存在，嘗試默認的osu!歌曲目錄
    if let Some(home) = home_dir() {
        let default_osu_path = home.join("AppData\\Local\\osu!\\Songs");
        if default_osu_path.exists() {
            // 如果默認目錄存在，保存並返回它
            let _ = save_download_directory(&default_osu_path);
            return Some(default_osu_path);
        }
    }

    // 如果默認目錄也不存在，返回None
    None
}

pub fn save_download_directory(download_directory: &PathBuf) -> Result<(), std::io::Error> {
    let path = get_app_data_path().join("download_directory.txt");
    fs::create_dir_all(path.parent().unwrap())?;
    fs::write(&path, download_directory.to_str().unwrap())?;
    Ok(())
}

// 新增一個函數來檢查是否需要選擇下載目錄
pub fn need_select_download_directory() -> bool {
    load_download_directory().is_none()
}

// 打開默認瀏覽器
pub fn open_url_default_browser(url: &str) -> io::Result<()> {
    if cfg!(target_os = "windows") {
        // 使用 PowerShell 來打開 URL
        Command::new("powershell")
            .arg("-Command")
            .arg(format!("Start-Process '{}'", url))
            .spawn()
            .map_err(|e| {
                io::Error::new(io::ErrorKind::Other, format!("Failed to open URL: {}", e))
            })?;
    } else if cfg!(target_os = "macos") {
        Command::new("open").arg(url).spawn().map_err(|e| {
            io::Error::new(io::ErrorKind::Other, format!("Failed to open URL: {}", e))
        })?;
    } else if cfg!(target_os = "linux") {
        Command::new("xdg-open").arg(url).spawn().map_err(|e| {
            io::Error::new(io::ErrorKind::Other, format!("Failed to open URL: {}", e))
        })?;
    } else {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            "Unsupported operating system",
        ));
    }

    Ok(())
}