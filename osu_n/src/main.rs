use reqwest::Client;
use serde::Deserialize;
use std::error::Error;
use std::io::{self, Write};

#[derive(Debug, Deserialize)]
struct Beatmap {
    // title: String,
    difficulty_rating: f32,
    id: i32,
    mode: String,
    status: String,
    total_length: i32,
    user_id: i32,
    version: String,
}
#[derive(Deserialize)]
struct TokenResponse {
    access_token: String,
}

#[derive(Debug, Deserialize)]
struct SearchResponse {
    beatmapsets: Vec<Beatmapset>,
}
#[derive(Debug, Deserialize)]
struct Beatmapset {
    beatmaps: Vec<Beatmap>,
    id: i32,
}

async fn get_beatmapsets(
    client: &Client,
    access_token: &str,
    song_name: &str,
) -> Result<Vec<Beatmapset>, Box<dyn std::error::Error>> {
    let response = client
        .get("https://osu.ppy.sh/api/v2/beatmapsets/search")
        .query(&[("query", song_name)])
        .bearer_auth(access_token)
        .send()
        .await?
        .json::<SearchResponse>()
        .await?;

    Ok(response.beatmapsets)
}

async fn get_token(
    client: &Client,
    client_id: &str,
    client_secret: &str,
) -> Result<String, Box<dyn Error>> {
    let url = "https://osu.ppy.sh/oauth/token";
    let params = [
        ("client_id", client_id),
        ("client_secret", client_secret),
        ("grant_type", "client_credentials"),
        ("scope", "public"),
    ];
    let response: TokenResponse = client.post(url).form(&params).send().await?.json().await?;
    Ok(response.access_token)
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let client = Client::new();
    let client_id = "31199";
    let client_secret = "XmEuMwBV2SmpmFRHYd75u05gJzQuL72pTA6gbFq4";


    print!("Please enter a song name: ");
    io::stdout().flush()?; // Make sure the prompt is immediately displayed

    let mut song_name = String::new();
    io::stdin().read_line(&mut song_name)?;
    let song_name = song_name.trim(); // Remove trailing newline

    let access_token = get_token(&client, client_id, &client_secret).await?;
    let beatmapsets = get_beatmapsets(&client, &access_token, song_name).await?;
    // Print the ID of each beatmapset
    for (index, beatmapset) in beatmapsets.iter().enumerate() {
        println!("{}: Beatmap Set ID: {}", index + 1, beatmapset.id);
        println!("Links: https://osu.ppy.sh/beatmapsets/{}", beatmapset.id);
        println!("-------------------------");

    }

    // Ask the user to choose a beatmapset
    println!("If you want to check the detail");
    print!("Please enter the item number: ");
    io::stdout().flush()?; // Make sure the prompt is immediately displayed

    let mut answer = String::new();
    io::stdin().read_line(&mut answer)?;
    let chosen_index: usize = answer.trim().parse()?;

    // Get the chosen beatmapset
    let chosen_beatmapset = &beatmapsets[chosen_index - 1];

    // Print the beatmaps in the chosen beatmapset
    for beatmap in &chosen_beatmapset.beatmaps {
        // println!("Song Name: {}", beatmap.title);
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
