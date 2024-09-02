// 本地模組
mod osu;
mod spotify;

// 標準庫導入
use std::cmp::Reverse;
use std::collections::BinaryHeap;
use std::collections::HashMap;
use std::default::Default;
use std::env;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

// 第三方庫導入
use anyhow::{anyhow, Context, Result};
use clipboard::{ClipboardContext, ClipboardProvider};
use eframe::{self, egui};
use egui::{
    FontData, FontDefinitions, FontFamily, TextureHandle, TextureWrapMode, ViewportBuilder,
};

use log::{debug, error, info, LevelFilter};
use reqwest::Client;
use rspotify::{scopes, AuthCodeSpotify, Credentials, OAuth};
use simplelog::*;
use thiserror::Error;
use tokio::{
    self,
    net::TcpListener,
    sync::{mpsc::Sender, Mutex as TokioMutex, RwLock},
    task::JoinHandle,
};

// 本地模組導入
use crate::osu::{
    get_beatmapset_by_id, get_beatmapset_details, get_beatmapsets, get_osu_token, load_osu_covers,
    parse_osu_url, print_beatmap_info_gui, Beatmapset,
};
use crate::spotify::{
    authorize_spotify, get_access_token, get_track_info, is_valid_spotify_url, load_spotify_icon,
    open_spotify_url, search_track, update_currently_playing_wrapper, Album, AuthStatus,
    CurrentlyPlaying, Image, SpotifyError, SpotifyUrlStatus, Track, TrackWithCover,
};
use lib::{read_config, set_log_level, ConfigError};

#[derive(Error, Debug)]
pub enum AppError {
    #[error("配置錯誤: {0}")]
    ConfigError(#[from] ConfigError),
    #[error("Spotify 錯誤: {0}")]
    SpotifyError(#[from] spotify::SpotifyError),
    #[error("Osu 錯誤: {0}")]
    OsuError(#[from] osu::OsuError),
    #[error("IO 錯誤: {0}")]
    IoError(#[from] std::io::Error),
    #[error(transparent)]
    AnyhowError(#[from] anyhow::Error),
    #[error("其他錯誤: {0}")]
    Other(String),
}

// 定義 AuthPlatform 列舉，用於標識不同的授權平台
#[derive(Eq, PartialEq, Hash, Debug, Clone)]
pub enum AuthPlatform {
    Spotify,
    // 未來可以添加其他平台
}

#[derive(Clone, Copy)]
enum ButtonType {
    Spotify,
    Osu,
}

// 定義 AuthManager 結構，儲存授權狀態和錯誤記錄
pub struct AuthManager {
    status: Mutex<HashMap<AuthPlatform, AuthStatus>>,
    error_logged: AtomicBool,
}

impl AuthManager {
    pub fn new() -> Self {
        let mut status = HashMap::new();
        status.insert(AuthPlatform::Spotify, AuthStatus::NotStarted);
        Self {
            status: Mutex::new(status),
            error_logged: AtomicBool::new(false),
        }
    }

    pub fn reset(&self, platform: &AuthPlatform) {
        let mut status = self.status.lock().unwrap();
        status.insert(platform.clone(), AuthStatus::NotStarted);
        self.error_logged.store(false, Ordering::Relaxed);
    }

    pub fn update_status(&self, platform: &AuthPlatform, new_status: AuthStatus) {
        let mut status = self.status.lock().unwrap();
        let old_status = status
            .get(platform)
            .cloned()
            .unwrap_or(AuthStatus::NotStarted);
        status.insert(platform.clone(), new_status.clone());

        if let AuthStatus::Failed(ref error) = new_status {
            if !matches!(old_status, AuthStatus::Failed(_))
                && !self.error_logged.load(Ordering::Relaxed)
            {
                error!("{:?} 授權失敗: {}", platform, error);
                self.error_logged.store(true, Ordering::Relaxed);
            }
        }
    }

    pub fn get_status(&self, platform: &AuthPlatform) -> AuthStatus {
        self.status
            .lock()
            .unwrap()
            .get(platform)
            .cloned()
            .unwrap_or(AuthStatus::NotStarted)
    }
}

// 定義 SpotifySearchApp結構，儲存程式狀態和數據
struct SearchApp {
    access_token: Arc<tokio::sync::Mutex<String>>,
    auth_error: Option<String>,
    auth_in_progress: Arc<AtomicBool>,
    auth_manager: Arc<AuthManager>,
    auth_start_time: Option<Instant>,
    avatar_load_handle: Option<tokio::task::JoinHandle<()>>,
    client: Arc<tokio::sync::Mutex<Client>>,
    config_errors: Arc<Mutex<Vec<String>>>,
    cover_textures: Arc<RwLock<HashMap<usize, Option<(Arc<TextureHandle>, (f32, f32))>>>>,
    ctx: egui::Context,
    currently_playing: Arc<Mutex<Option<CurrentlyPlaying>>>,
    should_detect_now_playing: Arc<AtomicBool>,
    debug_mode: bool,
    default_avatar_texture: Option<egui::TextureHandle>,
    displayed_osu_results: usize,
    displayed_spotify_results: usize,
    err_msg: Arc<tokio::sync::Mutex<String>>,
    error_message: Arc<tokio::sync::Mutex<String>>,
    global_font_size: f32,
    initialized: bool,
    is_searching: Arc<AtomicBool>,
    last_update: Arc<Mutex<Option<Instant>>>,
    listener: Arc<TokioMutex<Option<TcpListener>>>,
    need_reload_avatar: Arc<AtomicBool>,
    need_repaint: Arc<AtomicBool>,
    osu_search_results: Arc<tokio::sync::Mutex<Vec<Beatmapset>>>,
    receiver: Option<tokio::sync::mpsc::Receiver<(usize, Arc<TextureHandle>, (f32, f32))>>,
    relax_slider_value: i32,
    scroll_to_top: bool,
    search_query: String,
    search_results: Arc<tokio::sync::Mutex<Vec<Track>>>,
    selected_beatmapset: Option<usize>,
    sender: Sender<(usize, Arc<TextureHandle>, (f32, f32))>,
    show_auth_progress: bool,
    show_relax_window: bool,
    show_settings: bool,
    show_side_menu: bool,
    spotify_authorized: Arc<AtomicBool>,
    spotify_client: Arc<Mutex<Option<AuthCodeSpotify>>>,
    spotify_icon: Option<egui::TextureHandle>,
    spotify_user_avatar: Arc<Mutex<Option<egui::TextureHandle>>>,
    spotify_user_avatar_url: Arc<Mutex<Option<String>>>,
    spotify_user_name: Arc<Mutex<Option<String>>>,
    show_spotify_now_playing: bool,
    texture_cache: Arc<RwLock<HashMap<String, Arc<TextureHandle>>>>,
    texture_load_queue: Arc<Mutex<BinaryHeap<Reverse<(usize, String)>>>>,
    preloaded_icons: HashMap<String, egui::TextureHandle>,
    spotify_search_button_states: HashMap<usize, f32>,
    spotify_open_button_states: HashMap<usize, f32>,
    osu_search_button_states: HashMap<usize, f32>,
    osu_open_button_states: HashMap<usize, f32>,
    side_menu_animation: HashMap<egui::Id, f32>,
}

impl eframe::App for SearchApp {
    fn update(&mut self, ctx: &egui::Context, frame: &mut eframe::Frame) {
        // 初始化程式,和設置字體及獲取access token
        if !self.initialized {
            let client = self.client.clone();
            let osu_urls = vec![];
            let sender_clone = self.sender.clone();
            let ctx_clone = ctx.clone();
            let debug_mode = self.debug_mode;

            tokio::spawn(async move {
                if let Err(e) =
                    load_osu_covers(osu_urls.clone(), ctx_clone.clone(), sender_clone).await
                {
                    error!("初始化時載入 osu 封面發生錯誤: {:?}", e);
                    if debug_mode {
                        ctx_clone.request_repaint();
                        egui::Window::new("Error").show(&ctx_clone, |ui| {
                            ui.label(format!("載入 osu 封面錯誤: {:?}", e));
                        });
                    }
                }
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

        if self.spotify_user_avatar.lock().unwrap().is_none()
            && self.spotify_user_avatar_url.lock().unwrap().is_some()
            && self.need_reload_avatar.load(Ordering::SeqCst)
        {
            info!("觸發加載 Spotify 用戶頭像");
            let url = self
                .spotify_user_avatar_url
                .lock()
                .unwrap()
                .clone()
                .unwrap();
            let ctx_clone = ctx.clone();
            let need_reload_avatar = self.need_reload_avatar.clone();
            let spotify_user_avatar = self.spotify_user_avatar.clone();

            // 如果已經有正在進行的加載任務，先取消它
            if let Some(handle) = self.avatar_load_handle.take() {
                handle.abort();
            }

            // 啟動新的加載任務
            self.avatar_load_handle = Some(tokio::spawn(async move {
                match SearchApp::load_spotify_user_avatar(&url, &ctx_clone).await {
                    Ok(texture) => {
                        info!("Spotify 用戶頭像加載成功");
                        *spotify_user_avatar.lock().unwrap() = Some(texture);
                        need_reload_avatar.store(false, Ordering::SeqCst);
                        ctx_clone.request_repaint();
                    }
                    Err(e) => {
                        error!("加載 Spotify 用戶頭像失敗: {:?}", e);
                    }
                }
            }));
        }

        if self.need_reload_avatar.load(Ordering::SeqCst) {
            let url = self.spotify_user_avatar_url.lock().unwrap().clone();
            if let Some(url) = url {
                let ctx = ctx.clone();
                let spotify_user_avatar = self.spotify_user_avatar.clone();
                let need_reload_avatar = self.need_reload_avatar.clone();

                tokio::spawn(async move {
                    if let Err(e) = Self::load_spotify_avatar(
                        &ctx,
                        &url,
                        spotify_user_avatar,
                        need_reload_avatar,
                    )
                    .await
                    {
                        error!("加載 Spotify 頭像失敷: {:?}", e);
                    }
                });
            }
        }
        // 檢查授權狀態並更新 auth_in_progress
        if !self.auth_in_progress.load(Ordering::SeqCst) {
            match self.auth_manager.get_status(&AuthPlatform::Spotify) {
                AuthStatus::Completed | AuthStatus::Failed(_) => {
                    self.show_auth_progress = false;
                    self.auth_in_progress.store(false, Ordering::SeqCst);
                }
                _ => {}
            }
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

        egui::TopBottomPanel::top("top_panel").show(ctx, |ui| {
            self.render_top_panel(ui);
        });

        self.render_side_menu(ctx);

        self.render_central_panel(ctx);

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
            set_log_level(self.debug_mode); // 更新日誌級別
            self.search_query.clear(); // 清空搜索框
            info!("Debug mode: {}", self.debug_mode);
        }

        if self.should_update_current_playing()
            && self.should_detect_now_playing.load(Ordering::SeqCst)
        {
            let spotify_client = self.spotify_client.clone();
            let currently_playing = self.currently_playing.clone();
            let debug_mode = self.debug_mode;
            let ctx = ctx.clone();
            let spotify_authorized = self.spotify_authorized.clone();
            let should_detect_now_playing = self.should_detect_now_playing.clone();

            tokio::spawn(async move {
                match update_currently_playing_wrapper(
                    spotify_client.clone(),
                    currently_playing.clone(),
                    debug_mode,
                )
                .await
                {
                    Ok(_) => {}
                    Err(e) => {
                        error!("更新當前播放失敗: {:?}", e);
                        if e.to_string().contains("Token 無效")
                            || e.to_string().contains("需要重新授權")
                        {
                            info!("Token 無效或過期，需要重新授權");
                            spotify_authorized.store(false, Ordering::SeqCst);
                            should_detect_now_playing.store(false, Ordering::SeqCst);
                        }
                    }
                }

                ctx.request_repaint_after(std::time::Duration::from_secs(1));
            });
        }
        let status = self.auth_manager.get_status(&AuthPlatform::Spotify);
        match status {
            AuthStatus::Failed(ref reason) => {
                if self.auth_error.is_none() {
                    self.auth_error = Some(reason.clone());
                }
            }
            AuthStatus::Completed => {
                if self.show_auth_progress {
                    self.show_auth_progress = false;
                    info!("授權成功");
                }
            }
            _ => {}
        }
        let side_menu_state_start = self.show_side_menu;
        if self.show_side_menu != side_menu_state_start {
            info!(
                "Side menu state changed from {} to {}",
                side_menu_state_start, self.show_side_menu
            );
        }
    } // 在更新結束時再次檢查側邊選單狀態
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
    ) -> Result<Self, AppError> {
        let texture_cache: Arc<RwLock<HashMap<String, Arc<TextureHandle>>>> =
            Arc::new(RwLock::new(HashMap::new()));
        let texture_load_queue: Arc<Mutex<BinaryHeap<Reverse<(usize, String)>>>> =
            Arc::new(Mutex::new(BinaryHeap::new()));

        let texture_cache_clone = Arc::clone(&texture_cache);
        let texture_load_queue_clone = Arc::clone(&texture_load_queue);
        let need_repaint_clone = Arc::clone(&need_repaint);
        let ctx_clone = ctx.clone();

        let spotify_icon = load_spotify_icon(&ctx);
        let config = read_config(debug_mode)?;

        let creds = Credentials::new(&config.spotify.client_id, &config.spotify.client_secret);
        let mut oauth = OAuth::default();
        oauth.redirect_uri = "http://localhost:8888/callback".to_string();
        oauth.scopes = scopes!("user-read-currently-playing");

        let spotify = AuthCodeSpotify::new(creds, oauth.clone());
        let spotify_client = Arc::new(Mutex::new(Some(spotify)));

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

        let mut preloaded_icons = HashMap::new();
        let icon_paths = vec!["spotify_icon_black.png", "osu!logo.png"];
        for path in icon_paths {
            if let Some(texture) = Self::load_icon(&ctx, path) {
                // 注意這裡的 &ctx
                preloaded_icons.insert(path.to_string(), texture);
            }
        }

        // 啟動異步加載任務
        tokio::spawn(async move {
            loop {
                let item = {
                    let mut queue = texture_load_queue_clone.lock().unwrap();
                    queue.pop()
                };

                if let Some(Reverse((_, url))) = item {
                    if !texture_cache_clone.read().await.contains_key(&url) {
                        if let Some(texture) = Self::load_texture_async(&ctx_clone, &url).await {
                            texture_cache_clone
                                .write()
                                .await
                                .insert(url.clone(), Arc::new(texture));
                            need_repaint_clone.store(true, Ordering::SeqCst);
                        }
                    }
                }

                tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            }
        });

        let mut app = Self {
            access_token: Arc::new(tokio::sync::Mutex::new(String::new())),
            auth_error: None,
            auth_in_progress: Arc::new(AtomicBool::new(false)),
            auth_manager: Arc::new(AuthManager::new()),
            auth_start_time: None,
            avatar_load_handle: None,
            client,
            config_errors,
            cover_textures,
            ctx,
            currently_playing: Arc::new(Mutex::new(None)),
            should_detect_now_playing: Arc::new(AtomicBool::new(false)),
            debug_mode,
            default_avatar_texture: None,
            displayed_osu_results: 10,
            displayed_spotify_results: 10,
            err_msg: Arc::new(tokio::sync::Mutex::new(String::new())),
            error_message: Arc::new(tokio::sync::Mutex::new(String::new())),
            global_font_size: 16.0,
            initialized: false,
            is_searching: Arc::new(AtomicBool::new(false)),
            last_update: Arc::new(Mutex::new(None)),
            listener: Arc::new(TokioMutex::new(None)),
            need_reload_avatar: Arc::new(AtomicBool::new(false)),
            need_repaint,
            osu_search_results: Arc::new(tokio::sync::Mutex::new(Vec::new())),
            receiver: Some(receiver),
            relax_slider_value: 0,
            scroll_to_top: false,
            search_query: String::new(),
            search_results: Arc::new(tokio::sync::Mutex::new(Vec::new())),
            selected_beatmapset: None,
            sender,
            show_auth_progress: false,
            show_relax_window: false,
            show_settings: false,
            show_side_menu: false,
            spotify_authorized: Arc::new(AtomicBool::new(false)),
            spotify_client,
            spotify_icon,
            spotify_user_avatar: Arc::new(Mutex::new(None)),
            spotify_user_avatar_url: Arc::new(Mutex::new(None)),
            spotify_user_name: Arc::new(Mutex::new(None)),
            show_spotify_now_playing: false,
            texture_cache,
            texture_load_queue,
            preloaded_icons,
            spotify_search_button_states: HashMap::new(),
            spotify_open_button_states: HashMap::new(),
            osu_search_button_states: HashMap::new(),
            osu_open_button_states: HashMap::new(),
            side_menu_animation: HashMap::new(),
        };

        app.load_default_avatar();

        Ok(app)
    }

    fn cancel_authorization(&mut self) {
        self.auth_manager.reset(&AuthPlatform::Spotify);
        self.auth_start_time = None;
        self.auth_in_progress.store(false, Ordering::SeqCst);
        self.show_auth_progress = false;

        if let Ok(mut listener_guard) = self.listener.try_lock() {
            *listener_guard = None;
        }

        if let Ok(mut spotify_client) = self.spotify_client.try_lock() {
            *spotify_client = None;
        }

        // 確保將狀態設置為 NotStarted
        self.auth_manager
            .update_status(&AuthPlatform::Spotify, AuthStatus::NotStarted);

        error!("用戶取消了授權流程");
    }

    fn start_spotify_authorization(&mut self, ctx: egui::Context) {
        if self.auth_in_progress.load(Ordering::SeqCst) {
            info!("Spotify 授權已在進行中，請等待");
            return;
        }

        info!("開始 Spotify 授權流程");
        self.show_auth_progress = true;
        self.auth_in_progress.store(true, Ordering::SeqCst);
        self.auth_manager.reset(&AuthPlatform::Spotify);
        self.auth_start_time = Some(Instant::now());

        // 重置相關狀態
        self.spotify_authorized.store(false, Ordering::SeqCst);
        *self.spotify_user_avatar_url.lock().unwrap() = None;
        self.need_reload_avatar.store(true, Ordering::SeqCst);

        let spotify_client = self.spotify_client.clone();
        let debug_mode = self.debug_mode;
        let spotify_authorized = self.spotify_authorized.clone();
        let auth_manager = self.auth_manager.clone();
        let listener = self.listener.clone();
        let ctx_clone = ctx.clone();
        let spotify_user_avatar_url = self.spotify_user_avatar_url.clone();
        let need_reload_avatar = self.need_reload_avatar.clone();
        let spotify_user_avatar = self.spotify_user_avatar.clone();
        let spotify_user_name = self.spotify_user_name.clone();
        let auth_in_progress = self.auth_in_progress.clone();

        tokio::spawn(async move {
            // 關閉之前的監聽器（如果有的話）
            {
                let mut listener_guard = listener.lock().await;
                if let Some(l) = listener_guard.take() {
                    drop(l);
                }
            }

            let result = authorize_spotify(
                spotify_client.clone(),
                debug_mode,
                auth_manager.clone(),
                listener.clone(),
                spotify_authorized.clone(),
            )
            .await;

            match result {
                Ok((avatar_url, Some(user_name))) => {
                    info!(
                        "Spotify 授權成功，獲取到頭像 URL: {:?} 和用戶名稱: {}",
                        avatar_url, user_name
                    );
                    let avatar_url_clone = avatar_url.clone();
                    *spotify_user_avatar_url.lock().unwrap() = avatar_url;
                    *spotify_user_name.lock().unwrap() = Some(user_name);
                    need_reload_avatar.store(true, Ordering::SeqCst);
                    spotify_authorized.store(true, Ordering::SeqCst);
                    auth_manager.update_status(&AuthPlatform::Spotify, AuthStatus::Completed);

                    // 使用克隆的 avatar_url_clone
                    if let Some(url) = avatar_url_clone {
                        if let Err(e) = SearchApp::load_spotify_avatar(
                            &ctx_clone,
                            &url,
                            spotify_user_avatar.clone(),
                            need_reload_avatar.clone(),
                        )
                        .await
                        {
                            error!("加載 Spotify 頭像失敗: {:?}", e);
                        }
                    }
                }
                Ok((_, None)) => {
                    error!("Spotify 授權成功，但未獲取到用戶 ID");
                    spotify_authorized.store(true, Ordering::SeqCst);
                    auth_manager.update_status(&AuthPlatform::Spotify, AuthStatus::Completed);
                }
                Err(e) => {
                    error!("Spotify 授權失敗: {:?}", e);
                    auth_manager
                        .update_status(&AuthPlatform::Spotify, AuthStatus::Failed(e.to_string()));
                }
            }

            auth_in_progress.store(false, Ordering::SeqCst);
            ctx_clone.request_repaint();
        });
    }

    //顯示SETTINGS
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
            set_log_level(self.debug_mode);
            info!("Debug mode: {}", self.debug_mode);
        }

        ui.add_space(10.0);
    }

    fn should_update_current_playing(&self) -> bool {
        if !self.spotify_authorized.load(Ordering::SeqCst) {
            return false; // 如果未授權，不更新
        }

        let mut last_update = self.last_update.lock().unwrap();
        if last_update.is_none() || last_update.unwrap().elapsed() > Duration::from_secs(2) {
            *last_update = Some(Instant::now());
            true
        } else {
            false
        }
    }
    //創建右鍵選單
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

    //處理搜尋
    fn perform_search(&mut self, ctx: egui::Context) -> JoinHandle<Result<()>> {
        set_log_level(self.debug_mode); // 設置日誌級別

        let client = self.client.clone();
        let debug_mode = self.debug_mode;
        let query = self.search_query.clone();
        let search_results = self.search_results.clone();
        let osu_search_results = self.osu_search_results.clone();
        let is_searching = self.is_searching.clone();
        let need_repaint = self.need_repaint.clone();
        let err_msg = self.err_msg.clone();
        let sender = self.sender.clone();
        let ctx_clone = ctx.clone(); // 在這裡克隆 ctx
        self.displayed_osu_results = 10;
        self.clear_cover_textures();

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
                    .map_err(|e| match e {
                        SpotifyError::AccessTokenError(msg) => {
                            anyhow!("Spotify 錯誤：無法獲取 token: {}", msg)
                        }
                        SpotifyError::RequestError(e) => anyhow!("Spotify 請求錯誤：{}", e),
                        _ => anyhow!("Spotify 錯誤：{}", e),
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
                            index: twc.index,
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
                    for (index, beatmapset) in results.iter().enumerate() {
                        if let Some(cover_url) = &beatmapset.covers.cover {
                            osu_urls.push((index, cover_url.clone()));
                        }
                    }
                    *osu_search_results.lock().await = results;

                    if let Err(e) =
                        load_osu_covers(osu_urls, ctx_clone.clone(), sender.clone()).await
                    {
                        error!("載入 osu 封面時發生錯誤: {:?}", e);
                        if debug_mode {
                            ctx_clone.request_repaint();
                            egui::Window::new("Error").show(&ctx_clone, |ui| {
                                ui.label("部分 osu 封面載入失敗:");
                                ui.label(format!("{:?}", e));
                            });
                        }
                    }
                } else {
                    // 如果不是 osu! URL，執行原有的搜索邏輯
                    let spotify_result: Result<Vec<TrackWithCover>> =
                        match is_valid_spotify_url(&query) {
                            Ok(status) => match status {
                                SpotifyUrlStatus::Valid => {
                                    info!("Spotify 查詢 (URL): {}", query);
                                    let track_id = query
                                        .split('/')
                                        .last()
                                        .unwrap_or("")
                                        .split('?')
                                        .next()
                                        .unwrap_or("");
                                    let track = get_track_info(
                                        &*client.lock().await,
                                        track_id,
                                        &spotify_token,
                                    )
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
                                        index: 0, // 添加這行，給予一個固定的索引
                                    }])
                                }
                                SpotifyUrlStatus::Incomplete => {
                                    *error = "Spotify URL 不完整，請輸入完整的 URL".to_string();
                                    return Ok(());
                                }
                                SpotifyUrlStatus::Invalid => {
                                    *error = "無效的 Spotify URL".to_string();
                                    return Ok(());
                                }
                                SpotifyUrlStatus::NotSpotify => {
                                    // 執行普通搜索
                                    if !query.is_empty() {
                                        info!("Spotify 查詢 (關鍵字): {}", query);
                                        let limit = 50;
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
                                        .map_err(|e| anyhow!("Spotify 搜索錯誤: {}", e))
                                    } else {
                                        Ok(Vec::new())
                                    }
                                }
                            },
                            Err(e) => {
                                error!("驗證 Spotify URL 時發生錯誤: {:?}", e);
                                Err(anyhow!("Spotify URL 驗證錯誤"))
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
                                    index: twc.index,
                                })
                                .collect();

                            if matches!(is_valid_spotify_url(&query), Ok(SpotifyUrlStatus::Valid))
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
                    for (index, beatmapset) in results.iter().enumerate().take(10) {
                        if let Some(cover_url) = &beatmapset.covers.cover {
                            osu_urls.push((index, cover_url.clone()));
                        }
                    }
                    *osu_search_results.lock().await = results;

                    info!("初始加載 osu 封面：共 {} 個", osu_urls.len());

                    if let Err(e) =
                        load_osu_covers(osu_urls.clone(), ctx_clone.clone(), sender.clone()).await
                    {
                        error!("載入 osu 封面時發生錯誤: {:?}", e);
                        if debug_mode {
                            ctx_clone.request_repaint();
                            egui::Window::new("Error").show(&ctx_clone, |ui| {
                                ui.label("部分 osu 封面載入失敗:");
                                ui.label(format!("{:?}", e));
                            });
                        }
                    } else {
                        info!("成功初始加載 {} 個 osu 封面", osu_urls.len());
                    }
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
    //顯示Spotify搜索結果
    fn display_spotify_results(&mut self, ui: &mut egui::Ui, window_size: egui::Vec2) {
        egui::ScrollArea::vertical()
            .id_source("spotify_results_scroll")
            .show(ui, |ui| {
                let sorted_results = {
                    if let Ok(search_results_guard) = self.search_results.try_lock() {
                        let mut results = search_results_guard.clone();
                        results.sort_by_key(|track| track.index);
                        results
                    } else {
                        return;
                    }
                };
    
                let total_results = sorted_results.len();
                let displayed_results = self.displayed_spotify_results.min(total_results);
    
                // 記錄整個可視區域的頂部
                let top_rect = ui.max_rect();
    
                ui.horizontal(|ui| {
                    if window_size.x >= 1000.0 {
                        ui.heading(
                            egui::RichText::new("Spotify Results")
                                .size(self.global_font_size * 1.2),
                        );
                        ui.add_space(10.0);
                    }
                    ui.label(
                        egui::RichText::new(format!("總結果數: {}", total_results))
                            .size(self.global_font_size),
                    );
                    ui.add_space(10.0);
                    ui.label(
                        egui::RichText::new(format!("當前顯示結果數: {}", displayed_results))
                            .size(self.global_font_size),
                    );
                });
    
                ui.add_space(10.0);
    
                if !sorted_results.is_empty() {
                    for (index, track) in sorted_results.iter().take(displayed_results).enumerate() {
                        self.display_spotify_track(ui, track, index);
                    }
    
                    ui.add_space(30.0);
                    if displayed_results < total_results {
                        ui.vertical_centered(|ui| {
                            if ui
                                .add_sized(
                                    [200.0, 40.0],
                                    egui::Button::new(egui::RichText::new("顯示更多").size(18.0)),
                                )
                                .clicked()
                            {
                                self.displayed_spotify_results = (self.displayed_spotify_results + 10).min(total_results);
                            }
                        });
                    } else {
                        ui.horizontal(|ui| {
                            ui.label(egui::RichText::new("已顯示所有結果").size(18.0));
                            if ui
                                .add_sized(
                                    [120.0, 40.0],
                                    egui::Button::new(egui::RichText::new("回到頂部").size(18.0)),
                                )
                                .clicked()
                            {
                                // 滾動到整個可視區域的頂部
                                ui.scroll_to_rect(top_rect, Some(egui::Align::TOP));
                            }
                        });
                    }
                    ui.add_space(50.0);
                } else {
                    ui.label("沒有搜索結果");
                }
            });
    }

    fn display_spotify_track(&mut self, ui: &mut egui::Ui, track: &Track, index: usize) {
        let response = ui.add(
            egui::Button::new("")
                .frame(false)
                .min_size(egui::vec2(ui.available_width(), 100.0)),
        );

        ui.allocate_ui_at_rect(response.rect, |ui| {
            ui.horizontal(|ui| {
                ui.vertical(|ui| {
                    // 顯示專輯封面
                    if let Some(cover_url) = track.album.images.first().map(|img| &img.url) {
                        let texture_cache = self.texture_cache.clone();
                        let texture_load_queue = self.texture_load_queue.clone();

                        if let Ok(cache) = texture_cache.try_read() {
                            if let Some(texture) = cache.get(cover_url) {
                                let size = egui::Vec2::new(100.0, 100.0);
                                ui.add(egui::Image::new(egui::load::SizedTexture::new(
                                    texture.id(),
                                    size,
                                )));
                            } else {
                                if let Ok(mut queue) = texture_load_queue.lock() {
                                    if !queue.iter().any(|Reverse((_, url))| url == cover_url) {
                                        queue.push(Reverse((track.index, cover_url.to_string())));
                                    }
                                }
                                ui.add_sized([100.0, 100.0], egui::Spinner::new().size(32.0));
                            }
                        } else {
                            ui.add_sized([100.0, 100.0], egui::Spinner::new().size(32.0));
                        };
                    }
                });

                ui.add_space(10.0);

                ui.vertical(|ui| {
                    ui.label(
                        egui::RichText::new(&track.name)
                            .font(egui::FontId::proportional(self.global_font_size * 1.0))
                            .strong(),
                    );
                    ui.label(
                        egui::RichText::new(
                            &track
                                .artists
                                .iter()
                                .map(|a| a.name.clone())
                                .collect::<Vec<_>>()
                                .join(", "),
                        )
                        .font(egui::FontId::proportional(self.global_font_size * 0.9)),
                    );
                    ui.label(
                        egui::RichText::new(&track.album.name)
                            .font(egui::FontId::proportional(self.global_font_size * 0.8)),
                    );
                });
            });
        });

        let button_size = egui::vec2(30.0, 30.0);
        let spacing = 10.0; // 增加間距

        // "以此搜尋" 按鈕
        let search_button_rect = egui::Rect::from_min_size(
            response.rect.right_bottom()
                + egui::vec2(-(2.0 * button_size.x + spacing), -button_size.y - 5.0),
            button_size,
        );
        let search_button_response =
            self.draw_search_button(ui, index, search_button_rect, ButtonType::Spotify);

        if search_button_response.clicked() {
            if let Some(spotify_url) = track.external_urls.get("spotify") {
                self.search_query = spotify_url.clone();
            } else {
                self.search_query = format!(
                    "{} {}",
                    track.name,
                    track
                        .artists
                        .iter()
                        .map(|a| a.name.as_str())
                        .collect::<Vec<_>>()
                        .join(" ")
                );
            }
            let ctx = ui.ctx().clone();
            self.perform_search(ctx);
        }

        search_button_response.on_hover_text("以此搜尋");

        // "打開" 按鈕
        let open_button_rect = egui::Rect::from_min_size(
            response.rect.right_bottom() + egui::vec2(-(button_size.x + 5.0), -button_size.y - 5.0),
            button_size,
        );
        let open_button_response =
            self.draw_open_browser_button(ui, index, open_button_rect, ButtonType::Spotify);

        if open_button_response.clicked() {
            if let Some(url) = track.external_urls.get("spotify") {
                if let Err(e) = open_spotify_url(url) {
                    log::error!("無法開啟 URL: {}", e);
                }
            }
        }

        open_button_response.on_hover_text("打開");

        response.context_menu(|ui| {
            self.create_context_menu(ui, |add_button| {
                if let Some(url) = track.external_urls.get("spotify") {
                    add_button(
                        "複製連結",
                        Box::new(move || {
                            let mut ctx: ClipboardContext = ClipboardProvider::new().unwrap();
                            ctx.set_contents(url.clone()).unwrap();
                        }),
                    );
                    add_button(
                        "開啟",
                        Box::new(move || {
                            if let Err(e) = open_spotify_url(&url) {
                                log::error!("無法開啟 URL: {}", e);
                            }
                        }),
                    );
                }
            });
        });

        ui.add_space(5.0);
        ui.separator();
    }
    //顯示osu搜索結果
    fn display_osu_results(&mut self, ui: &mut egui::Ui, window_size: egui::Vec2) {
        let mut scroll_area = egui::ScrollArea::vertical().id_source("osu_results_scroll");

        if self.scroll_to_top {
            scroll_area = scroll_area.scroll_offset(egui::vec2(0.0, 0.0));
            self.scroll_to_top = false;
        }

        scroll_area.show(ui, |ui| {
            let mut should_load_more = false;
            let mut new_displayed_results = self.displayed_osu_results;

            let total_results;
            let displayed_results;
            let mut beatmapsets_to_display = Vec::new();

            {
                if let Ok(osu_search_results_guard) = self.osu_search_results.try_lock() {
                    total_results = osu_search_results_guard.len();
                    displayed_results = self.displayed_osu_results.min(total_results);
                    beatmapsets_to_display = osu_search_results_guard
                        .iter()
                        .take(displayed_results)
                        .cloned()
                        .collect();
                } else {
                    total_results = 0;
                    displayed_results = 0;
                }
            }

            ui.horizontal(|ui| {
                if window_size.x >= 1000.0 {
                    ui.heading(
                        egui::RichText::new("Osu Results").size(self.global_font_size * 1.2),
                    );
                    ui.add_space(10.0);
                }
                ui.label(
                    egui::RichText::new(format!("總結果數: {}", total_results))
                        .size(self.global_font_size),
                );
                ui.add_space(10.0);
                ui.label(
                    egui::RichText::new(format!("當前顯示結果數: {}", displayed_results))
                        .size(self.global_font_size),
                );
            });

            ui.add_space(10.0);

            if !beatmapsets_to_display.is_empty() {
                if let Some(selected_index) = self.selected_beatmapset {
                    if let Some(selected_beatmapset) = beatmapsets_to_display.get(selected_index) {
                        self.display_selected_beatmapset(ui, selected_beatmapset);
                    }
                } else {
                    for (index, beatmapset) in beatmapsets_to_display.iter().enumerate() {
                        self.display_beatmapset(ui, beatmapset, index);
                    }

                    ui.add_space(30.0);
                    if displayed_results < total_results {
                        ui.vertical_centered(|ui| {
                            if ui
                                .add_sized(
                                    [200.0, 40.0],
                                    egui::Button::new(egui::RichText::new("顯示更多").size(18.0)),
                                )
                                .clicked()
                            {
                                new_displayed_results = (displayed_results + 10).min(total_results);
                                should_load_more = true;
                            }
                        });
                    } else {
                        ui.horizontal(|ui| {
                            ui.label(egui::RichText::new("已顯示所有結果").size(18.0));
                            if ui
                                .add_sized(
                                    [120.0, 40.0],
                                    egui::Button::new(egui::RichText::new("回到頂部").size(18.0)),
                                )
                                .clicked()
                            {
                                self.scroll_to_top = true;
                                ui.ctx().request_repaint();
                            }
                        });
                    }
                    ui.add_space(50.0);
                }
            } else {
                ui.label("沒有搜索結果");
            }

            if should_load_more {
                self.displayed_osu_results = new_displayed_results;
                self.load_more_osu_covers(displayed_results, new_displayed_results);
            }
        });
    }

    //加載更多osu封面
    fn load_more_osu_covers(&self, start: usize, end: usize) {
        if let Ok(osu_search_results_guard) = self.osu_search_results.try_lock() {
            let mut osu_urls = Vec::new();
            for (index, beatmapset) in osu_search_results_guard
                .iter()
                .enumerate()
                .skip(start)
                .take(end - start)
            {
                if let Some(cover_url) = &beatmapset.covers.cover {
                    osu_urls.push((index, cover_url.clone()));
                }
            }

            // 新增：記錄本次加載的封面數量
            let loaded_covers_count = osu_urls.len();
            info!(
                "正在加載更多 osu 封面：從 {} 到 {}，共 {} 個",
                start, end, loaded_covers_count
            );

            let sender_clone = self.sender.clone();
            let debug_mode = self.debug_mode;
            let need_repaint = self.need_repaint.clone();
            let ctx = self.ctx.clone();

            tokio::spawn(async move {
                if let Err(e) = load_osu_covers(osu_urls, ctx.clone(), sender_clone).await {
                    error!("載入更多 osu 封面時發生錯誤: {:?}", e);
                    if debug_mode {
                        error!("載入更多 osu 封面錯誤: {:?}", e);
                    }
                } else {
                    // 新增：記錄成功加載的封面數量
                    info!("成功加載 {} 個 osu 封面", loaded_covers_count);
                }
                need_repaint.store(true, std::sync::atomic::Ordering::SeqCst);
            });
        }
    }

    //顯示osu譜面集
    fn display_beatmapset(&mut self, ui: &mut egui::Ui, beatmapset: &Beatmapset, index: usize) {
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
                    let is_image_loaded = if let Ok(textures) = self.cover_textures.try_read() {
                        textures.get(&index).map_or(false, |opt| opt.is_some())
                    } else {
                        false
                    };

                    if is_image_loaded {
                        if let Ok(textures) = self.cover_textures.try_read() {
                            if let Some(Some((texture, size))) = textures.get(&index) {
                                let max_height = 100.0;
                                let aspect_ratio = size.0 / size.1;
                                let image_size =
                                    egui::Vec2::new(max_height * aspect_ratio, max_height);
                                ui.image((texture.id(), image_size));
                            }
                        }
                    } else {
                        ui.add_sized([100.0, 100.0], egui::Spinner::new().size(32.0));
                    }
                });

                ui.add_space(10.0);

                ui.vertical(|ui| {
                    ui.label(
                        egui::RichText::new(&beatmapset.title)
                            .font(egui::FontId::proportional(self.global_font_size * 1.0))
                            .strong(),
                    );
                    ui.label(
                        egui::RichText::new(&beatmapset.artist)
                            .font(egui::FontId::proportional(self.global_font_size * 0.9)),
                    );
                    ui.label(
                        egui::RichText::new(format!("by {}", beatmapset.creator))
                            .font(egui::FontId::proportional(self.global_font_size * 0.8)),
                    );
                });
            });
        });

        let button_size = egui::vec2(30.0, 30.0);
        let spacing = 10.0;

        // "以此搜尋" 按鈕
        let search_button_rect = egui::Rect::from_min_size(
            response.rect.right_bottom()
                + egui::vec2(-(2.0 * button_size.x + spacing), -button_size.y - 5.0),
            button_size,
        );
        let search_button_response =
            self.draw_search_button(ui, index, search_button_rect, ButtonType::Osu);

        if search_button_response.clicked() {
            let osu_url = format!("https://osu.ppy.sh/beatmapsets/{}", beatmapset.id);
            self.search_query = osu_url;
            let ctx = ui.ctx().clone();
            self.perform_search(ctx);
        }

        search_button_response.on_hover_text("以此搜尋");

        // "打開" 按鈕
        let open_button_rect = egui::Rect::from_min_size(
            response.rect.right_bottom() + egui::vec2(-(button_size.x + 5.0), -button_size.y - 5.0),
            button_size,
        );
        let open_button_response =
            self.draw_open_browser_button(ui, index, open_button_rect, ButtonType::Osu);

        if open_button_response.clicked() {
            if let Err(e) = open::that(format!("https://osu.ppy.sh/beatmapsets/{}", beatmapset.id))
            {
                error!("無法打開瀏覽器: {:?}", e);
            }
        }

        open_button_response.on_hover_text("在瀏覽器中打開");

        ui.add_space(5.0);
        ui.separator();
    }

    //顯示osu譜面集詳情
    fn display_selected_beatmapset(&mut self, ui: &mut egui::Ui, beatmapset: &Beatmapset) {
        let beatmap_info = print_beatmap_info_gui(beatmapset);

        ui.heading(
            egui::RichText::new(format!("{} - {}", beatmap_info.title, beatmap_info.artist))
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
                egui::RichText::new(beatmap_info)
                    .font(egui::FontId::proportional(self.global_font_size * 1.0)),
            );
            ui.add_space(10.0);
            ui.separator();
        }
        if ui
            .add_sized(
                [100.0, 40.0],
                egui::Button::new(
                    egui::RichText::new("Back")
                        .font(egui::FontId::proportional(self.global_font_size * 1.0)),
                ),
            )
            .clicked()
        {
            self.selected_beatmapset = None;
        }
    }

    //清除封面紋理
    fn clear_cover_textures(&self) {
        if let Ok(mut textures) = self.cover_textures.try_write() {
            textures.clear();
        }
    }
    //繪製搜索按鈕
    fn draw_search_button(
        &mut self,
        ui: &mut egui::Ui,
        index: usize,
        rect: egui::Rect,
        button_type: ButtonType,
    ) -> egui::Response {
        let animation_progress = match button_type {
            ButtonType::Spotify => self
                .spotify_search_button_states
                .entry(index)
                .or_insert(0.0),
            ButtonType::Osu => self.osu_search_button_states.entry(index).or_insert(0.0),
        };
        let response = ui.allocate_rect(rect, egui::Sense::click());

        if response.hovered() {
            *animation_progress =
                (*animation_progress + ui.input(|i| i.unstable_dt) * 3.0).min(1.0);
        } else {
            *animation_progress =
                (*animation_progress - ui.input(|i| i.unstable_dt) * 3.0).max(0.0);
        }

        let center = rect.center();
        let radius = rect.height() / 2.0;

        // 繪製圓形背景
        let bg_color = egui::Color32::from_rgba_unmultiplied(
            200 + ((55) as f32 * *animation_progress) as u8,
            200 + ((55) as f32 * *animation_progress) as u8,
            200 + ((55) as f32 * *animation_progress) as u8,
            255,
        );
        ui.painter()
            .circle(center, radius, bg_color, egui::Stroke::NONE);

        // 繪製搜索圖標
        let icon_color = egui::Color32::from_rgba_unmultiplied(
            0 + ((255) as f32 * *animation_progress) as u8,
            0 + ((255) as f32 * *animation_progress) as u8,
            0 + ((255) as f32 * *animation_progress) as u8,
            255,
        );

        // 繪製放大鏡
        let glass_center = center + egui::vec2(-radius * 0.2, -radius * 0.2);
        let glass_radius = radius * 0.5;
        ui.painter().circle_stroke(
            glass_center,
            glass_radius,
            egui::Stroke::new(2.0, icon_color),
        );

        // 繪製放大鏡手柄
        let handle_start = glass_center + egui::vec2(glass_radius * 0.7, glass_radius * 0.7);
        let handle_end = handle_start + egui::vec2(radius * 0.4, radius * 0.4);
        ui.painter().line_segment(
            [handle_start, handle_end],
            egui::Stroke::new(2.0, icon_color),
        );

        response
    }

    //繪製前往瀏覽器按鈕
    fn draw_open_browser_button(
        &mut self,
        ui: &mut egui::Ui,
        index: usize,
        rect: egui::Rect,
        button_type: ButtonType,
    ) -> egui::Response {
        let animation_progress = match button_type {
            ButtonType::Spotify => self.spotify_open_button_states.entry(index).or_insert(0.0),
            ButtonType::Osu => self.osu_open_button_states.entry(index).or_insert(0.0),
        };
        let response = ui.allocate_rect(rect, egui::Sense::click());

        if response.hovered() {
            *animation_progress =
                (*animation_progress + ui.input(|i| i.unstable_dt) * 3.0).min(1.0);
        } else {
            *animation_progress =
                (*animation_progress - ui.input(|i| i.unstable_dt) * 3.0).max(0.0);
        }

        let center = rect.center();
        let radius = rect.height() / 2.0;

        // 繪製圓形背景
        let bg_color_start = egui::Color32::from_rgb(200, 200, 200);
        let bg_color_end = egui::Color32::LIGHT_BLUE;
        let bg_color = egui::Color32::from_rgba_unmultiplied(
            bg_color_start.r()
                + ((bg_color_end.r() as f32 - bg_color_start.r() as f32)
                    * *animation_progress as f32) as u8,
            bg_color_start.g()
                + ((bg_color_end.g() as f32 - bg_color_start.g() as f32)
                    * *animation_progress as f32) as u8,
            bg_color_start.b()
                + ((bg_color_end.b() as f32 - bg_color_start.b() as f32)
                    * *animation_progress as f32) as u8,
            255,
        );
        ui.painter()
            .circle(center, radius, bg_color, egui::Stroke::NONE);

        // 繪製瀏覽器圖標
        let icon_size = radius * 1.2;
        let top_left = center - egui::vec2(icon_size / 2.0, icon_size / 2.0);
        let bottom_right = center + egui::vec2(icon_size / 2.0, icon_size / 2.0);

        let icon_color_start = egui::Color32::BLACK;
        let icon_color_end = egui::Color32::WHITE;
        let icon_color = egui::Color32::from_rgba_unmultiplied(
            icon_color_start.r()
                + ((icon_color_end.r() as f32 - icon_color_start.r() as f32)
                    * *animation_progress as f32) as u8,
            icon_color_start.g()
                + ((icon_color_end.g() as f32 - icon_color_start.g() as f32)
                    * *animation_progress as f32) as u8,
            icon_color_start.b()
                + ((icon_color_end.b() as f32 - icon_color_start.b() as f32)
                    * *animation_progress as f32) as u8,
            255,
        );

        ui.painter().rect_stroke(
            egui::Rect::from_two_pos(top_left, bottom_right),
            egui::Rounding::same(2.0),
            egui::Stroke::new(2.0, icon_color),
        );

        // 繪製箭頭
        let arrow_start = center + egui::vec2(-icon_size / 4.0, 0.0);
        let arrow_end = center + egui::vec2(icon_size / 4.0, 0.0);
        ui.painter()
            .line_segment([arrow_start, arrow_end], egui::Stroke::new(2.0, icon_color));

        let arrow_top = arrow_end + egui::vec2(-icon_size / 8.0, -icon_size / 8.0);
        let arrow_bottom = arrow_end + egui::vec2(-icon_size / 8.0, icon_size / 8.0);
        ui.painter()
            .line_segment([arrow_end, arrow_top], egui::Stroke::new(2.0, icon_color));
        ui.painter().line_segment(
            [arrow_end, arrow_bottom],
            egui::Stroke::new(2.0, icon_color),
        );

        // 繪製動畫效果
        if *animation_progress > 0.0 {
            // 內圈動畫
            ui.painter().circle_stroke(
                center,
                radius * (1.0 + *animation_progress * 0.2),
                egui::Stroke::new(2.0 * *animation_progress as f32, egui::Color32::WHITE),
            );

            // 外圈動畫
            ui.painter().circle_stroke(
                center,
                radius * (1.0 + *animation_progress * 0.4),
                egui::Stroke::new(
                    1.0 * *animation_progress as f32,
                    egui::Color32::from_white_alpha((128.0 * *animation_progress) as u8),
                ),
            );
        }

        response
    }
    //加載默認頭像
    fn load_default_avatar(&mut self) {
        let default_avatar_bytes = include_bytes!("assets/login.png");
        let default_avatar_image = image::load_from_memory(default_avatar_bytes).unwrap();
        let default_avatar_size = [
            default_avatar_image.width() as _,
            default_avatar_image.height() as _,
        ];
        let default_avatar_pixels = default_avatar_image.to_rgba8();
        self.default_avatar_texture = Some(self.ctx.load_texture(
            "default_avatar",
            egui::ColorImage::from_rgba_unmultiplied(
                default_avatar_size,
                default_avatar_pixels.as_flat_samples().as_slice(),
            ),
            egui::TextureOptions::default(),
        ));
    }
    //渲染頂部面板
    fn render_top_panel(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                // 側邊菜單按鈕
                let button_size = egui::vec2(40.0, 40.0);
                let (rect, response) = ui.allocate_exact_size(button_size, egui::Sense::click());

                if ui.is_rect_visible(rect) {
                    let visuals = ui.style().interact(&response);
                    let animation_progress = self.side_menu_animation.entry(ui.id()).or_insert(0.0);

                    if response.hovered() {
                        *animation_progress =
                            (*animation_progress + ui.input(|i| i.unstable_dt) * 4.0).min(1.0);
                    } else {
                        *animation_progress =
                            (*animation_progress - ui.input(|i| i.unstable_dt) * 4.0).max(0.0);
                    }

                    let color = egui::Color32::from_rgba_unmultiplied(
                        255,
                        255,
                        255,
                        (255.0 * *animation_progress) as u8,
                    );

                    ui.painter().rect_filled(
                        rect.expand(*animation_progress * 4.0),
                        visuals.rounding,
                        color,
                    );

                    let font_id = egui::FontId::proportional(24.0);
                    let galley =
                        ui.painter()
                            .layout_no_wrap("☰".to_string(), font_id, visuals.text_color());

                    let text_pos = rect.center() - galley.size() / 2.0;
                    ui.painter().galley(text_pos, galley, visuals.text_color());
                }

                if response.clicked() {
                    self.show_side_menu = !self.show_side_menu;
                    info!(
                        "Side menu button clicked. New state: {}",
                        self.show_side_menu
                    );
                }

                ui.add_space(10.0);

                ui.with_layout(
                    egui::Layout::left_to_right(egui::Align::Center).with_main_justify(true),
                    |ui| {
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if self.spotify_authorized.load(Ordering::SeqCst) {
                                self.render_logged_in_user(ui);

                                let button =
                                    egui::Button::new(egui::RichText::new("🎵").size(16.0))
                                        .min_size(egui::vec2(32.0, 32.0))
                                        .frame(false);

                                let response = ui.add(button);

                                if response.clicked() {
                                    ui.memory_mut(|mem| {
                                        mem.toggle_popup(egui::Id::new("now_playing_popup"))
                                    });
                                    self.should_detect_now_playing.store(true, Ordering::SeqCst);
                                }

                                if response.hovered() {
                                    ui.painter().rect_stroke(
                                        response.rect,
                                        egui::Rounding::same(4.0),
                                        egui::Stroke::new(1.0, egui::Color32::LIGHT_BLUE),
                                    );
                                }

                                self.render_now_playing_popup(ui, &response);
                            } else {
                                self.render_guest_user(ui);
                            }
                        });
                    },
                );
            });
        });
    }

    fn render_side_menu(&mut self, ctx: &egui::Context) {
        if !self.show_side_menu {
            return;
        }
    
        egui::SidePanel::left("side_menu")
            .default_width(250.0)
            .show(ctx, |ui| {
                ui.debug_paint_cursor();
    
                ui.add_space(10.0);
    
                ui.horizontal(|ui| {
                    ui.heading("Menu");
                    if ui.button("✕").clicked() {
                        self.show_side_menu = false;
                        info!("Side menu close button clicked. New state: false");
                    }
                });
    
                ui.add_space(20.0);
    
                ui.style_mut().spacing.item_spacing.y = 2.0;
    
                let menu_items = vec![
                    ("🏠 Home", "home"),
                    ("🔍 Issues", "issues"),
                    ("🔀 Pull requests", "pull_requests"),
                    ("📁 Projects", "projects"),
                    ("💬 Discussions", "discussions"),
                    ("🧑‍💻 Codespaces", "codespaces"),
                    ("🌐 Explore", "explore"),
                    ("🛒 Marketplace", "marketplace"),
                ];
    
                for (label, id) in menu_items {
                    if ui.button(label).clicked() {
                        info!("Clicked on: {}", id);
                        self.show_side_menu = false;
                    }
                }
    
                ui.add_space(20.0);
                ui.separator();
                ui.add_space(10.0);
    
                ui.label("Repositories");
            });
    
        // 檢查是否在側邊欄外點擊
        if ctx.input(|i| i.pointer.any_released()) && !ctx.is_pointer_over_area() {
            self.show_side_menu = false;
            info!("Clicked outside side menu. New state: false");
        }
    }

    //渲染正在播放的彈窗
    fn render_now_playing_popup(&mut self, ui: &mut egui::Ui, response: &egui::Response) {
        egui::popup::popup_below_widget(ui, egui::Id::new("now_playing_popup"), response, |ui| {
            ui.set_min_width(250.0);
            ui.set_max_width(300.0);

            let current_playing = self
                .currently_playing
                .lock()
                .ok()
                .and_then(|guard| guard.clone());

            match current_playing {
                Some(current_playing) => {
                    ui.horizontal(|ui| {
                        if let Some(spotify_icon) = &self.spotify_icon {
                            let size = egui::vec2(24.0, 24.0);
                            ui.add(egui::Image::new(egui::load::SizedTexture::new(
                                spotify_icon.id(),
                                size,
                            )));
                        }
                        ui.label(egui::RichText::new("正在播放").strong());
                    });

                    ui.add_space(5.0);

                    ui.label(egui::RichText::new(&current_playing.track_info.name).size(16.0));
                    ui.label(egui::RichText::new(&current_playing.track_info.artists).size(14.0));

                    ui.add_space(10.0);

                    if ui.button("搜索此歌曲").clicked() {
                        if let Some(spotify_url) = &current_playing.spotify_url {
                            self.search_query = spotify_url.clone();
                        } else {
                            self.search_query = format!(
                                "{} {}",
                                current_playing.track_info.artists, current_playing.track_info.name
                            );
                        }
                        let ctx = ui.ctx().clone();
                        self.perform_search(ctx);
                        ui.close_menu();
                    }
                }
                None => {
                    ui.label("當前沒有正在播放的曲目");
                }
            }
        });
    }
    //渲染登錄用戶
    fn render_logged_in_user(&mut self, ui: &mut egui::Ui) {
        let avatar_size = egui::vec2(32.0, 32.0);
        let button_size = egui::vec2(40.0, 40.0); // 稍微增加按鈕大小，為頭像周圍留出一些空間

        let button = egui::Button::new("")
            .fill(egui::Color32::TRANSPARENT)
            .min_size(button_size)
            .frame(false);

        let response = ui.add(button);

        if ui.is_rect_visible(response.rect) {
            if let Some(avatar) = &*self.spotify_user_avatar.lock().unwrap() {
                let image_rect = egui::Rect::from_center_size(response.rect.center(), avatar_size);
                ui.painter().image(
                    avatar.id(),
                    image_rect,
                    egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                    egui::Color32::WHITE,
                );
            }
        }

        // 動態調整藍色圈選範圍
        if response.hovered() {
            ui.painter().rect_stroke(
                response.rect,
                egui::Rounding::same(4.0),
                egui::Stroke::new(1.0, egui::Color32::LIGHT_BLUE),
            );
        }

        if response.clicked() {
            ui.memory_mut(|mem| mem.toggle_popup(egui::Id::new("auth_popup")));
        }

        self.render_auth_popup(ui, &response);
    }

    //渲染授權彈窗
    fn render_auth_popup(&mut self, ui: &mut egui::Ui, response: &egui::Response) {
        egui::popup::popup_below_widget(ui, egui::Id::new("auth_popup"), response, |ui| {
            ui.set_min_width(200.0);

            // Spotify 授權部分
            if self.spotify_authorized.load(Ordering::SeqCst) {
                let user_name = self.spotify_user_name.lock().unwrap().clone();
                if let Some(user_name) = user_name {
                    let button_text = format!("{} (登出)", user_name);
                    if self
                        .create_auth_button(ui, &button_text, "spotify_icon_black.png")
                        .clicked()
                    {
                        self.logout_spotify();
                        ui.close_menu();
                    }
                }
            } else {
                let current_status = self.auth_manager.get_status(&AuthPlatform::Spotify);
                match current_status {
                    AuthStatus::NotStarted | AuthStatus::Failed(_) => {
                        if self
                            .create_auth_button(ui, "Spotify 授權", "spotify_icon_black.png")
                            .clicked()
                        {
                            info!("Spotify 授權按鈕被點擊了！");
                            let ctx = ui.ctx().clone();
                            self.start_spotify_authorization(ctx);
                        }
                    }
                    AuthStatus::WaitingForBrowser
                    | AuthStatus::Processing
                    | AuthStatus::TokenObtained => {
                        let button = egui::Button::new(egui::RichText::new("授權中...").size(16.0))
                            .min_size(egui::vec2(200.0, 40.0));
                        let response = ui.add(button);

                        if response.clicked() {
                            self.cancel_authorization();
                        }

                        if let Some(start_time) = self.auth_start_time {
                            let elapsed = start_time.elapsed();
                            let remaining = Duration::from_secs(30)
                                .checked_sub(elapsed)
                                .unwrap_or_default();
                            if remaining.as_secs() > 0 {
                                let progress = 1.0 - (remaining.as_secs() as f32 / 30.0);
                                let rect = response.rect;
                                let progress_rect = egui::Rect::from_min_size(
                                    rect.min,
                                    egui::vec2(rect.width() * progress, rect.height()),
                                );
                                ui.painter().rect_filled(
                                    progress_rect,
                                    0.0,
                                    egui::Color32::from_rgba_premultiplied(0, 255, 0, 100),
                                );
                                ui.painter().text(
                                    rect.center(),
                                    egui::Align2::CENTER_CENTER,
                                    format!("授權中...{}秒", remaining.as_secs()),
                                    egui::FontId::default(),
                                    egui::Color32::BLACK,
                                );
                            } else {
                                self.cancel_authorization();
                            }
                        }
                    }
                    AuthStatus::Completed => {
                        ui.label("Spotify 授權成功！");
                        if ui.button("關閉").clicked() {
                            ui.close_menu();
                        }
                    }
                }
            }

            ui.add_space(5.0);

            // Osu 授權部分
            if self
                .create_auth_button(ui, "Osu 授權", "osu!logo.png")
                .clicked()
            {
                info!("Osu 授權按鈕被點擊了！");
                // TODO: 實現 Osu 授權邏輯
                ui.close_menu();
            }
        });
    }

    fn logout_spotify(&mut self) {
        info!("用戶登出 Spotify");
        self.spotify_authorized.store(false, Ordering::SeqCst);
        *self.spotify_user_avatar.lock().unwrap() = None;
        *self.spotify_user_name.lock().unwrap() = None;
        *self.spotify_user_avatar_url.lock().unwrap() = None;
        self.need_reload_avatar.store(true, Ordering::SeqCst);
        self.show_spotify_now_playing = false;
        self.should_detect_now_playing
            .store(false, Ordering::SeqCst);
        *self.currently_playing.lock().unwrap() = None;

        // 重置 Spotify 客戶端
        if let Ok(mut spotify_client) = self.spotify_client.try_lock() {
            *spotify_client = None;
        }

        // 重置授權管理器
        self.auth_manager.reset(&AuthPlatform::Spotify);
        self.auth_start_time = None;
        self.auth_in_progress.store(false, Ordering::SeqCst);
        self.show_auth_progress = false;
    }

    fn render_guest_user(&mut self, ui: &mut egui::Ui) {
        let button = egui::Button::new("")
            .fill(egui::Color32::TRANSPARENT)
            .min_size(egui::vec2(100.0, 40.0))
            .frame(false);

        let response = ui.add(button);

        if ui.is_rect_visible(response.rect) {
            if let Some(default_avatar) = &self.default_avatar_texture {
                let image_rect = egui::Rect::from_min_size(
                    response.rect.right_top() + egui::vec2(-36.0, 4.0),
                    egui::vec2(32.0, 32.0),
                );
                ui.painter().image(
                    default_avatar.id(),
                    image_rect,
                    egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                    egui::Color32::WHITE,
                );
            }

            let text_pos = response.rect.left_center() + egui::vec2(5.0, 0.0);
            ui.painter().text(
                text_pos,
                egui::Align2::LEFT_CENTER,
                "Guest",
                egui::FontId::new(16.0, egui::FontFamily::Proportional),
                egui::Color32::BLACK,
            );
        }

        if response.hovered() {
            ui.painter().rect_stroke(
                response.rect,
                egui::Rounding::same(4.0),
                egui::Stroke::new(1.0, egui::Color32::LIGHT_BLUE),
            );
        }

        if response.clicked() {
            ui.memory_mut(|mem| mem.toggle_popup(egui::Id::new("auth_popup")));
        }

        self.render_auth_popup(ui, &response);
    }

    fn create_auth_button(&self, ui: &mut egui::Ui, text: &str, icon_path: &str) -> egui::Response {
        let button_padding = egui::vec2(8.0, 4.0);
        let icon_size = egui::vec2(24.0, 24.0);
        let spacing = 8.0;

        let total_width = 200.0;
        let total_height = 40.0;

        let (rect, response) =
            ui.allocate_exact_size(egui::vec2(total_width, total_height), egui::Sense::click());

        if ui.is_rect_visible(rect) {
            let visuals = ui.style().interact(&response);

            // 繪製按鈕背景
            ui.painter()
                .rect_filled(rect, visuals.rounding, visuals.bg_fill);

            let mut content_rect = rect.shrink2(button_padding);

            // 繪製圖標（如果有）
            if let Some(texture) = self.preloaded_icons.get(icon_path) {
                let icon_rect = egui::Rect::from_min_size(content_rect.min, icon_size);
                ui.painter().image(
                    texture.id(),
                    icon_rect,
                    egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                    egui::Color32::WHITE, // 使用白色以確保圖標可見
                );
                content_rect.min.x += icon_size.x + spacing;
            }

            // 繪製文字
            let galley = ui.painter().layout_no_wrap(
                text.to_owned(),
                egui::FontId::new(16.0, egui::FontFamily::Proportional),
                visuals.text_color(),
            );
            let text_pos = content_rect.left_center() - egui::vec2(0.0, galley.size().y / 2.0);
            ui.painter().galley(text_pos, galley, visuals.text_color());

            // 為了調試，繪製邊框
            ui.painter()
                .rect_stroke(rect, visuals.rounding, visuals.fg_stroke);
        }

        response
    }

    fn load_icon(ctx: &egui::Context, icon_path: &str) -> Option<egui::TextureHandle> {
        let icon_bytes: &[u8] = match icon_path {
            "spotify_icon_black.png" => {
                info!("嘗試加載 Spotify 圖標");
                include_bytes!("assets/spotify_icon_black.png")
            }
            "osu!logo.png" => {
                info!("嘗試加載 Osu 圖標");
                include_bytes!("assets/osu!logo.png")
            }
            _ => {
                error!("未知的圖標路徑: {}", icon_path);
                return None;
            }
        };

        match image::load_from_memory(icon_bytes) {
            Ok(image) => {
                let image = image.to_rgba8();
                let size = [image.width() as _, image.height() as _];
                let pixels = image.as_flat_samples();
                let color_image = egui::ColorImage::from_rgba_unmultiplied(size, pixels.as_slice());

                info!("成功加載圖標: {}", icon_path);
                Some(ctx.load_texture(icon_path, color_image, egui::TextureOptions::default()))
            }
            Err(e) => {
                error!("無法加載圖標 {}: {:?}", icon_path, e);
                None
            }
        }
    }

    // 渲染中央面板
    fn render_central_panel(&mut self, ctx: &egui::Context) {
        egui::CentralPanel::default().show(ctx, |ui| {
            ui.set_max_width(ui.available_width());
            ui.set_max_height(ui.available_height());

            let window_size = ui.available_size();

            // 使用 egui 的緩存機制來減少重繪
            let window_size_changed = ui
                .memory_mut(|mem| {
                    mem.data
                        .get_temp::<egui::Vec2>(egui::Id::new("window_size"))
                })
                .map_or(true, |old_size| old_size != window_size);

            if window_size_changed {
                ui.memory_mut(|mem| {
                    mem.data
                        .insert_temp(egui::Id::new("window_size"), window_size)
                });

                ui.horizontal(|ui| {
                    if ui.button("?").clicked() {
                        self.show_settings = !self.show_settings;
                    }
                    ui.label(format!(
                        "視窗大小: {} x {}",
                        window_size.x as i32, window_size.y as i32
                    ));
                    ui.heading(egui::RichText::new("搜尋歌曲:").size(self.global_font_size * 1.3));
                    ui.add_space(5.0);
                });
            }

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

                        // 檢測Enter是否按下，並處理調試模式
                        if text_edit_response.lost_focus()
                            && ui.input(|i| i.key_pressed(egui::Key::Enter))
                        {
                            if self.search_query.trim().to_lowercase() == "debug" {
                                self.debug_mode = !self.debug_mode;
                                self.search_query.clear();
                            } else {
                                self.perform_search(ctx.clone());
                            }
                        }

                        // 顯示調試模式狀態
                        if self.debug_mode {
                            ui.add_space(5.0);
                            ui.label(
                                egui::RichText::new("調試模式開啟")
                                    .color(egui::Color32::YELLOW)
                                    .size(self.global_font_size),
                            );
                        }
                    });
                });
            });

            // 使用緩存來減少不必要的樣式更新
            if ui
                .memory_mut(|mem| mem.data.get_temp::<f32>(egui::Id::new("global_font_size")))
                .map_or(true, |old_size| old_size != self.global_font_size)
            {
                ui.memory_mut(|mem| {
                    mem.data
                        .insert_temp(egui::Id::new("global_font_size"), self.global_font_size)
                });

                let text_style = egui::TextStyle::Body.resolve(ui.style());
                let mut new_text_style = text_style.clone();
                new_text_style.size = self.global_font_size;
                ui.style_mut()
                    .text_styles
                    .insert(egui::TextStyle::Body, new_text_style);
            }

            if let Ok(err_msg_guard) = self.err_msg.try_lock() {
                if !err_msg_guard.is_empty() {
                    ui.label(format!("{}", *err_msg_guard));
                }
            }

            // 根據視窗大小決定佈局
            if window_size.x >= 1000.0 {
                // 大視窗佈局
                ui.columns(2, |columns| {
                    // Spotify 結果
                    columns[0].vertical(|ui| {
                        ui.set_min_width(0.45 * window_size.x);
                        self.display_spotify_results(ui, window_size);
                    });

                    // Osu 結果
                    columns[1].vertical(|ui| {
                        ui.set_min_width(0.45 * window_size.x);
                        self.display_osu_results(ui, window_size);
                    });
                });
            } else {
                // 小視窗佈局（折疊式）
                egui::CollapsingHeader::new(
                    egui::RichText::new("Spotify 結果").size(self.global_font_size * 1.1),
                )
                .default_open(true)
                .show(ui, |ui| {
                    self.display_spotify_results(ui, window_size);
                });

                egui::CollapsingHeader::new(
                    egui::RichText::new("Osu 結果").size(self.global_font_size * 1.1),
                )
                .default_open(true)
                .show(ui, |ui| {
                    self.display_osu_results(ui, window_size);
                });
            }
        });
    }

    async fn load_spotify_avatar(
        ctx: &egui::Context,
        url: &str,
        spotify_user_avatar: Arc<Mutex<Option<egui::TextureHandle>>>,
        need_reload_avatar: Arc<AtomicBool>,
    ) -> Result<(), Box<dyn std::error::Error>> {
        if need_reload_avatar.load(Ordering::SeqCst) {
            info!("開始加載 Spotify 用戶頭像: {}", url);
            match Self::load_spotify_user_avatar(url, ctx).await {
                Ok(texture) => {
                    info!("Spotify 用戶頭像加載成功");
                    let mut avatar = spotify_user_avatar.lock().unwrap();
                    *avatar = Some(texture);
                    need_reload_avatar.store(false, Ordering::SeqCst);
                    ctx.request_repaint_after(std::time::Duration::from_secs(1));
                    Ok(())
                }
                Err(e) => {
                    error!("加載 Spotify 用戶頭像失敗: {:?}", e);
                    Err(e)
                }
            }
        } else {
            Ok(())
        }
    }

    async fn load_spotify_user_avatar(
        url: &str,
        ctx: &egui::Context,
    ) -> Result<egui::TextureHandle, Box<dyn std::error::Error>> {
        info!("開始從 URL 加載 Spotify 用戶頭像: {}", url);
        let client = reqwest::Client::new();
        let response = client.get(url).send().await?;
        info!("成功獲取頭像數據");
        let bytes = response.bytes().await?;
        info!("成功讀取頭像字節數據");
        let image = image::load_from_memory(&bytes)?;
        let size = [image.width() as _, image.height() as _];
        let image_buffer = image.to_rgba8();
        let pixels = image_buffer.as_flat_samples();
        let color_image = egui::ColorImage::from_rgba_unmultiplied(size, pixels.as_slice());

        let texture = ctx.load_texture(
            "spotify_user_avatar",
            color_image,
            egui::TextureOptions::default(),
        );

        info!("成功將頭像加載到 egui 上下文中");
        Ok(texture)
    }
}

#[tokio::main]
async fn main() -> Result<(), AppError> {
    // 初始化日誌
    let log_file = std::fs::File::create("output.log").context("Failed to create log file")?;
    let mut config_builder = simplelog::ConfigBuilder::new();
    if let Err(err) = config_builder.set_time_offset_to_local() {
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
    .context("Failed to initialize logger")?;

    info!("Welcome");

    // 讀取配置
    let config_errors = Arc::new(Mutex::new(Vec::new()));

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
        resizable: Some(true),
        maximize_button: Some(true),
        ..Default::default()
    };

    // 運行應用
    eframe::run_native(
        "Search App",
        native_options,
        Box::new(move |cc| {
            let ctx = cc.egui_ctx.clone();
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
                    Box::new(ErrorApp::new(e.to_string()))
                }
            }
        }),
    )
    .map_err(|e| anyhow::anyhow!("Failed to run eframe: {}", e))?;

    Ok(())
}
struct ErrorApp {
    error: String,
    font_size: f32,
}

impl ErrorApp {
    fn new(error: String) -> Self {
        Self {
            error,
            font_size: 24.0,
        }
    }
}

impl eframe::App for ErrorApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // 設置字體
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

        // 顯示錯誤訊息
        egui::CentralPanel::default().show(ctx, |ui| {
            ui.vertical_centered(|ui| {
                ui.add_space(ui.available_height() / 3.0);
                ui.heading(
                    egui::RichText::new("錯誤")
                        .size(self.font_size * 1.2)
                        .color(egui::Color32::RED),
                );
                ui.label(egui::RichText::new(&self.error).size(self.font_size));
            });
        });
    }
}
