use regex::Regex;
use serde::Deserialize;
use std::collections::HashMap;
use std::error::Error;
use std::fs::File;
use std::io::Read;

#[derive(Deserialize)]
struct AuthResponse {
    access_token: String,
}

#[derive(Deserialize)]
struct Artist {
    name: String,
}


#[derive(Deserialize)]
struct SearchResult {
    tracks: Tracks,
}

#[derive(Deserialize)]
struct Tracks {
    items: Vec<Track>,
    total: u32, 
}

#[derive(Deserialize)]
struct Track {
    name: String,
    artists: Vec<Artist>,
    external_urls: HashMap<String, String>,
}
#[derive(Deserialize)]
struct Config {
    client_id: String,
    client_secret: String,
}
async fn read_config() -> std::result::Result<Config, Box<dyn std::error::Error>> {
    let mut file = File::open("config.json")?;
    let mut content = String::new();
    file.read_to_string(&mut content)?;
    let config: Config = serde_json::from_str(&content)?;
    Ok(config)
}
fn is_valid_spotify_url(url: &str) -> bool {
    let re = Regex::new(r"(https?://)?open\.spotify\.com/track/([a-zA-Z0-9]{22})").unwrap();
    if let Some(captures) = re.captures(url) {
        if let Some(match_) = captures.get(2) {
            return match_.as_str().len() == 22;
        }
    }
    false
}

#[tokio::main]
async fn main() -> std::result::Result<(), Box<dyn std::error::Error>> {
    let client = reqwest::Client::new();
    let access_token = get_access_token(&client).await?;

    println!("Enter song name or Spotify URL: ");
    let mut input = String::new();
    std::io::stdin().read_line(&mut input).unwrap();
    let input = input.trim();

    if input.starts_with("http://") || input.starts_with("https://") || input.starts_with("open.spotify.com") {
        if !is_valid_spotify_url(input) {
            println!("你疑似輸入了 URL，但它不正確。");
            return Ok(());
        }
        fn extract_track_id_from_url(url: &str) -> Option<&str> {
            url.rsplit('/').next()
        }

        async fn get_track_info(
            client: &reqwest::Client,
            track_id: &str,
            access_token: &str,
        ) -> std::result::Result<Track, Box<dyn std::error::Error>> {
            let url = format!("https://api.spotify.com/v1/tracks/{}", track_id); // 假設這是獲取音軌信息的URL模板
            let response = client
                .get(&url)
                .header("Authorization", format!("Bearer {}", access_token))
                .send()
                .await
                .map_err(|e| -> Box<dyn std::error::Error> { From::from(e) })?;

            let body = response
                .text()
                .await
                .map_err(|e| -> Box<dyn std::error::Error> { From::from(e) })?;
            let track: Track = serde_json::from_str(&body)?;

            Ok(track)
        }
        let track_id = extract_track_id_from_url(input).unwrap();
        let track_info = get_track_info(&client, track_id, &access_token).await?;
        print_track_infos(vec![track_info]);
    } else {
        let limit = 10;
    let mut page = 1; 
    let (track_infos, total_pages) = search_track(&client, input, &access_token, page, limit).await?;
    print_track_infos(track_infos);
    println!("目前在第{}頁，總共{}頁。请输入您要的頁數，或者輸入'exit'退出：", page, total_pages);

    loop {
        let mut action = String::new();
        std::io::stdin().read_line(&mut action).unwrap();
        if action.trim().eq("exit") {
            break;
        } else if let Ok(requested_page) = action.trim().parse::<u32>() {
            if requested_page > 0 && requested_page <= total_pages {
                page = requested_page; // 更新当前页码
                let (track_infos, _) = search_track(&client, input, &access_token, page, limit).await?;
                print_track_infos(track_infos);
                println!("目前在第{}頁，共计{}頁。请输入您要的頁數，或者輸入'exit'退出：", page, total_pages);
            } else {
                println!("輸入的頁數錯誤，請輸入1到{}的數字，或者輸入'exit'退出：", total_pages);
            }
        } else {
            println!("錯誤，請輸入頁數數字，或輸入exit給我滾");
        }
    }
}
    Ok(())
}
fn print_track_infos(track_infos: Vec<Track>) {
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
        if let Some(spotify_url) = track_info.external_urls.get("spotify") {
            println!("Spotify URL: {}", spotify_url);
        }
        println!("------------------------");
    }
}

async fn search_track(
    client: &reqwest::Client,
    track_name: &str,
    access_token: &str,
    page: u32,
    limit: u32,
) -> Result<(Vec<Track>, u32), Box<dyn Error>> { 
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
    let total_pages = (search_result.tracks.total + limit - 1) / limit; // 计算总页数
    let track_infos = search_result
    
        .tracks
        .items
        .into_iter()
        .map(|track| Track {
            name: track.name,
            artists: track.artists,
            external_urls: track.external_urls,
        })
        .collect();
    Ok((track_infos, total_pages))
}

async fn get_access_token(client: &reqwest::Client) -> Result<String, Box<dyn Error>> {
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

    let response = request.send().await?;
    let auth_response: AuthResponse = response.json().await?;

    Ok(auth_response.access_token)
}
