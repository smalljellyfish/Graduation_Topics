use actix_web::{web, App, HttpServer, Responder, HttpResponse};
use actix_cors::Cors;
use reqwest::Client;
use serde_json::{json, Value};
use std::fs;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};
use serde::{Serialize, Deserialize};

#[derive(Deserialize)]
struct YouTubeData {
    title: String,
    description: Option<String>, 
}
#[derive(Serialize)]
struct AnalysisResult {
    result: String,
    cached: bool,
}
#[derive(Serialize, Deserialize)]
struct CacheEntry {
    result: String,
    timestamp: u64,
}
fn get_cache_dir() -> PathBuf {
    let mut path = dirs::cache_dir().unwrap_or(std::env::temp_dir());
    path.push("SongSearch");
    fs::create_dir_all(&path).unwrap();
    path
}

fn cache_key(title: &str, description: &str) -> String {
    use sha2::{Sha256, Digest};
    let mut hasher = Sha256::new();
    hasher.update(title);
    hasher.update(description);
    format!("{:x}", hasher.finalize())
}

async fn get_cached_result(title: &str, description: &str) -> Option<String> {
    let cache_file = get_cache_dir().join(cache_key(title, description));
    if let Ok(content) = fs::read_to_string(cache_file) {
        if let Ok(entry) = serde_json::from_str::<CacheEntry>(&content) {
            let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs();
            if now - entry.timestamp < 3600 { // 1 小時有效期
                return Some(entry.result);
            }
        }
    }
    None
}

async fn set_cached_result(title: &str, description: &str, result: &str) {
    let cache_file = get_cache_dir().join(cache_key(title, description));
    let entry = CacheEntry {
        result: result.to_string(),
        timestamp: SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs(),
    };
    if let Ok(content) = serde_json::to_string(&entry) {
        let _ = fs::write(cache_file, content);
    }
}

async fn receive_data(data: web::Json<YouTubeData>) -> impl Responder {
    println!("收到 YouTube 標題: {}", data.title);
    println!("收到 YouTube 描述: {:?}", data.description);
    
    let description = data.description.as_deref().unwrap_or("");
    
    if let Some(cached_result) = get_cached_result(&data.title, description).await {
        println!("從快取中獲取結果: {}", cached_result);
        return HttpResponse::Ok().json(AnalysisResult {
            result: cached_result,
            cached: true,
        });
    }
    
    match analyze_data(&data.title, description).await {
        Ok(result) => {
            set_cached_result(&data.title, description, &result).await;
            HttpResponse::Ok().json(AnalysisResult {
                result,
                cached: false,
            })
        },
        Err(_) => HttpResponse::InternalServerError().body("處理數據時出錯")
    }
}

async fn analyze_data(title: &str, description: &str) -> Result<String, Box<dyn std::error::Error>> {
    let client = Client::new();
    let response = client.post("https://goapi.gptnb.ai/v1/chat/completions")
        .header("Authorization", "Bearer sk-qHxTKYGqU8sPVd4274279842100e4573B320A5Ac7841F345")
        .json(&json!({
            "model": "gpt-4o-mini-2024-07-18",
            "messages": [
                {"role": "system", "content": "你是一個音樂專家，請分析給定的 YouTube 標題和描述並推測可能的歌曲名稱和歌手。請僅回覆格式為「歌手 - 歌曲名」，不要有其他文字。如果有多個歌手，用逗號分隔。如果無法判斷，請回覆「無法判斷」。如果可以判斷出歌手或歌曲名其中之一，也請回覆。若標題內含有Cover字樣，請判斷cover歌手是誰，並回覆cover歌手 - 歌曲名(原唱歌手)，若標題內未有Cover字樣，請回覆歌手 - 歌曲名"},
                {"role": "user", "content": format!("請分析這個 YouTube 標題和描述並推測可能的歌曲名稱和歌手：\n標題：{}\n描述：{}", title, description)}
            ]
        }))
        .send()
        .await?;

    let body: Value = response.json().await?;
    let result = body["choices"][0]["message"]["content"]
        .as_str()
        .unwrap_or("無法判斷")
        .trim()
        .to_string();

    println!("分析結果: {}", result);
    Ok(result)
}

#[actix_web::main]
async fn main() -> std::io::Result<()> {
    println!("啟動服務器在 http://localhost:8000");
    HttpServer::new(|| {
        let cors = Cors::default()
            .allow_any_origin()
            .allow_any_method()
            .allow_any_header();

        App::new()
            .wrap(cors)
            .route("/data", web::post().to(receive_data))
    })
    .bind("127.0.0.1:8000")?
    .run()
    .await
}