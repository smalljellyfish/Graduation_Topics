use anyhow::{Context, Result};
use serde::Deserialize;
use std::fs::File;
use std::io::Read;

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
        .with_context(|| format!("Failed to open config file: {}", file_path))?;
    let mut content = String::new();
    file.read_to_string(&mut content)
        .with_context(|| "Failed to read from config file")?;
    let config: Config = serde_json::from_str(&content)
        .with_context(|| "Failed to parse config file, please check the JSON format")?;
    Ok(config)
}

pub mod spotify_search {
    use anyhow::{Error, Result};
    use lazy_static::lazy_static;
    use log::error;
    use regex::Regex;
    use reqwest::header::{AUTHORIZATION, CONTENT_TYPE};
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
                None => Err("URL疑似錯誤，請重新輸入".into()),
            },
            None => Err("URL疑似錯誤，請重新輸入".into()),
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
    pub fn print_track_info_gui(track: &Track) -> (String, Option<String>, String) {
        let track_name = &track.name;
        let album_name = &track.album.name;
        let artist_names = track
            .artists
            .iter()
            .map(|artist| artist.name.as_str())
            .collect::<Vec<&str>>()
            .join(", ");

        let spotify_url = track.external_urls.get("spotify").cloned();

        let info = format!(
            "Track: {}\nArtists: {}\nAlbum: {}",
            track_name, artist_names, album_name
        );

        (info, spotify_url, track_name.clone())
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
        client: &reqwest::Client,
        track_name: &str,
        access_token: &str,
        page: u32,
        limit: u32,
    ) -> Result<(Vec<Track>, u32)> {
        let offset = (page - 1) * limit;
        let search_url = format!(
            "https://api.spotify.com/v1/search?q={}&type=track&limit={}&offset={}",
            track_name, limit, offset
        );
        let response = client
            .get(&search_url)
            .header("Authorization", format!("Bearer {}", access_token))
            .send()
            .await?;

        let search_result: SearchResult = response.json().await?;
        let total_pages = (search_result.tracks.clone().unwrap().total + limit - 1) / limit;
        let track_infos = search_result
            .tracks
            .unwrap()
            .items
            .into_iter()
            .map(|track| Track {
                name: track.name,
                artists: track.artists,
                external_urls: track.external_urls,
                album: track.album,
            })
            .collect();
        Ok((track_infos, total_pages))
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
    use reqwest::Client;
    use serde::Deserialize;
    use std::io::{self, Write};
    use anyhow::Result;

    #[derive(Debug, Deserialize)]
    pub struct Beatmap {
        // title: String,
        pub difficulty_rating: f32,
        pub id: i32,
        pub mode: String,
        pub status: String,
        pub total_length: i32,
        pub user_id: i32,
        pub version: String,
    }
    #[derive(Deserialize)]
    pub struct TokenResponse {
        access_token: String,
    }

    #[derive(Debug, Deserialize)]
    pub struct SearchResponse {
        beatmapsets: Vec<Beatmapset>,
    }
    #[derive(Debug, Deserialize)]
    pub struct Beatmapset {
        pub beatmaps: Vec<Beatmap>,
        pub id: i32,
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
        let config = read_config().await
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
        let response: TokenResponse = client.post(url)
            .form(&params)
            .send()
            .await
            .map_err(|e| anyhow::anyhow!("Error sending request: {}", e))?
            .json()
            .await
            .map_err(|e| anyhow::anyhow!("Error parsing response: {}", e))?;
        Ok(response.access_token)
    }
    pub fn print_beatmap_info_gui(beatmap: &Beatmap) -> String {
        format!(
            "Beatmap ID: {}\nDifficulty: {:.2}\nMode: {}\nStatus: {}\nLength: {} mins\nVersion: {}",
            beatmap.id,
            beatmap.difficulty_rating,
            beatmap.mode,
            beatmap.status,
            beatmap.total_length / 60,
            beatmap.version
        )
    }
    /*#[tokio::main]
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
    */
}
