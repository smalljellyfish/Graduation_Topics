use serde::Deserialize;
use std::error::Error;
use std::collections::HashMap;
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
struct Tracks{
    items: Vec<Track>,
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

#[tokio::main]
async fn main() -> std::result::Result<(), Box<dyn std::error::Error>> {
    let client = reqwest::Client::new();
    let access_token = get_access_token(&client).await?;

    println!("Enter song name or Spotify URL: ");
    let mut input = String::new();
    std::io::stdin().read_line(&mut input).unwrap();
    let input = input.trim();
    if input.starts_with("http://") || input.starts_with("https://") || input.starts_with("/"){
        fn extract_track_id_from_url(url: &str) -> Option<&str> {
            url.rsplit('/').next()
        }
        
        async fn get_track_info(client: &reqwest::Client, track_id: &str, access_token: &str) -> std::result::Result<Track, Box<dyn std::error::Error>> {
            let url = format!("https://api.spotify.com/v1/tracks/{}", track_id); // 假設這是獲取音軌信息的URL模板
            let response = client
                .get(&url)
                .header("Authorization", format!("Bearer {}", access_token))
                .send()
                .await
                .map_err(|e| -> Box<dyn std::error::Error> { From::from(e) })?;
        
            let body = response.text().await.map_err(|e| -> Box<dyn std::error::Error> { From::from(e) })?;
            let track: Track = serde_json::from_str(&body)?;
        
            Ok(track)
        }
        
        
        
        let track_id = extract_track_id_from_url(input).unwrap();
        let track_info = get_track_info(&client, track_id, &access_token).await?;
        print_track_infos(vec![track_info]);
    } else {
        let track_infos = search_track(&client, input, &access_token).await?;
        print_track_infos(track_infos);
    }

    Ok(())
}
fn print_track_infos(track_infos: Vec<Track>) {
    println!(" ");
    println!("------------------------");
    for track_info in track_infos {
        let artist_names: Vec<String> = track_info.artists.into_iter().map(|artist| artist.name).collect();
        println!("Track: {}", track_info.name);
        println!("Artists: {}", artist_names.join(", "));
        if let Some(spotify_url) = track_info.external_urls.get("spotify") {
            println!("Spotify URL: {}", spotify_url);
        }
        println!("------------------------");
    }
}
async fn search_track(client: &reqwest::Client, track_name: &str, access_token: &str) -> Result<Vec<Track>, Box<dyn Error>> {
    let search_url = format!("https://api.spotify.com/v1/search?q={}&type=track&limit=10", track_name);
    let response = client.get(&search_url)
        .header("Authorization", format!("Bearer {}", access_token))
        .send()
        .await?;

    let search_result: SearchResult = response.json().await?;
    let track_infos = search_result.tracks.items.into_iter().map(|track| {
        let artist_names: Vec<String> = track.artists.into_iter().map(|artist| artist.name).collect();
    let artists: Vec<Artist> = artist_names.into_iter().map(|name| Artist { name }).collect();
        Track{
            name: track.name,
            artists,
            external_urls: track.external_urls,
        }
    }).collect();
    Ok(track_infos)
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
