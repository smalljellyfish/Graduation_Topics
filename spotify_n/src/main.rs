// 引入serde庫的Deserialize特性，用於從JSON等格式中提取數據
use serde::Deserialize;
// 引入標準庫的Error特性，用於處理錯誤
use std::error::Error;
// 引入標準庫的HashMap，用於存儲鍵值對
use std::collections::HashMap;

// 定義一個結構體來存儲認證響應，包括訪問令牌
#[derive(Deserialize)]
struct AuthResponse {
    access_token: String,
}

// 定義一個結構體來存儲藝術家的信息，包括名稱
#[derive(Deserialize)]
struct Artist {
    name: String,
}

// 定義一個結構體來存儲搜索結果，包括音軌列表
#[derive(Deserialize)]
struct SearchResult {
    tracks: Tracks,
}

// 定義一個結構體來存儲音軌列表
#[derive(Deserialize)]
struct Tracks{
    items: Vec<Track>,
}

// 定義一個結構體來存儲音軌的信息，包括名稱、藝術家列表和外部連結
#[derive(Deserialize)]
struct Track {
    name: String,
    artists: Vec<Artist>,
    external_urls: HashMap<String, String>,
}

// 使用tokio庫的main標記來創建一個異步的main函數
#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    // 創建一個新的HTTP客戶端
    let client = reqwest::Client::new();
    // 獲取訪問令牌
    let access_token = get_access_token(&client).await?;

    // 提示用戶輸入歌曲名稱或Spotify的URL
    println!("Enter song name or Spotify URL: ");
    // 創建一個新的字符串來存儲用戶的輸入
    let mut input = String::new();
    // 從標準輸入讀取一行數據
    std::io::stdin().read_line(&mut input).unwrap();
    // 去掉輸入的前後空格
    let input = input.trim();
    // 檢查輸入是否以"http://"、"https://"或"/"開頭
    if input.starts_with("http://") || input.starts_with("https://") || input.starts_with("/"){
        // 如果是，則調用extract_track_id_from_url函數來從URL中提取音軌ID
        fn extract_track_id_from_url(url: &str) -> Option<&str> {
            url.rsplit('/').next()
        }
        
        // 定義一個異步函數來獲取音軌的信息
        async fn get_track_info(client: &reqwest::Client, track_id: &str, access_token: &str) -> Result<Track, Box<dyn std::error::Error>> {
            // 構造請求的URL
            let url = format!("https://api.spotify.com/v1/tracks/{}", track_id);
            // 發送請求並獲取響應
            let response = client.get(&url)
                .header("Authorization", format!("Bearer {}", access_token))
                .send()
                .await?
                .json::<Track>()
                .await?;
            // 返回響應
            Ok(response)
        }
        // 從URL中提取音軌ID
        let track_id = extract_track_id_from_url(input).unwrap();
        // 獲取音軌的信息
        let track_info = get_track_info(&client, track_id, &access_token).await?;
        // 打印音軌的信息
        print_track_infos(vec![track_info]);
    } else {
        // 如果輸入的不是URL，則調用search_track函數來搜索音軌
        let track_infos = search_track(&client, input, &access_token).await?;
        // 打印搜索到的音軌的信息
        print_track_infos(track_infos);
    }

    // 返回Ok結果
    Ok(())
}
// 定義一個函數來打印音軌的信息
fn print_track_infos(track_infos: Vec<Track>) {
    // 打印一個空行
    println!(" ");
    // 打印一條分隔線
    println!("------------------------");
    // 遍歷音軌信息列表
    for track_info in track_infos {
        // 將藝術家列表轉換為藝術家名稱列表
        let artist_names: Vec<String> = track_info.artists.into_iter().map(|artist| artist.name).collect();
        // 打印音軌的名稱
        println!("Track: {}", track_info.name);
        // 打印藝術家的名稱，名稱之間用逗號分隔
        println!("Artists: {}", artist_names.join(", "));
        // 如果音軌的外部連結中包含Spotify的URL，則打印該URL
        if let Some(spotify_url) = track_info.external_urls.get("spotify") {
            println!("Spotify URL: {}", spotify_url);
        }
        // 打印一條分隔線
        println!("------------------------");
    }
}
// 定義一個異步函數來搜索音軌
async fn search_track(client: &reqwest::Client, track_name: &str, access_token: &str) -> Result<Vec<Track>, Box<dyn Error>>{
    // 構造搜索的URL
    let search_url = format!("https://api.spotify.com/v1/search?q={}&type=track&limit=10", track_name);
    // 發送請求並獲取響應
    let response = client.get(&search_url)
        .header("Authorization", format!("Bearer {}", access_token))
        .send()
        .await?;

    // 將響應的JSON數據解析為SearchResult結構體
    let search_result: SearchResult = response.json().await?;
    // 將搜索結果轉換為音軌信息列表
    let track_infos = search_result.tracks.items.into_iter().map(|track| {
        // 將藝術家列表轉換為藝術家名稱列表
        let artist_names: Vec<String> = track.artists.into_iter().map(|artist| artist.name).collect();
        // 將藝術家名稱列表轉換為藝術家列表
        let artists: Vec<Artist> = artist_names.into_iter().map(|name| Artist { name }).collect();
        // 創建一個新的Track結構體
        Track{
            name: track.name,
            artists,
            external_urls: track.external_urls,
        }
    }).collect();
    // 返回音軌信息列表
    Ok(track_infos)
}

// 定義一個異步函數來獲取訪問令牌
async fn get_access_token(client: &reqwest::Client) -> Result<String, Box<dyn Error>> {
    // 定義客戶端ID
    let client_id = "your_client_id_here";
    // 定義客戶端密鑰
    let client_secret = "your_client_secret_here";

    // 定義認證URL
    let auth_url = "https://accounts.spotify.com/api/token";
    // 定義請求體
    let body = "grant_type=client_credentials";
    // 將客戶端ID和客戶端密鑰編碼為Base64格式
    let auth_header = base64::encode(format!("{}:{}", client_id, client_secret));
    // 創建一個POST請求
    let request = client
        .post(auth_url)
        .header("Authorization", format!("Basic {}", auth_header)) // 添加Authorization頭，值為"Basic "後接Base64編碼的客戶端ID和客戶端密鑰
        .header("Content-Type", "application/x-www-form-urlencoded") // 添加Content-Type頭，值為"application/x-www-form-urlencoded"
        .body(body); // 設置請求體

    // 發送請求並獲取響應
    let response = request.send().await?;
    // 將響應的JSON數據解析為AuthResponse結構體
    let auth_response: AuthResponse = response.json().await?;

    // 返回訪問令牌
    Ok(auth_response.access_token)
}