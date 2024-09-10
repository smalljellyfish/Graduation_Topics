// 標準庫導入
use std::sync::Arc;

// 第三方庫導入
use anyhow::Result;
use egui::{ColorImage, TextureHandle};
use image::load_from_memory;
use log::{debug, error, info};
use regex::Regex;
use reqwest::Client;
use serde::Deserialize;
use thiserror::Error;
use tokio::{sync::mpsc::Sender, try_join};

// 本地模組導入
use crate::read_config;

#[derive(Debug, Deserialize, Clone)]
pub struct Covers {
    pub cover: Option<String>,
    pub cover_2x: Option<String>,
    pub card: Option<String>,
    pub card_2x: Option<String>,
    pub list: Option<String>,
    pub list_2x: Option<String>,
    pub slimcover: Option<String>,
    pub slimcover_2x: Option<String>,
}
#[derive(Debug, Deserialize, Clone)] // 添加 Clone
pub struct Beatmapset {
    pub beatmaps: Vec<Beatmap>,
    pub id: i32,
    pub artist: String,
    pub title: String,
    pub creator: String,
    pub covers: Covers,
}
#[derive(Deserialize)]
pub struct TokenResponse {
    access_token: String,
}

#[derive(Debug, Deserialize)]
pub struct SearchResponse {
    beatmapsets: Vec<Beatmapset>,
}
#[derive(Debug, Deserialize, Clone)]
pub struct Beatmap {
    pub difficulty_rating: f32,
    pub id: i32,
    pub mode: String,
    pub status: String,
    pub total_length: i32,
    pub user_id: i32,
    pub version: String,
}
pub struct BeatmapInfo {
    pub title: String,
    pub artist: String,
    pub creator: String,
    pub beatmaps: Vec<String>,
}

#[derive(Error, Debug)]
pub enum OsuError {
    #[error("請求錯誤: {0}")]
    RequestError(#[from] reqwest::Error),
    #[error("JSON 解析錯誤: {0}")]
    JsonError(#[from] serde_json::Error),
    #[error("配置錯誤: {0}")]
    ConfigError(String),
    #[error("其他錯誤: {0}")]
    Other(String),
}

pub async fn get_beatmapsets(
    client: &Client,
    access_token: &str,
    song_name: &str,
    debug_mode: bool,
) -> Result<Vec<Beatmapset>, OsuError> {
    let response = client
        .get("https://osu.ppy.sh/api/v2/beatmapsets/search")
        .query(&[("query", song_name)])
        .bearer_auth(access_token)
        .send()
        .await
        .map_err(OsuError::RequestError)?;

    let response_text = response.text().await.map_err(OsuError::RequestError)?;

    if debug_mode {
        info!("Osu API 回應 JSON: {}", response_text);
    }

    let search_response: SearchResponse =
        serde_json::from_str(&response_text).map_err(OsuError::JsonError)?;

    Ok(search_response.beatmapsets)
}


pub async fn get_beatmapset_details(
    client: &Client,
    access_token: &str,
    beatmapset_id: &str,
    debug_mode: bool,
) -> Result<(String, String), OsuError> {
    let url = format!("https://osu.ppy.sh/api/v2/beatmapsets/{}", beatmapset_id);

    let response = client
        .get(&url)
        .bearer_auth(access_token)
        .send()
        .await
        .map_err(OsuError::RequestError)?;

    let beatmapset: serde_json::Value = response.json().await.map_err(OsuError::RequestError)?;

    if debug_mode {
        println!("Beatmapset details: {:?}", beatmapset);
    }

    let (artist, title) = try_join!(
        async {
            Ok::<_, OsuError>(
                beatmapset["artist"]
                    .as_str()
                    .unwrap_or("Unknown Artist")
                    .to_string(),
            )
        },
        async {
            Ok::<_, OsuError>(
                beatmapset["title"]
                    .as_str()
                    .unwrap_or("Unknown Title")
                    .to_string(),
            )
        }
    )?;

    Ok((artist, title))
}
pub async fn get_osu_token(client: &Client, debug_mode: bool) -> Result<String, OsuError> {
    if debug_mode {
        debug!("開始獲取 Osu token");
    }

    let config = read_config(debug_mode).map_err(|e| {
        error!("讀取配置文件時出錯: {}", e);
        OsuError::ConfigError(format!("Error reading config: {}", e))
    })?;

    let client_id = &config.osu.client_id;
    let client_secret = &config.osu.client_secret;

    if debug_mode {
        debug!("成功讀取 Osu client_id 和 client_secret");
    }

    let url = "https://osu.ppy.sh/oauth/token";
    let params = [
        ("client_id", client_id),
        ("client_secret", client_secret),
        ("grant_type", &"client_credentials".to_string()),
        ("scope", &"public".to_string()),
    ];

    if debug_mode {
        debug!("準備發送 Osu token 請求");
    }

    let response = client.post(url).form(&params).send().await.map_err(|e| {
        error!("發送 Osu token 請求時出錯: {}", e);
        OsuError::RequestError(e)
    })?;

    let token_response: TokenResponse = response.json().await.map_err(|e| {
        error!("解析 Osu token 回應時出錯: {}", e);
        OsuError::RequestError(e)
    })?;

    if debug_mode {
        debug!("成功獲取 Osu token");
    }

    Ok(token_response.access_token)
}

impl Beatmapset {
    pub fn format_info(&self) -> BeatmapInfo {
        let beatmaps = self.beatmaps.iter().map(|b| b.format_info()).collect();
        BeatmapInfo {
            title: self.title.clone(),
            artist: self.artist.clone(),
            creator: self.creator.clone(),
            beatmaps,
        }
    }
}

impl Beatmap {
    pub fn format_info(&self) -> String {
        format!(
            "Difficulty: {:.2} | Mode: {} | Status: {}\nLength: {} min {}s | Version: {}",
            self.difficulty_rating,
            self.mode,
            self.status,
            self.total_length / 60,
            self.total_length % 60,
            self.version
        )
    }
}

pub fn print_beatmap_info_gui(beatmapset: &Beatmapset) -> BeatmapInfo {
    beatmapset.format_info()
}
pub fn parse_osu_url(url: &str) -> Option<(String, Option<String>)> {
    let beatmapset_regex =
        Regex::new(r"https://osu\.ppy\.sh/beatmapsets/(\d+)(?:#(\w+)/(\d+))?$").unwrap();

    if let Some(captures) = beatmapset_regex.captures(url) {
        let beatmapset_id = captures.get(1).unwrap().as_str().to_string();
        let beatmap_id = captures.get(3).map(|m| m.as_str().to_string());
        Some((beatmapset_id, beatmap_id))
    } else {
        None
    }
}
pub async fn load_osu_covers(
    urls: Vec<(usize, String)>,
    ctx: egui::Context,
    sender: Sender<(usize, Arc<TextureHandle>, (f32, f32))>,
) -> Result<(), OsuError> {
    let client = Client::new();
    let mut errors = Vec::new();

    for (index, url) in urls.into_iter() {
        debug!("正在載入封面，URL: {}", url);
        match client.get(&url).send().await {
            Ok(response) => {
                if response.status().is_success() {
                    match response.bytes().await {
                        Ok(bytes) => match load_from_memory(&bytes) {
                            Ok(image) => {
                                debug!("成功從記憶體載入圖片，URL: {}", url);
                                let color_image = ColorImage::from_rgba_unmultiplied(
                                    [image.width() as usize, image.height() as usize],
                                    &image.to_rgba8(),
                                );
                                let texture = ctx.load_texture(
                                    format!("cover_{}", index),
                                    color_image,
                                    Default::default(),
                                );
                                let texture = Arc::new(texture);
                                let size = (image.width() as f32, image.height() as f32);
                                if let Err(e) = sender.send((index, texture, size)).await {
                                    error!("發送紋理失敗，URL: {}, 錯誤: {:?}", url, e);
                                    errors
                                        .push(format!("發送紋理失敗，URL: {}, 錯誤: {:?}", url, e));
                                } else {
                                    debug!("成功發送紋理，URL: {}", url);
                                }
                            }
                            Err(e) => {
                                error!("從記憶體載入圖片失敗，URL: {}, 錯誤: {:?}", url, e);
                                errors.push(format!(
                                    "從記憶體載入圖片失敗，URL: {}, 錯誤: {:?}",
                                    url, e
                                ));
                            }
                        },
                        Err(e) => {
                            error!("從回應獲取位元組失敗，URL: {}, 錯誤: {:?}", url, e);
                            errors
                                .push(format!("從回應獲取位元組失敗，URL: {}, 錯誤: {:?}", url, e));
                        }
                    }
                } else {
                    error!("載入封面失敗，URL: {}, 狀態碼: {}", url, response.status());
                    errors.push(format!(
                        "載入封面失敗，URL: {}, 狀態碼: {}",
                        url,
                        response.status()
                    ));
                }
            }
            Err(e) => {
                error!("發送請求失敗，URL: {}, 錯誤: {:?}", url, e);
                errors.push(format!("發送請求失敗，URL: {}, 錯誤: {:?}", url, e));
            }
        }
    }

    if errors.is_empty() {
        Ok(())
    } else {
        Err(OsuError::Other(errors.join("\n")))
    }
}

