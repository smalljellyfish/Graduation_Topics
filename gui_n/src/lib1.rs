pub mod spotify_search {
    use anyhow::{Error, Result,Context};
    use regex::Regex;
    use reqwest::header::{AUTHORIZATION, CONTENT_TYPE};
    use serde::{Deserialize, Serialize};
    use std::collections::HashMap;
    use std::fs::File;
    use std::io::Read;
    use lazy_static::lazy_static;
    use log::{info, error};
    use webbrowser;
    use std::process::Command;

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
    #[derive(Deserialize)]
    pub struct Config {
        client_id: String,
        client_secret: String,
    }

    async fn read_config() -> Result<Config> {
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

    lazy_static! {
        static ref SPOTIFY_URL_REGEX: Regex = Regex::new(r"https?://open\.spotify\.com/(track|album)/([a-zA-Z0-9]{22})?")
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
    ) -> Result<Track> {  // 使用anyhow的Result
        let url = format!("https://api.spotify.com/v1/tracks/{}", track_id);
        let response = client
            .get(&url)
            .header("Authorization", format!("Bearer {}", access_token))
            .send()
            .await
            .map_err(Error::from)?;  
    
        let body = response
            .text()
            .await
            .map_err(Error::from)?;  
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
        let client_id = config.client_id;
        let client_secret = config.client_secret;
    
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
            },
            Err(e) => {
                error!("Error sending request for token: {:?}", e);
                Err(e.into())
            }
        }
    }
    pub fn open_spotify_url(url: &str) {
        let track_id = url.split("/").last().unwrap_or_default();
        let spotify_uri = format!("spotify:track:{}", track_id);
    
        
        if Command::new("cmd").args(&["/C", "start", &spotify_uri]).status().is_err() {
            //失敗則在瀏覽器打開
            if webbrowser::open(url).is_ok() {
                info!("Opened URL in browser: {}", url);
            } else {
                error!("Failed to open URL in browser: {}", url);
            }
        } else {
            info!("Opened URL in Spotify App: {}", url);
        }
    }


}