use actix_web::{web, App, HttpServer, Responder, HttpResponse};
use actix_cors::Cors;
use serde::Deserialize;
use reqwest::Client;
use serde_json::{json, Value};

#[derive(Deserialize)]
struct TitleData {
    title: String,
}

async fn receive_title(title: web::Json<TitleData>) -> impl Responder {
    println!("收到 YouTube 標題: {}", title.title);
    
    // 調用 GPT API 分析標題
    match analyze_title(&title.title).await {
        Ok(result) => HttpResponse::Ok().body(result),
        Err(_) => HttpResponse::InternalServerError().body("處理標題時出錯")
    }
}

async fn analyze_title(title: &str) -> Result<String, Box<dyn std::error::Error>> {
    let client = Client::new();
    let response = client.post("https://goapi.gptnb.ai/v1/chat/completions")
        .header("Authorization", "Bearer sk-9ryxBSu9JB3qPi9G422c5f59A37048F682A5Cd2a0d0e5670")
        .json(&json!({
            "model": "claude-3-5-sonnet-20240620",
            "messages": [
                {"role": "system", "content": "你是一個音樂專家，請分析給定的 YouTube 標題並推測可能的歌曲名稱和歌手。請僅回覆格式為「歌手 - 歌曲名」，不要有其他文字。如果有多個歌手，用逗號分隔。如果無法判斷，請回覆「無法判斷」。如果可以判斷出歌手或歌取名其中之一，也請reply"},
                {"role": "user", "content": format!("請分析這個 YouTube 標題並推測可能的歌曲名稱和歌手：{}", title)}
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
            .route("/title", web::post().to(receive_title))
    })
    .bind("127.0.0.1:8000")?
    .run()
    .await
}