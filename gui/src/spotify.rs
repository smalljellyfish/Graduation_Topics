// 標準庫導入
use std::collections::HashMap;
use std::ffi::OsString;
use std::fs::{self, OpenOptions};
use std::future::Future;
use std::io::{self, Write};
use std::net::SocketAddr;
use std::os::windows::ffi::OsStrExt;
use std::pin::Pin;
use std::ptr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};


// 第三方庫導入
use anyhow::{anyhow, Error, Result};
use chrono::Local;
use chrono::Utc;
use lazy_static::lazy_static;
use log::{debug, error, info};
use regex::Regex;
use reqwest::Client;
use rspotify::{
    clients::{OAuthClient,BaseClient}, model::{PlayableItem,TrackId,FullTrack,PlaylistId}, scopes, AuthCodeSpotify, ClientError, Credentials,
    OAuth, Token,model::SimplifiedPlaylist,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use thiserror::Error;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::Mutex as TokioMutex;
use tokio::time::timeout;
use url::Url;
use winapi::{
    shared::{minwindef::HKEY, ntdef::LPCWSTR},
    um::{
        shellapi::ShellExecuteA,
        winreg::{RegCloseKey, RegOpenKeyExW, HKEY_CLASSES_ROOT},
        winuser::SW_SHOW,
    },
};



// 本地模組導入
use crate::{read_config, AuthManager, AuthPlatform};
use lib::{LoginInfo, save_login_info, open_url_default_browser};

// 常量定義
const SPOTIFY_API_BASE_URL: &str = "https://api.spotify.com/v1";
const SPOTIFY_AUTH_URL: &str = "https://accounts.spotify.com/api/token";

// 靜態變量
lazy_static! {
    static ref ERR_MSG: Mutex<String> = Mutex::new(String::new());
}

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
    #[error("配置錯誤: {0}")]
    ConfigError(String),
    #[error("Spotify 客戶端錯誤: {0}")]
    ClientError(#[from] ClientError),
}
//將std::io::Error轉換為SpotifyError的io error
impl From<io::Error> for SpotifyError {
    fn from(error: io::Error) -> Self {
        SpotifyError::IoError(error.to_string())
    }
}

#[derive(Clone, PartialEq)]
pub enum AuthStatus {
    NotStarted,
    WaitingForBrowser,
    Processing,
    TokenObtained,
    Completed,
    Failed(String),
}

// 定義 PlaylistCache 結構，用於緩存播放列表曲目
#[derive(Serialize, Deserialize)]
pub struct PlaylistCache {
    tracks: Vec<FullTrack>,
    last_updated: SystemTime,
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
    pub is_liked: Option<bool>,
    #[serde(skip)]
    pub index: usize,
    #[serde(skip)]
    pub on_artist_click: Option<Arc<Mutex<dyn Fn(&str) + Send + Sync>>>,
}
pub struct TrackWithCover {
    pub name: String,
    pub artists: Vec<Artist>,
    pub external_urls: HashMap<String, String>,
    pub album_name: String,
    pub cover_url: Option<String>,
    pub index: usize,
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

pub fn search_by_artist(artist_name: &str) {
    info!("Searching for artist: {}", artist_name);
}

pub fn is_valid_spotify_url(url: &str) -> Result<SpotifyUrlStatus, SpotifyError> {
    lazy_static! {
        static ref SPOTIFY_URL_REGEX: Regex = Regex::new(
            r"^https?://open\.spotify\.com/(track|album|playlist)/[a-zA-Z0-9]+(?:\?.*)?$"
        )
        .unwrap();
    }

    if let Ok(parsed_url) = url::Url::parse(url) {
        match parsed_url.domain() {
            Some("open.spotify.com") => {
                if SPOTIFY_URL_REGEX.is_match(url) {
                    Ok(SpotifyUrlStatus::Valid)
                } else {
                    Ok(SpotifyUrlStatus::Incomplete)
                }
            }
            Some(_) => {
                if url.contains("/track/") || url.contains("/album/") || url.contains("/playlist/")
                {
                    Ok(SpotifyUrlStatus::Invalid)
                } else {
                    Ok(SpotifyUrlStatus::NotSpotify)
                }
            }
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

    let response = client
        .get(&url)
        .bearer_auth(token)
        .send()
        .await
        .map_err(|e| SpotifyError::RequestError(e))?;

    if debug_mode {
        info!("Spotify API 請求詳情:");
        info!("  URL: {}", url);
        info!("收到回應狀態碼: {}", response.status());
    }

    let response_text = response
        .text()
        .await
        .map_err(|e| SpotifyError::RequestError(e))?;

    if debug_mode {
        info!("Spotify API 回應 JSON: {}", response_text);
    }

    let search_result: SearchResult =
        serde_json::from_str(&response_text).map_err(|e| SpotifyError::JsonError(e))?;

        match search_result.tracks {
            Some(tracks) => {
                let total_tracks = tracks.total;
                let total_pages = (total_tracks + limit - 1) / limit;

            if debug_mode {
                info!("找到 {} 首曲目，共 {} 頁", tracks.total, total_pages);
            }

            let track_infos: Vec<TrackWithCover> = tracks
                .items
                .into_iter()
                .enumerate()
                .map(|(index, track)| {
                    let cover_url = track.album.images.first().map(|img| img.url.clone());
                    let artists_names = track
                        .artists
                        .iter()
                        .map(|artist| artist.name.clone())
                        .collect::<Vec<String>>()
                        .join(", ");

                    if debug_mode {
                        if let Some(url) = &cover_url {
                            info!(
                                "處理曲目 {}: \"{}\" by {}",
                                index, track.name, artists_names
                            );
                            info!("  專輯封面 URL: {}", url);
                        } else {
                            error!(
                                "處理曲目 {} 時出錯: \"{}\" by {} - 缺少封面 URL",
                                index, track.name, artists_names
                            );
                        }
                    }

                    TrackWithCover {
                        name: track.name,
                        artists: track.artists,
                        external_urls: track.external_urls,
                        album_name: track.album.name,
                        cover_url,
                        index: index + (offset as usize),
                    }
                })
                .collect();

            if debug_mode {
                info!("成功處理 {} 首曲目", track_infos.len());
            }

            Ok((track_infos, total_pages))
        }
        None => Err(SpotifyError::ApiError("搜索結果中沒有找到曲目".to_string())),
    }
}


pub async fn get_access_token(
    client: &reqwest::Client,
    debug_mode: bool,
) -> Result<String, SpotifyError> {
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
        let auth_response: AuthResponse = response.json().await?; // 這裡直接使用 ?
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
                    name: track.name.clone(),
                    artists: artists
                        .iter()
                        .map(|a| a.name.clone())
                        .collect::<Vec<_>>()
                        .join(", "),
                    album: track.album.name.clone(),
                };
                let spotify_url = track.external_urls.get("spotify").cloned();

                if debug_mode {
                    info!("當前播放: {} - {}", track_info.artists, track_info.name);
                    if let Some(url) = &spotify_url {
                        info!("Spotify URL: {}", url);
                    }
                }

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
    auth_manager: Arc<AuthManager>,
    listener: Arc<TokioMutex<Option<TcpListener>>>,
    spotify_authorized: Arc<AtomicBool>,
) -> Pin<Box<dyn Future<Output = Result<(Option<String>, Option<String>), SpotifyError>> + Send>> {
    Box::pin(async move {
        // 重置授權狀態
        auth_manager.reset(&AuthPlatform::Spotify);

        // 讀取和解析 JSON 文件
        let config_str = fs::read_to_string("config.json")
            .map_err(|e| SpotifyError::IoError(format!("無法讀取配置文件: {}", e)))?;
        let config: Value = serde_json::from_str(&config_str)
            .map_err(|e| SpotifyError::ConfigError(format!("無法解析配置文件: {}", e)))?;

        let client_id = config["spotify"]["client_id"]
            .as_str()
            .ok_or_else(|| SpotifyError::ConfigError("Missing Spotify client ID".to_string()))?;
        let scope = "user-read-currently-playing user-read-private user-read-email user-library-read user-library-modify";

        // 檢查是否已有監聽器，如果沒有則創建新的
        let bound_port = {
            let mut listener_guard = listener.lock().await;
            if listener_guard.is_none() {
                let (new_listener, port) = create_listener(debug_mode).await?;
                *listener_guard = Some(new_listener);
                port
            } else {
                listener_guard.as_ref().unwrap().local_addr()?.port()
            }
        };

        // 更新重定向 URI
        let redirect_uri = format!("http://localhost:{}/callback", bound_port);

        let auth_url = create_spotify_auth_url(client_id, &redirect_uri, scope)?;

        if debug_mode {
            info!("Authorization URL: {}", auth_url);
            info!("Redirect URI: {}", redirect_uri);
        }

        auth_manager.update_status(&AuthPlatform::Spotify, AuthStatus::WaitingForBrowser);

        open_url_default_browser(&auth_url).map_err(|e| SpotifyError::IoError(e.to_string()))?;

        // 設置超時時間，增加到 3 分鐘
        let timeout_duration = Duration::from_secs(180);

        let result = match accept_connection(&listener, timeout_duration).await {
            Ok(stream) => {
                let (login_info, avatar_url, user_name) = process_successful_connection(
                    stream,
                    &spotify_client,
                    auth_manager.clone(),
                    &config,
                    &redirect_uri,
                    bound_port,
                    debug_mode,
                    spotify_authorized,
                )
                .await?;

                // 保存登入信息
                let mut login_info_map = HashMap::new();
                login_info_map.insert("spotify".to_string(), login_info);
                match save_login_info(&login_info_map) {
                    Ok(()) => info!("成功保存 Spotify 登入信息"),
                    Err(e) => error!("無法保存 Spotify 登入信息: {:?}", e),
                }

                Ok((avatar_url, user_name))
            }
            Err(e) => {
                let error_message = format!("授權過程中斷: {}", e);
                auth_manager.update_status(
                    &AuthPlatform::Spotify,
                    AuthStatus::Failed(error_message.clone()),
                );
                Err(SpotifyError::AuthorizationError(error_message))
            }
        };

        // 無論成功與否，都關閉監聽器
        {
            let mut listener_guard = listener.lock().await;
            *listener_guard = None;
        }

        result
    })
}

// 輔助函數來創建監聽器
async fn create_listener(debug_mode: bool) -> Result<(TcpListener, u16), SpotifyError> {
    let ports = vec![8888, 8889, 8890, 8891, 8892];
    for port in ports {
        let addr = SocketAddr::from(([127, 0, 0, 1], port));
        match TcpListener::bind(addr).await {
            Ok(listener) => return Ok((listener, port)),
            Err(e) if debug_mode => {
                info!("無法綁定到端口 {}: {}", port, e);
            }
            _ => {}
        }
    }
    Err(SpotifyError::IoError("無法找到可用的端口".to_string()))
}
// 新增的輔助函數來創建 Spotify 授權 URL
fn create_spotify_auth_url(
    client_id: &str,
    redirect_uri: &str,
    scope: &str,
) -> Result<String, SpotifyError> {
    let mut url = Url::parse("https://accounts.spotify.com/authorize")
        .map_err(SpotifyError::UrlParseError)?;
    url.query_pairs_mut()
        .append_pair("client_id", client_id)
        .append_pair("response_type", "code")
        .append_pair("redirect_uri", redirect_uri)
        .append_pair("scope", scope)
        .append_pair("show_dialog", "true");
    Ok(url.to_string())
}

async fn process_successful_connection(
    stream: TcpStream,
    spotify_client: &Arc<Mutex<Option<AuthCodeSpotify>>>,
    auth_manager: Arc<AuthManager>,
    config: &Value,
    redirect_uri: &str,
    port: u16,
    debug_mode: bool,
    spotify_authorized: Arc<AtomicBool>,
) -> Result<(LoginInfo, Option<String>, Option<String>), SpotifyError> {
    let mut reader = BufReader::new(stream);
    let mut request_line = String::new();
    reader
        .read_line(&mut request_line)
        .await
        .map_err(|e| SpotifyError::IoError(format!("無法讀取請求: {}", e)))?;

    let redirect_url = request_line
        .split_whitespace()
        .nth(1)
        .ok_or_else(|| SpotifyError::AuthorizationError("無效的請求".to_string()))?;
    let url = format!("http://localhost:{}{}", port, redirect_url);

    // 向瀏覽器發送響應
    let response = "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=UTF-8\r\n\r\n<html><body>授權成功，請關閉此窗口。</body></html>";
    reader
        .into_inner()
        .write_all(response.as_bytes())
        .await
        .map_err(|e| SpotifyError::IoError(format!("無法發送響應: {}", e)))?;

    if debug_mode {
        info!("Received callback URL: {}", url);
    }

    auth_manager.update_status(&AuthPlatform::Spotify, AuthStatus::Processing);

    // 處理授權回調
    process_authorization_callback(
        url,
        spotify_client,
        auth_manager,
        config,
        redirect_uri,
        spotify_authorized,
    )
    .await
}

async fn process_authorization_callback(
    url: String,
    spotify_client: &Arc<Mutex<Option<AuthCodeSpotify>>>,
    auth_manager: Arc<AuthManager>,
    config: &Value,
    redirect_uri: &str,
    spotify_authorized: Arc<AtomicBool>,
) -> Result<(LoginInfo, Option<String>, Option<String>), SpotifyError> {
    let parsed_url = Url::parse(&url).map_err(SpotifyError::UrlParseError)?;
    let code = parsed_url
        .query_pairs()
        .find(|(key, _)| key == "code")
        .map(|(_, value)| value.into_owned())
        .ok_or_else(|| {
            SpotifyError::AuthorizationError("無法從回調 URL 中解析授權碼".to_string())
        })?;

    let token_url = "https://accounts.spotify.com/api/token";
    let client = reqwest::Client::new();
    let params = [
        ("grant_type", "authorization_code"),
        ("code", &code),
        ("redirect_uri", redirect_uri),
    ];

    match timeout(
        Duration::from_secs(30),
        client
            .post(token_url)
            .basic_auth(
                config["spotify"]["client_id"].as_str().ok_or_else(|| {
                    SpotifyError::ConfigError("Missing Spotify client ID".to_string())
                })?,
                Some(config["spotify"]["client_secret"].as_str().ok_or_else(|| {
                    SpotifyError::ConfigError("Missing Spotify client secret".to_string())
                })?),
            )
            .form(&params)
            .send(),
    )
    .await
    {
        Ok(response_result) => match response_result {
            Ok(response) => {
                let status = response.status();
                if status.is_success() {
                    let token_data: Token = response.json().await?;

                    auth_manager.update_status(&AuthPlatform::Spotify, AuthStatus::TokenObtained);

                    let creds = Credentials::new(
                        config["spotify"]["client_id"].as_str().ok_or_else(|| {
                            SpotifyError::ConfigError("Missing Spotify client ID".to_string())
                        })?,
                        config["spotify"]["client_secret"].as_str().ok_or_else(|| {
                            SpotifyError::ConfigError("Missing Spotify client secret".to_string())
                        })?,
                    );
                    let oauth = OAuth {
                        redirect_uri: redirect_uri.to_string(),
                        scopes: scopes!(
                            "user-read-currently-playing",
                            "user-read-private",
                            "user-read-email"
                        ),
                        ..Default::default()
                    };

                    let new_spotify = AuthCodeSpotify::from_token_with_config(
                        token_data.clone(),
                        creds,
                        oauth,
                        rspotify::Config::default(),
                    );

                    let user = new_spotify
                        .current_user()
                        .await
                        .map_err(|e| SpotifyError::ApiError(format!("無法獲取用戶信息: {}", e)))?;

                    let user_name = user.display_name.unwrap_or_else(|| "未知用戶".to_string());
                    let user_avatar_url = user
                        .images
                        .and_then(|images| images.first().map(|image| image.url.clone()));

                    if let Some(url) = &user_avatar_url {
                        info!("成功獲取用戶頭像 URL: {}", url);
                    } else {
                        error!("用戶沒有頭像 URL");
                    }

                    let login_info = LoginInfo {
                        platform: "spotify".to_string(),
                        access_token: token_data.access_token.clone(),
                        refresh_token: token_data.refresh_token.clone().unwrap_or_default(),
                        expiry_time: Utc::now() + chrono::Duration::seconds(token_data.expires_in.num_seconds()),
                        avatar_url: user_avatar_url.clone(),
                        user_name: Some(user_name.clone()),  
                    };

                    let mut client = spotify_client.lock().map_err(|e| {
                        SpotifyError::IoError(format!("無法獲取 Spotify 客戶端鎖: {}", e))
                    })?;
                    *client = Some(new_spotify);

                    auth_manager.update_status(&AuthPlatform::Spotify, AuthStatus::Completed);
                    spotify_authorized.store(true, Ordering::SeqCst);

                    info!("Spotify 授權成功完成");

                    Ok((login_info, user_avatar_url, Some(user_name)))
                } else {
                    let error_body = response.text().await.map_err(SpotifyError::RequestError)?;
                    error!(
                        "獲取訪問令牌失敗. 狀態碼: {}, 錯誤內容: {}",
                        status, error_body
                    );
                    auth_manager.update_status(
                        &AuthPlatform::Spotify,
                        AuthStatus::Failed(format!(
                            "獲取訪問令牌失敗: {} - {}",
                            status, error_body
                        )),
                    );
                    Err(SpotifyError::ApiError(format!(
                        "獲取訪問令牌失敗: {} - {}",
                        status, error_body
                    )))
                }
            }
            Err(e) => {
                error!("請求訪問令牌時發生錯誤: {}", e);
                auth_manager.update_status(
                    &AuthPlatform::Spotify,
                    AuthStatus::Failed(format!("請求訪問令牌時發生錯誤: {}", e)),
                );
                Err(SpotifyError::RequestError(e))
            }
        },
        Err(_) => {
            error!("請求訪問令牌超時");
            auth_manager.update_status(
                &AuthPlatform::Spotify,
                AuthStatus::Failed("請求訪問令牌超時".to_string()),
            );
            Err(SpotifyError::ApiError("請求訪問令牌超時".to_string()))
        }
    }
}

async fn accept_connection(
    listener: &Arc<TokioMutex<Option<TcpListener>>>,
    timeout_duration: Duration,
) -> Result<TcpStream, SpotifyError> {
    let start_time = Instant::now();
    loop {
        if start_time.elapsed() >= timeout_duration {
            return Err(SpotifyError::AuthorizationError(
                "授權超時，請嘗試重新授權".to_string(),
            ));
        }

        if let Some(listener) = listener.lock().await.as_ref() {
            match tokio::time::timeout(Duration::from_millis(100), listener.accept()).await {
                Ok(Ok((stream, _))) => return Ok(stream),
                Ok(Err(e)) if e.kind() == io::ErrorKind::WouldBlock => continue,
                Ok(Err(e)) => return Err(SpotifyError::IoError(format!("接受連接失敗: {}", e))),
                Err(_) => continue, // 超時，繼續循環
            }
        } else {
            return Err(SpotifyError::AuthorizationError("監聽器已關閉".to_string()));
        }
    }
}

pub fn load_spotify_icon(ctx: &egui::Context) -> Option<egui::TextureHandle> {
    let icon_bytes = include_bytes!("assets/spotify_icon_black.png");

    match image::load_from_memory(icon_bytes) {
        Ok(image) => {
            let size = [image.width() as _, image.height() as _];
            let image_buffer = image.to_rgba8();
            let pixels = image_buffer.as_flat_samples();
            let image = egui::ColorImage::from_rgba_unmultiplied(size, pixels.as_slice());

            let texture_options = egui::TextureOptions {
                magnification: egui::TextureFilter::Linear,
                minification: egui::TextureFilter::Linear,
                wrap_mode: egui::TextureWrapMode::ClampToEdge,
            };

            Some(ctx.load_texture("spotify_icon", image, texture_options))
        }
        Err(e) => {
            error!("無法載入 Spotify 圖標：{:?}", e);
            None
        }
    }
}

pub async fn add_track_to_liked(
    spotify: &AuthCodeSpotify, 
    track_id: &str
) -> Result<(), SpotifyError> {
    let track_id = TrackId::from_id(track_id)
        .map_err(|e| SpotifyError::ApiError(format!("無效的曲目 ID: {}", e)))?;
    
    spotify.current_user_saved_tracks_add(vec![track_id])
        .await
        .map_err(|e| SpotifyError::ApiError(format!("無法將曲目添加到 Liked Songs: {}", e)))?;
    
    Ok(())
}
pub async fn remove_track_from_liked(
    spotify: &AuthCodeSpotify, 
    track_id: &str
) -> Result<(), SpotifyError> {
    let track_id = TrackId::from_id(track_id)
        .map_err(|e| SpotifyError::ApiError(format!("無效的曲目 ID: {}", e)))?;
    
    spotify.current_user_saved_tracks_delete(vec![track_id])
        .await
        .map_err(|e| SpotifyError::ApiError(format!("無法從 Liked Songs 中移除曲目: {}", e)))?;
    
    Ok(())
}
pub async fn get_user_playlists(spotify_client: Arc<Mutex<Option<AuthCodeSpotify>>>) -> Result<Vec<SimplifiedPlaylist>> {
    // 鎖定 Mutex，取得 Spotify 客戶端的克隆，然後立即釋放 MutexGuard
    let spotify_ref = {
        let spotify = spotify_client.lock().unwrap();
        spotify.as_ref().cloned()
    };

    if let Some(spotify) = spotify_ref {
        let mut playlists = Vec::new();
        let mut offset = 0;
        loop {
            // 在這裡執行異步操作，不再持有 MutexGuard
            let current_user_playlists = spotify.current_user_playlists_manual(Some(50), Some(offset)).await?;
            if current_user_playlists.items.is_empty() {
                break;
            }
            playlists.extend(current_user_playlists.items);
            offset += 50;
        }
        Ok(playlists)
    } else {
        Err(anyhow!("Spotify 客戶端未初始化"))
    }
}
pub async fn get_playlist_tracks(
    spotify_client: Arc<Mutex<Option<AuthCodeSpotify>>>,
    playlist_id: String,
) -> Result<Vec<FullTrack>> {
    let spotify_ref = {
        let spotify = spotify_client.lock().unwrap();
        spotify.as_ref().cloned()
    };

    if let Some(spotify) = spotify_ref {
        let mut tracks = Vec::new();
        let mut offset = 0;

        let playlist_id = PlaylistId::from_id(&playlist_id)?;

        loop {
            let playlist_items = spotify
                .playlist_items_manual(
                    playlist_id.clone(),
                    None,
                    None,
                    Some(100),
                    Some(offset),
                )
                .await?;

            if playlist_items.items.is_empty() {
                break;
            }

            for item in playlist_items.items {
                if let Some(PlayableItem::Track(track)) = item.track {
                    tracks.push(track);
                }
            }

            offset += 100;
        }

        Ok(tracks)
    } else {
        Err(anyhow!("Spotify 客戶端未初始化"))
    }
}
