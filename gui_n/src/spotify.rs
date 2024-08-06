use anyhow::{anyhow, Error, Result};
use lazy_static::lazy_static;
use log::{debug, error, info};
use regex::Regex;

use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use std::ffi::OsString;
use std::fs::OpenOptions;

use std::os::windows::ffi::OsStrExt;
use std::sync::Arc;
use std::sync::Mutex;
use std::time::Duration;

use std::process::Command;
use std::ptr;

use std::future::Future;
use std::io::{self, BufRead, BufReader, Write};
use std::net::TcpListener;
use std::pin::Pin;
use tokio::time::timeout;
use url::Url;

use winapi::{
    shared::{minwindef::HKEY, ntdef::LPCWSTR},
    um::{
        shellapi::ShellExecuteA,
        //winnt::KEY_READ,
        winreg::{RegCloseKey, RegOpenKeyExW, HKEY_CLASSES_ROOT},
        winuser::SW_SHOW,
    },
};

use crate::read_config;
use chrono::Local;
use rspotify::model::PlayableItem;

use rspotify::AuthCodeSpotify;

use rspotify::{clients::OAuthClient, scopes, Credentials, OAuth, Token};

lazy_static! {
    static ref ERR_MSG: Mutex<String> = Mutex::new(String::new());
}

const SPOTIFY_API_BASE_URL: &str = "https://api.spotify.com/v1";
const SPOTIFY_AUTH_URL: &str = "https://accounts.spotify.com/api/token";
const REDIRECT_URI: &str = "http://localhost:8888/callback";

use thiserror::Error;

#[derive(Error, Debug)]
pub enum SpotifyError {
    #[error("獲取 access token 失敗: {0}")]
    AccessTokenError(String),
    #[error("請求失敗: {0}")]
    RequestError(#[from] reqwest::Error),
    #[error("JSON 解析錯誤: {0}")]
    JsonError(#[from] serde_json::Error),
    #[error("IO 錯誤: {0}")]
    IoError(String),
    #[error("URL 解析錯誤: {0}")]
    UrlParseError(#[from] url::ParseError),
    #[error("Spotify API 錯誤: {0}")]
    ApiError(String),
    #[error("授權錯誤: {0}")]
    AuthorizationError(String),
}

#[derive(Clone)]
pub enum AuthStatus {
    NotStarted,
    WaitingForBrowser,
    Processing,
    TokenObtained,
    Completed,
    Failed(String),
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct Album {
    pub album_type: String,
    pub total_tracks: u32,
    pub external_urls: HashMap<String, String>,
    //href: String,
    pub id: String,
    pub images: Vec<Image>,
    pub name: String,
    pub release_date: String,
    //release_date_precision: String,
    //restrictions: Option<Restrictions>,
    //#[serde(rename = "type")]
    //album_type_field: String,
    //uri: String,
    pub artists: Vec<Artist>,
}
#[derive(Deserialize, Clone)]
pub struct Albums {
    pub items: Vec<Album>,
}
#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct Image {
    pub url: String,
    pub height: u32,
    pub width: u32,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct Restrictions {
    pub reason: String,
}

#[derive(Deserialize)]
pub struct AuthResponse {
    access_token: String,
}

#[derive(Deserialize, Serialize, Debug, Clone)]
pub struct Artist {
    pub name: String,
}

#[derive(Deserialize)]
pub struct SearchResult {
    pub tracks: Option<Tracks>,
    pub albums: Option<Albums>,
}

#[derive(Deserialize, Clone)]
pub struct Tracks {
    pub items: Vec<Track>,
    pub total: u32,
}

#[derive(Deserialize, Clone)]
pub struct Track {
    pub name: String,
    pub artists: Vec<Artist>,
    pub external_urls: HashMap<String, String>,
    pub album: Album,
}
pub struct TrackWithCover {
    pub name: String,
    pub artists: Vec<Artist>,
    pub external_urls: HashMap<String, String>,
    pub album_name: String,
    pub cover_url: Option<String>,
}

#[derive(Debug, Clone)]
pub struct TrackInfo {
    pub name: String,
    pub artists: String,
    pub album: String,
}

lazy_static! {
    static ref SPOTIFY_URL_REGEX: Regex =
        Regex::new(r"https?://open\.spotify\.com/(track|album)/([a-zA-Z0-9]{22})?")
            .expect("Failed to compile Spotify URL regex");
}

pub enum SpotifyUrlStatus {
    Valid,
    Incomplete,
    Invalid,
    NotSpotify,
}

#[derive(Debug, Clone)]
pub struct CurrentlyPlaying {
    pub track_info: TrackInfo,
    pub spotify_url: Option<String>,
}

pub fn is_valid_spotify_url(url: &str) -> Result<SpotifyUrlStatus, SpotifyError> {
    lazy_static! {
        static ref SPOTIFY_URL_REGEX: Regex = Regex::new(
            r"^https?://open\.spotify\.com/(track|album|playlist)/[a-zA-Z0-9]+(?:\?.*)?$"
        ).unwrap();
    }

    if let Ok(parsed_url) = url::Url::parse(url) {
        match parsed_url.domain() {
            Some("open.spotify.com") => {
                if SPOTIFY_URL_REGEX.is_match(url) {
                    Ok(SpotifyUrlStatus::Valid)
                } else {
                    Ok(SpotifyUrlStatus::Incomplete)
                }
            },
            Some(_) => {
                if url.contains("/track/") || url.contains("/album/") || url.contains("/playlist/") {
                    Ok(SpotifyUrlStatus::Invalid)
                } else {
                    Ok(SpotifyUrlStatus::NotSpotify)
                }
            },
            None => Ok(SpotifyUrlStatus::NotSpotify),
        }
    } else {
        Ok(SpotifyUrlStatus::NotSpotify)
    }
}
/*
pub async fn search_album_by_url(
    client: &reqwest::Client,
    url: &str,
    access_token: &str,
) -> Result<Album, Box<dyn std::error::Error>> {
    let re = Regex::new(r"https?://open\.spotify\.com/album/([a-zA-Z0-9]+)").unwrap();

    let album_id_result = match re.captures(url) {
        Some(caps) => match caps.get(1) {
            Some(m) => Ok(m.as_str().to_string()),
            None => {
                let mut err_msg = ERR_MSG.lock().unwrap();
                *err_msg = "URL疑似錯誤，請重新輸入".to_string();
                Err("URL疑似錯誤，請重新輸入".into())
            }
        },
        None => {
            let mut err_msg = ERR_MSG.lock().unwrap();
            *err_msg = "URL疑似錯誤，請重新輸入".to_string();
            Err("URL疑似錯誤，請重新輸入".into())
        }
    };

    match album_id_result {
        Ok(album_id) => {
            let api_url = format!("https://api.spotify.com/v1/albums/{}", album_id);
            let response = client
                .get(&api_url)
                .header(AUTHORIZATION, format!("Bearer {}", access_token))
                .header(CONTENT_TYPE, "application/json")
                .send()
                .await?
                .json::<Album>()
                .await?;

            Ok(response)
        }
        Err(e) => {
            println!("ERROR: {}", e);

            Err(e)
        }
    }
}

pub async fn search_album_by_name(
    client: &reqwest::Client,
    album_name: &str,
    access_token: &str,
    page: u32,
    limit: u32,
) -> Result<(Vec<Album>, u32), Box<dyn std::error::Error>> {
    let offset = (page - 1) * limit;
    let search_url = format!(
        "https://api.spotify.com/v1/search?q={}&type=album&limit={}&offset={}",
        album_name, limit, offset
    );
    let response = client
        .get(&search_url)
        .header("Authorization", format!("Bearer {}", access_token))
        .send()
        .await?;

    let search_result: SearchResult = response.json().await?;
    let total_pages =
        (search_result.albums.clone().unwrap().items.len() as u32 + limit - 1) / limit;
    let albums = search_result.albums.unwrap().items;
    Ok((albums, total_pages))
}


pub fn print_track_infos(track_infos: Vec<Track>) {
    println!(" ");
    println!("------------------------");
    for track_info in track_infos {
        let artist_names: Vec<String> = track_info
            .artists
            .into_iter()
            .map(|artist| artist.name)
            .collect();
        println!("Track: {}", track_info.name);
        println!("Artists: {}", artist_names.join(", "));
        println!("Album: {}", track_info.album.name);
        if let Some(spotify_url) = track_info.external_urls.get("spotify") {
            println!("Spotify URL: {}", spotify_url);
        }
        println!("------------------------");
    }
}
 */
pub fn print_track_info_gui(track: &Track) -> (TrackInfo, Option<String>) {
    let track_name = track.name.clone();
    let album_name = track.album.name.clone();
    let artist_names = track
        .artists
        .iter()
        .map(|artist| artist.name.clone())
        .collect::<Vec<String>>()
        .join(", ");

    let spotify_url = track.external_urls.get("spotify").cloned();

    let track_info = TrackInfo {
        name: track_name,
        artists: artist_names,
        album: album_name,
    };

    (track_info, spotify_url)
}

pub async fn get_track_info(
    client: &reqwest::Client,
    track_id: &str,
    access_token: &str,
) -> Result<Track> {
    let url = format!("{}/tracks/{}", SPOTIFY_API_BASE_URL, track_id);
    let response = client
        .get(&url)
        .header("Authorization", format!("Bearer {}", access_token))
        .send()
        .await
        .map_err(Error::from)?;

    let body = response.text().await.map_err(Error::from)?;
    let track: Track = serde_json::from_str(&body)?;

    Ok(track)
}

pub async fn search_track(
    client: &Client,
    query: &str,
    token: &str,
    limit: u32,
    offset: u32,
    debug_mode: bool,
) -> Result<(Vec<TrackWithCover>, u32), SpotifyError> {
    let url = format!(
        "{}/search?q={}&type=track&limit={}&offset={}",
        SPOTIFY_API_BASE_URL, query, limit, offset
    );

    let response = client.get(&url).bearer_auth(token).send().await
        .map_err(|e| SpotifyError::RequestError(e))?;

    if debug_mode {
        info!("Spotify API 請求詳情:");
        info!("  URL: {}", url);
        info!("收到回應狀態碼: {}", response.status());
    }

    let response_text = response.text().await
        .map_err(|e| SpotifyError::RequestError(e))?;

    if debug_mode {
        info!("Spotify API 回應 JSON: {}", response_text);
    }

    let search_result: SearchResult = serde_json::from_str(&response_text)
        .map_err(|e| SpotifyError::JsonError(e))?;

    match search_result.tracks {
        Some(tracks) => {
            let total_pages = (tracks.total + limit - 1) / limit;

            if debug_mode {
                info!("找到 {} 首曲目，共 {} 頁", tracks.total, total_pages);
            }

            let mut track_infos: Vec<TrackWithCover> = Vec::new();
            let mut error_occurred = false;

            for track in tracks.items {
                let cover_url = track.album.images.first().map(|img| img.url.clone());
                let artists_names = track
                    .artists
                    .iter()
                    .map(|artist| artist.name.clone())
                    .collect::<Vec<String>>()
                    .join(", ");

                if cover_url.is_none() {
                    error_occurred = true;
                    error!(
                        "處理曲目時出錯: \"{}\" by {} - 缺少封面 URL",
                        track.name, artists_names
                    );
                } else if debug_mode {
                    info!("處理曲目: \"{}\" by {}", track.name, artists_names);
                    info!("  專輯封面 URL: {}", cover_url.as_ref().unwrap());
                }

                track_infos.push(TrackWithCover {
                    name: track.name,
                    artists: track.artists,
                    external_urls: track.external_urls,
                    album_name: track.album.name,
                    cover_url,
                });
            }

            if error_occurred {
                error!("部分曲目處理出錯，請檢查錯誤日誌");
            } else if debug_mode {
                info!("成功處理 {} 首曲目", track_infos.len());
            }

            Ok((track_infos, total_pages))
        }
        None => Err(SpotifyError::ApiError("搜索結果中沒有找到曲目".to_string())),
    }
}

pub async fn get_access_token(client: &reqwest::Client, debug_mode: bool) -> Result<String, SpotifyError> {
    let config = read_config(debug_mode).map_err(|e| SpotifyError::IoError(e.to_string()))?;
    let client_id = &config.spotify.client_id;
    let client_secret = &config.spotify.client_secret;

    if debug_mode {
        debug!("正在獲取 Spotify access token");
    }

    let auth_url = SPOTIFY_AUTH_URL;
    let body = "grant_type=client_credentials";
    let auth_header = base64::encode(format!("{}:{}", client_id, client_secret));
    let request = client
        .post(auth_url)
        .header("Authorization", format!("Basic {}", auth_header))
        .header("Content-Type", "application/x-www-form-urlencoded")
        .body(body);

        let response = request.send().await.map_err(SpotifyError::RequestError)?;

        if response.status().is_success() {
            let auth_response: AuthResponse = response.json().await?;  // 這裡直接使用 ?
            if debug_mode {
                debug!("成功獲取 Spotify access token");
            }
            Ok(auth_response.access_token)
        } else {
            let error_text = response.text().await.map_err(SpotifyError::RequestError)?;
            error!("獲取 token 請求失敗: {}", error_text);
            Err(SpotifyError::AccessTokenError(error_text))
        }
}

pub fn open_spotify_url(url: &str) -> io::Result<()> {
    let current_time = Local::now().format("%H:%M:%S").to_string();
    let log_file_path = "output.log";
    let mut file = OpenOptions::new()
        .append(true)
        .create(true)
        .open(log_file_path)?;

    if url.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "URL cannot be empty",
        ));
    }

    let track_id = url
        .split("/")
        .last()
        .filter(|s| !s.is_empty())
        .unwrap_or_default();
    if track_id.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "Invalid URL format",
        ));
    }

    let spotify_uri = format!("spotify:track:{}", track_id);
    let web_url = format!("https://open.spotify.com/track/{}", track_id);

    if is_spotify_protocol_associated()? {
        let result = unsafe {
            ShellExecuteA(
                ptr::null_mut(),
                "open\0".as_ptr() as *const i8,
                spotify_uri.as_ptr() as *const i8,
                ptr::null(),
                ptr::null(),
                SW_SHOW,
            )
        };

        if result as usize > 32 {
            writeln!(
                file,
                "{} [INFO ] Successfully opened Spotify APP with {}",
                current_time, spotify_uri
            )?;
            return Ok(());
        } else {
            writeln!(
                file,
                "{} [ERROR] Failed to open Spotify APP with {}",
                current_time, spotify_uri
            )?;
        }
    }

    match open_url_default_browser(&web_url) {
        Ok(_) => {
            writeln!(
                file,
                "{} [INFO ] Successfully opened web URL with default browser: {}",
                current_time, web_url
            )?;
            Ok(())
        }
        Err(e) => {
            writeln!(
                file,
                "{} [ERROR] Failed to open web URL with default browser due to error: {}, URL: {}",
                current_time, e, web_url
            )?;
            Err(io::Error::new(
                io::ErrorKind::Other,
                "Failed to open Spotify URL",
            ))
        }
    }
}
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
fn is_spotify_protocol_associated() -> io::Result<bool> {
    let sub_key_os_string = OsString::from("spotify");
    let sub_key_vec: Vec<u16> = sub_key_os_string
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    let sub_key: LPCWSTR = sub_key_vec.as_ptr();

    let mut hkey: HKEY = ptr::null_mut();

    match unsafe {
        RegOpenKeyExW(
            HKEY_CLASSES_ROOT,
            sub_key,
            0,
            winapi::um::winnt::KEY_READ,
            &mut hkey,
        )
    } {
        0 => {
            unsafe {
                RegCloseKey(hkey);
            }
            Ok(true)
        }
        2 => Ok(false),
        _ => Err(io::Error::new(
            io::ErrorKind::Other,
            "Failed to check Spotify protocol association",
        )),
    }
}
pub async fn update_current_playing(
    spotify: &AuthCodeSpotify,
    currently_playing: Arc<Mutex<Option<CurrentlyPlaying>>>,
    debug_mode: bool,
) -> Result<Option<CurrentlyPlaying>> {
    match spotify.current_user_playing_item().await {
        Ok(Some(playing_context)) => {
            if let Some(PlayableItem::Track(track)) = playing_context.item {
                let artists = track
                    .artists
                    .iter()
                    .map(|a| Artist {
                        name: a.name.clone(),
                    })
                    .collect::<Vec<_>>();
                let track_info = TrackInfo {
                    name: track.name,
                    artists: artists
                        .iter()
                        .map(|a| a.name.clone())
                        .collect::<Vec<_>>()
                        .join(", "),
                    album: track.album.name,
                };
                let spotify_url = track.external_urls.get("spotify").cloned();
                let new_currently_playing = CurrentlyPlaying {
                    track_info,
                    spotify_url,
                };
                Ok(Some(new_currently_playing))
            } else {
                Ok(None)
            }
        }
        Ok(None) => Ok(None),
        Err(e) => {
            error!("獲取當前播放信息時發生錯誤: {:?}", e);
            Err(anyhow!("獲取當前播放信息失敗"))
        }
    }
}
pub async fn update_currently_playing_wrapper(
    spotify_client: Arc<Mutex<Option<AuthCodeSpotify>>>,
    currently_playing: Arc<Mutex<Option<CurrentlyPlaying>>>,
    debug_mode: bool,
) -> Result<()> {
    let spotify_ref = {
        let spotify = spotify_client.lock().unwrap();
        spotify.as_ref().cloned()
    };

    let update_result = if let Some(spotify) = spotify_ref {
        update_current_playing(&spotify, currently_playing.clone(), debug_mode).await
    } else {
        Err(anyhow!("Spotify 客戶端未初始化"))
    };

    match update_result {
        Ok(Some(new_currently_playing)) => {
            let mut currently_playing = currently_playing.lock().unwrap();
            *currently_playing = Some(new_currently_playing);
            Ok(())
        }
        Ok(None) => {
            let mut currently_playing = currently_playing.lock().unwrap();
            *currently_playing = None;
            Ok(())
        }
        Err(e) => {
            if e.to_string().contains("InvalidToken") {
                error!("Token 無效，需要重新授權");
                return Err(anyhow!("Token 無效，需要重新授權"));
            } else {
                error!("更新當前播放失敗: {:?}", e);
                Err(e)
            }
        }
    }
}

pub fn authorize_spotify(
    spotify_client: Arc<Mutex<Option<AuthCodeSpotify>>>,
    debug_mode: bool,
    auth_status: Arc<Mutex<AuthStatus>>,
) -> Pin<Box<dyn Future<Output = Result<(), SpotifyError>> + Send>> {
    Box::pin(async move {
        let config = read_config(debug_mode).map_err(|e| SpotifyError::IoError(e.to_string()))?;

        let client_id = &config.spotify.client_id;
        let redirect_uri = "http://localhost:8888/callback";
        let scope = "user-read-currently-playing";

        let mut url = Url::parse("https://accounts.spotify.com/authorize")
            .map_err(SpotifyError::UrlParseError)?;
        url.query_pairs_mut()
            .append_pair("client_id", client_id)
            .append_pair("response_type", "code")
            .append_pair("redirect_uri", redirect_uri)
            .append_pair("scope", scope)
            .append_pair("show_dialog", "true");

        let auth_url = url.to_string();

        if debug_mode {
            info!("Authorization URL: {}", auth_url);
        }

        {
            let mut status = auth_status.lock().map_err(|e| SpotifyError::IoError(e.to_string()))?;
            *status = AuthStatus::WaitingForBrowser;
        }

        open_url_default_browser(&auth_url).map_err(|e| SpotifyError::IoError(e.to_string()))?;

        // 啟動本地伺服器來捕獲回調
        let listener = TcpListener::bind("127.0.0.1:8888")
            .map_err(|e| SpotifyError::IoError(format!("無法啟動本地伺服器: {}", e)))?;

        // 等待回調
        let (mut stream, _) = listener.accept()
            .map_err(|e| SpotifyError::IoError(format!("無法接受連接: {}", e)))?;
        let mut reader = BufReader::new(&stream);
        let mut request_line = String::new();
        reader.read_line(&mut request_line)
            .map_err(|e| SpotifyError::IoError(format!("無法讀取請求: {}", e)))?;

        let redirect_url = request_line
            .split_whitespace()
            .nth(1)
            .ok_or_else(|| SpotifyError::AuthorizationError("無效的請求".to_string()))?;
        let url = format!("{}{}", REDIRECT_URI, redirect_url);

        // 向瀏覽器發送響應
        let response = "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=UTF-8\r\n\r\n<html><body>授權成功，請關閉此窗口。</body></html>";
        stream.write_all(response.as_bytes())
            .map_err(|e| SpotifyError::IoError(format!("無法發送響應: {}", e)))?;

        if debug_mode {
            info!("Received callback URL: {}", url);
        }

        {
            let mut status = auth_status.lock().map_err(|e| SpotifyError::IoError(e.to_string()))?;
            *status = AuthStatus::Processing;
        }

        let parsed_url = Url::parse(&url).map_err(SpotifyError::UrlParseError)?;
        let code = parsed_url
            .query_pairs()
            .find(|(key, _)| key == "code")
            .map(|(_, value)| value.into_owned())
            .ok_or_else(|| SpotifyError::AuthorizationError("無法從回調 URL 中解析授權碼".to_string()))?;

        // 當獲取到授權碼後，使用 client_id 和 client_secret 請求訪問令牌
        let token_url = "https://accounts.spotify.com/api/token";
        let client = reqwest::Client::new();
        let params = [
            ("grant_type", "authorization_code"),
            ("code", &code),
            ("redirect_uri", "http://localhost:8888/callback"),
        ];

        match timeout(
            Duration::from_secs(30),
            client
                .post(token_url)
                .basic_auth(
                    &config.spotify.client_id,
                    Some(&config.spotify.client_secret),
                )
                .form(&params)
                .send(),
        )
        .await
        {
            Ok(response_result) => {
                let response = response_result.map_err(SpotifyError::RequestError)?;
                if response.status().is_success() {
                    let token_data: Token = response.json().await?; 

                    {
                        let mut status = auth_status.lock().map_err(|e| SpotifyError::IoError(e.to_string()))?;
                        *status = AuthStatus::TokenObtained;
                    }

                    // 創建新的 AuthCodeSpotify 實例
                    let creds = Credentials::new(&config.spotify.client_id, &config.spotify.client_secret);
                    let oauth = OAuth {
                        redirect_uri: "http://localhost:8888/callback".to_string(),
                        scopes: scopes!("user-read-currently-playing"),
                        ..Default::default()
                    };

                    // 使用 from_token_with_config 方法，並使用完全限定的路徑
                    let new_spotify = AuthCodeSpotify::from_token_with_config(
                        token_data,
                        creds,
                        oauth,
                        rspotify::Config::default(), // 使用 rspotify::Config 而不是 Config
                    );

                    // 更新 spotify_client
                    let mut client = spotify_client
                        .lock()
                        .map_err(|e| SpotifyError::IoError(format!("無法獲取 Spotify 客戶端鎖: {}", e)))?;
                    *client = Some(new_spotify);

                    {
                        let mut status = auth_status.lock().map_err(|e| SpotifyError::IoError(e.to_string()))?;
                        *status = AuthStatus::Completed;
                    }
                } else {
                    return Err(SpotifyError::ApiError(format!("獲取訪問令牌失敗: {}", response.status())));
                }
            }
            Err(_) => return Err(SpotifyError::ApiError("請求訪問令牌超時".to_string())),
        }

        Ok(())
    })
}

pub fn load_spotify_icon(ctx: &egui::Context) -> Option<egui::TextureHandle> {
    let is_dark = ctx.style().visuals.dark_mode;

    let icon_name = if is_dark {
        "spotify_icon_black.png"
    } else {
        "spotify_icon_black.png"
    };

    // 獲取可執行文件的目錄
    let exe_dir = std::env::current_exe().ok()?;
    let exe_dir = exe_dir.parent()?;

    // icon 資料夾與 exe 檔在同一目錄
    let icon_dir = exe_dir.join("icon");

    // 構建圖標的絕對路徑
    let icon_path = icon_dir.join(icon_name);

    println!("Trying to load icon from: {:?}", icon_path);

    match load_image_from_path(&icon_path) {
        Ok(image) => {
            let texture_options = egui::TextureOptions {
                magnification: egui::TextureFilter::Linear,
                minification: egui::TextureFilter::Linear,
                wrap_mode: egui::TextureWrapMode::ClampToEdge,
            };
            Some(ctx.load_texture("spotify_icon", image, texture_options))
        }
        Err(e) => {
            eprintln!("Failed to load Spotify icon ({}): {:?}", icon_name, e);
            // 嘗試加載另一個圖標作為備用
            let fallback_icon_name = if is_dark {
                "spotify_icon_black.png"
            } else {
                "spotify_icon.png"
            };
            let fallback_icon_path = icon_dir.join(fallback_icon_name);

            println!(
                "Trying to load fallback icon from: {:?}",
                fallback_icon_path
            );

            match load_image_from_path(&fallback_icon_path) {
                Ok(fallback_image) => {
                    Some(ctx.load_texture("spotify_icon", fallback_image, Default::default()))
                }
                Err(e) => {
                    eprintln!("無法載入備用 Spotify 圖標：{:?}", e);
                    None
                }
            }
        }
    }
}
// 輔助函數來加載圖片
fn load_image_from_path(path: &std::path::Path) -> Result<egui::ColorImage, image::ImageError> {
    let image = image::io::Reader::open(path)?.decode()?;
    let size = [image.width() as _, image.height() as _];
    let image_buffer = image.into_rgba8();
    let pixels = image_buffer.as_flat_samples();

    // 手動處理透明度
    let mut color_image = egui::ColorImage::new(size, egui::Color32::TRANSPARENT);
    for (i, pixel) in pixels.as_slice().chunks_exact(4).enumerate() {
        let [r, g, b, a] = pixel else { continue };
        if *a > 0 {
            color_image.pixels[i] = egui::Color32::from_rgba_unmultiplied(*r, *g, *b, *a);
        }
    }

    Ok(color_image)
}
