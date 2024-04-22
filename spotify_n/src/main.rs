use regex::Regex;
use reqwest::header::{AUTHORIZATION, CONTENT_TYPE};
use serde::{Deserialize, Serialize};

use std::collections::HashMap;
use std::error::Error;
use std::fs::File;
use std::io::Read;

#[derive(Debug, Deserialize, Serialize, Clone)]
struct Album {
    album_type: String,
    total_tracks: u32,
    external_urls: HashMap<String, String>,
    //href: String,
    id: String,
    //images: Vec<Image>,
    name: String,
    release_date: String,
    //release_date_precision: String,
    //restrictions: Option<Restrictions>,
    //#[serde(rename = "type")]
    //album_type_field: String,
    //uri: String,
    artists: Vec<Artist>, 
}
#[derive(Deserialize, Clone)]
struct Albums {
    items: Vec<Album>,
}
#[derive(Debug, Deserialize, Serialize)]
struct Image {
    url: String,
    height: u32,
    width: u32,
}

#[derive(Debug, Deserialize, Serialize)]
struct Restrictions {
    reason: String,
}

#[derive(Deserialize)]
struct AuthResponse {
    access_token: String,
}

#[derive(Deserialize, Serialize, Debug, Clone)]
struct Artist {
    name: String,
}

#[derive(Deserialize)]
struct SearchResult {
    tracks: Option<Tracks>,
    albums: Option<Albums>,
}

#[derive(Deserialize, Clone)]
struct Tracks {
    items: Vec<Track>,
    total: u32,
}

#[derive(Deserialize, Clone)]
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
    let re = Regex::new(r"https?://open\.spotify\.com/(track|album)/([a-zA-Z0-9]{22})?").unwrap();
    if let Some(captures) = re.captures(url) {
        if captures.get(1).map_or(false, |m| m.as_str() == "track") {
            return captures.get(2).map_or(false, |m| m.as_str().len() == 22);
        } else if captures.get(1).map_or(false, |m| m.as_str() == "album") {
            return true;
        }
    }
    false
}

async fn search_album_by_url(
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

    // 在尝试使用album_id之前处理Result
    match album_id_result {
        Ok(album_id) => {
            // 现在album_id是一个String，可以直接使用
            let api_url = format!("https://api.spotify.com/v1/albums/{}", album_id);
            // 使用api_url进行后续操作...
            let response = client
                .get(&api_url)
                .header(AUTHORIZATION, format!("Bearer {}", access_token))
                .header(CONTENT_TYPE, "application/json")
                .send()
                .await?
                .json::<Album>()
                .await?;
    
            Ok(response)
        },
        Err(e) => {
            
            println!("ERROR: {}", e);
            
            Err(e) 
        }
    }
}
// 根据名称搜索专辑
async fn search_album_by_name(
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
#[tokio::main]
async fn main() -> std::result::Result<(), Box<dyn std::error::Error>> {
    let client = reqwest::Client::new();
    let access_token = get_access_token(&client).await?;

    println!("Enter song name or Spotify URL: ");
    let mut input = String::new();
    std::io::stdin().read_line(&mut input).unwrap();
    let input = input.trim();

    if input.starts_with("http://")
        || input.starts_with("https://")
        || input.starts_with("open.spotify.com")
    {
        if !is_valid_spotify_url(input) {
            println!("你疑似輸入了 URL，但它不正確。");
            return Ok(());
        }
        fn extract_track_id_from_url(url: &str) -> Option<&str> {
            url.rsplit('/').next()
        }
        if input.contains("open.spotify.com/track/") {
            let track_id = extract_track_id_from_url(input).unwrap();
            let track_info = get_track_info(&client, track_id, &access_token).await?;
            print_track_infos(vec![track_info]);
            // 这里调用处理歌曲URL的函数
        } else if input.contains("open.spotify.com/album/") {
            // 如果输入包含album URL，直接使用这个URL进行专辑搜索

            // 去除换行
            let album_url = input.trim();

            match search_album_by_url(&client, album_url, &access_token).await {
                Ok(album) => {
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
                Err(e) => println!("搜尋無結果 ， 請檢察網址是否正確 {}", e),
            }
        } else {
            println!("你疑似輸入了 URL，但它不正確。");
        }
    } else {
        println!("請選擇搜尋類型：");
        println!("1. 歌曲");
        println!("2. 專輯");
        let mut choice = String::new();
        std::io::stdin().read_line(&mut choice).unwrap();
        let choice: &str = choice.trim();
        match choice {
            "1" => {
                let limit = 10;
                let mut page = 1;
                let (track_infos, total_pages) =
                    search_track(&client, input, &access_token, page, limit).await?;
                print_track_infos(track_infos);
                println!(
                    "目前在第{}頁，總共{}頁。请输入您要的頁數，或者輸入'exit'退出：",
                    page, total_pages
                );

                loop {
                    let mut action = String::new();
                    std::io::stdin().read_line(&mut action).unwrap();
                    if action.trim().eq("exit") {
                        break;
                    } else if let Ok(requested_page) = action.trim().parse::<u32>() {
                        if requested_page > 0 && requested_page <= total_pages {
                            page = requested_page;
                            let (track_infos, _) =
                                search_track(&client, input, &access_token, page, limit).await?;
                            print_track_infos(track_infos);
                            println!(
                                "目前在第{}頁，共计{}頁。请输入您要的頁數，或者輸入'exit'退出：",
                                page, total_pages
                            );
                        } else {
                            println!(
                                "輸入的頁數錯誤，請輸入1到{}的數字，或者輸入'exit'退出：",
                                total_pages
                            );
                        }
                    } else {
                        println!("錯誤，請輸入頁數數字，或輸入exit給我滾");
                    }
                }
            }

            "2" => {
                let album_name = input; 
                let limit = 20; // 限制每頁顯示最多26張專輯
                let page = 1;
                
                // 这里假设search_album_by_name函数已经正确实现，能够返回正确的总页数
                let (albums, _) = search_album_by_name(&client, album_name, &access_token, page, limit).await?;
            
                // 展示专辑列表和选择提示
                albums.iter().enumerate().for_each(|(index, album)| {
                    println!("{}. {} - {} [{}]", 
                        char::from(b'a' + index as u8), // 将索引转换为字母
                        album.name, 
                        album.artists.iter().map(|a| a.name.as_str()).collect::<Vec<&str>>().join(", "),
                        album.external_urls.get("spotify").unwrap_or(&String::from("无URL"))
                    );
                });
            
                println!("請選擇專輯（a, b, c, ...）或輸入'exit'退出：");
                let mut choice = String::new();
                std::io::stdin().read_line(&mut choice).unwrap();
                let choice = choice.trim().to_lowercase();
                
                if choice == "exit" {
                    return Ok(());
                } else {
                    let index = choice.chars().next().unwrap() as usize - 'a' as usize;
                    if index < albums.len() {
                        let selected_album = &albums[index];
                        
                        println!("---------------------------------------------");
                        println!("專輯名: {}", selected_album.name);
                        println!(
                            "歌手: {}",
                            selected_album.artists.iter().map(|a| a.name.as_str()).collect::<Vec<&str>>().join(", ")
                        );
                        if let Some(url) = selected_album.external_urls.get("spotify") {
                            println!("URL: {}", url);
                        }
                        println!("---------------------------------------------");
                    } else {
                        println!("無效");
                    }
                } }
            _ => {
                
                println!("無效，請輸入1或2");
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
