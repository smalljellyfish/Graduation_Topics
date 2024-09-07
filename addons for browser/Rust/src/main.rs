use actix_web::{web, App, HttpServer, Responder, HttpResponse};
use actix_cors::Cors;
use serde::Deserialize;
use reqwest::Client;
use serde_json::{json, Value};

#[derive(Deserialize)]
struct YouTubeData {
    title: String,
    description: String,
}

async fn receive_data(data: web::Json<YouTubeData>) -> impl Responder {
    println!("收到 YouTube 標題: {}", data.title);
    println!("收到 YouTube 描述: {}", data.description);
    
    // 調用 GPT API 分析標題和描述
    match analyze_data(&data.title, &data.description).await {
        Ok(result) => HttpResponse::Ok().body(result),
        Err(_) => HttpResponse::InternalServerError().body("處理數據時出錯")
    }
}

async fn analyze_data(title: &str, description: &str) -> Result<String, Box<dyn std::error::Error>> {
    let client = Client::new();
    let response = client.post("https://goapi.gptnb.ai/v1/chat/completions")
        .header("Authorization", "Bearer sk-9ryxBSu9JB3qPi9G422c5f59A37048F682A5Cd2a0d0e5670")
        .json(&json!({
            "model": "gpt-4o-mini-2024-07-18",
            "messages": [
                {"role": "system", "content": "你是一個音樂專家，請分析給定的 YouTube 標題和描述並推測可能的歌曲名稱和歌手。請僅回覆格式為「歌手 - 歌曲名」，不要有其他文字。如果有多個歌手，用逗號分隔。如果無法判斷，請回覆「無法判斷」。如果可以判斷出歌手或歌曲名其中之一，也請回覆。若標題內含有Cover字樣，請判斷cover歌手是誰，並回覆cover歌手 - 歌曲名(原唱歌手)。"},
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