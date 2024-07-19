/*use spotify_search_lib::spotify_search::{
    get_access_token, get_track_info, is_valid_spotify_url, print_album_info, print_track_infos,
    search_album_by_name, search_album_by_url, search_track,
}; */
//上方為lib1裡的相關函數
// 引入所需模組
use lib::osu_search::{
    get_beatmapset_by_id, get_beatmapset_details, get_beatmapsets, get_osu_token,
    print_beatmap_info_gui, Beatmapset,
};
use lib::read_config;
use lib::spotify_search::{
    get_access_token, get_track_info, is_valid_spotify_url, open_spotify_url,
    open_url_default_browser, print_track_info_gui, search_track, Album, Artist, CurrentlyPlaying,
    Image, SpotifyUrlStatus, Track, TrackInfo, TrackWithCover,
};

use anyhow::{anyhow, Result};
use clipboard::{ClipboardContext, ClipboardProvider};
use eframe::{self, egui};
use egui::TextureWrapMode;
use egui::ViewportBuilder;
use egui::{ColorImage, TextureHandle};
use egui::{FontData, FontDefinitions, FontFamily};
use tokio;

use log::{debug, error, info, LevelFilter};
use reqwest::Client;
use simplelog::*;

use image::load_from_memory;

use std::default::Default;

use std::sync::atomic::{AtomicBool, Ordering};

use std::sync::{Arc, Mutex};

use std::future::Future;
use std::pin::Pin;
use std::time::{Duration, Instant};
use tokio::time::timeout;

use std::collections::HashMap;

//use tokio::sync::Mutex as AsyncMutex;
use tokio::sync::RwLock;
use tokio::task::JoinHandle;

use tokio::sync::mpsc::Sender;

use std::env;

use regex::Regex;


use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;
use url::Url;

use rspotify::model::PlayableItem;
use rspotify::scopes;
use rspotify::AuthCodeSpotify;

use rspotify::{
    clients::OAuthClient, model::CurrentlyPlayingContext, prelude::*, Credentials, OAuth, Token,
};

//錯誤應用
struct ErrorApp {
    error: String,
}

impl ErrorApp {
    fn new(error: String) -> Self {
        Self { error }
    }
}
impl eframe::App for ErrorApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        egui::CentralPanel::default().show(ctx, |ui| {
            ui.heading("Error");
            ui.label(&self.error);
        });
    }
}
// 定義 SpotifySearchApp結構，儲存程式狀態和數據
struct SearchApp {
    client: Arc<tokio::sync::Mutex<Client>>,
    access_token: Arc<tokio::sync::Mutex<String>>,
    search_query: String,
    search_results: Arc<tokio::sync::Mutex<Vec<Track>>>,
    osu_search_results: Arc<tokio::sync::Mutex<Vec<Beatmapset>>>,
    error_message: Arc<tokio::sync::Mutex<String>>,
    initialized: bool,
    is_searching: Arc<AtomicBool>,
    need_repaint: Arc<AtomicBool>,
    global_font_size: f32,
    show_settings: bool,
    show_relax_window: bool,
    relax_slider_value: i32,
    selected_beatmapset: Option<usize>,
    err_msg: Arc<tokio::sync::Mutex<String>>,
    receiver: Option<tokio::sync::mpsc::Receiver<(usize, Arc<TextureHandle>, (f32, f32))>>,
    cover_textures: Arc<RwLock<HashMap<usize, Option<(Arc<TextureHandle>, (f32, f32))>>>>,
    sender: Sender<(usize, Arc<TextureHandle>, (f32, f32))>,
    texture_cache: Arc<RwLock<HashMap<String, Arc<TextureHandle>>>>,
    texture_load_queue: Arc<Mutex<Vec<String>>>,
    config_errors: Arc<Mutex<Vec<String>>>,
    debug_mode: bool,
    spotify_icon: Option<egui::TextureHandle>,
    spotify_client: Arc<Mutex<Option<AuthCodeSpotify>>>,
    currently_playing: Arc<Mutex<Option<CurrentlyPlaying>>>,
    last_update: Arc<Mutex<Option<Instant>>>,
    spotify_authorized: Arc<AtomicBool>,
}

impl eframe::App for SearchApp {
    fn update(&mut self, ctx: &egui::Context, frame: &mut eframe::Frame) {
        // 請求更新介面，用於刷新GUI
        if self.need_repaint.load(Ordering::SeqCst) {
            ctx.request_repaint();
            self.need_repaint.store(false, Ordering::SeqCst);
        }

        // 初始化程式,和設置字體及獲取access token
        if !self.initialized {
            let client = self.client.clone();
            let osu_urls = vec![];
            let sender_clone = self.sender.clone();
            let ctx_clone = ctx.clone();
            tokio::spawn(async move {
                load_all_covers(osu_urls.clone(), ctx_clone, sender_clone).await;
            });

            let mut receiver = self.receiver.take().expect("Receiver already taken");
            let cover_textures = self.cover_textures.clone();
            let need_repaint_clone = self.need_repaint.clone();

            tokio::spawn(async move {
                while let Some((id, texture, dimensions)) = receiver.recv().await {
                    let mut textures = cover_textures.write().await;
                    textures.insert(id, Some((texture, dimensions)));
                    need_repaint_clone.store(true, Ordering::SeqCst);
                }
            });

            self.initialized = true;

            let access_token = self.access_token.clone();
            let error_message = self.error_message.clone();
            let client_clone = client.clone();
            let debug_mode = self.debug_mode;
            let is_searching = self.is_searching.clone();
            let need_repaint = self.need_repaint.clone();

            tokio::spawn(async move {
                let client_guard = client_clone.lock().await;
                match get_access_token(&*client_guard, debug_mode).await {
                    Ok(token) => {
                        let mut access_token_guard = access_token.lock().await;
                        *access_token_guard = token;
                    }
                    Err(e) => {
                        let mut error = error_message.lock().await;
                        *error = "Spotify 錯誤：無法獲取 token".to_string();
                        error!("獲取 Spotify token 錯誤: {:?}", e);
                        is_searching.store(false, Ordering::SeqCst);
                        need_repaint.store(true, Ordering::SeqCst);
                    }
                }
            });

            let mut fonts = FontDefinitions::default();
            let font_data = include_bytes!("jf-openhuninn-2.0.ttf");

            fonts.font_data.insert(
                "jf-openhuninn".to_owned(),
                FontData::from_owned(font_data.to_vec()),
            );

            if let Some(family) = fonts.families.get_mut(&FontFamily::Proportional) {
                family.insert(0, "jf-openhuninn".to_owned());
            }
            if let Some(family) = fonts.families.get_mut(&FontFamily::Monospace) {
                family.insert(0, "jf-openhuninn".to_owned());
            }

            ctx.set_fonts(fonts);

            let ctx_clone = ctx.clone();
            let err_msg_clone = self.err_msg.clone();
            tokio::spawn(async move {
                let err_msg = err_msg_clone.lock().await;
                if !err_msg.is_empty() {
                    ctx_clone.request_repaint();
                    egui::Window::new("Error").show(&ctx_clone, |ui| {
                        ui.label(&err_msg.to_string());
                    });
                }
            });
        }

        let mut should_close_error = false;

        if let Ok(errors) = self.config_errors.lock() {
            if !errors.is_empty() {
                egui::Window::new("")
                    .collapsible(false)
                    .resizable(false)
                    .default_size(egui::vec2(1200.0, 600.0))
                    .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
                    .show(ctx, |ui| {
                        ui.vertical_centered(|ui| {
                            ui.add_space(20.0);
                            ui.heading(egui::RichText::new("配置檢查錯誤：").size(32.0));
                            ui.add_space(20.0);

                            for error_msg in errors.iter() {
                                for error_line in error_msg.split('\n') {
                                    egui::Frame::none()
                                        .fill(egui::Color32::from_rgb(255, 200, 200))
                                        .show(ui, |ui| {
                                            ui.add_space(10.0);
                                            ui.label(
                                                egui::RichText::new(error_line)
                                                    .size(24.0)
                                                    .color(egui::Color32::RED),
                                            );
                                            ui.add_space(10.0);
                                        });
                                    ui.add_space(5.0);
                                }
                            }

                            ui.add_space(20.0);
                            if ui
                                .add_sized(
                                    [200.0, 60.0],
                                    egui::Button::new(egui::RichText::new("確定").size(40.0)),
                                )
                                .clicked()
                            {
                                should_close_error = true;
                            }
                        });
                    });
            }
        }

        // 在閉包外部處理錯誤視窗的關閉
        if should_close_error {
            if let Ok(mut errors) = self.config_errors.lock() {
                errors.clear();
            }
        }

        egui::CentralPanel::default().show(ctx, |ui| {
            ui.set_max_width(ui.available_width());
            ui.set_max_height(ui.available_height());

            let window_size = ui.available_size();

            ui.horizontal(|ui| {
                if ui.button("⚙").clicked() {
                    self.show_settings = !self.show_settings;
                }
                ui.label(format!(
                    "Window size: {} x {}",
                    window_size.x as i32, window_size.y as i32
                ));
                ui.heading(
                    egui::RichText::new("Search for a song:").size(self.global_font_size * 1.3),
                );
                ui.add_space(5.0);
            });

            if self.show_settings {
                self.show_settings(ui);
            }

            ui.horizontal(|ui| {
                let available_width = ui.available_width();
                let text_edit_width = available_width * 0.95;
                let text_edit_height = self.global_font_size * 2.2;

                let frame = egui::Frame::none()
                    .fill(ui.visuals().extreme_bg_color)
                    .inner_margin(egui::Margin::same(4.0))
                    .rounding(egui::Rounding::same(2.0));

                frame.show(ui, |ui| {
                    ui.horizontal(|ui| {
                        let text_edit = egui::TextEdit::singleline(&mut self.search_query)
                            .font(egui::FontId::proportional(self.global_font_size * 1.1))
                            .margin(egui::vec2(5.0, 0.0))
                            .desired_width(text_edit_width - self.global_font_size * 2.2)
                            .vertical_align(egui::Align::Center);

                        let text_edit_response = ui.add_sized(
                            egui::vec2(
                                text_edit_width - self.global_font_size * 2.2,
                                text_edit_height,
                            ),
                            text_edit,
                        );

                        if !self.search_query.is_empty() {
                            if ui
                                .add_sized(
                                    egui::vec2(self.global_font_size * 2.2, text_edit_height),
                                    egui::Button::new(
                                        egui::RichText::new("×").size(self.global_font_size * 1.3),
                                    )
                                    .frame(false),
                                )
                                .clicked()
                            {
                                self.search_query.clear();
                            }
                        }

                        let cloned_response = text_edit_response.clone();
                        // 搜索框的右鍵菜單
                        cloned_response.context_menu(|ui| {
                            let search_query =
                                Arc::new(std::sync::Mutex::new(self.search_query.clone()));
                            let show_relax_window =
                                Arc::new(std::sync::Mutex::new(self.show_relax_window));

                            let search_query_clone = Arc::clone(&search_query);
                            let show_relax_window_clone = Arc::clone(&show_relax_window);

                            self.create_context_menu(ui, |add_button| {
                                let search_query = Arc::clone(&search_query_clone);
                                add_button(
                                    "Paste",
                                    Box::new(move || {
                                        let mut ctx: ClipboardContext =
                                            ClipboardProvider::new().unwrap();
                                        if let Ok(clipboard_contents) = ctx.get_contents() {
                                            let mut search_query = search_query.lock().unwrap();
                                            *search_query = clipboard_contents;
                                        }
                                    }),
                                );

                                let show_relax_window = Arc::clone(&show_relax_window_clone);
                                add_button(
                                    "Relax",
                                    Box::new(move || {
                                        let mut show_relax_window =
                                            show_relax_window.lock().unwrap();
                                        *show_relax_window = true;
                                    }),
                                );
                            });

                            // 在這裡更新 self 的狀態
                            self.search_query = search_query.lock().unwrap().clone();
                            self.show_relax_window = *show_relax_window.lock().unwrap();
                        });

                        // 檢測Enter是否按下，並處理調試模式
                        if text_edit_response.lost_focus()
                            && ui.input(|i| i.key_pressed(egui::Key::Enter))
                        {
                            if self.search_query.trim().to_lowercase() == "debug" {
                                self.debug_mode = !self.debug_mode; // 切換調試模式
                                self.search_query.clear(); // 清空搜索框
                            } else {
                                self.perform_search(ctx.clone());
                            }
                        }
                        // 顯示調試模式狀態
                        if self.debug_mode {
                            ui.add_space(5.0);
                            ui.label(
                                egui::RichText::new("debug mode on")
                                    .color(egui::Color32::YELLOW)
                                    .size(self.global_font_size),
                            );
                        }
                    });
                });
            });
            let text_style = egui::TextStyle::Body.resolve(ui.style());
            let mut new_text_style = text_style.clone();
            new_text_style.size = self.global_font_size;
            ui.style_mut()
                .text_styles
                .insert(egui::TextStyle::Body, new_text_style);

            if let Ok(err_msg_guard) = self.err_msg.try_lock() {
                ui.label(format!("{}", *err_msg_guard));
            }
            // 根據視窗大小決定佈局
            if window_size.x >= 1000.0 {
                // 大視窗佈局
                ui.columns(2, |columns| {
                    // Spotify 結果
                    columns[0].vertical(|ui| {
                        ui.set_min_width(0.45 * window_size.x);
                        if let Some(icon) = &self.spotify_icon {
                            let size = egui::vec2(50.0, 50.0);
                            ui.add(
                                egui::Image::new(egui::load::SizedTexture::new(icon.id(), size))
                                    .tint(egui::Color32::WHITE)
                                    .bg_fill(egui::Color32::TRANSPARENT),
                            );
                        }
                        ui.add_space(5.0);
                        self.display_spotify_results(ui);
                    });

                    // Osu 結果
                    columns[1].vertical(|ui| {
                        ui.set_min_width(0.45 * window_size.x);
                        ui.heading(
                            egui::RichText::new("Osu Results").size(self.global_font_size * 1.2),
                        );
                        self.display_osu_results(ui);
                    });
                });
            } else {
                // 小視窗佈局（折疊式）
                egui::CollapsingHeader::new(
                    egui::RichText::new("Spotify Results").size(self.global_font_size * 1.1),
                )
                .default_open(true)
                .show(ui, |ui| {
                    self.display_spotify_results(ui);
                });

                egui::CollapsingHeader::new(
                    egui::RichText::new("Osu Results").size(self.global_font_size * 1.1),
                )
                .default_open(true)
                .show(ui, |ui| {
                    self.display_osu_results(ui);
                });
            }
        });

        if self.show_relax_window {
            egui::CentralPanel::default().show(ctx, |ui| {
                ui.vertical_centered(|ui| {
                    ui.heading("Waste your time");

                    let slider = egui::Slider::new(&mut self.relax_slider_value, 0..=999_999_999)
                        .clamp_to_range(true)
                        .text("我不知道這是做啥");
                    ui.add_sized([ui.available_width(), 20.0], slider);

                    ui.label(format!("Value: {}", self.relax_slider_value));

                    if ui.button("Close").clicked() {
                        self.show_relax_window = false;
                    }
                });
            });
        }

        if self.search_query.trim().to_lowercase() == "debug" {
            self.debug_mode = !self.debug_mode; // 切換調試模式
            self.set_log_level(); // 更新日誌級別
            self.search_query.clear(); // 清空搜索框
            info!("Debug mode: {}", self.debug_mode);
        }
        egui::TopBottomPanel::bottom("bottom_panel").show(ctx, |ui| {
            ui.horizontal(|ui| {
                self.render_bottom_panel(ui);
        
                // 在底部面板右側添加授權按鈕
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.add_space(10.0); // 添加一些右側間距
                    let button_text = if self.spotify_authorized.load(Ordering::SeqCst) {
                        "Spotify 已授權"
                    } else {
                        "授權 Spotify"
                    };
                    if ui.button(egui::RichText::new(button_text).size(14.0)).clicked() {
                        if !self.spotify_authorized.load(Ordering::SeqCst) {
                            let spotify_client = self.spotify_client.clone();
                            let debug_mode = self.debug_mode;
                            let spotify_authorized = self.spotify_authorized.clone();
                            let ctx = ctx.clone();
        
                            tokio::spawn(async move {
                                match Self::authorize_spotify(spotify_client, debug_mode).await {
                                    Ok(()) => {
                                        info!("Spotify 授權成功");
                                        spotify_authorized.store(true, Ordering::SeqCst);
                                    }
                                    Err(e) => {
                                        error!("Spotify 授權失敗: {:?}", e);
                                    }
                                }
                                ctx.request_repaint();
                            });
                        }
                    }
                });
            });
        });

        if self.should_update_current_playing() {
            let spotify_client = self.spotify_client.clone();
            let currently_playing = self.currently_playing.clone();
            let debug_mode = self.debug_mode;
            let ctx = ctx.clone();
            let spotify_authorized = self.spotify_authorized.clone();
        
            tokio::spawn(async move {
                match Self::update_currently_playing_wrapper(
                    spotify_client.clone(),
                    currently_playing.clone(),
                    debug_mode,
                )
                .await
                {
                    Ok(_) => {
                        spotify_authorized.store(true, Ordering::SeqCst);
                    }
                    Err(e) => {
                        error!("更新當前播放失敗: {:?}", e);
                        if e.to_string().contains("Token 無效") || e.to_string().contains("需要重新授權") {
                            info!("Token 無效或過期，需要重新授權");
                            spotify_authorized.store(false, Ordering::SeqCst);
                        }
                    }
                }
        
                ctx.request_repaint();
            });
        }
    }
}
async fn load_all_covers(
    urls: Vec<String>,
    ctx: egui::Context,
    sender: Sender<(usize, Arc<TextureHandle>, (f32, f32))>,
) {
    let client = Client::new();
    for (index, url) in urls.into_iter().enumerate() {
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
                                match sender.send((index, texture, size)).await {
                                    Ok(_) => debug!("成功發送紋理，URL: {}", url),
                                    Err(e) => error!("發送紋理失敗，URL: {}, 錯誤: {:?}", url, e),
                                }
                            }
                            Err(e) => error!("從記憶體載入圖片失敗，URL: {}, 錯誤: {:?}", url, e),
                        },
                        Err(e) => error!("從回應獲取位元組失敗，URL: {}, 錯誤: {:?}", url, e),
                    }
                } else {
                    error!("載入封面失敗，URL: {}, 狀態碼: {}", url, response.status());
                }
            }
            Err(e) => error!("發送請求失敗，URL: {}, 錯誤: {:?}", url, e),
        }
    }
}

fn parse_osu_url(url: &str) -> Option<(String, Option<String>)> {
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
impl SearchApp {
    fn new(
        client: Arc<tokio::sync::Mutex<Client>>,
        sender: Sender<(usize, Arc<TextureHandle>, (f32, f32))>,
        receiver: tokio::sync::mpsc::Receiver<(usize, Arc<TextureHandle>, (f32, f32))>,
        cover_textures: Arc<RwLock<HashMap<usize, Option<(Arc<TextureHandle>, (f32, f32))>>>>,
        need_repaint: Arc<AtomicBool>,
        ctx: egui::Context,
        config_errors: Arc<Mutex<Vec<String>>>,
        debug_mode: bool,
    ) -> Result<Self> {
        let texture_cache: Arc<RwLock<HashMap<String, Arc<TextureHandle>>>> =
            Arc::new(RwLock::new(HashMap::new()));
        let texture_load_queue = Arc::new(Mutex::new(Vec::<String>::new()));

        let texture_cache_clone = Arc::clone(&texture_cache);
        let texture_load_queue_clone = Arc::clone(&texture_load_queue);
        let need_repaint_clone = Arc::clone(&need_repaint);
        let ctx_clone = ctx.clone();

        let spotify_icon = Self::load_spotify_icon(&ctx);
        let config = read_config(debug_mode)?;

        let creds = Credentials::new(&config.spotify.client_id, &config.spotify.client_secret);
        let mut oauth = OAuth::default();
        oauth.redirect_uri = "http://localhost:8888/callback".to_string();
        oauth.scopes = scopes!("user-read-currently-playing");

        let spotify = AuthCodeSpotify::new(creds, oauth.clone());
        let spotify_client = Arc::new(Mutex::new(Some(spotify)));

        // 啟動異步加載任務
        tokio::spawn(async move {
            loop {
                let url = {
                    let mut queue = texture_load_queue_clone.lock().unwrap();
                    queue.pop()
                };

                if let Some(url) = url {
                    if !texture_cache_clone.read().await.contains_key(&url) {
                        if let Some(texture) = Self::load_texture_async(&ctx_clone, &url).await {
                            texture_cache_clone
                                .write()
                                .await
                                .insert(url, Arc::new(texture));
                            need_repaint_clone.store(true, Ordering::SeqCst);
                        }
                    }
                }

                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            }
        });

        Ok(Self {
            client,
            access_token: Arc::new(tokio::sync::Mutex::new(String::new())),
            search_query: String::new(),
            search_results: Arc::new(tokio::sync::Mutex::new(Vec::new())),
            osu_search_results: Arc::new(tokio::sync::Mutex::new(Vec::new())),
            error_message: Arc::new(tokio::sync::Mutex::new(String::new())),
            initialized: false,
            is_searching: Arc::new(AtomicBool::new(false)),
            need_repaint,
            global_font_size: 16.0,
            show_relax_window: false,
            relax_slider_value: 0,
            selected_beatmapset: None,
            err_msg: Arc::new(tokio::sync::Mutex::new(String::new())),
            cover_textures,
            sender,
            receiver: Some(receiver),
            texture_cache,
            texture_load_queue,
            config_errors,
            debug_mode,
            spotify_icon,
            show_settings: false,
            spotify_client,
            currently_playing: Arc::new(Mutex::new(None)),
            last_update: Arc::new(Mutex::new(None)),
            spotify_authorized: Arc::new(AtomicBool::new(false)),
        })
    }

    //設置日誌級別
    fn set_log_level(&self) {
        let log_level = if self.debug_mode {
            LevelFilter::Debug
        } else {
            LevelFilter::Info
        };
        log::set_max_level(log_level);
    }

    fn show_settings(&mut self, ui: &mut egui::Ui) {
        ui.heading("設置");
        ui.add_space(10.0);

        // 整體縮放設置
        ui.horizontal(|ui| {
            ui.label("整體縮放:");
            if ui.button("-").clicked() {
                let new_scale = (ui.ctx().pixels_per_point() - 0.1).max(0.5);
                ui.ctx().set_pixels_per_point(new_scale);
            }
            ui.label(format!("{:.2}", ui.ctx().pixels_per_point()));
            if ui.button("+").clicked() {
                let new_scale = (ui.ctx().pixels_per_point() + 0.1).min(3.0);
                ui.ctx().set_pixels_per_point(new_scale);
            }
        });

        ui.add_space(10.0);

        // Debug 模式設置
        let mut debug_mode = self.debug_mode;
        ui.checkbox(&mut debug_mode, "Debug Mode");
        if debug_mode != self.debug_mode {
            self.debug_mode = debug_mode;
            self.set_log_level();
            info!("Debug mode: {}", self.debug_mode);
        }

        ui.add_space(10.0);
    }

    fn should_update_current_playing(&self) -> bool {
        let mut last_update = self.last_update.lock().unwrap();
        if last_update.is_none() || last_update.unwrap().elapsed() > Duration::from_secs(30) {
            *last_update = Some(Instant::now());
            true
        } else {
            false
        }
    }

    async fn update_current_playing(
        spotify: &AuthCodeSpotify,
        currently_playing: Arc<Mutex<Option<CurrentlyPlaying>>>,
        debug_mode: bool,
    ) -> Result<Option<CurrentlyPlaying>> {
        match spotify.current_user_playing_item().await {
            Ok(Some(playing_context)) => {
                if let Some(PlayableItem::Track(track)) = playing_context.item {
                    let artists = track
                        .artists
                        .iter()
                        .map(|a| Artist {
                            name: a.name.clone(),
                        })
                        .collect::<Vec<_>>();
                    let track_info = TrackInfo {
                        name: track.name,
                        artists: artists
                            .iter()
                            .map(|a| a.name.clone())
                            .collect::<Vec<_>>()
                            .join(", "),
                        album: track.album.name,
                    };
                    let spotify_url = track.external_urls.get("spotify").cloned();
                    let new_currently_playing = CurrentlyPlaying {
                        track_info,
                        spotify_url,
                    };
                    Ok(Some(new_currently_playing))
                } else {
                    Ok(None)
                }
            }
            Ok(None) => Ok(None),
            Err(e) => {
                error!("獲取當前播放信息時發生錯誤: {:?}", e);
                Err(anyhow!("獲取當前播放信息失敗"))
            }
        }
    }

    async fn update_currently_playing_wrapper(
        spotify_client: Arc<Mutex<Option<AuthCodeSpotify>>>,
        currently_playing: Arc<Mutex<Option<CurrentlyPlaying>>>,
        debug_mode: bool,
    ) -> Result<()> {
        let spotify_ref = {
            let spotify = spotify_client.lock().unwrap();
            spotify.as_ref().cloned()
        };
    
        let update_result = if let Some(spotify) = spotify_ref {
            Self::update_current_playing(&spotify, currently_playing.clone(), debug_mode).await
        } else {
            Err(anyhow!("Spotify 客戶端未初始化"))
        };
    
        match update_result {
            Ok(Some(new_currently_playing)) => {
                let mut currently_playing = currently_playing.lock().unwrap();
                *currently_playing = Some(new_currently_playing);
                Ok(())
            }
            Ok(None) => {
                let mut currently_playing = currently_playing.lock().unwrap();
                *currently_playing = None;
                Ok(())
            }
            Err(e) => {
                if e.to_string().contains("InvalidToken") {
                    error!("Token 無效，需要重新授權");
                    return Err(anyhow!("Token 無效，需要重新授權"));
                } else {
                    error!("更新當前播放失敗: {:?}", e);
                    Err(e)
                }
            }
        }
    }

    fn authorize_spotify(
        spotify_client: Arc<Mutex<Option<AuthCodeSpotify>>>,
        debug_mode: bool,
    ) -> Pin<Box<dyn Future<Output = Result<()>> + Send>> {
        Box::pin(async move {
            let config = read_config(debug_mode)?;

            let client_id = &config.spotify.client_id;
            let redirect_uri = "http://localhost:8888/callback";
            let scope = "user-read-currently-playing";

            let mut url = Url::parse("https://accounts.spotify.com/authorize").unwrap();
            url.query_pairs_mut()
                .append_pair("client_id", client_id)
                .append_pair("response_type", "code")
                .append_pair("redirect_uri", redirect_uri)
                .append_pair("scope", scope)
                .append_pair("show_dialog", "true");

            let auth_url = url.to_string();

            if debug_mode {
                info!("Authorization URL: {}", auth_url);
            }

            open_url_default_browser(&auth_url)?;

            println!("請在瀏覽器中完成授權...");

            // 啟動本地伺服器來捕獲回調
            let listener = TcpListener::bind("127.0.0.1:8888").expect("無法啟動本地伺服器");

            // 等待回調
            let (mut stream, _) = listener.accept().expect("無法接受連接");
            let mut reader = BufReader::new(&stream);
            let mut request_line = String::new();
            reader.read_line(&mut request_line).expect("無法讀取請求");

            let redirect_url = request_line
                .split_whitespace()
                .nth(1)
                .ok_or_else(|| anyhow!("無效的請求"))?;
            let url = format!("http://localhost:8888{}", redirect_url);

            // 向瀏覽器發送響應
            let response = "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=UTF-8\r\n\r\n<html><body>授權成功，請關閉此窗口。</body></html>";
            stream.write_all(response.as_bytes()).expect("無法發送響應");

            if debug_mode {
                info!("Received callback URL: {}", url);
            }

            println!("正在處理授權...");

            let parsed_url = Url::parse(&url)?;
            let code = parsed_url
                .query_pairs()
                .find(|(key, _)| key == "code")
                .map(|(_, value)| value.into_owned())
                .ok_or_else(|| anyhow!("無法從回調 URL 中解析授權碼"))?;

            // 當獲取到授權碼後，使用 client_id 和 client_secret 請求訪問令牌
            let token_url = "https://accounts.spotify.com/api/token";
            let client = reqwest::Client::new();
            let params = [
                ("grant_type", "authorization_code"),
                ("code", &code),
                ("redirect_uri", "http://localhost:8888/callback"),
            ];

            match timeout(
                Duration::from_secs(30),
                client
                    .post(token_url)
                    .basic_auth(
                        &config.spotify.client_id,
                        Some(&config.spotify.client_secret),
                    )
                    .form(&params)
                    .send(),
            )
            .await
            {
                Ok(response_result) => {
                    let response = response_result?;
                    if response.status().is_success() {
                        let token_data: Token = response.json().await?;
                        println!("成功獲取訪問令牌！");

                        // 創建新的 AuthCodeSpotify 實例
                        let creds = Credentials::new(
                            &config.spotify.client_id,
                            &config.spotify.client_secret,
                        );
                        let oauth = OAuth {
                            redirect_uri: "http://localhost:8888/callback".to_string(),
                            scopes: scopes!("user-read-currently-playing"),
                            ..Default::default()
                        };

                        // 使用 from_token_with_config 方法，並使用完全限定的路徑
                        let new_spotify = AuthCodeSpotify::from_token_with_config(
                            token_data,
                            creds,
                            oauth,
                            rspotify::Config::default(), // 使用 rspotify::Config 而不是 Config
                        );

                        // 更新 spotify_client
                        let mut client = spotify_client
                            .lock()
                            .map_err(|_| anyhow!("無法獲取 Spotify 客戶端鎖"))?;
                        *client = Some(new_spotify);

                        println!("Spotify 授權成功！");
                    } else {
                        return Err(anyhow!("獲取訪問令牌失敗: {}", response.status()));
                    }
                }
                Err(_) => return Err(anyhow!("請求訪問令牌超時")),
            }

            Ok(())
        })
    }

    fn render_bottom_panel(&self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            let current_playing = self.currently_playing.lock().unwrap();
            if let Some(current_playing) = current_playing.as_ref() {
                if let Some(spotify_icon) = &self.spotify_icon {
                    let size = egui::vec2(24.0, 24.0);
                    ui.add(egui::Image::new(egui::load::SizedTexture::new(
                        spotify_icon.id(),
                        size,
                    )));
                }
                ui.label(format!(
                    "正在播放: {} - {}",
                    current_playing.track_info.artists, current_playing.track_info.name
                ));
            } else {
                ui.label("當前沒有正在播放的曲目");
            }
        });
    }

    fn load_spotify_icon(ctx: &egui::Context) -> Option<egui::TextureHandle> {
        let is_dark = ctx.style().visuals.dark_mode;

        let icon_name = if is_dark {
            "spotify_icon_black.png"
        } else {
            "spotify_icon_black.png"
        };

        // 獲取可執行文件的目錄
        let exe_dir = std::env::current_exe().ok()?;
        let exe_dir = exe_dir.parent()?;

        // icon 資料夾與 exe 檔在同一目錄
        let icon_dir = exe_dir.join("icon");

        // 構建圖標的絕對路徑
        let icon_path = icon_dir.join(icon_name);

        println!("Trying to load icon from: {:?}", icon_path);

        match Self::load_image_from_path(&icon_path) {
            Ok(image) => {
                let texture_options = egui::TextureOptions {
                    magnification: egui::TextureFilter::Linear,
                    minification: egui::TextureFilter::Linear,
                    wrap_mode: egui::TextureWrapMode::ClampToEdge,
                };
                Some(ctx.load_texture("spotify_icon", image, texture_options))
            }
            Err(e) => {
                eprintln!("Failed to load Spotify icon ({}): {:?}", icon_name, e);
                // 嘗試加載另一個圖標作為備用
                let fallback_icon_name = if is_dark {
                    "spotify_icon_black.png"
                } else {
                    "spotify_icon.png"
                };
                let fallback_icon_path = icon_dir.join(fallback_icon_name);

                println!(
                    "Trying to load fallback icon from: {:?}",
                    fallback_icon_path
                );

                match Self::load_image_from_path(&fallback_icon_path) {
                    Ok(fallback_image) => {
                        Some(ctx.load_texture("spotify_icon", fallback_image, Default::default()))
                    }
                    Err(e) => {
                        eprintln!("無法載入備用 Spotify 圖標：{:?}", e);
                        None
                    }
                }
            }
        }
    }
    fn create_context_menu<F>(&self, ui: &mut egui::Ui, content: F)
    where
        F: FnOnce(&mut dyn FnMut(&str, Box<dyn FnOnce() + '_>)),
    {
        ui.style_mut()
            .text_styles
            .iter_mut()
            .for_each(|(__, font_id)| {
                font_id.size = self.global_font_size * 1.2;
            });

        ui.style_mut().spacing.item_spacing.y = 5.0; // 減少項目間的垂直間距

        ui.vertical_centered(|ui| {
            ui.add_space(5.0);

            let button_width = ui.available_width().max(100.0);
            let button_height = 30.0;

            content(&mut |label: &str, on_click: Box<dyn FnOnce() + '_>| {
                if ui
                    .add_sized(
                        [button_width, button_height],
                        egui::Button::new(
                            egui::RichText::new(label)
                                .size(self.global_font_size * 1.2)
                                .text_style(egui::TextStyle::Button),
                        ),
                    )
                    .clicked()
                {
                    on_click();
                    ui.close_menu();
                }
            });

            ui.add_space(5.0);
        });
    }

    // 輔助函數來加載圖片
    fn load_image_from_path(path: &std::path::Path) -> Result<egui::ColorImage, image::ImageError> {
        let image = image::io::Reader::open(path)?.decode()?;
        let size = [image.width() as _, image.height() as _];
        let image_buffer = image.into_rgba8();
        let pixels = image_buffer.as_flat_samples();

        // 手動處理透明度
        let mut color_image = egui::ColorImage::new(size, egui::Color32::TRANSPARENT);
        for (i, pixel) in pixels.as_slice().chunks_exact(4).enumerate() {
            let [r, g, b, a] = pixel else { continue };
            if *a > 0 {
                color_image.pixels[i] = egui::Color32::from_rgba_unmultiplied(*r, *g, *b, *a);
            }
        }

        Ok(color_image)
    }
    async fn load_texture_async(ctx: &egui::Context, url: &str) -> Option<TextureHandle> {
        let bytes = reqwest::get(url).await.ok()?.bytes().await.ok()?;
        let image = image::load_from_memory(&bytes).ok()?;
        let size = [image.width() as _, image.height() as _];
        let image_buffer = image.to_rgba8();
        let pixels = image_buffer.as_flat_samples();
        let color_image = egui::ColorImage::from_rgba_unmultiplied(size, pixels.as_slice());
        let texture_options = egui::TextureOptions {
            magnification: egui::TextureFilter::Linear,
            minification: egui::TextureFilter::Linear,
            wrap_mode: TextureWrapMode::default(), // 使用默認值
        };
        Some(ctx.load_texture(url, color_image, texture_options))
    }

    fn perform_search(&mut self, ctx: egui::Context) -> JoinHandle<Result<()>> {
        self.set_log_level(); // 設置日誌級別

        let client = self.client.clone();
        let debug_mode = self.debug_mode;
        let query = self.search_query.clone();
        let search_results = self.search_results.clone();
        let osu_search_results = self.osu_search_results.clone();
        let is_searching = self.is_searching.clone();
        let need_repaint = self.need_repaint.clone();
        let err_msg = self.err_msg.clone();
        let sender = self.sender.clone();

        info!("使用者搜尋: {}", query);

        is_searching.store(true, Ordering::SeqCst);

        tokio::spawn(async move {
            let result: Result<()> = async {
                let mut error = err_msg.lock().await;
                error.clear();
                if debug_mode {
                    debug!("除錯模式開啟");
                }

                let spotify_token = get_access_token(&*client.lock().await, debug_mode)
                    .await
                    .map_err(|e| {
                        error!("獲取 Spotify token 錯誤: {:?}", e);
                        anyhow!("Spotify 錯誤：無法獲取 token")
                    })?;

                let osu_token = get_osu_token(&*client.lock().await, debug_mode)
                    .await
                    .map_err(|e| {
                        error!("獲取 Osu token 錯誤: {:?}", e);
                        anyhow!("Osu 錯誤：無法獲取 token")
                    })?;

                if let Some((beatmapset_id, _)) = parse_osu_url(&query) {
                    info!("Osu 搜尋: {}", query);

                    // 如果是 osu! URL，獲取譜面信息並進行反搜索
                    let (artist, title) = get_beatmapset_details(
                        &*client.lock().await,
                        &osu_token,
                        &beatmapset_id,
                        debug_mode,
                    )
                    .await
                    .map_err(|e| {
                        error!("獲取 Osu 譜面詳情錯誤: {:?}", e);
                        anyhow!("Osu 錯誤：獲取譜面詳情失敗")
                    })?;

                    let spotify_query = format!("{} {}", artist, title);
                    info!("Spotify 查詢 (從 osu): {}", spotify_query);

                    // 使用獲取的 artist 和 title 進行 Spotify 搜索
                    let tracks_with_cover = search_track(
                        &*client.lock().await,
                        &spotify_query,
                        &spotify_token,
                        10,
                        0,
                        debug_mode,
                    )
                    .await
                    .map(|(tracks_with_cover, _)| tracks_with_cover)
                    .map_err(|e| {
                        error!("Spotify 反搜索錯誤: {:?}", e);
                        anyhow!("Spotify 錯誤：反搜索失敗")
                    })?;

                    // 更新 Spotify 搜索結果
                    let mut search_results = search_results.lock().await;
                    *search_results = tracks_with_cover
                        .iter()
                        .map(|twc| Track {
                            name: twc.name.clone(),
                            artists: twc.artists.clone(),
                            album: Album {
                                name: twc.album_name.clone(),
                                album_type: String::new(),
                                artists: Vec::new(),
                                external_urls: HashMap::new(),
                                images: twc
                                    .cover_url
                                    .as_ref()
                                    .map(|url| {
                                        vec![Image {
                                            url: url.clone(),
                                            width: 0,
                                            height: 0,
                                        }]
                                    })
                                    .unwrap_or_default(),
                                id: String::new(),
                                release_date: String::new(),
                                total_tracks: 0,
                            },
                            external_urls: twc.external_urls.clone(),
                        })
                        .collect();

                    // 獲取 osu! beatmapset
                    let beatmapset = get_beatmapset_by_id(
                        &*client.lock().await,
                        &osu_token,
                        &beatmapset_id,
                        debug_mode,
                    )
                    .await
                    .map_err(|e| {
                        error!("獲取 Osu 譜面錯誤: {:?}", e);
                        anyhow!("Osu 錯誤：獲取譜面失敗")
                    })?;

                    let results = vec![beatmapset];
                    *osu_search_results.lock().await = results.clone();

                    let mut osu_urls = Vec::new();
                    if let Some(cover_url) = &results[0].covers.cover {
                        osu_urls.push(cover_url.clone());
                    }

                    let ctx_clone = ctx.clone();
                    let sender_clone = sender.clone();
                    tokio::spawn(async move {
                        load_all_covers(osu_urls, ctx_clone, sender_clone).await;
                    });
                } else {
                    // 如果不是 osu! URL，執行原有的搜索邏輯
                    let spotify_result: Result<Vec<TrackWithCover>> =
                        match is_valid_spotify_url(&query) {
                            SpotifyUrlStatus::Valid => {
                                info!("Spotify 查詢 (URL): {}", query);
                                let track_id = query
                                    .split('/')
                                    .last()
                                    .unwrap_or("")
                                    .split('?')
                                    .next()
                                    .unwrap_or("");
                                let track =
                                    get_track_info(&*client.lock().await, track_id, &spotify_token)
                                        .await
                                        .map_err(|e| anyhow!("獲取曲目資訊錯誤: {:?}", e))?;

                                Ok(vec![TrackWithCover {
                                    name: track.name.clone(),
                                    artists: track.artists.clone(),
                                    external_urls: track.external_urls.clone(),
                                    album_name: track.album.name.clone(),
                                    cover_url: track
                                        .album
                                        .images
                                        .first()
                                        .map(|img| img.url.clone()),
                                }])
                            }
                            SpotifyUrlStatus::Incomplete => {
                                *error = "Spotify URL 不完整，請輸入完整的 URL".to_string();
                                return Ok(());
                            }
                            SpotifyUrlStatus::Invalid => {
                                if !query.is_empty() {
                                    info!("Spotify 查詢 (關鍵字): {}", query);
                                    let limit = 10;
                                    let offset = 0;
                                    search_track(
                                        &*client.lock().await,
                                        &query,
                                        &spotify_token,
                                        limit,
                                        offset,
                                        debug_mode,
                                    )
                                    .await
                                    .map(|(tracks_with_cover, _)| tracks_with_cover)
                                } else {
                                    Ok(Vec::new())
                                }
                            }
                        };

                    let osu_query = match spotify_result {
                        Ok(ref tracks_with_cover) => {
                            info!("Spotify 搜索結果: {} 首曲目", tracks_with_cover.len());
                            let mut search_results = search_results.lock().await;
                            *search_results = tracks_with_cover
                                .iter()
                                .map(|twc| Track {
                                    name: twc.name.clone(),
                                    artists: twc.artists.clone(),
                                    album: Album {
                                        name: twc.album_name.clone(),
                                        album_type: String::new(),
                                        artists: Vec::new(),
                                        external_urls: HashMap::new(),
                                        images: twc
                                            .cover_url
                                            .as_ref()
                                            .map(|url| {
                                                vec![Image {
                                                    url: url.clone(),
                                                    width: 0,
                                                    height: 0,
                                                }]
                                            })
                                            .unwrap_or_default(),
                                        id: String::new(),
                                        release_date: String::new(),
                                        total_tracks: 0,
                                    },
                                    external_urls: twc.external_urls.clone(),
                                })
                                .collect();

                            if matches!(is_valid_spotify_url(&query), SpotifyUrlStatus::Valid)
                                && !tracks_with_cover.is_empty()
                            {
                                let osu_query = format!(
                                    "{} {}",
                                    tracks_with_cover[0]
                                        .artists
                                        .iter()
                                        .map(|a| a.name.clone())
                                        .collect::<Vec<_>>()
                                        .join(", "),
                                    tracks_with_cover[0].name
                                );
                                info!("Osu 查詢 (從 Spotify): {}", osu_query);
                                osu_query
                            } else {
                                info!("Osu 查詢 (關鍵字): {}", query);
                                query.clone()
                            }
                        }
                        Err(e) => {
                            error!("Spotify 搜索錯誤: {:?}", e);
                            return Err(anyhow!("Spotify 錯誤：搜索失敗"));
                        }
                    };

                    let results =
                        get_beatmapsets(&*client.lock().await, &osu_token, &osu_query, debug_mode)
                            .await
                            .map_err(|e| {
                                error!("Osu 搜索錯誤: {:?}", e);
                                anyhow!("Osu 錯誤：搜索失敗")
                            })?;

                    info!("Osu 搜索結果: {} 個 beatmapsets", results.len());
                    if debug_mode {
                        debug!("Osu 搜索結果詳情: {:?}", results);
                    }

                    let mut osu_urls = Vec::new();
                    for beatmapset in &results {
                        if let Some(cover_url) = &beatmapset.covers.cover {
                            osu_urls.push(cover_url.clone());
                        }
                    }
                    *osu_search_results.lock().await = results;
                    let ctx_clone = ctx.clone();
                    let sender_clone = sender.clone();

                    tokio::spawn(async move {
                        load_all_covers(osu_urls, ctx_clone, sender_clone).await;
                    });
                }

                Ok(())
            }
            .await;

            if let Err(e) = &result {
                let mut error = err_msg.lock().await;
                *error = e.to_string();
            }

            is_searching.store(false, Ordering::SeqCst);
            need_repaint.store(true, Ordering::SeqCst);
            result
        })
    }
    fn display_spotify_results(&self, ui: &mut egui::Ui) {
        ui.push_id("spotify_results", |ui| {
            egui::ScrollArea::vertical().show(ui, |ui| {
                if let Ok(search_results_guard) = self.search_results.try_lock() {
                    if !search_results_guard.is_empty() {
                        for track in search_results_guard.iter() {
                            ui.horizontal(|ui| {
                                ui.set_min_height(100.0); // 增加最小高度
                                                          // 顯示專輯封面
                                if let Some(cover_url) =
                                    &track.album.images.first().map(|img| &img.url)
                                {
                                    let texture_cache = self.texture_cache.clone();
                                    let texture_load_queue = self.texture_load_queue.clone();

                                    if let Ok(cache) = texture_cache.try_read() {
                                        if let Some(texture) = cache.get(*cover_url) {
                                            let size = egui::Vec2::new(100.0, 100.0); // 增加圖片大小
                                            ui.add(egui::Image::new(
                                                egui::load::SizedTexture::new(texture.id(), size),
                                            ));
                                        } else {
                                            if let Ok(mut queue) = texture_load_queue.lock() {
                                                if !queue.contains(cover_url) {
                                                    queue.push(cover_url.to_string());
                                                }
                                            }
                                            ui.add_sized(
                                                [100.0, 100.0],
                                                egui::Label::new(
                                                    egui::RichText::new("Loading...")
                                                        .size(self.global_font_size)
                                                        .text_style(egui::TextStyle::Monospace)
                                                        .color(egui::Color32::LIGHT_GRAY),
                                                ),
                                            )
                                            .on_hover_text("Loading album cover");
                                        }
                                    };
                                    ui.add_space(10.0);
                                }

                                ui.vertical(|ui| {
                                    let (track_info, spotify_url) = print_track_info_gui(track);

                                    // 顯示曲目名稱
                                    ui.label(
                                        egui::RichText::new(&track_info.name)
                                            .strong()
                                            .size(self.global_font_size * 1.2),
                                    );

                                    // 顯示藝術家
                                    ui.label(
                                        egui::RichText::new(&track_info.artists)
                                            .size(self.global_font_size),
                                    );

                                    // 顯示專輯名稱
                                    ui.label(
                                        egui::RichText::new(&track_info.album)
                                            .size(self.global_font_size * 0.9),
                                    );

                                    // 添加點擊和拖動的響應
                                    let response = ui.allocate_rect(
                                        ui.min_rect(),
                                        egui::Sense::click_and_drag(),
                                    );

                                    // 雙擊
                                    if response.double_clicked() {
                                        if let Some(url) = &spotify_url {
                                            if let Err(e) = open_spotify_url(url) {
                                                log::error!("Failed to open URL: {}", e);
                                            }
                                        }
                                    }

                                    // 右鍵菜單
                                    // Spotify 結果的右鍵菜單
                                    response.context_menu(|ui| {
                                        self.create_context_menu(ui, |add_button| {
                                            if let Some(url) = &spotify_url {
                                                add_button(
                                                    "🔗 Copy",
                                                    Box::new({
                                                        let url = url.clone();
                                                        move || {
                                                            let mut ctx: ClipboardContext =
                                                                ClipboardProvider::new().unwrap();
                                                            ctx.set_contents(url.clone()).unwrap();
                                                        }
                                                    }),
                                                );
                                                add_button(
                                                    "Open",
                                                    Box::new({
                                                        let url = url.clone();
                                                        move || {
                                                            if let Err(e) = open_spotify_url(&url) {
                                                                log::error!(
                                                                    "Failed to open URL: {}",
                                                                    e
                                                                );
                                                            }
                                                        }
                                                    }),
                                                );
                                            }
                                        });
                                    });
                                });
                            });

                            ui.add_space(15.0); // 增加間距
                            ui.separator();
                            ui.add_space(15.0);
                        }
                    }
                }
            });
        });
    }
    fn display_osu_results(&mut self, ui: &mut egui::Ui) {
        ui.push_id("osu_results", |ui| {
            egui::ScrollArea::vertical().show(ui, |ui| {
                if let Ok(osu_search_results_guard) = self.osu_search_results.try_lock() {
                    if !osu_search_results_guard.is_empty() {
                        if let Some(selected_index) = self.selected_beatmapset {
                            let selected_beatmapset = &osu_search_results_guard[selected_index];
                            let beatmap_info = print_beatmap_info_gui(selected_beatmapset);

                            ui.heading(
                                egui::RichText::new(format!(
                                    "{} - {}",
                                    beatmap_info.title, beatmap_info.artist
                                ))
                                .font(egui::FontId::proportional(self.global_font_size * 1.1)),
                            );
                            ui.label(
                                egui::RichText::new(format!("by {}", beatmap_info.creator))
                                    .font(egui::FontId::proportional(self.global_font_size * 0.9)),
                            );
                            ui.add_space(10.0);

                            for beatmap_info in beatmap_info.beatmaps {
                                ui.add_space(10.0);
                                ui.label(
                                    egui::RichText::new(beatmap_info).font(
                                        egui::FontId::proportional(self.global_font_size * 1.0),
                                    ),
                                );
                                ui.add_space(10.0);
                                ui.separator();
                            }
                            if ui
                                .add_sized(
                                    [100.0, 40.0],
                                    egui::Button::new(egui::RichText::new("Back").font(
                                        egui::FontId::proportional(self.global_font_size * 1.0),
                                    )),
                                )
                                .clicked()
                            {
                                self.selected_beatmapset = None;
                            }
                        } else {
                            for (index, beatmapset) in osu_search_results_guard.iter().enumerate() {
                                let response = ui.add(
                                    egui::Button::new("")
                                        .frame(false)
                                        .min_size(egui::vec2(ui.available_width(), 100.0)),
                                );

                                if response.clicked() {
                                    self.selected_beatmapset = Some(index);
                                }

                                ui.allocate_ui_at_rect(response.rect, |ui| {
                                    ui.horizontal(|ui| {
                                        ui.vertical(|ui| {
                                            if let Ok(textures) = self.cover_textures.try_read() {
                                                if let Some(Some((texture, size))) =
                                                    textures.get(&index)
                                                {
                                                    let max_height = 100.0;
                                                    let aspect_ratio = size.0 / size.1;
                                                    let image_size = egui::Vec2::new(
                                                        max_height * aspect_ratio,
                                                        max_height,
                                                    );
                                                    ui.image((texture.id(), image_size));
                                                } else {
                                                    ui.label("Loading...");
                                                }
                                            }
                                        });

                                        ui.add_space(10.0);

                                        ui.vertical(|ui| {
                                            ui.label(
                                                egui::RichText::new(&beatmapset.title)
                                                    .font(egui::FontId::proportional(
                                                        self.global_font_size * 1.0,
                                                    ))
                                                    .strong(),
                                            );
                                            ui.label(egui::RichText::new(&beatmapset.artist).font(
                                                egui::FontId::proportional(
                                                    self.global_font_size * 0.9,
                                                ),
                                            ));
                                            ui.label(
                                                egui::RichText::new(format!(
                                                    "by {}",
                                                    beatmapset.creator
                                                ))
                                                .font(egui::FontId::proportional(
                                                    self.global_font_size * 0.8,
                                                )),
                                            );
                                        });
                                    });
                                });

                                ui.add_space(5.0);
                                ui.separator();
                            }
                        }
                    }
                }
            });
        });
    }
}
#[tokio::main]
async fn main() {
    // 初始化日誌
    let log_file = std::fs::File::create("output.log").unwrap();
    let mut config_builder = simplelog::ConfigBuilder::new();
    let result = config_builder.set_time_offset_to_local();
    if let Err(err) = result {
        eprintln!("Failed to set local time offset: {:?}", err);
    }

    let debug_mode = env::var("DEBUG_MODE").unwrap_or_default() == "true"
        || env::args().any(|arg| arg == "--debug");

    let config = config_builder
        .set_target_level(LevelFilter::Error)
        .set_location_level(LevelFilter::Off)
        .set_thread_level(LevelFilter::Off)
        .set_level_padding(LevelPadding::Right)
        .build();
    WriteLogger::init(
        if debug_mode {
            LevelFilter::Debug
        } else {
            LevelFilter::Info
        },
        config,
        log_file,
    )
    .unwrap();

    info!("Welcome");

    // 讀取配置
    let config_result = read_config(debug_mode);
    let config_errors = match config_result {
        Ok(_) => Vec::new(),
        Err(e) => vec![e.to_string()],
    };

    // 將錯誤信息存儲，以便在 GUI 中顯示
    let config_errors = Arc::new(Mutex::new(config_errors));

    // 初始化 HTTP 客戶端
    let client = Arc::new(tokio::sync::Mutex::new(Client::new()));
    let (sender, receiver) = tokio::sync::mpsc::channel(100);

    // 定義 cover_textures
    let cover_textures: Arc<RwLock<HashMap<usize, Option<(Arc<TextureHandle>, (f32, f32))>>>> =
        Arc::new(RwLock::new(HashMap::new()));
    let need_repaint = Arc::new(AtomicBool::new(false));

    let mut native_options = eframe::NativeOptions::default();
    native_options.viewport = ViewportBuilder {
        title: Some(String::from("Search App")),
        inner_size: Some(egui::Vec2::new(730.0, 430.0)),
        min_inner_size: Some(egui::Vec2::new(730.0, 430.0)),
        resizable: Some(true),       // 允許調整視窗大小
        maximize_button: Some(true), // 顯示最大化按鈕
        ..Default::default()
    };

    // 將所有需要的變量移動到閉包內
    let client = client.clone();
    let sender = sender.clone();
    let cover_textures = cover_textures.clone();
    let need_repaint = need_repaint.clone();

    // 運行應用
    eframe::run_native(
        "Search App",
        native_options,
        Box::new(move |cc| {
            let ctx = cc.egui_ctx.clone();
            // 跟隨系統主題
            ctx.set_visuals(if dark_light::detect() == dark_light::Mode::Dark {
                egui::Visuals::dark()
            } else {
                egui::Visuals::light()
            });
            ctx.set_pixels_per_point(1.0);

            match SearchApp::new(
                client.clone(),
                sender,
                receiver,
                cover_textures.clone(),
                need_repaint.clone(),
                ctx,
                config_errors.clone(),
                debug_mode,
            ) {
                Ok(app) => Box::new(app),
                Err(e) => {
                    eprintln!("Failed to create SearchApp: {}", e);
                    Box::new(ErrorApp::new(format!("Failed to create SearchApp: {}", e)))
                }
            }
        }),
    )
    .unwrap_or_else(|e| eprintln!("Error running native app: {}", e));
}
