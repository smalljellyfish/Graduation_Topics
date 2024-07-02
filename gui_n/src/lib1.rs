use anyhow::{Context, Result, anyhow};
use serde_json::Value;
use regex::Regex;
use lazy_static::lazy_static;
use serde::Deserialize;
use std::fs::File;
use std::io::Read;
use std::sync::Mutex;
use log::error;
use log::info;

lazy_static! {
    static ref ERR_MSG: Mutex<String> = Mutex::new(String::new());
}
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

pub async fn read_config() -> Result<Config> {
    let file_path = "config.json";
    let mut file = File::open(file_path)
        .with_context(|| format!("無法開啟配置文件: {}", file_path))?;
    let mut content = String::new();
    file.read_to_string(&mut content)
        .with_context(|| "無法讀取配置文件內容")?;
    
    let config_value: Value = serde_json::from_str(&content)
        .with_context(|| "配置文件格式錯誤，請檢查 JSON 格式")?;

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
    let config: Config = serde_json::from_value(config_value)
        .with_context(|| "無法將配置文件解析為 Config 結構")?;
    
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

pub mod spotify_search {
    use crate::ERR_MSG;
    use anyhow::{Error, Result};
    use lazy_static::lazy_static;
    use log::{info, error};
    use regex::Regex;
    use reqwest::header::{AUTHORIZATION, CONTENT_TYPE};
    use reqwest::Client;
    use serde::{Deserialize, Serialize};
    use std::collections::HashMap;

    use std::ffi::OsString;
    use std::fs::OpenOptions;
    use std::io::{self, Write};
    use std::os::windows::ffi::OsStrExt;

    use std::process::Command;
    use std::ptr;

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

    pub fn is_valid_spotify_url(url: &str) -> bool {
        if let Some(captures) = SPOTIFY_URL_REGEX.captures(url) {
            if captures.get(1).map_or(false, |m| m.as_str() == "track") {
                return captures.get(2).map_or(false, |m| m.as_str().len() == 22);
            } else if captures.get(1).map_or(false, |m| m.as_str() == "album") {
                return true;
            }
        }
        false
    }

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

    pub fn print_album_info(album: &Album) {
        println!("---------------------------------------------");
        println!("專輯名: {}", album.name);
        println!("專輯歌曲數: {}", album.total_tracks);
        if let Some(spotify_album_url) = album.external_urls.get("spotify") {
            println!("URL: {}", spotify_album_url);
        }
        println!("發布日期: {}", album.release_date);
        println!(
            "歌手: {}",
            album
                .artists
                .iter()
                .map(|artist| artist.name.as_str())
                .collect::<Vec<&str>>()
                .join(", ")
        );
        println!("---------------------------------------------");
    }
    pub async fn get_track_info(
        client: &reqwest::Client,
        track_id: &str,
        access_token: &str,
    ) -> Result<Track> {
        let url = format!("https://api.spotify.com/v1/tracks/{}", track_id);
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
    ) -> Result<(Vec<TrackWithCover>, u32), anyhow::Error> {
        
    
        let url = format!(
            "https://api.spotify.com/v1/search?q={}&type=track&limit={}&offset={}",
            query, limit, offset
        );
        
        let response = client
            .get(&url)
            .bearer_auth(token)
            .send()
            .await?;

        if debug_mode {
            info!("Spotify API 請求詳情:");
            info!("  URL: {}", url);
        }
        
        info!("收到回應狀態碼: {}", response.status());

        // 獲取回應的文本內容
        let response_text = response.text().await?;
    
        // 記錄 Spotify API 的完整 JSON 回應
        if debug_mode {
            info!("Spotify API 回應 JSON: {}", response_text);
        }
        
        

        // 將文本解析為 SearchResult
        let search_result: SearchResult = serde_json::from_str(&response_text)?;
    
        match search_result.tracks {
            Some(tracks) => {
                let total_pages = (tracks.total + limit - 1) / limit;
                info!("找到 {} 首曲目，共 {} 頁", tracks.total, total_pages);
    
                let track_infos: Vec<TrackWithCover> = tracks
                    .items
                    .into_iter()
                    .map(|track| {
                        let cover_url = track.album.images.first().map(|img| img.url.clone());
                        let artists_names = track.artists.iter()
                        .fold(String::new(), |acc, artist| {
                            if acc.is_empty() {
                                artist.name.clone()
                            } else {
                                format!("{}, {}", acc, artist.name)
                            }
                        });
                        info!("處理曲目: \"{}\" by {}", track.name, artists_names);
                        if let Some(url) = &cover_url {
                            info!("  專輯封面 URL: {}", url);
                        }
                        TrackWithCover {
                            name: track.name,
                            artists: track.artists,
                            external_urls: track.external_urls,
                            album_name: track.album.name,
                            cover_url,
                        }
                    })
                    .collect();
    
                info!("成功處理 {} 首曲目", track_infos.len());
                Ok((track_infos, total_pages))
            }
            None => {
                error!("搜索結果中沒有找到曲目");
                Err(anyhow::anyhow!("搜索結果中沒有找到曲目"))
            }
        }
    }

    pub async fn get_access_token(client: &reqwest::Client) -> Result<String> {
        let config = read_config().await?;
        let client_id = &config.spotify.client_id;
        let client_secret = &config.spotify.client_secret;

        let auth_url = "https://accounts.spotify.com/api/token";
        let body = "grant_type=client_credentials";
        let auth_header = base64::encode(format!("{}:{}", client_id, client_secret));
        let request = client
            .post(auth_url)
            .header("Authorization", format!("Basic {}", auth_header))
            .header("Content-Type", "application/x-www-form-urlencoded")
            .body(body);

        let response = request.send().await;

        match response {
            Ok(resp) => {
                let auth_response: AuthResponse = resp.json().await?;
                Ok(auth_response.access_token)
            }
            Err(e) => {
                error!("Error sending request for token: {:?}", e);
                Err(e.into())
            }
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
    fn open_url_default_browser(url: &str) -> io::Result<()> {
        if cfg!(target_os = "windows") {
            Command::new("cmd")
                .args(&["/C", "start", "", url])
                .spawn()
                .map_err(|e| {
                    io::Error::new(io::ErrorKind::Other, format!("Failed to open URL: {}", e))
                })?
        } else if cfg!(target_os = "macos") {
            Command::new("open").arg(url).spawn().map_err(|e| {
                io::Error::new(io::ErrorKind::Other, format!("Failed to open URL: {}", e))
            })?
        } else if cfg!(target_os = "linux") {
            Command::new("xdg-open").arg(url).spawn().map_err(|e| {
                io::Error::new(io::ErrorKind::Other, format!("Failed to open URL: {}", e))
            })?
        } else {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                "Unsupported operating system",
            ));
        };

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
}

pub mod osu_search {
    use crate::read_config;
    use anyhow::Result;
    //use log::{error, info};
    use reqwest::Client;
    use serde::Deserialize;
    use std::io::{self, Write};

    #[derive(Debug, Deserialize)]
    pub struct Covers {
        pub cover: Option<String>,
        pub cover_2x: Option<String>,
        pub card: Option<String>,
        pub card_2x: Option<String>,
        pub list: Option<String>,
        pub list_2x: Option<String>,
        pub slimcover: Option<String>,
        pub slimcover_2x: Option<String>,
    }
    #[derive(Debug, Deserialize)]
    pub struct Beatmapset {
        pub beatmaps: Vec<Beatmap>,
        pub id: i32,
        pub artist: String,
        pub title: String,
        pub creator: String,
        pub covers: Covers,
    }
    #[derive(Deserialize)]
    pub struct TokenResponse {
        access_token: String,
    }

    #[derive(Debug, Deserialize)]
    pub struct SearchResponse {
        beatmapsets: Vec<Beatmapset>,
    }
    #[derive(Debug, Deserialize, Clone)] 
    pub struct Beatmap {
        pub difficulty_rating: f32,
        pub id: i32,
        pub mode: String,
        pub status: String,
        pub total_length: i32,
        pub user_id: i32,
        pub version: String,
    }
    pub struct BeatmapInfo {
        pub title: String,
        pub artist: String,
        pub creator: String,
        pub beatmaps: Vec<String>,
    }
    

    pub async fn get_beatmapsets(
        client: &Client,
        access_token: &str,
        song_name: &str,
    ) -> Result<Vec<Beatmapset>> {
        let response = client
            .get("https://osu.ppy.sh/api/v2/beatmapsets/search")
            .query(&[("query", song_name)])
            .bearer_auth(access_token)
            .send()
            .await
            .map_err(|e| anyhow::anyhow!("Error sending request: {}", e))?
            .json::<SearchResponse>()
            .await
            .map_err(|e| anyhow::anyhow!("Error parsing response: {}", e))?;

        Ok(response.beatmapsets)
    }

    pub async fn get_osu_token(client: &Client) -> Result<String> {
        let config = read_config()
            .await
            .map_err(|e| anyhow::anyhow!("Error reading config: {}", e))?;
        let client_id = &config.osu.client_id;
        let client_secret = &config.osu.client_secret;

        let url = "https://osu.ppy.sh/oauth/token";
        let params = [
            ("client_id", client_id),
            ("client_secret", client_secret),
            ("grant_type", &"client_credentials".to_string()),
            ("scope", &"public".to_string()),
        ];
        let response: TokenResponse = client
            .post(url)
            .form(&params)
            .send()
            .await
            .map_err(|e| anyhow::anyhow!("Error sending request: {}", e))?
            .json()
            .await
            .map_err(|e| anyhow::anyhow!("Error parsing response: {}", e))?;
        Ok(response.access_token)
    }

    pub fn print_beatmap_info_gui(beatmapset: &Beatmapset) -> BeatmapInfo {
        let mut beatmaps = Vec::new();
        for beatmap in &beatmapset.beatmaps {
            beatmaps.push(format!(
                "Difficulty: {:.2} | Mode: {} | Status: {}\nLength: {} min {}s | Version: {}",
                beatmap.difficulty_rating,
                beatmap.mode,
                beatmap.status,
                beatmap.total_length / 60,
                beatmap.total_length % 60,
                beatmap.version
            ));
        }
    
        BeatmapInfo {
            title: beatmapset.title.clone(),
            artist: beatmapset.artist.clone(),
            creator: beatmapset.creator.clone(),
            beatmaps,
        }
    }
    #[tokio::main]
    pub async fn main() -> Result<(), Box<dyn std::error::Error>> {
        let client = Client::new();
        print!("Please enter a song name: ");
        io::stdout().flush()?; // 確保提示符立即顯示

        let mut song_name = String::new();
        io::stdin().read_line(&mut song_name)?;
        let song_name = song_name.trim(); // 移除尾隨換行符

        // 從配置中讀取 client_id 和 client_secret

        let access_token = get_osu_token(&client).await?;
        let beatmapsets = get_beatmapsets(&client, &access_token, song_name).await?;

        // 打印每個 beatmapset 的 ID
        for (index, beatmapset) in beatmapsets.iter().enumerate() {
            println!("{}: Beatmap Set ID: {}", index + 1, beatmapset.id);
            println!("Links: https://osu.ppy.sh/beatmapsets/{}", beatmapset.id);
            println!("-------------------------");
        }

        // 詢問用戶選擇一個 beatmapset
        println!("If you want to check the detail");
        print!("Please enter the item number: ");
        io::stdout().flush()?; // 確保提示符立即顯示

        let mut answer = String::new();
        io::stdin().read_line(&mut answer)?;
        let chosen_index: usize = answer.trim().parse()?;

        // 獲取選定的 beatmapset
        let chosen_beatmapset = &beatmapsets[chosen_index - 1];

        // 打印選定 beatmapset 中的 beatmaps
        for beatmap in &chosen_beatmapset.beatmaps {
            println!("Beatmap ID: {}", beatmap.id);
            println!("Difficulty Rating: {}", beatmap.difficulty_rating);
            println!("Mode: {}", beatmap.mode);
            println!("Status: {}", beatmap.status);
            println!("Total Length: {}", beatmap.total_length / 60);
            println!("User ID: {}", beatmap.user_id);
            println!("Version: {}", beatmap.version);
            println!("-------------------------");
        }

        Ok(())
    }
}
